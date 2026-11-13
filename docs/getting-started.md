# Your first Rust project

This walkthrough takes you from an installed CrabCraft to editing, running and testing a
small Cargo project. For downloads and platform prerequisites, start with the
[installation instructions](../README.md#install).

## Prepare the tools

Install a stable Rust toolchain and make Cargo available on your PATH. Install the language
server with `rustup component add rust-analyzer`. Git is needed for version control;
GDB 14+ or LLVM `lldb-dap` is needed only for debugging. These tools are separate from
the CrabCraft application. Settings → Tools lets you configure executable paths.

## Create or open a workspace

Use the new-project dialog to create a Binary or Library, or open a folder containing
`Cargo.toml`. You can also create the example from a terminal:

```sh
cargo new hello-crabcraft
crabcraft ./hello-crabcraft
```

Opening an existing workspace begins in Restricted mode unless you previously trusted
its canonical folder. You can read, edit and search files immediately. Cargo discovery,
rust-analyzer, terminals, build and run actions require **Trust this project** in the status
bar. Review unfamiliar manifests, build scripts, toolchain files and Cargo configuration
before trusting them. Trust permits execution; it does not sandbox the project.
See [project trust](project-trust.md).

## Edit and navigate

Open `src/main.rs` from Project. Syntax highlighting works before trust; semantic completion,
diagnostics and navigation need a trusted project and a running rust-analyzer.
Use **Search Everywhere** to find files or **Find Action** to locate commands by name.
The ☰ main menu also exposes editing, build, run and tool-window commands.

Select your preferred profile in Settings → Keymap. The
[shortcut reference](rust-keymap.md) describes the RustRover profile; shortcuts vary by
profile and platform. Settings → Editor controls the editor font and save options.

## Build, run and test

After trusting your own example, choose **Build Project**. Compiler output includes file
locations; use a diagnostic to return to the source. Select the binary's run configuration
and choose **Run**, or use its gutter action. The Run window displays program output.
Use **Stop** to cancel an active operation.

For a library example, add this test to `src/lib.rs`:

```rust
pub fn double(value: i32) -> i32 {
    value * 2
}

#[cfg(test)]
mod tests {
    #[test]
    fn doubles_a_number() {
        assert_eq!(super::double(21), 42);
    }
}
```

Use its gutter action or **Run Tests**, then inspect the result tree in Tests. Ordinary
Cargo tests include doctests; the optional nextest runner does not run doctests.
See [the main feature guide](main-features.md#build-run-and-test).

## Debug and continue

In Settings → Tools, select an installed GDB or LLDB adapter. Put a breakpoint on an
executable line, select the binary configuration and choose **Debug**. When it stops,
inspect variables and the call stack, then step or resume. If a capability is unavailable,
check [debugger setup](rust-debuggers.md) and the selected adapter.

Continue with the [main feature guide](main-features.md), the
[documentation directory](README.md) or the complete [feature inventory](../FEATURES.md).
For missing tools or inactive actions, see [troubleshooting](troubleshooting.md).
