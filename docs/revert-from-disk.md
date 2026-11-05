# Revert from Disk

File > Revert from Disk…, Find Action, and the editor tab context menu read the current disk
version explicitly. The conflict banner's Load from Disk button uses the same workflow.
Dirty documents first show a native confirmation naming the file. Cancel leaves the buffer
and disk untouched; Discard and Reload starts the read. Generated documents are unsupported.

A worker reads a regular UTF-8 file up to 8 MiB in 64 KiB chunks, checking cancellation between
chunks. Unix opens use O_NONBLOCK, and the opened descriptor is checked again, so a file
replaced with a FIFO cannot stall startup. Binary and invalid UTF-8 content are refused.
Length, modification time and, on Unix, device/inode are checked on the open descriptor and
the current path after reading. A changing disk version is rejected rather than truncated
or silently decoded with replacement characters. A progress banner allows cancelling the
pending read; closing the project drops its job.

The result applies only if the document ID, path, text version and disk-event epoch still
match the request. Edits, undo, renames, close/reopen, saves and watcher events invalidate
pending results. Confirmation also refers to that exact document version; later edits need
a new request. The loaded snapshot is the version observed during the read. Changes after
the read are still handled by the existing file watcher.

Revert adopts the snapshot's BOM, newline style and modification time, clears the disk
conflict and marks that text saved. The caret's line/column follows the existing reload
policy. The old text is retained in Local History and in one undoable replacement. Undo
restores it as unsaved text without modifying disk; encoding metadata remains the loaded
disk version's. Redo returns to the saved disk text.

The older document reader now also requires regular files, verifies opened descriptors and
file identity, rejects binary/invalid UTF-8 content, and bounds actual reads at 128 MiB even
when a file grows. Files over 8 MiB retain the existing read-only/no-syntax opening policy.
The error type is available to new callers; legacy document/refactoring callers retain a
string-error adapter.

This does **not** complete E-12's asynchronous document pipeline. Opening, automatic watcher
reload and saving still perform synchronous I/O. Saving must continue to finish before
build/run/test/debug and before dependent filesystem actions. Deferring those actions until
queued writes finish requires a broader ordering change; this implementation preserves the
current flush-before-run ordering. Large-file responsiveness remains unverified.

Tests use real Cargo workspaces and cover external disk changes, dirty confirmation/cancel,
BOM/CRLF, undo/redo, a successful Cargo check, stale edits/save/rename/reopen/watcher results,
strict UTF-8 and size limits, cancellation, and no-writer FIFO rejection in both readers.

## External updates and cache invalidation

Filesystem notifications and Invalidate Caches now enqueue external reads instead of
reading source files on the UI thread. Changes coalesce by path, with at most two live
workers and 256 queued documents. Cancelled workers remain counted until they acknowledge
cancellation, so repeated events cannot create an unbounded number of threads. Closing a
project/document, saving, renaming and later file events invalidate earlier tickets; buffer
edits cause a fresh comparison so unsaved text is retained and a real disk conflict appears.

The shared reader checks regular-file identity before/after a bounded read, rejects binary
or invalid UTF-8 input, and checks cancellation between chunks. External reload is limited
to 8 MiB; unsupported/failed reads preserve the buffer, mark a conflict and report an error.
BOM/CRLF metadata survives reload and subsequent save. Normalization and comparison run in
the worker; source parsing is queued after applying an unchanged-version result. No project
tools run merely because its files changed.

`cargo test -p rc-app disk_watch::tests` verifies event coalescing, new local edits,
close/reopen identity, FIFO-without-writer rejection and actual native filesystem events in
a Cargo project. The native test checks encoding/conflicts and a later explicit Cargo run.
`cargo test -p rc-app invalidate_caches_rebuilds` checks asynchronous cache invalidation.

E-12 remains partial: ordinary file opening and save I/O, plus ordering saves before
build/run/test/debug, still need their asynchronous path. Synchronous `Docs` entry points
remain available for backend fixtures and existing callers pending that migration.
