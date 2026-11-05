# CrabCraft documentation

Start with the [project README](../README.md) for downloads, prerequisites and the native Rust
workflow. [FEATURES.md](../FEATURES.md) is the status of record: it records supported cases,
real-project evidence and remaining work. The current inventory has 246 ready, seven partial
and five out-of-scope entries. CrabCraft remains an early preview.

Release history is in [CHANGELOG.md](../CHANGELOG.md). A copyable Portuguese announcement
for Discord is available for [0.4.0](releases/0.4.0-pt-BR.md).

## Getting started

| Guide | What it covers |
| --- | --- |
| [Project trust](project-trust.md) | Opening without executing project code, explicit trust and tool execution boundaries. |
| [Rustup installation](rustup-installation.md) | Reviewed installation/tool discovery and its validation limits. |
| [Cloning projects](cloning-projects.md) | Explicit Git cloning and opening a generated workspace. |
| [Rust debugger setup](rust-debuggers.md) | GDB/LLVM LLDB DAP selection, prerequisites and adapter-specific capabilities. |
| [Rust keymap](rust-keymap.md), [key sequences](key-sequences.md) | Actions and shortcut customization. |
| [Accessibility](accessibility.md) | Editor text geometry, Project tree hierarchy and Linux/X11 Orca verification. |

Linux and Windows builds and core library tests are exercised by CI. That does not verify
interactive Windows desktop behavior or a real Windows/WSL installation. Individual guides
state the platforms and external tools actually exercised.

## Editor, navigation and refactoring

| Guides | What they cover |
| --- | --- |
| [Rust editor audit](rust-editor-audit.md), [editor migration](editor-migration.md) | Rust editing behavior and inherited implementation context. |
| [Find and replace](find-replace.md), [type hierarchy](type-hierarchy.md) | Searches, results and semantic hierarchy navigation. |
| [Change Signature](change-signature.md), [Move Items](moving-items.md) | Reviewed semantic refactorings and their refusal cases. |
| [Borrow-checker ranges](borrow-checker-ranges.md), [Complete Statement](complete-statement.md) | Compiler annotations and bounded source edits. |
| [Doctest completion](doctest-completion.md) | Rust examples embedded in documentation. |
| [Macro expansion](macro-one-step.md), [macro tester](macro-tester.md) | One-step expansion and isolated declarative macro testing. |
| [Large files](large-files.md), [document I/O](document-io.md) | File size policies, opening, navigation and guarded asynchronous edits. |
| [Background saving](background-saving.md), [revert from disk](revert-from-disk.md) | Save/format/execute ordering, cancellation, conflicts and undo. |
| [Workspace restoration](workspace-restoration.md), [Project tree reads](project-tree-io.md) | Bounded worker preparation, original session slots and cached rendering. |

## Cargo, execution and debugging

| Guides | What they cover |
| --- | --- |
| [Cargo dependencies](cargo-dependencies.md), [missing crates](missing-crates.md) | Reviewed manifest editing and dependency proposals. |
| [Coverage](coverage.md), [REPL variables](repl-variables.md), [wasm-pack](wasm-pack.md) | Optional tool prerequisites and explicit Cargo workflows. |
| [Debug sessions](debug-sessions.md), [parallel stacks](parallel-stacks.md) | Session selection and grouped thread stacks. |
| [Remote debugging](remote-debugging.md), [terminal debugging](terminal-debugging.md) | GDB server connections and terminal input behavior. |
| [Administrator execution](administrator-run.md), [administrator debugging](administrator-debugging.md) | Partial Linux PolicyKit flows, review requirements and unsupported operations. |
| [Ferrocene/CriticalUp](criticalup-provider.md) | Explicit toolchain selection; licensed compiler and compatible-service validation remain pending. |

## Git and terminal

| Guides | What they cover |
| --- | --- |
| [Git read safety](git-read-safety.md), [Git templates](git-templates.md) | Static inspection and reviewed configuration/template work. |
| [Shelves](shelves.md), [history editing](history-editing.md) | Saving local changes and explicit history operations. |
| [Hosting CLI](hosting-cli.md), [issue trackers](issue-trackers.md) | External hosting tools and reviewed issue actions. |
| [Terminal](terminal.md), [terminal completion](terminal-completion.md) | Native PTY sessions, trust and completion behavior. |
| [CLI comparison](cli-comparison.md) | Existing comparison methodology and its limits. |

## Optional integrations

These integrations use separately installed tools or services. Opening a project does not
start a model service, install a language server, deploy a file or run a container.

| Guides | What they cover |
| --- | --- |
| [Web languages](web-languages.md) | HTML/CSS/JS/TS syntax and explicitly configured external LSP servers. |
| [HTTP environments, OAuth and scripts](http-variables.md) | Shared CLI variables plus separately reviewed graphical-client actions. |
| [Axum endpoints](axum-endpoints.md) | Semantic routing, parameter/query hints and URL completion. |
| [Database tools](database.md), [SQL injection support](sql-injection.md) | SQLite/local PostgreSQL and editor SQL behavior. |
| [Local AI](local-ai.md) | On-demand local Ollama chat and reviewed source suggestions. |
| [ACP agents](acp-agents.md), [local plugins](local-plugins.md) | Explicit external processes, permissions and bounded protocols. |
| [Proofreading](proofreading.md) | A separately installed local LanguageTool service. |
| [Settings synchronization](settings-sync.md) | Reviewed transfer and selection of configuration. |

## Remote workspaces and deployment

| Guides | What they cover |
| --- | --- |
| [Remote Rust workspace](remote-development.md) | A verified SSH connection, independent editor buffers, remote RA/Cargo and reviewed writes. |
| [Remote files](remote-files.md), [SFTP/FTPS deployment](deployment.md) | Remote browsing and reviewed text/binary file transfers. FTP support means explicit TLS, not plaintext or implicit FTPS. |
| [WSL workspace](wsl-workspace.md) | Implemented transport with real Windows/WSL validation still pending. |
| [Docker tools](docker-tools.md) | Reviewed image/container CLI operations; actual container execution was blocked by the test host's AppArmor/runtime restriction. |
| [Dev Containers CLI](dev-containers.md), [existing-container workspace](container-workspace.md) | CLI integration and native RA/Cargo/editor sessions. Linux argv fixtures are not proof of execution through a Docker daemon. |

## Architecture and validation

[Architecture](architecture.md) describes the Cargo workspace and native UI conventions.
[Performance](performance.md) records measurement scope and [the inherited UI audit](inherited-ui-audit.md)
records earlier verification. Guides link to the relevant tests and describe external prerequisites.

The seven partial inventory entries are WSL, Ferrocene, Playground public upload, administrator
execution, administrator debugging, Docker and Dev Containers. Some have both missing runtime
validation and unsupported implementation scope. Read their individual entries before relying
on them. Public Playground upload is intentionally excluded from automatic tests because it
publishes code as a GitHub gist.

No comparison here establishes parity with another IDE. Bundled asset sources and notices live
in [the icon notice](../assets/icons/NOTICE.md) and [font documentation](../assets/fonts/README.md);
[third-party notices](../THIRD_PARTY_NOTICES.md) cover the distributed notices. Project licenses
remain [MIT](../LICENSE-MIT) or [Apache-2.0](../LICENSE-APACHE).
