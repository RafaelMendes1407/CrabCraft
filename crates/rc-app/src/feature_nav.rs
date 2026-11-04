//! Going between Cargo features and their `cfg` uses (N-13), which rust-analyzer does not do.
//!
//! In a `Cargo.toml`, Go to Declaration (⌃B, ⌃+click) or Find Usages on a `[features]` key lists
//! the `cfg` predicates of the package's sources that name it; on a feature another entry turns on
//! (`x = ["y"]`) it jumps to `y`'s line. In a Rust file, the same on `"x"` of `feature = "x"`
//! jumps to the feature's line of the owning package's `Cargo.toml`. The analysis is
//! `rc_cargo::feature_nav`; the package's sources are scanned on a background thread that a new
//! search cancels.

use crate::app::CrabCraftApp;
use crate::docs::Doc;
use crate::insight::Target;
use rc_cargo::feature_nav::{self, FeatureUse, ManifestFeature};
use rc_core::{Inbox, Waker};
use rc_kit::i18n::{tr, trf};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Larger manifests are not read.
const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;

struct Job {
    feature: String,
    cancel: Arc<AtomicBool>,
    inbox: Inbox<Option<Vec<FeatureUse>>>,
}

/// The running scan for the uses of a feature, if any.
#[derive(Default)]
pub struct FeatureSearch {
    job: Option<Job>,
}

impl FeatureSearch {
    /// Scans the `.rs` files of the package in `dir` for `feature`, cancelling the previous scan.
    /// `open` is the text of the documents open in the editor.
    pub fn start(&mut self, waker: Waker, dir: PathBuf, feature: String, open: HashMap<PathBuf, String>) {
        self.cancel();
        let cancel = Arc::new(AtomicBool::new(false));
        let inbox = Inbox::new(waker);
        let (tx, stop, name) = (inbox.sender(), cancel.clone(), feature.clone());
        std::thread::spawn(move || {
            tx.send(feature_nav::find_feature_uses(&dir, &name, &open, &stop));
        });
        self.job = Some(Job { feature, cancel, inbox });
    }

    pub fn cancel(&mut self) {
        if let Some(job) = self.job.take() {
            job.cancel.store(true, Ordering::Relaxed);
        }
    }

    /// Whether a scan is running (tests).
    #[cfg(test)]
    pub fn busy(&self) -> bool {
        self.job.is_some()
    }

    /// The finished scan: the feature and its uses (`None` when it was cancelled).
    fn poll(&mut self) -> Option<(String, Option<Vec<FeatureUse>>)> {
        let found = self.job.as_ref()?.inbox.drain().into_iter().next()?;
        let job = self.job.take()?;
        Some((job.feature, found))
    }
}

fn is_manifest(path: &Path) -> bool {
    path.file_name().is_some_and(|n| n == "Cargo.toml")
}

/// The feature of the `cfg` predicate string at `pos` of a Rust document.
fn cfg_feature_at(doc: &Doc, pos: usize) -> Option<String> {
    let rope = doc.editor.buffer.rope();
    if doc.editor.lang() != rc_syntax::Lang::Rust || pos > rope.len_chars() {
        return None;
    }
    // Lexing the file is cheap, but only near a line that names a feature is it worth it.
    let line = rope.char_to_line(pos);
    let last = rope.len_lines().saturating_sub(1);
    if !(line.saturating_sub(1)..=(line + 1).min(last)).any(|l| rope.line(l).to_string().contains("feature")) {
        return None;
    }
    feature_nav::cfg_feature_at(&rope.to_string(), pos)
}

/// Whether ⌃+click at `pos` is handled here rather than by the language server: anywhere in a
/// `Cargo.toml`, and on the feature string of a `cfg` predicate.
pub fn handles_click(doc: &Doc, pos: usize) -> bool {
    is_manifest(&doc.path) || cfg_feature_at(doc, pos).is_some()
}

impl CrabCraftApp {
    /// Go to Declaration (`usages` false) or Find Usages on a feature name at `pos` of document
    /// `i`. False when there is no feature name there, so other navigation can run.
    pub fn feature_navigate(&mut self, i: usize, pos: usize, usages: bool) -> bool {
        let Some(doc) = self.docs.list.get(i) else { return false };
        let path = doc.path.clone();
        let rope = doc.editor.buffer.rope();
        if is_manifest(&path) {
            let text = rope.to_string();
            let (name, jump) = match feature_nav::manifest_feature_at(&text, pos) {
                Some(ManifestFeature::Declared(name)) => (name, false),
                Some(ManifestFeature::Enabled(name)) => (name, !usages),
                None => return false,
            };
            if !jump {
                self.find_feature_uses(&path, name);
                return true;
            }
            match feature_nav::feature_declaration(&text, &name) {
                Some((line, col)) => {
                    self.push_nav();
                    self.open_at(&path, line, col);
                }
                None => self.notify(trf("The feature {name} is not declared in this Cargo.toml", &[("name", &name)]), false),
            }
            return true;
        }
        let Some(name) = cfg_feature_at(doc, pos) else { return false };
        let Some(manifest) = path.parent().and_then(rc_cargo::find_manifest) else {
            self.notify(tr("This file does not belong to a Cargo package"), false);
            return true;
        };
        if usages {
            self.find_feature_uses(&manifest, name);
            return true;
        }
        let text = match self.docs.list.iter().find(|d| d.path == manifest) {
            Some(d) => Some(d.editor.buffer.rope().to_string()),
            None => std::fs::metadata(&manifest).ok().filter(|m| m.len() <= MAX_MANIFEST_BYTES).and_then(|_| std::fs::read_to_string(&manifest).ok()),
        };
        match text.and_then(|t| feature_nav::feature_declaration(&t, &name)) {
            Some((line, col)) => {
                self.push_nav();
                self.open_at(&manifest, line, col);
            }
            None => self.notify(trf("The feature {name} is not declared in this package's Cargo.toml", &[("name", &name)]), false),
        }
        true
    }

    /// Starts listing the uses of `feature` in the package of `manifest`.
    fn find_feature_uses(&mut self, manifest: &Path, feature: String) {
        let Some(dir) = manifest.parent() else { return };
        let open = self.docs.list.iter().filter(|d| d.path.extension().is_some_and(|e| e == "rs")).map(|d| (d.path.clone(), d.editor.buffer.rope().to_string())).collect();
        self.feature_search.start(self.waker.clone(), dir.to_path_buf(), feature, open);
    }

    /// Shows the uses of a feature in the Find window once the scan has finished.
    pub fn poll_feature_search(&mut self) {
        let Some((name, found)) = self.feature_search.poll() else { return };
        let Some(uses) = found else { return };
        let title = trf("Uses of feature {name}", &[("name", &name)]);
        let targets: Vec<Target> = uses.into_iter().map(|u| Target { path: u.path, line: u.line, col: u.col, preview: u.preview }).collect();
        if targets.is_empty() {
            self.notify(trf("No cfg uses of the feature {name} found in the package's sources", &[("name", &name)]), false);
            // Like Find Usages: an empty answer still replaces the list of another symbol.
            if self.find_results.is_some() {
                self.find_results = Some((title, targets));
            }
            return;
        }
        self.find_results = Some((title, targets));
        self.dock.show(crate::toolwindow::ToolWindow::Find);
    }
}
