//! Rust projects through Cargo: finding the toolchain, the workspace model from
//! `cargo metadata`, and build, run and test with streamed output.
//!
//! Operations report through the event types of `rc-dotnet`, so the Build, Run and Tests windows
//! work for both backends while the .NET one is being retired.
//!
//! Loading a workspace uses `cargo metadata --no-deps`, which does not compile anything, so it is
//! safe on a project the user has not trusted. Build, run and test execute the project's code
//! (`build.rs`, proc macros) and only start from an explicit user action.

pub mod messages;
pub mod metadata;
pub mod new_project;
pub mod ops;
pub mod test_output;
pub mod toolchain;

pub use metadata::{CargoWorkspace, Library, Package, Target, TargetKind, find_manifest, load_libraries, load_workspace};
pub use ops::{RunTarget, command_line, prepare, start_build, start_command, start_run, start_tests, target_command};
pub use toolchain::{Cargo, ToolchainInfo, detect_toolchain, locate_rust_analyzer};
