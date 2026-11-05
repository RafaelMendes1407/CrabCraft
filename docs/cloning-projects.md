# Cloning a project

The native welcome screen has **Clone Repository…**. Enter an HTTPS, HTTP, SSH, Git-protocol
URL or a local Git repository path and a destination folder. Its parent must exist and the
folder itself must not exist, including an empty folder or dangling symlink. The operation
reserves the destination atomically and never overwrites an existing directory.

Git runs on the background runtime in a cancellable process group, with bounded progress and
error output. **Cancel Clone**, Escape or application shutdown stops it. Cancellation or errors
can leave an incomplete destination; the IDE does not recursively delete it. A failed clone
never opens a project. Authentication is noninteractive: SSH uses batch mode and can use an
agent; terminal/askpass prompts, custom remote helpers and configured credential helpers are
not supported by this flow.

Clone does not initialize submodules. It uses an empty template directory, disables hooks,
ignores system/global Git configuration and inherited repository/config injection variables,
and restricts Git to standard explicit protocols. This also prevents globally configured
smudge filters from running during checkout. Local clones use the upload-pack transport
instead of hard-linking the source repository's objects. These restrictions apply only to
this new clone process; existing repositories and Git settings are not rewritten.

After success the destination opens through the regular Cargo-project flow without trust.
Trust saved for an older project at the same destination is removed before opening. Cargo
metadata can load, while build scripts, proc macros and project execution require a separate
trust decision.

Tests use a real local Git repository containing a Rust crate and build-script marker. Native
welcome controls clone and open it; the marker stays absent and stale trust is not inherited.
A subsequent explicit `cargo test --offline` proves the cloned source builds. Additional tests
reject existing destinations and external remote helpers, cancel a Git process waiting on a
local HTTP server, and compare an ordinary Git checkout that runs test filters/template hooks
with the IDE clone that suppresses them. Public-host authentication was not exercised.
