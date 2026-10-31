<p align="center">
  <img src="assets/icon.png" width="112" height="112" alt="CrabCraft application icon">
</p>

<h1 align="center">CrabCraft</h1>

<p align="center">
  <strong>A lightweight, native IDE for Rust.</strong><br>
  A single fast desktop binary built with Rust and egui.
</p>

> **Status: early work in progress.** CrabCraft is being ported from
> [DotCraft](https://github.com/javy-fernandez/dotcraft), a native .NET IDE. Until the port is
> finished, parts of the application still target .NET. [FEATURES.md](FEATURES.md) tracks every
> planned feature and its real status.

CrabCraft aims to offer the workflow of a full Rust IDE in a native binary that starts in a moment
and stays responsive on large files. It takes its layout from the JetBrains New UI, and uses
[rust-analyzer](https://rust-analyzer.github.io/) for code intelligence and tree-sitter for syntax.

## Requirements

- Linux with glibc 2.35+ (Vulkan or OpenGL ES, X11 or Wayland), macOS 11+ or Windows 10/11.
- A Rust toolchain installed with [rustup](https://rustup.rs/).
- Git on `PATH` for the Git features.

## Build from source

CrabCraft needs Rust 1.90 or later (edition 2024).

```bash
cargo run --release -p rc-app
```

Run the tests:

```bash
cargo test --workspace
```

## Architecture

CrabCraft is a Cargo workspace; each crate has one job. See
[docs/architecture.md](docs/architecture.md) and [docs/performance.md](docs/performance.md).

## Credits and license

CrabCraft is based on [DotCraft](https://github.com/javy-fernandez/dotcraft) by javy-fernandez and
the DotCraft contributors, and keeps its Git history. The `upstream` remote points to that project.

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT)
at your option.

Icons in `assets/icons` come from the [IntelliJ Platform](https://github.com/JetBrains/intellij-community)
(Apache 2.0); see [assets/icons/NOTICE.md](assets/icons/NOTICE.md). CrabCraft is not affiliated with
JetBrains, the Rust Foundation or Microsoft.
