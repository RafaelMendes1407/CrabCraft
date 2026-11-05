# Remote Rust debugging

Open a trusted Cargo workspace, then use the Run menu's **Connect to GDB Server…**.
Enter a host, TCP port and absolute path to the local binary containing symbols for the
remote program. Start `gdbserver HOST:PORT /path/to/program` on the target beforehand.
CrabCraft uses the configured GDB executable and its DAP `attach` request with a validated
`target` endpoint and local `program`; no shell command, upload or remote launch is implicit.
This connection uses GDB even when LLDB is selected for ordinary local debugging.

The program must match the remote binary, and debug source paths must resolve locally.
Automatic source-path mapping, cross-architecture toolchain provisioning and SSH tunnels
are not configured by this dialog. GDB remote TCP has no transport authentication or
encryption: use an existing SSH tunnel or a network appropriate for the target.

Breakpoints, threads, stack frames, variables, stepping and continue use the existing native
debugger UI. Stop explicitly detaches before closing GDB; it never kills a local process
whose PID happens to match the remote PID. Adapter work and shutdown are asynchronous and
bounded. The connection dialog sends nothing before Connect, and project trust is checked
again at that point. Invalid hosts, shell-like endpoints, control characters and zero ports
are rejected before any adapter invocation.

The real integration creates a Cargo binary and runs it behind a local TCP gdbserver. It
verifies source breakpoints and a Rust local, continuation to completion, detach while
stopped and detach while running. Both detached processes complete and write their marker.
This caught a GDB 15 behavior where `disconnect(false)` alone is insufficient, and the
pause reply arriving before the asynchronous stop; shutdown now waits/retries detach.

Validation: `CRABCRAFT_TEST_GDBSERVER=/absolute/path/gdbserver cargo test -p rc-dap --test remote`
(requires ptrace). Native dialog cancellation and pre-trust Connect are tested separately.

Public references: [GDB DAP](https://www.sourceware.org/gdb/current/onlinedocs/gdb.html/Debugger-Adapter-Protocol.html)
and [remote connections](https://www.sourceware.org/gdb/current/onlinedocs/gdb.html/Connecting.html).
