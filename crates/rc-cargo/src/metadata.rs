//! The workspace model, from `cargo metadata --no-deps`.

use crate::toolchain::Cargo;
use anyhow::Context;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum TargetKind {
    Bin,
    Lib,
    Example,
    Test,
    Bench,
    /// `build.rs` and anything else Cargo does not run directly.
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub name: String,
    pub kind: TargetKind,
    pub src: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Package {
    pub name: String,
    pub manifest: PathBuf,
    pub targets: Vec<Target>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CargoWorkspace {
    pub root: PathBuf,
    /// The `Cargo.toml` at the workspace root.
    pub manifest: PathBuf,
    pub target_dir: PathBuf,
    /// Workspace members, in Cargo's order.
    pub packages: Vec<Package>,
}

impl CargoWorkspace {
    /// Runnable binaries as `(package, target)`.
    pub fn binaries(&self) -> impl Iterator<Item = (&Package, &Target)> {
        self.packages.iter().flat_map(|p| p.targets.iter().filter(|t| t.kind == TargetKind::Bin).map(move |t| (p, t)))
    }
}

/// The nearest `Cargo.toml` at or above `start` (a file or a folder).
pub fn find_manifest(start: &Path) -> Option<PathBuf> {
    if start.is_file() {
        return (start.file_name().is_some_and(|n| n == "Cargo.toml")).then(|| start.to_path_buf());
    }
    start.ancestors().map(|d| d.join("Cargo.toml")).find(|m| m.is_file())
}

/// Longest time a project may take to describe itself.
const LOAD_TIMEOUT: Duration = Duration::from_secs(30);

/// Loads the workspace of `manifest`. This blocks: call it from a background thread.
pub fn load_workspace(cargo: &Cargo, manifest: &Path) -> anyhow::Result<CargoWorkspace> {
    use std::process::Stdio;
    let dir = manifest.parent().context("the manifest has no folder")?;
    let mut child = cargo
        .std_command()
        .args(["metadata", "--no-deps", "--format-version", "1", "--manifest-path"])
        .arg(manifest)
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("could not start cargo")?;
    let (mut out, mut err) = (child.stdout.take().context("no stdout")?, child.stderr.take().context("no stderr")?);
    // Both pipes are read aside, or a big output would block the child.
    let reader = std::thread::spawn(move || {
        let mut text = Vec::new();
        let _ = std::io::Read::read_to_end(&mut out, &mut text);
        text
    });
    let err_reader = std::thread::spawn(move || {
        let mut text = Vec::new();
        let _ = std::io::Read::read_to_end(&mut err, &mut text);
        text
    });
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if started.elapsed() > LOAD_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            anyhow::bail!("cargo metadata did not finish in {} s", LOAD_TIMEOUT.as_secs());
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let stdout = reader.join().unwrap_or_default();
    let stderr = err_reader.join().unwrap_or_default();
    if !status.success() {
        anyhow::bail!("{}", String::from_utf8_lossy(&stderr).trim());
    }
    parse_metadata(&String::from_utf8_lossy(&stdout))
}

fn path_of(v: &Value, key: &str) -> anyhow::Result<PathBuf> {
    v.get(key).and_then(Value::as_str).map(PathBuf::from).with_context(|| format!("metadata without `{key}`"))
}

/// Parses the JSON printed by `cargo metadata --format-version 1`.
pub fn parse_metadata(text: &str) -> anyhow::Result<CargoWorkspace> {
    let v: Value = serde_json::from_str(text).context("cargo metadata printed invalid JSON")?;
    let root = path_of(&v, "workspace_root")?;
    let target_dir = path_of(&v, "target_directory")?;
    let members: Vec<&str> = v.get("workspace_members").and_then(Value::as_array).context("metadata without members")?.iter().filter_map(Value::as_str).collect();
    let mut packages = Vec::new();
    for p in v.get("packages").and_then(Value::as_array).context("metadata without packages")? {
        let id = p.get("id").and_then(Value::as_str).unwrap_or_default();
        if !members.contains(&id) {
            continue;
        }
        let mut targets = Vec::new();
        for t in p.get("targets").and_then(Value::as_array).map_or(&[][..], Vec::as_slice) {
            let Some(name) = t.get("name").and_then(Value::as_str) else { continue };
            let kinds: Vec<&str> = t.get("kind").and_then(Value::as_array).map_or(&[][..], Vec::as_slice).iter().filter_map(Value::as_str).collect();
            targets.push(Target { name: name.to_string(), kind: target_kind(&kinds), src: path_of(t, "src_path")? });
        }
        packages.push(Package { name: p.get("name").and_then(Value::as_str).unwrap_or_default().to_string(), manifest: path_of(p, "manifest_path")?, targets });
    }
    Ok(CargoWorkspace { manifest: root.join("Cargo.toml"), root, target_dir, packages })
}

fn target_kind(kinds: &[&str]) -> TargetKind {
    let has = |k: &str| kinds.contains(&k);
    if has("bin") {
        TargetKind::Bin
    } else if has("example") {
        TargetKind::Example
    } else if has("test") {
        TargetKind::Test
    } else if has("bench") {
        TargetKind::Bench
    } else if has("lib") || has("rlib") || has("cdylib") || has("dylib") || has("staticlib") || has("proc-macro") {
        TargetKind::Lib
    } else {
        TargetKind::Other
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{"packages":[
      {"name":"app","id":"path+file:///w/app#0.1.0","manifest_path":"/w/app/Cargo.toml","targets":[
        {"kind":["bin"],"name":"app","src_path":"/w/app/src/main.rs"},
        {"kind":["custom-build"],"name":"build-script-build","src_path":"/w/app/build.rs"},
        {"kind":["test"],"name":"smoke","src_path":"/w/app/tests/smoke.rs"}]},
      {"name":"lib_a","id":"path+file:///w/lib_a#0.1.0","manifest_path":"/w/lib_a/Cargo.toml","targets":[
        {"kind":["lib"],"name":"lib_a","src_path":"/w/lib_a/src/lib.rs"},
        {"kind":["example"],"name":"demo","src_path":"/w/lib_a/examples/demo.rs"}]},
      {"name":"other","id":"registry#other@1","manifest_path":"/r/other/Cargo.toml","targets":[]}],
      "workspace_members":["path+file:///w/app#0.1.0","path+file:///w/lib_a#0.1.0"],
      "target_directory":"/w/target","workspace_root":"/w"}"#;

    #[test]
    fn parses_members_and_their_targets() {
        let w = parse_metadata(SAMPLE).unwrap();
        assert_eq!(w.root, Path::new("/w"));
        assert_eq!(w.manifest, Path::new("/w/Cargo.toml"));
        assert_eq!(w.packages.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["app", "lib_a"]);
        let kinds: Vec<_> = w.packages[0].targets.iter().map(|t| t.kind).collect();
        assert_eq!(kinds, [TargetKind::Bin, TargetKind::Other, TargetKind::Test]);
        assert_eq!(w.packages[1].targets[0].kind, TargetKind::Lib);
        assert_eq!(w.packages[1].targets[1].kind, TargetKind::Example);
        assert_eq!(w.binaries().map(|(p, t)| (p.name.as_str(), t.name.as_str())).collect::<Vec<_>>(), [("app", "app")]);
    }

    #[test]
    fn malformed_metadata_is_an_error() {
        assert!(parse_metadata("").is_err());
        assert!(parse_metadata("[]").is_err());
        assert!(parse_metadata(r#"{"workspace_root":"/w"}"#).is_err());
    }

    #[test]
    fn finds_the_manifest_above_a_source_folder() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "").unwrap();
        assert_eq!(find_manifest(&src), Some(dir.path().join("Cargo.toml")));
        assert_eq!(find_manifest(&dir.path().join("Cargo.toml")), Some(dir.path().join("Cargo.toml")));
        std::fs::write(src.join("main.rs"), "").unwrap();
        assert_eq!(find_manifest(&src.join("main.rs")), None);
    }
}
