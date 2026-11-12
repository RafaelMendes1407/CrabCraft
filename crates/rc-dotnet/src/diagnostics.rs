//! Shared compiler diagnostic types used by Cargo.

use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Severity {
    Error,
    Warning,
    Info,
}

/// One build diagnostic. Lines and columns are 1-based.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Diagnostic {
    /// Source file, when available.
    pub file: Option<PathBuf>,
    /// Source origin reported by the compiler.
    pub origin: String,
    pub line: Option<u32>,
    pub column: Option<u32>,
    pub end_line: Option<u32>,
    pub end_column: Option<u32>,
    pub severity: Severity,
    /// Diagnostic code, when available.
    pub code: String,
    pub message: String,
    /// Package or project associated with the diagnostic.
    pub project: Option<PathBuf>,
    /// The compiler's suggestion, when it can be applied.
    pub fix: Option<Fix>,
    /// Bounded labeled rustc source ranges for borrow/lifetime diagnostics.
    pub borrow: Option<BorrowDiagnostic>,
}

/// Compiler-provided ranges, not inferred lifetime/NLL intervals.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct BorrowDiagnostic { pub root: PathBuf, pub spans: Vec<BorrowSpan>, pub notes: Vec<String>, pub limited: bool }
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct BorrowSpan {
    pub file: PathBuf, pub start: usize, pub end: usize,
    pub line: u32, pub column: u32, pub end_line: u32, pub end_column: u32,
    pub primary: bool, pub label: String, pub text: Vec<BorrowLine>,
}
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct BorrowLine { pub line: u32, pub text: String, pub start: u32, pub end: u32 }

/// Edits proposed by the compiler for the file of a diagnostic.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Fix {
    /// What the compiler says the change does.
    pub label: String,
    /// `source_hash` of the file the byte offsets refer to.
    pub source: u64,
    /// `(start byte, end byte, replacement)`, in file order, without overlaps.
    pub edits: Vec<(usize, usize, String)>,
}

/// Identifies the exact content of a file.
pub fn source_hash(bytes: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}
