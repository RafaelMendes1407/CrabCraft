# Axum endpoint snapshots and Reqwest calls

Open the **Endpoints** tool window in a Cargo project. It scans saved Rust sources in workspace
and attached package folders on a worker. **Refresh Endpoints** starts a new snapshot and
**Cancel Scan** or hiding the window cancels the scan. Opening a project does not run it,
expand macros or query a server.

The tree-sitter extractor recognizes Axum Router constructors, literal and raw-string `.route`
paths, HTTP method chains, `.nest`, `.merge`, and same-file router variables/factories. Names
and methods imported with aliases are supported. A result lists method, path and handler,
can be filtered, opens the route on double-click and offers navigation to a same-file handler
(or its expression when the definition cannot be resolved). Results are **static declarations**,
including inactive cfg/test branches, rather than a statement about a running server.

Macros, runtime strings/conditionals, mutable reassignment inference, wildcard imports,
cross-file composition are unsupported in the static scanner. In those cases
router-local paths may remain relative to an unresolved outer nesting prefix. The UI reports
unresolved expressions and incomplete scans; inspect the source and set the request base URL
accordingly. It does not promise a complete runtime route graph.

**Refresh Semantic Endpoints** is an explicit second scanner, enabled only for a trusted project
with an existing initialized Rust language server. It verifies constructor/method/handler
identities through rust-analyzer definitions and the defining package and impl. It follows
immutable bindings, literal path constants and zero-argument factory functions across saved
Rust modules, including modules that have never been opened in an editor. Aliased Axum imports,
`route`, `nest` and `merge` compose paths; handlers navigate to their defining source file.
Opening the Endpoints window still uses only the static scanner.

Semantic scanning runs on a cancellable worker. It starts no Cargo command, registry request,
server or explicit macro-expansion request. The existing trusted language server retains its
own configured analysis/build permissions. This is a source-declaration snapshot, including
inactive branches, not proof of a complete runtime graph. Dynamic paths, argument-dependent
factories, generated routers, unknown wrappers and cycles remain unsupported/incomplete.
Unresolved identities are never accepted merely because a method is named `route`.

The worker captures open Rust buffers, versions, paths, the language-server connection and
configuration epoch. Changes during scanning cancel publication; saved files are rechecked
before publication. Changing trust or restarting the server invalidates a published semantic
snapshot. Editing a source clears its matching hints; refresh before relying on cached routes.
Semantic limits are 128 files, 16 MiB of project source, 32 MiB including definition sources,
1,024 definition requests, 5,000 routes and a 45-second total deadline. Transient indexing errors
and temporarily absent definitions are retried for at most ten seconds within those limits.
Canceling drops pending requests. No source is saved or changed by either scanner.

**Route hints** shows methods and resolved static paths at the end of declaration lines in open
Rust editors whose contents match the snapshot. Editing a file clears its hints; refresh to
rebuild them.

In `.http` and `.rest` request lines, Ctrl+Space completes URL paths from the cached
Endpoints scan. Matching respects the request method (GET routes also accept HEAD), keeps
an existing scheme, host, query, fragment and HTTP version, and preserves typed capture
values. Missing capture values become editable snippet placeholders; fill them before
sending. Completion itself never scans, sends a request or executes project code.
Headers, bodies, scripts, comments and query positions do not offer route paths. Parsing
is bounded to the first 2,001 lines / 64 KiB before the caret and 8 KiB per line; at most
100 suggestions are shown. Refresh Endpoints after changing source routes.

F-06 remains partial. [RustRover's public Axum help](https://www.jetbrains.com/help/rust/axum.html)
describes automatic and manual path/query completion in Rust and non-Rust files. This
implementation provides manual HTTP request path completion and static Rust hints; it
does not complete query parameters or Rust URL strings and makes no parity claim. Semantic
composition and cross-file handler navigation are now available through the explicit scanner.

Select a result to generate a Reqwest call. Enter an HTTP(S) base URL, choose the method and
fill captured `{name}`, `{*wildcard}` or older `:name`/`*wildcard` path parameters. Values are
percent encoded, with wildcard slashes preserved; malformed parameters and dot-only segments
are refused. The preview is a standalone async Rust function using `reqwest::Client`, a
30-second timeout, status checking and response text. **Copy Reqwest Call** copies it; the IDE
sends nothing and never adds dependencies automatically. The caller needs Reqwest and an
async runtime. Headers, body/schema generation, query editing and authentication generation
are outside this version.

Safety bounds include 2 MB per regular source file, 64 MB total source bytes, 5,000 files /
visited directories, 50,000 directory entries, depth 32 and 10,000 project route rows. Symlinks,
project exclusions, target, .git, node_modules and .crabcraft folders are skipped. Parsing and
router traversal have separate depth/node budgets. File handles are checked as regular files;
Unix opens are nonblocking. The scanner never invokes Cargo, rustc, proc macros or build scripts.

Validation uses `tests/fixtures/RustAxum`, a real Axum 0.8.9 project that serves a parameterized
endpoint with Reqwest. Parser tests cover methods, nesting, aliases, raw strings and rejected
dynamic/unknown routers. Scanner tests cover exclusions/cancellation/symlinks. A headless
native test scans a copy of that project, navigates, checks hint invalidation and copies a
request without executing its build.rs. A separate live test compiles the generated Reqwest
function and verifies the encoded HTTP request against a local server. Those fixture-dependent
UI/compiler tests are ignored by default and were run explicitly.

The semantic fixture additionally verifies real Axum aliases, cross-file factories, nesting,
merging and resolved handler identities through rust-analyzer. Native controls scan initially
closed modules, navigate to a handler and exercise cancellation and stale configuration guards.

Primary references:

- [Axum Router API](https://docs.rs/axum/0.8.9/axum/struct.Router.html).
- [Axum HTTP method routing](https://docs.rs/axum/0.8.9/axum/routing/index.html).
- [Reqwest Client API](https://docs.rs/reqwest/0.13.5/reqwest/struct.Client.html).
