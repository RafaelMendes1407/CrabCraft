//! The dependency tree of a package, from `cargo tree`.
//!
//! `cargo tree` resolves the dependency graph and builds nothing, so it is safe on a project that
//! is not trusted. It runs `--offline --locked`: it never downloads and never writes `Cargo.lock`.

use crate::metadata::{LoadLog, run_to_end};
use crate::toolchain::Cargo;
use std::path::Path;
use std::time::Instant;

/// Which section of the parent's manifest a dependency comes from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DepKind {
    #[default]
    Normal,
    Build,
    Dev,
}

/// One line of the tree, in the order Cargo printed it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DepNode {
    /// 0 for the package itself.
    pub depth: usize,
    pub name: String,
    /// Without the leading `v`.
    pub version: String,
    /// What follows the version: a path, `(proc-macro)`, a git source…
    pub detail: String,
    pub kind: DepKind,
    /// Its dependencies are listed where it first appears (`(*)`).
    pub repeated: bool,
}

/// The tree of `package`, with the log of the run. This blocks: call it from a background thread.
pub fn load_tree_logged(cargo: &Cargo, manifest: &Path, package: &str) -> (anyhow::Result<Vec<DepNode>>, LoadLog) {
    let args: Vec<std::ffi::OsString> = ["tree", "--offline", "--locked", "--charset", "ascii", "--package", package, "--manifest-path"]
        .into_iter()
        .map(Into::into)
        .chain([manifest.as_os_str().to_owned()])
        .collect();
    let command = format!("cargo tree --offline --locked --charset ascii --package {package} --manifest-path {}", manifest.display());
    let started = Instant::now();
    let mut stderr = String::new();
    let result = run_to_end(cargo, manifest, &args, &mut stderr).map(|out| parse_tree(&out));
    let log = LoadLog { command, stderr, duration: started.elapsed(), success: result.is_ok() };
    (result, log)
}

/// Parses the ASCII output of `cargo tree`. Lines it does not understand are skipped.
pub fn parse_tree(text: &str) -> Vec<DepNode> {
    let mut nodes = Vec::new();
    // The section in force for the children at each depth.
    let mut kinds: Vec<DepKind> = vec![DepKind::Normal];
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let (groups, rest) = split_prefix(line);
        if let Some(section) = rest.strip_prefix('[') {
            // A header at `groups` sets the section of the children at `groups + 1`.
            let kind = match section.trim_end_matches(']') {
                "build-dependencies" => DepKind::Build,
                "dev-dependencies" => DepKind::Dev,
                _ => DepKind::Normal,
            };
            if kinds.len() <= groups + 1 {
                kinds.resize(groups + 2, DepKind::Normal);
            }
            kinds[groups + 1] = kind;
            continue;
        }
        let depth = groups;
        let kind = kinds.get(depth).copied().unwrap_or_default();
        // A new parent: its children start in the normal section.
        kinds.truncate(depth + 1);
        kinds.push(DepKind::Normal);
        let mut words = rest.splitn(3, ' ');
        let name = words.next().unwrap_or_default().to_string();
        let Some(version) = words.next().and_then(|v| v.strip_prefix('v')) else { continue };
        let mut detail = words.next().unwrap_or_default().trim().to_string();
        let repeated = detail.ends_with("(*)");
        if repeated {
            detail = detail.trim_end_matches("(*)").trim_end().to_string();
        }
        nodes.push(DepNode { depth, name, version: version.to_string(), detail, kind, repeated });
    }
    nodes
}

/// The number of four-character indentation groups at the start of `line`, and what follows.
fn split_prefix(line: &str) -> (usize, &str) {
    let mut rest = line;
    let mut groups = 0;
    while let Some(head) = rest.get(..4) {
        match head {
            "|   " | "    " => {
                groups += 1;
                rest = &rest[4..];
            }
            "|-- " | "`-- " => {
                groups += 1;
                rest = &rest[4..];
                break;
            }
            _ => break,
        }
    }
    (groups, rest)
}

/// For each node, whether it has children (the next node is deeper).
pub fn has_children(nodes: &[DepNode]) -> Vec<bool> {
    (0..nodes.len()).map(|i| nodes.get(i + 1).is_some_and(|n| n.depth > nodes[i].depth)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const OUT: &str = "app v0.1.0 (/w/app)
|-- anyhow v1.0.104
|-- dirs v6.0.0
|   `-- dirs-sys v0.5.0
|       |-- libc v0.2.191
|       `-- option-ext v0.2.0
|   [build-dependencies]
|   `-- cc v1.5.1
|-- serde v1.0.229 (*)
`-- derive v0.1.0 (proc-macro)
[build-dependencies]
`-- embed-resource v3.0.12
    `-- cc v1.5.1 (*)
[dev-dependencies]
`-- egui_kittest v0.36.2
";

    #[test]
    fn depths_sections_and_repeats() {
        let n = parse_tree(OUT);
        let short: Vec<(usize, &str, DepKind, bool)> = n.iter().map(|d| (d.depth, d.name.as_str(), d.kind, d.repeated)).collect();
        use DepKind::*;
        assert_eq!(
            short,
            vec![
                (0, "app", Normal, false),
                (1, "anyhow", Normal, false),
                (1, "dirs", Normal, false),
                (2, "dirs-sys", Normal, false),
                (3, "libc", Normal, false),
                (3, "option-ext", Normal, false),
                (2, "cc", Build, false),
                (1, "serde", Normal, true),
                (1, "derive", Normal, false),
                (1, "embed-resource", Build, false),
                (2, "cc", Normal, true),
                (1, "egui_kittest", Dev, false),
            ]
        );
        assert_eq!(n[0].detail, "(/w/app)");
        assert_eq!(n[7].version, "1.0.229");
        assert_eq!(n[7].detail, "");
        assert_eq!(n[8].detail, "(proc-macro)");
        let kids = has_children(&n);
        assert!(kids[0] && kids[2] && !kids[1] && !kids[11]);
    }

    #[test]
    fn garbage_is_skipped() {
        assert!(parse_tree("warning: something\n\n|-- \n").is_empty());
        assert_eq!(parse_tree("é\n|-- a v1").len(), 1);
    }
}
