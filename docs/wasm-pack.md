# wasm-pack run configurations

A Cargo workspace member with a `cdylib` target gets a `wasm-pack: <package>` configuration
in the run selector. This identifies a candidate: a `cdylib` is not necessarily compatible
with WebAssembly. The crate normally uses `wasm-bindgen` and must support
`wasm32-unknown-unknown`.

Trust the Cargo project before running the configuration. Opening it only reads metadata;
Run may execute build scripts and procedural macros. Output appears in the native Run
window, with Stop and Rerun. Stop terminates the process tree, including Cargo and build
scripts. The configuration builds a package; it does not open a browser or publish it.

Edit the configuration to choose a JavaScript target (`bundler`, `web`, `nodejs`,
`no-modules`, `deno`), development or release build, output directory, Cargo features, and
environment variables. Relative output directories are relative to the package folder.
Options persist in the project session. Configuration variables override project variables.

Install `wasm-pack`, the `wasm32-unknown-unknown` Rust target and a `wasm-bindgen` CLI version
matching the crate's dependency before running. Missing tools are reported in the Run
console. CrabCraft always passes `--mode no-install --no-opt` and disables rustup automatic
toolchain installation. It never installs build tools implicitly. Cargo may still download
dependencies; set `CARGO_NET_OFFLINE=true` to prevent that.

The initial configuration uses release mode, the `bundler` target and `pkg` output. Custom
Cargo profiles, browser serving, wasm-opt configuration, wasm-pack test/pack/publish, and
attached projects do not have controls here.

Reference: [wasm-pack build documentation](https://github.com/wasm-bindgen/wasm-pack/blob/master/docs/src/commands/build.md).
No third-party assets are distributed.
