# Changelog

## 0.4.0

### New and improved
- **Rust editing:** reviewed Change Signature and Move Items refactorings, one-step macro
  expansion, borrow-checker ranges, hierarchy/navigation and Cargo dependency workflows.
- **Responsive workspace:** background document opening, saving, formatting, session restoration
  and Project tree reads with cancellation and stale-result checks.
- **Debugging:** GDB and LLVM LLDB DAP selection, multiple sessions and parallel stacks,
  with adapter-specific capabilities documented.
- **Remote work:** a native SSH Rust editor with remote rust-analyzer/Cargo and reviewed writes;
  SFTP and explicit FTPS file deployment with integrity checks and bounded protocol support.
- **Optional integrations:** web-language syntax and configured LSP servers, HTTP environments,
  reviewed GUI OAuth/response scripts, Axum endpoint assistance, SQLite/local PostgreSQL,
  local Ollama assistance, ACP agents and local plugins. Tools and services remain separate.
- **Documentation and distribution:** a complete documentation index, clearer trust and platform
  prerequisites, consistent package versions, and bundled third-party license notices.

### Validation and limitations
- [FEATURES.md](https://github.com/RafaelMendes1407/CrabCraft/blob/v0.4.0/FEATURES.md) records 246 ready features, seven partial and five out of scope.
  Ready entries cite real Rust-project evidence within their documented scope.
- The seven partial entries are real Windows/WSL workflow validation; licensed Ferrocene SDK
  validation; real public Playground gist upload; authenticated administrator execution;
  authenticated administrator debugging; Docker runtime execution; and Dev Containers runtime
  workflows. Several also retain unsupported operations. Docker testing was limited by host
  AppArmor/runtime restrictions; fixture success does not establish container execution.
- Linux and Windows CI build/core-test coverage does not verify interactive Windows desktop
  behavior. Desktop/debugger/accessibility evidence is documented per guide, primarily on Linux.
- This remains an early preview, with no claim of feature parity with RustRover. Opening a
  project does not run its code; Rust tool execution requires explicit project trust.
- Builds are unsigned. Optional integrations need separately installed tools/services and
  reviewed execution; consult [the documentation index](https://github.com/RafaelMendes1407/CrabCraft/blob/v0.4.0/docs/README.md) before use.

### Downloads
- `CrabCraft-0.4.0-setup.exe`: Windows installer.
- `CrabCraft-0.4.0-windows-x64.zip`: portable Windows build.
- `CrabCraft-0.4.0-linux-x86_64.tar.gz`: Linux archive with `install.sh`.
- `SHA256SUMS.txt`: release checksums.

## 0.3.0

### New
- **Git**: merge, rebase and cherry-pick with continue, skip and abort; a three-pane merge tool
  for conflicts; stashes, tags, worktrees and patches; GPG-signed commits; a filtered log and the
  history of a file.
- **Debugger**: breakpoint groups and a list of every breakpoint, Set Execution Point to Cursor,
  attach to processes by name, Force Step Into/Over and Force Run to Cursor, memory and
  disassembly views, setting variable values, integers in hexadecimal.
- **Cargo**: completion of Cargo.toml tables, keys, crate names and versions; offline
  inspections of Cargo.toml with quick fixes; locked and newest dependency versions with docs.rs
  links; each member's dependency tree; Cargo features turned on and off from the gutter, with a
  quick fix for a name a feature hides; the analysis target picked from the status bar; attached
  Cargo projects.
- **Code insight**: a preview of the highlighted Alt+Enter action, inspection profiles with
  per-inspection severity, Inspect Code with Clippy, rustc error codes explained in the IDE,
  imports added to pasted code, and code turned off by a false `#[cfg]` shown faded.
- **Refactoring**: rename previews its changes and lists conflicts before it applies them.
- **Run**: compound configurations, folders of configurations, temporary test configurations,
  several instances, configurations stored in the project, before-launch tools, and a terminal
  emulated in the Run console.
- **Tools**: an HTTP Client for `.http` files, a profiler with `perf`, flame graph and call tree,
  external tools, and rustup updates and components from Settings.
- **Editor**: live templates, copyright headers, sharing code on the Rust Playground, a code
  preview on the scrollbar, Alt+Q for the enclosing declarations, LightEdit for files opened
  without a project, scopes and file colors, `.editorconfig` and `rustfmt.toml` respected, and
  Rust scratch files that are analysed and can be run.
- **Projects**: new procedural macro and WebAssembly projects.
- **Keymap**: a VS Code keymap, and shortcuts recorded, removed and reset in Settings.
- **Command line**: open files at a line and column in the running window.

### Fixed
- Alt+Enter in a split editor opens at the active editor, and clicking an action applies it.
- Quick fixes that rename a file, such as rust-analyzer's "Rename to `snake_case`" for a module,
  rename it.
- New items in a Rust project are no longer offered as C# (`.cs`) files.
- The bottom tool windows span the whole width, under the explorer too.
- Settings keeps one size for every page, and the Keymap list fills the page.
- The window has an icon on Linux that matches the desktop entry.

### Limitations
- This is still an early preview. [FEATURES.md](FEATURES.md) lists what is done and what is
  missing, in Portuguese.
- It has not been compared feature by feature with RustRover.
- The HTTP Client does not support variables, environments or scripts yet. The profiler needs
  `kernel.perf_event_paranoid` at 2 or less, and the window explains how to set it.
- rust-analyzer starts only after you choose **Trust this project** in the status bar.
- The builds are not code-signed, so Windows SmartScreen may warn.

### Downloads
- `CrabCraft-0.3.0-setup.exe`: Windows installer.
- `CrabCraft-0.3.0-windows-x64.zip`: portable Windows build.
- `CrabCraft-0.3.0-linux-x86_64.tar.gz`: Linux build. Unpack it and run `./install.sh` or
  `./crabcraft`.
- `SHA256SUMS.txt`: checksums.

## 0.2.0

### New
- **Editor**: editor groups with tab drag and drop and floating windows, sticky lines, and Local
  History of saved versions with a diff preview and revert.
- **Code insight**: expand the macro call at the caret into a read-only tab, derive traits from
  the Generate popup, and suppress a rustc or Clippy lint with `#[allow]` from Alt+Enter.
- **Search**: Ctrl+P Search Everywhere with Classes and Text tabs and a scope selector.
- **Refactoring**: renaming a file lists the references to update.
- **Project**: environment variables applied to Cargo and rust-analyzer, Invalidate Caches without
  closing the project, and folders and files excluded from the index, search and rust-analyzer.
- **Tests**: run Rust tests with cargo-nextest.
- **rustfmt**: configurable arguments, channel and environment.
- **Explorer**: icons for Rust and Cargo files and for source and test folders.
- **Scratch files** kept outside the project and listed to reopen.

### Fixed
- Go to Declaration from the context menu and from Find Action, where it is also found as
  Go to Definition (#138).
- Cut, Copy and Paste from the editor's context menu (#137).
- Find in Files suggests `*.rs` as the file mask instead of `*.cs` (#136).
- Find Usages always replaces the Find window's results (#135).
- Search Everywhere results grow with the window and keep their scroll position (#132).
- The branches popup scrolls inside the window (#131).
- On Windows, rust-analyzer and Cargo paths match the open files, so Ctrl+click reaches
  definitions.

### Limitations
- This is still an early preview. [FEATURES.md](FEATURES.md) lists what is done and what is
  missing, in Portuguese.
- It has not been compared feature by feature with RustRover.
- Macro expansion only expands recursively; expanding one step is not supported yet. Rust scratch
  files are not analysed by rust-analyzer and cannot be run yet.
- rust-analyzer starts only after you choose **Trust this project** in the status bar.
- The builds are not code-signed, so Windows SmartScreen may warn.

### Downloads
- `CrabCraft-0.2.0-setup.exe`: Windows installer.
- `CrabCraft-0.2.0-windows-x64.zip`: portable Windows build.
- `CrabCraft-0.2.0-linux-x86_64.tar.gz`: Linux build. Unpack it and run `./install.sh` or
  `./crabcraft`.
- `SHA256SUMS.txt`: checksums.

## 0.1.0

The first public preview of CrabCraft, a native Rust IDE based on DotCraft.

### Included
- **Code insight** with rust-analyzer: completion with documentation and auto-import, navigation,
  find usages, rename, Refactor This, quick fixes, inlay hints, call and type hierarchies.
- **Cargo**: build, run and test from the gutter, the Cargo tool window, Run Anything, compiler
  suggestions you can apply, and new projects and crates.
- **Debugging** with GDB: breakpoints, conditions, logpoints, stepping that skips the standard
  library, variables, watches and inline values.
- **Editor**: tree-sitter syntax, multiple carets, folding, bookmarks, breadcrumbs, Select In and
  an integrated terminal.
- **Git**: changes, diff, blame and commit.

### Limitations
- This is an early preview. [FEATURES.md](FEATURES.md) lists what is done and what is missing, in
  Portuguese.
- It has not been compared feature by feature with RustRover.
- Debugging was only tested on Linux. The Windows build compiles, but it has not been used on a
  real Windows machine yet.
- rust-analyzer starts only after you choose **Trust this project** in the status bar, because it
  runs the project's build scripts and procedural macros. Completion stays off until then.
- The builds are not code-signed, so Windows SmartScreen may warn.
- Inherited .NET code is still in the source tree.

### Downloads
- `CrabCraft-0.1.0-setup.exe`: Windows installer.
- `CrabCraft-0.1.0-windows-x64.zip`: portable Windows build.
- `CrabCraft-0.1.0-linux-x86_64.tar.gz`: Linux build. Unpack it and run `./install.sh` or
  `./crabcraft`.
- `SHA256SUMS.txt`: checksums.
