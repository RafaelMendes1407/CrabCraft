# IDE-local shelves

The Commit window's **Shelve Checked…** opens Git → **Shelves** with the checked files.
Enter a shelf name and choose **Shelve Checked Files**. By default it saves a durable local
backup, then removes the selected files' **unstaged** changes from the working tree. Enable
**Keep changes in the working tree** to create only a backup. Saved project files are
required; wait for formatting to finish first. Edits made while the operation runs stay in
the editor and are handled by the normal disk-conflict protection.

These shelves are not renamed Git stashes. They are private application records under
`CrabCraft/shelves/<repository hash>/<shelf id>/`, containing a binary patch and a small
JSON manifest. Git stash refs and commit history are untouched. The patch compares the
working tree with the index, so existing staged changes remain staged, with the index
unchanged. Shelving staged-only changes reports that there is no unstaged delta to save.

Select a shelf to preview its patch (first 64 KiB), **Restore Shelf**, or **Delete Shelf…**.
Restore checks applicability before applying, retains the backup, and never stages files.
A conflicting file is not overwritten. Delete needs confirmation and only removes that
local backup; it does not change working files. Shelves survive closing and reopening the
IDE. Git projects must be explicitly trusted before using these operations.

The current implementation supports selected tracked regular files, including binary
files, executable modes, literal filenames and deletions. Untracked files, symlinks,
submodules, conflicted index entries and unsafe paths are visibly unsupported. Use Git
stash for untracked files. Other working files are left alone; staged content is not part
of a shelf. There is no automatic three-way merge or recovery UI for conflicts.

Work runs in the background with cancellation. The backup is written and synced before
removing changes; failed or cancelled cleanup keeps the saved shelf available. A process
cancelled during application can leave some working files changed; the durable backup is
retained, and the index is still never written. Inspect the files before retrying.
Checksum, regular-file checks, path/mode checks and bounded reads reject corrupt or
unsafe records. Limits: 32 MiB patch, 1,000 selected paths, 500 shelves per repository,
120-character names and a bounded directory scan. Corrupt records are not executed.

Linux evidence uses a real Rust crate and Git repository: mixed staged/unstaged changes,
binary content, literal/deleted paths, byte-identical index, unrelated/untracked file
preservation, persistent backup, conflict refusal, copy mode, cancelled operations and
corrupt/unsafe input rejection. The restored crate passes `cargo test --offline`. A native
headless test exercises checked-file shelving, patch preview, restore and confirmed delete.
Cross-platform behavior and screenshots have not been verified.
