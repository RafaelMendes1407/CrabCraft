//! The code style a project shares in its files: `.editorconfig` for every file and, for Rust
//! sources, `rustfmt.toml` / `.rustfmt.toml` (what rustfmt formats with).
//!
//! Both are read from the project, so they are untrusted: big files are skipped, values out of
//! range are ignored and nothing in them can fail the caller.

use std::path::{Path, PathBuf};

/// Files bigger than this are not style files anyone writes by hand.
const MAX_FILE: u64 = 256 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IndentStyle {
    Tab,
    Space,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EndOfLine {
    Lf,
    CrLf,
}

/// What the style files say about one file; `None` where they say nothing.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CodeStyle {
    pub indent_style: Option<IndentStyle>,
    /// Columns of one indentation level.
    pub indent_size: Option<usize>,
    /// Columns a tab character takes.
    pub tab_width: Option<usize>,
    pub trim_trailing_whitespace: Option<bool>,
    pub insert_final_newline: Option<bool>,
    pub max_line_length: Option<usize>,
    pub end_of_line: Option<EndOfLine>,
    /// The files these values come from, nearest first.
    pub sources: Vec<PathBuf>,
}

impl CodeStyle {
    pub fn is_empty(&self) -> bool {
        self.sources.is_empty()
    }

    /// The indentation unit and tab width, when the style sets the indentation.
    pub fn indent(&self) -> Option<(String, usize)> {
        let style = self.indent_style?;
        let size = self.indent_size.or(self.tab_width).unwrap_or(4);
        let tab_width = self.tab_width.or(self.indent_size).unwrap_or(4);
        Some(match style {
            IndentStyle::Tab => ("\t".to_string(), tab_width),
            IndentStyle::Space => (" ".repeat(size), tab_width),
        })
    }

    /// Fills what `self` leaves unset from `other`.
    fn or(mut self, other: CodeStyle) -> CodeStyle {
        self.indent_style = self.indent_style.or(other.indent_style);
        self.indent_size = self.indent_size.or(other.indent_size);
        self.tab_width = self.tab_width.or(other.tab_width);
        self.trim_trailing_whitespace = self.trim_trailing_whitespace.or(other.trim_trailing_whitespace);
        self.insert_final_newline = self.insert_final_newline.or(other.insert_final_newline);
        self.max_line_length = self.max_line_length.or(other.max_line_length);
        self.end_of_line = self.end_of_line.or(other.end_of_line);
        self.sources.extend(other.sources);
        self
    }
}

/// The style of `file`. Style files are looked for from its folder up to `root` (the project)
/// and no further; without a root, up to an `.editorconfig` with `root = true`.
pub fn for_file(file: &Path, root: Option<&Path>) -> CodeStyle {
    let rust = file.extension().is_some_and(|e| e == "rs");
    let mut style = CodeStyle::default();
    let mut rustfmt_found = !rust;
    let mut editorconfig_done = false;
    let mut dir = file.parent();
    while let Some(d) = dir {
        if !rustfmt_found && let Some((path, text)) = ["rustfmt.toml", ".rustfmt.toml"].iter().find_map(|n| read(&d.join(n)).map(|t| (d.join(n), t))) {
            rustfmt_found = true;
            // rustfmt formats Rust files, so its settings come first.
            let found = parse_rustfmt(&text, path);
            style = found.or(style);
        }
        if !editorconfig_done && let Some(text) = read(&d.join(".editorconfig")) {
            let (found, is_root) = parse_editorconfig(&text, d, file);
            if found != CodeStyle::default() {
                let mut found = found;
                found.sources.push(d.join(".editorconfig"));
                style = style.or(found);
            }
            editorconfig_done = is_root;
        }
        if root.is_some_and(|r| d == r) || (editorconfig_done && rustfmt_found) {
            break;
        }
        dir = d.parent();
    }
    style
}

fn read(path: &Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_FILE {
        return None;
    }
    // A style file can be replaced after metadata: never block on a special file or
    // read an unbounded replacement while preparing a compiler source snapshot.
    use std::io::Read;
    let mut options = std::fs::OpenOptions::new(); options.read(true);
    #[cfg(unix)] { use std::os::unix::fs::OpenOptionsExt; options.custom_flags(libc::O_NONBLOCK); }
    let file = options.open(path).ok()?;
    if !file.metadata().ok()?.is_file() { return None; }
    let mut text = String::new(); file.take(MAX_FILE + 1).read_to_string(&mut text).ok()?;
    (text.len() as u64 <= MAX_FILE).then_some(text)
}

fn size(value: &str) -> Option<usize> {
    value.parse::<usize>().ok().filter(|n| (1..=32).contains(n))
}

fn flag(value: &str) -> Option<bool> {
    match value {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

/// `hard_tabs`, `tab_spaces`, `max_width` and `newline_style` of a rustfmt configuration.
/// Only top-level `key = value` lines matter here, so a full TOML parser is not needed.
fn parse_rustfmt(text: &str, path: PathBuf) -> CodeStyle {
    let mut style = CodeStyle::default();
    let (mut hard_tabs, mut tab_spaces) = (None, None);
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.starts_with('[') {
            break;
        }
        let Some((key, value)) = line.split_once('=') else { continue };
        let value = value.trim().trim_matches('"');
        match key.trim() {
            "hard_tabs" => hard_tabs = flag(value),
            "tab_spaces" => tab_spaces = size(value),
            "max_width" => style.max_line_length = value.parse::<usize>().ok().filter(|n| (10..=1000).contains(n)),
            "newline_style" => {
                style.end_of_line = match value {
                    "Unix" => Some(EndOfLine::Lf),
                    "Windows" => Some(EndOfLine::CrLf),
                    _ => None,
                }
            }
            _ => {}
        }
    }
    if hard_tabs.is_some() || tab_spaces.is_some() {
        style.indent_style = Some(if hard_tabs == Some(true) { IndentStyle::Tab } else { IndentStyle::Space });
        style.indent_size = Some(tab_spaces.unwrap_or(4));
        style.tab_width = Some(tab_spaces.unwrap_or(4));
    }
    style.sources.push(path);
    style
}

/// The properties of the sections of an `.editorconfig` in `dir` that match `file` (later
/// sections win), and whether it is marked `root = true`.
fn parse_editorconfig(text: &str, dir: &Path, file: &Path) -> (CodeStyle, bool) {
    let relative = file.strip_prefix(dir).ok().map(|p| p.to_string_lossy().replace('\\', "/"));
    let mut style = CodeStyle::default();
    let mut is_root = false;
    let mut matching = false;
    let mut preamble = true;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(section) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            preamble = false;
            matching = relative.as_deref().is_some_and(|r| section_matches(section, r));
            continue;
        }
        let Some((key, value)) = line.split_once('=') else { continue };
        let key = key.trim().to_ascii_lowercase();
        let value = value.trim().to_ascii_lowercase();
        if preamble {
            if key == "root" {
                is_root = value == "true";
            }
            continue;
        }
        if !matching {
            continue;
        }
        // `unset` clears what an earlier section set.
        let unset = value == "unset";
        match key.as_str() {
            "indent_style" => {
                style.indent_style = match value.as_str() {
                    "tab" => Some(IndentStyle::Tab),
                    "space" => Some(IndentStyle::Space),
                    _ => None,
                }
            }
            "indent_size" if value == "tab" => style.indent_size = None,
            "indent_size" => style.indent_size = if unset { None } else { size(&value) },
            "tab_width" => style.tab_width = if unset { None } else { size(&value) },
            "trim_trailing_whitespace" => style.trim_trailing_whitespace = flag(&value),
            "insert_final_newline" => style.insert_final_newline = flag(&value),
            "max_line_length" => style.max_line_length = value.parse::<usize>().ok().filter(|n| (10..=1000).contains(n)),
            "end_of_line" => {
                style.end_of_line = match value.as_str() {
                    "lf" => Some(EndOfLine::Lf),
                    "crlf" => Some(EndOfLine::CrLf),
                    _ => None,
                }
            }
            _ => {}
        }
    }
    (style, is_root)
}

/// EditorConfig globs: a section without `/` matches the file name in any folder; one with
/// `/` matches the path from the `.editorconfig`'s folder. `*` stays inside a folder, `**`
/// crosses them, plus `?`, `[...]` and `{a,b}`.
fn section_matches(section: &str, relative: &str) -> bool {
    let pattern = section.strip_prefix('/').map(str::to_string).unwrap_or_else(|| if section.contains('/') { section.to_string() } else { format!("**/{section}") });
    // globset only lets `**` cross folders as a whole path component: `a/**.md` → `a/**/*.md`.
    let mut normalized = String::with_capacity(pattern.len() + 4);
    let mut rest = pattern.as_str();
    while let Some(at) = rest.find("**") {
        let (before, after) = (&rest[..at], &rest[at + 2..]);
        normalized.push_str(before);
        if !(before.is_empty() || before.ends_with('/')) {
            normalized.push_str("*/");
        }
        normalized.push_str("**");
        if !(after.is_empty() || after.starts_with('/')) {
            normalized.push_str("/*");
        }
        rest = after;
    }
    normalized.push_str(rest);
    globset::GlobBuilder::new(&normalized).literal_separator(true).backslash_escape(true).build().is_ok_and(|g| g.compile_matcher().is_match(relative))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, name: &str, text: &str) -> PathBuf {
        let path = root.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
        path
    }

    #[test]
    fn editorconfig_sections_apply_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, ".editorconfig", "root = true\n\n[*]\nindent_style = space\nindent_size = 4\ninsert_final_newline = true\ntrim_trailing_whitespace = true\n\n[*.{json,yml}]\nindent_size = 2\n\n[Makefile]\nindent_style = tab\n\n[docs/**.md]\ntrim_trailing_whitespace = false\n");
        let json = for_file(&root.join("a/b/c.json"), Some(root));
        assert_eq!(json.indent(), Some(("  ".to_string(), 2)));
        assert_eq!(json.insert_final_newline, Some(true));
        assert_eq!(json.sources, vec![root.join(".editorconfig")]);
        let make = for_file(&root.join("sub/Makefile"), Some(root));
        assert_eq!(make.indent(), Some(("\t".to_string(), 4)));
        assert_eq!(for_file(&root.join("docs/x/readme.md"), Some(root)).trim_trailing_whitespace, Some(false));
        assert_eq!(for_file(&root.join("readme.md"), Some(root)).trim_trailing_whitespace, Some(true));
    }

    #[test]
    fn the_nearest_editorconfig_wins_until_a_root_one() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, ".editorconfig", "[*]\nindent_style = space\nindent_size = 8\nmax_line_length = 100\n");
        write(root, "crate/.editorconfig", "[*.rs]\nindent_size = 2\n");
        let style = for_file(&root.join("crate/src/lib.rs"), Some(root));
        assert_eq!(style.indent(), Some(("  ".to_string(), 2)));
        assert_eq!(style.max_line_length, Some(100));
        assert_eq!(style.sources, vec![root.join("crate/.editorconfig"), root.join(".editorconfig")]);

        write(root, "crate/.editorconfig", "root = true\n[*.rs]\nindent_size = 2\n");
        let style = for_file(&root.join("crate/src/lib.rs"), Some(root));
        assert_eq!(style.indent_style, None, "the parent is not read past a root file");
        assert_eq!(style.indent(), None);
    }

    #[test]
    fn rustfmt_settings_come_first_for_rust_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, ".editorconfig", "[*]\nindent_style = space\nindent_size = 4\ninsert_final_newline = true\n");
        write(root, "rustfmt.toml", "# shared style\nhard_tabs = true\ntab_spaces = 2 # columns\nmax_width = 80\nnewline_style = \"Windows\"\n[unstable]\nmax_width = 5\n");
        let rs = for_file(&root.join("src/main.rs"), Some(root));
        assert_eq!(rs.indent(), Some(("\t".to_string(), 2)));
        assert_eq!(rs.max_line_length, Some(80));
        assert_eq!(rs.end_of_line, Some(EndOfLine::CrLf));
        assert_eq!(rs.insert_final_newline, Some(true), "what rustfmt.toml does not say comes from .editorconfig");
        assert_eq!(rs.sources, vec![root.join("rustfmt.toml"), root.join(".editorconfig")]);
        // Other files ignore rustfmt.
        assert_eq!(for_file(&root.join("Cargo.toml"), Some(root)).indent(), Some(("    ".to_string(), 4)));

        write(root, "rustfmt.toml", "tab_spaces = 3\n");
        assert_eq!(for_file(&root.join("src/main.rs"), Some(root)).indent(), Some(("   ".to_string(), 3)));
    }

    #[test]
    fn nothing_outside_the_project_is_read() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), ".editorconfig", "[*]\nindent_style = tab\n");
        let project = dir.path().join("project");
        let style = for_file(&project.join("src/lib.rs"), Some(&project));
        assert!(style.is_empty());
        assert_eq!(style, CodeStyle::default());
    }

    #[test]
    fn bad_values_are_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, ".editorconfig", "[*]\nindent_style = sideways\nindent_size = 400\ntab_width = -1\nmax_line_length = off\ngarbage line\n[[broken\nend_of_line = cr\n");
        let style = for_file(&root.join("a.txt"), Some(root));
        assert_eq!((style.indent_style, style.indent_size, style.tab_width, style.max_line_length, style.end_of_line), (None, None, None, None, None));
        write(root, "rustfmt.toml", &"x".repeat(MAX_FILE as usize + 1));
        assert!(for_file(&root.join("b.rs"), Some(root)).sources.iter().all(|s| !s.ends_with("rustfmt.toml")));
    }
}
