//! Target triples the analysis can be done for: the host, the targets rustup knows (installed or
//! not) and the ones a project's `.cargo/config.toml` builds for. Nothing here runs project code.

use crate::toolchain::{Cargo, rustup_path};
use std::path::Path;

/// Whether `triple` can be passed to `rustc --target`/`cargo.target`: only `[A-Za-z0-9_.-]`, not
/// starting with `-` (it would read as an option), and of a sane length. A path to a target JSON
/// file is refused.
pub fn valid_triple(triple: &str) -> bool {
    !triple.is_empty()
        && triple.len() <= 100
        && !triple.starts_with('-')
        && triple.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
}

/// A target offered in the status bar list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TripleChoice {
    pub triple: String,
    /// The machine the IDE runs on (the default).
    pub host: bool,
    /// Installed with rustup; `false` also when rustup is missing and it comes from elsewhere.
    pub installed: bool,
    /// Named by the project's `.cargo/config.toml` `[build] target`.
    pub from_config: bool,
}

/// What `rustc -vV` prints on its `host:` line.
pub fn parse_host(text: &str) -> Option<String> {
    text.lines().find_map(|l| l.strip_prefix("host:")).map(str::trim).filter(|t| valid_triple(t)).map(str::to_string)
}

/// Reads `rustup target list`: one triple per line, `(installed)` after the installed ones.
/// Lines that are not a valid triple (with at least one `-`) are skipped.
pub fn parse_rustup_targets(text: &str) -> Vec<(String, bool)> {
    text.lines()
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            let triple = words.next()?;
            let installed = words.any(|w| w == "(installed)");
            (valid_triple(triple) && triple.contains('-')).then(|| (triple.to_string(), installed))
        })
        .collect()
}

/// The `[build] target` of a `.cargo/config.toml`, as text: `target = "x"` or
/// `target = ["x", "y"]` inside `[build]`, or `build.target = …` at the top. Values that are not
/// a valid triple (a JSON target file, say) are left out.
pub fn config_build_targets(text: &str) -> Vec<String> {
    let mut table = String::new();
    let mut out: Vec<String> = Vec::new();
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or_default().trim();
        if line.starts_with('[') {
            table = line.trim_matches(|c| c == '[' || c == ']').trim().to_string();
            continue;
        }
        let Some((key, value)) = line.split_once('=') else { continue };
        let key: String = key.split('.').map(|k| k.trim().trim_matches('"')).collect::<Vec<_>>().join(".");
        let full = if table.is_empty() { key } else { format!("{table}.{key}") };
        if full != "build.target" {
            continue;
        }
        for part in value.trim().trim_start_matches('[').trim_end_matches(']').split(',') {
            let t = part.trim().trim_matches(|c| c == '"' || c == '\'');
            if valid_triple(t) && !out.iter().any(|o| o == t) {
                out.push(t.to_string());
            }
        }
    }
    out
}

/// The `[build] target`s of `root/.cargo/config.toml` (or the older `config`). Reads text only.
pub fn project_build_targets(root: &Path) -> Vec<String> {
    let dir = root.join(".cargo");
    let text = ["config.toml", "config"].iter().find_map(|n| std::fs::read_to_string(dir.join(n)).ok()).unwrap_or_default();
    // A huge file is not a config anyone wrote by hand.
    if text.len() > 1 << 20 {
        return Vec::new();
    }
    config_build_targets(&text)
}

/// The list shown to the user: the host first, then the project's targets, the installed ones
/// and, when rustup listed them, the rest (marked as not installed). Each triple once.
pub fn triple_choices(host: Option<&str>, rustup: &[(String, bool)], config: &[String]) -> Vec<TripleChoice> {
    let installed = |t: &str| rustup.iter().any(|(r, i)| r == t && *i) || (rustup.is_empty() && host == Some(t));
    let mut out: Vec<TripleChoice> = Vec::new();
    let add = |out: &mut Vec<TripleChoice>, t: &str| {
        if !out.iter().any(|c| c.triple == t) {
            out.push(TripleChoice { triple: t.to_string(), host: host == Some(t), installed: installed(t), from_config: config.iter().any(|c| c == t) });
        }
    };
    if let Some(h) = host {
        add(&mut out, h);
    }
    for t in config {
        add(&mut out, t);
    }
    for (t, _) in rustup.iter().filter(|(_, i)| *i) {
        add(&mut out, t);
    }
    for (t, _) in rustup.iter().filter(|(_, i)| !*i) {
        add(&mut out, t);
    }
    out
}

/// The host triple, from `rustc -vV` next to `cargo`. It compiles nothing. This blocks.
pub fn host_triple(cargo: &Cargo) -> Option<String> {
    let out = cargo.std_tool_command("rustc").arg("-vV").stdin(std::process::Stdio::null()).output().ok().filter(|o| o.status.success())?;
    parse_host(&String::from_utf8_lossy(&out.stdout))
}

/// The targets rustup knows for the default toolchain, with whether each is installed. Run from
/// the temporary folder, so a project's `rust-toolchain.toml` cannot make rustup install
/// anything. It reads local files only. Empty without rustup. This blocks.
pub fn rustup_targets(cargo: &Cargo) -> Vec<(String, bool)> {
    let Some(rustup) = rustup_path(cargo) else { return Vec::new() };
    let run = |args: &[&str]| {
        let out = std::process::Command::new(&rustup)
            .args(args)
            .current_dir(std::env::temp_dir())
            .stdin(std::process::Stdio::null())
            .output()
            .ok()
            .filter(|o| o.status.success())?;
        Some(String::from_utf8_lossy(&out.stdout).into_owned())
    };
    if let Some(text) = run(&["target", "list"]) {
        let list = parse_rustup_targets(&text);
        if !list.is_empty() {
            return list;
        }
    }
    run(&["target", "list", "--installed"]).map(|t| parse_rustup_targets(&t).into_iter().map(|(t, _)| (t, true)).collect()).unwrap_or_default()
}

/// Everything the target list needs, for the project at `root`. This blocks.
pub fn load_triple_choices(cargo: Option<&Cargo>, root: &Path) -> (Option<String>, Vec<TripleChoice>) {
    let host = cargo.and_then(host_triple);
    let rustup = cargo.map(rustup_targets).unwrap_or_default();
    let config = project_build_targets(root);
    let list = triple_choices(host.as_deref(), &rustup, &config);
    (host, list)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_plain_triples_are_valid() {
        for ok in ["x86_64-pc-windows-msvc", "thumbv7em-none-eabihf", "wasm32-unknown-unknown", "aarch64-apple-ios-macabi", "x86_64-unknown-linux-gnu.2.17"] {
            assert!(valid_triple(ok), "{ok}");
        }
        for bad in ["", "-Zunstable", "a b", "x;rm", "../my.json", "/abs/t.json", "t\n", "é", &"a".repeat(101)] {
            assert!(!valid_triple(bad), "{bad:?}");
        }
    }

    #[test]
    fn reads_the_host_of_rustc() {
        assert_eq!(parse_host("rustc 1.99.0\nbinary: rustc\nhost: x86_64-unknown-linux-gnu\nrelease: 1.99.0\n").as_deref(), Some("x86_64-unknown-linux-gnu"));
        assert_eq!(parse_host("host: bad triple\n"), None);
        assert_eq!(parse_host(""), None);
    }

    #[test]
    fn reads_the_rustup_list() {
        let text = "aarch64-apple-darwin\nx86_64-unknown-linux-gnu (installed)\n\nnot a; triple\n";
        assert_eq!(parse_rustup_targets(text), vec![("aarch64-apple-darwin".to_string(), false), ("x86_64-unknown-linux-gnu".to_string(), true)]);
    }

    #[test]
    fn reads_the_build_target_of_the_config() {
        assert_eq!(config_build_targets("[build]\ntarget = \"thumbv7em-none-eabihf\" # board\n"), vec!["thumbv7em-none-eabihf"]);
        assert_eq!(config_build_targets("[build]\ntarget = [\"a-b-c\", 'x-y-z', \"a-b-c\"]\n"), vec!["a-b-c", "x-y-z"]);
        assert_eq!(config_build_targets("build.target = \"wasm32-unknown-unknown\"\n"), vec!["wasm32-unknown-unknown"]);
        // Other tables, a JSON target file and garbage are ignored.
        assert!(config_build_targets("[target.x]\ntarget = \"a-b-c\"\n[build]\ntarget = \"my/t.json\"\n").is_empty());
        assert!(config_build_targets("[[[\n=\n\"").is_empty());
    }

    #[test]
    fn the_host_comes_first_and_uninstalled_last() {
        let rustup =
            vec![("aarch64-apple-darwin".to_string(), false), ("x86_64-unknown-linux-gnu".to_string(), true), ("wasm32-unknown-unknown".to_string(), true)];
        let list = triple_choices(Some("x86_64-unknown-linux-gnu"), &rustup, &["thumbv7em-none-eabihf".to_string()]);
        let names: Vec<&str> = list.iter().map(|c| c.triple.as_str()).collect();
        assert_eq!(names, vec!["x86_64-unknown-linux-gnu", "thumbv7em-none-eabihf", "wasm32-unknown-unknown", "aarch64-apple-darwin"]);
        assert!(list[0].host && list[0].installed);
        assert!(list[1].from_config && !list[1].installed);
        assert!(!list[3].installed);
        // Without rustup, the host still counts as installed.
        assert!(triple_choices(Some("h-o-st"), &[], &[])[0].installed);
    }
}
