# Response to `short_audit.md` (2026-11-12)

The supplied short audit is a useful list of checks, not a certification of CRA,
NIS2 or GDPR compliance. Its scores and comparisons with other products have no
published test method. This document records what was verified in this repository.

| Audit item | Result |
| --- | --- |
| `libsqlite3-sys` RUSTSEC-2026-0459 | Updated to 0.39.1 through `rusqlite` 0.41.1. |
| `pulldown-cmark` RUSTSEC-2026-0433 | Updated to 0.13.6. The advisory requires `ENABLE_WIKILINKS`; the dependency is patched regardless. |
| `rustix` RUSTSEC-2026-0486 | Updated the affected 1.x series to 1.1.6. The 0.38 series is unaffected. |
| Dependency checks and inventory | CI runs cargo-deny (advisories, licenses and sources), cargo-audit and generates CycloneDX SBOM artifacts. Dependabot checks Cargo and Actions weekly. |
| Erasure APIs | `Session::clear(workspace)` deletes a persisted session; `Settings::clear()` deletes preferences and the unreadable-file backup. A complete data-erasure workflow still needs to cover local history, scratch files, logs, project-local configuration and external backups. |
| Roslyn download/TLS pinning | Not applicable: Roslyn download and its URL were removed before this audit. Rust tooling does not use that URL. |
| `rc-dotnet` isolation | Dotnet-specific modules were removed before this audit. The crate name remains for upstream compatibility and contains shared process/types code. |
| `Session.project_env` | `parse_env` validates `NAME=value` lines before they are supplied to Cargo or rust-analyzer. The persisted representation remains a string to preserve existing session files and the editor draft. |
| `FileIndex` TTL | The index is an in-memory `Arc` snapshot, not a persistent database. Its contents are released when the workspace and its remaining references close. A time-to-live that evicts an open project's index would break search without adding disk erasure. |
| `clippy::expect_used` | Not enforced workspace-wide yet: production code still contains `expect` calls. Enabling a deny lint before replacing these calls would break CI. |
| Encryption at rest | Not implemented. Settings and sessions remain local JSON. A useful design needs OS-backed key management, migration, backup handling, and recovery behavior; storing a key next to encrypted files would give little protection. GDPR Article 32 calls for measures appropriate to risk, including encryption *as appropriate*, rather than mandating AES-GCM for every local JSON file. |
| Security logging and rotation | Not implemented. A privacy-aware event schema, retention policy and incident process are needed before writing an audit trail. Adding a `tracing` dependency alone would not establish NIS2 compliance. |

The updated `Cargo.lock` was checked locally with `cargo audit` and `cargo deny
check advisories licenses sources`. No vulnerability advisory remained. RustSec
still reports `zbus_names`, `zvariant` and `zvariant_derive` as unmaintained
transitive dependencies without a fixed upgrade. The four SQLite integration
tests and the `rc-core` unit tests passed; the app passed `cargo check`.

Sources: [SQLite advisory](https://rustsec.org/advisories/RUSTSEC-2026-0459.html),
[Markdown advisory](https://rustsec.org/advisories/RUSTSEC-2026-0433.html),
[rustix advisory](https://rustsec.org/advisories/RUSTSEC-2026-0486.html),
[GDPR Article 32](https://eur-lex.europa.eu/eli/reg/2016/679/oj/eng/).
