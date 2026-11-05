# Conservative public Rust item moves

Refactor › Move public Rust item… and Find Action open a native preview window. A trusted
Cargo project and the existing rust-analyzer session are required. Opening the window does
not run Cargo, save a file, discover/install a server, or move anything. Open the saved crate
root and both sibling module files first, select an item with one caret, and choose the
already-open destination. The operation moves the selected definition and leaves an explicit
re-export at its original path.

The verified scope is deliberately conservative:

- Plain `pub`/`pub(crate)` free functions whose types and expressions can be proved independent
  of module context: primitive parameters/results, local bindings, arithmetic/control flow,
  with no calls, paths, attributes, generics or captured external declarations.
- `pub`/`pub(crate)` structs with primitive fields. Every named/tuple field must explicitly be
  `pub` or `pub(crate)`; unit structs are accepted. Existing impl blocks stay in the original
  module and resolve the re-exported type. Private fields would change access, so are refused.
- Public enums with primitive payloads/simple discriminants, literal/local primitive constants,
  and aliases to primitive types.

Examples of the two changes shown together:

```rust
// src/source.rs before
pub fn score(value: i32) -> i32 { value * 2 + 1 }
// after
pub use super::destination::score;

// src/destination.rs after
pub fn score(value: i32) -> i32 { value * 2 + 1 }
```

The visibility on the definition and source re-export is preserved. Existing callers, aliases
and function values keep their old spelling through the re-export; they are not rewritten
based on an incomplete references search. The canonical definition/module path does change,
as do path-sensitive type names/debug metadata; this is not an ABI/type-name stability promise.
New destination-name collisions or existing destination references with that name are refused.
No private item/field is silently made more visible.

## Identity and namespace proof

Planning runs on a cancellable worker over frozen open-buffer ropes. Tree-sitter identifies
one top-level item. rust-analyzer must return one unambiguous definition at that item's exact
name. Every identifier inside the item must resolve within its own frozen range, and matching
names across the captured crate sources must resolve unambiguously. Nonlocal names and unknown
server answers fail without edits. No guessed import insertion/qualification is performed.

The supported graph is two conventional external `src/name.rs` sibling modules declared in
one saved `src/main.rs` or `src/lib.rs`, in a single implicit Cargo target. Raw/non-ASCII item
or module names, methods, module items, nested/custom `#[path]` graphs, custom target tables,
build scripts, and test/example/bench/bin directories are unsupported. Local binding names
and source contents may be Unicode; the native test uses an emoji before a `café` parameter.

To avoid undiscovered generated namespaces or new glob ambiguities, the bounded source scan
refuses macros, attributes, documentation comments and glob imports throughout the captured
crate sources, including closed files. Primitive-name shadowing imports/declarations and
malformed syntax are also refused. These restrictions are explicit in the window; the feature
does not claim to relocate arbitrary Rust items or match another IDE's refactoring coverage.
General dependency qualification, private visibility repair and generated/nested module graphs
remain outside the shipped scope.

## Frozen preview, cancellation and applying

Preview item move changes no buffers. The preview shows source and destination before/after,
limited to 16,384 characters per pane. Apply move to buffers first rechecks captured disk
bytes, directory membership, manifests and conventional target layout on a worker. Before
applying either edit, the UI checks all captured buffer versions/paths/read-only states,
metadata epoch, project trust and language-server connection identity. A changed/closed buffer,
server restart, metadata change, outside disk edit or new source file invalidates the preview.
Changing the destination cancels the pending job and drops its old preview.

Application is confined to the two already-open editor buffers. Both are checked before either
is changed; each replacement is undoable in its own editor. No autosave, filesystem rename,
resource operation or Cargo execution is performed. Save or undo each file explicitly. Closing
the window/project or Cancel cancels its worker and pending requests. Disk preflight is not a
filesystem lock; an external change after verification is still possible, but this feature
does not overwrite disk files automatically.

Bounds: 1 MB per file, 8 MB total source/open-buffer text, 128 source files/open buffers,
2,048 directory entries, 64 KB moved item, 128 local identifiers and 256 total identity queries.
The planning budget is 20 seconds and disk verification 15 seconds. Symlinks/special files and
unbounded/ambiguous results are refused with typed errors.

## Evidence

`cargo test -p rc-app move_items --offline -- --nocapture` passes three tests:

- AST/visibility/context guards accept public primitive shapes and refuse private fields/items,
  dependent calls/paths, generics, macros, attributes, doc comments and glob imports.
- Disk verification rejects external changes/cancellation and preserves the external contents.
- The native fixture creates a real Cargo application and starts real rust-analyzer only after
  explicit test authorization. It clicks preview/cancel, proves stale-buffer and real captured
  disk refusal, then moves a Unicode-parameter function, a `pub(crate)` function, a public
  struct, a payload enum, constant and type alias through preview/Apply. Existing impls and
  source aliases remain intact; disk remains unchanged until an explicit save. Undo/redo is
  checked independently in both buffers. A function that depends on an outer constant is
  refused using its real RA identity. `cargo run --offline` then executes the changed project,
  verifies old callers/aliases/struct methods/enum payloads/constant/alias values, and navigation
  from the old caller lands on the new definition.

## Public references

- [Rust use declarations and re-exports](https://doc.rust-lang.org/reference/items/use-declarations.html).
- [Rust visibility and privacy](https://doc.rust-lang.org/reference/visibility-and-privacy.html).
- [Rust inherent implementations and re-exported type paths](https://doc.rust-lang.org/reference/items/implementations.html).
- [LSP definition requests](https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#textDocument_definition).

No third-party assets or documentation prose are copied. Existing copyright/license notices
are unchanged. No RustRover parity comparison is claimed.
