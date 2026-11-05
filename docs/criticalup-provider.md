# Explicit CriticalUp provider

The native **Rust Toolchain Provider** window selects either Rustup/PATH or a separately installed CriticalUp manager for IDE Cargo actions. The status bar always identifies the current provider. A new session starts with Rustup/PATH; CriticalUp selection is not automatically restored after restarting the IDE. Reopening or reloading the **same** workspace within the session preserves a selected CriticalUp context, including a pending/failed selection, so a reload cannot silently revert to rustup. Opening another workspace starts a new explicit selection.

## Selection and trust

Inspect reads only a regular `criticalup.toml` at the canonical Cargo workspace root. It runs on a cancellable worker, refuses symlinks/nonregular files including FIFOs, bounds input to 64 KiB and requires UTF-8 TOML with `manifest-version = 1`, one `ferrocene` product, bounded package/release names and Cargo/rustc packages. Parent manifests, extra products/settings and unsupported placeholder variables are rejected. No tool starts during inspection or merely because a project/layout opens.

Inspection creates a private temporary copy of the validated manifest and shows its release, packages and SHA-256. Activation requires an explicit Select action and trusted project. It runs the chosen manager's version and local `which --project <snapshot>` queries on a cancellable worker with 64 KiB output-pipe and 15-second per-query limits. CriticalUp 1.4 or newer is required. The manager path is canonicalized **before invocation**, and its resulting filename must be `criticalup`/`criticalup.exe`; a symlinked Cargo proxy may resolve to that manager, but hardlinked Cargo/rustc proxies and rustup binaries are refused. This preserves management-command dispatch rather than accidentally invoking a tool proxy with management arguments.

Activation does not sign in, install or download a compiler. Missing installations are reported as manager errors, without substituting rustup. Authenticate and install licensed Ferrocene outside the IDE when required. No credential/token field, portal call or compiler distribution is added. CriticalUp itself may maintain its local state; CrabCraft does not store account credentials or alter the original manifest.

Choosing a provider stops build/run/test and incompatible language services, cancels pending open-editor formatting, and discards old metadata result inboxes. Pending or failed CriticalUp selection leaves Cargo blocked. Only the explicit **Use Rustup / PATH** button returns to the default provider. Operation generations ignore late activation responses after cancellation or a newer selection.

## Coherent command routing

A selected context owns its private manifest until all Cargo/formatter jobs release it. Cargo, rustc and rustfmt commands use separate argv elements:

```
criticalup run --strict --project <private-manifest> cargo ...
criticalup run --strict --project <private-manifest> rustc ...
criticalup run --strict --project <private-manifest> rustfmt ...
```

The snapshot is deliberately pinned: inspect and select again after changing the original manifest. This prevents a new Cargo installation being selected from changed data while retaining an old rustc path. Activation resolves installed Cargo and rustc from that same snapshot, requires canonical tool filenames and a shared installation `bin` directory, and retains their absolute paths. `RUSTC`, `RUSTDOC` and `CARGO` are fixed to those resolved tools. Missing rustdoc is an explicit missing-tool path, not a PATH fallback. Project compiler/wrapper/CriticalUp-control overrides are filtered; wrapper variables are explicitly empty to override Cargo configuration too. Management-only inherited manifest/test/token variables are cleared. This does not modify the IDE's global environment or affect terminals, plugins, user external tools or agents, which retain their own environments.

Metadata returns the actual Cargo provider together with its workspace to Runner. Dependency/library/attached-project queries, cfg/host/sysroot probes, scratch compilation, build/run/test and rustc explanations retain that provider. Formatting uses its named tool and snapshot lifetime; absent CriticalUp rustfmt and Rustup-only formatter channels have provider-specific errors rather than suggesting a rustup installation. Clippy/fmt helper commands use strict named `cargo-clippy`/`cargo-fmt` routing. Unknown external Cargo extensions are refused instead of resolving a rustup proxy from PATH.

Rust language services/doctest completion, REPL, wasm-pack, coverage execution and other third-party Cargo extensions including nextest remain visibly unsupported in CriticalUp mode. They are paused, disabled or rejected before invocation. LCOV import remains available. This avoids those independent tools silently selecting an upstream compiler. Existing language services resume only when explicitly returning to Rustup/PATH. Qualified Ferrocene compilation and debugging have **not** been established by these tests; no compiler qualification or IDE certification is claimed.

## Evidence

- Three Core tests pass for schema/size/symlink/FIFO rejection, canonical manager/proxy names, missing-tool errors and cancellation. A deterministic management fixture routes to already installed **upstream** Cargo/rustc solely to test the integration: real metadata/build, cfg/host probes and rustfmt work despite poisoned `RUSTC`, wrapper variables and Cargo config. Editing the original manifest does not change the snapshot. These are routing tests, **not Ferrocene compiler evidence**.
- Two native headless tests pass in real Cargo projects. They prove no manager starts before explicit trusted selection, pending/failed selection blocks Runner, metadata preserves the selected provider, Runner builds and executes real Rust through the fixture, same-workspace reload preserves it, cancellation ignores late results, and return to Rustup requires the explicit button. No wrong-compiler marker runs.
- The separately downloaded **official CriticalUp 1.6.0** binary, verified against its official SHA-256, passes an opt-in isolated-state test: version works and missing installed toolchain produces an error rather than invoking rustup. Its version output is correctly accepted from stderr. No token, authentication, installation, network compiler download or licensed compiler is exercised.
- The existing `rc-cargo` suite passed outside the restricted sandbox: 121 unit tests and 11 integration tests, with seven opt-in tests ignored (`/tmp/crabcraft-criticalup-regressions-root.log`). After adding one neutral-directory explanation guard, the final sandbox run passed 119 unit tests and 11 integration tests, filtering only the three unchanged local-server bind cases already verified by the unrestricted run (`/tmp/crabcraft-criticalup-regressions-final.log`). Explanation requests without a trusted project use a neutral directory and no project environment; an untrusted CriticalUp context is refused rather than selecting a global compiler.

P-05 remains partial because a licensed Ferrocene toolchain is unavailable and language-service/auxiliary integration still needs matching-toolchain validation. The manager is not bundled with CrabCraft.

## Public references

- [CriticalUp running tools: strict mode, explicit tool paths and proxies](https://criticalup.ferrocene.dev/using-criticalup/running-tools.html)
- [CriticalUp toolchain management and external installation/authentication prerequisites](https://criticalup.ferrocene.dev/using-criticalup/toolchain-management.html)
- [CriticalUp changelog, including strict-run behavior](https://criticalup.ferrocene.dev/changelog.html)
- [Official 1.6.0 release and checksums used for the isolated CLI test](https://github.com/ferrocene/criticalup/releases/tag/v1.6.0)

No proprietary compiler, binary, artwork or qualification material is distributed.
