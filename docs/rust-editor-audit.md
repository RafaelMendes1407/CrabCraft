# Inherited editor behavior verified in Rust

The headless native editor tests in `editor_rust_audit.rs` create real Cargo
projects and compile the resulting Rust files with rustc. They exercise:

- Ctrl+D occurrence selection, typing at three carets, grouped undo/redo,
  Escape to collapse carets, Ctrl+Alt+Down to clone a caret, and Ctrl+Shift+L
  to select all occurrences. Non-ASCII text before the edits checks char offsets.
- Native typing of generic/reference lifetimes, loop labels, Unicode character
  literals, closures with arguments and empty `||` closures.
- Automatic ()/[]/{}/double-quote closing, typing over existing closing brackets
  and quotes, and tree-sitter Rust bracket matching on the edited result.
- Save/reload of Rust files with changed BOM and CRLF, plus modified-file conflict
  detection and the Load from Disk document path.

Two defects were fixed during verification. The inherited generic quote handler
inserted a second apostrophe while typing Rust lifetimes in blank contexts,
leaving invalid `&'a'` text. Rust now inserts apostrophes literally, since their
meaning is ambiguous until further text arrives. Selection wrapping and typing
over an existing closing character quote remain available. Ordinary brackets
and double quotes still close automatically; `|` stays literal for closures.

A disk reload previously retained the old line-ending mode. Encoding-only external
edits also failed to update the document BOM. Reload now adopts the external
file's line endings and BOM before marking it saved, so the next save preserves
the external encoding instead of silently rewriting it.

E-12 remains partial. Save and external reload are verified, but a dedicated
Revert from Disk action is absent (Load from Disk is offered on conflicts), and
the inherited document file I/O is still synchronous. This audit does not claim
parity with another IDE or exercise input against real operating-system windows.

```sh
cargo test -p rc-app editor_rust_audit
```
