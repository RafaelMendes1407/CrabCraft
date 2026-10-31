//! GDB as the debug adapter for Rust: it speaks DAP natively since version 14. `rust-gdb` runs it
//! with the pretty printers of the active toolchain.

use crate::netcoredbg::{AdapterCommand, Origin};
use std::path::{Path, PathBuf};

/// The interpreter flag and options that keep GDB from asking anything on start.
const ARGS: [&str; 6] = ["-i=dap", "-quiet", "-iex", "set debuginfod enabled off", "-iex", SKIP_STD];

/// Step Into walks past the standard library, by function name: GDB's file patterns stop at each
/// `/`. Trait impls for the project's types keep their own path first, so they are not skipped.
/// Breakpoints and panics still stop in the library.
const SKIP_STD: &str = r"skip -rfu ^<?(core|alloc|std)::";

fn in_path(name: &str, dirs: &[PathBuf]) -> Option<PathBuf> {
    dirs.iter().map(|d| d.join(name)).find(|p| p.is_file())
}

/// `rust-gdb` when the toolchain has it and GDB is installed, or plain `gdb`.
pub fn locate() -> Option<AdapterCommand> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH").iter().flat_map(std::env::split_paths).collect();
    dirs.extend(std::env::var_os("HOME").map(|h| Path::new(&h).join(".cargo").join("bin")));
    let gdb = in_path("gdb", &dirs)?;
    let program = in_path("rust-gdb", &dirs).unwrap_or(gdb);
    let mut command = AdapterCommand::new(program, Origin::Gdb);
    command.prefix = ARGS.iter().map(ToString::to_string).collect();
    Some(command)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gdb_runs_with_its_own_interpreter_flag() {
        let mut c = AdapterCommand::new("/bin/gdb", Origin::Gdb);
        c.prefix = ARGS.iter().map(ToString::to_string).collect();
        assert_eq!(c.args()[0], "-i=dap");
        assert!(!c.args().iter().any(|a| a == "--interpreter=vscode"));
    }

    #[test]
    fn finds_a_program_in_the_given_folders() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("gdb"), "").unwrap();
        assert_eq!(in_path("gdb", &[dir.path().to_path_buf()]), Some(dir.path().join("gdb")));
        assert_eq!(in_path("rust-gdb", &[dir.path().to_path_buf()]), None);
    }
}
