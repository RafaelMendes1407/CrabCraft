# Native terminal

The Terminal tool window starts the user's shell on a pseudo-terminal in the Cargo
workspace folder (or the folder chosen with Open in Terminal). The emulator and the widget
are native Rust (`portable-pty`, `alacritty_terminal`, egui). Startup happens in a worker
thread; Cancel dismisses a pending startup and closing the project cancels it.

New Terminal Tab creates an independent shell. Selecting a tab switches sessions without
restarting them. Closing a tab terminates its process and leaves the remaining sessions.
Terminal sessions are not restored across application restarts.

The widget renders ANSI colours, scrollback, terminal selection, links, clipboard commands,
and keyboard/IME input. Resizing updates the PTY dimensions. Shell startup files remain
under the user's control; the terminal runs only when explicitly opened.

Verification in a real Cargo workspace with spaces in its folder name runs
`cargo run --offline --quiet` and checks the Rust program's output. A native headless test
also creates, switches and closes independent tabs. The real PTY tests cover resize,
terminal environment, process exit/termination, colours, selection, wide characters and
bracketed paste. This evidence does not assert feature parity with another IDE.

## Splits and search

Split Terminal Right and Split Terminal Down start another independent shell asynchronously
in the current pane's starting directory. Two native panes share a draggable divider.
Clicking a pane gives it keyboard focus; its tab becomes active. Close Terminal Split
returns to a single pane while preserving both shells as tabs. Closing either pane's tab
terminates that shell and removes the split. Cancelling a pending split or tab leaves
existing panes visible. One pair is shown at a time; creating another
split replaces the visible pair without killing the older sessions. Splits are not nested
or restored across restarts.

Find in Terminal searches the active pane's scrollback for a case-sensitive literal string
(up to 256 characters). It includes Unicode wide/combining cells. Previous/Next cycle
through matches and reveal/select them for copying. Search runs on a cancellable worker,
locking one terminal row at a time so painting and input can continue. Editing the query,
closing search, changing project or dropping the search handle cancels its worker.

Search examines at most the most recent two million terminal cells and returns at most
5,000 matches, with a visible limit message. It searches rendered rows, not patterns spanning
wrapped rows or newline boundaries. Output or size changes invalidate saved coordinates;
Refresh searches the new snapshot instead of selecting stale positions. Regex and
case-insensitive search do not have controls.

A native headless test opens a real Cargo workspace, runs its Rust program, creates both
split orientations, verifies PTY dimensions and finds/selects the program's output. Backend
tests cover scrollback matches, Unicode cell coordinates, literal regex metacharacters,
stale output rejection and the match limit.
