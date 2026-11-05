# Parallel Stacks

While a debug session is paused, Debug › Parallel Stacks reads the top call
frames of each stopped thread. Identical captured snapshots (function name,
source path and line) share a group; adapter frame IDs are excluded from the
grouping key because each thread has its own IDs. Choose a thread in a group,
then click a frame to select its source location and variable/watch context.

The view uses the existing DAP session and native egui debugger. It does not
start a second debugging session. It groups identical captured stacks (prefixes when the frame limit is reached); it does
not draw a graph that merges partially shared callers, show async tasks, freeze
individual threads, or combine independent debug sessions.

At most 64 stopped threads and 64 frames per thread are retained, with at most
four stack requests in flight. Limits and per-thread failures are visible. Long
function names, adapter error text and source paths are bounded. Snapshots are
invalidated on resume, a new stop or a new debug session. They are loaded only
while the tab is visible. Refresh re-reads the thread list and stacks; Cancel
stops scheduling requests and discards pending replies. Each request has an
eight-second timeout. DAP cancel is sent only when the adapter advertises its
supportsCancelRequest capability; otherwise the local pending request is dropped.

If the adapter does not report allThreadsStopped, DAP only allows querying the
thread identified by its stopped event. The view then shows that one thread
and explains that other threads may still be running.

Protocol references:

- https://microsoft.github.io/debug-adapter-protocol/overview.html
- https://github.com/microsoft/vscode-debugadapter-node/blob/main/debugProtocol.json

Validation uses GDB 15.1 and a real Rust Cargo binary with two named worker
threads and the main thread. A barrier ensures the workers exist before the
breakpoint. The integration test reads all three stacks, checks the four-request
limit and cancellation, clicks native Refresh/Cancel controls, selects a worker
frame and verifies its source context, then resumes and checks that the old
snapshot disappears. Run outside a sandbox that restricts ptrace:

```sh
cargo test -p rc-app real_rust_threads_load_cancel_refresh_and_select -- --ignored --nocapture
cargo test -p rc-app parallel_stacks::tests
cargo test -p rc-dap parallel_stack_responses
```

Session tests also verify bounded untrusted adapter replies and ignored stale
generations. No UI input is sent to a real operating-system window.
