# Rust type and trait hierarchy

Place the caret on a Rust trait or type and invoke **Type Hierarchy**
(Ctrl+Alt+Shift+H). The native Hierarchy window shows resolved implementations.
Traits with relationships also show **Supertraits** and **Subtraits** groups,
including transitive relationships. Expand/collapse branches with their arrow;
click a declaration row to navigate to its exact source location.

The graph is collected only on request for a trusted workspace with a running
rust-analyzer. A background worker parses workspace Rust sources, including
unsaved open buffers. It resolves each supertrait path using rust-analyzer's
`textDocument/definition`; graph edges use the definition URI and UTF-16 selection
position, rather than matching names. Consequently, imported aliases and traits
with identical names in different modules remain distinct. Direct `: Base<T>`,
higher-ranked bounds and `where Self: Base` contribute trait edges; generic
parameter bounds and method bounds do not. Implementations still come from
`textDocument/implementation`.

Collection and source reads stay off the UI thread. The window shows progress and
**Cancel**. Replacing a request, stopping analysis or closing the workspace cancels
the job and outstanding RPC requests. A changed, closed, renamed or newly opened
Rust buffer invalidates the entire pending snapshot. Closed sources are read
again before publication to reject changes during collection. FIFO, symlink and
other special-file inputs are refused; Unix reads additionally use nonblocking,
no-follow open flags and verify the opened descriptor is a regular file. The
collector never builds, runs a binary, or expands project macros itself.

The current supported scope is explicit trait declarations in workspace Rust
files. External supertrait definitions can be displayed as leaves; reverse
searches across downloaded dependencies, external transitive expansion,
macro-generated traits, trait aliases and an exhaustive inventory of conditional
compilation variants are unsupported. Relationships are included only when the
running analyzer resolves their definition. This is a source hierarchy, not a
proof that every candidate satisfies all trait bounds or an assertion of
RustRover parity.

The worker skips target, .git, .hg, node_modules and .idea directories and ignores
symlinks. Limits are 1,024 Rust files, 2 MiB per file, 32 MiB of workspace source,
2,048 declarations/rendered relation nodes, 4,096 bound-resolution requests,
100,000 filesystem entries, 500,000 AST visits per file, 16 relationship levels,
and 30 seconds. Reaching a limit fails visibly instead of presenting a silently
truncated graph. There is no filesystem transaction: further external edits after
a finished collection require requesting the hierarchy again.

Validation uses an actual Cargo project and the real rust-analyzer. It checks
same-named traits in two modules, an imported alias, transitive super/subtraits,
`where Self` bounds, implementation navigation, an unsaved subtrait and stale
snapshot cancellation. A headless native click navigates to the unsaved trait.
The existing trait/type/empty implementation-list test remains passing. Separate
unit tests cover AST bound selection, cancellation, and nonblocking refusal of
FIFO and symlink sources.

Public references:

- [LSP definition](https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#textDocument_definition)
- [LSP implementation](https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#textDocument_implementation)
- [Rust supertraits](https://doc.rust-lang.org/reference/items/traits.html#supertraits)
- [tree-sitter-rust grammar](https://github.com/tree-sitter/tree-sitter-rust)
