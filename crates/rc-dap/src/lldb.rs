//! LLVM's `lldb-dap` (or its older `lldb-vscode` name), speaking DAP over stdio.
use crate::adapter::{AdapterCommand, Origin};
use std::path::{Path, PathBuf};

/// Discovery only checks files. It never executes a project tool or silently falls back
/// to a different debugger when the user's explicit path is unavailable.
pub fn locate(configured: Option<&Path>) -> Option<AdapterCommand> {
    let program = if let Some(path) = configured {
        path.is_absolute().then(|| path.to_path_buf()).filter(|p| p.is_file())?
    } else {
        let dirs: Vec<_> = std::env::var_os("PATH").iter().flat_map(std::env::split_paths).filter(|p| p.is_absolute()).collect();
        ["lldb-dap", "lldb-vscode"].iter().flat_map(|name| dirs.iter().map(move |dir| dir.join(exe(name))))
            .find(|path| path.is_file())?
    };
    Some(AdapterCommand::new(program, Origin::Lldb))
}

fn exe(name: &str) -> PathBuf { if cfg!(windows) { format!("{name}.exe").into() } else { name.into() } }

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn configured_adapter_uses_stdio() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(exe("lldb-dap"));
        std::fs::write(&path, "not executed during discovery").unwrap();
        let adapter = locate(Some(&path)).unwrap();
        assert_eq!(adapter.origin, Origin::Lldb);
        assert!(adapter.args().is_empty());
        assert!(locate(Some(&dir.path().join("missing"))).is_none());
        assert!(locate(Some(Path::new("./lldb-dap"))).is_none());
    }
}
