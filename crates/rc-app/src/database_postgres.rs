//! PostgreSQL requests run on a private worker runtime. Local Unix sockets / loopback only;
//! no URL password persistence, remote plaintext credentials, shell or project processes.
use crate::database_core::{self as db, Action, Cell, Column, Grid, Page, Reply, Table, Transfer};
pub use db::Error;
use futures_util::StreamExt;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio_postgres::{
    Client, NoTls, SimpleQueryMessage,
    types::{Format, IsNull, ToSql, Type},
};
#[cfg(test)]
pub(super) static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
#[path = "database_pg_io.rs"]
mod wire;
#[derive(Clone)]
pub struct Config {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub database: String,
    pub password: String,
    pub write: bool,
}
fn server(error: tokio_postgres::Error) -> Error {
    if error.code().is_some_and(|code| code.code() == "40001") {
        return Error::Stale;
    }
    if error.code().is_some_and(|code| code.code() == "25006") {
        return Error::ReadOnly;
    }
    let message = error
        .as_db_error()
        .map(|error| format!("{} [{}]", error.message(), error.code().code()))
        .unwrap_or_else(|| error.to_string());
    Error::Database(message.chars().take(2048).collect())
}
fn quote(name: &str) -> Result<String, Error> {
    if name.is_empty() || name.len() > 1024 || name.contains('\0') {
        return Err(Error::Input);
    }
    Ok(format!("\"{}\"", name.replace('"', "\"\"")))
}
#[derive(Debug)]
struct Parameter(Cell);
impl ToSql for Parameter {
    fn to_sql(
        &self,
        _: &Type,
        out: &mut bytes::BytesMut,
    ) -> Result<IsNull, Box<dyn std::error::Error + Send + Sync>> {
        match &self.0 {
            Cell::Null => return Ok(IsNull::Yes),
            Cell::Blob(value) => {
                out.extend_from_slice(b"\\x");
                for byte in value {
                    out.extend_from_slice(format!("{byte:02x}").as_bytes());
                }
            }
            Cell::Text(value) => out.extend_from_slice(value.as_bytes()),
            value => out.extend_from_slice(value.display().as_bytes()),
        };
        Ok(IsNull::No)
    }
    fn accepts(_: &Type) -> bool {
        true
    }
    fn encode_format(&self, _: &Type) -> Format {
        Format::Text
    }
    tokio_postgres::types::to_sql_checked!();
}
fn values(cells: &[Cell]) -> Result<Vec<Parameter>, Error> {
    if cells.len() > 1000 || cells.iter().map(Cell::size).sum::<usize>() > 2 * 1024 * 1024 {
        return Err(Error::Limit);
    }
    if cells
        .iter()
        .any(|cell| matches!(cell,Cell::Real(value)if !value.is_finite()))
    {
        return Err(Error::Input);
    }
    Ok(cells.iter().cloned().map(Parameter).collect())
}
fn cell(text: Option<&str>, kind: &Type) -> Result<Cell, Error> {
    let Some(text) = text else {
        return Ok(Cell::Null);
    };
    if text.len()
        > if *kind == Type::BYTEA {
            2 * 1024 * 1024
        } else {
            1024 * 1024
        }
    {
        return Err(Error::Limit);
    }
    Ok(match *kind {
        Type::INT2 | Type::INT4 | Type::INT8 => {
            Cell::Integer(text.parse().map_err(|_| Error::Input)?)
        }
        Type::FLOAT4 | Type::FLOAT8 => match text.parse::<f64>() {
            Ok(number) if number.is_finite() => Cell::Real(number),
            _ => Cell::Text(text.into()),
        },
        Type::BYTEA => {
            let bytes = text.strip_prefix("\\x").ok_or(Error::Input)?;
            if bytes.len() % 2 != 0 || !bytes.is_ascii() {
                return Err(Error::Input);
            }
            let value = bytes
                .as_bytes()
                .chunks_exact(2)
                .map(|pair| {
                    std::str::from_utf8(pair)
                        .ok()
                        .and_then(|pair| u8::from_str_radix(pair, 16).ok())
                        .ok_or(Error::Input)
                })
                .collect::<Result<Vec<_>, _>>()?;
            Cell::Blob(value)
        }
        _ => Cell::Text(text.into()),
    })
}
fn command(sql: &str) -> String {
    rc_syntax::sql::tokens(sql)
        .into_iter()
        .find(|token| !matches!(token.role, rc_syntax::Hl::Comment | rc_syntax::Hl::Punct))
        .and_then(|token| sql.get(token.range).map(str::to_ascii_uppercase))
        .unwrap_or_default()
}
fn changes_rows(sql: &str) -> bool {
    let mut depth = 0_i32;
    for token in rc_syntax::sql::tokens(sql) {
        let word = &sql[token.range];
        if word == "(" {
            depth += 1;
            continue;
        }
        if word == ")" {
            depth -= 1;
            continue;
        }
        if depth == 0 && matches!(token.role, rc_syntax::Hl::Keyword | rc_syntax::Hl::Field) {
            if matches!(
                word.to_ascii_uppercase().as_str(),
                "INSERT" | "UPDATE" | "DELETE" | "MERGE" | "COPY" | "TRUNCATE"
            ) {
                return true;
            }
            if matches!(
                word.to_ascii_uppercase().as_str(),
                "SELECT" | "EXPLAIN" | "SHOW" | "VALUES"
            ) {
                return false;
            }
        }
    }
    false
}
struct Rows {
    grid: Grid,
    raw: Vec<Vec<Option<String>>>,
}
async fn query(client: &Client, sql: &str, max: usize) -> Result<Rows, Error> {
    if sql.len() > 64 * 1024 {
        return Err(Error::Limit);
    }
    let prepared = client.prepare(sql).await.map_err(server)?;
    let columns = prepared.columns();
    if columns.len() > 256 {
        return Err(Error::Limit);
    }
    let mut grid = Grid {
        columns: columns
            .iter()
            .map(|column| column.name().to_string())
            .collect(),
        ..Default::default()
    };
    let stream = client.simple_query_raw(sql).await.map_err(server)?;
    futures_util::pin_mut!(stream);
    let mut raw = Vec::new();
    let mut bytes = 0_usize;
    let mutating = changes_rows(sql);
    while let Some(message) = stream.next().await {
        match message.map_err(server)? {
            SimpleQueryMessage::Row(row) => {
                if row.len() != columns.len() {
                    return Err(Error::Input);
                }
                if grid.rows.len() >= max {
                    grid.truncated = true;
                    continue;
                }
                let mut data = Vec::new();
                let mut original = Vec::new();
                for (index, column) in columns.iter().enumerate() {
                    let value = row.try_get(index).map_err(server)?;
                    let parsed = cell(value, column.type_())?;
                    bytes = bytes.saturating_add(parsed.size());
                    if bytes > 2 * 1024 * 1024 {
                        return Err(Error::Limit);
                    }
                    data.push(parsed);
                    original.push(value.map(str::to_string));
                }
                grid.rows.push(data);
                raw.push(original);
            }
            SimpleQueryMessage::CommandComplete(count) => {
                if mutating {
                    grid.affected = usize::try_from(count).map_err(|_| Error::Limit)?;
                }
            }
            _ => {}
        }
    }
    Ok(Rows { grid, raw })
}
async fn table(client: &Client, key: &str) -> Result<(Table, i64), Error> {
    if key.len() > 2048 {
        return Err(Error::Input);
    }
    let row=client.query_opt("SELECT c.oid::bigint,c.relname,n.nspname,c.relkind::text,c.xmin::text::bigint FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace WHERE pg_catalog.format('%I.%I',n.nspname,c.relname)=$1 AND c.relkind IN ('r','p','v','m') AND n.nspname NOT IN ('pg_catalog','information_schema')",&[&key]).await.map_err(server)?.ok_or(Error::Input)?;
    let oid: i64 = row.try_get(0).map_err(server)?;
    let local: String = row.try_get(1).map_err(server)?;
    let schema: String = row.try_get(2).map_err(server)?;
    let kind: String = row.try_get(3).map_err(server)?;
    let version: i64 = row.try_get(4).map_err(server)?;
    let rows=client.query("SELECT a.attname,pg_catalog.format_type(a.atttypid,a.atttypmod),coalesce(array_position(i.indkey::smallint[],a.attnum),0)::integer,(a.attgenerated<>'' OR a.attidentity='a'),left(coalesce(pg_catalog.pg_get_expr(d.adbin,d.adrelid),''),4096),length(coalesce(pg_catalog.pg_get_expr(d.adbin,d.adrelid),'')),a.attnotnull FROM pg_catalog.pg_attribute a LEFT JOIN pg_catalog.pg_index i ON i.indrelid=a.attrelid AND i.indisprimary LEFT JOIN pg_catalog.pg_attrdef d ON d.adrelid=a.attrelid AND d.adnum=a.attnum WHERE a.attrelid=$1::bigint::oid AND a.attnum>0 AND NOT a.attisdropped ORDER BY a.attnum LIMIT 257",&[&oid]).await.map_err(server)?;
    if rows.len() > 256 {
        return Err(Error::Limit);
    }
    let mut columns = Vec::new();
    let mut definition = format!(
        "-- PostgreSQL {} {}.{}\n",
        kind,
        quote(&schema)?,
        quote(&local)?
    );
    for row in rows {
        let name: String = row.try_get(0).map_err(server)?;
        let declared: String = row.try_get(1).map_err(server)?;
        let primary: i32 = row.try_get(2).map_err(server)?;
        let generated: bool = row.try_get(3).map_err(server)?;
        let default: String = row.try_get(4).map_err(server)?;
        let length: i32 = row.try_get(5).map_err(server)?;
        let required: bool = row.try_get(6).map_err(server)?;
        if length > 4096 {
            return Err(Error::Limit);
        }
        quote(&name)?;
        definition.push_str(&format!(
            "{} {} generated={} not_null={} default={}\n",
            quote(&name)?,
            declared,
            generated,
            required,
            default
        ));
        columns.push(Column {
            name,
            declared,
            primary: usize::try_from(primary.max(0)).map_err(|_| Error::Input)?,
            generated,
        });
    }
    // PostgreSQL int2vector arrays are zero-based: normalize key ordinals to one-based.
    let pk=client.query("SELECT a.attname,k.ordinality::integer FROM pg_catalog.pg_index i CROSS JOIN LATERAL unnest(i.indkey) WITH ORDINALITY k(attnum,ordinality) JOIN pg_catalog.pg_attribute a ON a.attrelid=i.indrelid AND a.attnum=k.attnum WHERE i.indrelid=$1::bigint::oid AND i.indisprimary AND k.ordinality<=i.indnkeyatts ORDER BY k.ordinality",&[&oid]).await.map_err(server)?;
    let mut primary_names = std::collections::BTreeMap::new();
    for row in pk {
        let name: String = row.try_get(0).map_err(server)?;
        let position: i32 = row.try_get(1).map_err(server)?;
        primary_names.insert(name, position.max(0) as usize);
    }
    for column in &mut columns {
        column.primary = primary_names.get(&column.name).copied().unwrap_or(0);
    }
    let rows=client.query("SELECT 'index'::text,indexname,left(indexdef,8192),length(indexdef) FROM pg_catalog.pg_indexes WHERE schemaname=$1 AND tablename=$2 UNION ALL SELECT 'trigger',t.tgname,left(pg_catalog.pg_get_triggerdef(t.oid),8192),length(pg_catalog.pg_get_triggerdef(t.oid)) FROM pg_catalog.pg_trigger t WHERE t.tgrelid=$3::bigint::oid AND NOT t.tgisinternal UNION ALL SELECT 'constraint',c.conname,left(pg_catalog.pg_get_constraintdef(c.oid,true),8192),length(pg_catalog.pg_get_constraintdef(c.oid,true)) FROM pg_catalog.pg_constraint c WHERE c.conrelid=$3::bigint::oid ORDER BY 1,2 LIMIT 129",&[&schema,&local,&oid]).await.map_err(server)?;
    if rows.len() > 128 {
        return Err(Error::Limit);
    }
    let mut structures = Vec::new();
    for row in rows {
        let length: i32 = row.try_get(3).map_err(server)?;
        if length > 8192 {
            return Err(Error::Limit);
        }
        let item = (
            row.try_get::<_, String>(0).map_err(server)?,
            row.try_get::<_, String>(1).map_err(server)?,
            row.try_get::<_, String>(2).map_err(server)?,
        );
        definition.push_str(&item.2);
        definition.push('\n');
        structures.push(item);
    }
    if matches!(kind.as_str(), "v" | "m") {
        let view=client.query_one("SELECT left(pg_catalog.pg_get_viewdef($1::bigint::oid,true),65536),length(pg_catalog.pg_get_viewdef($1::bigint::oid,true))",&[&oid]).await.map_err(server)?;
        let size: i32 = view.try_get(1).map_err(server)?;
        if size > 65536 {
            return Err(Error::Limit);
        }
        let source: String = view.try_get(0).map_err(server)?;
        definition.push_str(&source);
    }
    if definition.len() > 64 * 1024 {
        return Err(Error::Limit);
    }
    Ok((
        Table {
            local_name: Some(local),
            name: key.into(),
            kind: if matches!(kind.as_str(), "r" | "p") {
                "table".into()
            } else {
                "view".into()
            },
            sql: definition,
            columns,
            structures,
        },
        version,
    ))
}
async fn schema(client: &Client) -> Result<Vec<Table>, Error> {
    let rows=client.query("SELECT pg_catalog.format('%I.%I',n.nspname,c.relname) FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace WHERE c.relkind IN ('r','p','v','m') AND n.nspname NOT IN ('pg_catalog','information_schema') AND n.nspname !~ '^pg_toast' AND n.nspname !~ '^pg_temp' AND pg_catalog.has_table_privilege(c.oid,'SELECT') ORDER BY n.nspname,c.relname LIMIT 201",&[]).await.map_err(server)?;
    if rows.len() > 200 {
        return Err(Error::Limit);
    }
    let mut tables = Vec::new();
    let mut bytes = 0_usize;
    for row in rows {
        let key: String = row.try_get(0).map_err(server)?;
        let (meta, _) = table(client, &key).await?;
        bytes += meta.sql.len();
        if bytes > 2 * 1024 * 1024 {
            return Err(Error::Limit);
        }
        tables.push(meta);
    }
    Ok(tables)
}
async fn browse(client: &Client, key: &str, offset: usize) -> Result<Page, Error> {
    if offset > 1_000_000 {
        return Err(Error::Limit);
    }
    let (meta, version) = table(client, key).await?;
    let order = meta
        .columns
        .iter()
        .filter(|column| column.primary > 0)
        .map(|column| quote(&column.name))
        .collect::<Result<Vec<_>, _>>()?;
    let ordering = if order.is_empty() {
        String::new()
    } else {
        format!(" ORDER BY {}", order.join(","))
    };
    let rows = query(
        client,
        &format!(
            "SELECT * FROM {}{ordering} LIMIT 501 OFFSET {offset}",
            meta.name
        ),
        500,
    )
    .await?;
    let mut primary: Vec<_> = meta
        .columns
        .iter()
        .enumerate()
        .filter(|(_, column)| column.primary > 0)
        .collect();
    primary.sort_by_key(|(_, column)| column.primary);
    let key_names = if meta.kind == "table" {
        primary
            .iter()
            .map(|(_, column)| column.name.clone())
            .collect()
    } else {
        Vec::new()
    };
    let keys = rows
        .grid
        .rows
        .iter()
        .map(|row| {
            primary
                .iter()
                .filter_map(|(index, _)| row.get(*index).cloned())
                .collect()
        })
        .collect();
    Ok(Page {
        original_text: Some(rows.raw),
        table: meta,
        grid: rows.grid,
        keys,
        key_names,
        offset,
        schema_version: version,
    })
}
async fn update(
    client: &Client,
    page: &Page,
    row: usize,
    column: usize,
    value: Cell,
) -> Result<(), Error> {
    let (meta, version) = table(client, &page.table.name).await?;
    if meta.sql != page.table.sql || version != page.schema_version || page.key_names.is_empty() {
        return Err(Error::Stale);
    }
    let target = meta
        .columns
        .get(column)
        .filter(|column| !column.generated)
        .ok_or(Error::Input)?;
    let keys = page
        .keys
        .get(row)
        .filter(|keys| keys.len() == page.key_names.len())
        .ok_or(Error::Input)?;
    let original = page
        .original_text
        .as_ref()
        .and_then(|rows| rows.get(row))
        .filter(|row| row.len() == meta.columns.len())
        .ok_or(Error::Input)?;
    if value.size() > 64 * 1024 {
        return Err(Error::Limit);
    }
    let mut cells = vec![value];
    let mut predicates = Vec::new();
    for (name, key) in page.key_names.iter().zip(keys) {
        cells.push(key.clone());
        predicates.push(format!(
            "{} IS NOT DISTINCT FROM ${}",
            quote(name)?,
            cells.len()
        ));
    }
    for (column, original) in meta.columns.iter().zip(original) {
        cells.push(
            original
                .as_ref()
                .map_or(Cell::Null, |text| Cell::Text(text.clone())),
        );
        predicates.push(format!(
            "({}::text COLLATE \"C\") IS NOT DISTINCT FROM ((${}::{})::text COLLATE \"C\")",
            quote(&column.name)?,
            cells.len(),
            column.declared
        ));
    }
    let params = values(&cells)?;
    let references: Vec<&(dyn ToSql + Sync)> = params
        .iter()
        .map(|value| value as &(dyn ToSql + Sync))
        .collect();
    let affected = client
        .execute(
            &format!(
                "UPDATE {} SET {}=$1 WHERE {}",
                meta.name,
                quote(&target.name)?,
                predicates.join(" AND ")
            ),
            &references,
        )
        .await
        .map_err(server)?;
    if affected != 1 {
        return Err(Error::Stale);
    }
    Ok(())
}
async fn action(
    client: &Client,
    request: Action,
    write: bool,
    cancel: &AtomicBool,
) -> Result<Reply, Error> {
    match request {
        Action::Schema => schema(client).await.map(Reply::Schema),
        Action::Browse { table, offset } => browse(client, &table, offset).await.map(Reply::Page),
        Action::Query(sql) => {
            let first = command(&sql);
            if first.is_empty() {
                return Err(Error::Input);
            }
            if matches!(
                first.as_str(),
                "BEGIN" | "COMMIT" | "ROLLBACK" | "SAVEPOINT" | "RELEASE" | "END"
            ) {
                return Err(Error::Input);
            }
            if !write
                && !matches!(
                    first.as_str(),
                    "SELECT" | "WITH" | "VALUES" | "TABLE" | "SHOW" | "EXPLAIN"
                )
            {
                return Err(Error::ReadOnly);
            }
            query(client, &sql, 500)
                .await
                .map(|result| Reply::Query(result.grid))
        }
        Action::Update {
            page,
            row,
            column,
            value,
        } => {
            if !write {
                return Err(Error::ReadOnly);
            }
            update(client, &page, row, column, value).await?;
            Ok(Reply::Updated)
        }
        Action::Export { table: key, path } => {
            let (meta, _) = table(client, &key).await?;
            let columns: Vec<_> = meta
                .columns
                .iter()
                .filter(|column| !column.generated)
                .map(|column| column.name.clone())
                .collect();
            if columns.is_empty() {
                return Err(Error::Input);
            }
            let result = query(
                client,
                &format!(
                    "SELECT {} FROM {}",
                    columns
                        .iter()
                        .map(|name| quote(name))
                        .collect::<Result<Vec<_>, _>>()?
                        .join(","),
                    meta.name
                ),
                5000,
            )
            .await?;
            if result.grid.truncated {
                return Err(Error::Limit);
            }
            let count = db::save_transfer(
                &path,
                &Transfer {
                    format: "crabcraft.postgresql.rows.v1".into(),
                    columns,
                    rows: result.grid.rows,
                },
                cancel,
            )?;
            Ok(Reply::Exported(count))
        }
        Action::Import { table: key, path } => {
            if !write {
                return Err(Error::ReadOnly);
            }
            let transfer = db::load_transfer(&path, cancel)?;
            if !matches!(
                transfer.format.as_str(),
                "crabcraft.postgresql.rows.v1" | "crabcraft.csv.rows.v1"
            ) || transfer.rows.len() > 5000
            {
                return Err(Error::Input);
            }
            let (meta, _) = table(client, &key).await?;
            let columns: Vec<_> = meta
                .columns
                .iter()
                .filter(|column| !column.generated)
                .map(|column| column.name.clone())
                .collect();
            if columns.is_empty() || columns != transfer.columns {
                return Err(Error::Input);
            }
            let statement = client
                .prepare(&format!(
                    "INSERT INTO {} ({}) VALUES ({})",
                    meta.name,
                    columns
                        .iter()
                        .map(|name| quote(name))
                        .collect::<Result<Vec<_>, _>>()?
                        .join(","),
                    (1..=columns.len())
                        .map(|index| format!("${index}"))
                        .collect::<Vec<_>>()
                        .join(",")
                ))
                .await
                .map_err(server)?;
            let mut bytes = 0;
            for row in &transfer.rows {
                if cancel.load(Ordering::Relaxed) {
                    return Err(Error::Cancelled);
                }
                if row.len() != columns.len() {
                    return Err(Error::Input);
                }
                bytes += row.iter().map(Cell::size).sum::<usize>();
                if bytes > 2 * 1024 * 1024 {
                    return Err(Error::Limit);
                }
                let params = values(row)?;
                let references: Vec<&(dyn ToSql + Sync)> = params
                    .iter()
                    .map(|value| value as &(dyn ToSql + Sync))
                    .collect();
                client
                    .execute(&statement, &references)
                    .await
                    .map_err(server)?;
            }
            Ok(Reply::Imported(transfer.rows.len()))
        }
    }
}
pub fn run(config: &Config, request: Action, cancel: Arc<AtomicBool>) -> Result<Reply, Error> {
    if config.host.len() > 8192
        || config.user.len() > 1024
        || config.database.len() > 1024
        || config.password.len() > 4096
        || config.user.is_empty()
        || config.database.is_empty()
    {
        return Err(Error::Input);
    }
    if cancel.load(Ordering::Relaxed) {
        return Err(Error::Cancelled);
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async{
        let limited=Arc::new(AtomicBool::new(false));let setup=async{let stream=wire::Wire::open(config,limited.clone()).await?;let mut pg=tokio_postgres::Config::new();pg.user(&config.user).dbname(&config.database).password(&config.password).application_name("CrabCraft").ssl_mode(tokio_postgres::config::SslMode::Disable).options("-c statement_timeout=10000 -c lock_timeout=150 -c idle_in_transaction_session_timeout=10000 -c search_path=pg_catalog,public -c bytea_output=hex -c DateStyle=ISO -c TimeZone=UTC");pg.connect_raw(stream,NoTls).await.map_err(server)};
        let startup_cancel=async{loop{if cancel.load(Ordering::Relaxed){return;}tokio::time::sleep(Duration::from_millis(20)).await;}};let(client,connection)=tokio::select!{result=tokio::time::timeout(Duration::from_secs(3),setup)=>result.map_err(|_|Error::Timeout)??,()=startup_cancel=>return Err(Error::Cancelled)};let driver=tokio::spawn(async move{let _=connection.await;});let token=client.cancel_token();
        let work=async{client.batch_execute(if config.write{"BEGIN ISOLATION LEVEL REPEATABLE READ"}else{"BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY"}).await.map_err(server)?;let reply=action(&client,request,config.write,&cancel).await?;if cancel.load(Ordering::Relaxed){return Err(Error::Cancelled);}client.batch_execute("COMMIT").await.map_err(server)?;Ok::<_,Error>(reply)};
        let stopped=async{loop{if cancel.load(Ordering::Relaxed){break;}tokio::time::sleep(Duration::from_millis(20)).await;}};
        let result=tokio::select!{result=work=>result,()=stopped=>Err(Error::Cancelled),()=tokio::time::sleep(Duration::from_secs(12))=>Err(Error::Timeout)};
        if result.is_err(){if let Ok(Ok(stream))=tokio::time::timeout(Duration::from_millis(500),wire::Wire::open(config,Arc::new(AtomicBool::new(false)))).await{let _=tokio::time::timeout(Duration::from_secs(1),token.cancel_query_raw(stream,NoTls)).await;}}
        driver.abort();if limited.load(Ordering::Relaxed){Err(Error::Limit)}else{result}
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn config() -> Config {
        Config {
            host: "/tmp/crabcraft-postgres/socket".into(),
            port: 55432,
            user: "crabcraft".into(),
            database: "postgres".into(),
            password: String::new(),
            write: true,
        }
    }
    fn request(config: &Config, action: Action) -> Reply {
        run(config, action, Arc::new(AtomicBool::new(false)))
            .unwrap_or_else(|error| panic!("PostgreSQL: {error:?}"))
    }
    fn sql(config: &Config, sql: &str) -> Grid {
        match request(config, Action::Query(sql.into())) {
            Reply::Query(grid) => grid,
            _ => panic!("query result"),
        }
    }
    fn page(config: &Config, table: &str) -> Page {
        match request(
            config,
            Action::Browse {
                table: table.into(),
                offset: 0,
            },
        ) {
            Reply::Page(page) => page,
            _ => panic!("page result"),
        }
    }
    #[test]
    fn statement_counts_ignore_returned_select_rows_and_nested_cte_actions() {
        assert!(!changes_rows("SELECT 42"));
        assert!(!changes_rows("WITH rows AS (DELETE FROM t RETURNING *) SELECT * FROM rows"));
        assert!(changes_rows("WITH rows AS (SELECT 42) UPDATE t SET n=42"));
        assert!(changes_rows("TRUNCATE t"));
        assert!(changes_rows("MERGE INTO t USING s ON t.id=s.id WHEN MATCHED THEN DELETE"));
    }
    #[test]
    #[ignore = "Requires a real PostgreSQL server on the private /tmp/crabcraft-postgres socket"]
    fn real_postgres_schema_edit_stale_readonly_transfer_rollback_and_cancel() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let config = config();
        let name = format!(
            "crabcraft_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        sql(&config, &format!("CREATE SCHEMA {name}"));
        let people = format!("{name}.people");
        let copy = format!("{name}.copy");
        sql(
            &config,
            &format!(
                "CREATE TABLE {people}(id bigint PRIMARY KEY,name text,amount numeric(30,10),payload bytea,missing text,enabled boolean)"
            ),
        );
        sql(
            &config,
            &format!("CREATE TABLE {copy}(LIKE {people} INCLUDING ALL)"),
        );
        sql(
            &config,
            &format!(
                "INSERT INTO {people} VALUES(1,'O''Brien🦀',12345678901234567890.1234567890,'\\x00ff',NULL,true)"
            ),
        );
        let before = page(&config, &people);
        assert_eq!(before.grid.rows[0][0], Cell::Integer(1));
        assert_eq!(before.grid.rows[0][1], Cell::Text("O'Brien🦀".into()));
        assert_eq!(
            before.grid.rows[0][2],
            Cell::Text("12345678901234567890.1234567890".into())
        );
        assert_eq!(before.grid.rows[0][3], Cell::Blob(vec![0, 255]));
        assert_eq!(before.grid.rows[0][4], Cell::Null);
        request(
            &config,
            Action::Update {
                page: before.clone(),
                row: 0,
                column: 1,
                value: Cell::Text("edited 🦀".into()),
            },
        );
        assert_eq!(
            page(&config, &people).grid.rows[0][1],
            Cell::Text("edited 🦀".into())
        );
        assert!(matches!(
            run(
                &config,
                Action::Update {
                    page: before,
                    row: 0,
                    column: 1,
                    value: Cell::Text("stale".into())
                },
                Arc::new(AtomicBool::new(false))
            ),
            Err(Error::Stale)
        ));
        let mut readonly = config.clone();
        readonly.write = false;
        assert!(matches!(
            run(
                &readonly,
                Action::Query(format!("DELETE FROM {people}")),
                Arc::new(AtomicBool::new(false))
            ),
            Err(Error::ReadOnly)
        ));
        assert_eq!(page(&readonly, &people).grid.rows.len(), 1);
        let files = tempfile::tempdir().unwrap();
        let path = files.path().join("postgres.json");
        request(
            &config,
            Action::Export {
                table: people.clone(),
                path: path.clone(),
            },
        );
        request(
            &config,
            Action::Import {
                table: copy.clone(),
                path: path.clone(),
            },
        );
        assert_eq!(
            page(&config, &people).grid.rows,
            page(&config, &copy).grid.rows
        );
        assert!(
            run(
                &config,
                Action::Import {
                    table: copy.clone(),
                    path
                },
                Arc::new(AtomicBool::new(false))
            )
            .is_err()
        );
        assert_eq!(page(&config, &copy).grid.rows.len(), 1);
        sql(&config, &format!("DELETE FROM {copy}"));
        let csv = files.path().join("postgres.csv");
        request(
            &config,
            Action::Export {
                table: people.clone(),
                path: csv.clone(),
            },
        );
        request(
            &config,
            Action::Import {
                table: copy.clone(),
                path: csv,
            },
        );
        assert_eq!(
            page(&config, &people).grid.rows,
            page(&config, &copy).grid.rows
        );
        let cancel = Arc::new(AtomicBool::new(false));
        let stop = cancel.clone();
        let query_config = config.clone();
        let start = std::time::Instant::now();
        let worker = std::thread::spawn(move || {
            run(
                &query_config,
                Action::Query("SELECT pg_sleep(20)".into()),
                cancel,
            )
        });
        std::thread::sleep(Duration::from_millis(100));
        stop.store(true, Ordering::Relaxed);
        assert!(matches!(worker.join().unwrap(), Err(Error::Cancelled)));
        assert!(start.elapsed() < Duration::from_secs(3));
        assert!(matches!(
            run(
                &config,
                Action::Query("SELECT repeat('x',3000000)".into()),
                Arc::new(AtomicBool::new(false))
            ),
            Err(Error::Limit)
        ));
        sql(&config, &format!("DROP SCHEMA {name} CASCADE"));
    }
    #[test]
    #[ignore = "Requires the private PostgreSQL fixture with loopback SCRAM authentication"]
    fn real_postgres_loopback_scram_authentication_and_secret_non_persistence() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let mut config = config();
        config.host = "127.0.0.1".into();
        config.password = "CrabCraft-temporary-only-2026".into();
        config.write = false;
        let grid = sql(&config, "SELECT current_user,version()");
        assert_eq!(grid.rows[0][0], Cell::Text("crabcraft".into()));
        assert_eq!(grid.affected, 0);
        config.password = "invalid-private-test-password".into();
        let error = run(&config, Action::Schema, Arc::new(AtomicBool::new(false)))
            .err()
            .unwrap();
        let message = format!("{error:?}");
        assert!(!message.contains(&config.password));
        config.host = "203.0.113.1".into();
        assert!(matches!(
            run(&config, Action::Schema, Arc::new(AtomicBool::new(false))),
            Err(Error::Input)
        ));
    }
}
