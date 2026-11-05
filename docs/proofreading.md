# Local spelling and grammar checks

The native **Spelling and Grammar** tool window checks prose in an editable Rust file's
line/block/doc comments. Start a separately installed LanguageTool HTTP server locally,
enter its base address and a language code such as `en-US` or `pt-BR`, trust the project,
then choose **Check Comments**. No requests run on project opening, typing or tool-window
restoration. The entire configuration and results remain in memory.

Comment parsing, normalization and HTTP work stay off the UI thread. Only extracted comment
prose is sent; code tokens, strings, fenced/indented examples and inline code corrections
are excluded. Localhost resolves explicitly to loopback; credentials, remote hosts, proxy
paths and redirects are rejected, and environment HTTP proxies are disabled. The external
server receives the comment text and controls its own processing/logging. Use a server you
trust; the IDE does not start, install or configure it.

Each finding shows its category, message, original text and proposed replacements. Selecting
a finding navigates to the exact source range, including after Unicode characters. **Apply
Comment Correction** changes only that range in memory as one undo step. It never saves or
runs the project. Editing/saving/renaming/switching files, changing settings, losing project
trust, hiding the pane or cancelling discards obsolete work. Applying one correction clears
the other findings; run another check for fresh offsets.

Server replies are untrusted: UTF-16 ranges must align to real characters within one prose
segment. Proposed text cannot add line breaks, comment delimiters across boundaries, unsafe
control characters, code fences or a changed doc-comment prefix. Invalid/unsafe suggestions
are not applicable. Rust syntax must be supported by the bundled parser. This is comment
proofreading, not Rust identifier/string-literal checking or a claim of another IDE's parity.

Limits: 1 MiB source, 128 prose segments, 16,384 UTF-16 units of text, 256 findings, eight
displayed replacements per finding, 1 KiB replacement, 1 MiB reply and 30 seconds per HTTP
request. Cancel aborts the local HTTP future; the independent server retains its own limits.
No dictionary, Java runtime, LanguageTool program or third-party rules are distributed in
CrabCraft. Language availability and quality depend on the server; English and Brazilian
Portuguese were tested with LanguageTool 6.6.

External reference: [LanguageTool HTTP server documentation](https://dev.languagetool.org/http-server).
The separately installed project is [LanguageTool](https://github.com/languagetool-org/languagetool),
whose program uses LGPL and whose additional resources retain their individual licenses.
For example, its documented local invocation is `java -cp languagetool-server.jar
org.languagetool.server.HTTPServer --port 8081`; omit `--public` to keep it local. Check the
upstream installation requirements rather than executing tools from a project directory.

`cargo test -p rc-app proofreader` runs the mapping and injection-guard tests. With a local
server at port 8086, `CRABCRAFT_TEST_LANGUAGETOOL=http://127.0.0.1:8086 cargo test -p rc-app
proofreader -- --include-ignored --nocapture` also tests response limits/redirect/cancellation
and a native Cargo fixture: grammar plus spelling, review/apply/undo, stale results, no
build-script execution during checking, Portuguese suggestions and a later explicit Cargo run.
