# Semantic completion in Rust documentation examples

Ctrl+Space (Code > Code Completion) now falls back to a semantic Rust projection
when the main language server returns no candidates inside a documentation code
fence. Ordinary derive/macro completion continues through the project's server.
The fallback supports Rust library examples in outer/inner line documentation
and block documentation, ordinary backtick/tilde fences, hidden setup lines,
escaped hash prefixes and standard Rustdoc edition/test flags.

The worker extracts the current example into a temporary function, builds its
crate graph from actual offline Cargo metadata and Rust standard-library source,
and asks an isolated rust-analyzer for completion. Dependency aliases and
re-exports retain their actual Cargo edges. Unsaved Rust buffers are overlaid
without changing project files. Suggestions are real type/member completion,
not words harvested from comments or a hand-maintained API list.

The project must be trusted and rust-src must be installed. Dependency graphs
require an existing offline-resolved Cargo.lock; a fresh dependency-free library
uses exact --no-deps metadata. The worker never creates or updates Cargo.lock.
It runs no build/check and loads no procedural-macro DLLs. Project-selected
Cargo/rustc tools and wrappers may execute, so the application's trust gate
applies before starting the worker. Rustup automatic installation is disabled.

The result's UTF-16 ranges map back through the documentation prefixes. Inserting
a multiline item repeats the comment/hidden-line prefix. Invalid ranges,
surrogate splits, out-of-fence edits and commands are rejected. Worker items do
not send opaque data to the different main server for lazy resolution. Additional
imports are unsupported: imports must already be present in the example.
Block-comment insertions cannot introduce a closing comment delimiter.

Editing, moving the caret, renaming/closing a document, changing project analysis
settings or changing any captured Rust buffer cancels the pending snapshot.
Escape dismisses the loading popup and cancels its worker. Cargo/rustc process
groups and the isolated analyzer are terminated on cancellation or timeout.
Language-server children are reaped asynchronously; on Unix their owned process
groups include helpers. LSP input now rejects oversized headers and bodies before
allocating an untrusted advertised body length.

Budgets are 1 MiB per overlaid document, 8 MiB of dirty overlays, 256 open
documents, 64 KiB/512 example lines, 8 MiB of Cargo metadata, 512 library crates,
8,192 dependency edges, 256 returned candidates and a 25-second total deadline.
The fallback is explicitly requested; it does not launch on ordinary typing or
project opening. Binary-only packages, external scratch/attached Cargo graphs,
autoimports, build-script-generated configuration, procedural-macro-generated
APIs and arbitrary Markdown layouts beyond ordinary fenced examples are outside
the verified subset. This is not a claim of parity with another IDE.

Validation:

- Native Ctrl+Space and Tab insert String::len after Unicode text in a block
  doctest with a hidden import. The completed example passes cargo test --doc.
- A Cargo alias dependency exposes a method that exists only in its unsaved
  editor buffer; completion inserts that method. Saving the accepted changes
  and dependency buffer then passes the real Cargo doctest.
- Escape and editing during a deliberately hanging analyzer request terminate
  the analyzer and never install a late result.
- Mapping tests cover line/block prefixes, hidden lines, UTF-16, malformed edits
  and foreign commands. Real process tests cover output limits, timeouts and
  cancellation. LSP tests reject oversized/malformed frames and kill a helper
  process with its server.
- The existing real-Rust derive/macro/import completion regression still passes.

Run cargo test -p rc-app doctest_completion -- --nocapture.
Run cargo test -p rc-app -p rc-lsp --lib for protocol/configuration checks.

References: [Rustdoc documentation tests](https://doc.rust-lang.org/rustdoc/documentation-tests.html)
and [rust-analyzer non-Cargo project graphs](https://rust-analyzer.github.io/book/non_cargo_based_projects.html).
