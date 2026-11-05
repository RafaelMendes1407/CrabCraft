//! Rust projects through Cargo: finding the toolchain, the workspace model from
//! `cargo metadata`, and build, run and test with streamed output.
//!
//! Operations report through the event types of `rc-dotnet`, so the Build, Run and Tests windows
//! work for both backends while the .NET one is being retired.
//!
//! Metadata and all project-selected tool invocations are deferred until application trust;
//! Cargo/rustup configuration can select executable wrappers even for metadata-only commands.

pub mod crates_io;
pub mod criticalup;
pub mod dependencies;
pub mod dependency_names;
pub mod explain;
pub mod feature_fix;
pub mod feature_nav;
pub mod format;
pub mod generate;
pub mod inspect;
pub mod manifest_complete;
pub mod messages;
pub mod metadata;
pub mod modules;
pub mod new_project;
pub mod ops;
pub mod playground;
pub mod rustup_install;
pub mod test_output;
pub mod toolchain;
pub mod tree;
pub mod triple;
pub mod versions;

pub use explain::{ExplainError, Explainer, error_code};
pub use format::{FormatError, FormatFile, FormatJob, Rustfmt, format_files, split_args};
pub use metadata::{CargoWorkspace, Library, LoadLog, Package, Target, TargetKind, feature_lines, find_manifest, load_libraries, load_libraries_logged, load_workspace, load_workspace_logged, valid_feature_entry};
pub use ops::{BeforeLaunch, CargoOptions, PrepareEvent, RunTarget, inspect_args, start_prepare, command_line, prepare, start_build, start_command, start_process, start_run, start_scratch, start_tests, target_command};
pub use triple::{TripleChoice, load_triple_choices, project_build_targets, valid_triple};
pub use tree::{DepKind, DepNode, has_children, load_tree_logged, parse_tree};
pub use versions::{DepProblem, DepSource, Locked, Lockfile, ManifestDep, Published, dependency_lines, fetch_crates_io_versions, find_lockfile, valid_crate_name};
pub use toolchain::{Cargo, ToolchainInfo, ToolchainUpdate, detect_toolchain, locate_rust_analyzer, parse_rustup_check, print_cfg, rustup_check, rustup_path};
