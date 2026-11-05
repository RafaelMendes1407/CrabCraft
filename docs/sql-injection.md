# SQL inside Rust strings

Rust SQL literals receive local keyword, identifier, number, parameter, string and comment
colors in the native editor. Ctrl+Space offers SQL keywords and the tables/columns of the
explicitly connected database. The source remains Rust: ordinary string escapes and raw-string
hash delimiters are preserved. Whole-string language-server semantic tokens cannot erase the
injected SQL colors.

Use a preceding `// language=SQL` comment (or immediate `/* language=SQL */`) for an explicit
injection. Direct first string arguments of sqlx::query/query_as/query_scalar macros/functions,
diesel::sql_query, and common database methods are also recognized. The method heuristic is
limited to receivers named db, conn, connection, client, pool, tx or transaction and SQL-like
initial keywords. It is a syntax hint, not compiler type resolution; annotate ambiguous cases.
Ordinary println strings and non-SQL method arguments are not classified merely by content.
Byte strings, dynamic format/concat expressions and arbitrary interpolation are unsupported.

The editor context menu offers **Open SQL in database console…** while the caret is inside a
recognized literal. It copies decoded SQL to the existing Database pane and does not execute
anything or rewrite the Rust source. Choose the connection, review permissions/parameters and
run explicitly. SQL with parameters may need values adapted in the console: arbitrary parameter
binding is not added by this action. There is no automatic database connection or background
SQL execution when opening, highlighting or completing a Rust file.

SQL colors use a handwritten lexer, including standard quotes, comments, placeholders and
PostgreSQL dollar-quoted strings. This is not a full dialect parser or SQL language server:
server-side type checking, schema diagnostics, query planning, query execution and arbitrary
SQL refactoring are outside this injection feature. Database metadata completion describes the
connected SQLite schema; it does not infer which connection a Rust variable refers to.

Completion quotes unusual/reserved database identifiers and encodes them for the original
Rust literal. A candidate that could terminate a raw-string delimiter is omitted rather than
changing delimiters unexpectedly. Local completion cancels pending Rust completion requests;
there is no completion/resolve request or registry/network lookup. Read-only Rust documents do
not accept local completion edits. Extraction requires a current parser snapshot, so stale
syntax is not used to copy another region.

## Bounds and evidence

Injection detection is bounded to 2 MB Rust documents, 64 KB literals, 16 visible injections
and 16,384 tokens per literal. Visible captures come from the existing Rust syntax tree; no
project code or macros are evaluated. Keyword/schema choices are bounded to 512 candidates
before filtering and 200 displayed results. The fragment never contains source outside its
literal; byte/Unicode offsets are mapped back to original Rust text.

`cargo test -p rc-app sql_in_rust_raw_and_escaped -- --nocapture` uses a real Rust rusqlite
project and SQLite file. Native Connect loads its schema, Ctrl+Space completes a table name
inside a multiline raw literal, Enter applies the edit, and extraction opens the SQL console
without a query/build-script run. The test checks quote/hash/escape preservation and executes
the edited Rust consumer with cargo run --offline against the actual database.

`cargo test -p rc-syntax sql` checks raw/escaped source mapping, SQL strings/comments/parameters,
non-SQL refusal, Unicode range validity, size limits and raw-delimiter-safe completion encoding.
`cargo test -p rc-app -p rc-editor whole_string_semantic_tokens` verifies that injected SQL and
Rust escapes retain their colors under a whole-string semantic token from a language server.

The lexer/keyword list and integration are handwritten; no third-party grammar/query assets
or proprietary descriptions are copied. Public references:
[SQLite language](https://sqlite.org/lang.html), [PostgreSQL lexical structure](https://www.postgresql.org/docs/current/sql-syntax-lexical.html),
and [Rust string literals](https://doc.rust-lang.org/reference/tokens.html#string-literals).
