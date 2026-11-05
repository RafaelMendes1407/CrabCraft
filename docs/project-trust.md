# Project trust

Opening any workspace starts in **Restricted mode** unless its canonical root is already
in the application's trust store. The status bar and Git windows offer **Trust this project**.
Trust is an explicit permission to execute the project's tools and configuration; it is not
a sandbox. Review unfamiliar source and configuration before granting it.

Restricted mode permits editing, syntax parsing and file searches.
It does not start Cargo metadata, dependency metadata, rustc cfg/target queries, project
language servers, Git discovery/status or configured Git helpers. Cargo metadata can execute
a custom toolchain selected by `rust-toolchain.toml`, even when `--no-deps` compiles nothing.
Compiler wrappers and other Cargo configuration can execute programs too. These are project
execution and require the same trust decision as builds, tests and procedural macros.

Build, clean, Cargo commands, saved run commands, Run Anything, test, debug, profile,
coverage, external project tools, scratches within a workspace, terminals and Rust REPL
cannot bypass this decision. Restarting a stopped or restricted language server does not
start it. Standalone user-requested scratches and IDE tool maintenance remain explicit
operations; IDE rustup commands use the home folder without the project's environment.

Trust reloads the Cargo model and starts the appropriate language server and Git discovery.
The application stores trust separately from project sessions. Cloning clears old trust
for the destination, and clone itself suppresses template hooks, global checkout filters,
recursive submodules and interactive authentication. Cloning does not grant trust.

The Git library also treats discovered repositories as untrusted by default. Its safe read
commands suppress configured helpers such as fsmonitor, filters, text conversion and
signature programs, and prohibit remote lazy fetch; writes require an explicitly trusted
repository. The desktop does not invoke Git at all while a workspace is restricted.

Evidence on Linux includes native controls on a real Rust/Cargo fixture with a custom
rustup toolchain, a compiler wrapper and build-script sentinels. None executes before trust,
including after cache invalidation or attempted run/build/test/debug/terminal actions.
The native Trust button then reloads metadata and permits the same effective custom cargo;
a later metadata reload proves the wrapper is effective too. A plain Git folder similarly
leaves its fsmonitor sentinel untouched until native Trust enables status. Real Git app
tests cover commits, branches, diffs, stashes, worktrees, conflicts, signed commits and
local remotes after explicit trust. The library tests exercise malicious Git helper
controls independently. These tests do not establish sandbox isolation or RustRover parity.

Read-only Git subcommands also reject output redirection and write switches before any
process starts: `log`, `show` and `diff --output=...` can otherwise write files. Integration
controls verify the untrusted API refuses them without creating the output file.
Git commands own a process group until normal completion; cancellation, future drop and
I/O-limit failures kill editor/hook descendants too. Background-shell regressions verify
no delayed child writes survive cancellation or a dropped future.
