# Web language syntax support

The native editor detects HTML, CSS, JavaScript (including JSX, MJS and CJS),
TypeScript (including MTS and CTS) and TSX by extension. The background parser
supplies syntax highlights and folding ranges. HTML retains automatic closing
of tags and quoted attributes. Comment actions use each language's delimiters.

HTML script/style bodies are parsed and highlighted on the background parser. Each region is
limited to 64 KB, 256 KB total, 32 regions and 16,384 emitted spans per document. Editing clears
old embedded layers until a current parse arrives. JavaScript, module scripts, basic TypeScript
script markers and CSS style blocks are supported; JSON/template script bodies are skipped.
Style attributes, template languages and arbitrary preprocessors are not injected. JSX tags
currently retain the available JavaScript grammar colors.

## Explicit external language servers

Settings › Tools accepts absolute global paths for HTML, CSS, JavaScript/TypeScript language
servers and the global tsserver.js. No server is detected in node_modules, installed, downloaded
or launched for an untrusted workspace. A configured session starts only after trust and when
an appropriate web file is open. The global executable/tsserver path must be outside the
workspace. HTML, CSS and TS/JS/TSX use separate sessions alongside rust-analyzer; web edits are
never sent as Rust/C# documents. Closing the project stops them and cancels requests.

The implementation speaks standard LSP over stdio (`--stdio`): completion and acceptance,
hover/signatures, diagnostics in the editor/Problems, definitions/type definitions/usages,
rename and formatting where the configured server supports those capabilities. Unimplemented
actions visibly report unsupported. The servers are external dependencies, not bundled IDE
plugins. The settings UI explicitly says an empty web path is unconfigured. Initialization has
a 20-second deadline; failure is reported when an action is requested. Settings changes stop
and replace the affected server. There is no automatic retry/install/network operation.

The adapter disables TypeScript automatic typing acquisition and points it at the separately
configured global tsserver.js. NODE_OPTIONS/NODE_PATH are cleared. Trusted project TypeScript
configuration and the installed server still run with normal user privileges; this is not an
OS sandbox and users must trust those external tools and the workspace. No npm script is run
by CrabCraft. External servers may implement their own behavior; do not configure untrusted
executables.

Web documents are limited to 2 MB for server synchronization. Server diagnostics are accepted
only for eligible open documents, checked against a version when supplied, and bounded to
1,000 diagnostics with 4,096 message characters. Unsolicited workspace/applyEdit is refused.
Rename may modify only unchanged, writable, open web files in this workspace: load all affected
files first. A snapshot/version mismatch, resource operation, closed/external target, more than
128 affected entries/1,000 edits or 2 MB replacement text is refused. Normal editor edits remain
undoable. General project-wide refactorings, framework plugins, arbitrary embedded-language
LSP routing and complete IDE parity are not claimed.

The queries in `crates/rc-syntax/queries/{html,css,javascript,typescript}.scm`
were written for CrabCraft. The linked parser dependencies use the MIT license:

- HTML: https://github.com/tree-sitter/tree-sitter-html (tree-sitter-html 0.23.2)
- CSS: https://github.com/tree-sitter/tree-sitter-css (tree-sitter-css 0.25.0)
- JavaScript: https://github.com/tree-sitter/tree-sitter-javascript (tree-sitter-javascript 0.25.0)
- TypeScript/TSX: https://github.com/tree-sitter/tree-sitter-typescript (tree-sitter-typescript 0.23.2)

The libraries retain their upstream license files in the Cargo packages. There
are no new proprietary assets or embedded server packages. External tools are installed by
the user, separately from the project. Public server/API references:

- [Microsoft HTML language service](https://github.com/microsoft/vscode-html-languageservice) and [CSS language service](https://github.com/microsoft/vscode-css-languageservice).
- [vscode-langservers-extracted](https://github.com/hrsh7th/vscode-langservers-extracted), the external CLI wrapper used in the test fixture.
- [typescript-language-server](https://github.com/typescript-language-server/typescript-language-server), its initialization options and global tsserver configuration.
- [Language Server Protocol specification](https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/).

Validation uses the source assets in `tests/fixtures/RustWeb`: a real Cargo
binary includes them with `include_str!`. The syntax tests parse the same files,
check representative colors and verify folding. Run:

```sh
cargo test -p rc-syntax
cargo run --offline --manifest-path tests/fixtures/RustWeb/Cargo.toml
```

The ignored native integration test requires an explicitly prepared external installation at
/tmp/crabcraft-web-tools. It was exercised with Node 18.19.1, vscode-langservers-extracted
4.10.0, typescript-language-server 4.3.3 and TypeScript 5.9.3. Installation is not part of the app:

```sh
npm install --prefix /tmp/crabcraft-web-tools --ignore-scripts --no-audit --no-fund \
  vscode-langservers-extracted@4.10.0 typescript-language-server@4.3.3 typescript@5.9.3
cargo test -p rc-app native_rust_web_servers --offline -- --ignored --nocapture
```

The test creates a real Rust application containing HTML/CSS/JS/TS assets, verifies no server
or build.rs execution before trust, accepts real server completions through the native popup,
navigates to JS/TS declarations, renames through the native dialog, and observes a real
TypeScript TS2322 diagnostic appear in Problems and disappear after Undo. It checks embedded
CSS/JS colors and runs the Rust binary offline against the changed assets. It then explicitly starts a real
rust-analyzer alongside all three web sessions, verifies their document sets stay separate and
requests a Rust local-variable completion. A separate parser
worker test uses the repository RustWeb assets and proves stale embedded layers are discarded;
a workspace-edit test refuses stale/read-only/unopened targets and resource operations.
This evidence does not compare against RustRover.
