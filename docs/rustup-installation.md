# Installing rustup from CrabCraft

Settings → Tools → rustup → **Install rustup…** displays the official installer URL, the
Cargo and rustup installation directories, and the exact arguments. Nothing is downloaded
or executed until the user checks the confirmation and presses **Download and install rustup**.
Changing the toolchain option clears that confirmation.

The installer defaults to minimal stable Rust with `rust-src` and `rust-analyzer`. Uncheck the
option to install rustup and its proxies without downloading a toolchain. This mode requires
configuring a toolchain later. Existing component-install buttons remain available in Tools.

The native implementation downloads the executable and its `.sha256` sidecar from
`https://static.rust-lang.org/rustup/dist/<host>/rustup-init[.exe]`. Both requests require HTTPS;
there is a connection timeout, a total timeout, response-size limits and cancellation. The
checksum is mandatory and verified before a private executable file is written. The installer
is not bundled with CrabCraft. Supported hosts are x86-64 and AArch64 Linux (GNU/musl), macOS
and Windows MSVC; other platforms visibly decline automatic installation.

Execution uses `--no-modify-path`, explicit CARGO_HOME/RUSTUP_HOME, official Rust distribution
roots and a private working directory. Project environment variables, code and rust-toolchain
files are not involved. The installer may replace proxies or change the default stable toolchain
in the displayed destinations. Cancellation can leave completed changes on disk. Tool discovery
and the Cargo project model are refreshed afterwards; shell PATH must be adjusted separately
when necessary. Output is bounded and displayed in Tools. A running download or process can be
cancelled there even after closing and reopening Settings.

Validation on Linux used the real official binary: a headless UI test accepted the confirmation
and installed rustup-only into temporary homes. A backend test linked an existing toolchain
into those temporary homes and successfully ran `cargo test --offline` through the installed
Cargo proxy on a Rust crate. Tests also cover missing confirmation, option changes, cancellation
before download and corrupt checksum rejection. These live tests are ignored by default and
run only when explicitly requested. They do not install a second stable toolchain or change the
user's Rust installation. Windows/macOS installation has not been exercised.

Public references consulted:

- [Rustup installation and installation homes](https://rust-lang.github.io/rustup/installation/index.html).
- [Official installation methods and installer options](https://rust-lang.github.io/rustup/installation/other.html).
- [Rustup's public self-install/update implementation](https://github.com/rust-lang/rustup/blob/main/src/cli/self_update.rs), including the distribution URL and checksum protocol.
