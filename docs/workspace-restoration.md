# Background workspace preparation and restoration

Opening a workspace leaves the current workspace and buffers intact while one cancellable
worker prepares the replacement. Newer requests replace pending navigation. Cancelled workers
remain counted until acknowledgement; no cancellation creates an unbounded stream of workers.
The Opening workspace window provides cancellation. Saved-session and explicit CLI/drop file
requests are installed only after the actual workspace transition, retaining order, line/column
positions and preview flags. Requests for the old workspace can cancel a pending transition.

The worker canonicalizes/classifies the root, reads the session, static solution/project/run
configuration and restored documents, and registers a watcher with its own private event inbox.
No watcher event is delivered to the previous workspace during preparation. Static Runner
configuration is prepared on the worker and does not execute project tools. Cargo metadata,
analysis processes and VCS discovery start only after installation and a live trust check.
The copyright-author Git lookup is deferred until trust. Request settings are used only for
worker tool-path discovery; installing the result does not replace current settings or trust.

Session/configuration reads require regular UTF-8 files, use nonblocking/no-follow descriptor
reads with a 4 MiB bound, check cancellation between 64 KiB chunks and validate descriptor/path
metadata after reading. A missing session starts a default session; an invalid, special,
changed or excessive session aborts opening and retains existing buffers. Project/solution
loaders use checked bounded readers; the solution parser additionally limits nesting/items.
Folder discovery limits root enumeration to 8,192 entries and project traversal to 32,768
entries/depth four. A root Cargo.toml takes precedence without enumerating the directory.

At most 256 session files/512 MiB of text are prepared, using the existing checked document
reader (regular UTF-8, 128 MiB per-file open limit, read-only above 8 MiB). Rope construction,
encoding detection, code style, initial parse, folded-line preparation, caret and scroll
restoration run off the UI. Layout is bounded to 512 nodes/64 floating windows; invalid sizes
and scroll values cannot become unbounded geometry. Missing/unreadable individual session
files are reported and skipped. Original session slots map to editor IDs, so a skipped file
cannot shift active-tab or split-layout references.

The UI reserves monotonically increasing document IDs and installs prepared documents into
the existing Docs/parser pool. A fingerprint of document ID/path/version/disk epoch/dirty and
conflict state prevents a result from replacing newer edits or invalidated disk snapshots.
A conservative stale result keeps the original workspace and asks for another open request.
Settings/trust are read live again before starting services. Same-workspace reload re-prepares
the session after saving if bookmarks/run options change while preparation is in flight.

The installation barrier waits for document writers, session/settings writers, rustfmt jobs,
pending save formatting and guarded file edits. It never continues after close_workspace has
deferred its teardown. Cancel saving also clears the pending workspace and queued file batch.
Pending file edits complete before fingerprint validation; an edit changing the previous
buffer makes the workspace result stale and preserves that dirty buffer.

Test-only `open_workspace_and_wait`, `open_file_and_wait` and `save_session_and_wait` await the
actual worker/queue flow. They do not bypass production preparation or saving. The five
workspace tests plus the late-edit barrier regression cover real Cargo/Rust restoration,
BOM/CRLF/folding/pinned/caret/layout/breakpoints, skipped original slots, NoCode before trust,
explicit Cargo run after trust, cancelled/stale/latest navigation and deferred positions,
FIFO/symlink/oversized sessions, live trust/settings changes, and run-option edits during
reload. CLI/native batches also cover multiple files, outside-root files and source positions.

The separate Explorer worker passed seven tests and four accessibility regressions. A final
private Linux/X11 Orca run observed five editor snapshots and four spoken utterances with
source geometry and real Explorer parents intact; no project code ran. See project-tree-io.md.
Other feature-specific filesystem operations retain their own migration scope; this document
does not claim that every operation in the IDE is free of synchronous filesystem work.
