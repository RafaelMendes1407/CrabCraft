//! Offline inspections of the open `Cargo.toml` (I-22): unknown keys, a dev dependency whose
//! version is incompatible with the regular one, dependencies without a source, features that
//! name what does not exist, unknown editions and workspace members or path dependencies with
//! no package. The checks themselves are in `rc_cargo::inspect`.
//!
//! The manifest text is inspected on a background thread (it also looks at member and path
//! directories on disk), once per text version and Cargo epoch; the UI only turns the answer
//! into diagnostics and quick fixes. The diagnostics go to the editor and to the Problems window
//! next to the crates.io ones (C-04), with their own `code`.

use crate::dep_versions::{merge_into_editor, merge_into_problems};
use crate::docs::Docs;
use rc_cargo::inspect::{Fix, FixKind, Issue, Level, Problem};
use rc_core::{Inbox, Waker};
use rc_editor::{Diagnostic, Severity};
use rc_kit::i18n::trf;
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

/// The `code` of the diagnostics made here.
pub const DIAG_CODE: &str = "Cargo.toml";

/// The text version and Cargo epoch an inspection was made for.
type Key = (u64, u64);

struct Done {
    key: Key,
    diags: Vec<Diagnostic>,
    fixes: Vec<Fix>,
}

pub struct ManifestInspections {
    waker: Waker,
    /// The inspection running for each editor.
    running: HashMap<u64, (Key, Inbox<Vec<Issue>>)>,
    /// The last finished inspection of each editor.
    done: HashMap<u64, Done>,
}

impl ManifestInspections {
    pub fn new(waker: Waker) -> Self {
        ManifestInspections { waker, running: HashMap::new(), done: HashMap::new() }
    }

    /// Whether an inspection is still running (tests).
    #[cfg(test)]
    pub fn busy(&self) -> bool {
        !self.running.is_empty()
    }

    /// Takes the finished inspections and, for the active `Cargo.toml`, starts one when its text
    /// or the Cargo project changed, and puts the current diagnostics in the editor and in
    /// `problems`.
    pub fn update(&mut self, docs: &mut Docs, problems: &mut BTreeMap<PathBuf, Vec<Diagnostic>>, epoch: u64) {
        let finished: Vec<(u64, Key, Vec<Issue>)> = self.running.iter().filter_map(|(id, (key, inbox))| inbox.drain().pop().map(|issues| (*id, *key, issues))).collect();
        for (id, key, issues) in finished {
            self.running.remove(&id);
            let diags = issues.iter().map(diagnostic).collect();
            let fixes = issues.into_iter().filter_map(|i| i.fix).collect();
            self.done.insert(id, Done { key, diags, fixes });
        }
        let Some(i) = docs.active else { return };
        if !crate::manifest_completion::is_manifest(&docs.list[i]) {
            return;
        }
        let ids: Vec<u64> = docs.list.iter().map(|d| d.editor.id).collect();
        self.done.retain(|id, _| ids.contains(id));
        let doc = &mut docs.list[i];
        let id = doc.editor.id;
        let key = (doc.editor.buffer.version(), epoch);
        let current = self.done.get(&id).is_some_and(|d| d.key == key);
        if !current && !self.running.contains_key(&id) {
            let text = doc.editor.buffer.rope().to_string();
            let dir = doc.path.parent().map(PathBuf::from);
            let inbox = Inbox::new(self.waker.clone());
            let out = inbox.sender();
            std::thread::spawn(move || {
                out.send(rc_cargo::inspect::inspect(&text, dir.as_deref()));
            });
            self.running.insert(id, (key, inbox));
        }
        // Diagnostics of an older text stay as the editor moved them until the new ones come.
        if let Some(done) = self.done.get(&id).filter(|d| d.key.0 == key.0) {
            merge_into_editor(&mut doc.editor, DIAG_CODE, &done.diags);
            merge_into_problems(problems, &doc.path, DIAG_CODE, &done.diags);
        }
    }

    /// The quick fixes whose range touches `line` of the editor's current text.
    pub fn fixes_on_line(&self, doc: &crate::docs::Doc, line: usize) -> Vec<Fix> {
        let editor = &doc.editor;
        let Some(done) = self.done.get(&editor.id).filter(|d| d.key.0 == editor.buffer.version()) else { return Vec::new() };
        let rope = editor.buffer.rope();
        let len = rope.len_chars();
        done.fixes.iter().filter(|f| f.start <= f.end && f.end <= len && rope.char_to_line(f.start) <= line && line <= rope.char_to_line(f.end)).cloned().collect()
    }
}

/// The title of a quick fix.
pub fn fix_title(fix: &Fix) -> String {
    match &fix.kind {
        FixKind::ChangeTo(text) => trf("Change to `{text}`", &[("text", text)]),
        FixKind::MakeOptional(name) => trf("Make `{name}` optional", &[("name", name)]),
    }
}

fn diagnostic(issue: &Issue) -> Diagnostic {
    let severity = match issue.level {
        Level::Error => Severity::Error,
        Level::Warning => Severity::Warning,
    };
    Diagnostic { start: issue.start, end: issue.end, severity, message: message(&issue.problem), code: Some(DIAG_CODE.to_string()), unnecessary: false, deprecated: false }
}

/// What the user reads for `problem`.
pub fn message(problem: &Problem) -> String {
    match problem {
        Problem::UnknownKey { key, table, suggestion } => match (table.is_empty(), suggestion) {
            (true, None) => trf("Unknown top-level key `{key}`; Cargo ignores it", &[("key", key)]),
            (true, Some(s)) => trf("Unknown top-level key `{key}`; did you mean `{suggestion}`?", &[("key", key), ("suggestion", s)]),
            (false, None) => trf("Unknown key `{key}` in `[{table}]`; Cargo ignores it", &[("key", key), ("table", table)]),
            (false, Some(s)) => trf("Unknown key `{key}` in `[{table}]`; did you mean `{suggestion}`?", &[("key", key), ("table", table), ("suggestion", s)]),
        },
        Problem::DuplicateDependency { name, other } => trf("`{name}` is also in `[dependencies]` with the incompatible requirement `{req}`", &[("name", name), ("req", other)]),
        Problem::NoSource { name } => trf("Dependency `{name}` has no version, path, git or workspace source", &[("name", name)]),
        Problem::UnknownFeature { feature, value, suggestion: None } => trf("Feature `{feature}` includes `{value}`, which is neither a feature nor an optional dependency", &[("feature", feature), ("value", value)]),
        Problem::UnknownFeature { feature, value, suggestion: Some(s) } => {
            trf("Feature `{feature}` includes `{value}`, which is neither a feature nor an optional dependency; did you mean `{suggestion}`?", &[("feature", feature), ("value", value), ("suggestion", s)])
        }
        Problem::NotADependency { feature, value, dep } => trf("Feature `{feature}` includes `{value}`, but `{dep}` is not a dependency", &[("feature", feature), ("value", value), ("dep", dep)]),
        Problem::NotOptional { feature, value, dep } => trf("Feature `{feature}` includes `{value}`, but `{dep}` is not an optional dependency", &[("feature", feature), ("value", value), ("dep", dep)]),
        Problem::NoImplicitFeature { feature, dep } => trf("Feature `{feature}` includes `{dep}`, which has no implicit feature because `dep:{dep}` is used; write `dep:{dep}`", &[("feature", feature), ("dep", dep)]),
        Problem::InvalidEdition { value } => trf("Unknown edition `{value}`; expected 2015, 2018, 2021 or 2024", &[("value", value)]),
        Problem::EditionNotString => trf("The edition must be a string, such as `{example}`", &[("example", &"\"2021\"")]),
        Problem::MissingMember { path } => trf("No package at workspace member `{path}`", &[("path", path)]),
        Problem::MissingPathDependency { name, path } => trf("No package at `{path}` for dependency `{name}`", &[("path", path), ("name", name)]),
    }
}
