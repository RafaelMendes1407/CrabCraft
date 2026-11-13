<p align="center">
  <img src="assets/icon.png" width="112" height="112" alt="CrabCraft application icon">
</p>

<h1 align="center">CrabCraft</h1>

<p align="center">
  <strong>A lightweight, native IDE for Rust.</strong><br>
  A single fast desktop binary built with Rust and egui.
</p>

<p align="center">
  <a href="https://github.com/RafaelMendes1407/CrabCraft/releases/latest"><img alt="Download for Windows" src="https://img.shields.io/badge/Download-Windows-e8590c?style=for-the-badge&logo=windows&logoColor=white"></a>
  <a href="https://github.com/RafaelMendes1407/CrabCraft/releases/latest"><img alt="Download for Linux" src="https://img.shields.io/badge/Download-Linux-e8590c?style=for-the-badge&logo=linux&logoColor=white"></a>
  <a href="https://github.com/RafaelMendes1407/CrabCraft/releases/latest"><img alt="Latest release" src="https://img.shields.io/github/v/release/RafaelMendes1407/CrabCraft?style=for-the-badge&label=latest&color=2b2b2b"></a>
</p>

![CrabCraft showing a Rust project](docs/screenshots/overview.png)

> **Status: early preview.** CrabCraft is tested with real Rust projects, but supported scope and
> platform verification vary by feature. The current inventory records **247 ready features,
> seven partial features and five out of scope**. [FEATURES.md](FEATURES.md) is the status of
> record. Start with [your first project](docs/getting-started.md), explore the
> [main feature guide](docs/main-features.md), or browse the [documentation directory](docs/README.md).

CrabCraft is a native Rust/egui desktop application, with no webview shell. It uses
[rust-analyzer](https://rust-analyzer.github.io/) for Rust code intelligence and tree-sitter for
syntax. It began as a port of [DotCraft](https://github.com/javy-fernandez/dotcraft) and keeps
that project's history and license notices. It is independent and does not claim feature parity
with RustRover.

## Install

Download the latest build from the [releases page](https://github.com/RafaelMendes1407/CrabCraft/releases/latest):

| Platform | File | How to use it |
| --- | --- | --- |
| Windows x64 | `CrabCraft-<version>-setup.exe` | Run the installer. |
| Windows x64 | `CrabCraft-<version>-windows-x64.zip` | Unzip it and run `CrabCraft.exe`. |
| Linux x86_64 | `CrabCraft-<version>-linux-x86_64.tar.gz` | Unpack it, then run `./install.sh` or `./crabcraft`. |

The builds are not code-signed, so Windows SmartScreen may warn. `SHA256SUMS.txt` lists the
checksums. Linux and Windows builds and core library tests run in CI. Interactive desktop,
debugger and screen-reader verification described in the guides was performed on Linux;
Windows desktop and real Windows/WSL sessions still need validation.

### What you also need

- A Rust toolchain from [rustup](https://rustup.rs/), with rust-analyzer:
  `rustup component add rust-analyzer`
- Git on `PATH`, for the Git features.
- GDB 14+ or LLVM `lldb-dap`, for debugging. The verified workflows are on Linux; see
  [debugger setup and capabilities](docs/rust-debuggers.md).
- Linux needs glibc 2.35+ and Vulkan or OpenGL ES, on X11 or Wayland. Windows needs Windows 10/11.

### Trust the project first

Opening a project never runs its code. Code insight starts after you click **Trust this project**
in the status bar, because rust-analyzer runs build scripts and procedural macros. Until then there
is no Rust language-server completion, navigation or error checking. See the
[trust boundary](docs/project-trust.md) for the full behavior.

## Features

The [main feature guide](docs/main-features.md) explains how these workflows fit together,
including setup, everyday actions and their limits. For help with inactive actions or
missing tools, see [troubleshooting](docs/troubleshooting.md).

### Code insight

![Rust method completion from rust-analyzer](docs/screenshots/completion.png)

Completion with documentation and auto-import, go to definition, find usages, rename, Refactor This
(`Ctrl+Alt+Shift+T`), quick fixes, inlay hints and Optimize Imports, powered by rust-analyzer.
[Change Signature](docs/change-signature.md), [Move Items](docs/moving-items.md),
[macro expansion](docs/macro-one-step.md), Cargo feature/cfg navigation and borrow-checker
ranges extend the Rust workflow. Each guide describes its supported cases.

### Debugging

![Stopped on a breakpoint with variables and inline values](docs/screenshots/debug.png)

Choose GDB or LLVM LLDB DAP. Supported actions include breakpoints, conditions, logpoints,
stepping, variables, watches and inline values, subject to the selected adapter's capabilities.
[Multiple debug sessions](docs/debug-sessions.md), [parallel stacks](docs/parallel-stacks.md),
[backend details](docs/rust-debuggers.md) have documented scopes.
Administrator execution and debugging remain partial; see the limitations below.

### Tests and Cargo

![A test run with its results](docs/screenshots/tests.png)

Run binaries, tests and benchmarks from the gutter, see the results in a tree, and apply
compiler suggestions from the build output. Run Anything accepts Cargo commands. Explore
[dependencies](docs/cargo-dependencies.md), [coverage](docs/coverage.md),
[REPL variables](docs/repl-variables.md) and [WebAssembly packaging](docs/wasm-pack.md). Tools such
as cargo-llvm-cov, evcxr and wasm-pack must be installed separately for their respective actions.

### Navigation

![Callers of a method in the Hierarchy window](docs/screenshots/call-hierarchy.png)

Call and type hierarchies, Search Everywhere (double `Shift`), breadcrumbs, bookmarks and Select In
(`Alt+F1`).

### Also included

- **Responsive document work:** cancellable background opening, saving, rustfmt, session
  restoration and Project tree reads; stale-result checks preserve edits. See
  [document I/O](docs/document-io.md) and [background saves](docs/background-saving.md).
- **Git and terminal:** changes, diff, blame, commits, shelves, history editing, worktrees and a
  native integrated terminal. See the [Git and terminal guides](docs/README.md#git-and-terminal).
- **Remote development:** a separate native Rust workspace over verified SSH, with remote
  rust-analyzer, Cargo and reviewed writes; [remote setup](docs/remote-development.md).
- **Deployment:** reviewed binary/new/existing file transfers over SFTP and explicit FTPS,
  with integrity checks and protocol limits; [deployment guide](docs/deployment.md).
- **Web and HTTP:** HTML/CSS/JS/TS syntax, explicitly configured external language servers,
  HTTP environments, reviewed OAuth/script actions and semantic Axum endpoint completion;
  [web languages](docs/web-languages.md), [HTTP](docs/http-variables.md), [Axum](docs/axum-endpoints.md).
- **Data and optional assistance:** native SQLite/local PostgreSQL tools, on-demand local
  Ollama assistance, ACP clients and reviewed local plugins. External services and tools are
  not bundled or started merely by opening a project; [integration guides](docs/README.md#optional-integrations).
- **Accessibility and localization:** eight interface languages, embedded CJK fallback fonts,
  and Linux/X11 source-text and Project tree verification with Orca/AT-SPI;
  [accessibility scope](docs/accessibility.md).

### Partial features and platform limits

The seven partial entries remain partial even when their controls or transport implementations
exist. Build success and a local executable fixture do not establish real target-platform support.

| Feature | Remaining work or verification |
| --- | --- |
| WSL toolchain/workspace | Validate the complete workflow on real Windows with WSL. Windows cross-compilation and Linux argument/transport fixtures already pass. |
| Ferrocene/CriticalUp | Validate a licensed Ferrocene installation and compatible services. Several language/REPL/wasm/coverage actions remain unsupported for this provider. |
| Rust Playground sharing | Exercise a real upload; it creates a public gist and is excluded from automatic tests. |
| Administrator execution | Validate actual OS authentication/elevation and complete the execution/PTY scope. Unsupported run options are rejected explicitly. |
| Administrator debugging | Validate a real authenticated PolicyKit/GDB session. Expressions, conditions, console, attach and restart are unsupported in this mode. |
| Docker | Real image build passed, but container execution was blocked by the test host's AppArmor/runtime restrictions. Compose, volumes and ports remain unsupported. |
| Dev Containers | CLI integration and an existing-container editor/RA/Cargo mode exist; real Up/Exec, IDE and EOF/cancellation verification need a working Docker runtime. No integrated debug or automatic attachment. |

Use [FEATURES.md](FEATURES.md) and the [detailed guides](docs/README.md) for exact limits and
validation evidence.

## Command line tools

```bash
crabcraft format /path/to/project
crabcraft format --check --offline /path/to/project
crabcraft inspect --trust-project --offline /path/to/project
```

Both commands accept a project folder or `Cargo.toml` and default to the current folder.
They run without opening a window and return Cargo's exit code. Formatting uses `cargo fmt
--all`; `--check` reports formatting changes without writing. Inspection uses `cargo clippy
--workspace --all-targets -- -D warnings`, so warnings also fail the check. Inspection requires
`--trust-project` on each invocation because build scripts and procedural macros may run.
`--offline` prevents Cargo network access. Use Ctrl+C to cancel.

`crabcraft http requests.http --request 2` sends the second HTTP request and writes a JSON
response (status, headers, body, duration and warnings) to stdout without opening a window.
For a file with one request, `--request` is optional; multiple requests require an explicit
selection. Exit status is 1 for HTTP errors, network errors or a truncated response. The
request file and response body are limited to 8 MB; requests time out after 60 seconds.
File variables such as `@host = localhost:8080` and `{{host}}` work in URLs, headers and
bodies. Missing variables and unsupported file bodies block sending. Explicit environment files
are supported:

```bash
crabcraft http requests.http --request 2 --env-file ./http-client.env.json --env local
```

The CLI never acquires OAuth tokens or executes response scripts. Those are separate, reviewed
graphical-client actions; see [HTTP environments, OAuth and scripts](docs/http-variables.md).

## Build from source

Use a current stable Rust toolchain and the checked-in `Cargo.lock` (edition 2024).

```bash
cargo run --locked --release -p rc-app
```

Linux also needs `libxkbcommon-dev` and `libwayland-dev`.

Run the tests that need no external tools:

```bash
cargo test --locked -p rc-text -p rc-syntax -p rc-core -p rc-cargo --lib
```

The command above covers library tests. Native application and integration tests use real tools
such as Cargo, rust-analyzer, Git and debuggers; prepare their dependencies and run:

```bash
cargo test --locked --workspace
```

Some tests are intentionally ignored because they need a GPU, an explicit external service or
public publication. Read their individual prerequisites; do not enable all ignored tests blindly.
See [testing and implementation guides](docs/README.md#architecture-and-validation).

## Architecture

CrabCraft is a Cargo workspace; each crate has one job. See
[docs/architecture.md](docs/architecture.md) and [docs/performance.md](docs/performance.md).

## Credits and license

CrabCraft is based on [DotCraft](https://github.com/javy-fernandez/dotcraft) by javy-fernandez and
the DotCraft contributors, and keeps its Git history.

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT)
at your option.

Icons in `assets/icons` come from the [IntelliJ Platform](https://github.com/JetBrains/intellij-community)
(Apache 2.0) and, for the Rust and Cargo file icons, the
[IntelliJ Rust plugin](https://github.com/intellij-rust/intellij-rust) (MIT); see [assets/icons/NOTICE.md](assets/icons/NOTICE.md). CrabCraft is not affiliated with
JetBrains, the Rust Foundation or Microsoft.

Bundled font sources and license details are recorded in [assets/fonts/README.md](assets/fonts/README.md).
See [third-party notices](THIRD_PARTY_NOTICES.md) for the distributed dependency and asset notices.
