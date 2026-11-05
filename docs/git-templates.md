# Git project templates

New Project › Git Template generates a Cargo project from a Git repository URL
or local Git repository. Set the name, parent folder and optional branch, then
review the repository and check **I trust this template repository**. Changing
the source or branch resets that decision. The generated project still goes
through CrabCraft's normal project trust handling when it opens.

Install cargo-generate 0.26 or newer separately:

```sh
cargo install --locked cargo-generate
```

Generation runs off the UI thread and Cancel closes the dialog and terminates
the process group. Errors preserve any partial destination for inspection;
existing folders are never overwritten. A result without Cargo.toml is refused.

The invocation uses noninteractive mode. Template placeholders need defaults;
custom value entry and nested template selection are not implemented. Git
submodules, automatic changes to a parent workspace, formatting and template
shell commands are disabled. Template hook scripts can still run inside
cargo-generate, which is why the separate trust decision is required.

References (cargo-generate is MIT OR Apache-2.0):

- https://github.com/cargo-generate/cargo-generate
- https://cargo-generate.github.io/cargo-generate/templates/scripting.system-commands.html
- https://docs.rs/cargo-generate/latest/cargo_generate/struct.GenerateArgs.html

Validation includes a headless native dialog test and a real local Git template
that generates a Rust crate. Its build.rs is not run by generation; a subsequent
explicit cargo test builds and tests the generated crate. The same integration
test verifies refusal of existing destinations and shell-command hooks.

```sh
CRABCRAFT_CARGO_GENERATE=/path/to/cargo-generate cargo test -p rc-cargo real_git_template -- --ignored
cargo test -p rc-app git_template_dialog
```
