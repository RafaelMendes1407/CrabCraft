//! Completion in `Cargo.toml`, computed here from the text (rust-analyzer does not complete
//! TOML): the analysis is `rc_cargo::manifest_complete`; this fills the editor's completion
//! popup with it, on ⌃Space and while typing.
//!
//! In a dependency table, keys are crate names (C-02): with the online check on, the typed text
//! is searched on crates.io once typing pauses, stale searches are cancelled, and answers are
//! kept for the session; with it off, the names come from `Cargo.lock` and Cargo's local
//! registry, and nothing is sent. Inside a version string the crate's published versions are
//! offered, from the sparse index (or the lock when offline). Nothing here blocks the UI.

use crate::dep_versions::{DepVersions, Lookup};
use crate::docs::Doc;
use rc_cargo::crates_io::CrateHit;
use rc_cargo::manifest_complete::{self, Context, Kind};
use rc_core::{Inbox, Waker};
use rc_editor::{Completion, CompletionItem, EditorEvent};
use rc_kit::i18n::tr;
use rc_kit::ico;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// `data` of a local item: no server item behind it.
pub const LOCAL_ITEM: usize = usize::MAX;
/// `data` of a local item that leaves the caret where a value can be completed.
pub const LOCAL_ITEM_THEN_VALUE: usize = usize::MAX - 1;

/// How long typing must pause before crates.io is searched.
const SEARCH_DELAY: Duration = Duration::from_millis(300);
const SEARCH_TIMEOUT: Duration = Duration::from_secs(10);
/// The most local names put in one list.
const MAX_LOCAL_ITEMS: usize = 300;
/// The most versions listed.
const MAX_VERSIONS: usize = 60;

pub fn is_manifest(doc: &Doc) -> bool {
    doc.path.file_name().is_some_and(|n| n == "Cargo.toml")
}

/// A crate-name list being typed into.
struct NameList {
    editor: u64,
    anchor: usize,
    /// The text after `anchor` when it last changed, and when.
    typed: String,
    since: Instant,
    /// The search answer the list shows (`None`: local names only).
    shown: Option<String>,
    /// Whether the list showed "loading" last time.
    loading: bool,
}

/// A version list waiting for the crate's versions.
struct VersionList {
    editor: u64,
    anchor: usize,
    name: String,
}

/// A running search.
struct Search {
    query: String,
    cancel: Arc<AtomicBool>,
}

/// Crate names and versions for `Cargo.toml` completion.
pub struct CrateNames {
    waker: Waker,
    /// crates.io answers by query, for the session (`None`: the search failed).
    answers: HashMap<String, Option<Vec<CrateHit>>>,
    searching: Option<Search>,
    found: Inbox<(String, Option<Vec<CrateHit>>)>,
    /// The crates in Cargo's local registry, read once on a background thread.
    local: Option<Arc<Vec<String>>>,
    local_loading: Option<Inbox<Vec<String>>>,
    names: Option<NameList>,
    versions: Option<VersionList>,
}

/// What completion needs from the rest of the app.
pub struct Cx<'a> {
    pub names: &'a mut CrateNames,
    pub deps: &'a mut DepVersions,
    /// The online check (`check_crates_io`).
    pub online: bool,
}

impl CrateNames {
    pub fn new(waker: Waker) -> Self {
        CrateNames { found: Inbox::new(waker.clone()), waker, answers: HashMap::new(), searching: None, local: None, local_loading: None, names: None, versions: None }
    }

    /// Records a crates.io search answer without the network (tests).
    #[cfg(test)]
    pub fn set_answer(&mut self, query: &str, hits: Vec<CrateHit>) {
        self.answers.insert(query.to_string(), Some(hits));
    }

    /// Sets the local registry names without reading the disk (tests).
    #[cfg(test)]
    pub fn set_local(&mut self, names: Vec<String>) {
        self.local = Some(Arc::new(names));
    }

    /// The query of the search running, if any (tests).
    #[cfg(test)]
    pub fn searching(&self) -> Option<&str> {
        self.searching.as_ref().map(|s| s.query.as_str())
    }

    fn load_local(&mut self) {
        if self.local.is_some() || self.local_loading.is_some() {
            return;
        }
        let inbox = Inbox::new(self.waker.clone());
        let out = inbox.sender();
        std::thread::spawn(move || {
            out.send(rc_cargo::crates_io::cargo_home().map(|h| rc_cargo::crates_io::local_crate_names(&h)).unwrap_or_default());
        });
        self.local_loading = Some(inbox);
    }

    fn cancel_search(&mut self) {
        if let Some(s) = self.searching.take() {
            s.cancel.store(true, Ordering::Relaxed);
        }
    }

    fn search(&mut self, query: &str) {
        self.cancel_search();
        let cancel = Arc::new(AtomicBool::new(false));
        self.searching = Some(Search { query: query.to_string(), cancel: cancel.clone() });
        let out = self.found.sender();
        let q = query.to_string();
        std::thread::spawn(move || {
            let hits = rc_cargo::crates_io::search_crates_io(&q, SEARCH_TIMEOUT, &cancel);
            if !cancel.load(Ordering::Relaxed) {
                out.send((q, hits.ok()));
            }
        });
    }

    /// The best answer for `typed`: its own, else the one for its longest prefix.
    fn best_answer(&self, typed: &str) -> Option<String> {
        let typed = typed.to_ascii_lowercase();
        // Char boundaries only: what is typed may be any text.
        let ends = typed.char_indices().map(|(i, c)| i + c.len_utf8());
        ends.rev().map(|n| &typed[..n]).find(|q| matches!(self.answers.get(*q), Some(Some(_)))).map(str::to_string)
    }

    /// Takes finished searches and look-ups, starts the search for what is typed once typing
    /// pauses, and refreshes an open list. Returns how soon to call again while waiting.
    pub fn update(docs: &mut crate::docs::Docs, cx: &mut Cx) -> Option<Duration> {
        let me = &mut *cx.names;
        if let Some(names) = me.local_loading.as_ref().and_then(|i| i.drain().into_iter().last()) {
            me.local = Some(Arc::new(names));
            me.local_loading = None;
        }
        for (query, hits) in me.found.drain() {
            if me.searching.as_ref().is_some_and(|s| s.query == query) {
                me.searching = None;
            }
            me.answers.insert(query, hits);
        }
        let doc = docs.active_doc_mut()?;
        let open = doc.editor.popups.completion.as_ref().map(|c| c.anchor);
        // A version list waiting for crates.io.
        if let Some(v) = &me.versions {
            if v.editor != doc.editor.id || open != Some(v.anchor) {
                me.versions = None;
            } else if !matches!(cx.deps.lookup(&v.name), Some(Lookup::Asking)) {
                me.versions = None;
                complete(doc, false, cx);
                return None;
            }
        }
        let me = &mut *cx.names;
        let Some(list) = &mut me.names else { return None };
        if list.editor != doc.editor.id || open != Some(list.anchor) {
            me.names = None;
            me.cancel_search();
            return None;
        }
        let caret = doc.editor.caret();
        let rope = doc.editor.buffer.rope();
        if caret < list.anchor || caret > rope.len_chars() {
            return None;
        }
        let typed = rope.slice(list.anchor..caret).to_string();
        let now = Instant::now();
        if typed != list.typed {
            list.typed = typed.clone();
            list.since = now;
        }
        let mut wait = None;
        let query = typed.to_ascii_lowercase();
        let pending = cx.online && rc_cargo::valid_crate_name(&query) && !me.answers.contains_key(&query);
        if pending && me.searching.as_ref().is_none_or(|s| s.query != query) {
            let rest = now.duration_since(list.since);
            if rest >= SEARCH_DELAY {
                me.search(&query);
            } else {
                // A new key stroke: the older search is stale.
                me.cancel_search();
                wait = Some(SEARCH_DELAY - rest);
            }
        }
        let best = me.best_answer(&typed);
        let Some(list) = &me.names else { return wait };
        if best != list.shown || pending != list.loading {
            let anchor = list.anchor;
            let items = name_items(me, cx.deps, doc, &typed, cx.online, best.as_deref());
            let mut c = Completion::new(anchor, items, pending);
            c.extra_word_chars = "-";
            let selected = doc.editor.popups.completion.as_ref().and_then(|c| c.selected_item().map(|i| i.label.clone()));
            c.filter(&typed);
            if let Some(label) = selected {
                c.selected = c.filtered.iter().position(|&i| c.items[i].label == label).unwrap_or(0);
            }
            doc.editor.popups.completion = Some(c);
            if let Some(list) = &mut me.names {
                list.shown = best;
                list.loading = pending;
            }
        }
        wait
    }
}

/// Opens the completion list at the caret. `explicit` (⌃Space) shows "No suggestions" instead
/// of nothing.
pub fn complete(doc: &mut Doc, explicit: bool, cx: &mut Cx) {
    let caret = doc.editor.caret();
    // Manifests are small; the analysis reads the text once, without blocking on anything.
    let text = doc.editor.buffer.rope().to_string();
    match manifest_complete::context(&text, caret) {
        Some((anchor, Context::Key { table, inline: false })) if manifest_complete::is_dependency_table(&table) => {
            return complete_names(doc, anchor, explicit, cx);
        }
        Some((anchor, Context::Value { table, key, in_string: true })) => {
            if let Some(name) = manifest_complete::version_crate(&table, &key).filter(|n| rc_cargo::valid_crate_name(n)) {
                return complete_versions(doc, &text, anchor, &name, explicit, cx);
            }
        }
        _ => {}
    }
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
    show(doc, anchor, items, false, "-.", explicit);
}

/// Puts a list at `anchor`, filtered by what is typed after it.
fn show(doc: &mut Doc, anchor: usize, items: Vec<CompletionItem>, loading: bool, word_chars: &'static str, explicit: bool) {
    let caret = doc.editor.caret();
    let prefix = doc.editor.buffer.rope().slice(anchor..caret).to_string();
    let mut c = Completion::new(anchor, items, loading);
    c.extra_word_chars = word_chars;
    c.filter(&prefix);
    if c.filtered.is_empty() && !explicit && !loading {
        return;
    }
    doc.editor.popups.completion = Some(c);
}

/// What follows the caret's word on its line, trimmed.
fn rest_of_line(doc: &Doc) -> String {
    let rope = doc.editor.buffer.rope();
    let caret = doc.editor.caret();
    let line = rope.char_to_line(caret);
    let end = rope.line_to_char(line) + rc_text::buffer::line_len(rope, line);
    let rest = rope.slice(caret..end.max(caret)).to_string();
    rest.trim_start_matches(|c: char| c.is_alphanumeric() || c == '_' || c == '-').trim().to_string()
}

/// "12.3M" downloads.
fn downloads(n: u64) -> String {
    match n {
        0..1_000 => n.to_string(),
        1_000..1_000_000 => format!("{:.1}k", n as f64 / 1e3),
        1_000_000..1_000_000_000 => format!("{:.1}M", n as f64 / 1e6),
        _ => format!("{:.1}B", n as f64 / 1e9),
    }
}

fn name_items(me: &CrateNames, deps: &DepVersions, doc: &Doc, typed: &str, online: bool, answer: Option<&str>) -> Vec<CompletionItem> {
    let bare = !rest_of_line(doc).is_empty();
    let item = |name: &str, detail: String, type_text: String, doc_text: Option<String>| CompletionItem {
        label: name.to_string(),
        detail,
        type_text,
        icon: ico::PACKAGE,
        insert: if bare { name.to_string() } else { format!("{name} = \"$0\"") },
        replace: None,
        deprecated: false,
        data: if bare { LOCAL_ITEM } else { LOCAL_ITEM_THEN_VALUE },
        doc: doc_text,
    };
    let mut items: Vec<CompletionItem> = Vec::new();
    if let Some(Some(hits)) = answer.and_then(|q| me.answers.get(q)) {
        for h in hits {
            let detail = h.version.clone().unwrap_or_default();
            let count = if h.downloads > 0 { rc_kit::i18n::trf("{n} downloads", &[("n", &downloads(h.downloads))]) } else { String::new() };
            items.push(item(&h.name, detail, count, h.description.clone()));
        }
    }
    // Names already on this machine: the lock's, and the local registry's when offline.
    let lower = typed.to_ascii_lowercase();
    let mut local = deps.locked_names(&doc.path);
    if !online && let Some(l) = &me.local {
        local.extend(l.iter().cloned());
    }
    local.sort();
    local.dedup();
    let mut added = 0;
    for name in local {
        if added >= MAX_LOCAL_ITEMS {
            break;
        }
        if name.to_ascii_lowercase().contains(&lower) && !items.iter().any(|i| i.label == name) {
            items.push(item(&name, String::new(), String::new(), None));
            added += 1;
        }
    }
    items
}

fn complete_names(doc: &mut Doc, anchor: usize, explicit: bool, cx: &mut Cx) {
    if !cx.online {
        cx.names.load_local();
    }
    let caret = doc.editor.caret();
    let typed = doc.editor.buffer.rope().slice(anchor..caret).to_string();
    let query = typed.to_ascii_lowercase();
    let best = cx.names.best_answer(&typed);
    let pending = cx.online && rc_cargo::valid_crate_name(&query) && !cx.names.answers.contains_key(&query);
    let items = name_items(cx.names, cx.deps, doc, &typed, cx.online, best.as_deref());
    // Online, the list stays open for the search answer even when nothing local matches.
    show(doc, anchor, items, pending, "-", explicit || pending);
    if doc.editor.popups.completion.as_ref().is_some_and(|c| c.anchor == anchor) {
        cx.names.names = Some(NameList { editor: doc.editor.id, anchor, typed, since: Instant::now(), shown: best, loading: pending });
    }
}

fn complete_versions(doc: &mut Doc, text: &str, anchor: usize, name: &str, explicit: bool, cx: &mut Cx) {
    let caret = doc.editor.caret();
    let chars: Vec<char> = text.chars().collect();
    let quote = chars[..anchor.min(chars.len())].last().copied().filter(|q| *q == '"' || *q == '\'').unwrap_or('"');
    let closed = chars.get(caret) == Some(&quote);
    let mut choices: Vec<(String, bool)> = Vec::new();
    let mut loading = false;
    if cx.online {
        match cx.deps.lookup(name) {
            Some(Lookup::Found(p)) => choices = rc_cargo::versions::version_choices(p, MAX_VERSIONS),
            Some(Lookup::Asking) => loading = true,
            Some(Lookup::Missing | Lookup::Failed) => {}
            None => {
                cx.deps.request(name);
                loading = true;
            }
        }
    }
    if choices.is_empty() {
        choices = cx.deps.locked_versions(&doc.path, name).into_iter().map(|v| (v.to_string(), false)).collect();
    }
    let items = choices
        .into_iter()
        .map(|(v, short)| CompletionItem {
            label: v.clone(),
            detail: if short { tr("requirement").to_string() } else { String::new() },
            type_text: String::new(),
            icon: ico::CONSTANT,
            insert: if closed { v } else { format!("{v}{quote}") },
            replace: None,
            deprecated: false,
            data: LOCAL_ITEM,
            doc: None,
        })
        .collect();
    show(doc, anchor, items, loading, "-.+", explicit || loading);
    cx.names.versions = loading.then(|| VersionList { editor: doc.editor.id, anchor, name: name.to_string() });
}

/// Typing opens the list where a header, key or value starts (IntelliJ's autopopup).
pub fn on_editor_events(doc: &mut Doc, events: &[EditorEvent], cx: &mut Cx) {
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
            complete(doc, false, cx);
        }
    }
}

/// After the editor inserted a local item: a key whose value comes from a fixed set opens the
/// list of values.
pub fn accepted(doc: &mut Doc, data: usize, cx: &mut Cx) {
    if data == LOCAL_ITEM_THEN_VALUE {
        complete(doc, false, cx);
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

    #[test]
    fn answers_are_found_for_any_typed_text() {
        let mut names = super::CrateNames::new(rc_core::Waker::noop());
        names.set_answer("se", Vec::new());
        assert_eq!(names.best_answer("Serde").as_deref(), Some("se"));
        assert_eq!(names.best_answer("séé"), None);
        assert_eq!(names.best_answer("seé").as_deref(), Some("se"));
        assert_eq!(names.best_answer(""), None);
    }

    #[test]
    fn downloads_are_short() {
        assert_eq!(super::downloads(999), "999");
        assert_eq!(super::downloads(12_345), "12.3k");
        assert_eq!(super::downloads(1_640_193_905), "1.6B");
    }
}
