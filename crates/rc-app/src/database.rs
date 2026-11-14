//! Native database tool window. Opening a project never opens a database or executes SQL.
use crate::database_core::{self as db, Action, Cell, Config, Grid, Page, Reply, Table};
use rc_core::{Inbox, Waker};
use rc_kit::i18n::{tr, trf};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
#[derive(Clone)]
enum Connection {
    Sqlite(Config),
    Postgres(crate::database_postgres::Config),
}
impl Connection {
    fn writes(&self) -> bool {
        match self {
            Self::Sqlite(config) => config.write,
            Self::Postgres(config) => config.write,
        }
    }
    fn existing(&mut self) {
        if let Self::Sqlite(config) = self {
            config.create = false;
        }
    }
}
struct Edit {
    row: usize,
    column: usize,
    text: String,
    kind: usize,
}
pub struct Database {
    pub path: String,
    pub sql: String,
    pub transfer: String,
    pub write: bool,
    config: Option<Connection>,
    pending: Option<Connection>,
    pub backend: usize,
    pub pg_host: String,
    pub pg_port: String,
    pub pg_user: String,
    pub pg_database: String,
    pub pg_password: String,
    pub tables: Vec<Table>,
    pub page: Option<Page>,
    grid: Option<Grid>,
    edit: Option<Edit>,
    selected: String,
    inbox: Inbox<(u64, Result<Reply, db::Error>)>,
    cancel: Option<Arc<AtomicBool>>,
    generation: u64,
    pub status: String,
    pub error: Option<String>,
}
impl Database {
    pub fn new(waker: Waker) -> Self {
        Self {
            path: String::new(),
            sql: "SELECT sqlite_version();".into(),
            transfer: String::new(),
            write: false,
            backend: 0,
            pg_host: if cfg!(unix) { "/var/run/postgresql".into() } else { "127.0.0.1".into() },
            pg_port: "5432".into(),
            pg_user: String::new(),
            pg_database: "postgres".into(),
            pg_password: String::new(),
            config: None,
            pending: None,
            tables: Vec::new(),
            page: None,
            grid: None,
            edit: None,
            selected: String::new(),
            inbox: Inbox::new(waker),
            cancel: None,
            generation: 0,
            status: String::new(),
            error: None,
        }
    }
    pub fn cancel(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        self.generation = self.generation.wrapping_add(1);
        self.pending = None;
    }
    fn disconnect(&mut self) {
        self.cancel();
        self.config = None;
        self.write = false;
        self.pg_password.clear();
        self.tables.clear();
        self.page = None;
        self.grid = None;
        self.edit = None;
        self.selected.clear();
        self.status.clear();
        self.error = None;
    }
    fn start(&mut self, mut config: Connection, action: Action, trusted: bool) {
        if !trusted {
            self.error = Some(tr("Trust the project before using database connections").into());
            return;
        }
        if self.cancel.is_some() {
            return;
        }
        self.error = None;
        self.status.clear();
        self.edit = None;
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = Some(cancel.clone());
        let out = self.inbox.sender();
        self.pending = if matches!(action, Action::Schema) {
            let mut active = config.clone();
            active.existing();
            Some(active)
        } else {
            None
        };
        if !matches!(action, Action::Schema) {
            config.existing();
        }
        std::thread::spawn(move || {
            let result = match config {
                Connection::Sqlite(config) => db::run(&config, action, cancel),
                Connection::Postgres(config) => crate::database_postgres::run(&config, action, cancel),
            };
            out.send((generation, result));
        });
    }
    fn action(&mut self, action: Action, trusted: bool) {
        if let Some(config) = self.config.clone() {
            self.start(config, action, trusted);
        }
    }
    pub fn poll(&mut self, trusted: bool) {
        if !trusted && (self.config.is_some() || self.cancel.is_some()) {
            self.disconnect();
            return;
        }
        let mut reload = false;
        for (generation, result) in self.inbox.drain() {
            if generation != self.generation {
                continue;
            }
            self.cancel = None;
            match result {
                Ok(Reply::Schema(tables)) => {
                    self.config = self.pending.take();
                    self.tables = tables;
                    self.page = None;
                    self.grid = None;
                    self.status = tr("Database connected").into();
                }
                Ok(Reply::Page(page)) => {
                    self.selected = page.table.name.clone();
                    self.page = Some(page);
                    self.grid = None;
                }
                Ok(Reply::Query(grid)) => {
                    self.status = trf("{rows} rows returned; {changes} rows changed", &[("rows", &grid.rows.len()), ("changes", &grid.affected)]);
                    self.grid = Some(grid);
                    self.page = None;
                }
                Ok(Reply::Updated) => {
                    self.status = tr("Cell updated").into();
                    reload = true;
                }
                Ok(Reply::Exported(count)) => self.status = trf("Exported {rows} rows", &[("rows", &count)]),
                Ok(Reply::Imported(count)) => {
                    self.status = trf("Imported {rows} rows", &[("rows", &count)]);
                    reload = true;
                }
                Err(error) => {
                    self.pending = None;
                    self.error = Some(error_text(error));
                }
            }
        }
        if reload && !self.selected.is_empty() {
            self.action(Action::Browse { table: self.selected.clone(), offset: self.page.as_ref().map_or(0, |page| page.offset) }, trusted);
        }
    }
    fn connect(&mut self, root: Option<&Path>, create: bool, trusted: bool) {
        let config = if self.backend == 0 {
            Connection::Sqlite(Config { path: resolve(root, &self.path), write: self.write, create })
        } else {
            let Some(port) = self.pg_port.parse::<u16>().ok().filter(|port| *port > 0) else {
                self.error = Some(error_text(db::Error::Input));
                return;
            };
            if self.sql == "SELECT sqlite_version();" {
                self.sql = "SELECT version();".into();
            }
            Connection::Postgres(crate::database_postgres::Config {
                host: self.pg_host.trim().into(),
                port,
                user: self.pg_user.clone(),
                database: self.pg_database.clone(),
                password: self.pg_password.clone(),
                write: self.write,
            })
        };
        self.start(config, Action::Schema, trusted);
    }
    fn ui(&mut self, ui: &mut egui::Ui, root: Option<&Path>, trusted: bool) {
        let busy = self.cancel.is_some();
        let connected = self.config.is_some();
        if self.path.is_empty()
            && let Some(root) = root
        {
            self.path = root.join("project.sqlite").display().to_string();
        }
        if !trusted {
            ui.colored_label(rc_kit::pal(ui).warn, tr("Trust the project before using database connections"));
        }
        ui.horizontal(|ui| {
            ui.label(tr("Database engine"));
            ui.add_enabled_ui(!busy && !connected, |ui| {
                egui::ComboBox::from_id_salt("database_engine").selected_text(if self.backend == 0 { "SQLite" } else { "PostgreSQL" }).show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.backend, 0, "SQLite");
                    ui.selectable_value(&mut self.backend, 1, "PostgreSQL");
                });
                ui.checkbox(&mut self.write, tr("Enable database writes"));
            });
            if ui.add_enabled(trusted && !busy && !connected, egui::Button::new(tr("Connect"))).clicked() {
                self.connect(root, false, trusted);
            }
            if self.backend == 0 && ui.add_enabled(trusted && !busy && !connected && self.write, egui::Button::new(tr("Create SQLite database"))).clicked() {
                self.connect(root, true, trusted);
            }
            if ui.add_enabled(connected, egui::Button::new(tr("Disconnect"))).clicked() {
                self.disconnect();
            }
        });
        ui.add_enabled_ui(!busy && !connected, |ui| {
            if self.backend == 0 {
                ui.horizontal(|ui| {
                    ui.label(tr("SQLite file"));
                    ui.add(egui::TextEdit::singleline(&mut self.path).char_limit(8192).desired_width(440.0));
                });
            } else {
                ui.horizontal_wrapped(|ui| {
                    ui.label(tr("Host"));
                    ui.add(egui::TextEdit::singleline(&mut self.pg_host).char_limit(8192).desired_width(220.0));
                    ui.label(tr("Port"));
                    ui.add(egui::TextEdit::singleline(&mut self.pg_port).char_limit(5).desired_width(55.0));
                    ui.label(tr("User"));
                    ui.add(egui::TextEdit::singleline(&mut self.pg_user).char_limit(1024).desired_width(110.0));
                    ui.label(tr("Database"));
                    ui.add(egui::TextEdit::singleline(&mut self.pg_database).char_limit(1024).desired_width(110.0));
                    ui.label(tr("Password"));
                    ui.add(egui::TextEdit::singleline(&mut self.pg_password).password(true).char_limit(4096).desired_width(120.0));
                });
                ui.small(tr("PostgreSQL: Unix socket or loopback only; remote TLS connections are not supported."));
                ui.small(tr("Database passwords stay in memory and are never saved to project settings."));
            }
        });
        if busy {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(tr("Database operation in progress…"));
                if ui.button(tr("Cancel")).clicked() {
                    self.cancel();
                    self.status = tr("Cancellation requested; a completed write may already be committed").into();
                }
            });
        }
        if let Some(error) = &self.error {
            ui.colored_label(rc_kit::pal(ui).error, error);
        }
        if !self.status.is_empty() {
            ui.label(&self.status);
        }
        if !connected {
            return;
        }
        let previous_table = self.selected.clone();
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("database_table")
                .selected_text(if self.selected.is_empty() { tr("Select a database table") } else { &self.selected })
                .show_ui(ui, |ui| {
                    for table in &self.tables {
                        ui.selectable_value(&mut self.selected, table.name.clone(), format!("{} [{}]", table.name, table.kind));
                    }
                });
            if ui.add_enabled(!busy && !self.selected.is_empty(), egui::Button::new(tr("Browse table"))).clicked() {
                self.action(Action::Browse { table: self.selected.clone(), offset: 0 }, trusted);
            }
            if ui.add_enabled(!busy, egui::Button::new(tr("Refresh schema"))).clicked() {
                self.action(Action::Schema, trusted);
            }
            if let Some(table) = self.tables.iter().find(|table| table.name == self.selected) {
                ui.menu_button(tr("Table schema"), |ui| {
                    egui::ScrollArea::vertical().max_height(200.0).show(ui, |ui| {
                        for column in &table.columns {
                            ui.monospace(format!(
                                "{}: {}{}",
                                column.name,
                                column.declared.chars().take(256).collect::<String>(),
                                if column.primary > 0 { " [PK]" } else { "" }
                            ));
                        }
                        ui.monospace(table.sql.chars().take(8192).collect::<String>());
                        for (kind, name, sql) in &table.structures {
                            ui.collapsing(format!("{kind} {name}"), |ui| {
                                ui.monospace(sql.chars().take(8192).collect::<String>());
                            });
                        }
                    });
                });
            }
        });
        if previous_table != self.selected {
            self.page = None;
            self.grid = None;
            self.edit = None;
        }
        ui.horizontal(|ui| {
            ui.label(tr("SQL console"));
            ui.small(tr("One statement per run; successful requests commit automatically. Writes require explicit opt-in."));
        });
        let mut sql_layout = |ui: &egui::Ui, text: &dyn egui::TextBuffer, width: f32| crate::sql_injection::layout(ui, text.as_str(), width);
        ui.add(
            egui::TextEdit::multiline(&mut self.sql).layouter(&mut sql_layout).code_editor().desired_rows(3).char_limit(64 * 1024).desired_width(f32::INFINITY),
        );
        if ui.add_enabled(!busy && !self.sql.trim().is_empty(), egui::Button::new(tr("Run SQL"))).clicked() {
            self.action(Action::Query(self.sql.clone()), trusted);
        }
        let mut edit = None;
        let mut page_action = None;
        if let Some(page) = &self.page {
            ui.horizontal(|ui| {
                if ui.add_enabled(!busy && page.offset >= 500, egui::Button::new(tr("Previous page"))).clicked() {
                    page_action = Some(page.offset - 500);
                }
                ui.label(trf("Rows {start}–{end}", &[("start", &(page.offset + 1)), ("end", &(page.offset + page.grid.rows.len()))]));
                if ui.add_enabled(!busy && page.grid.truncated, egui::Button::new(tr("Next page"))).clicked() {
                    page_action = Some(page.offset + 500);
                }
            });
        }
        let grid = self.page.as_ref().map(|page| &page.grid).or(self.grid.as_ref());
        if let Some(grid) = grid {
            if grid.truncated {
                ui.small(tr("Results truncated at 500 rows; use table pages or a narrower query"));
            }
            let editable = self.config.as_ref().is_some_and(Connection::writes) && self.page.as_ref().is_some_and(|page| !page.key_names.is_empty());
            egui::ScrollArea::both().id_salt("database_results").max_height(190.0).show_rows(ui, 24.0, grid.rows.len() + 1, |ui, range| {
                egui::Grid::new("database_cells").striped(true).show(ui, |ui| {
                    for index in range {
                        if index == 0 {
                            for name in &grid.columns {
                                ui.strong(name);
                            }
                            ui.end_row();
                            continue;
                        }
                        let row = index - 1;
                        for (column, value) in grid.rows[row].iter().enumerate() {
                            let enabled = editable
                                && !busy
                                && value_size(value) <= 64 * 1024
                                && self.page.as_ref().is_some_and(|page| page.table.columns.get(column).is_some_and(|column| !column.generated));
                            let label = format!("{}: {}", grid.columns.get(column).map_or("", String::as_str), preview(value));
                            if ui.add_enabled(enabled, egui::Button::new(label).small()).clicked() {
                                edit = Some(Edit { row, column, text: value.display(), kind: cell_kind(value) });
                            }
                        }
                        ui.end_row();
                    }
                });
            });
        }
        if let Some(edit) = edit {
            self.edit = Some(edit);
        }
        if let Some(offset) = page_action {
            self.action(Action::Browse { table: self.selected.clone(), offset }, trusted);
        }
        if let Some(edit) = &mut self.edit {
            let mut apply = false;
            let mut close = false;
            ui.horizontal(|ui| {
                ui.label(tr("Edit database cell"));
                egui::ComboBox::from_id_salt("database_cell_type").selected_text(["NULL", "INTEGER", "REAL", "TEXT", "BLOB"][edit.kind]).show_ui(ui, |ui| {
                    for (index, kind) in ["NULL", "INTEGER", "REAL", "TEXT", "BLOB"].iter().enumerate() {
                        ui.selectable_value(&mut edit.kind, index, *kind);
                    }
                });
                ui.add(egui::TextEdit::singleline(&mut edit.text).hint_text(tr("Cell value")).char_limit(64 * 1024));
                apply = ui.button(tr("Apply cell edit")).clicked();
                close = ui.button(tr("Cancel cell edit")).clicked();
            });
            if close {
                self.edit = None;
            } else if apply {
                let value = parse_cell(edit.kind, &edit.text);
                if let (Ok(value), Some(page)) = (value, self.page.clone()) {
                    let (row, column) = (edit.row, edit.column);
                    self.action(Action::Update { page, row, column, value }, trusted);
                } else {
                    self.error = Some(tr("Enter a valid value for the selected data type; BLOB uses hexadecimal").into());
                }
            }
        }
        ui.horizontal(|ui| {
            ui.label(tr("Transfer file (.json or .csv)"));
            ui.add(egui::TextEdit::singleline(&mut self.transfer).char_limit(8192).desired_width(300.0));
            let path = resolve(root, &self.transfer);
            let enabled = !busy && !self.selected.is_empty() && !self.transfer.trim().is_empty();
            if ui.add_enabled(enabled, egui::Button::new(tr("Export table"))).clicked() {
                self.action(Action::Export { table: self.selected.clone(), path: path.clone() }, trusted);
            }
            if ui.add_enabled(enabled && self.write, egui::Button::new(tr("Import into table"))).clicked() {
                self.action(Action::Import { table: self.selected.clone(), path }, trusted);
            }
        });
        ui.small(tr("Exports never overwrite files. Imports append rows atomically and require matching writable columns."));
        ui.small(tr("CSV uses explicit NULL and binary markers; JSON preserves exact SQLite value types."));
    }
}
impl Drop for Database {
    fn drop(&mut self) {
        self.cancel();
    }
}
fn resolve(root: Option<&Path>, value: &str) -> PathBuf {
    let path = PathBuf::from(value.trim());
    if path.is_absolute() { path } else { root.map_or(path.clone(), |root| root.join(path)) }
}
fn value_size(value: &Cell) -> usize {
    match value {
        Cell::Text(value) => value.len(),
        Cell::Blob(value) => value.len(),
        _ => 16,
    }
}
fn cell_kind(value: &Cell) -> usize {
    match value {
        Cell::Null => 0,
        Cell::Integer(_) => 1,
        Cell::Real(_) => 2,
        Cell::Text(_) => 3,
        Cell::Blob(_) => 4,
    }
}
fn preview(value: &Cell) -> String {
    match value {
        Cell::Text(value) => value.chars().take(128).collect(),
        Cell::Blob(value) => value.iter().take(64).map(|byte| format!("{byte:02x}")).collect(),
        _ => value.display(),
    }
}
fn parse_cell(kind: usize, text: &str) -> Result<Cell, ()> {
    match kind {
        0 => Ok(Cell::Null),
        1 => text.parse().map(Cell::Integer).map_err(|_| ()),
        2 => text.parse::<f64>().ok().filter(|value| value.is_finite()).map(Cell::Real).ok_or(()),
        3 => Ok(Cell::Text(text.into())),
        4 => {
            if text.len() % 2 != 0 || !text.is_ascii() {
                return Err(());
            }
            text.as_bytes()
                .chunks_exact(2)
                .map(|pair| std::str::from_utf8(pair).ok().and_then(|pair| u8::from_str_radix(pair, 16).ok()).ok_or(()))
                .collect::<Result<Vec<_>, _>>()
                .map(Cell::Blob)
        }
        _ => Err(()),
    }
}
fn error_text(error: db::Error) -> String {
    match error {
        db::Error::Cancelled => tr("Cancelled").into(),
        db::Error::Timeout => tr("Database connection or statement timed out").into(),
        db::Error::Limit => tr("Database operation exceeded its input, result or transfer limit").into(),
        db::Error::Input => tr("Invalid database input or unsupported SQL statement").into(),
        db::Error::ReadOnly => tr("Reconnect with database writes enabled before changing data").into(),
        db::Error::Stale => tr("The database row or schema changed; refresh before editing").into(),
        db::Error::Path => tr("Choose a regular input file and an explicit transfer path").into(),
        db::Error::Exists => tr("The destination already exists; choose a new file").into(),
        db::Error::Database(error) | db::Error::Io(error) => trf("Database error: {error}", &[("error", &error)]),
    }
}
pub fn show(app: &mut crate::app::CrabCraftApp, ui: &mut egui::Ui) {
    if crate::ui::toolwindows::header(ui, tr("Database")) {
        app.toggle_tool(crate::toolwindow::ToolWindow::Database);
    }
    let root = app.workspace.as_ref().map(|workspace| workspace.ws.root.clone());
    let trusted = root.as_ref().is_some_and(|root| app.settings.trusted_projects.contains(root));
    app.database.ui(ui, root.as_deref(), trusted);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::{HOME_LOCK, harness, test_app};
    use egui_kittest::kittest::Queryable;
    use std::time::{Duration, Instant};
    #[track_caller]
    fn pump(h: &mut egui_kittest::Harness<'static, crate::app::CrabCraftApp>, done: impl Fn(&Database) -> bool) {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(20) {
            h.run_steps(2);
            if done(&h.state().database) {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!(
            "database UI timeout: error={:?} status={:?} connected={} busy={} tables={:?} selected={:?}",
            h.state().database.error,
            h.state().database.status,
            h.state().database.config.is_some(),
            h.state().database.cancel.is_some(),
            h.state().database.tables.iter().map(|table| &table.name).collect::<Vec<_>>(),
            h.state().database.selected
        );
    }
    #[test]
    fn native_sqlite_connect_console_edit_transfer_cancel_and_real_rust_consumer() {
        let _lock = HOME_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let home = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let root = project.path().canonicalize().unwrap();
        std::fs::create_dir(root.join("src")).unwrap();
        std::fs::write(root.join("Cargo.toml"),"[package]\nname=\"native_sqlite_consumer\"\nversion=\"0.1.0\"\nedition=\"2021\"\n[dependencies]\nrusqlite={version=\"0.40.2\",features=[\"bundled\"]}\n").unwrap();
        std::fs::write(root.join("src/main.rs"),r#"fn main(){let db=rusqlite::Connection::open(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("records.sqlite")).unwrap();let name:String=db.query_row("SELECT name FROM people WHERE id=1",[],|row|row.get(0)).unwrap();assert_eq!(name,"changed 🦀");let copied:i64=db.query_row("SELECT count(*) FROM copy",[],|row|row.get(0)).unwrap();assert_eq!(copied,1);}"#).unwrap();
        std::fs::write(root.join("build.rs"), "fn main(){std::fs::write(\"build-ran\",\"yes\").unwrap();}").unwrap();
        let dbpath = root.join("records.sqlite");
        let connection = rusqlite::Connection::open(&dbpath).unwrap();
        connection.execute_batch("CREATE TABLE people(id INTEGER PRIMARY KEY,name TEXT); INSERT INTO people VALUES(1,'initial'); CREATE TABLE copy(id INTEGER PRIMARY KEY,name TEXT);").unwrap();
        drop(connection);
        let mut app = test_app(home.path(), true);
        app.open_workspace_and_wait(&root);
        app.dock.show(crate::toolwindow::ToolWindow::Database);
        app.dock.sizes.insert(crate::toolwindow::ToolWindow::Database, 650.0);
        app.database.path = dbpath.display().to_string();
        let mut h = harness(app, egui::vec2(1440.0, 1200.0));
        h.get_by_label(tr("Connect")).click();
        h.run_steps(2);
        assert!(h.state().database.config.is_none());
        assert!(!root.join("build-ran").exists());
        // Explicit test authorization permits DB operations without starting a language server.
        h.state_mut().settings.trusted_projects.push(root.clone());
        h.state_mut().database.write = true;
        h.run_steps(2);
        h.get_by_label(tr("Connect")).click();
        h.run_steps(2);
        pump(&mut h, |db| db.config.is_some() && db.tables.len() == 2);
        h.state_mut().database.selected = "people".into();
        h.run_steps(2);
        h.get_by_label(tr("Browse table")).click();
        h.run_steps(2);
        pump(&mut h, |db| db.page.is_some());
        h.get_by_label("name: initial").click();
        h.run_steps(2);
        h.state_mut().database.edit.as_mut().unwrap().text = "changed 🦀".into();
        h.get_by_label(tr("Apply cell edit")).click();
        h.run_steps(2);
        pump(&mut h, |db| db.cancel.is_none() && db.page.as_ref().is_some_and(|page| page.grid.rows[0][1] == Cell::Text("changed 🦀".into())));
        let external = rusqlite::Connection::open(&dbpath).unwrap();
        external.execute("UPDATE people SET name='external'", []).unwrap();
        h.get_by_label("name: changed 🦀").click();
        h.run_steps(2);
        h.state_mut().database.edit.as_mut().unwrap().text = "stale overwrite".into();
        h.get_by_label(tr("Apply cell edit")).click();
        h.run_steps(2);
        pump(&mut h, |db| db.error.is_some());
        assert_eq!(external.query_row("SELECT name FROM people", [], |row| row.get::<_, String>(0)).unwrap(), "external");
        drop(external);
        h.state_mut().database.sql = "UPDATE people SET name='changed 🦀' WHERE id=1".into();
        h.get_by_label(tr("Run SQL")).click();
        h.run_steps(2);
        pump(&mut h, |db| db.grid.is_some() && db.cancel.is_none());
        assert_eq!(h.state().database.grid.as_ref().unwrap().affected, 1);
        h.state_mut().database.transfer = root.join("rows.json").display().to_string();
        h.get_by_label(tr("Export table")).click();
        h.run_steps(2);
        pump(&mut h, |db| db.status.contains('1') && db.cancel.is_none());
        assert!(root.join("rows.json").is_file());
        h.state_mut().database.selected = "copy".into();
        h.get_by_label(tr("Import into table")).click();
        h.run_steps(2);
        pump(&mut h, |db| db.cancel.is_none() && db.page.as_ref().is_some_and(|page| page.table.name == "copy" && page.grid.rows.len() == 1));
        h.state_mut().database.sql = "WITH RECURSIVE loop(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM loop) SELECT sum(x) FROM loop".into();
        h.get_by_label(tr("Run SQL")).click();
        h.run_steps(2);
        assert!(h.state().database.cancel.is_some());
        h.get_by_label(tr("Cancel")).click();
        h.run_steps(2);
        assert!(h.state().database.cancel.is_none());
        assert!(!root.join("build-ran").exists());
        let output =
            std::process::Command::new("cargo").args(["run", "--offline", "--quiet"]).current_dir(&root).env("CARGO_BUILD_JOBS", "2").output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        assert!(root.join("build-ran").exists());
    }
    #[test]
    #[ignore = "Requires the real private PostgreSQL server; exercises native UI and builds a Rust consumer offline"]
    fn native_postgres_connection_edit_csv_and_rust_consumer() {
        let _home = HOME_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let _pg = crate::database_postgres::TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let config = crate::database_postgres::Config {
            host: "/tmp/crabcraft-postgres/socket".into(),
            port: 55432,
            user: "crabcraft".into(),
            database: "postgres".into(),
            password: String::new(),
            write: true,
        };
        let query = |sql: &str| {
            crate::database_postgres::run(&config, Action::Query(sql.into()), Arc::new(AtomicBool::new(false)))
                .unwrap_or_else(|error| panic!("PG setup: {error:?}"))
        };
        let namespace =
            format!("crabcraft_native_{}_{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());
        query(&format!("CREATE SCHEMA {namespace}"));
        let people = format!("{namespace}.people");
        let copy = format!("{namespace}.copy");
        query(&format!("CREATE TABLE {people}(id bigint PRIMARY KEY,name text,payload bytea)"));
        query(&format!("CREATE TABLE {copy}(LIKE {people} INCLUDING ALL)"));
        query(&format!("INSERT INTO {people} VALUES(1,'initial PG','\\x00ff')"));
        let home = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let root = project.path().canonicalize().unwrap();
        std::fs::create_dir(root.join("src")).unwrap();
        std::fs::write(root.join("Cargo.toml"),"[package]\nname=\"postgres_ui_evidence\"\nversion=\"0.1.0\"\nedition=\"2021\"\n[dependencies]\ntokio-postgres=\"0.7.18\"\ntokio={version=\"1\",features=[\"rt\",\"macros\",\"net\"]}\n").unwrap();
        let program = format!(
            r#"#[tokio::main(flavor="current_thread")] async fn main(){{let mut cfg=tokio_postgres::Config::new();cfg.host_path("/tmp/crabcraft-postgres/socket").port(55432).user("crabcraft").dbname("postgres");let(client,connection)=cfg.connect(tokio_postgres::NoTls).await.unwrap();tokio::spawn(async move{{connection.await.unwrap();}});let row=client.query_one("SELECT name FROM {people} WHERE id=1",&[]).await.unwrap();assert_eq!(row.get::<_,String>(0),"changed PostgreSQL 🦀");let count=client.query_one("SELECT count(*) FROM {copy}",&[]).await.unwrap();assert_eq!(count.get::<_,i64>(0),1);}}"#
        );
        std::fs::write(root.join("src/main.rs"), program).unwrap();
        std::fs::write(root.join("build.rs"), "fn main(){std::fs::write(\"build-ran\",\"yes\").unwrap();}").unwrap();
        let mut app = test_app(home.path(), true);
        app.open_workspace_and_wait(&root);
        app.dock.show(crate::toolwindow::ToolWindow::Database);
        app.dock.sizes.insert(crate::toolwindow::ToolWindow::Database, 700.0);
        app.database.backend = 1;
        app.database.pg_host = config.host.clone();
        app.database.pg_port = config.port.to_string();
        app.database.pg_user = config.user.clone();
        app.database.pg_database = config.database.clone();
        let mut h = harness(app, egui::vec2(1500.0, 1300.0));
        h.get_by_label(tr("Connect")).click();
        h.run_steps(2);
        assert!(h.state().database.config.is_none());
        h.state_mut().settings.trusted_projects.push(root.clone());
        h.run_steps(2);
        h.get_by_label(tr("Connect")).click();
        h.run_steps(2);
        pump(&mut h, |db| db.config.is_some() && db.tables.iter().any(|table| table.name == people));
        h.state_mut().database.selected = people.clone();
        h.run_steps(2);
        h.get_by_label(tr("Browse table")).click();
        h.run_steps(2);
        pump(&mut h, |db| db.page.is_some());
        h.get_by_label("name: initial PG").click();
        h.run_steps(2);
        assert!(h.state().database.edit.is_none());
        h.state_mut().database.sql = format!("DELETE FROM {people}");
        h.get_by_label(tr("Run SQL")).click();
        h.run_steps(2);
        pump(&mut h, |db| db.error.is_some());
        assert_eq!(h.state().database.page.as_ref().unwrap().grid.rows.len(), 1);
        h.get_by_label(tr("Disconnect")).click();
        h.run_steps(2);
        h.state_mut().database.write = true;
        h.get_by_label(tr("Connect")).click();
        h.run_steps(2);
        pump(&mut h, |db| db.config.is_some());
        h.state_mut().database.selected = people.clone();
        h.run_steps(2);
        h.get_by_label(tr("Browse table")).click();
        h.run_steps(2);
        pump(&mut h, |db| db.page.is_some());
        h.get_by_label("name: initial PG").click();
        h.run_steps(2);
        h.state_mut().database.edit.as_mut().unwrap().text = "changed PostgreSQL 🦀".into();
        h.get_by_label(tr("Apply cell edit")).click();
        h.run_steps(2);
        pump(&mut h, |db| db.cancel.is_none() && db.page.as_ref().is_some_and(|page| page.grid.rows[0][1] == Cell::Text("changed PostgreSQL 🦀".into())));
        h.state_mut().database.transfer = root.join("postgres.csv").display().to_string();
        h.get_by_label(tr("Export table")).click();
        h.run_steps(2);
        pump(&mut h, |db| db.cancel.is_none() && db.status.contains('1'));
        assert!(root.join("postgres.csv").exists());
        h.state_mut().database.selected = copy.clone();
        h.run_steps(2);
        h.get_by_label(tr("Import into table")).click();
        h.run_steps(2);
        pump(&mut h, |db| db.cancel.is_none() && db.page.as_ref().is_some_and(|page| page.table.name == copy && page.grid.rows.len() == 1));
        assert!(!root.join("build-ran").exists());
        let output =
            std::process::Command::new("cargo").args(["run", "--offline", "--quiet"]).current_dir(&root).env("CARGO_BUILD_JOBS", "2").output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        query(&format!("DROP SCHEMA {namespace} CASCADE"));
    }
}
