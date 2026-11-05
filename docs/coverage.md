# Rust line coverage

Open the Tests tool window and select Coverage. Run with Coverage saves open documents and
runs `cargo llvm-cov --workspace --all-targets --lcov` for the trusted Cargo workspace.
Build scripts, procedural macros and tests can execute, so project trust is required.
Opening a project never starts coverage.

Install `cargo-llvm-cov` and the matching toolchain's `llvm-tools-preview` component before
running. CrabCraft sets `CARGO_LLVM_COV_SETUP=no` and `RUSTUP_AUTO_INSTALL=0`; it does not
install tools implicitly. Project environment variables apply, including
`CARGO_NET_OFFLINE=true` when network access is unwanted. The tool can download Cargo
dependencies otherwise.

Output streams into the coverage console. Stop terminates the process group, including
Cargo, tests and build scripts. Closing the project cancels collection. Only a successful,
fully parsed report becomes a new suite; cancelled or failed runs retain earlier suites.
LCOV output is written to a temporary directory outside the source tree and deleted after
parsing. Instrumentation artifacts stay under Cargo's `target` directory.

The native panel lists covered and instrumented lines per file and for the entire suite.
Select a file to list its execution counts; click a line or double-click a file to open
its source. Lists are virtualized. Coverage records refer to the source version when the
report was generated; editing files afterwards may make their line numbers stale.

Import LCOV adds a report without running project code. Relative source paths are relative
to the current project root. Absolute paths must resolve inside that root. Missing sources
and paths escaping through `..` or symlinks are skipped and counted. Reports from another
checkout may therefore need source-path adjustment before import.

Suites are independent snapshots in the current session, with distinct names and a Remove
Suite action. Up to 20 suites are retained; the oldest is removed when another is added.
They do not persist across application restarts or project changes. There is no combined
suite calculation or automatic comparison.

The LCOV reader requires a regular file, verifies the opened descriptor, and uses a
nonblocking open on Unix so FIFO replacement cannot stall startup. Bounded chunked reads
check cancellation between chunks. It accepts UTF-8 reports up to 32 MiB, 10,000 source files and 500,000 line
records. Line numbers must be between 1 and 10,000,000 and execution counts must fit `u64`.
Repeated source records merge by canonical path and sum line hits with saturation. Totals
come from actual `DA` records, not untrusted `LF`/`LH` declarations. Malformed section
structure, invalid line records and unterminated sections are errors. Function and branch
coverage records are ignored; only line coverage is currently supported. Import does not
fetch sources or execute commands from a report.

References:

- [cargo-llvm-cov usage and setup](https://github.com/taiki-e/cargo-llvm-cov/blob/main/README.md)
- [LCOV tracefile format](https://github.com/linux-test-project/lcov/blob/master/docs/man/geninfo.rst)

No third-party assets are distributed.
