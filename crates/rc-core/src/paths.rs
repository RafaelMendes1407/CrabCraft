//! Where CrabCraft keeps its files: `~/Library/Application Support/CrabCraft` on macOS,
//! `%APPDATA%\CrabCraft` on Windows. `CRABCRAFT_HOME` overrides it (tests, portable installs).

use std::path::PathBuf;

pub fn data_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("CRABCRAFT_HOME").filter(|v| !v.is_empty()) {
        return PathBuf::from(dir);
    }
    dirs::data_dir().unwrap_or_else(|| PathBuf::from(".")).join("CrabCraft")
}

/// Downloaded and installed tools.
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
    let check = || if cancel.load(std::sync::atomic::Ordering::Acquire) { Err(std::io::Error::from(std::io::ErrorKind::Interrupted)) } else { Ok(()) };
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
    for chunk in bytes.chunks(64 * 1024) {
        check()?;
        temporary.write_all(chunk)?;
    }
    check()?;
    if let Some(permissions) = permissions {
        temporary.as_file().set_permissions(permissions)?;
    }
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
        use std::os::unix::{
            ffi::OsStrExt,
            fs::{PermissionsExt, symlink},
        };
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
        let link = root.path().join("link.rs");
        symlink(&victim, &link).unwrap();
        assert!(write_atomic(&link, b"replace").is_err());
        let fifo = root.path().join("fifo");
        let name = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        assert!(write_atomic(&fifo, b"replace").is_err());
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "keep");
    }
}

/// `path` without Windows' verbatim prefix (`\\?\C:\x` becomes `C:\x`, `\\?\UNC\h\s` becomes
/// `\\h\s`), for tools such as Git that cannot open verbatim paths. Other paths, and every
/// path on other systems, are returned unchanged.
pub fn plain(path: &std::path::Path) -> PathBuf {
    #[cfg(windows)]
    if let Some(text) = path.to_str() {
        if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{rest}"));
        }
        // Only drive paths: `\\?\GLOBALROOT\…` and similar have no plain form.
        if let Some(rest) = text.strip_prefix(r"\\?\")
            && rest.as_bytes().get(1) == Some(&b':')
        {
            return PathBuf::from(rest);
        }
    }
    path.to_path_buf()
}

#[cfg(test)]
mod plain_tests {
    use std::path::{Path, PathBuf};
    #[test]
    #[cfg(windows)]
    fn verbatim_drive_and_unc_paths_lose_their_prefix() {
        assert_eq!(super::plain(Path::new(r"\\?\C:\Users\me\a b")), PathBuf::from(r"C:\Users\me\a b"));
        assert_eq!(super::plain(Path::new(r"\\?\UNC\host\share\x")), PathBuf::from(r"\\host\share\x"));
        assert_eq!(super::plain(Path::new(r"\\?\GLOBALROOT\Device")), PathBuf::from(r"\\?\GLOBALROOT\Device"));
        assert_eq!(super::plain(Path::new(r"C:\x")), PathBuf::from(r"C:\x"));
    }
    #[test]
    fn plain_paths_are_unchanged() {
        assert_eq!(super::plain(Path::new("relative/x.rs")), PathBuf::from("relative/x.rs"));
    }
}

/// Whether two paths spell the same location without touching the disk. On Windows a verbatim
/// prefix and `/` versus `\` separators do not matter (`\\?\C:\a\b` equals `C:\a/b`); elsewhere
/// this is plain equality. It does not resolve `..`, symlinks or letter case.
pub fn same_spelling(a: &std::path::Path, b: &std::path::Path) -> bool {
    if a == b {
        return true;
    }
    #[cfg(windows)]
    {
        let key = |path: &std::path::Path| plain(path).to_string_lossy().replace('/', "\\");
        key(a) == key(b)
    }
    #[cfg(not(windows))]
    false
}

#[cfg(test)]
mod spelling_tests {
    use std::path::Path;
    #[test]
    #[cfg(windows)]
    fn verbatim_and_separator_variants_match_on_windows() {
        assert!(super::same_spelling(Path::new(r"\\?\C:\a\src\main.rs"), Path::new(r"C:\a\src/main.rs")));
        assert!(!super::same_spelling(Path::new(r"C:\a\src\main.rs"), Path::new(r"C:\a\src\lib.rs")));
    }
    #[test]
    fn identical_paths_match_and_different_ones_do_not() {
        assert!(super::same_spelling(Path::new("a/b.rs"), Path::new("a/b.rs")));
        assert!(!super::same_spelling(Path::new("a/b.rs"), Path::new("a/c.rs")));
    }
}
