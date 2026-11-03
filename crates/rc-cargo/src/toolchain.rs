//! Finds the `cargo` executable and reads the toolchain's version.

use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cargo {
    path: PathBuf,
    /// The project's environment, set on every command.
    env: Vec<(String, String)>,
}

impl Cargo {
    pub fn new(path: impl Into<PathBuf>) -> Cargo {
        Cargo { path: path.into(), env: Vec::new() }
    }

    /// The same `cargo`, with `env` set on every command it starts.
    pub fn with_env(mut self, env: Vec<(String, String)>) -> Cargo {
        self.env = env;
        self
    }

    pub fn env(&self) -> &[(String, String)] {
        &self.env
    }

    /// The first `cargo` found: `$PATH`, then `~/.cargo/bin` (apps opened from a desktop launcher
    /// often have a `PATH` without it).
    pub fn locate() -> Option<Cargo> {
        candidates(std::env::var_os("PATH"), home()).into_iter().find(|p| p.is_file()).map(Cargo::new)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn command(&self) -> tokio::process::Command {
        let mut cmd = tokio::process::Command::new(&self.path);
        cmd.envs(self.env.iter().map(|(k, v)| (k, v)));
        cmd
    }

    pub fn std_command(&self) -> std::process::Command {
        let mut cmd = std::process::Command::new(&self.path);
        cmd.envs(self.env.iter().map(|(k, v)| (k, v)));
        cmd
    }

    /// `cargo 1.99.0 (...)`, as printed.
    pub fn version(&self) -> anyhow::Result<String> {
        let out = self.std_command().arg("--version").output()?;
        anyhow::ensure!(out.status.success(), "cargo --version failed");
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }
}

/// What is known about the Rust toolchain in use; `None` for what could not be found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ToolchainInfo {
    pub cargo: Option<String>,
    pub rustc: Option<String>,
    pub rust_analyzer: Option<String>,
    pub stdlib: Option<PathBuf>,
}

/// Asks each tool for its version. `project` is the folder whose `rust-toolchain` file applies;
/// only pass a trusted one, since rustup may download what that file names.
pub fn detect_toolchain(cargo: Option<&Cargo>, analyzer: Option<&Path>, project: Option<&Path>) -> ToolchainInfo {
    let output = |program: &Path, args: &[&str]| {
        let mut cmd = std::process::Command::new(program);
        cmd.args(args);
        if let Some(dir) = project {
            cmd.current_dir(dir);
        }
        let out = cmd.output().ok().filter(|o| o.status.success())?;
        Some(String::from_utf8_lossy(&out.stdout).trim().to_string()).filter(|s| !s.is_empty())
    };
    let Some(cargo) = cargo else { return ToolchainInfo::default() };
    let rustc = cargo.path().with_file_name(exe("rustc"));
    ToolchainInfo {
        cargo: output(cargo.path(), &["--version"]),
        rustc: output(&rustc, &["--version"]),
        rust_analyzer: analyzer.and_then(|a| output(a, &["--version"])),
        stdlib: output(&rustc, &["--print", "sysroot"]).and_then(|s| stdlib_sources(Path::new(&s))),
    }
}

/// What `rustc --print cfg` prints for `target` (`None`: the host), run from `dir` so a
/// `rust-toolchain.toml` there picks the compiler. It compiles nothing, and works for targets
/// that are not installed. An invalid triple gives `None`. This blocks.
pub fn print_cfg(cargo: &Cargo, dir: &Path, target: Option<&str>) -> Option<String> {
    let rustc = cargo.path().with_file_name(exe("rustc"));
    let mut cmd = std::process::Command::new(rustc);
    cmd.args(["--print", "cfg"]);
    if let Some(t) = target {
        if !crate::triple::valid_triple(t) {
            return None;
        }
        cmd.args(["--target", t]);
    }
    let out = cmd.current_dir(dir).stdin(std::process::Stdio::null()).output().ok().filter(|o| o.status.success())?;
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The sources of the standard library inside a sysroot, from the `rust-src` component.
pub fn stdlib_sources(sysroot: &Path) -> Option<PathBuf> {
    Some(sysroot.join("lib/rustlib/src/rust/library")).filter(|p| p.is_dir())
}

/// `rustup`, next to `cargo` (where rustup installs its proxies).
pub fn rustup_path(cargo: &Cargo) -> Option<PathBuf> {
    Some(cargo.path().with_file_name(exe("rustup"))).filter(|p| p.is_file())
}

/// One line of `rustup check`: a toolchain (or rustup itself) and its newer version, if any.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolchainUpdate {
    pub name: String,
    pub current: String,
    /// The version an update would install; `None` when it is up to date.
    pub available: Option<String>,
}

/// Reads `rustup check`: `stable-x86_64-unknown-linux-gnu - up to date: 1.99.1 (…)` and
/// `rustup - update available : 1.29.2 -> 1.29.3`. Other lines are skipped.
pub fn parse_rustup_check(text: &str) -> Vec<ToolchainUpdate> {
    let version = |s: &str| s.split_whitespace().next().unwrap_or_default().to_string();
    text.lines()
        .filter_map(|line| {
            let (name, status) = line.split_once(" - ")?;
            let (state, versions) = status.split_once(':')?;
            let state = state.trim().to_ascii_lowercase();
            let name = name.trim().to_string();
            if state == "up to date" {
                Some(ToolchainUpdate { name, current: version(versions), available: None })
            } else if state == "update available" {
                let (current, new) = versions.split_once("->")?;
                Some(ToolchainUpdate { name, current: version(current), available: Some(version(new)) })
            } else {
                None
            }
        })
        .collect()
}

/// Asks rustup which toolchains have updates. It reads the network: run it on a thread, and
/// only when the user asks.
pub fn rustup_check(rustup: &Path) -> anyhow::Result<Vec<ToolchainUpdate>> {
    let out = std::process::Command::new(rustup).arg("check").stdin(std::process::Stdio::null()).output()?;
    let list = parse_rustup_check(&String::from_utf8_lossy(&out.stdout));
    // rustup exits with 100 when there are updates.
    anyhow::ensure!(!list.is_empty() || out.status.success(), "{}", String::from_utf8_lossy(&out.stderr).trim());
    Ok(list)
}

fn home() -> Option<PathBuf> {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from)
}

fn exe(name: &str) -> String {
    if cfg!(windows) { format!("{name}.exe") } else { name.to_string() }
}

/// Where to look for `cargo`, in order.
pub fn candidates(path_var: Option<std::ffi::OsString>, home: Option<PathBuf>) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = path_var.iter().flat_map(std::env::split_paths).map(|d| d.join(exe("cargo"))).collect();
    out.extend(home.map(|h| h.join(".cargo").join("bin").join(exe("cargo"))));
    out
}

/// `rust-analyzer`, from `$PATH`, `~/.cargo/bin` or the active rustup toolchain. A rustup proxy
/// for a component that is not installed exists but fails, so each candidate is tried.
pub fn locate_rust_analyzer(configured: Option<&Path>) -> Option<PathBuf> {
    if let Some(p) = configured.filter(|p| p.is_file()) {
        return Some(p.to_path_buf());
    }
    let dirs = candidates(std::env::var_os("PATH"), home());
    dirs.into_iter().filter_map(|p| p.parent().map(|d| d.join(exe("rust-analyzer")))).filter(|p| p.is_file()).find(|p| {
        std::process::Command::new(p).arg("--version").output().is_ok_and(|o| o.status.success())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_rustup_check() {
        let text = "stable-x86_64-unknown-linux-gnu - up to date: 1.99.1 (58cf8f9c3 2026-10-13)\nnightly-x86_64-unknown-linux-gnu - Update available : 1.100.0-nightly (aaa 2026-10-01) -> 1.100.0-nightly (bbb 2026-11-02)\nrustup - update available : 1.29.2 -> 1.29.3\ninfo: noise\n";
        let list = parse_rustup_check(text);
        assert_eq!(list.len(), 3);
        assert_eq!(list[0], ToolchainUpdate { name: "stable-x86_64-unknown-linux-gnu".into(), current: "1.99.1".into(), available: None });
        assert_eq!(list[1].available.as_deref(), Some("1.100.0-nightly"));
        assert_eq!(list[2], ToolchainUpdate { name: "rustup".into(), current: "1.29.2".into(), available: Some("1.29.3".into()) });
        assert!(parse_rustup_check("error: no network").is_empty());
    }

    #[test]
    fn path_comes_before_the_cargo_home() {
        let list = candidates(Some(std::env::join_paths(["/a", "/b"]).unwrap()), Some("/home/u".into()));
        assert_eq!(list[0], Path::new("/a").join(exe("cargo")));
        assert_eq!(list[1], Path::new("/b").join(exe("cargo")));
        assert_eq!(list[2], Path::new("/home/u/.cargo/bin").join(exe("cargo")));
    }

    #[test]
    fn reads_the_versions_of_the_real_toolchain() {
        let cargo = Cargo::locate().expect("cargo is installed");
        let info = detect_toolchain(Some(&cargo), None, None);
        assert!(info.cargo.as_deref().is_some_and(|v| v.starts_with("cargo ")), "{info:?}");
        assert!(info.rustc.as_deref().is_some_and(|v| v.starts_with("rustc ")), "{info:?}");
        assert_eq!(info.rust_analyzer, None);
    }

    #[test]
    fn a_broken_toolchain_reports_nothing_instead_of_failing() {
        let dir = tempfile::tempdir().unwrap();
        let cargo = Cargo::new(dir.path().join("missing"));
        assert_eq!(detect_toolchain(Some(&cargo), Some(&dir.path().join("none")), Some(dir.path())), ToolchainInfo::default());
        assert_eq!(detect_toolchain(None, None, None), ToolchainInfo::default());
    }

    #[test]
    fn the_standard_library_needs_its_sources() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(stdlib_sources(dir.path()), None);
        let library = dir.path().join("lib/rustlib/src/rust/library");
        std::fs::create_dir_all(&library).unwrap();
        assert_eq!(stdlib_sources(dir.path()), Some(library));
    }

    #[test]
    fn nothing_to_search_gives_no_candidates() {
        assert!(candidates(None, None).is_empty());
    }
}
