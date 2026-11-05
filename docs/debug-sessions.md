# Parallel debug sessions

The Debug window offers **Debug in New Session**, a numbered session selector, **Close Debug
Session** and **Stop All Debug Sessions**. Select another Cargo run configuration and start a
new session to keep the existing one alive. Ordinary Debug/Resume and Restart retain their
existing active-session meaning. Up to eight sessions (including retained finished consoles)
can coexist; close a finished session to free a slot.

Each session owns its adapter, console, frames, threads, variables, watches, evaluations,
memory/disassembly state and temporary breakpoints. Background adapters are pumped normally,
but their stops do not switch the selected session or replace its execution marker or inline
values. Background error notifications include the session number. Session selection restores
the execution point and refreshes inline values even when frame IDs overlap across adapters.

Stop affects the selected session. Stop All, project close and IDE exit stop every session
and cancel pending adapter downloads. Remote sessions retain their detach semantics. Sessions
are process-local, are not reopened automatically and require normal project trust.

`cargo test -p rc-app debug_sessions -- --nocapture` uses the native egui controls and four
real Rust debug launches. It pauses two binaries simultaneously, switches sessions, checks
independent watches, resumes each to distinct marker values, confirms stopping one preserves
the other, and exercises Stop All, Close and workspace cleanup. Ptrace is required.
