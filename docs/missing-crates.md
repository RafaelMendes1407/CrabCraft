# Missing Cargo dependency proposals

The native editor analyses Rust text locally after a 450 ms pause. A paste schedules the
same analysis immediately and identifies references inside the pasted range. No Cargo
command, registry lookup or manifest edit occurs automatically. This works without a
running language server once the Cargo member model has loaded. Loading that model now
requires project trust under P26; an untrusted opening does not invoke Cargo to discover
members and therefore does not display member-dependent proposals.

**Review dependency…** opens the existing Cargo Dependencies editor with the owning
member, package name and dependency section filled in. A verified workspace library also
fills its local path. Review only reads the manifest. Search is still a separate explicit
request. Add requires project trust, clean open manifests and an explicit click; it uses
the existing cancellable Cargo operation. Select Offline to prevent Cargo registry access.
Versions and features must be reviewed there: proposals do not infer feature flags from
code or promise that a crate API matches the pasted example.

## Conservative scope

The initial handwritten catalog contains 20 import roots: serde, serde_json, tokio,
anyhow, thiserror, regex, clap, reqwest, tracing, axum, uuid, chrono, rand, rayon, bytes,
url, tempfile, futures, parking_lot and crossbeam_channel (package crossbeam-channel).
Names are factual identifiers, not recommendations, rankings, copied descriptions or
assets. Other names are proposed only for a unique library in the loaded workspaces whose
Cargo package and import names agree after hyphen normalization. Unknown registry names,
custom library-name mappings and ambiguous attached-workspace libraries require manual
review. I08 remains partial because it does not discover arbitrary registry crates.

The owner is the unique exact Cargo target or innermost loaded member. Normal source uses
dependencies; test/example/bench targets and conventional directories use dev-dependencies;
exact custom-build targets use build-dependencies. Simple cfg(test) modules and test
functions use development scope. Complex cfg expressions, arbitrary build-script submodule
paths and macro-generated imports/modules are not resolved. Optional and target-specific
entries are conservatively considered present; no cfg/features evaluation is performed.

Rust syntax trees ignore comments, strings and macro token trees. Local modules, types,
generic parameters, import aliases and standard-library roots suppress guesses. Conventional
ancestor modules and target roots are read to detect local names, with open buffers taking
precedence over disk. Sibling module files also suppress proposals. Glob imports and an
existing dependency renamed to a different alias produce a manual-review notice rather
than a duplicate Add suggestion. This is deliberately conservative and may omit valid
suggestions. It is not a compiler name-resolution replacement or a RustRover parity claim.

## Bounds and cancellation

The analysis runs on a worker; editing, switching documents/projects or changing loaded
Cargo metadata cancels stale work. Results carry a generation, document version, metadata
epoch and open-source/manifest versions. Opening a project runs no project code. Disk reads
are bounded regular files (nonblocking/no-follow on Unix), sources are at most 1 MB, each
manifest at most 4 MB, and ancestor/target sources at most 64 files / 4 MB. There are at most
256 packages, 64 open source/manifest snapshots totaling 8 MB, 50,000 syntax nodes, 256
references, eight proposals and eight notices. The UI scrolls the bounded proposal banner.
Large or unreadable inputs are skipped. Analysis does not watch arbitrary unopened module
files itself; document/metadata changes schedule a refresh.

## Evidence

`cargo test -p rc-app cargo_dependencies::tests` includes a native egui paste in an actual
two-member Rust workspace. It verifies pasted serde and local-library proposals, no manifest
changes/search/build-script execution before explicit action, trust gating, offline cached
serde addition in normal scope, a local path dependency in dev scope, preservation of the
other member manifest, and successful explicit `cargo check --offline --tests`. A normal
text-edit event also produces a fresh proposal without contacting a registry.

`cargo test -p rc-app missing_crates::tests`, `cargo test -p rc-syntax crate_refs` and
`cargo test -p rc-cargo dependency_names` cover local modules, aliases, globs, cfg(test),
normal/dev/build scopes, optional dependencies, workspace inheritance, target tables and
cancellation.

## Public references

- [Cargo dependency declarations](https://doc.rust-lang.org/cargo/reference/specifying-dependencies.html): aliases, dependency sections, paths, target tables and optional dependencies.
- [Cargo workspaces](https://doc.rust-lang.org/cargo/reference/workspaces.html): inherited workspace dependencies.
- [Serde documentation](https://serde.rs/derive.html), [Tokio public API](https://docs.rs/tokio/latest/tokio/) and [Reqwest public API](https://docs.rs/reqwest/latest/reqwest/): examples of published crate roots. The catalog contains only handwritten package identifiers; no third-party assets or prose are distributed.
