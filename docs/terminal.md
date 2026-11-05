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
