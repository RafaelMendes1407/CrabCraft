//! Scopes: named sets of files used to narrow Find in Files and to colour files. A scope is a
//! [`FileMask`] (globs, `!` to leave files out) or a fixed set of paths (the changed files).

use crate::mask::FileMask;
use std::collections::HashSet;
use std::sync::Arc;

/// The files of a scope, by root-relative, `/`-separated path.
#[derive(Clone, Debug)]
pub enum ScopeFilter {
    Mask(FileMask),
    Paths(Arc<HashSet<String>>),
}

impl ScopeFilter {
    /// Whether a root-relative, `/`-separated path is in the scope.
    pub fn matches(&self, rel: &str) -> bool {
        match self {
            ScopeFilter::Mask(m) => m.matches(rel),
            ScopeFilter::Paths(set) => set.contains(rel),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_and_path_sets() {
        let Ok(mask) = FileMask::parse("src/**/*.rs, !**/generated/**") else { panic!("valid mask") };
        let f = ScopeFilter::Mask(mask);
        assert!(f.matches("src/main.rs") && f.matches("src/a/b.rs"));
        assert!(!f.matches("src/generated/x.rs") && !f.matches("tests/t.rs") && !f.matches("target/debug/build/x.rs"));
        let f = ScopeFilter::Paths(Arc::new(["src/lib.rs".to_string()].into_iter().collect()));
        assert!(f.matches("src/lib.rs") && !f.matches("src/main.rs"));
    }
}
