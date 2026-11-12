//! Command for an installed Rust DAP adapter.

use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdapterCommand {
    pub program: PathBuf,
    pub prefix: Vec<String>,
    pub origin: Origin,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    Gdb,
    Lldb,
    /// Explicit DAP command used by protocol tests and adapter proxies.
    Generic,
}

impl AdapterCommand {
    pub fn args(&self) -> Vec<String> {
        self.prefix.clone()
    }

    pub fn new(program: impl Into<PathBuf>, origin: Origin) -> Self {
        Self { program: program.into(), prefix: Vec::new(), origin }
    }
}
