# Declarative macro tester

Code > Test Declarative Macro… and Find Action open a native editor for local
macro_rules definitions and one invocation. When the caret is inside a current
parsed macro definition, the tester copies that definition from the editor's
buffer, including unsaved text, and proposes an empty invocation with its name.
Otherwise existing input is retained; the initial sample demonstrates a twice macro. Edit the arguments and
click Test macro expansion to inspect the recursive expansion. Editing either
input clears the previous result and cancels pending work. Nothing is written
back to the project.

Testing requires explicit project trust. Opening the window starts no analyzer.
An explicitly requested test runs its own rust-analyzer on a temporary Rust 2024
crate graph with no dependencies, sysroot, target, Cargo manifest or runnable.
Cargo checks, build scripts and procedural macros are disabled. The tester does
not reuse or restart the project's language server. It expands syntax; it does
not run a user's invocation or evaluate an expression.

The worker owns its language-server process and kills it on cancellation, window
or project close, an obsolete input, timeout or completion. Killed LSP children
are now reaped asynchronously. Requests check cancellation every 10 ms while
waiting. Initialization and expansion have a 20-second deadline; an invocation
that remains unexpandable is rejected after five seconds. Error messages and
partial expansions produced by rust-analyzer remain visible as analyzer output;
no previous successful result is substituted for an invalid matcher.

Each input is limited to 16 KiB, with at most 16 local definitions and 16,384
syntax nodes examined. Output is limited to 64 KiB and analyzer errors to 4,096 characters.
The local parser rejects executable items outside the macro definitions, multiple
invocations and invocations of a macro that is not declared in the input.
Repetitions, fragment specifiers, nested calls to definitions in the input and
recursive expansion are handled by rust-analyzer. External/project macros,
procedural macros, runtime evaluation, per-matcher stepping/visualization and
Rust editions other than 2024 are unsupported. This does not claim parity with
another IDE's macro debugger.

Four tests in macro_tester.rs cover the supported subset:

- A native test opens a real Cargo project with a build-script sentinel, edits its
  macro without saving, opens the action, confirms the untrusted action cannot
  start work, authorizes tools explicitly, types an invocation and inspects its
  expansion. A later invalid matcher replaces the successful output. The project
  source stays unchanged, no target/build-script output is created and its own
  language server remains stopped.
- Real rust-analyzer expands nested macros and repetitions. rustc compiles and
  runs the returned expressions, verifying 42 and a three-element tuple.
- A hanging analyzer process is cancelled by changing the input; its PID disappears
  and no late result is accepted.
- Input validation covers unrelated code, unknown names, multiple calls, bounds
  and the explicit trust/cancel gates.

Run: cargo test -p rc-app macro_tester -- --nocapture

Public protocol references: [rust-analyzer configuration](https://rust-analyzer.github.io/book/configuration),
[non-Cargo crate graphs](https://rust-analyzer.github.io/book/non_cargo_based_projects.html)
and [expandMacro extension](https://github.com/rust-lang/rust-analyzer/blob/master/docs/book/src/contributing/lsp-extensions.md).
The implementation uses the public protocol; it does not copy another IDE's UI.
