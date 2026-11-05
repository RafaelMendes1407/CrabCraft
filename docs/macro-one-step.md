# One-step declarative macro expansion

For a trusted Rust project with a running rust-analyzer, place the caret on a
`macro_rules!` invocation and choose **Code → Expand Declarative Macro One Step**,
or select the same action in Find Action. The native progress window offers
Cancel. The result opens in a generated, read-only editor tab. The existing
**Expand Macro Recursively** action still asks rust-analyzer for recursive output.

One-step expansion resolves the invocation's actual declaration through
`textDocument/definition`, including imported and qualified names. The IDE reads
that definition from the current open buffer when available, so unsaved macro
changes are included. It runs the open-source macro-by-example matcher once:
repetitions, metavariable substitution, matcher arms and expression fragments use
the matcher's Rust grammar, rather than textual replacement. Nested calls,
built-ins such as `include!`, self-recursion and calls through a captured macro
name remain calls in the output. They are not invoked. Opaque expression
fragments retain parentheses needed to preserve precedence.

Matching takes place in a separate private IDE helper, before any GUI, settings
or instance initialization. It accepts bounded JSON containing only rule and
argument tokens and edition numbers, not filesystem paths or executable
commands. It does not load build scripts, procedural macros, linked Cargo
projects or dependencies. The live project analyzer is used only for declaration
lookup and remains subject to the normal project trust barrier. No project
binary or compiler is launched by the feature.

The helper enforces a 2 GiB address-space budget and 10-second CPU budget through
Unix resource limits. An overall 20-second deadline covers lookup, reading and
matching; Cancel kills and reaps the helper process tree and cancels the pending
analyzer request. A failed helper produces a typed error; its partial output is
never displayed as an expansion. Platforms without this enforced memory budget
are visibly unsupported in the action label and refused before a helper starts.
Current real-project validation is on Linux; no cross-platform or RustRover
parity is claimed.

Limits are 16 KiB each for rule and invocation tokens, 8,192 input token-tree
entries, 65,536 output entries and 1 MiB of rendered output. Inputs and replies
also have transport limits. Invocation selection is bounded to 500,000 token visits and 256 ancestors.
Source files are at most 2 MiB, with at most 64 open
Rust source snapshots and 16 MiB of buffer snapshots. Reads reject symlinks,
FIFOs and nonregular files and check the opened descriptor; on Unix they use
nonblocking/no-follow flags. Up to 64 disk sources/configurations and 16 MiB of
observed text are tracked and re-read before publication. Source buffer changes,
closing/renaming a buffer or a changed disk definition/manifest invalidate the
request. Further external edits after completion require a new request.

Cargo edition 2015, 2018, 2021 or 2024 is read from the owning manifest, including
normal `edition.workspace = true` inheritance. Missing edition uses Cargo's 2015
default. Unsupported or unresolved ownership/inheritance fails visibly. This is
token-level expansion: it does not resolve every hygienic path into a fully
qualified name. `$crate` can remain in the display. Procedural/built-in outer
macros, newer `macro` syntax, definitions without a resolvable owning Cargo
manifest and new syntax beyond the pinned matcher's grammar are unsupported.

Generated expansion tabs, including recursive expansions, are now memory-only.
They neither read nor write expansion cache files and do not perform a blocking
parse. Parsing is queued to the existing syntax pool. Repeating an expansion
updates its generated tab; it never overwrites a normal document with a colliding
path. Save is a no-op for generated tabs. They stay out of recent files and the
persisted session, with normal source-file layout indexes remapped correctly.

## Evidence and provenance

The direct matcher tests cover nested/built-in/self-recursive calls, a captured
macro identifier, repetitions, expression precedence and invalid matchers. A
real spawned helper generates Rust that rustc compiles and executes. The native
Rust-project test starts untrusted, resolves an unsaved definition through the
real rust-analyzer, retains a nested call in the generated read-only tab,
compiles and executes the expanded result and rejects a stale source version.
The existing recursive-expansion test remains passing. A headless UI test checks
memory-only generated tabs, Save, MRU and session/layout indexes. Separate tests
cancel and time out a deliberately stalled process and verify it is reaped.

The matcher, token tree, span and syntax bridge crates are pinned to
`ra_ap_mbe`, `ra_ap_tt`, `ra_ap_span` and `ra_ap_syntax-bridge` **0.0.273**.
These rust-analyzer components are **MIT OR Apache-2.0**, distributed as Cargo
dependencies, not copied from proprietary software. Their original package
notices remain intact. The repository's Apache-2.0 license text is included in
`LICENSE-APACHE`. No model weights, additional executables or assets are bundled
for this feature; the private helper is the existing desktop executable.

Public sources:

- [rust-analyzer LSP extensions](https://rust-analyzer.github.io/book/contributing/lsp-extensions.html#expand-macro)
- [Macro-by-example crate](https://crates.io/crates/ra_ap_mbe/0.0.273)
- [rust-analyzer source and licensing](https://github.com/rust-lang/rust-analyzer)
- [Rust macro-by-example reference](https://doc.rust-lang.org/reference/macros-by-example.html)
- [Rust edition compatibility for macro fragments](https://doc.rust-lang.org/edition-guide/rust-2024/macro-fragment-specifiers.html)
