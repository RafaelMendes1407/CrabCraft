# Borrow-checker source ranges

The native Build tool window offers **Visualize E0502 source ranges…** (and the analogous
action for other supported codes). Select a diagnostic to see the compiler's primary and
related locations, original source snippets with colored ranges, attached notes, and an
**Open source range** action. Labels and snippets are the rustc output, not IDE-generated
explanations. They are not complete inferred lifetimes, a flow simulation or an NLL dump.

The supported initial codes are E0499, E0500, E0501, E0502, E0503, E0505, E0506, E0507,
E0515, E0597, E0621, E0623 and E0716. The view preserves all reported primary flags: it does
not assume there is exactly one primary range. Child diagnostic spans and notes remain
associated with their original compiler diagnostic. Macro expansion chains are not expanded
into invented source locations; missing spans produce no visualization action.

## Navigation and stale data

Opening a range starts a cancellable worker. It reads a regular UTF-8 `.rs` file up to 1 MB
inside the diagnostic's workspace, checks source lines against the compiler snippets, and
checks that byte offsets agree with Unicode line/column positions. BOM and CRLF are mapped
to the editor's normalized character coordinates. Modified emitted lines, invalid positions,
truncated multiline ranges, unreadable or external sources produce a visible error. This
validates reported regions; it does not promise unchanged file content outside those regions.

An already open document must match the checked disk content. Its ID, buffer version and
disk epoch must also match when the worker returns; closing/reopening or editing it rejects
the result. Closed source is installed from the worker's snapshot without UI disk I/O or a
blocking parse. A selected range uses the existing editor selection/highlight renderer, and
its highlight is removed after edits. Existing user text is never overwritten.

Cancel, closing the view, choosing another diagnostic or switching projects invalidates the
request generation and cancels pending work. A tiny filesystem change/read-to-display race
is unavoidable without locking the user's files; the displayed source snapshot remains the
validated compiler region. As with all other Build diagnostics, the list is a captured build
result: rebuild explicitly to obtain new compiler errors. Opening/reviewing ranges does not
run Cargo or project code; generating diagnostics requires an explicit trusted build.

## Bounds

Each borrow diagnostic retains at most 32 spans, 16 child notes, four levels/32 nodes of child
context and 64 KB of span-path/label/source/note data. At most 128 raw spans are inspected;
labels/notes are at most 2 KB, paths 8 KB, source snippets eight lines per span and 4 KB per
line. Truncated detail is identified in the view; invalid/unverifiable ranges cannot navigate.
The diagnostic title is limited to 2,048 characters in the detail view. The range pane scrolls
at 240 px; long file contents are never read or rendered there.

## Evidence

`cargo test -p rc-app real_rust_borrow_errors_show -- --nocapture` creates a real Rust Cargo
project with E0502, E0499 and E0597. It checks actual compiler primary/secondary labels,
validates every reported source region with BOM, CRLF and preceding Unicode, then clicks
native Visualize and Open source actions. It verifies exact editor selection and highlighting,
refusal after an unsaved edit, changed compiler source on disk and cancellation. The build
script marker is absent before the explicit test build.

`cargo test -p rc-cargo compiler_ranges` covers preservation of primary and related child
spans/notes and rejection of malformed positions. Existing Cargo message and .NET diagnostic
parser tests verify that non-borrow compiler diagnostics retain their previous behavior.

## Public references

- [rustc JSON output](https://doc.rust-lang.org/rustc/json.html): primary/secondary spans, byte offsets, Unicode character positions, original source snippets and child diagnostics.
- [E0502](https://doc.rust-lang.org/error_codes/E0502.html), [E0499](https://doc.rust-lang.org/error_codes/E0499.html) and [E0597](https://doc.rust-lang.org/error_codes/E0597.html): official error semantics. The test project is handwritten; no reference prose or assets are distributed.
