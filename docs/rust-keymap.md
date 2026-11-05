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

The RustRover family now selects its macOS or Windows/Linux reference profile for the host,
including after portable preferences are imported. Custom overrides retain their explicitly
recorded modifiers. The macOS counterpart compares 26 functional bindings with the public
IntelliJ-platform macOS reference; RustRover's own configuration help identifies macOS as
its default macOS keymap. Using that common-platform reference is an inference, not an
installed RustRover comparison. Tests emulate Command-key events in the native headless UI
on Linux with a real Cargo project; native NSApp/hardware behavior is not claimed as tested.

Complete Current Statement remains unsupported. Configurable two-stage sequences are implemented and
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

Additional public references for the macOS counterpart (2026-11-05):

- [RustRover keymap configuration](https://www.jetbrains.com/help/rust/configuring-keyboard-and-mouse-shortcuts.html)
- [Common platform macOS keymap](https://www.jetbrains.com/help/idea/reference-keymap-mac-default.html)

The corrected macOS assignments include parameter info (Command+P), generation (Command+N),
refactoring (Control+T), symbol search (Option+Command+O), bookmarks (F3 / Command+F3) and
attach (Option+Shift+F5). Existing editing bindings remain under their original notices.
