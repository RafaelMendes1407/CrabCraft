# Changelog

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
