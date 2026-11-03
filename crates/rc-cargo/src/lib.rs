//! Rust projects through Cargo: finding the toolchain, the workspace model from
//! `cargo metadata`, and build, run and test with streamed output.
//!
//! Operations report through the event types of `rc-dotnet`, so the Build, Run and Tests windows
//! work for both backends while the .NET one is being retired.
//!
//! Loading a workspace uses `cargo metadata --no-deps`, which does not compile anything, so it is
//! safe on a project the user has not trusted. Build, run and test execute the project's code
//! (`build.rs`, proc macros) and only start from an explicit user action.

pub mod explain;
pub mod format;
pub mod manifest_complete;
pub mod messages;
pub mod metadata;
pub mod modules;
pub mod new_project;
pub mod ops;
pub mod test_output;
pub mod toolchain;
pub mod tree;

pub use explain::{ExplainError, Explainer, error_code};
pub use format::{FormatError, FormatFile, FormatJob, Rustfmt, format_files, split_args};
pub use metadata::{CargoWorkspace, Library, LoadLog, Package, Target, TargetKind, feature_lines, find_manifest, load_libraries, load_libraries_logged, load_workspace, load_workspace_logged, valid_feature_entry};
pub use ops::{BeforeLaunch, CargoOptions, PrepareEvent, RunTarget, inspect_args, start_prepare, command_line, prepare, start_build, start_command, start_process, start_run, start_scratch, start_tests, target_command};
pub use tree::{DepKind, DepNode, has_children, load_tree_logged, parse_tree};
pub use toolchain::{Cargo, ToolchainInfo, ToolchainUpdate, detect_toolchain, locate_rust_analyzer, parse_rustup_check, print_cfg, rustup_check, rustup_path};
