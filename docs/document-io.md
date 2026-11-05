# Document I/O migration

Ordinary editor file opening, including Project preview tabs, Search Everywhere,
line/column navigation (including language-server targets and diagnostics), and Back/Forward
now prepare their document on a bounded worker. Path canonicalization, checked
UTF-8 reads, BOM/CRLF decoding, code-style and LightEdit project discovery, Rope construction,
and initial small-file syntax analysis happen outside the UI thread. This does not execute
Cargo, build scripts or proc macros. Installation into the UI does no disk I/O.

The current navigation request has an Opening file window with Cancel. A newer request,
existing-tab navigation, close-tab action or project switch cancels earlier requests.
At most two readers exist, including cancelled readers winding down, with only the latest
pending request retained. Cancellation is checked between 64 KiB reads and preparation
stages. OS filesystem calls already in progress can delay worker cancellation; they do not
block the UI. Each request captures the active document identity; changing tabs before its
result arrives prevents the old result from taking focus. Existing documents, including a
canonical path reached through a symlink alias, are activated without replacing their edits.

The worker verifies regular-file descriptors and before/after file metadata, rejects binary,
invalid UTF-8 and files above 128 MiB, and opens files above 8 MiB read-only without a syntax
request. Metadata-as-source and dependency roots stay read-only; ordinary dependency sources
still get syntax analysis. BOM/line endings, indentation preferences, bookmarks, breakpoints,
preview replacement rules and navigation positions are retained. Only an unmodified preview
can be replaced. Later watcher updates continue through their own cancellable reader.

Two tests pass with the real native egui model and a Cargo project: UTF-8/BOM/CRLF, caret and
style, preserved unsaved buffers, cancelled/stale navigation, canonical alias deduplication,
preview replacement, closed-file Back/Forward history without clearing its forward stack,
actual subsequent Cargo execution, FIFO rejection, explicit cancellation,
128 MiB rejection, and large/read-only syntax policy. Six search, three navigation, six Cargo-feature and five native UI regressions also pass.

File/drop/dialog batches now prepare files serially on the same cancellable reader, retaining
input order and existing unsaved buffers. A batch is limited to 256 files and 512 MiB;
individual file limits still apply. Changing the active tab discards the remaining batch.
Compiler suggestions and Cargo feature fixes prepare checked source snapshots and edits on
workers, including closed files. Installation rejects changed documents and workspace context;
BOM/CRLF compiler byte offsets map to normalized editor character offsets. Save and tool
continuations wait for this preparation, and cancellation or rejection aborts waiting execution.
Native tests cover ordered multi-file opening, cancellation, canonical aliases, closed-file
Unicode/BOM/CRLF edits, undo, stale edits and a real Cargo build observing the edited feature.
Existing real compiler and rust-analyzer feature-fix tests also pass.

Saving and dependent execution now use the barriers described in background-saving.md.
Session restoration and workspace preparation remain under migration; E-12 stays partial.
The generic atomic writer already uses unique sibling temporaries, preserves existing file
permissions and rejects special destinations; it does not promise power-loss durability or
filesystem-wide compare-and-swap semantics.

Test fixtures explicitly await the same worker-backed opening API through a cfg(test) helper;
they do not replace its reader, preparation or stale-result checks. New-file template carets,
recent-location offsets and breakpoint navigation are applied after their document arrives.

Static inherited .NET solution/project/launch-settings reads use regular descriptors, nonblocking Unix opens and a 4 MiB limit. Solution parsing accepts already-read text and bounds item/folder/path depth before recursive tree construction. The 39 rc-dotnet unit tests include FIFO-without-writer, oversize and hostile nested solution configuration; no project tool is run to inspect these files.
