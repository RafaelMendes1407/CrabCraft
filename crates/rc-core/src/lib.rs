//! Core services of CrabCraft without UI: the shared async runtime, inboxes that wake the UI,
//! settings and per-workspace sessions, the local history of files, the workspace's file tree and file watching.

pub mod changelists;
pub mod code_style;
pub mod editor_import;
pub mod external_tools;
pub mod history;
pub mod inbox;
pub mod inspections;
pub mod live_templates;
pub mod paths;
pub mod processes;
pub mod run_files;
pub mod runtime;
pub mod scratch;
pub mod session;
pub mod settings;
pub mod settings_transfer;
pub mod watch;
pub mod workspace;

pub use inbox::{Inbox, Waker};
pub use runtime::Runtime;
pub use session::Session;
pub use settings::{RustfmtSettings, Settings};
pub use workspace::{FileNode, Workspace, WorkspaceKind};
