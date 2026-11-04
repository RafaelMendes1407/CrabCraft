//! Git integration of CrabCraft through the `git` CLI (never libgit2): repository discovery,
//! status, diffs, log, blame and write operations as async functions meant to run on
//! `rc_core::Runtime`, plus in-process line diffs for the gutter and the side-by-side viewer.
//! No UI code lives here.

pub mod blame;
pub mod branch;
pub mod cancel;
pub mod diff;
pub mod error;
pub mod git;
pub mod graph;
pub mod gutter;
pub mod log;
pub mod ops;
pub mod patch;
pub mod remote;
pub mod repo;
pub mod sequence;
pub mod side_by_side;
pub mod sign;
pub mod stash;
pub mod status;
pub mod tag;
pub mod textdiff;
pub mod worktree;

pub use blame::{Blame, BlameCommit, BlameLine};
pub use branch::{Branch, BranchKind};
pub use cancel::CancelToken;
pub use diff::{DiffLine, DiffLineKind, DiffOptions, DiffSpec, DiffStatus, FileContent, FileDiff, Hunk, Revision};
pub use error::{GitError, Result};
pub use git::{Git, GitVersion, Progress, ProgressSink, progress_to};
pub use graph::{Edge, Graph, GraphRow, Half};
pub use gutter::{GutterMark, LineChange, LineChangeKind, LineChanges};
pub use log::{ChangedFile, Commit, LogOptions, LogOrder, RefLabel};
pub use ops::{CommitOptions, ConflictVersions};
pub use patch::{MAX_PATCH_BYTES, PatchFile, PatchKind, PatchPreview, check_patch_size, patch_kind};
pub use remote::{FetchOptions, PullOptions, PushOptions, Remote};
pub use repo::{RepoState, Repository};
pub use sequence::{FastForward, MergeOptions, Operation};
pub use sign::{Signature, SignatureStatus, SigningConfig};
pub use side_by_side::{Row, RowKind, SideBySide, inline_changes};
pub use stash::{Stash, StashOptions};
pub use status::{BranchStatus, ConflictKind, FileState, Status, StatusEntry, StatusOptions};
pub use tag::Tag;
pub use worktree::{Worktree, WorktreeCheckout};
