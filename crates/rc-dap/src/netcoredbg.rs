//! Legacy adapter command discovery. Rust debugging uses GDB or LLDB.

use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdapterCommand {
    pub program: PathBuf,
    pub prefix: Vec<String>,
    pub origin: Origin,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    Settings,
    Path,
    Gdb,
    Lldb,
}

impl AdapterCommand {
    pub fn args(&self) -> Vec<String> {
        let mut args = self.prefix.clone();
        if !matches!(self.origin, Origin::Gdb | Origin::Lldb) {
            args.push("--interpreter=vscode".to_string());
        }
        args
    }

    pub fn new(program: impl Into<PathBuf>, origin: Origin) -> Self {
        Self { program: program.into(), prefix: Vec::new(), origin }
    }
}

fn exe_name() -> &'static str {
    if cfg!(windows) { "netcoredbg.exe" } else { "netcoredbg" }
}

/// Locate an existing adapter; no tool is downloaded.
pub fn locate(configured: Option<&Path>) -> Option<AdapterCommand> {
    if let Some(path) = configured {
        let path = if path.is_dir() { path.join(exe_name()) } else { path.to_path_buf() };
        if path.is_file() {
            return Some(AdapterCommand::new(path, Origin::Settings));
        }
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(exe_name()))
        .find(|path| path.is_file())
        .map(|program| AdapterCommand::new(program, Origin::Path))
}
