# Native SQLite database tools

Open the Database tool window from the native dock/window menu. Database work requires a
trusted project, but connecting does not start project code. Nothing opens a database or
runs SQL merely because a project contains a database file.

Enter the SQLite file and press Connect. Read-only is the default. To create a new database,
explicitly enable writes and choose Create SQLite database; an existing path is refused.
Disconnect before changing file or write permissions. PostgreSQL is visibly unsupported in
this first increment; it is not included in the verified status below.

## Schema, console and cells

Choose a table/view and Browse table. Table schema shows column declarations, primary keys,
the original CREATE statement, indexes and triggers. Results page in groups of 500. Views,
virtual tables, generated columns and tables with no safe key remain read-only. Rowid tables
use an unshadowed rowid alias; WITHOUT ROWID tables use their ordered primary key.

SQL console executes one statement per Run SQL, using a fresh connection and autocommit.
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
Database constraints, triggers and cascading changes apply as normal SQLite semantics.

## Typed JSON transfer

Choose a table and enter a transfer path. Export table JSON writes typed values (including
NULL, 64-bit integers, Unicode, real values and byte arrays) in the versioned
`crabcraft.sqlite.rows.v1` format. It never overwrites an existing file. This format is a data
transfer, not a database backup or SQL dump; it contains no DDL, indexes or executable SQL.
Generated columns are omitted.

Import JSON into table appends to the explicitly selected table, requiring the exact ordered
writable column names. All inserted rows use one transaction and bound parameters. Constraint
errors/cancellation before commit roll the whole import back. No SQL stored inside an imported
text cell is executed. CSV and other transfer formats are not supported in this increment.

## Work scheduling and bounds

All connection, SQLite, file import/export and serialization work occurs on cancellable
workers. Closing the pane/project cancels pending work and discards stale generations. SQLite
checks cancellation/deadline every 1,000 VM instructions, with a ten-second execution budget
and 150 ms busy timeout. Cancelling a completed/committing write cannot promise to undo it;
the pane says that a write may already have committed. Failed/cancelled work does not silently
retry SQL.

Limits: one regular non-symlink SQLite file; 64 KB SQL; 256 columns; 200 schema tables/views;
128 indexes/triggers per table; 500 visible rows per page, 2 MB result/value budget and 64 KB
editable cell values. Transfers permit 5,000 rows, 2 MB values and 8 MB JSON. An oversized
export fails, never writes a silent partial export. Pages beyond one million rows are refused.
SQLite itself additionally limits values/rows to 1 MB, expressions, compound queries, VM size,
bound variables and trigger recursion. Huge/unsupported table definitions are rejected.

Connections use defensive mode, trusted_schema=OFF and foreign-key enforcement. SQL cannot
ATTACH/DETACH other databases, load extensions, create virtual tables, alter dangerous pragmas
or enable writable_schema. No extension loader or shell executable is installed. Read-only
means SQLite database writes are disabled; normal SQLite WAL/shared-memory behavior is still
subject to its own filesystem rules. Like any local file API, concurrent hostile filesystem
replacement has narrow races; this is not an OS filesystem sandbox.

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
until that explicit Rust compilation. This evidence does not verify PostgreSQL or claim
RustRover parity.

## Public references and dependency provenance

- [rusqlite 0.40 documentation](https://docs.rs/rusqlite/0.40.2/rusqlite/): native Rust API, prepared statements, progress hooks, authorizers and runtime limits.
- [SQLite defensive programming](https://sqlite.org/security.html), [authorizer](https://sqlite.org/c3ref/set_authorizer.html), [limits](https://sqlite.org/limits.html) and [transactions](https://sqlite.org/lang_transaction.html).
- rusqlite/libsqlite3-sys are distributed as Cargo dependencies under their published licenses; the bundled SQLite engine is public domain. No third-party interface assets or documentation prose are copied.
