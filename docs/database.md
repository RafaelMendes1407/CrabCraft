# Native SQLite and PostgreSQL database tools

Open the Database tool window from the native dock/window menu. Database work requires a
trusted project, but connecting does not start project code. Nothing opens a database or
runs SQL merely because a project contains a database file.

Enter the SQLite file and press Connect. Read-only is the default. To create a new database,
explicitly enable writes and choose Create SQLite database; an existing path is refused.
Disconnect before changing connection or write permissions. Select PostgreSQL for an existing
local server: Unix socket directory or a loopback IP/localhost, port, database, user and optional
password. Remote hosts and TLS are visibly unsupported. No DNS lookup, CLI helper or project
process is used. Passwords remain in memory, are masked, clear on Disconnect/project close and
are never serialized to project settings. PostgreSQL creation/administration is outside this tool.

## Schema, console and cells

Choose a table/view and Browse table. Table schema shows column declarations, primary keys,
the original SQLite CREATE statement, indexes and triggers. PostgreSQL additionally shows
constraints, catalog column/default declarations and view definitions (not a reconstructed
restorable SQL dump). Schema preview is limited to 8,192 characters. Results page in groups of 500. Views,
virtual tables, generated columns and tables with no safe key remain read-only. Rowid tables
use an unshadowed rowid alias; WITHOUT ROWID tables use their ordered primary key. PostgreSQL
tables require a primary key; names are schema-qualified and safely quoted.

SQL console executes one statement per Run SQL on a fresh connection. SQLite uses autocommit;
PostgreSQL wraps each request in a repeatable-read transaction and commits after success.
Parameters for arbitrary console SQL and persistent/manual transactions are not supported;
BEGIN/COMMIT/savepoint and multiple statements are refused. Read-only connections reject
writes. Enable database writes explicitly before connecting to use INSERT/UPDATE/DELETE/DDL.
Query results are bounded and identify truncation. A mutating RETURNING query is drained to
completion under the time limit; a truncated visible result is not a partial SQL execution.

Click a writable cell, choose NULL/INTEGER/REAL/TEXT/BLOB, enter a value, then Apply cell edit.
BLOB uses hexadecimal; NULL is a distinct type. Updates use bound parameters, quoted identifiers,
a unique key and every original column value/type with binary text comparison. An immediate
transaction checks the captured schema version and row snapshot before updating; external
row/schema changes are rejected rather than overwritten. Generated fields are not edited.
Database constraints, triggers and cascading changes apply normally. PostgreSQL compares
primary keys, original column text normalized through catalog types and a captured catalog
version/definition inside its transaction. Serialization conflicts and changed snapshots require
a refresh. PostgreSQL NUMERIC, dates, JSON, UUID, booleans and other nonprimitive values are
shown as server text; this avoids converting exact decimal values to floating point.

## JSON and CSV transfer

Choose a table and enter a .json or .csv transfer path. Export table writes typed JSON values (including
NULL, 64-bit integers, Unicode, real values and byte arrays) in the versioned
`crabcraft.sqlite.rows.v1` format, or `crabcraft.postgresql.rows.v1` for PostgreSQL. It never overwrites an existing file. This format is a data
transfer, not a database backup or SQL dump; it contains no DDL, indexes or executable SQL.
Generated columns are omitted.

Import into table appends to the explicitly selected table, requiring the exact ordered
writable column names. All inserted rows use one transaction and bound parameters. Constraint
errors/cancellation before commit roll the whole import back. No SQL stored inside an imported
text cell is executed.

CSV is UTF-8 with a header row, comma separators and RFC 4180 quotation/newline rules. The
CrabCraft dialect reserves `\N` for NULL and `\x` plus hex digits for binary values; empty
fields are empty text. Leading backslashes in text are escaped with another backslash. Formula-like
text beginning with =, +, -, or @ (after whitespace) is prefixed with a backslash on export and
unescaped on import. Import follows the selected SQL column types/SQLite affinity; CSV does
not preserve every dynamic SQLite value type. Use JSON when exact SQLite types matter.
Other CSV dialects, SQL dumps and streaming bulk COPY are not implemented.

## Work scheduling and bounds

All connection, database, file import/export and serialization work occurs on cancellable
workers. Closing the pane/project cancels pending work and discards stale generations. SQLite
checks cancellation/deadline every 1,000 VM instructions, with a ten-second execution budget
and 150 ms busy timeout. Cancelling a completed/committing write cannot promise to undo it;
the pane says that a write may already have committed. Failed/cancelled work does not silently
retry SQL. PostgreSQL sends a protocol cancellation request, closes the worker connection and
uses statement_timeout=10s, lock_timeout=150ms, a 3s connection budget and a 12s job budget.
Its protocol wrapper validates every frame before forwarding its length to the driver: 2 MB per
frame and 8 MB total inbound traffic per job, including startup/authentication and discarded rows.

Limits: one regular non-symlink SQLite file; 64 KB SQL; 256 columns; 200 schema tables/views;
128 indexes/triggers/constraints per table; 500 visible rows per page, 2 MB result/value budget and 64 KB
editable cell values. Transfers permit 5,000 rows, 2 MB values and 8 MB transfer files. An oversized
export fails, never writes a silent partial export. Pages beyond one million rows are refused.
SQLite itself additionally limits values/rows to 1 MB, expressions, compound queries, VM size,
bound variables and trigger recursion. Huge/unsupported table definitions are rejected.

Connections use defensive mode, trusted_schema=OFF and foreign-key enforcement. SQL cannot
ATTACH/DETACH other databases, load extensions, create virtual tables, alter dangerous pragmas
or enable writable_schema. No extension loader or shell executable is installed. Read-only
means SQLite database writes are disabled; normal SQLite WAL/shared-memory behavior is still
subject to its own filesystem rules. Like any local file API, concurrent hostile filesystem
replacement has narrow races; this is not an OS filesystem sandbox.

A read-only PostgreSQL transaction is a database write restriction, not a sandbox for functions
installed on the server. Use a least-privilege database role. Explicitly submitted SQL and
selected views/triggers run with normal server privileges; superuser SQL can have external
effects. The local-only connection restriction does not turn a hostile server into trusted code.

## Verified evidence

`cargo test -p rc-app database_core::tests` creates actual SQLite files inside a Rust project:
Unicode/NULL/BLOB/REAL, schema metadata, generated fields, rowid/composite-key editing, stale
row/schema rejection, read-only refusal, typed JSON round trip, no-clobber export, constraint
rollback, unsafe/multiple SQL refusal, large values and cancellation of a nonterminating CTE.

`cargo test -p rc-app native_sqlite_connect_console_edit -- --nocapture` clicks the native
controls: untrusted Connect is ineffective, explicit test authorization enables schema/browse,
Apply updates a cell, an external edit causes refusal, SQL UPDATE runs, JSON exports/imports,
and Cancel stops a recursive query. A real standalone Rust rusqlite consumer then runs with
`cargo run --offline` and verifies edited/imported rows. Its build.rs marker remains absent
until that explicit Rust compilation.

`cargo test -p rc-app postgres --offline -- --ignored --nocapture` uses a real PostgreSQL 16
server (Unix socket and loopback SCRAM authentication). Backend tests cover metadata, exact
NUMERIC/bytea/NULL/Unicode values, keyed edits, stale/refused read-only writes, JSON/CSV round
trips, constraint rollback, bounded large results and cancellation of pg_sleep. The native test
clicks Connect/Browse/Apply/Export/Import, checks the trust gate and compiles/runs a standalone
Rust tokio-postgres consumer against the edited/imported database. The TCP test checks SCRAM
success, bad-password refusal and remote-host refusal. These tests are ignored by default
because they require that explicit private server fixture. No RustRover parity is claimed.

## Public references and dependency provenance

- [rusqlite 0.40 documentation](https://docs.rs/rusqlite/0.40.2/rusqlite/): native Rust API, prepared statements, progress hooks, authorizers and runtime limits.
- [SQLite defensive programming](https://sqlite.org/security.html), [authorizer](https://sqlite.org/c3ref/set_authorizer.html), [limits](https://sqlite.org/limits.html) and [transactions](https://sqlite.org/lang_transaction.html).
- [PostgreSQL transaction isolation](https://www.postgresql.org/docs/current/transaction-iso.html), [read-only transactions](https://www.postgresql.org/docs/current/sql-set-transaction.html), and [tokio-postgres APIs](https://docs.rs/tokio-postgres/0.7.18/tokio_postgres/).
- [RFC 4180 CSV quotation rules](https://www.rfc-editor.org/rfc/rfc4180). The marker/escape dialect described above is implemented locally.
- tokio-postgres/postgres-protocol and other Rust dependencies retain their published Cargo license notices.
- rusqlite/libsqlite3-sys are distributed as Cargo dependencies under their published licenses; the bundled SQLite engine is public domain. No third-party interface assets or documentation prose are copied.
