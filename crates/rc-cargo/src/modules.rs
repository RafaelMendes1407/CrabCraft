//! Rust's module tree on disk: which file declares a new module, and the `mod x;` line itself.

use std::path::{Path, PathBuf};

const KEYWORDS: &[&str] = &[
    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern", "false", "fn", "for", "if", "impl", "in", "let", "loop",
    "match", "mod", "move", "mut", "pub", "ref", "return", "self", "Self", "static", "struct", "super", "trait", "true", "type", "unsafe", "use", "where",
    "while",
];

/// Whether `name` can follow `mod`.
pub fn is_module_name(name: &str) -> bool {
    let mut chars = name.chars();
    let start = chars.next().is_some_and(|c| c == '_' || c.is_alphabetic());
    start && name != "_" && chars.all(|c| c == '_' || c.is_alphanumeric()) && !KEYWORDS.contains(&name)
}

/// The file that declares the modules of `dir`: `dir/mod.rs`, then `dir.rs`, then a crate root.
pub fn parent_module(dir: &Path) -> Option<PathBuf> {
    let beside = dir.file_name().and_then(|name| Some(dir.parent()?.join(format!("{}.rs", name.to_str()?))));
    [Some(dir.join("mod.rs")), beside, Some(dir.join("lib.rs")), Some(dir.join("main.rs"))].into_iter().flatten().find(|p| p.is_file())
}

/// Whether a crate created in `dir` joins a workspace: `cargo new` only adds it to one.
pub fn in_workspace(dir: &Path) -> bool {
    let declares = |d: &Path| std::fs::read_to_string(d.join("Cargo.toml")).is_ok_and(|t| t.lines().any(|l| l.trim() == "[workspace]"));
    dir.ancestors().any(declares)
}

/// Where to insert the declaration of `name` in `text` and what, as a byte offset and a line;
/// `None` when it is already declared. It goes after the last `mod` of the file, else after the
/// inner attributes and documentation that have to stay first.
pub fn declaration(text: &str, name: &str, public: bool) -> Option<(usize, String)> {
    let eol = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let line = format!("{}mod {name};", if public { "pub " } else { "" });
    let mut after_mod = None;
    let mut after_header = 0;
    let mut in_header = true;
    let mut open = 0i32;
    let mut at = 0;
    for raw in text.split_inclusive('\n') {
        let end = at + raw.len();
        let code = raw.trim_end();
        if let Some(declared) = declared_module(code) {
            if declared == name || declared == format!("r#{name}") {
                return None;
            }
            after_mod = Some(end);
        }
        if in_header {
            let trimmed = code.trim_start();
            if open > 0 || trimmed.starts_with("#![") {
                open += bracket_balance(trimmed);
                after_header = end;
            } else if trimmed.starts_with("//!") {
                after_header = end;
            } else if !trimmed.is_empty() {
                in_header = false;
            }
        }
        at = end;
    }
    let place = after_mod.unwrap_or(after_header);
    // A file without a final line break, or a declaration that opens a file with code.
    let before = if place > 0 && !text[..place].ends_with('\n') { eol } else { "" };
    let blank = if after_mod.is_none() && after_header > 0 { eol } else { "" };
    let rest = &text[place..];
    let spaced = rest.trim_start().is_empty() || rest.starts_with('\n') || rest.starts_with("\r\n");
    let after = if after_mod.is_none() && !spaced { eol } else { "" };
    Some((place, format!("{before}{blank}{line}{eol}{after}")))
}

/// `x` of `mod x;`, `pub mod x;` or `pub(crate) mod x;` at the start of a line.
fn declared_module(line: &str) -> Option<&str> {
    let mut rest = line.strip_prefix("pub").unwrap_or(line);
    if rest.len() != line.len() {
        if let Some(open) = rest.strip_prefix('(') {
            rest = open.split_once(')')?.1;
        }
        rest = rest.trim_start();
    }
    rest.strip_prefix("mod ")?.trim().strip_suffix(';').map(str::trim)
}

fn bracket_balance(line: &str) -> i32 {
    line.chars().map(|c| i32::from(c == '[') - i32::from(c == ']')).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn added(text: &str, name: &str, public: bool) -> String {
        let (at, line) = declaration(text, name, public).expect("a new declaration");
        format!("{}{line}{}", &text[..at], &text[at..])
    }

    #[test]
    fn module_names_are_identifiers_that_are_not_keywords() {
        for good in ["a", "_a", "my_mod2", "módulo"] {
            assert!(is_module_name(good), "{good}");
        }
        for bad in ["", "_", "2a", "my-mod", "a b", "a.b", "type", "mod", "self"] {
            assert!(!is_module_name(bad), "{bad}");
        }
    }

    #[test]
    fn the_declaration_goes_after_the_last_module() {
        assert_eq!(added("mod a;\npub mod b;\n\nfn main() {}\n", "c", false), "mod a;\npub mod b;\nmod c;\n\nfn main() {}\n");
        assert_eq!(added("pub(crate) mod a;\nfn f() {}\n", "c", true), "pub(crate) mod a;\npub mod c;\nfn f() {}\n");
        assert_eq!(added("mod a;", "c", false), "mod a;\nmod c;\n");
    }

    #[test]
    fn inner_attributes_and_documentation_stay_first() {
        assert_eq!(added("//! Docs.\n#![allow(unused)]\n\nfn f() {}\n", "c", false), "//! Docs.\n#![allow(unused)]\n\nmod c;\n\nfn f() {}\n");
        assert_eq!(
            added("#![cfg_attr(\n    test,\n    allow(unused)\n)]\nfn f() {}\n", "c", false),
            "#![cfg_attr(\n    test,\n    allow(unused)\n)]\n\nmod c;\n\nfn f() {}\n"
        );
        assert_eq!(added("fn f() {}\n", "c", false), "mod c;\n\nfn f() {}\n");
        assert_eq!(added("", "c", false), "mod c;\n");
    }

    #[test]
    fn line_endings_and_existing_declarations_are_respected() {
        assert_eq!(added("mod a;\r\nfn f() {}\r\n", "c", false), "mod a;\r\nmod c;\r\nfn f() {}\r\n");
        assert_eq!(declaration("mod a;\n", "a", false), None);
        assert_eq!(declaration("pub mod a;\n", "a", false), None);
        assert_eq!(declaration("    mod a;\n", "a", false).map(|d| d.0), Some(0), "nested in another item");
    }

    #[test]
    fn a_workspace_is_found_upwards() {
        let dir = tempfile::tempdir().unwrap();
        let inner = dir.path().join("a/b");
        std::fs::create_dir_all(&inner).unwrap();
        assert!(!in_workspace(&inner));
        std::fs::write(dir.path().join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
        assert!(!in_workspace(&inner));
        std::fs::write(dir.path().join("Cargo.toml"), "[workspace]\nmembers = []\n").unwrap();
        assert!(in_workspace(&inner));
    }

    #[test]
    fn the_parent_is_the_nearest_declaring_file() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        std::fs::create_dir_all(src.join("net/http")).unwrap();
        std::fs::create_dir_all(src.join("bin")).unwrap();
        std::fs::write(src.join("main.rs"), "").unwrap();
        assert_eq!(parent_module(&src), Some(src.join("main.rs")));
        std::fs::write(src.join("lib.rs"), "").unwrap();
        assert_eq!(parent_module(&src), Some(src.join("lib.rs")));
        assert_eq!(parent_module(&src.join("bin")), None);
        std::fs::write(src.join("net.rs"), "").unwrap();
        assert_eq!(parent_module(&src.join("net")), Some(src.join("net.rs")));
        std::fs::write(src.join("net/mod.rs"), "").unwrap();
        assert_eq!(parent_module(&src.join("net")), Some(src.join("net/mod.rs")));
        assert_eq!(parent_module(&src.join("net/http")), None);
    }
}
