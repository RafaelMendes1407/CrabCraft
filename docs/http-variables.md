# HTTP file variables

The graphical client and `crabcraft http` share the same variable resolver:

```http
@host = localhost:8080
@base = http://{{host}}
@token = example-token
### List items
GET {{base}}/items
Authorization: Bearer {{token}}
```

Declarations before a request apply to the entire file, including other `###` blocks.
The last declaration of a name wins. Names are case sensitive; surrounding whitespace
inside `{{ name }}` is ignored. References work in URLs, header names and values, and
request bodies. Substitution is literal; encode URL components and JSON strings yourself.
The request source and editor offsets are preserved.

A missing reference, cycle, missing closing braces or excessive expansion prevents the
request from being sent. Limits: 256 declared variables, 32 nested references and 1 MB
per expanded field and across cached variable values. Plain fields without substitutions
keep the existing HTTP client limits. The resolver does not read the process environment or
execute code. Environment files, OAuth acquisition and response scripts require separate
explicit actions described below; no variables are automatically loaded from the project.

## Explicit environment files

The HTTP Client window accepts an explicit JSON file path and environment name. Click
**Load HTTP environment** to read a bounded snapshot on a cancellable worker, then send a
request separately. There is no automatic search for `http-client.env.json`, private files,
process environment, or project configuration. Loading never sends a request or runs code.

```json
{
  "$shared": { "base": "http://127.0.0.1:3000" },
  "local": { "token": "example-token" },
  "staging": { "base": "https://example.test", "token": "another-example" }
}
```

Values must be strings. Precedence is `$shared`, selected environment, then `@name = value`
declarations in the request file. Nested references use the same bounded resolver. A selected
but missing, failed, cancelled or changed environment blocks sending instead of silently
falling back. Changing a field invalidates its snapshot; changes to disk need another explicit
load. Clear removes the selection, restoring file-only variables. Loaded values and paths
are memory-only and cleared on project changes; values are not shown in the environment
summary. The file may contain secrets: requests transmit substituted values to their chosen
endpoint, and the existing CLI response includes the resolved URL.

The CLI accepts both flags together:

```sh
crabcraft http requests.http --request 1 --env-file ./http-client.env.json --env local
```

There is no private-file merging or ambient-variable access. OAuth and reviewed scripts are
separate graphical-client actions below. Files must be regular UTF-8 JSON, at most 1 MiB, with 64 environments,
256 variables per object and 256 merged variables; names are bounded. FIFO/device reads are
rejected before opening and checked again on the opened descriptor. JSON nesting uses
serde_json's recursion limit. A native egui test loads an environment without making a request,
sends explicitly to a real loopback server with an authorization header, rejects stale fields,
and compiles/runs its real Cargo project only afterward. CLI, resolver, cancellation and
FIFO tests accompany it.

## OAuth 2.0 client credentials

The HTTP Client has a separate, explicit client-credentials panel. Enter the token endpoint,
client ID, client secret, optional scope, and resource **origin** (scheme, host, optional port;
no path). Acquire the token, then explicitly enable its use. Acquisition never sends the
resource request. The password field is masked; tokens and credentials are kept only in
memory, hidden from diagnostics and history, and cleared when switching projects.

The worker POSTs `grant_type=client_credentials` and scope as form data. Client credentials
use HTTP Basic with individually form-encoded ID/secret, as described by
[RFC 6749 §2.3.1](https://www.rfc-editor.org/rfc/rfc6749#section-2.3.1) and
[§4.4](https://www.rfc-editor.org/rfc/rfc6749#section-4.4). The acquired Bearer token goes
only into the Authorization header, following
[RFC 6750 §2.1](https://www.rfc-editor.org/rfc/rfc6750#section-2.1), and only for the reviewed
resource origin. A pre-existing Authorization header is rejected. Both endpoints require
HTTPS; literal loopback/localhost HTTP is accepted for local service testing. URL credentials,
fragments, endpoint queries, resource paths and origins without a host are rejected. Neither
token requests nor resource requests carrying these tokens follow redirects or use a proxy.

Changing any OAuth field invalidates the token. Expired/missing tokens block a request when
the use checkbox is enabled, rather than silently sending it without authentication. Token
responses are bounded to 64 KiB and must contain a Bearer `access_token` (up to 8192 ASCII
bearer characters) and integer `expires_in` between 1 and 86400 seconds. Expiry is measured
conservatively from before the exchange. Requests time out after 30 seconds; cancellation
closes the exchange. Server bodies and network details are deliberately omitted from errors
to avoid exposing credentials. There is no automatic refresh/retry, browser authorization,
PKCE, device flow, password grant, CLI acquisition, discovery or persistent credential store.

Seventeen HTTP tests pass, including native acquisition/use against an actual local token
endpoint in a real Cargo project, Basic/form encoding, origin/header conflicts, expiry,
redirect refusal on both requests, invalid/oversize replies and cancellation. The project
build script remains untouched until a separate explicit Cargo test. Response scripts require the separate review below and never run implicitly.

## Reviewed response scripts

One complete inline `> {% ... %}` response script (up to 64 KiB) is retained with its request
snapshot. Sending the HTTP request **never** runs it. In the HTTP Client, select the finished
request and open **Reviewed response script**. The project must be trusted. Select an absolute
path to a separately installed Node.js executable, review the complete source, approve one
execution, then click **Run reviewed response script**. A new selection/runtime or revoked
trust clears approval; each run consumes it. Files changed since the request was sent do not
replace the reviewed snapshot. The CLI never runs scripts.

```http
GET http://127.0.0.1:3000/items

> {%
client.test("status", () => client.assert(response.status === 200));
client.test("body", () => client.assert(response.body.value === 42));
client.log(response.headers.valueOf("Content-Type"));
%}
```

Supported synchronous helpers are `client.test(name, callback)`, `client.assert(condition,
message?)`, `client.log(value)`, `response.status`, `response.body`, `response.contentType`,
and case-insensitive `response.headers.valueOf(name)`. JSON bodies are decoded when declared
JSON; other UTF-8 bodies remain strings. Results show individual pass/fail reports and logs.
There is no global-variable mutation, environment mutation, pre-request scripts, external
script loading, async completion, imports API, automatic chaining or protocol compatibility
claim with other editors. Missing, multiple, malformed and oversized inline handlers remain
visibly unsupported. Responses must be complete UTF-8, up to 1 MiB, with headers up to 64 KiB;
serialized input is capped at 2 MiB. Request credentials/OAuth settings are not passed to
Node; the script receives its response, which itself may contain sensitive service data.

The fixed driver runs on a background worker in a private temporary directory, with an empty
inherited environment, a 128 MiB Node heap setting, five-second synchronous VM timeout,
ten-second process timeout, and stdout/stderr limits of 128 KiB each. Cancel, switching
selection/project, exiting, or revoking trust terminates the owned process group. Cancellation
does not undo script side effects or contain a trusted script that deliberately detaches.
The script can access files and network with the user's OS permissions. These resource
limits **are not an OS sandbox**: Node's [VM documentation](https://nodejs.org/api/vm.html)
explicitly excludes it as a security boundary. Only approve scripts you trust. Node is not
bundled or installed automatically.

Three script tests passed with real Node 18.19.1: bounded parsing and binary rejection,
actual JS/JSON assertions with an intentional failure and logs, cancellation, and a native
review/trust flow in a real Rust project against a real local HTTP response. The last test
proves that sending/opening/rejecting does not execute the handler, one approval permits one
run, changing selection clears it, and the build script runs only during a subsequent
explicit Cargo command. The two Node-dependent tests are opt-in (`cargo test -p rc-app
http_client::scripts -- --include-ignored`); the normal HTTP regression suite passes 18 tests
with five opt-in tests ignored.
