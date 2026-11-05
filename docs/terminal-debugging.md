# Debug a process started in the terminal

On Linux, the Terminal header's **Debug Terminal Process** button opens a native picker
for that terminal's shell and descendants. A trusted, loaded Cargo project is required;
opening a project or showing the terminal never automatically attaches a debugger.
Choose the Rust program after starting it normally in the shell. Attaching pauses it
under the configured Rust debugger; existing breakpoints, locals, stepping and the Debug
window are shared with ordinary attachment. Stop disconnects an attached program and
leaves the shell alive. No command is injected to rebuild or restart the selected program.

The list is collected on a cancellable worker, includes at most 256 descendants, and
supports filtering, refresh and cancellation. PID/start-time/executable/command identity
and ancestry are checked again on a worker before attaching. Exited/reused identities,
unrelated processes and closed/switched terminal sessions are rejected. The final OS
attach still depends on ptrace permissions and the process remaining alive; failures are
reported by the debugger. The feature neither elevates permissions nor changes system
ptrace settings. The test fixture explicitly permits attachment to itself.

Other platforms show the control disabled and an explicit Linux support message. The
picker uses bounded Linux `/proc` snapshots; a platform process-tree provider is needed
before enabling it elsewhere.

Validation:

- `cargo test -p rc-core processes::tests`: real shell/child process enumeration,
  unrelated-process exclusion, reused-identity rejection and cancellation.
- `cargo test -p rc-app terminal_attach::tests -- --ignored --nocapture`: a real Cargo
  program is built, started from a native PTY shell, selected through the terminal header
  and picker, and stopped at a GDB breakpoint. After disconnecting, it writes a success
  marker and exits normally while the terminal shell stays alive. This explicit test
  needs PTY access, GDB and ptrace permissions.
