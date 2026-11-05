# Complete Current Statement

Code → Complete Current Statement, action search, or the Rust reference profile's
Ctrl+Shift+Enter (Command+Shift+Enter on macOS) completes the current Rust line.
The caret can be anywhere on that line. Missing closing parentheses/brackets and
declaration semicolons are supplied; supported function/control headers gain a block.
The caret moves to the next indented line or into the new block, using the editor's
indentation settings. Rust tail expressions retain their return semantics.

This is conservative syntax completion, not type inference or a value generator.
Comments on the affected line, ambiguous input, errors remaining elsewhere and
incomplete values that cannot be repaired with punctuation show an explicit message
and leave the buffer unchanged. Existing strings, raw strings and character literals
are excluded from delimiter balancing. Multi-line constructs needing further input
can require manual editing. One caret and an editable `.rs` file of at most 1 MiB are
required. The action does not invoke Cargo, rust-analyzer, build scripts or macros.

Parsing and validation happen on a worker. Changes to the selection/document, switching
or closing the document, or a new completion request discard/cancel the previous job.
Edits apply only to the original buffer version and form one undo step. No files are
saved automatically.

`cargo test -p rc-app complete_statement::tests` exercises delimiter/string/Unicode
handling, tail expressions, control blocks, ambiguous input and cancellation. Its
native egui test invokes the shortcut in a real Cargo project, verifies no build script
or Cargo.lock appeared, undoes the edit, rejects a read-only/stale document, then
explicitly runs the repaired Cargo program successfully. Keymap tests cover platform
bindings and collisions. This does not claim parity with another installed IDE.
