# Find in Files replacement jobs

Find in Files supports case/whole-word/regex queries, file masks and scopes. Replace
All now runs file reads, regex compilation/expansion, edit computation and closed-file
writes in a cancellable background job. Search controls stay disabled while it runs;
Cancel, progress and individual file errors remain visible. A new search or changed
exclusions cancel the old replacement. Closing the project drops and cancels its job.

Open documents use a captured rope snapshot of their current unsaved text. Returned
edits are applied as one undoable edit only when the path, document ID, version and
read-only state still permit it. Closed/reopened or changed buffers are skipped.
Open buffers are not saved automatically. Closed files are written atomically in
the same directory, preserving BOM, line endings and permissions. Read-only files,
symlinks and special files such as FIFOs are refused.

Closed-file replacements require the metadata captured by the search to match, then
re-read and compare the original bytes immediately before persisting. Detected
external changes are preserved and reported. There remains a narrow check/rename
race with an external writer; avoid concurrent writes to a file being replaced.
Cancellation retains replacements already completed. The operation is not a
multi-file transaction; open-document edits can be undone separately.

Replace All is unavailable for a truncated or cancelled search. Narrow the query
before replacing. The input set is the complete displayed search result set: search
reads disk contents, and listed open files use their latest buffer. Unsaved matches
in an open file absent from disk results are not included.

Limits are 5000 files, 8 MB per file/output, 100000 edits overall, 10000 edits per
open buffer, 64 MB of captured open text and 64 MB of changed output per job. Queries,
replacement text and file masks are capped at 64 KB. The first 100 file errors are
kept; path/message display is bounded. Regex expansion bounds repeated capture-group
references before copying their text, and cancellation is checked during capture
expansion, between matches and between file read/write chunks. Byte regexes may not
split UTF-8 characters. The same matching and replacement syntax is documented by:

- https://docs.rs/regex/latest/regex/bytes/struct.Captures.html
- https://docs.rs/regex-automata/latest/regex_automata/util/interpolate/index.html

The native headless test creates a real Cargo workspace in a path with spaces,
opens an unsaved Rust main.rs and leaves its module closed, replaces regex capture
groups through Replace All, checks undo/redo and verifies the saved result with
cargo check. A build.rs marker proves that search/replacement did not execute the
project. Separate tests preserve changed documents/disk files, cancellation and
reopened buffers. Core tests cover BOM/CRLF, permissions, read-only/special files,
UTF-8 boundaries and excessive capture expansion.

```sh
cargo test -p rc-app replace_files::tests
cargo test -p rc-search
cargo test -p rc-app scopes_narrow_find_in_files_and_color_files
```
