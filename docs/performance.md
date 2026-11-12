# Editor performance

The original measurements in this document used a generated C# file and are not evidence for the Rust editor. The benchmarks now use generated Rust source. Run the current benchmarks with:

    cargo bench -p rc-syntax
    cargo bench -p rc-editor --bench frame

Record new measurements on a real Rust project before using them as feature evidence.
