//! Finds the `cargo` executable and reads the toolchain's version.

use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cargo {
    path: PathBuf,
    /// The project's environment, set on every command.
    env: Vec<(String, String)>,
    criticalup: Option<std::sync::Arc<crate::criticalup::Context>>,
}

impl Cargo {
    pub fn new(path: impl Into<PathBuf>) -> Cargo {
        Cargo { path: path.into(), env: Vec::new(), criticalup: None }
    }

    /// The same `cargo`, with `env` set on every command it starts.
    pub fn with_env(mut self, env: Vec<(String, String)>) -> Cargo {
        self.env = self.provider_env(env);
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

    pub(crate) fn from_criticalup(path: PathBuf, context: std::sync::Arc<crate::criticalup::Context>) -> Self {
        let mut cargo = Self { path, env: Vec::new(), criticalup: Some(context) };
        cargo.env = cargo.provider_env(Vec::new());
        cargo
    }
    pub fn is_criticalup(&self) -> bool {
        self.criticalup.is_some()
    }
    pub fn criticalup_release(&self) -> Option<&str> {
        self.criticalup.as_ref().map(|context| context.snapshot.release())
    }
    pub fn criticalup_manifest(&self) -> Option<&Path> {
        self.criticalup.as_ref().map(|context| context.snapshot.path())
    }
    pub fn tool_path(&self, name: &str) -> Option<PathBuf> {
        if let Some(context) = &self.criticalup {
            context.tools.get(name).cloned()
        } else {
            Some(if name == "cargo" { self.path.clone() } else { self.path.with_file_name(exe(name)) })
        }
    }
    pub fn rustc_path(&self) -> PathBuf {
        self.criticalup.as_ref().map_or_else(|| self.path.with_file_name(exe("rustc")), |context| context.tool("rustc"))
    }
    fn compiler_override(key: &str) -> bool {
        if key.to_ascii_uppercase().starts_with("CRITICALUP_") {
            return true;
        }
        ["RUSTC", "RUSTDOC", "CARGO", "RUSTUP_TOOLCHAIN", "RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER"].iter().any(|name| key.eq_ignore_ascii_case(name))
    }
    fn provider_env(&self, mut env: Vec<(String, String)>) -> Vec<(String, String)> {
        if let Some(context) = &self.criticalup {
            env.retain(|(key, _)| !Self::compiler_override(key));
            for (key, tool) in [("RUSTC", "rustc"), ("RUSTDOC", "rustdoc"), ("CARGO", "cargo")] {
                env.push((key.into(), context.tool(tool).to_string_lossy().into_owned()));
            }
        }
        env
    }
    pub fn command(&self) -> tokio::process::Command {
        self.tool_command("cargo")
    }
    pub fn supports_cargo_args(&self, args: &[String]) -> bool {
        if !self.is_criticalup() {
            return true;
        }
        matches!(
            args.first().map(String::as_str),
            Some(
                "add"
                    | "remove"
                    | "build"
                    | "check"
                    | "clean"
                    | "clippy"
                    | "doc"
                    | "fetch"
                    | "fix"
                    | "fmt"
                    | "generate-lockfile"
                    | "init"
                    | "install"
                    | "locate-project"
                    | "metadata"
                    | "new"
                    | "package"
                    | "pkgid"
                    | "publish"
                    | "read-manifest"
                    | "run"
                    | "rustc"
                    | "rustdoc"
                    | "search"
                    | "test"
                    | "tree"
                    | "uninstall"
                    | "update"
                    | "vendor"
                    | "verify-project"
                    | "version"
                    | "help"
                    | "--version"
                    | "-V"
                    | "--help"
                    | "-h"
            )
        )
    }
    pub fn command_with_args(&self, args: &[String]) -> tokio::process::Command {
        if !self.supports_cargo_args(args) {
            return tokio::process::Command::new(self.criticalup_manifest().unwrap_or_else(|| Path::new("/")).with_file_name("unsupported-cargo-subcommand"));
        }
        let mut cmd = if self.is_criticalup() {
            match args.first().map(String::as_str) {
                Some("clippy") => self.tool_command("cargo-clippy"),
                Some("fmt") => self.tool_command("cargo-fmt"),
                _ => self.command(),
            }
        } else {
            self.command()
        };
        cmd.args(args);
        cmd
    }
    pub fn std_command(&self) -> std::process::Command {
        self.std_tool_command("cargo")
    }
    pub fn tool_command(&self, name: &str) -> tokio::process::Command {
        let mut cmd = tokio::process::Command::new(if self.criticalup.is_some() {
            self.path.clone()
        } else {
            if name == "cargo" { self.path.clone() } else { self.path.with_file_name(exe(name)) }
        });
        if let Some(context) = &self.criticalup {
            cmd.args(["run", "--strict", "--project"]).arg(context.snapshot.path()).arg(name).current_dir(context.snapshot.root());
            for key in [
                "RUSTUP_TOOLCHAIN",
                "RUSTC_WRAPPER",
                "RUSTC_WORKSPACE_WRAPPER",
                "CRITICALUP_CURRENT_PROJ_MANIFEST_CANONICAL_PATH",
                "CRITICALUP_TEST_OVERRIDE_ARG",
                "CRITICALUP_TOKEN",
            ] {
                cmd.env_remove(key);
            }
        }
        cmd.envs(self.env.iter().map(|(k, v)| (k, v)));
        if self.is_criticalup() {
            cmd.env("RUSTC_WRAPPER", "").env("RUSTC_WORKSPACE_WRAPPER", "");
        }
        cmd
    }
    pub fn std_tool_command(&self, name: &str) -> std::process::Command {
        let mut cmd = std::process::Command::new(if self.criticalup.is_some() {
            self.path.clone()
        } else {
            if name == "cargo" { self.path.clone() } else { self.path.with_file_name(exe(name)) }
        });
        if let Some(context) = &self.criticalup {
            cmd.args(["run", "--strict", "--project"]).arg(context.snapshot.path()).arg(name).current_dir(context.snapshot.root());
            for key in [
                "RUSTUP_TOOLCHAIN",
                "RUSTC_WRAPPER",
                "RUSTC_WORKSPACE_WRAPPER",
                "CRITICALUP_CURRENT_PROJ_MANIFEST_CANONICAL_PATH",
                "CRITICALUP_TEST_OVERRIDE_ARG",
                "CRITICALUP_TOKEN",
            ] {
                cmd.env_remove(key);
            }
        }
        cmd.envs(self.env.iter().map(|(k, v)| (k, v)));
        if self.is_criticalup() {
            cmd.env("RUSTC_WRAPPER", "").env("RUSTC_WORKSPACE_WRAPPER", "");
        }
        cmd
    }

    /// `cargo 1.99.0 (...)`, as printed.
    pub fn version(&self) -> anyhow::Result<String> {
        let out = if self.is_criticalup() { self.std_command().arg("--version").output().ok() } else { neutral_output(&self.path, &["--version"]) }
            .ok_or_else(|| anyhow::anyhow!("cargo --version failed"))?;
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
        let out = if let Some(dir) = project {
            std::process::Command::new(program).args(args).current_dir(dir).output().ok()?
        } else {
            neutral_output(program, args)?
        };
        if !out.status.success() {
            return None;
        }
        Some(String::from_utf8_lossy(&out.stdout).trim().to_string()).filter(|s| !s.is_empty())
    };
    let Some(cargo) = cargo else { return ToolchainInfo::default() };
    if cargo.is_criticalup() {
        let Some(dir) = project else {
            return ToolchainInfo::default();
        };
        let named = |name: &str, args: &[&str]| {
            let out = cargo.std_tool_command(name).args(args).current_dir(dir).stdin(std::process::Stdio::null()).output().ok()?;
            out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string()).filter(|s| !s.is_empty())
        };
        return ToolchainInfo {
            cargo: named("cargo", &["--version"]),
            rustc: named("rustc", &["--version"]),
            rust_analyzer: None,
            stdlib: named("rustc", &["--print", "sysroot"]).and_then(|s| stdlib_sources(Path::new(&s))),
        };
    }
    let rustc = cargo.path().with_file_name(exe("rustc"));
    ToolchainInfo {
        cargo: output(cargo.path(), &["--version"]),
        rustc: output(&rustc, &["--version"]),
        rust_analyzer: analyzer.and_then(|a| output(a, &["--version"])),
        stdlib: output(&rustc, &["--print", "sysroot"]).and_then(|s| stdlib_sources(Path::new(&s))),
    }
}

/// Version discovery must not inherit a project selected by the IDE's launch directory.
/// Rustup's `default` subcommand reads user configuration without invoking a toolchain.
fn neutral_output(program: &Path, args: &[&str]) -> Option<std::process::Output> {
    if !program.is_absolute() {
        return None;
    }
    let dir = tempfile::tempdir().ok()?;
    let mut cmd = std::process::Command::new(program);
    cmd.args(args).current_dir(dir.path()).stdin(std::process::Stdio::null()).env("RUSTUP_AUTO_INSTALL", "0");
    let rustup = program.with_file_name(exe("rustup"));
    if rustup.is_file() {
        let default = std::process::Command::new(rustup)
            .arg("default")
            .current_dir(dir.path())
            .env_remove("RUSTUP_TOOLCHAIN")
            .env("RUSTUP_AUTO_INSTALL", "0")
            .stdin(std::process::Stdio::null())
            .output()
            .ok()?;
        if !default.status.success() {
            return None;
        }
        let stdout = String::from_utf8_lossy(&default.stdout);
        let toolchain = stdout.split_whitespace().next()?;
        cmd.env("RUSTUP_TOOLCHAIN", toolchain);
    }
    cmd.output().ok()
}

/// What `rustc --print cfg` prints for `target` (`None`: the host), run from `dir` so a
/// `rust-toolchain.toml` there picks the compiler. It compiles nothing, and works for targets
/// that are not installed. An invalid triple gives `None`. This blocks.
pub fn print_cfg(cargo: &Cargo, dir: &Path, target: Option<&str>) -> Option<String> {
    let mut cmd = cargo.std_tool_command("rustc");
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
    if cargo.is_criticalup() {
        return None;
    }
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

/// A tool installed where Cargo's are: in `$PATH` or `~/.cargo/bin` (`wasm-pack`...).
pub fn find_tool(name: &str) -> Option<PathBuf> {
    candidates(std::env::var_os("PATH"), home()).into_iter().filter_map(|p| p.parent().map(|d| d.join(exe(name)))).find(|p| p.is_file())
}

/// Whether the standard library for `target` is installed in the sysroot of the toolchain that
/// runs in `dir`. `dir` must not be an untrusted project: rustup installs what its
/// `rust-toolchain` file names. This blocks.
pub fn has_target(cargo: &Cargo, dir: &Path, target: &str) -> bool {
    if !crate::triple::valid_triple(target) {
        return false;
    }
    let rustc = cargo.path().with_file_name(exe("rustc"));
    let Ok(out) = std::process::Command::new(rustc).args(["--print", "sysroot"]).current_dir(dir).stdin(std::process::Stdio::null()).output() else {
        return false;
    };
    let sysroot = String::from_utf8_lossy(&out.stdout).trim().to_string();
    out.status.success() && !sysroot.is_empty() && Path::new(&sysroot).join("lib").join("rustlib").join(target).join("lib").is_dir()
}

pub(crate) fn home() -> Option<PathBuf> {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from)
}

fn exe(name: &str) -> String {
    if cfg!(windows) { format!("{name}.exe") } else { name.to_string() }
}

/// Where to look for `cargo`, in order.
pub fn candidates(path_var: Option<std::ffi::OsString>, home: Option<PathBuf>) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = path_var.iter().flat_map(std::env::split_paths).filter(|d| d.is_absolute()).map(|d| d.join(exe("cargo"))).collect();
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
    dirs.into_iter()
        .filter_map(|p| p.parent().map(|d| d.join(exe("rust-analyzer"))))
        .filter(|p| p.is_file())
        .find(|p| neutral_output(p, &["--version"]).is_some_and(|o| o.status.success()))
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
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a");
        let b = dir.path().join("b");
        let home = dir.path().join("home");
        let list = candidates(Some(std::env::join_paths([&a, &b]).unwrap()), Some(home.clone()));
        assert_eq!(list, vec![a.join(exe("cargo")), b.join(exe("cargo")), home.join(".cargo").join("bin").join(exe("cargo")),]);
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

    #[test]
    fn relative_path_entries_cannot_select_project_executables() {
        let dir = tempfile::tempdir().unwrap();
        let absolute = dir.path().join("bin");
        let mut entries = vec![PathBuf::from("."), PathBuf::from("tools")];
        #[cfg(windows)]
        entries.extend([PathBuf::from(r"\tools"), PathBuf::from("C:tools")]);
        entries.push(absolute.clone());
        let path = std::env::join_paths(entries).unwrap();
        assert_eq!(candidates(Some(path), None), vec![absolute.join(exe("cargo"))]);
    }

    #[cfg(unix)]
    #[test]
    fn neutral_probes_select_the_user_default_and_leave_the_launch_directory() {
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;
        // Concurrent forks can inherit writable script descriptors until exec,
        // causing ETXTBSY. Write in a child and wait before executing the fixture.
        fn write_script(path: &Path, text: &str) -> std::io::Result<()> {
            let mut child =
                std::process::Command::new("/bin/sh").args(["-c", "cat > \"$1\"", "probe-fixture"]).arg(path).stdin(std::process::Stdio::piped()).spawn()?;
            child.stdin.take().unwrap().write_all(text.as_bytes())?;
            if child.wait()?.success() { Ok(()) } else { Err(std::io::Error::other("fixture script writer failed")) }
        }
        let tools = tempfile::tempdir().unwrap();
        let rustup = tools.path().join("rustup");
        write_script(&rustup, "#!/bin/sh\n[ \"$1\" = default ] || exit 1\n[ -z \"$RUSTUP_TOOLCHAIN\" ] || exit 2\nprintf 'safe-default (default)\\n'\n")
            .unwrap();
        std::fs::set_permissions(&rustup, std::fs::Permissions::from_mode(0o755)).unwrap();
        let cargo = tools.path().join("cargo");
        write_script(
            &cargo,
            "#!/bin/sh\n[ \"$RUSTUP_TOOLCHAIN\" = safe-default ] || exit 3\n[ \"$RUSTUP_AUTO_INSTALL\" = 0 ] || exit 4\nprintf '%s\\n' \"$PWD\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&cargo, std::fs::Permissions::from_mode(0o755)).unwrap();
        let output = neutral_output(&cargo, &["--version"]).unwrap();
        assert!(output.status.success());
        let probe_dir = String::from_utf8(output.stdout).unwrap();
        assert_ne!(Path::new(probe_dir.trim()), std::env::current_dir().unwrap());
    }
}
