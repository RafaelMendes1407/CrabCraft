# Terminal completion

The native terminal forwards Tab and Shift+Tab to the interactive shell and keeps keyboard
focus inside the terminal. Command and path completion therefore use the shell's ordinary
completion engine, including any completion functions the user has configured. Completion
does not evaluate or execute a selected command; Enter remains the explicit execution step.
Shells without a completion engine do not gain one from the terminal emulator.

`cargo test -p rc-app terminal_completion_audit -- --nocapture` compiles a genuine Cargo
binary, starts an isolated interactive Bash PTY and sends keyboard events only to the
headless native terminal widget. Tab completes a unique executable from PATH without creating
its marker. Enter runs it and the Rust program writes its marker and prints its output. The
same native Tab completes `src/main.rs` before an explicit `cat` command reads the source.
No custom shell scripts or completion packages are needed for this Bash verification.

Cargo subcommand/argument completions depend on the user's shell configuration. The IDE does
not silently install completion scripts or run a command to generate suggestions. Opening a
terminal in a project still requires project trust.
