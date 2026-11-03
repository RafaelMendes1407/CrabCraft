//! Completion in `Cargo.toml`, computed here from the text (rust-analyzer does not complete
//! TOML): the analysis is `rc_cargo::manifest_complete`; this fills the editor's completion
//! popup with it, on ⌃Space and while typing.

use crate::docs::Doc;
use rc_cargo::manifest_complete::{self, Kind};
use rc_editor::{Completion, CompletionItem, EditorEvent};
use rc_kit::i18n::tr;
use rc_kit::ico;

/// `data` of a local item: no server item behind it.
pub const LOCAL_ITEM: usize = usize::MAX;
/// `data` of a local item that leaves the caret where a value can be completed.
pub const LOCAL_ITEM_THEN_VALUE: usize = usize::MAX - 1;

pub fn is_manifest(doc: &Doc) -> bool {
    doc.path.file_name().is_some_and(|n| n == "Cargo.toml")
}

/// Opens the completion list at the caret. `explicit` (⌃Space) shows "No suggestions" instead
/// of nothing.
pub fn complete(doc: &mut Doc, explicit: bool) {
    let caret = doc.editor.caret();
    let rope = doc.editor.buffer.rope();
    // Manifests are small; the analysis reads the text once, without blocking on anything.
    let text = rope.to_string();
    let found = manifest_complete::complete(&text, caret);
    let Some(found) = found.filter(|f| !f.items.is_empty() || explicit) else {
        if explicit {
            doc.editor.popups.completion = Some(Completion::new(caret, Vec::new(), false));
        }
        return;
    };
    let anchor = found.anchor.min(caret);
    let items = found
        .items
        .into_iter()
        .map(|s| CompletionItem {
            label: s.label,
            detail: String::new(),
            type_text: String::new(),
            icon: match s.kind {
                Kind::Table => ico::NAMESPACE,
                Kind::Key => ico::PROPERTY,
                Kind::Value => ico::CONSTANT,
            },
            insert: s.insert,
            replace: None,
            deprecated: false,
            data: if s.then_value { LOCAL_ITEM_THEN_VALUE } else { LOCAL_ITEM },
            doc: s.doc.map(|d| tr(d).to_string()),
        })
        .collect();
    let prefix = rope.slice(anchor..caret).to_string();
    let mut c = Completion::new(anchor, items, false);
    c.extra_word_chars = "-.";
    c.filter(&prefix);
    if c.filtered.is_empty() && !explicit {
        return;
    }
    doc.editor.popups.completion = Some(c);
}

/// Typing opens the list where a header, key or value starts (IntelliJ's autopopup).
pub fn on_editor_events(doc: &mut Doc, events: &[EditorEvent]) {
    for ev in events {
        let EditorEvent::Typed(c) = ev else { continue };
        if doc.editor.popups.completion.is_some() {
            continue;
        }
        let caret = doc.editor.caret();
        let rope = doc.editor.buffer.rope();
        let word = |ch: char| ch.is_alphanumeric() || ch == '_' || ch == '-';
        let starts_word = word(*c) && (caret < 2 || !word(rope.char(caret - 2)));
        if starts_word || matches!(c, '[' | '"' | '\'') {
            complete(doc, false);
        }
    }
}

/// After the editor inserted a local item: a key whose value comes from a fixed set opens the
/// list of values.
pub fn accepted(doc: &mut Doc, data: usize) {
    if data == LOCAL_ITEM_THEN_VALUE {
        complete(doc, false);
    }
}

#[cfg(test)]
mod tests {
    /// Every key and table description has a translation in every locale.
    #[test]
    fn descriptions_are_translated() {
        for (name, source) in [
            ("es", include_str!("../../../locales/es.json")),
            ("fr", include_str!("../../../locales/fr.json")),
            ("pt-BR", include_str!("../../../locales/pt-BR.json")),
            ("de", include_str!("../../../locales/de.json")),
            ("pl", include_str!("../../../locales/pl.json")),
            ("zh-Hans", include_str!("../../../locales/zh-Hans.json")),
            ("ja", include_str!("../../../locales/ja.json")),
        ] {
            let areas: serde_json::Map<String, serde_json::Value> = serde_json::from_str(source).unwrap_or_default();
            let missing: Vec<_> = rc_cargo::manifest_complete::all_docs()
                .into_iter()
                .filter(|d| !areas.values().any(|a| a.get(*d).is_some_and(|v| v.as_str().is_some_and(|s| !s.is_empty()))))
                .collect();
            assert!(missing.is_empty(), "{name}: no translation for {missing:?}");
        }
    }
}
