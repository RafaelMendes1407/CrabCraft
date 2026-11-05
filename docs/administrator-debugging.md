# Administrator debugging on Linux (partial)

The Administrator Run tool window has a separate Administrator Debug tab. It prepares the
selected Cargo binary as the ordinary desktop user, waits for document saves and formatting,
then keeps a private frozen ELF copy and its SHA-256. The review shows the original artifact,
arguments, working directory and hash. Trust and a separate explicit approval are required
before Request Administrator Debugging can invoke the operating system's PolicyKit agent.
The IDE neither requests nor stores an administrator password. Nothing elevates on open.

Preparation runs Cargo, build scripts, proc macros and before-launch tools as the ordinary
user. The privileged helper has no Cargo/build entry point. It rechecks the frozen copy and
copies it into its own private directory before GDB loads it. Later rebuilds of the original
artifact do not replace the reviewed snapshot. Buffer changes after preparation require a
new build/review. Cancel, trust revocation, provider changes and workspace changes invalidate
preparation and abort pending authorization continuations through the save barrier.

The existing GDB DAP integration is used rather than a shell or a user-configured debugger
wrapper. The privileged helper accepts only a root-owned, non-group/other-writable regular
`/usr/bin/gdb`. It runs with a neutral initial directory, cleared environment, `-nx`, `-nh`,
`-iex 'set auto-load off'`, disabled debuginfod, and `set startup-with-shell off`.
The reviewed inferior working directory and arguments are set explicitly. Project environment
variables and debugger init commands are not forwarded. The program and debugger have full
administrator permissions; this is **not an OS sandbox**. Shared libraries and other external
resources are not frozen with the executable.

DAP owns stdin/stdout. A separate Unix socket in a private 0700 directory controls watchdog
lifetime, with peer UID checks on both sides. The unprivileged proxy owns that socket; killing
the IDE/proxy closes it. EOF, a Cancel byte, protocol failure or the session time limit stops
the privileged debugger and inferior. The dedicated helper becomes a Linux subreaper, scans
all threads' child lists, pins descendants with pidfds, stops them before collection, kills
members of different process groups, and reaps adopted descendants. It scans new adoptions
again after killing ancestors. Ordinary PID reuse cannot redirect a pidfd signal. Linux
pidfd/subreaper support and readable `/proc` are required.

DAP headers are bounded to 8 KiB, individual messages to 1 MiB, output to 16 MiB and a session
to one hour. Launch arguments are rewritten to the verified private copy. Only the supported
inspection/stepping protocol is forwarded. Evaluated expressions, variable/memory writes,
conditional breakpoints, logpoints, debugger-console commands, attach and automatic restart
are unavailable. Breakpoints become unconditional; the built-in Rust panic function is the
only function breakpoint accepted. A new administrator session requires another review and
approval. The UI explicitly identifies these limits.

D-23 stays partial: no authenticated root session or real PolicyKit authorization has been
run in verification. Tests never invoke pkexec or relax authorization, ptrace, AppArmor or
other system policies. Root-owned output/resource behavior, installed Polkit agent interaction
and authenticated elevated DAP remain unverified. Closing a kernel I/O operation already in
progress may delay cleanup; this implementation does not promise containment against a
malicious program that already has administrator permissions.

## Verification

Five default tests cover frozen artifact/hash identity after a real Rust rebuild; trust and
approval guards; bounded DAP parsing; rewritten launch/environment and refused protocol;
normal-user Cargo/build.rs preparation after Save/rustfmt; native approval UI; changed-buffer
rejection; and cancellation/provider changes preventing a late review from becoming active.
The Cargo fixture compares the build script's actual `/proc/self/status` EUID to the desktop
test process EUID.

Two opt-in tests use installed system GDB and a real compiled Rust program under an ordinary
user, with a dedicated unprivileged helper subprocess. Protocol and watchdog use independent
private fixture sockets; the private helper entry is ignored except when called by these
tests. They verify a Rust source stack and ignored project `.gdbinit`, watchdog EOF, and
Cancel killing/reaping a child in a different process group (absence from `/proc`, including
zombies). The tests make no authenticated elevation claim.

```
cargo test -p rc-app privileged_debug -- --nocapture
cargo test -p rc-app privileged_debug_backend::tests::real_ -- --ignored --nocapture
```

Public references: [GDB auto-loading](https://www.sourceware.org/gdb/current/onlinedocs/gdb.html/Auto_002dloading.html),
[GDB startup ordering](https://www.sourceware.org/gdb/current/onlinedocs/gdb.html/Startup.html),
[GDB shell startup behavior](https://www.sourceware.org/gdb/current/onlinedocs/gdb.html/Starting.html),
and [native GDB DAP](https://www.sourceware.org/gdb/current/onlinedocs/gdb.html/Debugger-Adapter-Protocol.html).
GDB 14/15 starts on `launch`, so the fixture configures breakpoints and finishes
`configurationDone` before launch, matching the existing rc-dap production handshake.
No external debugger assets or source text are distributed by this change.
