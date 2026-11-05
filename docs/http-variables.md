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
keep the existing HTTP client limits. No process environment, environment JSON file,
response script or OAuth helper is evaluated. Secrets stay in the file supplied by the user;
no variables are automatically loaded from the project.

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

No private-file merging, OAuth acquisition, JavaScript response handlers, or ambient variables
are provided. Files must be regular UTF-8 JSON, at most 1 MiB, with 64 environments,
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
build script remains untouched until a separate explicit Cargo test. Response scripts remain
unsupported and never run implicitly.
