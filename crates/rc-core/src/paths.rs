//! Where CrabCraft keeps its files: `~/Library/Application Support/CrabCraft` on macOS,
//! `%APPDATA%\CrabCraft` on Windows. `CRABCRAFT_HOME` overrides it (tests, portable installs).

use std::path::PathBuf;

pub fn data_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("CRABCRAFT_HOME").filter(|v| !v.is_empty()) {
        return PathBuf::from(dir);
    }
    dirs::data_dir().unwrap_or_else(|| PathBuf::from(".")).join("CrabCraft")
}

/// Downloaded tools (the Roslyn language server, netcoredbg).
pub fn tools_dir() -> PathBuf {
    data_dir().join("tools")
}

pub fn sessions_dir() -> PathBuf {
    data_dir().join("sessions")
}

/// Earlier versions of edited files (local history).
pub fn history_dir() -> PathBuf {
    data_dir().join("history")
}

/// Scratch files, shared by every project.
pub fn scratches_dir() -> PathBuf {
    data_dir().join("scratches")
}

/// Macro expansions shown in read-only tabs.
pub fn expansions_dir() -> PathBuf {
    data_dir().join("macro-expansions")
}

pub fn logs_dir() -> PathBuf {
    data_dir().join("logs")
}

/// Writes `bytes` atomically: a crash halfway never leaves a truncated file.
pub fn write_atomic(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}
