//! Local SQL colors/completion/extraction in Rust literals. No query is executed here.
use crate::docs::Doc;
use rc_editor::{Completion, CompletionItem};
use rc_kit::{i18n::tr, ico};
use rc_syntax::{
    Lang,
    sql::{self, Injection},
};

pub fn current(doc: &Doc) -> Option<Injection> {
    if doc.editor.lang() != Lang::Rust || !doc.editor.syntax.is_current() {
        return None;
    }
    let rope = doc.editor.buffer.rope();
    sql::at(
        doc.editor.syntax.tree()?,
        rope,
        rope.char_to_byte(doc.editor.caret()),
    )
}
pub fn complete(doc: &mut Doc, tables: &[crate::database_core::Table]) -> bool {
    let Some(injection) = current(doc) else {
        return false;
    };
    if doc.editor.read_only {
        doc.editor.popups.completion = None;
        return true;
    }
    let rope = doc.editor.buffer.rope();
    let caret = doc.editor.caret();
    let byte = rope.char_to_byte(caret);
    let Some(offset) = injection.sql_offset(byte) else {
        return true;
    };
    if sql::tokens(&injection.sql).iter().any(|token| {
        token.range.start < offset
            && offset < token.range.end
            && matches!(token.role, rc_syntax::Hl::String | rc_syntax::Hl::Comment | rc_syntax::Hl::Property)
    }) {
        doc.editor.popups.completion = None;
        return true;
    }
    let before = &injection.sql[..offset];
    let start = before
        .rfind(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'))
        .map_or(0, |position| position + 1);
    let prefix = &before[start..];
    let source_start = if start == offset {
        byte
    } else {
        injection
            .source_range(start..offset)
            .map_or(byte, |range| range.start)
    };
    let source_prefix = rope.byte_slice(source_start..byte).to_string();
    if source_prefix != prefix {
        doc.editor.popups.completion = None;
        return true;
    }
    let anchor = rope.byte_to_char(source_start);
    let mut choices: Vec<(String, String, bool, Option<String>)> = sql::KEYWORDS
        .iter()
        .map(|word| (word.to_string(), tr("SQL keyword").into(), true, None))
        .collect();
    let typed = prefix.to_ascii_lowercase();
    let dotted=before[..start].ends_with('.');
    let qualifier=before[..start].strip_suffix('.').and_then(|text|text.rsplit(|ch:char|!(ch.is_ascii_alphanumeric()||ch=='_')).next()).filter(|text|!text.is_empty()).map(|text|format!("{text}."));
    'tables: for table in tables.iter().take(200) {
        let label=table.local_name.as_ref().unwrap_or(&table.name);
        let insert=if table.local_name.is_some(){if dotted{qualifier.as_ref().and_then(|prefix|table.name.strip_prefix(prefix)).map(str::to_string)}else{Some(table.name.clone())}}else{None};
        if label.to_ascii_lowercase().starts_with(&typed)&&(!dotted||insert.is_some()) {
            choices.push((label.clone(),format!("{}: {}",tr("Connected database table"),table.name),false,insert));
        }
        for column in table.columns.iter().take(256) {
            if column.name.to_ascii_lowercase().starts_with(&typed) {
                choices.push((
                    column.name.clone(),
                    tr("Connected database column").into(),
                    false,
                    None,
                ));
            }
            if choices.len() >= 512 {
                break 'tables;
            }
        }
    }
    choices.sort_by(|a, b| a.0.cmp(&b.0));
    choices.dedup_by(|a, b| a.0 == b.0 && a.2 == b.2 && a.3 == b.3);
    let prefix_lower = prefix.to_ascii_lowercase();
    let items = choices
        .into_iter()
        .filter(|(name, _, _, _)| name.to_ascii_lowercase().starts_with(&prefix_lower))
        .take(200)
        .filter_map(|(label, detail, keyword, qualified)| {
            let reserved = sql::KEYWORDS
                .iter()
                .any(|word| word.eq_ignore_ascii_case(&label));
            let plain = !reserved
                && label
                    .chars()
                    .next()
                    .is_some_and(|ch| ch.is_ascii_alphabetic() || ch == '_')
                && label
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || ch == '_');
            let inserted = if let Some(qualified)=qualified { qualified } else if keyword || plain {
                label.clone()
            } else {
                format!("\"{}\"", label.replace('"', "\"\""))
            };
            let insert = sql::encode(&injection, rope, &inserted)?;
            Some(CompletionItem {
                label,
                detail,
                type_text: String::new(),
                icon: if keyword {
                    ico::CONSTANT
                } else {
                    ico::PROPERTY
                },
                insert,
                replace: Some((anchor, caret)),
                deprecated: false,
                data: usize::MAX,
                doc: None,
            })
        })
        .collect();
    let mut completion = Completion::new(anchor, items, false);
    completion.filter(prefix);
    doc.editor.popups.completion = Some(completion);
    true
}
pub fn open_console(app: &mut crate::app::CrabCraftApp, index: usize) -> bool {
    let Some(injection) = app.docs.list.get(index).and_then(current) else {
        return false;
    };
    app.database.sql = injection.sql;
    app.dock.show(crate::toolwindow::ToolWindow::Database);
    app.session_dirty = true;
    app.notify(
        tr("SQL copied to the database console; review and run explicitly"),
        false,
    );
    true
}
pub fn layout(ui: &egui::Ui, text: &str, wrap_width: f32) -> std::sync::Arc<egui::Galley> {
    let scheme = rc_kit::theme::scheme(ui.visuals().dark_mode);
    let mut job = egui::text::LayoutJob::default();
    let mut end = 0;
    for token in sql::tokens(text) {
        if token.range.start > end {
            job.append(
                &text[end..token.range.start],
                0.0,
                rc_editor::layout::format_for(rc_syntax::Hl::Plain, &scheme, 13.0),
            );
        }
        job.append(
            &text[token.range.clone()],
            0.0,
            rc_editor::layout::format_for(token.role, &scheme, 13.0),
        );
        end = token.range.end;
    }
    if end < text.len() {
        job.append(
            &text[end..],
            0.0,
            rc_editor::layout::format_for(rc_syntax::Hl::Plain, &scheme, 13.0),
        );
    }
    job.wrap.max_width = wrap_width;
    ui.fonts_mut(|fonts| fonts.layout_job(job))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::{HOME_LOCK, harness, test_app};
    use egui_kittest::kittest::Queryable;
    use std::time::{Duration, Instant};
    #[test]
    fn sql_in_rust_raw_and_escaped_literals_highlights_completes_and_opens_without_execution() {
        let _lock = HOME_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let home = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let root = project.path().canonicalize().unwrap();
        std::fs::create_dir(root.join("src")).unwrap();
        std::fs::write(root.join("Cargo.toml"),"[package]\nname=\"sql_injection_evidence\"\nversion=\"0.1.0\"\nedition=\"2021\"\n[dependencies]\nrusqlite={version=\"0.40.2\",features=[\"bundled\"]}\n").unwrap();
        let source = r###"// Unicode 🦀 precedes byte offsets.
// language=SQL
const QUERY:&str=r##"SELECT name FROM peo WHERE id=?1"##;
fn main(){
 let connection=rusqlite::Connection::open(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("records.sqlite")).unwrap();
 let name:String=connection.query_row(QUERY,[1],|row|row.get(0)).unwrap(); assert_eq!(name,"Ada");
 let sql="SELECT \"name\"\nFROM people WHERE id=?1";
 let other:String=connection.query_row(sql,[1],|row|row.get(0)).unwrap();assert_eq!(other,"Ada");
}
"###;
        let path = root.join("src/main.rs");
        std::fs::write(&path, source).unwrap();
        std::fs::write(
            root.join("build.rs"),
            "fn main(){std::fs::write(\"build-ran\",\"yes\").unwrap();}",
        )
        .unwrap();
        let dbpath = root.join("records.sqlite");
        let connection = rusqlite::Connection::open(&dbpath).unwrap();
        connection.execute_batch("CREATE TABLE people(id INTEGER PRIMARY KEY,name TEXT);INSERT INTO people VALUES(1,'Ada');").unwrap();
        drop(connection);
        let mut app = test_app(home.path(), true);
        app.open_workspace(&root);
        app.settings.trusted_projects.push(root.clone());
        app.open_file(&path);
        let at = source.find("peo WHERE").unwrap() + 3;
        let doc = app.docs.active_doc_mut().unwrap();
        doc.editor
            .set_selection(rc_text::Selection::point(source[..at].chars().count()));
        doc.editor.parse_blocking();
        app.database.path = dbpath.display().to_string();
        app.dock.show(crate::toolwindow::ToolWindow::Database);
        app.dock
            .sizes
            .insert(crate::toolwindow::ToolWindow::Database, 480.0);
        let mut h = harness(app, egui::vec2(1440.0, 1100.0));
        h.get_by_label(tr("Connect")).click();
        h.run_steps(2);
        let start = Instant::now();
        while h.state().database.tables.is_empty() && start.elapsed() < Duration::from_secs(10) {
            h.run_steps(2);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(h.state().database.tables[0].name, "people");
        let doc = h.state().docs.active_doc().unwrap();
        let injection = current(doc).unwrap();
        assert_eq!(injection.sql, "SELECT name FROM peo WHERE id=?1");
        let rope = doc.editor.buffer.rope();
        let tree = doc.editor.syntax.tree().unwrap();
        let spans = rc_syntax::highlight_lines(Lang::Rust, tree, rope, 0, rope.len_lines());
        assert!(
            spans
                .iter()
                .flatten()
                .any(|span| span.hl == rc_syntax::Hl::SqlKeyword)
        );
        h.state_mut()
            .run_insight_action(rc_editor::Action::Complete, egui::Pos2::ZERO);
        let popup = h
            .state()
            .docs
            .active_doc()
            .unwrap()
            .editor
            .popups
            .completion
            .as_ref()
            .unwrap();
        assert_eq!(popup.selected_item().unwrap().label, "people");
        h.state_mut().focus_editor = true;
        h.run_steps(2);
        h.key_press(egui::Key::Enter);
        h.run_steps(3);
        let text = h
            .state()
            .docs
            .active_doc()
            .unwrap()
            .editor
            .buffer
            .rope()
            .to_string();
        assert!(text.contains("r##\"SELECT name FROM people WHERE id=?1\"##"));
        assert!(text.contains("SELECT \\\"name\\\"\\nFROM people"));
        let index = h.state().docs.active.unwrap();
        {
            let app = h.state_mut();
            app.docs.list[index].editor.parse_blocking();
            assert!(open_console(app, index));
        }
        h.run_steps(2);
        assert_eq!(
            h.state().database.sql,
            "SELECT name FROM people WHERE id=?1"
        );
        assert!(h.state().database.page.is_none());
        assert!(!root.join("build-ran").exists());
        h.state_mut().docs.save(index).unwrap();
        let output = std::process::Command::new("cargo")
            .args(["run", "--offline", "--quiet"])
            .current_dir(&root)
            .env("CARGO_BUILD_JOBS", "2")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
