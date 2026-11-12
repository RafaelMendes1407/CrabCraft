# Own-code `unsafe` and FFI review map

`cargo-geiger` produces the machine-readable dependency inventory. The local
audit also lists source locations in `unsafe-own-code.json`; that lexical list
includes tests and needs human review. The production groups below explain the
intended safety argument for each known block; they do not prove a block safe.

| Source locations | Purpose and safety condition | Review priority |
| --- | --- | --- |
| `rc-app/src/main.rs:139` | Set `PATH` before starting application threads; changing process environment later would race. | Medium |
| `rc-app/src/macro_step_engine.rs:113,121,128,136` | `getrlimit`/`setrlimit` on valid local structs in a dedicated child; return values are checked. | Medium |
| `rc-app/src/open_files.rs:42,43,49,52` | macOS Objective-C callback, nullable URL pointer, ABI conversion and method registration. The declared encoding and callback signature must match; inspect on macOS when changing objc2. | High |
| `rc-app/src/privileged_debug_backend.rs:379,383` | Read/set child-subreaper mode in a dedicated helper process; error paths check syscall results. | High |
| `rc-app/src/privileged_debug_backend.rs:391,395,400` | Open a pidfd, convert it to one owned descriptor only after a nonnegative return, then signal that descriptor. | High |
| `rc-app/src/privileged_debug_backend.rs:418,502,513,562` | Poll the owned pidfd, reap captured descendants, restore subreaper mode and test effective UID. Inspect PID lifecycle and helper boundaries on Linux. | High |
| `rc-app/src/privileged_run.rs:187,257,262` | Check effective UID; duplicate stdin with close-on-exec and take ownership of the new descriptor only after a successful `fcntl`. | High |
| `rc-app/src/profiler.rs:375` | Signal the process group created for the profiler, with a nonzero PID guard. | Medium |
| `rc-dotnet/src/process.rs:223,243` | Signal a child process group created by the shared process runner; the crate retains its name for upstream compatibility. | Medium |
| `rc-lsp/src/rpc.rs:238` | Terminate the language-server group created for that child. | Medium |
| `rc-vcs/src/git.rs:156` | Kill the Git child process group on drop; confirm group creation remains paired with this path. | Medium |

Test-only uses of `unsafe` include Unix FIFO/process probes and setting
`CRABCRAFT_HOME` in isolated fixtures; they appear in the generated source
inventory and should be checked when those tests change. Embedded C/C++ code
and operating-system libraries are outside `cargo-geiger`'s Rust source scan.

The Objective-C ABI bridge, pidfd lifecycle and process-group boundaries are
the first places for a manual platform review. This inventory does not change
the implementation or add lint suppressions.
