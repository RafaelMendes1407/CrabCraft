//! Explicit CriticalUp provider. Activation never installs/authenticates and never falls back to rustup.
use crate::Cargo;
use rc_dotnet::CancelToken;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    io::Read,
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::Duration,
};
use tokio::io::AsyncReadExt;
const MANIFEST: usize = 64 * 1024;
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    Trust,
    Path,
    Manifest,
    Version,
    Io,
    Limit,
    Cancelled,
    ToolMismatch,
    Manager { code: Option<i32>, message: String },
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct Manifest {
    manifest_version: u32,
    products: BTreeMap<String, Product>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Product {
    release: String,
    packages: Vec<String>,
}
#[derive(Clone, Debug)]
pub struct Snapshot(Arc<ManifestData>);
#[derive(Debug)]
struct ManifestData {
    root: PathBuf,
    path: PathBuf,
    release: String,
    packages: Vec<String>,
    digest: String,
    _directory: tempfile::TempDir,
}
impl PartialEq for Snapshot {
    fn eq(&self, other: &Self) -> bool {
        self.0.path == other.0.path
    }
}
impl Eq for Snapshot {}
impl Snapshot {
    pub fn root(&self) -> &Path {
        &self.0.root
    }
    pub fn path(&self) -> &Path {
        &self.0.path
    }
    pub fn release(&self) -> &str {
        &self.0.release
    }
    pub fn packages(&self) -> &[String] {
        &self.0.packages
    }
    pub fn digest(&self) -> &str {
        &self.0.digest
    }
}
fn cancelled(stop: &CancelToken) -> Result<(), Error> {
    if stop.is_cancelled() { Err(Error::Cancelled) } else { Ok(()) }
}
/// Data-only manifest inspection, on a worker. No program or network request is started.
pub fn inspect(root: &Path, stop: &CancelToken) -> Result<Snapshot, Error> {
    cancelled(stop)?;
    let root = root.canonicalize().map_err(|_| Error::Path)?;
    if !root.is_absolute() || !root.is_dir() {
        return Err(Error::Path);
    }
    let path = root.join("criticalup.toml");
    if !std::fs::symlink_metadata(&path).map_err(|_| Error::Io)?.is_file() {
        return Err(Error::Path);
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let mut file = options.open(path).map_err(|_| Error::Path)?;
    let metadata = file.metadata().map_err(|_| Error::Io)?;
    if !metadata.is_file() || metadata.len() > MANIFEST as u64 {
        return Err(Error::Limit);
    }
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        cancelled(stop)?;
        let n = file.read(&mut chunk).map_err(|_| Error::Io)?;
        if n == 0 {
            break;
        }
        if bytes.len() + n > MANIFEST {
            return Err(Error::Limit);
        }
        bytes.extend_from_slice(&chunk[..n]);
    }
    let text = std::str::from_utf8(&bytes).map_err(|_| Error::Manifest)?;
    let manifest: Manifest = toml::from_str(text).map_err(|_| Error::Manifest)?;
    if manifest.manifest_version != 1 || manifest.products.len() != 1 {
        return Err(Error::Manifest);
    }
    let product = manifest.products.get("ferrocene").ok_or(Error::Manifest)?;
    if product.release.is_empty()
        || product.release.len() > 128
        || !product.release.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        || product.packages.is_empty()
        || product.packages.len() > 64
        || !product.packages.iter().any(|p| p.starts_with("cargo-"))
        || !product.packages.iter().any(|p| p.starts_with("rustc-"))
    {
        return Err(Error::Manifest);
    }
    let mut unique = std::collections::HashSet::new();
    for package in &product.packages {
        let literal = package.replace("${rustc-host}", "");
        if !unique.insert(package) || package.len() > 256 || literal.is_empty() || !literal.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b)) {
            return Err(Error::Manifest);
        }
    }
    cancelled(stop)?;
    let directory = tempfile::Builder::new().prefix("crabcraft-criticalup-manifest-").tempdir().map_err(|_| Error::Io)?;
    let path = directory.path().join("criticalup.toml");
    std::fs::write(&path, &bytes).map_err(|_| Error::Io)?;
    Ok(Snapshot(Arc::new(ManifestData {
        root,
        path,
        release: product.release.clone(),
        packages: product.packages.clone(),
        digest: format!("{:x}", Sha256::digest(&bytes)),
        _directory: directory,
    })))
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Context {
    pub snapshot: Snapshot,
    pub tools: BTreeMap<String, PathBuf>,
    pub version: String,
}
impl Context {
    pub fn tool(&self, name: &str) -> PathBuf {
        self.tools.get(name).cloned().unwrap_or_else(|| self.snapshot.path().with_file_name(format!("missing-{name}")))
    }
}
fn manager_path(program: &Path) -> Result<PathBuf, Error> {
    if !program.is_absolute() {
        return Err(Error::Path);
    }
    let path = program.canonicalize().map_err(|_| Error::Path)?;
    let name = path.file_name().and_then(|s| s.to_str()).ok_or(Error::Path)?;
    if !name.eq_ignore_ascii_case(if cfg!(windows) { "criticalup.exe" } else { "criticalup" }) || !path.is_file() {
        return Err(Error::Path);
    }
    Ok(path)
}
struct Group(Option<u32>);
impl Drop for Group {
    fn drop(&mut self) {
        if let Some(pid) = self.0 {
            rc_dotnet::kill_tree(pid);
        }
    }
}
async fn limited<R: tokio::io::AsyncRead + Unpin>(reader: R) -> Result<Vec<u8>, Error> {
    let mut bytes = Vec::new();
    reader.take(MANIFEST as u64 + 1).read_to_end(&mut bytes).await.map_err(|_| Error::Io)?;
    if bytes.len() > MANIFEST {
        return Err(Error::Limit);
    }
    Ok(bytes)
}
async fn query(program: &Path, args: &[std::ffi::OsString], root: &Path, stop: &CancelToken) -> Result<String, Error> {
    cancelled(stop)?;
    let mut cmd = tokio::process::Command::new(program);
    cmd.args(args)
        .current_dir(root)
        .env_remove("RUSTUP_TOOLCHAIN")
        .env_remove("CRITICALUP_CURRENT_PROJ_MANIFEST_CANONICAL_PATH")
        .env_remove("CRITICALUP_TEST_OVERRIDE_ARG")
        .env_remove("CRITICALUP_TOKEN")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    cmd.process_group(0);
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000);
    let mut child = cmd.spawn().map_err(|_| Error::Io)?;
    let group = Group(child.id());
    let stdout = child.stdout.take().ok_or(Error::Io)?;
    let stderr = child.stderr.take().ok_or(Error::Io)?;
    let work = async {
        let (out, err) = tokio::try_join!(limited(stdout), limited(stderr))?;
        let status = child.wait().await.map_err(|_| Error::Io)?;
        if !status.success() {
            return Err(Error::Manager { code: status.code(), message: String::from_utf8_lossy(&err).chars().take(4096).collect() });
        }
        String::from_utf8(if out.is_empty() { err } else { out }).map_err(|_| Error::Io)
    };
    let result =
        tokio::select! {result=tokio::time::timeout(Duration::from_secs(15),work)=>result.map_err(|_|Error::Limit)?,()=stop.cancelled()=>Err(Error::Cancelled)};
    drop(group);
    let _ = child.start_kill();
    let _ = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;
    result
}
async fn which(program: &Path, snapshot: &Snapshot, name: &str, stop: &CancelToken) -> Result<PathBuf, Error> {
    let args = vec!["which".into(), "--project".into(), snapshot.path().as_os_str().to_owned(), name.into()];
    let output = query(program, &args, snapshot.root(), stop).await?;
    let output = output.trim();
    if output.lines().count() != 1 || output.len() > 4096 {
        return Err(Error::ToolMismatch);
    }
    let path = Path::new(output);
    if !path.is_absolute() {
        return Err(Error::ToolMismatch);
    }
    let path = path.canonicalize().map_err(|_| Error::ToolMismatch)?;
    let filename = path.file_name().and_then(|s| s.to_str()).ok_or(Error::ToolMismatch)?;
    let expected = format!("{name}{}", std::env::consts::EXE_SUFFIX);
    if !filename.eq_ignore_ascii_case(&expected) || !path.is_file() {
        return Err(Error::ToolMismatch);
    }
    Ok(path)
}
/// Resolves an already installed toolchain only, after explicit application trust.
pub async fn activate(program: PathBuf, snapshot: Snapshot, trusted: bool, stop: CancelToken) -> Result<Cargo, Error> {
    if !trusted {
        return Err(Error::Trust);
    }
    cancelled(&stop)?;
    let program = manager_path(&program)?;
    let version = query(&program, &["--version".into()], snapshot.root(), &stop).await?;
    let version_number = version.split_whitespace().find_map(|part| semver::Version::parse(part).ok()).ok_or(Error::Version)?;
    if !version.starts_with("criticalup ") || version_number < semver::Version::new(1, 4, 0) {
        return Err(Error::Version);
    }
    let cargo = which(&program, &snapshot, "cargo", &stop).await?;
    let rustc = which(&program, &snapshot, "rustc", &stop).await?;
    if cargo.parent() != rustc.parent() {
        return Err(Error::ToolMismatch);
    }
    let mut tools = BTreeMap::from([("cargo".into(), cargo), ("rustc".into(), rustc)]);
    for name in ["rustfmt", "rustdoc"] {
        match which(&program, &snapshot, name, &stop).await {
            Ok(path) => {
                if path.parent() != tools.get("rustc").and_then(|p| p.parent()) {
                    return Err(Error::ToolMismatch);
                }
                tools.insert(name.into(), path);
            }
            Err(Error::Cancelled) => return Err(Error::Cancelled),
            Err(_) => {}
        }
    }
    cancelled(&stop)?;
    Ok(Cargo::from_criticalup(program, Arc::new(Context { snapshot, tools, version: version.trim().into() })))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn manifest(root: &Path) {
        std::fs::write(root.join("criticalup.toml"),"manifest-version = 1\n[products.ferrocene]\nrelease = \"fixture-1\"\npackages = [\"cargo-${rustc-host}\",\"rustc-${rustc-host}\",\"rustfmt-${rustc-host}\"]\n").unwrap();
    }
    #[cfg(unix)]
    pub(crate) fn tools() -> BTreeMap<String, PathBuf> {
        let cargo = Cargo::locate().unwrap();
        let rustup = cargo.path().with_file_name("rustup");
        let neutral = tempfile::tempdir().unwrap();
        let mut result = BTreeMap::new();
        for name in ["cargo", "rustc", "rustdoc", "rustfmt"] {
            let path = if rustup.is_file() {
                let output =
                    std::process::Command::new(&rustup).args(["which", name]).current_dir(neutral.path()).env("RUSTUP_AUTO_INSTALL", "0").output().unwrap();
                if !output.status.success() {
                    continue;
                }
                PathBuf::from(String::from_utf8(output.stdout).unwrap().trim())
            } else {
                cargo.path().with_file_name(name)
            };
            if path.is_file() {
                result.insert(name.into(), path.canonicalize().unwrap());
            }
        }
        result
    }
    #[cfg(unix)]
    pub(crate) fn manager(root: &Path, tools: &BTreeMap<String, PathBuf>, missing: bool, slow: bool) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let manager = root.join("manager");
        std::fs::create_dir_all(&manager).unwrap();
        let program = manager.join("criticalup");
        let paths = serde_json::to_string(tools).unwrap();
        let wire = serde_json::to_string(&root.join("criticalup-wire").to_string_lossy()).unwrap();
        let script = format!(
            r#"#!/usr/bin/python3
import json,os,sys,time
with open({wire},'a') as f:f.write(json.dumps({{'args':sys.argv[1:],'rustc':os.environ.get('RUSTC'),'wrapper':os.environ.get('RUSTC_WRAPPER')}})+'\n')
{sleep}
tools=json.loads({paths:?})
a=sys.argv[1:]
if a==['--version']:print('criticalup 1.6.0')
elif a[0]=='which':
 if {missing} or a[-1] not in tools:print('toolchain not installed',file=sys.stderr);sys.exit(1)
 print(tools[a[-1]])
elif a[0]=='run':
 if '--strict' not in a or '--project' not in a:sys.exit(4)
 i=a.index('--project');p=a[i+1];name=a[i+2];args=a[i+3:]
 if 'fixture-1' not in open(p).read():sys.exit(5)
 if name not in tools:sys.exit(6)
 os.execve(tools[name],[tools[name]]+args,os.environ)
else:sys.exit(7)
"#,
            sleep = if slow { "time.sleep(60)" } else { "" },
            missing = if missing { "True" } else { "False" }
        );
        std::fs::write(&program, script).unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        program
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn strict_prefix_freezes_manifest_and_keeps_cargo_rustc_and_formatter_coherent() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        manifest(&root);
        std::fs::create_dir(root.join("src")).unwrap();
        std::fs::write(root.join("Cargo.toml"), "[package]\nname=\"criticalup_provider_fixture\"\nversion=\"0.1.0\"\nedition=\"2024\"\n").unwrap();
        std::fs::write(root.join("src/main.rs"), "fn main(){println!(\"42\");}").unwrap();
        std::fs::write(root.join("build.rs"), "fn main(){std::fs::write(\"build-ran\",\"yes\").unwrap();}").unwrap();
        let tools = tools();
        let program = manager(&root, &tools, false, false);
        let snapshot = inspect(&root, &CancelToken::new()).unwrap();
        assert!(!root.join("criticalup-wire").exists());
        assert!(matches!(activate(program.clone(), snapshot.clone(), false, CancelToken::new()).await, Err(Error::Trust)));
        assert!(!root.join("criticalup-wire").exists());
        let cargo = activate(program, snapshot, true, CancelToken::new()).await.unwrap();
        assert!(cargo.is_criticalup());
        assert_eq!(cargo.rustc_path(), tools["rustc"]);
        assert!(crate::rustup_path(&cargo).is_none());
        std::fs::write(root.join("criticalup.toml"), "changed after selection").unwrap();
        let poison = root.join("poison");
        std::fs::write(&poison, format!("#!/bin/sh\ntouch {:?}\nexit 70\n", root.join("wrong-compiler"))).unwrap();
        std::fs::set_permissions(&poison, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::create_dir(root.join(".cargo")).unwrap();
        std::fs::write(
            root.join(".cargo/config.toml"),
            format!("[build]\nrustc = {:?}\nrustc-wrapper = {:?}\nrustc-workspace-wrapper = {:?}\n", poison, poison, poison),
        )
        .unwrap();
        let cargo = cargo.with_env(vec![
            ("RUSTC".into(), poison.to_string_lossy().into()),
            ("RUSTUP_TOOLCHAIN".into(), "never-use-rustup".into()),
            ("RUSTC_WRAPPER".into(), poison.to_string_lossy().into()),
            ("CARGO_NET_OFFLINE".into(), "true".into()),
            ("CARGO_HOME".into(), root.join("cargo-home").to_string_lossy().into()),
        ]);
        let metadata = crate::load_workspace(&cargo, &root.join("Cargo.toml")).unwrap();
        assert_eq!(metadata.packages[0].name, "criticalup_provider_fixture");
        assert!(!root.join("build-ran").exists());
        let output = cargo.std_command().args(["build", "--offline"]).current_dir(&root).output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        assert!(root.join("build-ran").exists());
        assert!(!root.join("wrong-compiler").exists());
        let cfg = crate::print_cfg(&cargo, &root, None).unwrap();
        assert!(cfg.contains("target_arch"));
        assert!(crate::triple::host_triple(&cargo).is_some());
        let formatter = crate::Rustfmt::new(&cargo, &rc_core::RustfmtSettings::default()).unwrap();
        assert!(formatter.format(&root.join("src/main.rs"), "2024", "fn main(){println!(\"42\");}").await.unwrap().contains("fn main() {"));
        assert!(crate::Rustfmt::new(&cargo, &rc_core::RustfmtSettings { channel: "nightly".into(), ..Default::default() }).is_err());
        let wire = std::fs::read_to_string(root.join("criticalup-wire")).unwrap();
        assert!(wire.contains("--strict") && wire.contains("--project"));
        assert!(wire.contains("rustfmt") && wire.contains("rustc"));
        let mut unsupported = cargo.command_with_args(&["llvm-cov".into()]);
        assert!(unsupported.output().await.is_err());
        assert!(!root.join("wrong-compiler").exists());
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn manager_proxy_names_missing_toolchains_and_cancellation_fail_without_rustup_fallback() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        manifest(&root);
        let tools = tools();
        let program = manager(&root, &tools, false, false);
        let proxy = root.join("cargo");
        symlink(&program, &proxy).unwrap();
        let snapshot = inspect(&root, &CancelToken::new()).unwrap();
        let cargo = activate(proxy, snapshot.clone(), true, CancelToken::new()).await.unwrap();
        assert_eq!(cargo.path(), program.canonicalize().unwrap());
        std::fs::create_dir(root.join("hardlinks")).unwrap();
        let hardlink = root.join("hardlinks/cargo");
        std::fs::hard_link(&program, &hardlink).unwrap();
        assert!(matches!(activate(hardlink, snapshot.clone(), true, CancelToken::new()).await, Err(Error::Path)));
        let missing_dir = tempfile::tempdir().unwrap();
        let missing = manager(missing_dir.path(), &tools, true, false);
        assert!(matches!(activate(missing, snapshot.clone(), true, CancelToken::new()).await, Err(Error::Manager { .. })));
        let slow_dir = tempfile::tempdir().unwrap();
        let slow = manager(slow_dir.path(), &tools, false, true);
        let stop = CancelToken::new();
        let task = tokio::spawn(activate(slow, snapshot, true, stop.clone()));
        tokio::time::sleep(Duration::from_millis(30)).await;
        stop.cancel();
        assert!(matches!(tokio::time::timeout(Duration::from_secs(3), task).await.unwrap().unwrap(), Err(Error::Cancelled)));
    }
    #[cfg(unix)]
    #[test]
    fn manifest_rejects_symlink_fifo_oversize_unknown_fields_and_wrong_products() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        manifest(&root);
        let path = root.join("criticalup.toml");
        let original = std::fs::read_to_string(&path).unwrap();
        for invalid in [
            original.replace("manifest-version = 1", "manifest-version = 2"),
            original.replace("products.ferrocene", "products.other"),
            format!("{original}\nunexpected = true\n"),
            original.replace("cargo-${rustc-host}", "../cargo"),
        ] {
            std::fs::write(&path, invalid).unwrap();
            assert!(matches!(inspect(&root, &CancelToken::new()), Err(Error::Manifest)));
        }
        std::fs::write(&path, vec![b'x'; MANIFEST + 1]).unwrap();
        assert!(matches!(inspect(&root, &CancelToken::new()), Err(Error::Limit)));
        std::fs::remove_file(&path).unwrap();
        let source = root.join("source.toml");
        std::fs::write(&source, original).unwrap();
        symlink(source, &path).unwrap();
        assert!(matches!(inspect(&root, &CancelToken::new()), Err(Error::Path)));
        std::fs::remove_file(&path).unwrap();
        use std::os::unix::ffi::OsStrExt;
        let c = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        let start = std::time::Instant::now();
        assert!(matches!(inspect(&root, &CancelToken::new()), Err(Error::Path)));
        assert!(start.elapsed() < Duration::from_secs(1));
    }
    #[cfg(unix)]
    #[tokio::test]
    #[ignore = "Requires the separately downloaded official CriticalUp CLI; isolated state only, no authentication/install/compiler"]
    async fn official_manager_without_a_toolchain_reports_error_instead_of_running_rustup() {
        use std::os::unix::fs::PermissionsExt;
        let official = PathBuf::from(std::env::var("CRABCRAFT_TEST_CRITICALUP").expect("explicit official CLI"));
        assert!(official.is_absolute());
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        manifest(&root);
        let program = root.join("criticalup");
        let state = root.join("state");
        let cache = root.join("cache");
        let script = format!(
            "#!/usr/bin/python3\nimport os,sys\nenv=os.environ.copy();env['CRITICALUP_ROOT']={state:?};env['CRITICALUP_CACHE_DIR']={cache:?}\nos.execve({program:?},[{program:?}]+sys.argv[1:],env)\n",
            state = state.to_string_lossy(),
            cache = cache.to_string_lossy(),
            program = official.to_string_lossy()
        );
        std::fs::write(&program, script).unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        let snapshot = inspect(&root, &CancelToken::new()).unwrap();
        let error = activate(program, snapshot, true, CancelToken::new()).await.unwrap_err();
        assert!(
            matches!(&error,Error::Manager{message,..}if message.to_lowercase().contains("install")||message.to_lowercase().contains("toolchain")),
            "{error:?}"
        );
    }
}
