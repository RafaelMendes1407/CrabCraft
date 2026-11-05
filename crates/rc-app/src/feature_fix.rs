//! Quick fixes that turn on a Cargo feature (C-10).
//!
//! When `cargo check` (run by rust-analyzer on save, in trusted projects only) says the name or
//! import under the caret is hidden behind a feature, Alt+Enter offers to turn it on: for a
//! feature of the file's own package, in the IDE's analysis (the C-07 set, given to
//! rust-analyzer) or in the package's `default`; for a feature of a dependency, in that
//! dependency's `features = [...]` of the package's `Cargo.toml`. Manifest edits are made in the
//! manifest's editor tab, where Undo takes them back; nothing is saved for the user. The gates come
//! from `rc_cargo::feature_fix`; working them out reads at most the package's manifest.

use crate::app::{CrabCraftApp, FeatureState};
use rc_cargo::feature_fix::{self, FeatureGate, TextEdit};
use rc_kit::i18n::{tr, trf};
use rc_lsp::lsp_types as lsp;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The command of a feature quick fix (argument: the [`FeatureFix`]), run by the IDE.
pub const ENABLE_FEATURE_COMMAND: &str = "crabcraft.enable-feature";

/// Larger manifests are not read.
const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;

/// One way to turn a feature on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum FeatureFix {
    /// Turn on `feature` of the workspace package `package` for the analysis.
    Analysis { package: String, feature: String },
    /// Add `feature` to `default` in `manifest`.
    Default { manifest: PathBuf, package: String, feature: String },
    /// Add `feature` to the `features` of the dependency declared as `key` in `manifest`.
    Dependency { manifest: PathBuf, key: String, name: String, feature: String },
}

impl FeatureFix {
    pub fn title(&self) -> String {
        match self {
            FeatureFix::Analysis { feature, .. } => trf("Turn on feature `{feature}` for code analysis", &[("feature", feature)]),
            FeatureFix::Default { package, feature, .. } => trf("Add `{feature}` to the default features of {package}", &[("feature", feature), ("package", package)]),
            FeatureFix::Dependency { name, feature, .. } => trf("Add feature `{feature}` to the {name} dependency", &[("feature", feature), ("name", name)]),
        }
    }

    /// The `Cargo.toml` the fix edits (`None`: it changes the analysis settings only).
    pub fn manifest(&self) -> Option<&Path> {
        match self {
            FeatureFix::Analysis { .. } => None,
            FeatureFix::Default { manifest, .. } | FeatureFix::Dependency { manifest, .. } => Some(manifest),
        }
    }

    /// The edit the fix makes in the text of its manifest (`None`: already there, or no place).
    pub fn manifest_edit(&self, text: &str) -> Option<TextEdit> {
        match self {
            FeatureFix::Analysis { .. } => None,
            FeatureFix::Default { feature, .. } => feature_fix::add_default_feature(text, feature),
            FeatureFix::Dependency { key, feature, .. } => {
                let dep = rc_cargo::dependency_lines(text).into_iter().find(|d| d.key == *key && !d.in_workspace)?;
                feature_fix::add_dependency_feature(text, &dep, feature)
            }
        }
    }

    pub fn action(&self) -> lsp::CodeActionOrCommand {
        let command = lsp::Command { title: self.title(), command: ENABLE_FEATURE_COMMAND.to_string(), arguments: Some(vec![serde_json::to_value(self).unwrap_or_default()]) };
        lsp::CodeActionOrCommand::CodeAction(lsp::CodeAction { title: command.title.clone(), kind: Some(lsp::CodeActionKind::QUICKFIX), command: Some(command), ..Default::default() })
    }
}

/// The text of a manifest: its editor's when it is open, else the file's.
fn manifest_text(app: &CrabCraftApp, manifest: &Path) -> Option<String> {
    if let Some(d) = app.docs.list.iter().find(|d| d.path == manifest) {
        return Some(d.editor.buffer.rope().to_string());
    }
    std::fs::metadata(manifest).ok().filter(|m| m.len() <= MAX_MANIFEST_BYTES)?;
    std::fs::read_to_string(manifest).ok()
}

impl CrabCraftApp {
    /// The feature fixes for the diagnostics on the caret line of `docs.list[i]`.
    pub fn feature_fixes(&self, i: usize) -> Vec<FeatureFix> {
        let Some(doc) = self.docs.list.get(i) else { return Vec::new() };
        let Some(known) = self.insight.feature_gates.get(&doc.path) else { return Vec::new() };
        let line = doc.editor.caret_line_col().0;
        let on_line: Vec<&str> = doc.editor.diagnostics.iter().filter(|d| doc.editor.buffer.line_col(d.start).0 <= line && line <= doc.editor.buffer.line_col(d.end).0).map(|d| d.message.as_str()).collect();
        let gates: Vec<&FeatureGate> = known.iter().filter(|(m, _)| on_line.contains(&m.as_str())).flat_map(|(_, g)| g).collect();
        if gates.is_empty() {
            return Vec::new();
        }
        let workspaces: Vec<&rc_cargo::CargoWorkspace> = self.runner.cargo_workspace.iter().chain(&self.runner.attached).collect();
        let Some((root, current)) = workspaces.iter().find_map(|w| w.package_of(&doc.path).map(|p| (w.root.clone(), p.clone()))) else { return Vec::new() };
        let text = manifest_text(self, &current.manifest);
        let mut out = Vec::new();
        for gate in gates {
            let file = if gate.file.is_absolute() { gate.file.clone() } else { root.join(&gate.file) };
            let owner = workspaces.iter().find_map(|w| w.package_of(&file));
            if owner.is_some_and(|o| o.manifest == current.manifest) {
                if self.feature_state(&current, &gate.feature) != Some(FeatureState::Off) {
                    continue;
                }
                out.push(FeatureFix::Analysis { package: current.name.clone(), feature: gate.feature.clone() });
                if text.as_deref().and_then(|t| feature_fix::add_default_feature(t, &gate.feature)).is_some() {
                    out.push(FeatureFix::Default { manifest: current.manifest.clone(), package: current.name.clone(), feature: gate.feature.clone() });
                }
                continue;
            }
            // Another package: a member, a downloaded library or a crate of the registry cache.
            let name = owner.map(|p| p.name.clone()).or_else(|| self.explorer.libraries.iter().filter(|l| file.starts_with(&l.root)).max_by_key(|l| l.root.components().count()).map(|l| l.name.clone())).or_else(|| feature_fix::registry_crate_of(&file));
            let (Some(name), Some(text)) = (name, text.as_deref()) else { continue };
            let dep = rc_cargo::dependency_lines(text).into_iter().find(|d| !d.in_workspace && d.name == name && feature_fix::add_dependency_feature(text, d, &gate.feature).is_some());
            if let Some(dep) = dep {
                out.push(FeatureFix::Dependency { manifest: current.manifest.clone(), key: dep.key, name, feature: gate.feature.clone() });
            }
        }
        out.dedup();
        out
    }

    /// Runs a feature fix chosen in the Alt+Enter popup.
    pub fn apply_feature_fix(&mut self, fix: FeatureFix) {
        match fix {
            FeatureFix::Analysis { package, feature } => {
                if !rc_cargo::valid_feature_entry(&format!("{package}/{feature}")) {
                    return;
                }
                self.set_cargo_feature(&package, &feature, true);
                self.notify(trf("Feature {feature} of {package} is on for code analysis; turn it off in the gutter of Cargo.toml", &[("feature", &feature), ("package", &package)]), false);
            }
            FeatureFix::Default { ref manifest, .. } | FeatureFix::Dependency { ref manifest, .. } => {
                let manifest = manifest.clone();
                self.edit_manifest(&manifest, |text| fix.manifest_edit(text));
            }
        }
    }

    /// Opens `manifest` and makes the edit `edit` finds in its current text, undoably.
    fn edit_manifest(&mut self, manifest: &Path, edit: impl FnOnce(&str) -> Option<TextEdit>) {
        if manifest.file_name().is_none_or(|n| n != "Cargo.toml") {
            return;
        }
        self.open_file_sync(manifest);
        let Some(doc) = self.docs.active_doc_mut().filter(|d| d.path == manifest) else { return };
        let text = doc.editor.buffer.rope().to_string();
        match edit(&text) {
            Some(e) => {
                doc.editor.replace(e.start, e.end, &e.text);
                doc.editor.selection = rc_text::Selection::point(e.start + e.text.chars().count());
            }
            None => self.notify(tr("The feature is already turned on in Cargo.toml"), false),
        }
    }
}
