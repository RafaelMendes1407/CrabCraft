//! Conservative, syntax-checked Rust statement completion. Parsing stays off the UI thread.
use crate::docs::Docs;
use rc_core::{Inbox, Waker};
use rc_kit::i18n::tr;
use rc_text::{Rope, Selection};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

#[derive(Debug, PartialEq)]
enum Error {
    Unsupported,
    Cancelled,
}
struct Edit {
    start: usize,
    end: usize,
    text: String,
    caret: usize,
}
pub struct Job {
    id: u64,
    version: u64,
    selection: Selection,
    cancel: Arc<AtomicBool>,
    result: Inbox<Result<Edit, Error>>,
}
impl Drop for Job {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl Job {
    pub fn start(docs: &Docs, waker: Waker) -> Result<Self, String> {
        let doc = docs.active.and_then(|i| docs.list.get(i)).ok_or_else(|| tr("Open an editable Rust file with one caret").to_string())?;
        let editor = &doc.editor;
        if editor.read_only
            || doc.path.extension().is_none_or(|ext| ext != "rs")
            || editor.selection.ranges().len() != 1
            || !editor.selection.primary().is_empty()
            || editor.buffer.rope().len_bytes() > 1024 * 1024
        {
            return Err(tr("Open an editable Rust file with one caret").to_string());
        }
        let rope = editor.buffer.rope().clone();
        let caret = editor.caret();
        let indent_unit = editor.indent.unit.clone();
        let result = Inbox::new(waker);
        let out = result.sender();
        let cancel = Arc::new(AtomicBool::new(false));
        let token = cancel.clone();
        std::thread::spawn(move || {
            out.send(plan_with_indent(&rope, caret, &token, &indent_unit));
        });
        Ok(Self { id: editor.id, version: editor.buffer.version(), selection: editor.selection.clone(), cancel, result })
    }

    /// None while running; stale, cancelled or closed documents never receive edits.
    pub fn poll(&self, docs: &mut Docs) -> Option<Result<(), String>> {
        let Some(doc) = docs
            .active
            .and_then(|i| docs.list.get_mut(i))
            .filter(|d| d.editor.id == self.id && d.editor.buffer.version() == self.version && d.editor.selection == self.selection && !d.editor.read_only)
        else {
            return Some(Ok(()));
        };
        let result = self.result.drain().into_iter().next()?;
        Some(match result {
            Ok(edit) => {
                doc.editor.replace(edit.start, edit.end, &edit.text);
                doc.editor.selection = Selection::point(edit.caret);
                doc.editor.request_scroll_to_caret();
                Ok(())
            }
            Err(Error::Cancelled) => Ok(()),
            Err(Error::Unsupported) => Err(tr("This statement needs manual editing; no changes were made").to_string()),
        })
    }
}

fn plan_with_indent(rope: &Rope, caret: usize, cancel: &AtomicBool, indent_unit: &str) -> Result<Edit, Error> {
    if cancel.load(Ordering::Relaxed) {
        return Err(Error::Cancelled);
    }
    if caret > rope.len_chars() {
        return Err(Error::Unsupported);
    }
    let line = rope.char_to_line(caret);
    let start_char = rope.line_to_char(line);
    let raw = rope.line(line).to_string();
    let body = raw.trim_end_matches(['\n', '\r', ' ', '\t']);
    let indent: String = body.chars().take_while(|c| matches!(c, ' ' | '\t')).collect();
    let text = body.trim_start();
    if text.is_empty() || text.starts_with(['}', '/', '#']) {
        return Err(Error::Unsupported);
    }
    let source = rope.to_string();
    let start_byte = rope.char_to_byte(start_char);
    let end_byte = start_byte + body.len();
    let tree = rc_syntax::parse_now(rc_syntax::Lang::Rust, rope, None).ok_or(Error::Unsupported)?;
    let mut masked = body.as_bytes().to_vec();
    fn hide(node: rc_syntax::tree_sitter::Node<'_>, from: usize, to: usize, mask: &mut [u8]) -> Result<(), Error> {
        if node.end_byte() <= from || node.start_byte() >= to {
            return Ok(());
        }
        if matches!(node.kind(), "line_comment" | "block_comment") {
            return Err(Error::Unsupported);
        }
        if matches!(node.kind(), "string_literal" | "raw_string_literal" | "char_literal") {
            if node.start_byte() < from || node.end_byte() > to || node.has_error() {
                return Err(Error::Unsupported);
            }
            mask[node.start_byte() - from..node.end_byte() - from].fill(b' ');
            return Ok(());
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            hide(child, from, to, mask)?;
        }
        Ok(())
    }
    hide(tree.root_node(), start_byte, end_byte, &mut masked)?;
    let mut stack = Vec::new();
    for byte in masked {
        match byte {
            b'(' => stack.push(b')'),
            b'[' => stack.push(b']'),
            b')' | b']' => {
                if stack.pop() != Some(byte) {
                    return Err(Error::Unsupported);
                }
            }
            _ => (),
        }
    }
    let mut suffix: String = stack.into_iter().rev().map(char::from).collect();
    let declaration = ["let ", "const ", "static "].iter().any(|prefix| text.starts_with(prefix));
    if declaration && !text.ends_with(';') {
        suffix.push(';');
    }
    let header =
        ["fn ", "pub fn ", "pub(crate) fn ", "async fn ", "pub async fn ", "if ", "while ", "for ", "match "].iter().any(|prefix| text.starts_with(prefix))
            || text == "loop"
            || text == "else";
    let add_block = header && !text.contains('{') && !text.ends_with(';');
    let inner_indent = format!("{indent}{indent_unit}");
    let (replacement, cursor_bytes) = if add_block {
        let prefix = format!("{body}{suffix} {{\n{inner_indent}");
        (format!("{prefix}\n{indent}}}"), prefix.len())
    } else {
        let next_indent = if text.ends_with('{') { &inner_indent } else { &indent };
        let replacement = format!("{body}{suffix}\n{next_indent}");
        let len = replacement.len();
        (replacement, len)
    };
    if cancel.load(Ordering::Relaxed) {
        return Err(Error::Cancelled);
    }
    let candidate = format!("{}{}{}", &source[..start_byte], replacement, &source[end_byte..]);
    let checked = rc_syntax::parse_now(rc_syntax::Lang::Rust, &Rope::from_str(&candidate), None).ok_or(Error::Unsupported)?;
    if checked.root_node().has_error() {
        return Err(Error::Unsupported);
    }
    if cancel.load(Ordering::Relaxed) {
        return Err(Error::Cancelled);
    }
    let caret = start_char + replacement[..cursor_bytes].chars().count();
    Ok(Edit { start: start_char, end: start_char + body.chars().count(), text: replacement, caret })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn plan(rope: &Rope, caret: usize, cancel: &AtomicBool) -> Result<Edit, Error> {
        plan_with_indent(rope, caret, cancel, "    ")
    }
    #[test]
    fn closes_delimiters_and_let_without_changing_tail_semantics() {
        for (source, expected) in [
            ("fn main() {\n    let café = vec![1, 2\n}\n", "let café = vec![1, 2];"),
            ("fn answer() -> i32 {\n    (40 + 2\n}\n", "    (40 + 2)\n"),
            ("fn main() {\n    println!(\"hello )]\"\n}\n", "println!(\"hello )]\")\n"),
            ("fn main() {\n    if true\n}\n", "if true {\n        \n    }"),
        ] {
            let rope = Rope::from_str(source);
            let edit = plan(&rope, rope.line_to_char(1) + 4, &AtomicBool::new(false)).expect("supported statement");
            let mut changed = source.to_string();
            changed.replace_range(rope.char_to_byte(edit.start)..rope.char_to_byte(edit.end), &edit.text);
            assert!(changed.contains(expected), "{changed}");
        }
    }
    #[test]
    fn ambiguous_comments_bad_strings_and_other_errors_are_rejected() {
        for source in ["fn main() {\n let x = 1 // comment\n}\n", "fn main() {\n let x = \"bad\n}\n", "fn main() {\n let x = missing +\n}\n"] {
            let rope = Rope::from_str(source);
            assert!(matches!(plan(&rope, rope.line_to_char(1), &AtomicBool::new(false)), Err(Error::Unsupported)));
        }
        assert!(matches!(plan(&Rope::from_str(""), 0, &AtomicBool::new(true)), Err(Error::Cancelled)));
    }

    #[test]
    fn native_shortcut_repairs_real_cargo_source_is_undoable_and_never_runs_project_code() {
        let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("src")).unwrap();
        std::fs::write(root.path().join("Cargo.toml"), "[package]\nname=\"statement_fixture\"\nversion=\"0.1.0\"\nedition=\"2024\"\n[workspace]\n").unwrap();
        std::fs::write(root.path().join("build.rs"), "fn main() { std::fs::write(\"build-marker\", \"ran\").unwrap(); }\n").unwrap();
        let source = "fn main() {\n    let café = vec![1, 2\n    assert_eq!(café.len(), 2);\n}\n";
        let path = root.path().join("src/main.rs");
        std::fs::write(&path, source).unwrap();
        let mut app = crate::screens::test_app(home.path(), false);
        app.settings.keymap = rc_core::settings::KeymapChoice::RustRover;
        app.keymap = rc_editor::Keymap::new(rc_editor::KeymapKind::RustRover);
        app.open_workspace_and_wait(root.path());
        app.open_file_and_wait(&path);
        app.docs.active_doc_mut().unwrap().editor.selection = Selection::point(source[..source.find("vec!").unwrap()].chars().count());
        let mut h = crate::screens::harness(app, egui::vec2(1280.0, 800.0));
        h.state_mut().focus_editor = true;
        h.run_steps(2);
        h.key_press_modifiers(egui::Modifiers::CTRL | egui::Modifiers::SHIFT, egui::Key::Enter);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            h.run_steps(1);
            if h.state().docs.active_doc().unwrap().editor.buffer.rope().to_string().contains("vec![1, 2];") {
                break;
            }
            assert!(std::time::Instant::now() < deadline, "shortcut did not complete the statement");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(!root.path().join("build-marker").exists());
        assert!(!root.path().join("Cargo.lock").exists());
        let completed = h.state().docs.active_doc().unwrap().editor.buffer.rope().to_string();
        h.state_mut().docs.active_doc_mut().unwrap().editor.undo();
        assert_eq!(h.state().docs.active_doc().unwrap().editor.buffer.rope().to_string(), source);
        h.state_mut().docs.active_doc_mut().unwrap().editor.read_only = true;
        assert!(Job::start(&h.state().docs, Waker::noop()).is_err());
        h.state_mut().docs.active_doc_mut().unwrap().editor.read_only = false;
        let job = Job::start(&h.state().docs, Waker::noop()).unwrap();
        h.state_mut().docs.active_doc_mut().unwrap().editor.replace(0, 0, "// newer edit\n");
        assert!(matches!(job.poll(&mut h.state_mut().docs), Some(Ok(()))));
        assert!(h.state().docs.active_doc().unwrap().editor.buffer.rope().to_string().starts_with("// newer edit"));
        std::fs::write(&path, completed).unwrap();
        let output = std::process::Command::new("cargo").args(["run", "--offline", "--quiet"]).current_dir(root.path()).output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        assert!(root.path().join("build-marker").exists());
    }
}
