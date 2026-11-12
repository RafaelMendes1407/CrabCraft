# Project tree reads

The native Project panel reads directories in a shared two-worker pool. Drawing, expanding a row and showing a context menu use cached data. A cache miss queues work and returns immediately. Loading and read errors are visible. Cancel stops pending reads cooperatively. Refresh clears caches and errors. A workspace path change cancels pending work; reply IDs reject late results after invalidation, refresh or workspace switches.

Directory reads inspect at most 20,000 entries and retain at most 5,000 visible entries or 8 MiB of names and paths. The combined cache holds at most 256 directories, 50,000 entries and 64 MiB of estimated payload. Error storage is capped at 256 keys. At most 64 reads may be pending. The UI accepts at most two worker replies per frame and constructs at most 20,000 rows across 64 nesting levels. Reaching a limit reports an error.

Cancellation cannot interrupt a filesystem call already waiting in the kernel. Such a call occupies one worker, never the UI thread. The panel refreshes directories on request after queue, limit or cancellation errors.

The explorer tests exercise loading, refresh, cancellation, stale reply rejection, workspace switches, limits, icons and exclusions on a real Cargo project. No project code is executed by the tree reader.
