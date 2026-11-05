# Reviewed administrator execution

The Administrator Run native toolwindow provides an initial Linux-only route for launching a prebuilt console executable from the current trusted project through the separately installed `/usr/bin/pkexec` and the operating system's PolicyKit authentication agent. The review/authorization flow remains separate from ordinary Run. You can inspect a prebuilt executable, or explicitly choose **Build and Review Selected Administrator Run** for the selected Cargo binary configuration. Preparation builds as your ordinary user and never launches the resulting program or requests privileges. Interactive program stdin, GUI applications, forwarded project environment, automatic rerun and other platforms are visibly unsupported in this execution flow. A separate Administrator Debug tab implements the guarded partial route described in [administrator-debugging.md](administrator-debugging.md).

Opening a project, restoring the panel or inspecting a binary never requests privileges. Program paths, arguments, previews and output stay in memory. Inspect runs a cancellable worker and accepts only an absolute canonical regular ELF executable inside the project's canonical directory, with executable permission, at most 64 MiB. It refuses symlinks, scripts and nonregular files. At most 32 separate arguments, each at most 4096 bytes, fit into a 64 KiB serialized request. There is no host shell parsing.

The UI shows the exact program/arguments, SHA-256 and working directory. Request Administrator Execution requires project trust and a separate review checkbox. Editing the input invalidates its preview. The backend independently checks trust, confirmation and the frozen request. Nothing requests authorization automatically on project opening or restores an armed root-run configuration. Each request consumes the preview and requires another inspection before rerun.

## Selected Cargo configuration

The explicit build/review button uses the runner's selected binary, Cargo channel/profile/features,
required features, arguments and exact configured Cargo provider/environment. It does not locate a
fallback Cargo. The normal asynchronous Save barrier runs before preparation; a change of selected
configuration/options/provider while saving refuses the deferred request. Builds are cancellable.

This bounded route refuses PTY, parallel runs, stdin files, run-environment overrides, custom working
directories and before-launch tasks. Arguments must be nonempty single-line values (up to 32,
4,096 bytes each and the existing 64 KiB request limit), so the native review can represent them
without reparsing or silently dropping values. Only binaries whose manifest and resulting canonical
ELF artifact belong to the current project are accepted. Cargo's normal build environment may be
needed to compile; it is **not** forwarded to the administrator process.

The review binds root, selected configuration/options, exact Cargo provider, metadata epoch and
up to 256 open source-document identities/versions/saved states. A change invalidates the review
before it is displayed or authorized. Provider switching also cancels preparation through the app's
existing toolchain switch path. Late replies from canceled inspections/builds cannot rearm a review.
Editing a reviewed program/argument field explicitly discards its configuration binding and review.
Every completed build starts with the authorization checkbox clear. Canceling a running administrator
operation retains its pending lifecycle until the supervisor reports completion; preparing a new
configuration never bypasses that cleanup.

## Authentication and privileged supervisor

Only PolicyKit's existing OS agent can request administrator credentials. CrabCraft does not collect, display, store or log an administrator password, install a PolicyKit policy, change authorization defaults, or run a terminal password prompt. The fixed `pkexec --disable-internal-agent` invocation fails if the OS agent/authorization is unavailable. Standard OS authorization caches and policies remain the OS's responsibility; the IDE always requires an explicit request and review checkbox.

The OS dialog identifies CrabCraft's supervisor executable rather than the final target. The native preview explains this relationship. The supervisor is an early private CLI mode, before GUI initialization. It checks effective UID 0, validates the bounded JSON request again and re-reads the executable without following symlinks. Its content must still match the reviewed hash. It copies those exact bytes into a private temporary executable owned by the supervisor, then launches that copy with the project as its working directory. It clears the child's environment except for a fixed system PATH; program stdin is `/dev/null`. Binary-relative resources and dynamic dependencies outside the copied executable are not snapshotted. This does not sandbox the program: administrator code can read or modify the system, spawn independent processes and create root-owned project files. Trust the complete program and its dependencies before authorizing it.

An ordinary user's `killpg` cannot reliably terminate a root-owned child. Therefore the privileged supervisor owns and kills its child's process group itself. A dedicated inherited control pipe is separate from the program's stdin. Cancel sends a control byte and closes the pipe; dropping the launch future or closing the IDE also closes the pipe. EOF cancels the supervisor, including a request already cancelled during binary inspection/copy before the target starts. The child receives no copy of this control stream. The parent waits briefly for privileged cleanup rather than assuming that killing a user process kills its elevated descendants. Trusted malicious root code can escape a process group; this is not a root-code sandbox.

The program has a one-hour supervisor deadline and the request has a bounded overall deadline including authentication. Output pipes are bounded to 4 MiB each; the panel shows at most 16,384 characters. Output-limit and error paths close the control pipe too. A final supervisor status distinguishes a real program exit code 126/127 from PolicyKit authorization failure and preserves console output on program failure.

## Evidence and remaining work

`cargo test -p rc-app privileged_run::tests -- --nocapture` passes six tests with a compiled Rust executable in a real Cargo project. They prove:

- The native selected-configuration route saves an edited real Rust source, builds through a configured Cargo wrapper, preserves a quoted argument containing spaces, enables a feature required to compile the fixture, uses the release profile, and stops at an unarmed review without executing the artifact or clicking the OS authorization button. Configuration edits, unsupported PTY and provider changes invalidate/refuse preparation. A canceled late inspection cannot reactivate its preview.
- Backend trust/confirmation refusal occurs before any `pkexec` invocation, and an ordinary user is rejected by the root-required helper.
- A cancellable supervisor run **as an ordinary test user**, using the same child/watchdog logic with a test-only root-check bypass, terminates a real compiled Rust child on controller EOF and reaps it before its delayed write. This is not evidence of host administrator execution.
- Closed control input before launch, changed executable bytes, symlink/script/outside-project paths, unknown helper fields and oversized arguments fail without executing the artifact.
- The native inspection/review UI refuses an untrusted request and a trusted request without confirmation. The test intentionally stops after checking the review checkbox, **before clicking the enabled OS authorization button**. It never displays an OS credential prompt or runs a privileged program.

The installed PolicyKit tool reports version 124, but no real administrator credential or authorization has been exercised. B-10 therefore remains partial. Selected Cargo binary configuration preparation is now integrated, but real administrator authorization and complete elevated run/PTY integration remain unverified/unsupported. D-23 now has a separate partial GDB DAP flow with an independent watchdog/control socket and explicit review. It remains unverified with real administrator authorization; see [administrator-debugging.md](administrator-debugging.md). Prefixing a debugger with `sudo`/`pkexec` without that lifecycle is insufficient.

## Public reference

[PolicyKit pkexec manual](https://www.freedesktop.org/software/polkit/docs/latest/pkexec.1.html) describes OS authentication agents, `--disable-internal-agent`, safe environment handling, authorization exit codes and the need to validate arguments. Its [upstream source](https://github.com/polkit-org/polkit/blob/master/docs/man/pkexec.xml) is also available. CrabCraft adds no system authorization policy or bundled privilege service.

Selected-configuration preparation follows the public [Cargo build reference](https://doc.rust-lang.org/cargo/commands/cargo-build.html) for package/binary selection, features, profiles and artifact messages.
