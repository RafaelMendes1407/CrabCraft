# Portable preferences synchronization

Settings → Preferences backup retains file export/import and adds explicit synchronization
with a private HTTPS object URL. The endpoint must support GET, PUT, strong ETags and
conditional writes. An optional Bearer token stays in memory with the URL and is discarded
when the settings dialog closes. No account, token or destination is saved in preferences.
Plain HTTP is accepted only for loopback hosts; embedded URL credentials and fragments are
rejected. Redirects are never followed, so credentials do not move to another endpoint.

Download fetches at most 1 MiB and validates the same versioned allowlist as a file backup.
It opens the existing import preview. **Apply imported preferences** modifies the draft;
**OK** is still required to persist and apply that draft. Download never silently replaces
current settings. Trust, project paths, history, executable paths, commands and environment
variables are excluded in both directions.

Download first to learn the remote version. A missing object enables creation with
`If-None-Match: *`; an existing object requires its strong ETag in `If-Match`. HTTP 409/412
invalidates the local revision and requires another download/review. Servers without a strong
ETag can be read but cannot be overwritten by this client. Changing URL or token invalidates
the version. Upload sends the current draft's portable fields; it is an explicit action.

Operations run on the asynchronous runtime with a 15-second timeout and bounded bodies.
Cancel or closing Settings aborts the task and discards pending results. Errors show status
or translated categories, never tokens, signed URLs or response bodies. There is no automatic
background synchronization, provider-specific login flow or implied subscription/service.
Use the same private endpoint on another machine and explicitly download/review there.

`cargo test -p rc-app settings_sync` validates round-trip and optimistic conflicts against
a real local HTTP server, secret exclusion, invalid URLs/tokens/backups, size bounds and task
cancellation. The native test opens an actual untrusted Cargo project, types URL/token, fetches
preferences, proves current settings stay unchanged until Apply then OK, verifies trust is
retained and confirms no Cargo/build-script execution. The token is absent from saved JSON.
