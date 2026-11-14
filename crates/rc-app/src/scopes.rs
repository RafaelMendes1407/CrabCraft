//! Scopes and file colours (N-15): named sets of files — built-in ones (Project Files, Tests,
//! Changed Files) and the project's own, defined by globs and saved with its session. Find in
//! Files searches one of them; a file colour paints the files of a scope in the explorer and on
//! the editor tabs. Matching is cached per path, so painting a frame does not match globs again.

use rc_core::session::{BUILTIN_SCOPE_PREFIX, FileColor, NamedScope, clean_file_colors, clean_scopes, parse_hex_color};
use rc_kit::i18n::{tr, trf};
use rc_search::{FileMask, ScopeFilter};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Every file of the project (what the explorer shows, without the excluded folders).
pub const PROJECT: &str = "builtin:project";
/// Rust tests: `tests/` folders, `tests.rs` modules and `*_test(s).rs` files.
pub const TESTS: &str = "builtin:tests";
/// The files git reports as changed (the Commit window's list).
pub const CHANGED: &str = "builtin:changed";
pub const TESTS_PATTERN: &str = "tests/**, **/tests/**, tests.rs, *_test.rs, *_tests.rs";
/// Colour of a new file colour (green, like test files).
pub const DEFAULT_COLOR: [u8; 3] = [0x5f, 0xb8, 0x65];
/// Paths whose colour is remembered before the cache starts over.
const CACHE_LIMIT: usize = 20_000;

/// A scope whose pattern does not parse.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScopeError {
    pub scope: String,
    pub glob: String,
    pub message: String,
}

impl ScopeError {
    pub fn text(&self) -> String {
        trf("Scope \"{scope}\": invalid pattern '{glob}': {message}", &[("scope", &self.scope), ("glob", &self.glob), ("message", &self.message)])
    }
}

/// Parses a scope's pattern.
pub fn parse_pattern(name: &str, pattern: &str) -> Result<FileMask, ScopeError> {
    FileMask::parse(pattern).map_err(|e| ScopeError { scope: name.to_string(), glob: e.glob, message: e.message })
}

/// What Settings › Scopes edits before OK.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Draft {
    pub user: Vec<NamedScope>,
    pub colors: Vec<FileColor>,
}

/// The project's scopes and file colours.
pub struct Scopes {
    pub user: Vec<NamedScope>,
    pub colors: Vec<FileColor>,
    /// Settings › Scopes while it is open.
    pub draft: Option<Draft>,
    root: Option<PathBuf>,
    compiled: HashMap<String, Result<FileMask, ScopeError>>,
    tests: Option<FileMask>,
    changed: Arc<HashSet<String>>,
    changed_version: Option<u64>,
    /// Colours in the order they apply, parsed.
    rules: Vec<(String, egui::Color32)>,
    cache: HashMap<String, Option<egui::Color32>>,
}

impl Default for Scopes {
    fn default() -> Self {
        Scopes {
            user: Vec::new(),
            colors: Vec::new(),
            draft: None,
            root: None,
            compiled: HashMap::new(),
            tests: FileMask::parse(TESTS_PATTERN).ok(),
            changed: Arc::default(),
            changed_version: None,
            rules: Vec::new(),
            cache: HashMap::new(),
        }
    }
}

impl Scopes {
    /// The scopes of the project at `root`, from its (untrusted) session.
    pub fn open(&mut self, root: &Path, user: &[NamedScope], colors: &[FileColor]) {
        *self = Scopes { root: Some(root.to_path_buf()), ..Scopes::default() };
        self.set(user, colors);
    }

    pub fn close(&mut self) {
        *self = Scopes::default();
    }

    /// Replaces the user scopes and the file colours (cleaned: they may come from a file).
    pub fn set(&mut self, user: &[NamedScope], colors: &[FileColor]) {
        self.user = clean_scopes(user);
        self.compiled = self.user.iter().map(|s| (s.name.clone(), parse_pattern(&s.name, &s.pattern))).collect();
        let names: HashSet<&str> = self.user.iter().map(|s| s.name.as_str()).collect();
        self.colors = clean_file_colors(colors, |k| is_builtin(k) || names.contains(k));
        self.rules =
            self.colors.iter().filter_map(|c| parse_hex_color(&c.color).map(|[r, g, b]| (c.scope.clone(), egui::Color32::from_rgb(r, g, b)))).collect();
        self.cache.clear();
    }

    /// The scopes to pick from: key and name, built-in ones first.
    pub fn list(&self) -> Vec<(String, String)> {
        let mut v: Vec<(String, String)> = [PROJECT, TESTS, CHANGED].iter().map(|k| (k.to_string(), builtin_name(k).to_string())).collect();
        v.extend(self.user.iter().map(|s| (s.name.clone(), s.name.clone())));
        v
    }

    /// The name of a scope.
    pub fn label(&self, key: &str) -> String {
        if is_builtin(key) { builtin_name(key).to_string() } else { key.to_string() }
    }

    /// The files of a scope for Find in Files: `None` is every file (also for a scope that is
    /// gone); a pattern that does not parse is an error.
    pub fn filter(&self, key: &str) -> Result<Option<ScopeFilter>, ScopeError> {
        match key {
            TESTS => Ok(self.tests.clone().map(ScopeFilter::Mask)),
            CHANGED => Ok(Some(ScopeFilter::Paths(self.changed.clone()))),
            PROJECT => Ok(None),
            name => match self.compiled.get(name) {
                Some(Ok(mask)) => Ok(Some(ScopeFilter::Mask(mask.clone()))),
                Some(Err(e)) => Err(e.clone()),
                None => Ok(None),
            },
        }
    }

    /// The changed files (root-relative), when git's status is newer than `version`.
    pub fn sync_changed(&mut self, version: u64, changed: impl FnOnce() -> HashSet<String>) {
        if self.changed_version == Some(version) {
            return;
        }
        self.changed_version = Some(version);
        let set = changed();
        if *self.changed != set {
            self.changed = Arc::new(set);
            self.cache.clear();
        }
    }

    fn holds(&self, key: &str, rel: &str) -> bool {
        match key {
            PROJECT => true,
            TESTS => self.tests.as_ref().is_some_and(|m| m.matches(rel)),
            CHANGED => self.changed.contains(rel),
            name => matches!(self.compiled.get(name), Some(Ok(m)) if m.matches(rel)),
        }
    }

    /// The colour of a root-relative file: the first file colour whose scope holds it.
    pub fn color_of_rel(&mut self, rel: &str) -> Option<egui::Color32> {
        if self.rules.is_empty() {
            return None;
        }
        if let Some(c) = self.cache.get(rel) {
            return *c;
        }
        let color = self.rules.iter().find(|(k, _)| self.holds(k, rel)).map(|(_, c)| *c);
        if self.cache.len() >= CACHE_LIMIT {
            self.cache.clear();
        }
        self.cache.insert(rel.to_string(), color);
        color
    }

    /// The colour of a file of the project (none outside it).
    pub fn color_of(&mut self, path: &Path) -> Option<egui::Color32> {
        if self.rules.is_empty() {
            return None;
        }
        let rel = rc_core::workspace::relative_path(self.root.as_deref()?, path)?;
        self.color_of_rel(&rel)
    }

    /// A draft for Settings › Scopes.
    pub fn draft(&self) -> Draft {
        Draft { user: self.user.clone(), colors: self.colors.clone() }
    }
}

pub fn is_builtin(key: &str) -> bool {
    matches!(key, PROJECT | TESTS | CHANGED)
}

fn builtin_name(key: &str) -> &'static str {
    match key {
        PROJECT => tr("Project Files"),
        TESTS => tr("Tests"),
        CHANGED => tr("Changed Files"),
        _ => "",
    }
}

/// What is wrong with a user scope in the draft, if anything: an empty, reserved or repeated
/// name, or a pattern that does not parse.
pub fn draft_problem(draft: &Draft, i: usize) -> Option<String> {
    let s = draft.user.get(i)?;
    let name = s.name.trim();
    if name.is_empty() {
        return Some(tr("The scope needs a name").to_string());
    }
    if name.starts_with(BUILTIN_SCOPE_PREFIX)
        || draft.user[..i].iter().any(|o| o.name.trim() == name)
        || [PROJECT, TESTS, CHANGED].iter().any(|k| builtin_name(k) == name)
    {
        return Some(tr("Another scope has this name").to_string());
    }
    parse_pattern(name, &s.pattern).err().map(|e| e.text())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope(name: &str, pattern: &str) -> NamedScope {
        NamedScope { name: name.into(), pattern: pattern.into() }
    }

    fn color(scope: &str, color: &str) -> FileColor {
        FileColor { scope: scope.into(), color: color.into() }
    }

    #[test]
    fn the_first_scope_that_holds_a_file_gives_its_colour() {
        let mut s = Scopes::default();
        s.open(
            Path::new("/ws"),
            &[scope("Sources", "src/**/*.rs, !**/generated/**"), scope("Broken", "a[")],
            &[color(TESTS, "#00ff00"), color("Sources", "#0000ff"), color("Broken", "#ff0000"), color("Gone", "#ffffff")],
        );
        assert_eq!(s.colors.len(), 3, "the colour of a scope that is gone is dropped");
        let green = Some(egui::Color32::from_rgb(0, 255, 0));
        let blue = Some(egui::Color32::from_rgb(0, 0, 255));
        assert_eq!(s.color_of(Path::new("/ws/src/main.rs")), blue);
        assert_eq!(s.color_of(Path::new("/ws/src/tests.rs")), green);
        assert_eq!(s.color_of(Path::new("/ws/tests/it.rs")), green);
        assert_eq!(s.color_of(Path::new("/ws/crates/a/tests/it.rs")), green);
        assert_eq!(s.color_of(Path::new("/ws/src/generated/x.rs")), None);
        assert_eq!(s.color_of(Path::new("/ws/README.md")), None);
        assert_eq!(s.color_of(Path::new("/elsewhere/src/main.rs")), None);
        assert!(matches!(s.filter("Broken"), Err(ScopeError { glob, .. }) if glob == "a["));
        assert!(matches!(s.filter(PROJECT), Ok(None)) && matches!(s.filter("Gone"), Ok(None)));
    }

    #[test]
    fn changed_files_follow_git() {
        let mut s = Scopes::default();
        s.open(Path::new("/ws"), &[], &[color(CHANGED, "#e08e3c")]);
        assert_eq!(s.color_of_rel("src/lib.rs"), None);
        s.sync_changed(1, || ["src/lib.rs".to_string()].into_iter().collect());
        assert!(s.color_of_rel("src/lib.rs").is_some(), "the cache is cleared when the changes change");
        s.sync_changed(1, || panic!("the same status is not read again"));
        let Ok(Some(f)) = s.filter(CHANGED) else { panic!("changed files are a scope") };
        assert!(f.matches("src/lib.rs") && !f.matches("src/main.rs"));
    }

    #[test]
    fn draft_problems_are_explained() {
        let draft = Draft { user: vec![scope("A", "*.rs"), scope("A", "*.md"), scope("", "*"), scope("B", "a[")], colors: Vec::new() };
        assert_eq!(draft_problem(&draft, 0), None);
        assert!(draft_problem(&draft, 1).is_some() && draft_problem(&draft, 2).is_some());
        assert!(draft_problem(&draft, 3).is_some_and(|t| t.contains("a[")));
    }
}
