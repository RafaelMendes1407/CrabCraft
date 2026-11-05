# Large Rust files

The native editor uses a rope and paints the visible viewport. Files up to 8 MiB retain
editing and background syntax parsing; larger files open read-only without a syntax tree.
Files over 128 MiB are rejected before allocating their contents. The regular-file and
UTF-8 checks also apply. Initial document disk I/O is still synchronous (tracked in E-12).

`cargo test -p rc-app large_file_audit -- --nocapture` creates a genuine Cargo project and
checks Rust files of 2,097,233 bytes (23,566 lines) and 9,437,241 bytes (106,038 lines).
It waits for background syntax on the editable file, navigates the native editor to distant
lines, types and undoes an edit, verifies that read-only input leaves the larger file intact,
and runs offline `cargo check` on both. A sparse 129 MiB file is refused without a document.

On the local test run, eight navigation frames took approximately 14 ms and 5 ms respectively;
the complete test including both compilations took 1.21 seconds. These are debug-build local
measurements, not a performance guarantee or a comparison with another IDE. There is no
fragile frame-time assertion in the regression test.
