//! Independent trusted web sessions alongside rust-analyzer. No package discovery/install.
use crate::{
    docs::{Doc, Docs},
    insight::{Insight, InsightOutput, NavKind},
};
use rc_core::{Waker, settings::Settings};
use rc_editor::keymap::Action;
use rc_kit::i18n::{tr, trf};
use rc_lsp::web::Kind;
use rc_syntax::Lang;
use std::path::Path;
pub fn kind(lang: Lang) -> Option<Kind> {
    match lang {
        Lang::Html => Some(Kind::Html),
        Lang::Css => Some(Kind::Css),
        Lang::JavaScript | Lang::TypeScript | Lang::Tsx => Some(Kind::TypeScript),
        _ => None,
    }
}
pub fn language_id(lang: Lang) -> Option<&'static str> {
    match lang {
        Lang::Html => Some("html"),
        Lang::Css => Some("css"),
        Lang::JavaScript => Some("javascript"),
        Lang::TypeScript => Some("typescript"),
        Lang::Tsx => Some("typescriptreact"),
        _ => None,
    }
}
pub struct WebLanguages {
    pub html: Insight,
    pub css: Insight,
    pub typescript: Insight,
    configured: Vec<Option<rc_lsp::web::Settings>>,
    root: Option<std::path::PathBuf>,
}
impl WebLanguages {
    pub fn new(waker: Waker) -> Self {
        Self {
            html: Insight::new(waker.clone()),
            css: Insight::new(waker.clone()),
            typescript: Insight::new(waker),
            configured: vec![None, None, None],
            root: None,
        }
    }
    pub fn initializing(&self) -> bool {
        [&self.html, &self.css, &self.typescript]
            .iter()
            .any(|session| {
                matches!(
                    session.status,
                    crate::insight::InsightStatus::Starting
                        | crate::insight::InsightStatus::Loading(_)
                )
            })
    }
    pub fn stop(&mut self) {
        self.html.stop();
        self.css.stop();
        self.typescript.stop();
        self.configured = vec![None, None, None];
        self.root = None;
    }
    pub fn insight(&mut self, doc: &Doc) -> Option<&mut Insight> {
        match kind(doc.editor.lang())? {
            Kind::Html => Some(&mut self.html),
            Kind::Css => Some(&mut self.css),
            Kind::TypeScript => Some(&mut self.typescript),
        }
    }
    pub fn update(
        &mut self,
        docs: &mut Docs,
        root: Option<&Path>,
        settings: &Settings,
        trusted: bool,
    ) -> Vec<InsightOutput> {
        if !trusted || root.is_none() {
            if self.root.is_some() {
                self.stop();
            }
            return Vec::new();
        }
        let Some(root) = root else { return Vec::new() };
        if self.root.as_deref() != Some(root) {
            self.stop();
            self.root = Some(root.into());
        }
        let configs = [
            settings
                .web_html_server
                .as_ref()
                .map(|program| rc_lsp::web::Settings {
                    kind: Kind::Html,
                    program: program.clone(),
                    tsserver: None,
                }),
            settings
                .web_css_server
                .as_ref()
                .map(|program| rc_lsp::web::Settings {
                    kind: Kind::Css,
                    program: program.clone(),
                    tsserver: None,
                }),
            settings
                .web_typescript_server
                .as_ref()
                .map(|program| rc_lsp::web::Settings {
                    kind: Kind::TypeScript,
                    program: program.clone(),
                    tsserver: settings.web_tsserver_path.clone(),
                }),
        ];
        let mut outputs = Vec::new();
        for (index, session) in [&mut self.html, &mut self.css, &mut self.typescript]
            .into_iter()
            .enumerate()
        {
            let family = [Kind::Html, Kind::Css, Kind::TypeScript][index];
            let needed = docs
                .list
                .iter()
                .any(|doc| kind(doc.editor.lang()) == Some(family));
            if self.configured[index] != configs[index] {
                session.stop();
                self.configured[index] = configs[index].clone();
            }
            if needed
                && session.session.is_none()
                && matches!(session.status, crate::insight::InsightStatus::Idle)
            {
                if let Some(config) = configs[index].clone() {
                    session.start_web(root, config);
                }
            }
            if session.session.as_ref().is_some_and(|session| {
                !session.is_initialized()
                    && session.started.elapsed() > std::time::Duration::from_secs(20)
            }) {
                session.stop();
                session.status = crate::insight::InsightStatus::Failed(
                    tr("Web language server initialization timed out").into(),
                );
            }
            outputs.extend(session.update(docs, false));
        }
        outputs
    }
    pub fn merge_problems(&self, main: &mut Insight) {
        main.problems
            .retain(|path, _| kind(Lang::from_path(path)).is_none());
        for session in [&self.html, &self.css, &self.typescript] {
            for (path, problems) in &session.problems {
                main.problems.insert(path.clone(), problems.clone());
            }
        }
    }
    pub fn action(
        &mut self,
        doc: &mut Doc,
        action: Action,
        screen: egui::Pos2,
    ) -> Option<Vec<InsightOutput>> {
        kind(doc.editor.lang())?;
        if doc.generated || doc.scratch || doc.editor.buffer.rope().len_bytes() > 2 * 1024 * 1024 {
            return Some(vec![InsightOutput::Message(
                tr("Language server actions are not available in this file").into(),
            )]);
        }
        let insight = self.insight(doc)?;
        if !insight.ready() {
            let message=match &insight.status{crate::insight::InsightStatus::Failed(error)=>trf("Web language server failed: {error}",&[("error",error)]),crate::insight::InsightStatus::Starting|crate::insight::InsightStatus::Loading(_)=>tr("The web language server is still initializing").into(),_=>tr("Configure external web language servers in Settings › Tools and trust the project before using this action").into()};
            return Some(vec![InsightOutput::Message(message)]);
        }
        insight.flush(doc);
        let caret = doc.editor.caret();
        match action {
            Action::Complete => insight.complete(doc, None),
            Action::ParameterInfo => insight.parameter_info(doc),
            Action::QuickDoc => insight.quick_doc(doc, screen),
            Action::GotoDeclaration => insight.navigate(doc, caret, NavKind::Declaration),
            Action::GotoImplementation => insight.navigate(doc, caret, NavKind::Implementation),
            Action::GotoTypeDeclaration => insight.navigate(doc, caret, NavKind::TypeDeclaration),
            Action::FindUsages => insight.navigate(doc, caret, NavKind::Usages),
            Action::Rename => insight.prepare_rename(doc),
            Action::Reformat => insight.format(doc),
            _ => {
                return Some(vec![InsightOutput::Message(
                    tr("This web language server action is not supported yet").into(),
                )]);
            }
        }
        Some(Vec::new())
    }
}
pub fn valid_edit(
    edit: &rc_lsp::lsp_types::WorkspaceEdit,
    docs: &Docs,
    snapshot: &std::collections::HashMap<std::path::PathBuf, u64>,
    root: Option<&Path>,
) -> bool {
    use rc_lsp::lsp_types::{DocumentChangeOperation, DocumentChanges, OneOf};
    let Some(root) = root else { return false };
    let mut targets = Vec::new();
    let mut ranges = Vec::new();
    let mut count = 0_usize;
    let mut bytes = 0_usize;
    if let Some(changes) = &edit.changes {
        if changes.len() > 128 {
            return false;
        }
        for (uri, edits) in changes {
            targets.push(uri);
            count = count.saturating_add(edits.len());
            if count > 1000 {
                return false;
            }
            for edit in edits {
                ranges.push((uri, edit));
                bytes = bytes.saturating_add(edit.new_text.len());
                if bytes > 2 * 1024 * 1024 {
                    return false;
                }
            }
        }
    }
    let mut documents = Vec::new();
    match &edit.document_changes {
        Some(DocumentChanges::Edits(edits)) => {
            if edits.len() > 128 {
                return false;
            }
            documents.extend(edits.iter());
        }
        Some(DocumentChanges::Operations(operations)) => {
            if operations.len() > 128 {
                return false;
            }
            for operation in operations {
                match operation {
                    DocumentChangeOperation::Edit(edit) => documents.push(edit),
                    DocumentChangeOperation::Op(_) => return false,
                }
            }
        }
        None => {}
    }
    for document in documents {
        targets.push(&document.text_document.uri);
        count = count.saturating_add(document.edits.len());
        if count > 1000 {
            return false;
        }
        for edit in &document.edits {
            let edit = match edit {
                OneOf::Left(edit) => edit,
                OneOf::Right(edit) => &edit.text_edit,
            };
            ranges.push((&document.text_document.uri, edit));
            bytes = bytes.saturating_add(edit.new_text.len());
            if bytes > 2 * 1024 * 1024 {
                return false;
            }
        }
    }
    let paths_ok = targets.len() <= 128
        && targets.into_iter().all(|uri| {
            rc_lsp::uri_to_path(uri).is_some_and(|path| {
                path.starts_with(root)
                    && docs.list.iter().any(|doc| {
                        doc.path == path
                            && !doc.editor.read_only
                            && kind(doc.editor.lang()).is_some()
                            && snapshot.get(&path) == Some(&doc.editor.buffer.version())
                    })
            })
        });
    if !paths_ok {
        return false;
    }
    let mut per_file = std::collections::BTreeMap::<std::path::PathBuf, Vec<(usize, usize)>>::new();
    for (uri, edit) in ranges {
        let Some(path) = rc_lsp::uri_to_path(uri) else {
            return false;
        };
        let Some(doc) = docs.list.iter().find(|doc| doc.path == path) else {
            return false;
        };
        let rope = doc.editor.buffer.rope();
        let start = crate::convert::char_of(rope, edit.range.start);
        let end = crate::convert::char_of(rope, edit.range.end);
        if start > end
            || crate::convert::lsp_position(rope, start) != edit.range.start
            || crate::convert::lsp_position(rope, end) != edit.range.end
        {
            return false;
        }
        per_file.entry(path).or_default().push((start, end));
    }
    per_file.values_mut().all(|ranges| {
        ranges.sort_unstable();
        ranges.windows(2).all(|pair| pair[0].1 <= pair[1].0)
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::{HOME_LOCK, harness, test_app};
    use egui_kittest::kittest::Queryable;
    use std::time::{Duration, Instant};
    #[track_caller]
    fn pump(
        h: &mut egui_kittest::Harness<'static, crate::app::CrabCraftApp>,
        done: impl Fn(&crate::app::CrabCraftApp) -> bool,
    ) {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(25) {
            h.run_steps(2);
            if done(h.state()) {
                return;
            }
            std::thread::sleep(Duration::from_millis(15));
        }
        panic!(
            "web timeout: HTML={:?} CSS={:?} TS={:?}",
            h.state().web_languages.html.status,
            h.state().web_languages.css.status,
            h.state().web_languages.typescript.status
        );
    }
    fn caret(app: &mut crate::app::CrabCraftApp, needle: &str, end: bool) {
        let doc = app.docs.active_doc_mut().unwrap();
        let text = doc.editor.buffer.rope().to_string();
        let at = text.rfind(needle).unwrap() + if end { needle.len() } else { 0 };
        doc.editor
            .set_selection(rc_text::Selection::point(text[..at].chars().count()));
        app.focus_editor = true;
    }
    #[test]
    #[ignore = "Requires explicitly installed external web servers under /tmp/crabcraft-web-tools"]
    fn native_rust_web_servers_complete_navigate_rename_and_preserve_rust_trust() {
        let _home = HOME_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let home = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let root = project.path().canonicalize().unwrap();
        std::fs::create_dir(root.join("src")).unwrap();
        std::fs::create_dir(root.join("static")).unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname=\"native_rust_web\"\nversion=\"0.1.0\"\nedition=\"2021\"\n",
        )
        .unwrap();
        std::fs::write(root.join("src/main.rs"),"fn main(){assert!(include_str!(\"../static/index.html\").contains(\"div\"));assert!(include_str!(\"../static/app.css\").contains(\"color\"));assert!(include_str!(\"../static/app.js\").contains(\"renamedJs\"));assert!(include_str!(\"../static/app.ts\").contains(\"renamedTs\"));}").unwrap();
        std::fs::write(
            root.join("build.rs"),
            "fn main(){std::fs::write(\"build-ran\",\"yes\").unwrap();}",
        )
        .unwrap();
        let html = "<!doctype html>\n<style>.card { color: red; }</style>\n<script>const embeddedValue = 42;</script>\n<di\n";
        let css = ".card { col }\n";
        let js = "const unicodeJs=\"🦀\"; function greetJs(name){return name;}\ngreetJ\n";
        let ts =
            "const unicodeTs=\"🦀\"; function greetTs(name:string):string{return name;}\ngreetT\n";
        for (file, text) in [
            ("index.html", html),
            ("app.css", css),
            ("app.js", js),
            ("app.ts", ts),
        ] {
            std::fs::write(root.join("static").join(file), text).unwrap();
        }
        let mut app = test_app(home.path(), true);
        app.settings.web_html_server =
            Some("/tmp/crabcraft-web-tools/node_modules/.bin/vscode-html-language-server".into());
        app.settings.web_css_server =
            Some("/tmp/crabcraft-web-tools/node_modules/.bin/vscode-css-language-server".into());
        app.settings.web_typescript_server =
            Some("/tmp/crabcraft-web-tools/node_modules/.bin/typescript-language-server".into());
        app.settings.web_tsserver_path =
            Some("/tmp/crabcraft-web-tools/node_modules/typescript/lib/tsserver.js".into());
        app.open_workspace(&root);
        for file in ["index.html", "app.css", "app.js", "app.ts"] {
            app.open_file(&root.join("static").join(file));
        }
        let mut h = harness(app, egui::vec2(1500.0, 1000.0));
        h.run_steps(3);
        assert!(h.state().web_languages.html.session.is_none());
        assert!(h.state().web_languages.typescript.session.is_none());
        assert!(!root.join("build-ran").exists());
        h.state_mut().settings.trusted_projects.push(root.clone());
        h.run_steps(2);
        pump(&mut h, |app| {
            app.web_languages.html.ready()
                && app.web_languages.css.ready()
                && app.web_languages.typescript.ready()
        });
        assert!(!root.join("build-ran").exists());
        assert!(
            h.state().insight.session.is_none(),
            "web support must not launch rust-analyzer"
        );
        for (file, prefix, wanted) in [
            ("index.html", "di", "div"),
            ("app.css", "col", "color"),
            ("app.js", "greetJ", "greetJs"),
            ("app.ts", "greetT", "greetTs"),
        ] {
            h.state_mut().open_file(&root.join("static").join(file));
            caret(h.state_mut(), prefix, true);
            h.run_steps(2);
            h.state_mut()
                .run_insight_action(Action::Complete, egui::Pos2::ZERO);
            pump(&mut h, |app| {
                app.docs
                    .active_doc()
                    .unwrap()
                    .editor
                    .popups
                    .completion
                    .as_ref()
                    .is_some_and(|popup| popup.items.iter().any(|item| item.label == wanted))
            });
            let popup = h
                .state_mut()
                .docs
                .active_doc_mut()
                .unwrap()
                .editor
                .popups
                .completion
                .as_mut()
                .unwrap();
            let selected = popup
                .filtered
                .iter()
                .position(|index| popup.items[*index].label == wanted)
                .unwrap();
            popup.selected = selected;
            h.state_mut().focus_editor = true;
            h.run_steps(2);
            h.key_press(egui::Key::Enter);
            h.run_steps(3);
            assert!(
                h.state()
                    .docs
                    .active_doc()
                    .unwrap()
                    .editor
                    .buffer
                    .rope()
                    .to_string()
                    .contains(wanted),
                "{file}"
            );
        }
        h.state_mut().open_file(&root.join("static/app.css"));
        let doc = h.state_mut().docs.active_doc_mut().unwrap();
        let length = doc.editor.buffer.len_chars();
        doc.editor.replace(0, length, ".card{color:red;}");
        let old = doc.editor.buffer.version();
        h.run_steps(2);
        h.state_mut()
            .run_insight_action(Action::Reformat, egui::Pos2::ZERO);
        pump(&mut h, |app| {
            app.docs.active_doc().unwrap().editor.buffer.version() > old
        });
        assert!(
            h.state()
                .docs
                .active_doc()
                .unwrap()
                .editor
                .buffer
                .rope()
                .to_string()
                .contains("color: red")
        );
        for (file, symbol, renamed) in [
            ("app.js", "greetJs", "renamedJs"),
            ("app.ts", "greetTs", "renamedTs"),
        ] {
            h.state_mut().open_file(&root.join("static").join(file));
            caret(h.state_mut(), symbol, false);
            h.run_steps(2);
            h.state_mut()
                .run_insight_action(Action::GotoDeclaration, egui::Pos2::ZERO);
            pump(&mut h, |app| {
                app.docs.active_doc().unwrap().editor.caret_line_col().0 == 0
            });
            caret(h.state_mut(), symbol, false);
            h.run_steps(2);
            h.state_mut()
                .run_insight_action(Action::Rename, egui::Pos2::ZERO);
            pump(&mut h, |app| {
                matches!(
                    app.dialog,
                    Some(crate::ui::dialogs::Dialog::RenameSymbol { .. })
                )
            });
            if let Some(crate::ui::dialogs::Dialog::RenameSymbol { name, .. }) =
                &mut h.state_mut().dialog
            {
                *name = renamed.into();
            }
            h.run_steps(2);
            h.get_by_label(tr("Rename")).click();
            h.run_steps(2);
            pump(&mut h, |app| {
                app.docs
                    .active_doc()
                    .unwrap()
                    .editor
                    .buffer
                    .rope()
                    .to_string()
                    .matches(renamed)
                    .count()
                    == 2
            });
        }
        let ts_path = root.join("static/app.ts");
        let doc = h.state_mut().docs.active_doc_mut().unwrap();
        let at = doc.editor.buffer.len_chars();
        doc.editor
            .replace(at, at, "const wrongType: number = \"text\";\n");
        h.run_steps(2);
        pump(&mut h, |app| {
            app.insight.problems.get(&ts_path).is_some_and(|problems| {
                problems
                    .iter()
                    .any(|problem| problem.code.as_deref() == Some("2322"))
            })
        });
        h.state_mut().docs.active_doc_mut().unwrap().editor.undo();
        h.run_steps(2);
        pump(&mut h, |app| {
            !app.insight.problems.get(&ts_path).is_some_and(|problems| {
                problems
                    .iter()
                    .any(|problem| problem.code.as_deref() == Some("2322"))
            })
        });
        h.state_mut().open_file(&root.join("static/index.html"));
        h.state_mut()
            .docs
            .active_doc_mut()
            .unwrap()
            .editor
            .parse_blocking();
        let syntax = &h.state().docs.active_doc().unwrap().editor.syntax;
        let rope = h.state().docs.active_doc().unwrap().editor.buffer.rope();
        let spans = syntax.highlight(rope, 0, rope.len_lines());
        assert!(
            spans[1]
                .iter()
                .any(|span| span.hl == rc_syntax::Hl::Property)
        );
        assert!(
            spans[2]
                .iter()
                .any(|span| span.hl == rc_syntax::Hl::Keyword)
        );
        assert!(h.state_mut().docs.save_all().is_empty());
        let result = std::process::Command::new("cargo")
            .args(["run", "--offline", "--quiet"])
            .current_dir(&root)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(root.join("build-ran").exists());
        // Explicitly start Rust analysis after the authorized Rust compilation. Web sessions
        // remain separate and cannot consume Rust edits or resolve its completion popup.
        h.state_mut().open_file(&root.join("src/main.rs"));
        let doc = h.state_mut().docs.active_doc_mut().unwrap();
        let source = doc.editor.buffer.rope().to_string();
        let at = source[..source.rfind('}').unwrap()].chars().count();
        doc.editor
            .replace(at, at, "let rustCounter:u32=1; rustCoun");
        h.state_mut().insight.start_rust(&root, None, false);
        pump(&mut h, |app| {
            app.insight.status == crate::insight::InsightStatus::Ready
                && app
                    .insight
                    .session
                    .as_ref()
                    .is_some_and(|session| session.docs.is_open(&root.join("src/main.rs")))
        });
        assert!(
            h.state()
                .insight
                .session
                .as_ref()
                .unwrap()
                .docs
                .paths()
                .iter()
                .all(|path| path.extension().is_some_and(|extension| extension == "rs"))
        );
        assert!(
            !h.state()
                .web_languages
                .html
                .session
                .as_ref()
                .unwrap()
                .docs
                .is_open(&root.join("src/main.rs"))
        );
        assert!(
            !h.state()
                .web_languages
                .typescript
                .session
                .as_ref()
                .unwrap()
                .docs
                .is_open(&root.join("src/main.rs"))
        );
        caret(h.state_mut(), "rustCoun", true);
        h.run_steps(2);
        h.state_mut()
            .run_insight_action(Action::Complete, egui::Pos2::ZERO);
        pump(&mut h, |app| {
            app.docs
                .active_doc()
                .unwrap()
                .editor
                .popups
                .completion
                .as_ref()
                .is_some_and(|popup| {
                    popup
                        .items
                        .iter()
                        .any(|item| item.label.starts_with("rustCounter"))
                })
        });
        h.state_mut().docs.active_doc_mut().unwrap().editor.undo();
    }
}

#[cfg(test)]
mod edit_tests {
    use super::*;
    use serde_json::json;
    use std::collections::HashMap;
    #[test]
    fn web_workspace_edits_refuse_stale_closed_readonly_and_external_files() {
        let _home = crate::screens::HOME_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let home = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let root = project.path().canonicalize().unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname=\"web_edit_safety\"\nversion=\"0.1.0\"\n",
        )
        .unwrap();
        let path = root.join("app.ts");
        std::fs::write(&path, "const name=1;\n").unwrap();
        let mut app = crate::screens::test_app(home.path(), true);
        app.open_workspace(&root);
        app.open_file(&path);
        let uri = rc_lsp::path_to_uri(&path);
        let edit:rc_lsp::lsp_types::WorkspaceEdit=serde_json::from_value(json!({"changes":{uri.as_str():[{"range":{"start":{"line":0,"character":6},"end":{"line":0,"character":10}},"newText":"renamed"}]}})).unwrap();
        let version = app.docs.active_doc().unwrap().editor.buffer.version();
        let snapshot = HashMap::from([(path.clone(), version)]);
        assert!(valid_edit(&edit, &app.docs, &snapshot, Some(&root)));
        let mut malformed = edit.clone();
        malformed
            .changes
            .as_mut()
            .unwrap()
            .values_mut()
            .next()
            .unwrap()[0]
            .range
            .start
            .line = 99;
        assert!(!valid_edit(&malformed, &app.docs, &snapshot, Some(&root)));
        let mut overlapping = edit.clone();
        let changes = overlapping
            .changes
            .as_mut()
            .unwrap()
            .values_mut()
            .next()
            .unwrap();
        changes.push(changes[0].clone());
        assert!(!valid_edit(&overlapping, &app.docs, &snapshot, Some(&root)));
        app.docs.active_doc_mut().unwrap().editor.read_only = true;
        assert!(!valid_edit(&edit, &app.docs, &snapshot, Some(&root)));
        app.docs.active_doc_mut().unwrap().editor.read_only = false;
        app.docs
            .active_doc_mut()
            .unwrap()
            .editor
            .replace(0, 0, "// changed\n");
        assert!(!valid_edit(&edit, &app.docs, &snapshot, Some(&root)));
        assert!(!valid_edit(&edit, &app.docs, &HashMap::new(), Some(&root)));
        let resource: rc_lsp::lsp_types::WorkspaceEdit = serde_json::from_value(
            json!({"documentChanges":[{"kind":"delete","uri":uri.as_str()}]}),
        )
        .unwrap();
        assert!(!valid_edit(&resource, &app.docs, &snapshot, Some(&root)));
        let external = tempfile::tempdir().unwrap();
        let external_path = external.path().join("external.ts");
        std::fs::write(&external_path, "const outside=1;").unwrap();
        app.open_file(&external_path);
        let external_uri = rc_lsp::path_to_uri(&external_path);
        let external_edit:rc_lsp::lsp_types::WorkspaceEdit=serde_json::from_value(json!({"changes":{external_uri.as_str():[{"range":{"start":{"line":0,"character":6},"end":{"line":0,"character":13}},"newText":"renamed"}]}})).unwrap();
        let external_snapshot = HashMap::from([(
            external_path,
            app.docs.active_doc().unwrap().editor.buffer.version(),
        )]);
        assert!(!valid_edit(
            &external_edit,
            &app.docs,
            &external_snapshot,
            Some(&root)
        ));
    }
}
