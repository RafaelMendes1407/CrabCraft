//! Official rustup bootstrap, prepared on a worker after explicit user confirmation.
//! The binary and its mandatory SHA-256 sidecar come from static.rust-lang.org over TLS.
use rc_dotnet::CancelToken;
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::time::Duration;

const ROOT: &str = "https://static.rust-lang.org/rustup/dist";
const MAX_INSTALLER: usize = 64 * 1024 * 1024;
#[derive(Debug)]
pub enum InstallError {
    Unsupported,
    Home,
    Cancelled,
    Network(String),
    TooLarge,
    Checksum,
    Io(std::io::Error),
}
impl From<std::io::Error> for InstallError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

pub fn host() -> Result<&'static str, InstallError> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Ok(if cfg!(target_env = "musl") { "x86_64-unknown-linux-musl" } else { "x86_64-unknown-linux-gnu" }),
        ("linux", "aarch64") => Ok(if cfg!(target_env = "musl") { "aarch64-unknown-linux-musl" } else { "aarch64-unknown-linux-gnu" }),
        ("macos", "x86_64") => Ok("x86_64-apple-darwin"),
        ("macos", "aarch64") => Ok("aarch64-apple-darwin"),
        ("windows", "x86_64") => Ok("x86_64-pc-windows-msvc"),
        ("windows", "aarch64") => Ok("aarch64-pc-windows-msvc"),
        _ => Err(InstallError::Unsupported),
    }
}
pub fn url() -> Result<String, InstallError> {
    Ok(format!("{ROOT}/{}/rustup-init{}", host()?, std::env::consts::EXE_SUFFIX))
}
pub fn arguments(stable: bool) -> Vec<String> {
    let mut args: Vec<String> = ["-y", "--no-modify-path", "--profile", "minimal", "--default-toolchain", if stable { "stable" } else { "none" }]
        .into_iter()
        .map(str::to_string)
        .collect();
    if stable {
        args.extend(["--component", "rust-src", "--component", "rust-analyzer"].into_iter().map(str::to_string));
    }
    args
}
#[derive(Clone, Debug)]
pub struct Destinations {
    pub cargo: PathBuf,
    pub rustup: PathBuf,
}
pub fn destinations() -> Result<Destinations, InstallError> {
    let home = crate::toolchain::home().ok_or(InstallError::Home)?;
    let cargo = std::env::var_os("CARGO_HOME").filter(|s| !s.is_empty()).map(PathBuf::from).unwrap_or_else(|| home.join(".cargo"));
    let rustup = std::env::var_os("RUSTUP_HOME").filter(|s| !s.is_empty()).map(PathBuf::from).unwrap_or_else(|| home.join(".rustup"));
    // Rustup's relative homes depend on cwd. Freeze absolute paths before displaying them.
    let absolute = |path: PathBuf| -> Result<PathBuf, InstallError> { if path.is_absolute() { Ok(path) } else { Ok(std::env::current_dir()?.join(path)) } };
    let destinations = Destinations { cargo: absolute(cargo)?, rustup: absolute(rustup)? };
    if destinations.cargo.to_str().is_none() || destinations.rustup.to_str().is_none() {
        return Err(InstallError::Home);
    }
    Ok(destinations)
}

pub struct Prepared {
    directory: tempfile::TempDir,
    pub executable: PathBuf,
}
impl Prepared {
    /// An installer runs outside every project, with explicit installation homes and official
    /// distribution roots. No project settings or environment are copied into the plan.
    pub fn plan(&self, stable: bool, destinations: &Destinations) -> rc_dotnet::LaunchPlan {
        rc_dotnet::LaunchPlan {
            program: self.executable.clone(),
            args: arguments(stable),
            working_dir: self.directory.path().to_path_buf(),
            env: vec![
                ("CARGO_HOME".into(), destinations.cargo.to_string_lossy().into_owned()),
                ("RUSTUP_HOME".into(), destinations.rustup.to_string_lossy().into_owned()),
                ("RUSTUP_DIST_SERVER".into(), "https://static.rust-lang.org".into()),
                ("RUSTUP_UPDATE_ROOT".into(), "https://static.rust-lang.org/rustup".into()),
            ],
            urls: Vec::new(),
        }
    }
}
fn verify(binary: &[u8], checksum: &[u8]) -> Result<(), InstallError> {
    let text = std::str::from_utf8(checksum).map_err(|_| InstallError::Checksum)?;
    let hash = text.split_whitespace().next().filter(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())).ok_or(InstallError::Checksum)?;
    let actual = format!("{:x}", Sha256::digest(binary));
    if actual.eq_ignore_ascii_case(hash) { Ok(()) } else { Err(InstallError::Checksum) }
}
async fn read(http: &reqwest::Client, url: &str, max: usize) -> Result<Vec<u8>, InstallError> {
    let mut response =
        http.get(url).send().await.map_err(|e| InstallError::Network(e.to_string()))?.error_for_status().map_err(|e| InstallError::Network(e.to_string()))?;
    if response.content_length().is_some_and(|size| size > max as u64) {
        return Err(InstallError::TooLarge);
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|e| InstallError::Network(e.to_string()))? {
        if bytes.len().saturating_add(chunk.len()) > max {
            return Err(InstallError::TooLarge);
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}
/// Blocks on a private runtime: call from a worker. No file is written until both downloads
/// pass verification, and no executable is returned after cancellation or a checksum failure.
pub fn prepare(cancel: &CancelToken) -> Result<Prepared, InstallError> {
    let url = url()?;
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    runtime.block_on(async {
        let work = async {
            let http = reqwest::Client::builder()
                .https_only(true)
                .user_agent(crate::crates_io::USER_AGENT)
                .connect_timeout(Duration::from_secs(15))
                .timeout(Duration::from_secs(120))
                .redirect(reqwest::redirect::Policy::limited(3))
                .build()
                .map_err(|e| InstallError::Network(e.to_string()))?;
            let checksum = read(&http, &format!("{url}.sha256"), 4096).await?;
            let binary = read(&http, &url, MAX_INSTALLER).await?;
            verify(&binary, &checksum)?;
            if cancel.is_cancelled() {
                return Err(InstallError::Cancelled);
            }
            let directory = tempfile::Builder::new().prefix("crabcraft-rustup-").tempdir()?;
            let executable = directory.path().join(format!("rustup-init{}", std::env::consts::EXE_SUFFIX));
            std::fs::write(&executable, binary)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700))?;
            }
            Ok(Prepared { directory, executable })
        };
        tokio::select! { biased; () = cancel.cancelled() => Err(InstallError::Cancelled), result = work => result }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn refuses_modified_installers_and_preserves_fixed_install_options() {
        // Published SHA-256 test vector for "abc".
        let hash = b"ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad  rustup-init\n";
        assert!(verify(b"abc", hash).is_ok());
        assert!(matches!(verify(b"modified", hash), Err(InstallError::Checksum)));
        assert!(matches!(verify(b"abc", b"not a hash"), Err(InstallError::Checksum)));
        assert!(arguments(true).contains(&"--no-modify-path".to_string()));
        assert!(arguments(false).contains(&"none".to_string()));
        let cancel = CancelToken::new();
        cancel.cancel();
        assert!(matches!(prepare(&cancel), Err(InstallError::Cancelled)));
    }
    #[test]
    #[cfg(unix)]
    #[ignore = "Downloads the verified official installer and installs only into temporary homes"]
    fn official_installer_sets_up_isolated_proxies_that_test_a_real_rust_project() {
        let prepared = prepare(&CancelToken::new()).unwrap();
        let root = tempfile::tempdir().unwrap();
        let destinations = Destinations { cargo: root.path().join("cargo"), rustup: root.path().join("rustup") };
        let plan = prepared.plan(false, &destinations);
        let output = std::process::Command::new(&plan.program).args(&plan.args).envs(plan.env.iter().cloned()).current_dir(&plan.working_dir).output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        let rustup = destinations.cargo.join("bin").join(format!("rustup{}", std::env::consts::EXE_SUFFIX));
        assert!(rustup.is_file());
        let sysroot = std::process::Command::new("rustc").args(["--print", "sysroot"]).output().unwrap();
        assert!(sysroot.status.success());
        let sysroot = String::from_utf8_lossy(&sysroot.stdout).trim().to_string();
        let output =
            std::process::Command::new(&rustup).args(["toolchain", "link", "crabcraft-test", &sysroot]).envs(plan.env.iter().cloned()).output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        let project = root.path().join("project");
        std::fs::create_dir_all(project.join("src")).unwrap();
        std::fs::write(project.join("Cargo.toml"), "[package]\nname = \"rustup_fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n").unwrap();
        std::fs::write(project.join("src/lib.rs"), "pub fn value() -> u32 { 42 }\n#[test] fn works() { assert_eq!(value(), 42); }\n").unwrap();
        let cargo = destinations.cargo.join("bin").join(format!("cargo{}", std::env::consts::EXE_SUFFIX));
        let output = std::process::Command::new(cargo)
            .args(["+crabcraft-test", "test", "--offline"])
            .envs(plan.env.iter().cloned())
            .current_dir(project)
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    }
}
