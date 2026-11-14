# Security and compliance audit

This is a technical self-assessment pipeline, not a certification of the
application or of regulatory compliance. The design keeps the existing native
Rust app and its build/release workflows intact.

## Architecture

`scripts/security-audit.sh` is the local entry point. Its Python orchestrator
executes each installed tool without a shell, captures stdout/stderr and exit
codes, and writes `security/reports/security-audit.json` plus a Markdown
summary after every check. `PASS`, `FAIL`, `WARNING`, `SKIPPED` and `ERROR`
remain distinct. An absent tool or malformed output is `ERROR`, never `PASS`.
Generated evidence is ignored by Git and should be reviewed before sharing.

The dedicated [workflow](../.github/workflows/security-audit.yml) runs on PRs
to `main`, pushes to `main`, Monday schedules and manual dispatch. Separate
Linux jobs run dependency audit, policy checks, static analysis, unsafe scan,
secret scan, SBOM and security tests. Fuzz smoke tests run on schedules and
manual dispatch only; PRs do not spend unlimited time fuzzing. Every job
uploads its report even if the check fails, and an always-running final job
combines the job reports. A missing job artifact is `ERROR`.

## Local commands and tools

From the repository root:

```sh
./scripts/security-audit.sh all
./scripts/security-audit.sh dependencies licenses
./scripts/security-audit.sh static
./scripts/security-audit.sh unsafe
./scripts/security-audit.sh secrets
./scripts/security-audit.sh sbom
./scripts/security-audit.sh tests
./scripts/security-audit.sh fuzz
```

Exit codes: `0` for PASS/WARNING, `1` for FAIL, `2` for ERROR. The script needs
Python 3, Rust/Cargo and the relevant tools in `PATH`: cargo-audit 0.23.0,
cargo-deny 0.20.2, cargo-geiger 0.14.0, cargo-cyclonedx 0.5.10, Gitleaks
8.30.1 and cargo-fuzz 0.13.2 with a nightly toolchain for fuzzing. The workflow
installs pinned tool versions. Gitleaks is downloaded with a checked SHA-256.
For a local scan with an already populated RustSec database, set
`SECURITY_AUDIT_OFFLINE=1`; this still requires a writable lock in Cargo's
advisory database directory. `GITLEAKS_BIN` may point to a locally installed
binary. `SECURITY_FUZZ_TOOLCHAIN` selects the nightly toolchain; the workflow
uses `nightly-2026-11-11`.

`cargo fmt --all -- --check` and
`cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`
cover the entire workspace; the crates currently have no Cargo feature flags.
Linux executes these checks. The existing CI builds/tests Linux and Windows;
the release workflow packages both. macOS-only Objective-C code needs a
separate platform review and is not built by this audit workflow.

## Policies and exceptions

`deny.toml` checks RustSec advisories, the licenses already present in the
resolved dependency graph and third-party notices, crates.io as the permitted
registry, unknown Git sources and wildcard/duplicate versions. Parallel
versions are recorded as warnings because the GUI/Windows graph contains
multiple upstream major versions. Local `rc-*` path dependencies appear as
wildcards in Cargo metadata and are also warnings. Any new external Git source
or registry fails. Review licenses against `THIRD_PARTY_NOTICES.md` before
changing the allow list; the list is a distribution policy, not proof that a
package is legally compatible in every use.

Critical and high vulnerability advisories block. Medium findings require
assessment and stay blocked until reviewed. The cargo-audit report records low
and informational findings as warnings, but cargo-deny 0.20.2 treats every
vulnerability advisory as an error, so the combined CI remains blocked until
any such finding is reviewed. Advisories without a reliable severity also block
until reviewed. Only an explicit numerical CVSS score is classified: critical
9.0–10, high 7.0–8.9, medium 4.0–6.9, low 0.1–3.9, informational 0.
A CVSS vector or title alone does not provide a score to this pipeline. This
conservative cargo-deny behavior is stricter than the low/informational
tracking threshold; review the finding before any narrow exception.
Unmaintained-package notices are warnings.

Exceptions require an identifier, narrow scope, justification, owner, review
date and expiry in `security/exceptions.json`. The two current Gitleaks
exceptions are exact historical fingerprints of a synthetic test signature
key; the script checks expiry and matches them to `.gitleaksignore`. No RustSec
vulnerability is ignored. A future RustSec exception must be reviewed and
represented in both the policy and the report; adding a broad allowlist or
discarding a failed command is not acceptable.

## Evidence and coverage

The root `Cargo.lock` and independent `fuzz/Cargo.lock` are both audited;
their Cargo graphs are also checked by cargo-deny. `cargo-audit` writes
structured advisory results; cargo-deny writes JSON
diagnostics. `cargo-geiger` scans Rust unsafe use in `rc-app` and dependencies
and the audit writes an own-source inventory. See [unsafe-review](../security/unsafe-review.md)
for the safety arguments and platform review priorities. Unsafe presence is
not itself a vulnerability. Gitleaks scans the Git history with redaction and
no persisted checkout credentials; it does not see ignored or untracked files.

`cargo-cyclonedx` creates one CycloneDX JSON per workspace crate with resolved
dependency versions, package identifiers and licenses when available. The
`rc-app` SBOM is copied to `security/reports/sbom.cdx.json`; all crate SBOMs
remain under `sbom-components/`. These files inventory Cargo packages, not a
complete installed system. The binary additionally depends on bundled SQLite,
tree-sitter grammars, platform libraries and distributed font/icon assets.
`THIRD_PARTY_NOTICES.md`, `assets/fonts/README.md`, release `ldd` output and
Windows DLL inspection supply separate evidence; do not claim these are fully
represented in CycloneDX without verifying the built package.

Security tests exercise session/path handling in `rc-core` and SQLite limits,
authorization and import/export in `rc-app`. The fuzz targets exercise
untrusted session JSON/environment text and Cargo manifest inspection, capped
at 16 KiB per input and ten seconds per target in the smoke job. AddressSanitizer
remains enabled; LeakSanitizer is disabled because some container ptrace policies
prevent it from completing. Deep fuzzing
and corpus triage are manual follow-up work.

Filesystem review: `rc-core::paths::write_atomic` uses an exclusive sibling
temporary file and rejects a symlink destination. Opening a workspace must not
execute `build.rs` or proc macros before trust. Review remaining boundaries
for symlink swaps, traversal, oversized files, temporary permissions and
external process output, especially deployment, WSL, privileged helpers and
settings backups. The audit inventories risks; it does not prove all paths
safe or modify application behavior.

## Current baseline and maintenance

At introduction, RustSec reported no known vulnerability in the main
`Cargo.lock`, but it warned about unmaintained transitive `zbus_names`,
`zvariant` and `zvariant_derive`. `cargo fmt --check` and strict Clippy failed
on pre-existing formatting/lint debt across 277 Rust files. Source changes later
invalidated the exact-source exception, causing the static job and consolidated
report to fail while builds and tests passed. The workspace is now formatted and
strict Clippy findings are corrected in separate commits. The baseline file and
its downgrade logic have been removed: either command failing blocks the audit.
A regression test verifies that formatting and/or Clippy failures remain FAIL.
`cargo-deny` 0.20.2 also emits internal `unresolved-workspace-dependency`
diagnostics for some inherited workspace aliases. The result is WARNING,
with raw diagnostics retained for review; no policy error is ignored.

When updating dependencies or tools, review upstream release notes and
licenses, update the pinned version and checksum/SHA, rerun the local audit,
then inspect the CI artifacts. Source checksums in `Cargo.lock` and locked
Cargo resolution provide registry integrity; they do not attest the build
scripts or external native libraries. Third-party GitHub Actions are pinned
to commit SHAs, checkout does not persist credentials, jobs use read-only
repository permission, and PR code receives no secrets.

Primary references: [RustSec cargo-audit](https://github.com/rustsec/rustsec/tree/main/cargo-audit),
[cargo-deny](https://embarkstudios.github.io/cargo-deny/),
[cargo-geiger](https://github.com/rust-secure-code/cargo-geiger),
[cargo-fuzz](https://github.com/rust-fuzz/cargo-fuzz),
[CycloneDX Rust](https://github.com/CycloneDX/cyclonedx-rust-cargo),
[Gitleaks](https://github.com/gitleaks/gitleaks).
