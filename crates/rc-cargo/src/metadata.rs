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
    /// Features Cargo needs before it builds this target.
    pub required_features: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Package {
    /// Has a cdylib target (a candidate for wasm-pack, not a compatibility guarantee).
    pub cdylib: bool,
    pub name: String,
    pub manifest: PathBuf,
    pub edition: String,
    pub targets: Vec<Target>,
    /// The `[features]` table: each feature and what it enables.
    pub features: std::collections::BTreeMap<String, Vec<String>>,
}

impl Package {
    /// The features a plain `cargo build` enables: `default` and what it enables, transitively,
    /// plus `extra`. Optional dependencies (`dep:x`) and features of dependencies (`x/f`) are
    /// left out; a feature of the package that names an optional dependency is kept.
    pub fn enabled_features(&self, extra: &[String]) -> Vec<String> {
        let mut on: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        let mut todo: Vec<String> = extra.to_vec();
        if self.features.contains_key("default") {
            todo.push("default".into());
        }
        while let Some(f) = todo.pop() {
            if f.starts_with("dep:") || f.contains('/') || !on.insert(f.clone()) {
                continue;
            }
            todo.extend(self.features.get(&f).into_iter().flatten().cloned());
        }
        on.into_iter().collect()
    }

    /// The features of this package among `chosen`, which holds `package/feature` entries for
    /// the whole workspace (as rust-analyzer's `cargo.features` takes them). Unknown names are
    /// dropped: the manifest may have changed since they were chosen.
    pub fn chosen_features(&self, chosen: &[String]) -> Vec<String> {
        chosen.iter().filter_map(|c| c.split_once('/')).filter(|(p, f)| *p == self.name && self.features.contains_key(*f)).map(|(_, f)| f.to_string()).collect()
    }
}

/// Whether `entry` is a `package/feature` pair Cargo could accept (`--features p/f`). Session
/// files are untrusted: anything else would reach a command line.
pub fn valid_feature_entry(entry: &str) -> bool {
    let ok = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '+' | '.'));
    entry.split_once('/').is_some_and(|(p, f)| ok(p) && ok(f))
}

/// The entries of the `[features]` table of a `Cargo.toml`: 0-based line and feature name.
/// A plain line scan, so it works on unsaved text; continuation lines of a multi-line array
/// and comments are skipped.
pub fn feature_lines(text: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut inside = false;
    let mut in_array = false;
    for (i, line) in text.lines().enumerate() {
        let t = line.split('#').next().unwrap_or("").trim();
        if in_array {
            in_array = !t.contains(']');
            continue;
        }
        if t.starts_with('[') {
            inside = !t.starts_with("[[") && t.trim_start_matches('[').trim_end_matches(']').trim() == "features";
            continue;
        }
        if !inside {
            continue;
        }
        let Some((key, value)) = t.split_once('=') else { continue };
        let key = key.trim();
        let key = key.strip_prefix('"').and_then(|k| k.strip_suffix('"')).unwrap_or(key);
        if !key.is_empty() && key.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '+' | '.')) {
            out.push((i, key.to_string()));
        }
        let value = value.trim();
        in_array = value.starts_with('[') && !value.contains(']');
    }
    out
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
    /// The edition of the member that owns `path`, the innermost one when members are nested.
    pub fn edition_of(&self, path: &Path) -> Option<&str> {
        self.package_of(path).map(|p| p.edition.as_str())
    }

    /// The member that owns `path`, the innermost one when members are nested.
    pub fn package_of(&self, path: &Path) -> Option<&Package> {
        // Canonical on both sides: on Windows one may be verbatim (`\\?\C:\…`) and the other `C:\…`.
        let canon = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
        let path = &canon(path);
        let folder = |p: &&Package| p.manifest.parent().map(canon).unwrap_or_default();
        self.packages.iter().filter(|p| path.starts_with(folder(p))).max_by_key(|p| folder(p).components().count())
    }

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
    load_workspace_logged(cargo, manifest).0
}

/// What one `cargo metadata` run did, for the Sync log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadLog {
    /// The command line, as a user would type it.
    pub command: String,
    /// What cargo printed on stderr (downloads, warnings, the error).
    pub stderr: String,
    pub duration: Duration,
    pub success: bool,
}

/// [`load_workspace`], with the log of the run.
pub fn load_workspace_logged(cargo: &Cargo, manifest: &Path) -> (anyhow::Result<CargoWorkspace>, LoadLog) {
    let (out, log) = run_metadata_logged(cargo, manifest, &["--no-deps"]);
    (out.and_then(|text| parse_metadata(&text)), log)
}

/// [`load_libraries`], with the log of the run.
pub fn load_libraries_logged(cargo: &Cargo, manifest: &Path) -> (anyhow::Result<Vec<Library>>, LoadLog) {
    let (out, log) = run_metadata_logged(cargo, manifest, &["--offline", "--locked"]);
    (out.and_then(|text| parse_libraries(&text)), log)
}

/// A dependency of the workspace, with its sources on disk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Library {
    pub name: String,
    pub version: String,
    pub root: PathBuf,
    /// Cargo downloaded it: changing its files would break other projects. Path dependencies are
    /// the user's own code.
    pub read_only: bool,
}

/// The dependencies recorded in `Cargo.lock`, without touching the network or that file. It fails
/// when the lock file is missing, stale or names a crate that was not downloaded yet.
pub fn load_libraries(cargo: &Cargo, manifest: &Path) -> anyhow::Result<Vec<Library>> {
    load_libraries_logged(cargo, manifest).0
}

/// Every package of the metadata that is not a workspace member, by name.
pub fn parse_libraries(text: &str) -> anyhow::Result<Vec<Library>> {
    let v: Value = serde_json::from_str(text).context("cargo metadata printed invalid JSON")?;
    let members: Vec<&str> =
        v.get("workspace_members").and_then(Value::as_array).context("metadata without members")?.iter().filter_map(Value::as_str).collect();
    let mut libraries = Vec::new();
    for p in v.get("packages").and_then(Value::as_array).context("metadata without packages")? {
        if p.get("id").and_then(Value::as_str).is_some_and(|id| members.contains(&id)) {
            continue;
        }
        let manifest = path_of(p, "manifest_path")?;
        let root = manifest.parent().context("a package without a folder")?.to_path_buf();
        let text = |key: &str| p.get(key).and_then(Value::as_str).unwrap_or_default().to_string();
        libraries.push(Library { name: text("name"), version: text("version"), root, read_only: p.get("source").is_some_and(|s| !s.is_null()) });
    }
    libraries.sort_by(|a, b| (&a.name, &a.version).cmp(&(&b.name, &b.version)));
    Ok(libraries)
}

/// Runs `cargo metadata` and keeps what it printed on stderr, also when it fails.
fn run_metadata_logged(cargo: &Cargo, manifest: &Path, options: &[&str]) -> (anyhow::Result<String>, LoadLog) {
    let mut command = format!("cargo metadata {} --format-version 1 --manifest-path {}", options.join(" "), manifest.display());
    command = command.replace("  ", " ");
    let started = Instant::now();
    let mut stderr = String::new();
    let result = run_metadata(cargo, manifest, options, &mut stderr);
    let log = LoadLog { command, stderr, duration: started.elapsed(), success: result.is_ok() };
    (result, log)
}

fn run_metadata(cargo: &Cargo, manifest: &Path, options: &[&str], stderr_text: &mut String) -> anyhow::Result<String> {
    let mut args: Vec<std::ffi::OsString> = vec!["metadata".into()];
    args.extend(options.iter().map(Into::into));
    args.extend(["--format-version".into(), "1".into(), "--manifest-path".into(), manifest.as_os_str().to_owned()]);
    run_to_end(cargo, manifest, &args, stderr_text)
}

/// Runs a Cargo command that only reads the project, from the manifest's folder, and returns what
/// it printed. It is killed after [`LOAD_TIMEOUT`]. This blocks.
pub(crate) fn run_to_end(cargo: &Cargo, manifest: &Path, args: &[std::ffi::OsString], stderr_text: &mut String) -> anyhow::Result<String> {
    use std::process::Stdio;
    let dir = manifest.parent().context("the manifest has no folder")?;
    let name = args.first().map(|a| a.to_string_lossy().into_owned()).unwrap_or_default();
    let mut child = cargo
        .std_command()
        .args(args)
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
            anyhow::bail!("cargo {name} did not finish in {} s", LOAD_TIMEOUT.as_secs());
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let stdout = reader.join().unwrap_or_default();
    let stderr = err_reader.join().unwrap_or_default();
    *stderr_text = String::from_utf8_lossy(&stderr).into_owned();
    if !status.success() {
        anyhow::bail!("{}", String::from_utf8_lossy(&stderr).trim());
    }
    Ok(String::from_utf8_lossy(&stdout).into_owned())
}

fn path_of(v: &Value, key: &str) -> anyhow::Result<PathBuf> {
    v.get(key).and_then(Value::as_str).map(PathBuf::from).with_context(|| format!("metadata without `{key}`"))
}

/// Parses the JSON printed by `cargo metadata --format-version 1`.
pub fn parse_metadata(text: &str) -> anyhow::Result<CargoWorkspace> {
    let v: Value = serde_json::from_str(text).context("cargo metadata printed invalid JSON")?;
    let root = path_of(&v, "workspace_root")?;
    let target_dir = path_of(&v, "target_directory")?;
    let members: Vec<&str> =
        v.get("workspace_members").and_then(Value::as_array).context("metadata without members")?.iter().filter_map(Value::as_str).collect();
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
            let required_features = t
                .get("required-features")
                .and_then(Value::as_array)
                .map_or(&[][..], Vec::as_slice)
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect();
            targets.push(Target { name: name.to_string(), kind: target_kind(&kinds), src: path_of(t, "src_path")?, required_features });
        }
        let features = p
            .get("features")
            .and_then(Value::as_object)
            .map(|m| {
                m.iter()
                    .map(|(k, v)| (k.clone(), v.as_array().map_or(&[][..], Vec::as_slice).iter().filter_map(Value::as_str).map(str::to_string).collect()))
                    .collect()
            })
            .unwrap_or_default();
        let cdylib = p
            .get("targets")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .any(|t| t.get("crate_types").and_then(Value::as_array).into_iter().flatten().any(|kind| kind.as_str() == Some("cdylib")));
        packages.push(Package {
            cdylib,
            name: p.get("name").and_then(Value::as_str).unwrap_or_default().to_string(),
            manifest: path_of(p, "manifest_path")?,
            edition: p.get("edition").and_then(Value::as_str).unwrap_or("2015").to_string(),
            targets,
            features,
        });
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
    fn enabled_features_follow_default() {
        let features =
            [("default", vec!["std"]), ("std", vec!["alloc", "dep:libc", "serde/std"]), ("alloc", vec![]), ("extra", vec![]), ("libc", vec!["dep:libc"])]
                .into_iter()
                .map(|(k, v)| (k.to_string(), v.into_iter().map(str::to_string).collect()))
                .collect();
        let p = Package { cdylib: false, name: "p".into(), manifest: "Cargo.toml".into(), edition: "2021".into(), targets: Vec::new(), features };
        assert_eq!(p.enabled_features(&[]), vec!["alloc", "default", "std"]);
        assert_eq!(p.enabled_features(&["extra".to_string(), "libc".to_string()]), vec!["alloc", "default", "extra", "libc", "std"]);
        let none = Package { features: Default::default(), ..p };
        assert!(none.enabled_features(&[]).is_empty());
    }

    #[test]
    fn feature_lines_follow_the_features_table() {
        let text = "[package]\nname = \"p\"\n\n[features]\n# a comment\ndefault = [\"std\"]\nstd = [\n    \"alloc\", # why\n    \"x = 1\",\n]\n\"quoted-name\" = []\nalloc = [] # inline\n\n[dependencies]\nserde = \"1\"\n";
        assert_eq!(feature_lines(text), vec![(5, "default".to_string()), (6, "std".to_string()), (10, "quoted-name".to_string()), (11, "alloc".to_string())]);
        assert!(feature_lines("[package]\nname = \"p\"\n").is_empty());
    }

    #[test]
    fn chosen_features_are_per_package_and_checked() {
        let features = ["extra", "std"].into_iter().map(|k| (k.to_string(), Vec::new())).collect();
        let p = Package { cdylib: false, name: "p".into(), manifest: "Cargo.toml".into(), edition: "2021".into(), targets: Vec::new(), features };
        let chosen = ["p/extra", "q/extra", "p/gone", "extra"].map(String::from);
        assert_eq!(p.chosen_features(&chosen), vec!["extra"]);
        assert!(valid_feature_entry("my-crate/serde+json"));
        for bad in ["extra", "/x", "p/", "p/a b", "p/a,b", "p/--x\n", "a/b/c"] {
            assert!(!valid_feature_entry(bad), "{bad}");
        }
    }

    #[test]
    fn malformed_metadata_is_an_error() {
        let text = r#"{"workspace_members":["m"],"packages":[
            {"id":"m","name":"mine","version":"1.0.0","source":null,"manifest_path":"/w/Cargo.toml"},
            {"id":"z","name":"zed","version":"2.0.0","source":"registry+https://x","manifest_path":"/r/zed/Cargo.toml"},
            {"id":"a","name":"near","version":"0.1.0","source":null,"manifest_path":"/n/Cargo.toml"}]}"#;
        let list = parse_libraries(text).unwrap();
        assert_eq!(list.iter().map(|l| (l.name.as_str(), l.read_only)).collect::<Vec<_>>(), [("near", false), ("zed", true)]);
        assert_eq!(list[1].root, Path::new("/r/zed"));
        assert!(parse_libraries("").is_err());
        assert!(parse_libraries("{}").is_err());
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
