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
