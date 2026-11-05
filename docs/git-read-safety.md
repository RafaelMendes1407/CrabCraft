# Git helper execution and project trust

`Repository::discover` and `Git::command` start in restricted mode. An explicit
`Repository::with_trust(true)` or `GitCommand::trusted(true)` enables mutations and the
repository's configured helper programs. Callers must obtain project trust first; this API
is an authorization boundary, not a confirmation dialog. The desktop additionally defers
Git discovery/reads until project trust (see the integrated P26 project-trust barrier).

A restricted command accepts only a small read-command allowlist; unknown commands,
mutations and explicit external-diff/textconv/filter requests return `GitError::TrustRequired`.
It disables hooks, fsmonitor, pagers, automatic signature display, GPG helper programs,
submodule recursion/summary, alternate-ref commands and automatic maintenance. It blocks
all transport protocols and lazy object fetching. Missing objects in a partial clone fail
locally rather than invoking a remote/SSH/credential helper. Signing verification may report
CannotCheck until explicit trust enables the configured signing program.

Git can execute clean/process filters even during status or working-tree diff. The restricted
runner therefore enumerates configuration **names only**, including included driver names,
with a cancellable five-second / 1 MB preflight. It overrides each configured filter's clean,
smudge, process and required settings for the actual read. Dynamic diff command/textconv drivers are also neutralized. At most 256 combined drivers and
8 KB per configuration name are accepted. Unreadable, oversized or timed-out config fails
closed with `GitError::UnsafeReadConfig`. `cat-file` source reads always return raw blobs,
never smudge filters, LFS downloads or checkout conversions. Diff/log/blame disable external
diffs and text conversion. Submodule status is deliberately omitted in restricted mode;
reading nested repositories with their own helper config is not silently allowed.

Restricted reads are defense in depth for a static hostile repository, not an OS process
sandbox. Configuration could be replaced between preflight and command startup. The desktop's
strict policy of doing **no Git work before project trust** prevents that preflight race from
weakening the opening-project execution boundary. The Git executable itself must be a trusted
system/user tool. Explicit trust permits configured code: hooks, clean/smudge/process filters,
GPG signing, transport helpers and merge tools are not promised safe after that authorization.

All Git process output is bounded: stdout at 32 MB and stderr at 4 MB. Exceeding either kills
the process through the existing drop/cancellation path and returns `GitError::OutputLimit`.
Configuration preflight stderr has a separate 64 KB limit. Existing cancellation propagates
to both preflight and the real command. No config values, credentials or real user repository
files are changed by the restricted runner.

## Evidence

`cargo test -p rc-vcs --test read_safety -- --nocapture` creates temporary Rust repositories.
Its deliberate ordinary-Git controls create marker files proving execution through:

- core.fsmonitor and post-index-change hooks;
- clean, smudge and long-running process filters;
- textconv and external diff;
- repository-configured GPG verification of a synthetic signed commit;
- partial-clone lazy fetching through an ext transport.

The corresponding restricted status, log, blame, diff, raw blob, refs, stash, tag, worktree,
signature and missing-object reads leave all markers absent. Untrusted staging is rejected;
explicit trust restores clean-filter/hook execution. A 33 MB Rust source blob produces a
bounded output error, and cancellation terminates a config preflight blocked on a temporary
FIFO. All fixtures and executable control scripts exist only in temporary directories.

`cargo test -p rc-vcs --lib` and `cargo test -p rc-vcs --test repo` cover existing read/write,
patch, stash, worktree, branch, merge, transport and signing behavior. The signing integration
requires an environment that permits its temporary gpg-agent; sandbox denial is not a pass.
Test repositories performing writes explicitly opt into trust.

## Primary references

- [Git configuration](https://git-scm.com/docs/git-config): fsmonitor, hooksPath, filter drivers, GPG programs, automatic signature display and pagers.
- [Git hooks](https://git-scm.com/docs/githooks): fsmonitor and post-index-change execution.
- [Git attributes](https://git-scm.com/docs/gitattributes): clean/smudge/process and textconv drivers.
- [Git environment](https://git-scm.com/docs/git): GIT_ALLOW_PROTOCOL and GIT_NO_LAZY_FETCH.
- [Partial clone](https://git-scm.com/docs/partial-clone): on-demand fetching of missing objects.
