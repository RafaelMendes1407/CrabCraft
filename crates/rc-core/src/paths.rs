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

/// Explanations of rustc error codes shown in read-only tabs.
pub fn explanations_dir() -> PathBuf {
    data_dir().join("error-explanations")
}

/// Macro expansions shown in read-only tabs.
pub fn expansions_dir() -> PathBuf {
    data_dir().join("macro-expansions")
}

pub fn logs_dir() -> PathBuf {
    data_dir().join("logs")
}

/// Replaces a regular file atomically using an exclusively created sibling temporary file.
/// Existing permissions are preserved; this does not promise power-loss durability.
pub fn write_atomic(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    write_atomic_cancellable(path, bytes, &std::sync::atomic::AtomicBool::new(false))
}

/// Cancellation before the atomic replacement preserves the old file. An already committed
/// replacement is not rolled back. Cancellation is checked between bounded writes.
pub fn write_atomic_cancellable(path: &std::path::Path, bytes: &[u8], cancel: &std::sync::atomic::AtomicBool) -> std::io::Result<()> {
    let check = || if cancel.load(std::sync::atomic::Ordering::Acquire) {Err(std::io::Error::from(std::io::ErrorKind::Interrupted))} else {Ok(())};
    check()?;
    use std::io::Write;
    let parent = path.parent().filter(|parent| !parent.as_os_str().is_empty()).unwrap_or_else(|| std::path::Path::new("."));
    std::fs::create_dir_all(parent)?;
    let permissions = match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => Some(metadata.permissions()),
        Ok(_) => return Err(std::io::Error::from(std::io::ErrorKind::InvalidInput)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    let mut temporary = tempfile::Builder::new().prefix(".crabcraft-save-").tempfile_in(parent)?;
    for chunk in bytes.chunks(64*1024) {check()?; temporary.write_all(chunk)?;}
    check()?;
    if let Some(permissions) = permissions { temporary.as_file().set_permissions(permissions)?; }
    check()?;
    temporary.persist(path).map_err(|error| error.error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn atomic_replacement_never_touches_predictable_sibling_names() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("main.rs");
        let sibling = root.path().join("main.tmp");
        std::fs::write(&source, "fn main() {}\n").unwrap();
        std::fs::write(&sibling, "unrelated user file").unwrap();
        write_atomic(&source, b"fn main() { println!(\"saved\"); }\n").unwrap();
        assert!(std::fs::read_to_string(&source).unwrap().contains("saved"));
        assert_eq!(std::fs::read_to_string(&sibling).unwrap(), "unrelated user file");
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 2);
        assert!(write_atomic(root.path(), b"invalid").is_err());
    }
    #[cfg(unix)]
    #[test]
    fn preplanted_symlinks_are_not_followed_and_permissions_are_preserved() {
        use std::os::unix::{fs::{PermissionsExt, symlink}, ffi::OsStrExt};
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("main.rs");
        let victim = root.path().join("unrelated.txt");
        std::fs::write(&source, "old").unwrap();
        std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o640)).unwrap();
        std::fs::write(&victim, "keep").unwrap();
        symlink(&victim, source.with_extension("tmp")).unwrap();
        write_atomic(&source, b"new").unwrap();
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "keep");
        assert_eq!(std::fs::metadata(&source).unwrap().permissions().mode() & 0o777, 0o640);
        let link = root.path().join("link.rs"); symlink(&victim, &link).unwrap();
        assert!(write_atomic(&link, b"replace").is_err());
        let fifo = root.path().join("fifo"); let name = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        assert!(write_atomic(&fifo, b"replace").is_err());
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "keep");
    }
}
