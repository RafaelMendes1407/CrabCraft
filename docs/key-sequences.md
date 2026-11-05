# Two-step keyboard shortcuts

Settings → Keymap offers **Record two-step shortcuts**. Enable it, filter an action and use
Record Shortcut, then press both steps. The first must include Ctrl or Command; the second
can include modifiers or be an ordinary key. Escape cancels recording. Saved settings use
the existing shortcut map, for example `ctrl+K ctrl+S`; older single-step values are retained.

The prefix no longer triggers a conflicting single-step action. Recording a conflicting
custom shortcut disables the previous custom assignment. Menu/tooltips show both steps;
native macOS accelerators are rebuilt when the keymap changes and do not capture half a
sequence. The app dispatches complete sequences through its normal action handling, so
project trust and read-only editor restrictions still apply.

A runtime prefix expires after two seconds and cancels on Escape, mouse input, focus loss
or IME composition. An unknown second key is handled normally. A matching unmodified second
key does not type its character into the document, including delayed character events and
held-key repeats. Closing Settings while recording clears the recorder; it cannot leave
global shortcuts disabled. Standalone editor widgets also support the same sequences.

Native tests record, persist and execute a settings sequence in a genuine Cargo project,
exercise timeout/cancel and fold/unfold commands, verify no extra typed character and confirm
opening the project starts no build script. A separate native editor test verifies editing
sequences and read-only source protection. The UI test simulates 60 Hz input so separate
key-down/key-up/text events represent ordinary desktop timing. Run
`cargo test -p rc-app key_sequence_audit` and
`cargo test -p rc-app -p rc-editor keymap::tests`.

This completes configurable sequences, not every other editor's predefined multi-step alias.
The RustRover macOS counterpart is covered separately in [the reference profile](rust-keymap.md).
Complete Current Statement is covered in [statement completion](complete-statement.md).
No product parity is claimed.
