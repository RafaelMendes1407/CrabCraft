//! SQLite worker operations. No project processes, shell commands, loadable extensions or
//! SQL from imported data. Every request uses a bounded connection and typed parameters.
use rusqlite::{
    Connection, OpenFlags,
    config::DbConfig,
    hooks::{AuthAction, Authorization},
    limits::Limit,
    params_from_iter,
    types::{Value, ValueRef},
};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
const MAX_ROWS: usize = 500;
const MAX_BYTES: usize = 2 * 1024 * 1024;
const MAX_TRANSFER: usize = 8 * 1024 * 1024;
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "lowercase")]
pub enum Cell {
    Null,
    Integer(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
}
impl Cell {
    pub fn display(&self) -> String {
        match self {
            Self::Null => "NULL".into(),
            Self::Integer(value) => value.to_string(),
            Self::Real(value) => value.to_string(),
            Self::Text(value) => value.clone(),
            Self::Blob(value) => value.iter().map(|byte| format!("{byte:02x}")).collect(),
        }
    }
    pub(super) fn size(&self) -> usize {
        match self {
            Self::Text(value) => value.len().max(16),
            Self::Blob(value) => value.len().max(16),
            _ => 16,
        }
    }
    fn sql(&self) -> Result<Value, Error> {
        Ok(match self {
            Self::Null => Value::Null,
            Self::Integer(value) => Value::Integer(*value),
            Self::Real(value) if value.is_finite() => Value::Real(*value),
            Self::Real(_) => return Err(Error::Input),
            Self::Text(value) => Value::Text(value.clone()),
            Self::Blob(value) => Value::Blob(value.clone()),
        })
    }
    fn kind(&self) -> &'static str {
        match self {
            Self::Null => "null",
            Self::Integer(_) => "integer",
            Self::Real(_) => "real",
            Self::Text(_) => "text",
            Self::Blob(_) => "blob",
        }
    }
}
#[derive(Clone, Debug)]
pub struct Config {
    pub path: PathBuf,
    pub write: bool,
    pub create: bool,
}
#[derive(Clone, Debug)]
pub struct Column {
    pub name: String,
    pub declared: String,
    pub primary: usize,
    pub generated: bool,
}
#[derive(Clone, Debug)]
pub struct Table {
    pub local_name: Option<String>,
    pub name: String,
    pub kind: String,
    pub sql: String,
    pub columns: Vec<Column>,
    pub structures: Vec<(String, String, String)>,
}
#[derive(Clone, Debug, Default)]
pub struct Grid {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Cell>>,
    pub truncated: bool,
    pub affected: usize,
}
#[derive(Clone, Debug)]
pub struct Page {
    pub original_text: Option<Vec<Vec<Option<String>>>>,
    pub table: Table,
    pub grid: Grid,
    pub keys: Vec<Vec<Cell>>,
    pub key_names: Vec<String>,
    pub offset: usize,
    pub schema_version: i64,
}
#[derive(Clone)]
pub enum Action {
    Schema,
    Browse { table: String, offset: usize },
    Query(String),
    Update { page: Page, row: usize, column: usize, value: Cell },
    Export { table: String, path: PathBuf },
    Import { table: String, path: PathBuf },
}
pub enum Reply {
    Schema(Vec<Table>),
    Page(Page),
    Query(Grid),
    Updated,
    Exported(usize),
    Imported(usize),
}
#[derive(Debug)]
pub enum Error {
    Cancelled,
    Timeout,
    Limit,
    Input,
    ReadOnly,
    Stale,
    Path,
    Exists,
    Database(String),
    Io(String),
}
impl From<rusqlite::Error> for Error {
    fn from(error: rusqlite::Error) -> Self {
        Self::Database(error.to_string().chars().take(2048).collect())
    }
}
impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error.to_string().chars().take(2048).collect())
    }
}
fn ident(value: &str) -> Result<String, Error> {
    if value.is_empty() || value.len() > 1024 || value.contains('\0') {
        return Err(Error::Input);
    }
    Ok(format!("\"{}\"", value.replace('"', "\"\"")))
}
fn check(cancel: &AtomicBool) -> Result<(), Error> {
    if cancel.load(Ordering::Relaxed) { Err(Error::Cancelled) } else { Ok(()) }
}
fn open(config: &Config, cancel: Arc<AtomicBool>) -> Result<Connection, Error> {
    check(&cancel)?;
    if config.path.as_os_str().len() > 8192 {
        return Err(Error::Path);
    }
    if config.create {
        if !config.write {
            return Err(Error::ReadOnly);
        }
        let result = std::fs::OpenOptions::new().write(true).create_new(true).open(&config.path);
        match result {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err(Error::Exists);
            }
            Err(error) => return Err(error.into()),
        }
    }
    let metadata = std::fs::symlink_metadata(&config.path).map_err(|_| Error::Path)?;
    if !metadata.is_file() {
        return Err(Error::Path);
    }
    let flags = if config.write { OpenFlags::SQLITE_OPEN_READ_WRITE } else { OpenFlags::SQLITE_OPEN_READ_ONLY }
        | OpenFlags::SQLITE_OPEN_NO_MUTEX
        | OpenFlags::SQLITE_OPEN_NOFOLLOW;
    let mut connection = Connection::open_with_flags(&config.path, flags)?;
    connection.busy_timeout(Duration::from_millis(150))?;
    connection.set_db_config(DbConfig::SQLITE_DBCONFIG_DEFENSIVE, true)?;
    connection.set_db_config(DbConfig::SQLITE_DBCONFIG_TRUSTED_SCHEMA, false)?;
    connection.pragma_update(None, "foreign_keys", true)?;
    connection.pragma_update(None, "temp_store", "MEMORY")?;
    connection.pragma_update(None, "cache_size", -4096)?;
    for (limit, value) in [
        (Limit::SQLITE_LIMIT_LENGTH, 1024 * 1024),
        (Limit::SQLITE_LIMIT_SQL_LENGTH, 64 * 1024),
        (Limit::SQLITE_LIMIT_COLUMN, 256),
        (Limit::SQLITE_LIMIT_EXPR_DEPTH, 100),
        (Limit::SQLITE_LIMIT_COMPOUND_SELECT, 50),
        (Limit::SQLITE_LIMIT_VARIABLE_NUMBER, 1000),
        (Limit::SQLITE_LIMIT_VDBE_OP, 100_000),
        (Limit::SQLITE_LIMIT_ATTACHED, 0),
        (Limit::SQLITE_LIMIT_TRIGGER_DEPTH, 32),
        (Limit::SQLITE_LIMIT_WORKER_THREADS, 0),
    ] {
        connection.set_limit(limit, value)?;
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    connection.progress_handler(1000, Some(move || cancel.load(Ordering::Relaxed) || Instant::now() >= deadline))?;
    connection.authorizer(Some(authorize))?;
    Ok(connection)
}
fn authorize(context: rusqlite::hooks::AuthContext<'_>) -> Authorization {
    match context.action {
        AuthAction::Attach { .. }
        | AuthAction::Detach { .. }
        | AuthAction::Unknown { .. }
        | AuthAction::CreateVtable { .. }
        | AuthAction::DropVtable { .. } => Authorization::Deny,
        AuthAction::Function { function_name } if function_name.eq_ignore_ascii_case("load_extension") => Authorization::Deny,
        AuthAction::Pragma { pragma_name, pragma_value }
            if pragma_value.is_some()
                && !matches!(pragma_name.to_ascii_lowercase().as_str(), "table_info" | "table_xinfo" | "index_list" | "index_info" | "foreign_key_list") =>
        {
            Authorization::Deny
        }
        AuthAction::Pragma { pragma_name, .. }
            if !matches!(
                pragma_name.to_ascii_lowercase().as_str(),
                "table_info"
                    | "table_xinfo"
                    | "table_list"
                    | "index_list"
                    | "index_info"
                    | "foreign_key_list"
                    | "database_list"
                    | "compile_options"
                    | "foreign_keys"
                    | "foreign_key_check"
                    | "integrity_check"
                    | "quick_check"
                    | "user_version"
                    | "schema_version"
            ) =>
        {
            Authorization::Deny
        }
        _ => Authorization::Allow,
    }
}
fn cells(row: &rusqlite::Row<'_>, count: usize, bytes: &mut usize) -> Result<Vec<Cell>, Error> {
    let mut result = Vec::with_capacity(count);
    for index in 0..count {
        let value = match row.get_ref(index)? {
            ValueRef::Null => Cell::Null,
            ValueRef::Integer(value) => Cell::Integer(value),
            ValueRef::Real(value) => Cell::Real(value),
            ValueRef::Text(value) => Cell::Text(std::str::from_utf8(value).map_err(|_| Error::Input)?.into()),
            ValueRef::Blob(value) => Cell::Blob(value.into()),
        };
        *bytes = bytes.saturating_add(value.size());
        if *bytes > MAX_BYTES {
            return Err(Error::Limit);
        }
        result.push(value);
    }
    Ok(result)
}
fn query(connection: &Connection, sql: &str, max_rows: usize, cancel: &AtomicBool) -> Result<Grid, Error> {
    if sql.len() > 64 * 1024 {
        return Err(Error::Limit);
    }
    check(cancel)?;
    use fallible_iterator::FallibleIterator;
    let mut batch = rusqlite::Batch::new(connection, sql);
    let mut statement = batch.next()?.ok_or(Error::Input)?;
    if batch.next()?.is_some() {
        return Err(Error::Input);
    }
    let count = statement.column_count();
    if count > 256 {
        return Err(Error::Limit);
    }
    let mut result = Grid { columns: statement.column_names().map(|name| name.to_string()).collect(), ..Default::default() };
    if count == 0 {
        result.affected = statement.execute([])?;
        return Ok(result);
    }
    let readonly = statement.readonly();
    let mut rows = statement.query([])?;
    let mut bytes = 0;
    while let Some(row) = rows.next()? {
        check(cancel)?;
        if result.rows.len() >= max_rows {
            result.truncated = true;
            if readonly {
                break;
            } else {
                continue;
            }
        }
        result.rows.push(cells(row, count, &mut bytes)?);
    }
    result.affected = connection.changes() as usize;
    Ok(result)
}
fn table(connection: &Connection, name: &str) -> Result<Table, Error> {
    ident(name)?;
    let (kind, sql): (String, String) =
        connection.query_row("SELECT type,coalesce(sql,'') FROM sqlite_schema WHERE name=?1 AND type IN ('table','view')", [name], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })?;
    if sql.len() > 64 * 1024 {
        return Err(Error::Limit);
    }
    let mut statement = connection.prepare("SELECT name,type,pk,hidden FROM pragma_table_xinfo(?1) ORDER BY cid")?;
    let mut rows = statement.query([name])?;
    let mut columns = Vec::new();
    while let Some(row) = rows.next()? {
        if columns.len() >= 256 {
            return Err(Error::Limit);
        }
        let name: String = row.get(0)?;
        ident(&name)?;
        let hidden: i64 = row.get(3)?;
        if hidden == 1 {
            continue;
        }
        columns.push(Column { name, declared: row.get(1)?, primary: row.get::<_, i64>(2)?.max(0) as usize, generated: hidden != 0 });
    }
    let mut statement = connection
        .prepare("SELECT type,name,coalesce(sql,'') FROM sqlite_schema WHERE tbl_name=?1 AND type IN ('index','trigger') ORDER BY type,name LIMIT 129")?;
    let structures = statement
        .query_map([name], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    if structures.len() > 128 || structures.iter().map(|item| item.2.len()).sum::<usize>() > 256 * 1024 {
        return Err(Error::Limit);
    }
    Ok(Table { local_name: None, name: name.into(), kind, sql, columns, structures })
}
fn schema(connection: &Connection) -> Result<Vec<Table>, Error> {
    let mut statement =
        connection.prepare("SELECT name FROM sqlite_schema WHERE type IN ('table','view') AND name NOT LIKE 'sqlite_%' ORDER BY name LIMIT 201")?;
    let names = statement.query_map([], |row| row.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?;
    if names.len() > 200 {
        return Err(Error::Limit);
    }
    names.into_iter().map(|name| table(connection, &name)).collect()
}
fn browse(connection: &mut Connection, name: &str, offset: usize, cancel: &AtomicBool) -> Result<Page, Error> {
    if offset > 1_000_000 {
        return Err(Error::Limit);
    }
    let transaction = connection.transaction()?;
    let connection = &*transaction;
    let schema_version = connection.query_row("PRAGMA schema_version", [], |row| row.get::<_, i64>(0))?;
    let table = table(connection, name)?;
    let (without_rowid, kind): (i64, String) =
        connection.query_row("SELECT wr,type FROM pragma_table_list WHERE schema='main' AND name=?1", [name], |row| Ok((row.get(0)?, row.get(1)?)))?;
    let key_names = if kind != "table" {
        Vec::new()
    } else if without_rowid != 0 {
        let mut columns: Vec<_> = table.columns.iter().filter(|column| column.primary > 0).collect();
        columns.sort_by_key(|column| column.primary);
        columns.into_iter().map(|column| column.name.clone()).collect()
    } else {
        ["_rowid_", "rowid", "oid"]
            .iter()
            .find(|alias| !table.columns.iter().any(|column| column.name.eq_ignore_ascii_case(alias)))
            .map(|alias| vec![alias.to_string()])
            .unwrap_or_default()
    };
    let rowid = without_rowid == 0 && !key_names.is_empty();
    let prefix = if rowid { format!("{},", ident(&key_names[0])?) } else { String::new() };
    let sql = format!("SELECT {prefix}* FROM {} LIMIT {} OFFSET {offset}", ident(name)?, MAX_ROWS + 1);
    let mut grid = query(connection, &sql, MAX_ROWS, cancel)?;
    let mut keys = Vec::new();
    for row in &mut grid.rows {
        let key = if rowid {
            vec![row.remove(0)]
        } else {
            key_names.iter().filter_map(|name| table.columns.iter().position(|column| &column.name == name).and_then(|index| row.get(index).cloned())).collect()
        };
        keys.push(key);
    }
    if rowid {
        grid.columns.remove(0);
    }
    Ok(Page { original_text: None, table, grid, keys, key_names, offset, schema_version })
}
fn update(connection: &mut Connection, page: &Page, row: usize, column: usize, value: &Cell) -> Result<(), Error> {
    let transaction = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let connection = &*transaction;
    let version = connection.query_row("PRAGMA schema_version", [], |row| row.get::<_, i64>(0))?;
    if version != page.schema_version {
        return Err(Error::Stale);
    }
    let current = table(connection, &page.table.name)?;
    if current.sql != page.table.sql || page.key_names.is_empty() {
        return Err(Error::Stale);
    }
    let target = current.columns.get(column).filter(|column| !column.generated).ok_or(Error::Input)?;
    let before = page.grid.rows.get(row).ok_or(Error::Input)?;
    let keys = page.keys.get(row).filter(|keys| keys.len() == page.key_names.len()).ok_or(Error::Input)?;
    if before.len() != current.columns.len() || value.size() > 64 * 1024 {
        return Err(Error::Limit);
    }
    let mut values = vec![value.sql()?];
    let mut predicates = Vec::new();
    for (name, key) in page.key_names.iter().zip(keys) {
        predicates.push(format!("{} IS ?", ident(name)?));
        values.push(key.sql()?);
    }
    for (column, value) in current.columns.iter().zip(before) {
        predicates.push(format!("({} COLLATE BINARY IS ? AND typeof({})=?)", ident(&column.name)?, ident(&column.name)?));
        values.push(value.sql()?);
        values.push(Value::Text(value.kind().into()));
    }
    let sql = format!("UPDATE {} SET {}=? WHERE {}", ident(&current.name)?, ident(&target.name)?, predicates.join(" AND "));
    if connection.execute(&sql, params_from_iter(values))? != 1 {
        return Err(Error::Stale);
    }
    transaction.commit()?;
    Ok(())
}
#[derive(Serialize, Deserialize)]
pub(super) struct Transfer {
    pub format: String,
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Cell>>,
}
fn export(connection: &Connection, name: &str, path: &Path, cancel: &AtomicBool) -> Result<usize, Error> {
    let table = table(connection, name)?;
    let columns: Vec<_> = table.columns.iter().filter(|column| !column.generated).map(|column| column.name.clone()).collect();
    if columns.is_empty() {
        return Err(Error::Input);
    }
    let mut grid = query(
        connection,
        &format!("SELECT {} FROM {}", columns.iter().map(|name| ident(name)).collect::<Result<Vec<_>, _>>()?.join(","), ident(name)?),
        5000,
        cancel,
    )?;
    if grid.truncated {
        return Err(Error::Limit);
    }
    save_transfer(path, &Transfer { format: "crabcraft.sqlite.rows.v1".into(), columns, rows: std::mem::take(&mut grid.rows) }, cancel)
}
pub(super) fn save_transfer(path: &Path, transfer: &Transfer, cancel: &AtomicBool) -> Result<usize, Error> {
    let count = transfer.rows.len();
    let data = if path.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("csv")) {
        crate::database_csv::encode(transfer)?
    } else {
        serde_json::to_vec_pretty(transfer).map_err(|_| Error::Input)?
    };
    if data.len() > MAX_TRANSFER {
        return Err(Error::Limit);
    }
    check(cancel)?;
    use std::io::Write;
    let parent = path.parent().filter(|parent| !parent.as_os_str().is_empty()).ok_or(Error::Path)?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    temp.write_all(&data)?;
    temp.as_file().sync_all()?;
    check(cancel)?;
    temp.persist_noclobber(path)
        .map_err(|error| if error.error.kind() == std::io::ErrorKind::AlreadyExists { Error::Exists } else { Error::Io(error.error.to_string()) })?;
    Ok(count)
}
pub(super) fn load_transfer(path: &Path, cancel: &AtomicBool) -> Result<Transfer, Error> {
    let metadata = std::fs::symlink_metadata(path).map_err(|_| Error::Path)?;
    if !metadata.is_file() || metadata.len() > MAX_TRANSFER as u64 {
        return Err(Error::Limit);
    }
    use std::io::Read;
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW);
    }
    let file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(Error::Path);
    }
    let mut data = Vec::new();
    file.take(MAX_TRANSFER as u64 + 1).read_to_end(&mut data)?;
    if data.len() > MAX_TRANSFER {
        return Err(Error::Limit);
    }
    check(cancel)?;
    let transfer: Transfer = if path.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("csv")) {
        crate::database_csv::decode(&data)?
    } else {
        serde_json::from_slice(&data).map_err(|_| Error::Input)?
    };
    Ok(transfer)
}
fn import(connection: &mut Connection, name: &str, path: &Path, cancel: &AtomicBool) -> Result<usize, Error> {
    let transfer = load_transfer(path, cancel)?;
    if !matches!(transfer.format.as_str(), "crabcraft.sqlite.rows.v1" | "crabcraft.csv.rows.v1") || transfer.rows.len() > 5000 {
        return Err(Error::Input);
    }
    let current = table(connection, name)?;
    let columns: Vec<_> = current.columns.iter().filter(|column| !column.generated).map(|column| column.name.clone()).collect();
    if columns != transfer.columns || columns.is_empty() {
        return Err(Error::Input);
    }
    let sql = format!(
        "INSERT INTO {} ({}) VALUES ({})",
        ident(name)?,
        columns.iter().map(|name| ident(name)).collect::<Result<Vec<_>, _>>()?.join(","),
        vec!["?"; columns.len()].join(",")
    );
    let transaction = connection.transaction()?;
    let mut bytes = 0_usize;
    {
        let mut statement = transaction.prepare(&sql)?;
        for row in &transfer.rows {
            check(cancel)?;
            if row.len() != columns.len() {
                return Err(Error::Input);
            }
            bytes = bytes.saturating_add(row.iter().map(Cell::size).sum::<usize>());
            if bytes > MAX_BYTES {
                return Err(Error::Limit);
            }
            let values = row.iter().map(Cell::sql).collect::<Result<Vec<_>, _>>()?;
            statement.execute(params_from_iter(values))?;
        }
    }
    check(cancel)?;
    transaction.commit()?;
    Ok(transfer.rows.len())
}
pub fn run(config: &Config, action: Action, cancel: Arc<AtomicBool>) -> Result<Reply, Error> {
    let write = matches!(action, Action::Update { .. } | Action::Import { .. });
    if write && !config.write {
        return Err(Error::ReadOnly);
    }
    let mut connection = open(config, cancel.clone())?;
    let result = match action {
        Action::Schema => schema(&connection).map(Reply::Schema),
        Action::Browse { table, offset } => browse(&mut connection, &table, offset, &cancel).map(Reply::Page),
        Action::Query(sql) => {
            connection.authorizer(Some(|context: rusqlite::hooks::AuthContext<'_>| match context.action {
                AuthAction::Transaction { .. } | AuthAction::Savepoint { .. } => Authorization::Deny,
                _ => authorize(context),
            }))?;
            query(&connection, &sql, MAX_ROWS, &cancel).map(Reply::Query)
        }
        Action::Update { page, row, column, value } => update(&mut connection, &page, row, column, &value).map(|()| Reply::Updated),
        Action::Export { table, path } => export(&connection, &table, &path, &cancel).map(Reply::Exported),
        Action::Import { table, path } => import(&mut connection, &table, &path, &cancel).map(Reply::Imported),
    };
    if cancel.load(Ordering::Relaxed) { Err(Error::Cancelled) } else { result }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (tempfile::TempDir, Config) {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir(temp.path().join("src")).unwrap();
        std::fs::write(temp.path().join("Cargo.toml"), "[package]\nname=\"sqlite_evidence\"\nversion=\"0.1.0\"\nedition=\"2021\"\n").unwrap();
        std::fs::write(temp.path().join("src/main.rs"), "fn main() {}\n").unwrap();
        let mut config = Config { path: temp.path().join("project.sqlite"), write: true, create: true };
        run(&config, Action::Schema, Arc::new(AtomicBool::new(false))).unwrap();
        config.create = false;
        (temp, config)
    }
    fn sql(config: &Config, sql: &str) -> Grid {
        match run(config, Action::Query(sql.into()), Arc::new(AtomicBool::new(false))).unwrap() {
            Reply::Query(grid) => grid,
            _ => panic!("query response"),
        }
    }
    fn page(config: &Config, name: &str) -> Page {
        match run(config, Action::Browse { table: name.into(), offset: 0 }, Arc::new(AtomicBool::new(false))).unwrap() {
            Reply::Page(page) => page,
            _ => panic!("page response"),
        }
    }
    #[test]
    fn rust_project_sqlite_schema_typed_rows_keyed_edit_and_stale_protection() {
        let (_temp, config) = fixture();
        sql(
            &config,
            "CREATE TABLE \"user records\"(id INTEGER PRIMARY KEY,name TEXT COLLATE NOCASE,score REAL,data BLOB,missing TEXT,derived TEXT GENERATED ALWAYS AS (name||'!') VIRTUAL)",
        );
        sql(&config, "INSERT INTO \"user records\"(name,score,data) VALUES('O''Brien🦀',1.25,X'0001ff')");
        let before = page(&config, "user records");
        assert_eq!(before.grid.rows.len(), 1);
        assert_eq!(before.grid.rows[0][1], Cell::Text("O'Brien🦀".into()));
        assert_eq!(before.grid.rows[0][3], Cell::Blob(vec![0, 1, 255]));
        assert_eq!(before.grid.rows[0][4], Cell::Null);
        assert!(before.table.columns[5].generated);
        run(&config, Action::Update { page: before.clone(), row: 0, column: 1, value: Cell::Text("edited'🦀".into()) }, Arc::new(AtomicBool::new(false)))
            .unwrap();
        assert_eq!(page(&config, "user records").grid.rows[0][1], Cell::Text("edited'🦀".into()));
        assert!(matches!(
            run(&config, Action::Update { page: before, row: 0, column: 1, value: Cell::Text("stale".into()) }, Arc::new(AtomicBool::new(false))),
            Err(Error::Stale)
        ));
        let versioned = page(&config, "user records");
        sql(&config, "CREATE INDEX user_name_idx ON \"user records\"(name)");
        assert!(matches!(
            run(&config, Action::Update { page: versioned, row: 0, column: 1, value: Cell::Text("schema stale".into()) }, Arc::new(AtomicBool::new(false))),
            Err(Error::Stale)
        ));
        let mut readonly = config.clone();
        readonly.write = false;
        assert!(run(&readonly, Action::Query("DELETE FROM \"user records\"".into()), Arc::new(AtomicBool::new(false))).is_err());
        assert_eq!(page(&config, "user records").grid.rows.len(), 1);
        sql(&config, "CREATE TABLE composite(a TEXT,b INTEGER,value TEXT,PRIMARY KEY(a,b)) WITHOUT ROWID");
        sql(&config, "INSERT INTO composite VALUES('a',2,'before')");
        let composite = page(&config, "composite");
        assert_eq!(composite.key_names, ["a", "b"]);
        run(&config, Action::Update { page: composite, row: 0, column: 2, value: Cell::Text("after".into()) }, Arc::new(AtomicBool::new(false))).unwrap();
        assert_eq!(page(&config, "composite").grid.rows[0][2], Cell::Text("after".into()));
    }
    #[test]
    fn typed_json_round_trip_is_noclobber_and_import_rolls_back_on_constraint_failure() {
        let (temp, config) = fixture();
        sql(&config, "CREATE TABLE source(id INTEGER PRIMARY KEY,t TEXT,r REAL,b BLOB,n INTEGER)");
        sql(&config, "INSERT INTO source VALUES(1,'comma, newline'||char(10)||'🦀',1.5,X'00ff',NULL)");
        sql(&config, "INSERT INTO source VALUES(2,'second',2.25,X'',42)");
        sql(&config, "CREATE TABLE destination(id INTEGER PRIMARY KEY,t TEXT,r REAL,b BLOB,n INTEGER)");
        let path = temp.path().join("rows.json");
        run(&config, Action::Export { table: "source".into(), path: path.clone() }, Arc::new(AtomicBool::new(false))).unwrap();
        assert!(matches!(run(&config, Action::Export { table: "source".into(), path: path.clone() }, Arc::new(AtomicBool::new(false))), Err(Error::Exists)));
        run(&config, Action::Import { table: "destination".into(), path: path.clone() }, Arc::new(AtomicBool::new(false))).unwrap();
        assert_eq!(page(&config, "source").grid.rows, page(&config, "destination").grid.rows);
        sql(&config, "DELETE FROM destination WHERE id=1");
        assert!(run(&config, Action::Import { table: "destination".into(), path }, Arc::new(AtomicBool::new(false))).is_err());
        assert_eq!(page(&config, "destination").grid.rows.len(), 1);
    }
    #[test]
    fn sqlite_csv_round_trip_preserves_schema_affinity_null_blob_and_quoted_unicode() {
        let (temp, config) = fixture();
        sql(&config, "CREATE TABLE source(id INTEGER PRIMARY KEY,name TEXT,real_value REAL,data BLOB,n TEXT)");
        sql(&config, "CREATE TABLE target(id INTEGER PRIMARY KEY,name TEXT,real_value REAL,data BLOB,n TEXT)");
        sql(&config, "INSERT INTO source VALUES(1,'=formula, quote'||char(34)||'🦀'||char(10),1.25,X'00ff',NULL)");
        let path = temp.path().join("rows.csv");
        run(&config, Action::Export { table: "source".into(), path: path.clone() }, Arc::new(AtomicBool::new(false))).unwrap();
        run(&config, Action::Import { table: "target".into(), path }, Arc::new(AtomicBool::new(false))).unwrap();
        assert_eq!(page(&config, "source").grid.rows, page(&config, "target").grid.rows);
    }
    #[test]
    fn unsafe_sql_multiple_statements_and_long_queries_are_bounded_or_cancelled() {
        let (temp, config) = fixture();
        for statement in [
            "BEGIN",
            "SAVEPOINT x",
            "COMMIT",
            "SELECT load_extension('evil')",
            "ATTACH 'extra.sqlite' AS extra",
            "PRAGMA writable_schema=ON",
            "PRAGMA journal_mode=OFF",
            "SELECT 1; SELECT 2",
        ] {
            assert!(run(&config, Action::Query(statement.into()), Arc::new(AtomicBool::new(false))).is_err(), "{statement}");
        }
        assert!(!temp.path().join("extra.sqlite").exists());
        assert!(run(&config, Action::Query("SELECT randomblob(2000000)".into()), Arc::new(AtomicBool::new(false))).is_err());
        let cancel = Arc::new(AtomicBool::new(false));
        let stop = cancel.clone();
        let worker = std::thread::spawn(move || {
            run(&config, Action::Query("WITH RECURSIVE loop(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM loop) SELECT sum(x) FROM loop".into()), cancel)
        });
        std::thread::sleep(Duration::from_millis(20));
        stop.store(true, Ordering::Relaxed);
        assert!(matches!(worker.join().unwrap(), Err(Error::Cancelled)));
    }
}
