# CrabCraft architecture

CrabCraft is a Cargo workspace (`crates/*`, Rust edition 2024) for a native Rust IDE built
with eframe/egui 0.36 + wgpu 30. The UI thread never blocks: background work runs on threads
or on the shared tokio runtime and reports back through channels that wake the UI.

| Crate | Role |
| --- | --- |
| `rc-app` (bin `crabcraft`) | Window, tool windows, editor tabs, keymaps, session. Only crate that wires everything together. |
| `rc-kit` | Islands theme (`theme`), New UI icons (`icons`, SVGs in `assets/icons`), window chrome, branding, i18n (`i18n::tr`). |
| `rc-core` | Shared tokio `Runtime`, `Inbox`/`Outbox`/`Waker`, settings, per-workspace session, workspace file tree, file watcher. |
| `rc-text` | Rope buffer, change sets, multi-cursor selections, grouped undo, UTF-16 positions. No UI. |
| `rc-syntax` | tree-sitter parsing on a background thread, highlighting, folding, bracket matching. |
| `rc-editor` | The code editor widget (egui), keymaps (`keymap::Action`), code-insight popups. |
| `rc-lsp` | JSON-RPC/LSP client, rust-analyzer, and explicitly configured web-language servers. |
| `rc-dap` | DAP client, GDB and LLVM LLDB DAP sessions, and inherited netcoredbg support. |
| `rc-cargo` | Cargo: toolchain, workspace model, build, run and test. |
| `rc-dotnet` | SDKs, solutions/projects, MSBuild, build/run/test, TRX. |
| `rc-vcs` | Git through the `git` CLI. |
| `rc-search` | File index, fuzzy search, find/replace in files. |
| `rc-term` | Terminal emulator (alacritty_terminal + portable-pty) rendered in egui. |

## Conventions

- **Threads.** Never block the UI thread on I/O or processes. Long work runs on
  `rc_core::Runtime::spawn` (tokio) or a dedicated thread and sends results through an
  `rc_core::Outbox<T>` (its `send` wakes the UI with `request_repaint`). The UI drains its
  `rc_core::Inbox<T>` once per frame.
- **Cancellation.** Every long operation (build, test run, search, LSP request) can be
  cancelled; dropping a handle or calling `cancel()` must stop the process / task.
- **Text.** Everything user-facing goes through `rc_kit::i18n::tr` / `trf` (English source
  text). Translations live in `locales/*.json`, imported from `scripts/i18n/*.tsv` with
  `python3 scripts/i18n_tsv.py <area> scripts/i18n/<file>.tsv` (columns: en, es, fr, pt-BR,
  de, pl, zh-Hans, ja). The test `every_text_in_the_code_has_a_translation` fails on any
  missing entry.
- **Look.** Colours come from `rc_kit::pal(ui)` (UI) and `rc_kit::theme::scheme(dark)`
  (editor/terminal), icons from `rc_kit::ico::*` painted with `rc_kit::icons::paint` (16 pt,
  pixel-snapped). Row height 24 pt for trees/lists, 36 pt tool window headers, 13 pt text.
- **Tests.** Use a current stable Rust toolchain with the checked-in lockfile. Core library
  tests run without a window; UI widget tests use `egui_kittest` headlessly. GPU captures
  and tests needing external services or publication are individually ignored with prerequisites.
  Rust integration tests use real Cargo projects and explicitly installed tools such as
  rust-analyzer, Git, GDB or LLVM `lldb-dap`. Optional HTTP, database, SSH, container, AI and
  coverage workflows document their own prerequisites; a fixture or build does not prove a
  real target-platform workflow. Inherited .NET fixtures remain separate from Rust evidence.
  See [the documentation index](README.md) and [FEATURES.md](../FEATURES.md).
- **Commits.** Make small, reviewable commits with short English messages. Update
  `FEATURES.md` in the commit that changes a feature and run
  `python3 scripts/features_summary.py`. Mark a feature ready only with real Rust-project evidence.

## Packaging

Fonts, icons and translations are embedded in the binary. External toolchains, language
servers, debuggers and optional services have separate installation and trust requirements;
opening a project must not install or execute them automatically.

The official [release workflow](../.github/workflows/release.yml) builds a Linux x86_64
archive, Windows x64 portable archive and Windows installer. Tag publication uses the matching
workspace version, the version section in [CHANGELOG.md](../CHANGELOG.md), and SHA-256 checksums.
Manual workflow runs produce artifacts without publishing a release. The Linux package includes
its desktop entry, icon and installer; Windows packaging checks runtime DLL dependencies.

`scripts/copy_notices.py` copies project licenses and third-party asset notices into each
package while preserving their relative paths. Source and license details are in
[THIRD_PARTY_NOTICES.md](../THIRD_PARTY_NOTICES.md). Keep these files in distributed packages.

The optional `scripts/bundle.sh` supports macOS application bundles, universal binaries, DMG/ZIP,
and cargo-xwin Windows builds. These are separate from the official Linux/Windows release jobs;
their existence is not evidence of tested macOS desktop support. Application icons are generated
from `assets/icon.svg` by `scripts/make_icon.py`; Windows embeds `assets/icon.ico` through
`crates/rc-app/build.rs`.
