# Rust shortcut reference profile

Settings → Keymap offers **RustRover (Windows/Linux)** alongside the existing Rider,
Visual Studio and VS Code profiles. Existing saved choices and user overrides are kept.
The new profile uses the existing IntelliJ-style editing bindings, then replaces the
conflicting primary shortcuts below. Thus Ctrl+P asks for parameters and Ctrl+Shift+R
opens replacement across files, instead of their inherited search/rename assignments.

| Operation | Windows/Linux binding |
| --- | --- |
| Action search | Ctrl+Shift+A |
| File search | Ctrl+Shift+N |
| Parameter popup | Ctrl+P |
| Context actions | Alt+Enter |
| Next/previous issue | F2 / Shift+F2 |
| Recent files | Ctrl+E |
| Format | Ctrl+Alt+L |
| Refactoring menu | Ctrl+Alt+Shift+T |
| Extend/shrink selection | Ctrl+W / Ctrl+Shift+W |
| Line/block comment | Ctrl+/ / Ctrl+Shift+/ |
| Declaration / usages | Ctrl+B / Alt+F7 |
| Rename / replace in files | Shift+F6 / Ctrl+Shift+R |
| Run / debug | Shift+F10 / Shift+F9 |
| Stop / resume | Ctrl+F2 / F9 |
| Toggle breakpoint | Ctrl+F8 |
| Step into/over/out | F7 / F8 / Shift+F8 |
| Run/force run to cursor | Alt+F9 / Ctrl+Alt+F9 |

Unit tests compare 26 action bindings and check duplicate chords. A native headless test
selects the profile through Settings in a real Cargo project, persists the choice and uses
its action/file search shortcuts without starting project code; the Rust source compiles.
Validation ran on Linux. This compares published shortcut facts, not the installed
RustRover product, and makes no product parity claim.

The RustRover-specific macOS profile is not implemented; its Windows/Linux choice is
visibly disabled on macOS. Rider and VS Code macOS profiles remain available. Complete
Current Statement remains unsupported. Configurable two-stage sequences are implemented and
verified through native recording and real Rust editor actions; see [key sequences](key-sequences.md). Extra bindings
inherited from the existing editing map have not all been compared with the reference.

Public sources consulted on 2026-11-04:

- [RustRover keyboard overview](https://www.jetbrains.com/help/rust/mastering-keyboard-shortcuts.html)
- [Reference popups](https://www.jetbrains.com/help/rust/viewing-reference-information.html)
- [Project context actions](https://www.jetbrains.com/help/rust/project-tool-window.html)
- [Test run/debug shortcuts](https://www.jetbrains.com/help/rust/performing-tests.html)
- [Breakpoints](https://www.jetbrains.com/help/rust/using-breakpoints.html)
- [Stepping](https://www.jetbrains.com/help/rust/stepping-through-the-program.html)
- [Common platform debugging shortcuts](https://www.jetbrains.com/help/idea/run-debug-and-test-scala.html)

No reference-card PDFs, proprietary assets or logos are distributed. Binding code and this
comparison are original CrabCraft work under the repository's MIT/Apache-2.0 licenses.
