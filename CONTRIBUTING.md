# Contributing to CrabCraft

CrabCraft is a native Rust/egui desktop IDE. Read [AGENTS.md](AGENTS.md) and the
[architecture](docs/architecture.md) before changing code. Keep existing DotCraft copyright
and license notices and the `rc-*` crate names so upstream changes remain mergeable.

## Build and check a change

Use current stable Rust, the checked-in Cargo.lock, and the
[platform dependencies](README.md#build-from-source).

```sh
cargo check --workspace --all-targets --locked
cargo test --locked -p rc-text -p rc-syntax -p rc-core -p rc-cargo --lib
```

Run focused tests for the changed behavior. Application/integration tests may need real
Cargo, rust-analyzer, Git or debugger adapters. GPU captures and external-service tests
state their prerequisites individually; do not run every ignored test indiscriminately.
See [security audit](docs/SECURITY_AUDIT.md) for independent audit commands and existing debt.

The repository's rustfmt.toml uses `max_width = 160` and `use_small_heuristics = "Max"`.
Keep broad formatting separate from behavior changes. Formatting/Clippy debt remains visible;
do not hide new failures by automatically renewing a static-audit exception.

## Implementation rules

Keep I/O and processes off the UI thread and make long work cancellable. Treat files,
settings and process output as untrusted input; return typed errors rather than panicking.
User-facing text goes through `rc_kit::i18n::tr`/`trf`, with translations in locales/.
Opening a project must not execute its build scripts, procedural macros or configured tools
before explicit [project trust](docs/project-trust.md).

## Feature status and documentation

[FEATURES.md](FEATURES.md) is the English status of record. Preserve stable feature IDs and
update the relevant row in the same commit as a feature change. Ready requires evidence on
a real Rust project. An inherited implementation, a compiling build or a mock is insufficient
to establish a platform workflow or parity with another IDE.

After changing the inventory, regenerate its summary:

```sh
python3 scripts/features_summary.py
```

Add user-facing instructions to the appropriate guide: prerequisites, where to find the
feature, a normal workflow and material limitations. Link new guides from
[the documentation directory](docs/README.md). Keep commands, menu labels and tool requirements
consistent with the implementation. Preserve evidence identifiers and links when translating.
Release announcements retain their original language and historical counts.

Screenshots must show the actual native interface and use shareable example projects without
credentials or private data. Record capture commands and prerequisites alongside the images.
Distributed assets need source/license records; see [third-party notices](THIRD_PARTY_NOTICES.md).

Submit small, reviewable commits in English. Describe the problem, resulting behavior and
validation in the PR. Use [SECURITY.md](SECURITY.md) for private vulnerability reports.
