# GitHub and GitLab hosting

Git → **Hosting** uses installed `gh` or `glab`, their existing credential stores and native
Rust UI. No command runs when opening/restoring a project or switching tabs. Choose the
service, HTTPS origin, namespace/project and CLI executable; **Check CLI** explicitly checks
its version. A custom executable must be a name on PATH or an absolute path. Configuration
remembers only service/origin/project; executable overrides, account data and form contents
are session-memory data. The project must be trusted before any CLI action.

**Load CLI Account** reads the active identity. **Log In in Terminal** opens the chosen CLI's
interactive authentication in a native terminal, without the project's environment or cwd.
GitHub uses its browser flow; GitLab uses its interactive flow. The CLI manages credentials;
CrabCraft does not request, display or store access tokens. Authentication scopes, multiple
accounts and account logout remain managed by the CLI.

Read controls list up to 50 open PRs/MRs, request details and a text diff, Actions/CI runs,
jobs and job logs. Select a row to fill the appropriate identifier; fetching remains explicit.
GitHub reads runs through Actions endpoints and `gh run view --log`; GitLab uses pipeline,
job and trace endpoints. Responses are bounded and displayed as native text, without HTML
or terminal execution. Large/truncated resources are not silently treated as complete.

Native forms prepare creation of a PR/MR, a top-level review/comment or approval, a remote
fork in the active account, a gist/personal snippet, and CI retry/cancellation. GitHub also
supports requesting changes. GitLab requesting-changes is visibly disabled. Load request
details before reviewing: GitHub review payloads carry the fetched commit ID, and GitLab
approvals carry its SHA. Draft requests and non-public sharing are defaults. Secret GitHub
gists are **unlisted**, not access-controlled; the UI warns before sharing. GitLab's
non-public snippet setting is private.

Every write first displays the target account, service, endpoint and exact JSON payload.
It requires a separate confirmation checkbox and **Send Reviewed Operation**. Editing
configuration or relevant fields invalidates the preview. The backend independently refuses
unconfirmed/untrusted writes and re-reads the CLI account immediately before sending; a
changed identity prevents the write. There is no automatic retry or rollback. Cancellation
can only stop the local process: the server may already have accepted the mutation, so
inspect the service before retrying. Fork never clones or changes local remotes.

Commands use argument vectors and JSON on stdin, never a shell, `--field` interpolation or
`@filename` upload magic. They execute in a private neutral directory without project env,
repository inference, CLI debug output or a pager. The child process group is killed on
cancel, timeout, output overflow or dropped work. Limits include 60 seconds per CLI command,
2 MiB stdout, 64 KiB stderr, 128 KiB JSON bodies, 16 KiB descriptions and 64 KiB snippet
content. Displayed output is limited to the first 64 KiB. No network background polling runs.

Outside this scope: automatic installation, repository checkout/merge, inline review
threads, organization fork selection, editing existing gists/snippets, workflow dispatch,
artifact download, large-result pagination and GitLab subpath-origin configuration. API
permissions and server-specific features can reject an otherwise valid request.

Linux evidence includes native tests on real Rust/Git projects for both providers using a
CLI fixture: no execution on opening, explicit version/account/list/detail/diff, confirmation
gating before mock POST, unchanged index, no build script execution during hosting, and a
subsequent explicit `cargo test --offline`. Backend fixtures exercise all seven write flows,
JSON literal handling, private defaults, SHA binding, account-change refusal and cancellation.
Real installed gh 2.45 reads public Rust PRs, and real glab 1.36 accepts the supported CLI
version/argument surface. No authenticated live GitLab API call, real remote write or real
login is performed by tests; no proof PR, review, fork, gist or snippet is published.
Cross-platform behavior and screenshots have not been verified.

References: [gh api](https://cli.github.com/manual/gh_api),
[glab api](https://docs.gitlab.com/cli/api/),
[GitHub reviews](https://docs.github.com/en/rest/pulls/reviews),
[GitHub gists](https://docs.github.com/en/rest/gists/gists),
[GitLab approvals](https://docs.gitlab.com/api/merge_request_approvals/) and
[GitLab snippets](https://docs.gitlab.com/api/snippets/). No RustRover parity is claimed.
