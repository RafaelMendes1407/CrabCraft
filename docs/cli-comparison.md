# Native diff and merge from the command line

```sh
crabcraft diff old.rs new.rs
crabcraft merge left.rs right.rs base.rs merged.rs
```

Each command opens its own native egui window and waits for it to close. Paths
are relative to the calling directory. Use `--` before a filename beginning
with `-`; filenames are literal, so a `:line` suffix is not interpreted here.

Diff shows aligned, read-only panes with syntax colors, changed-line backgrounds
and synchronized scrolling. It returns 0 after successfully loading the files,
or 1 if loading fails or the window closes before loading completes.

Merge opens the same three-pane editor used for Git conflicts: left, editable
result and right. The base is the common ancestor; changes made by only one
side are applied automatically. Accept Left/Right/Both, Keep Base, next/previous
change and whole-file actions remain available, with undo/redo and synchronized
scrolling. Save Result writes only the explicit output path. Unresolved changes
require the separate Save Anyway action. Closing without saving returns 1 and
leaves the output untouched; successful saving returns 0. Invalid arguments
return 2 before a window opens.

The result is written to a temporary file in the output directory and persisted
atomically. Existing output permissions are retained. A changed output is refused;
new outputs use no-clobber persistence. There remains a narrow check/rename race
for an existing output, so avoid concurrent writes while saving a merge. The
output may be one of the input paths if that is explicitly requested. Symlinks
are accepted as input but refused as output. Parent directories must exist.

Files must be regular UTF-8 text without NUL bytes, at most 4 MB and 100000 line
breaks per file. Loading and model construction happen off the UI thread. Close
cancels pending file reads and discards late results. Saving also runs in the
background; window close waits for an accepted save to finish. The window does
not restore sessions, open a Cargo workspace, invoke Git, launch a language server
or execute project code.

The headless tests use Rust files inside a real Cargo project with a build.rs
that would fail if executed. They exercise the native merge controls, save the
selected side and explicitly compile the resulting file with rustc. They also
check read-only diff panes, rejected binary/oversized input, cancellation and
protection against an output changed after opening.

```sh
cargo test -p rc-app compare_cli::tests
cargo test -p rc-app the_merge_tool_resolves_a_conflicted_file
```
