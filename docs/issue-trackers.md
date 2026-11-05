# Issue trackers

Git → **Issues** provides read-only GitHub and GitLab issue lookup for the open, trusted
Git project. Choose the service, API base URL and namespace/project. GitHub defaults to
`https://api.github.com`; GitLab defaults to `https://gitlab.com/api/v4`. GitHub Enterprise
and GitLab subpath installations can use their HTTPS API base. GitLab subgroup paths are
encoded as one project identifier. Numeric-only GitLab project IDs are unsupported.

Requests start only after **Fetch Issues**, **Previous** or **Next**. Opening a project,
restoring its session or switching tabs makes no request. Filters are open/closed/all;
pagination is explicit, 100 raw service records per page and at most 100 pages. GitHub
pull requests are filtered out of issue results. Empty pages can still be followed because
an entire GitHub page may contain pull requests.

An optional token is a masked, memory-only field. It is never stored in the project session,
Git configuration or settings. Changing API/project/service clears it and cached results.
It is marked sensitive in HTTP headers, and errors do not print credentials or server
response bodies. Requests have bounded timeouts, refuse redirects and accept HTTPS;
literal loopback HTTP is available for local development. Loopback bypasses proxies.
Private repositories require a service token with appropriate read permission; OAuth login
and credential storage are outside this feature.

Select a result to read its plain-text description or open its issue URL. Links are rebuilt
from the configured service/project and issue number; untrusted URLs returned by the server
are ignored. **Copy Issue Reference** copies a qualified `namespace/project#number`.
**Use in Commit Message** appends the reference/title to the current commit draft.
**Create Issue Branch…** opens the existing native branch dialog with a sanitized suggestion;
the branch is created and checked out only after explicit confirmation. This integration
never creates, edits, comments on or closes remote issues.

HTTP work is cancellable and runs off the UI thread. Limits include a 2 MiB response,
100 records, 1 KiB titles, 64 KiB descriptions and 4 KiB tokens. Invalid payloads, unsafe API
addresses, credential-bearing URLs, redirects and oversized results are refused. Configuration
is remembered in the workspace session; issue bodies and credentials are not persisted.

Linux evidence: a native test on a real Rust/Git project proves no request on open, explicitly
fetches a loopback API, excludes a pull request, keeps a token out of the serialized session,
appends a commit reference, confirms a real issue branch and passes `cargo test --offline`.
A public, read-only GitHub smoke test verifies the documented live issue contract without
a token. GitLab payload/header/path handling, cancellation/disconnection, redirect token
protection, response limits and input validation are exercised with local fixtures.
No remote write is performed by these tests. Live GitLab and cross-platform UI have not
been verified; screenshots have not been reviewed.

References: [GitHub issues REST API](https://docs.github.com/en/rest/issues/issues) and
[GitLab issues API](https://docs.gitlab.com/api/issues/). No parity claim is made.
