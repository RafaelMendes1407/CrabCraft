# Rust debugger selection

Settings → Tools offers an explicit GDB / LLDB selection, plus optional absolute executable
paths. The selected backend applies to subsequent Rust launch and attach operations; an
existing session keeps its adapter. A missing configured executable reports an error instead
of silently switching backend. Paths and the choice remain local machine settings.

GDB uses its native DAP interpreter (`gdb` 14 or newer), preferring `rust-gdb` for pretty
printers when automatically discovered. LLDB uses LLVM's `lldb-dap` or the older
`lldb-vscode` name over stdio, with the adapter arguments required by LLDB. Rust launch
arguments include program, arguments, working directory, environment and stop-on-entry.
Project trust is required before either adapter is started. Discovery does not execute them.

LLVM DAP is documented at https://lldb.llvm.org/use/lldbdap.html (public LLVM documentation).
The integration uses its documented DAP launch/attach fields and `initCommands`; it preserves
ASLR with `settings set target.disable-aslr false` so debugging does not require a process
personality change. No LLVM assets are distributed with CrabCraft.

Validation on Linux: LLVM 19 DAP built a real Cargo binary, stopped at a source breakpoint,
showed a typed Rust local, stepped, captured stdout and observed exit 0. The existing real
GDB Rust workflow also passed, as did all 45 DAP unit tests. The native settings test switches
both directions and verifies persisted settings without running the project's build script.
LLDB-specific pretty printers and every advanced GDB action are not claimed to be equivalent;
existing capability checks govern individual debugger actions.

Run the integration with an installed adapter or set `CRABCRAFT_TEST_LLDB_DAP` to an absolute
adapter path: `cargo test -p rc-dap --test lldb`. Debugger integration tests require ptrace.
