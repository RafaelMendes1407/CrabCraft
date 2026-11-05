# Edit history

Git → **Edit History** loads a reviewed plan for the latest 1–20 commits on the current
local branch. Loading a plan is read-only. The working tree and index must be clean,
including untracked files, editor buffers and pending formatting. Root commits, merges,
detached HEAD and commits reachable from known remote-tracking refs are refused. Refresh
remote-tracking refs yourself before reviewing published history; unfetched remote state
cannot be inferred offline. The project must already be explicitly trusted.

Rows run oldest first. Keep a commit with **Pick**, change its message with **Reword**,
omit it with **Drop commit**, or discard its message and fold its changes into the previous
surviving commit with **Fixup into previous**. Move rows earlier/later to reorder them.
A fixup cannot be the first surviving row, and dropping every row is refused. Reword
messages are limited to 4 KiB. Changing the count discards the old plan.

Review the plan, then check **I reviewed this history plan and accept rewritten commit IDs**
and choose **Rewrite History**. Application rechecks the branch, original hashes, clean
state and remote-tracking reachability. A stale plan cannot rewrite newly changed history.
Before rebasing it creates `refs/crabcraft/history/edit-…` pointing at the original HEAD.
The recovery reference remains visible and reachable from Git; create a branch at it to
recover the original history. Other branches are not automatically updated.

The operation uses Git interactive rebase, with a generated private sequence editor and
fixed helper commands for message changes. Subject/message text is never shell code.
The helper runs before GUI startup, checks application trust and the active rebase's private
plan path, and only writes the generated todo or amends the selected planned message.
Private plan files remain under the Git directory so **Continue** still works after a
conflict, cancellation or IDE restart. No helper runs when merely opening a project.

Reorder/drop/fixup can cause conflicts. Use the existing Git operation controls to resolve,
continue, skip or abort; inspect Git output and the recovery ref before retrying after
cancellation. Git commands kill their editor/hook process group on cancellation or future
drop. Explicitly trusted project hooks can run during history editing. Rewritten commits
get new IDs and lose original signatures; the UI warns about this before confirmation.
Commit splitting, merge/root editing, published-history overrides, automatic force push
and automatic recovery reset are unsupported.

Evidence on a real Linux Rust project: an actual native helper reordered commits, changed
a message containing quotes and shell-looking text as literal data, fixed up a commit,
dropped another, retained the original HEAD in a recovery ref, left a clean working tree
and passed `cargo test --offline`. Guard tests refuse dirty/untracked, known-published,
root and invalid plans; a stale plan cannot be reapplied. Native controls test loading,
confirmation gating and explicit rewrite. Git library regressions cover read-side output
redirection rejection plus helper descendants killed on cancellation and future drop.
Cross-platform UI behavior and screenshots have not been verified.

Reference: [Git interactive rebase documentation](https://git-scm.com/docs/git-rebase).
This implements the documented linear operations without claiming RustRover parity.
