# Background editor saving

## Idle autosave

Settings › Editor › **Save files automatically after a pause of N s** (on by default, 5 s,
1–600 s) saves every edited, writable document once no key, text, paste or click arrived for
that delay. It uses the same queued writer as Save All, but never runs rustfmt, so text does
not move under the caret; format-on-save still applies to explicit saves. Typing again within
the delay restarts the countdown. It waits while multi-file edits or a save continuation are
running. Saving never builds or runs the project. Focus-loss autosave remains a separate option.

## Writer

Save, Save All and focus-loss autosave capture immutable document snapshots and queue
writes outside the egui thread. A single document writer owns a bounded queue of 256
paths; later pending snapshots for the same path replace earlier pending requests.
Cancelled writers remain counted until they acknowledge cancellation. Existing permissions,
UTF-8 BOM and CRLF/LF encoding are preserved. Rope encoding happens on the worker.
The shared atomic writer creates an exclusive sibling tempfile and checks cancellation
between 64 KiB writes and before replacement. Symlink, FIFO and other special destinations
are rejected. An OS filesystem call already in progress can delay cancellation; the UI
continues running. A replacement already committed before cancellation is not rolled back.

Each request records the editor identity, path, buffer version, disk epoch and expected
modification time. Completion updates only the matching document/epoch and records both
its former saved Rope and the persisted snapshot in Local History. A later edit is never
marked saved by completion of an older write; its Undo history remains intact. Reload,
watcher invalidation and document destruction cancel the shared writer token. Stale watcher
reads are cancelled before a save and events for an actively written path are ignored;
subsequent events are checked normally. A file changed externally before the write is
reported instead of overwritten; the explicit Keep Mine action permits replacement.
These checks do not provide a cross-process filesystem lock.

Rust format-on-save is ordered before the write, with one save-format job at a time.
Tool discovery and Rope conversion happen off the UI thread. Applied edits remain a
separate undo operation. A formatting failure preserves the text through a raw snapshot
save but aborts a waiting execution action and reports the error. A stale format result
never replaces newer edits.

Build, Run, Test, Debug, Coverage, Profile, Cargo window commands, inspections, Git commit,
external tools, scratch runs and build/test rerun controls use an App continuation barrier.
The barrier waits for saves, save formatting and queued session/settings writes. Edits made
while a write runs get a further snapshot before the continuation is dispatched. Save
errors, Cancel, workspace changes, revoked project trust and toolchain-provider changes
abort waiting execution. Dependency-manager mutation controls require saved documents and
an idle save/format queue. Rename application refuses to race a document writer.

Close Tab, Close Project, workspace switches and normal window Close keep buffers alive
until the barrier succeeds. Session snapshots use a separate bounded serial worker; it never overlaps a document
writer (including an editor open on the same settings/session JSON). Session writes queued before a switch complete before restoration of the next workspace. Cancel
keeps the original workspace and its buffers. The Saving documents window provides an
explicit Cancel saving button. Forced OS termination cannot guarantee pending writes.
`pending_workspace_target()` exposes a deferred switch for callers that need to schedule
new-file navigation after the actual transition.

E-12 remains partial while the separate batch/restoration and immediate-open callback
migration is being completed. Other feature-specific disk operations (for example rename,
closed-file refactoring and project settings) retain their own existing I/O paths; this
change does not claim that every file operation in the IDE is asynchronous.

## Verification

`cargo test -p rc-app disk_save:: -- --nocapture` passes nine tests. The tests cover a real
Cargo project whose build script and program observe the final saved buffer before spawn;
failed/cancelled saves, trust revocation and provider changes preventing spawn; close/switch
cancellation retaining buffers; actual rustfmt before Cargo with a separate Undo step;
BOM/CRLF, permission and Local History preservation; delayed writes with later edits;
serialization, stale reload/close invalidation; and symlink/FIFO/external-change rejection.
An egui-kittest interaction clicks the native Cancel saving button while a real document
writer is held, retaining the Rust buffer and preventing the queued Cargo command.

Test helpers use `save_all_and_wait()` to await the production worker and format pipeline.
The older synchronous Docs save helpers are compiled only in tests.
