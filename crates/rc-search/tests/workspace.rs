//! Index, Find in Files and Replace in Files on real folder trees.

use rc_core::watch::FsEvent;
use rc_core::{Inbox, Waker};
use rc_search::*;
use std::fs;
use std::path::Path;
use std::time::Duration;

fn write(root: &Path, rel: &str, text: &str) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, text).unwrap();
}

/// A small Rust workspace with build output, tool folders and a gitignore.
fn tree() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let r = dir.path();
    write(r, ".gitignore", "*.log\ngenerated/\n");
    write(r, "Cargo.toml", "[workspace]\n");
    write(r, "src/demo/Cargo.toml", "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n");
    write(r, "src/demo/order.rs", "pub fn place_order() {\n    // TODO: order\n}\n");
    write(r, "src/demo/generated.rs", "// TODO: designer\n");
    write(r, "src/web/handler.rs", "pub fn handler() {}\n");
    write(r, "src/web/notes.txt", "todo later\n");
    write(r, "src/demo/target/debug/demo.rs", "// TODO: build output\n");
    write(r, "src/demo/target/metadata.json", "TODO");
    write(r, "node_modules/x/index.js", "TODO");
    write(r, ".idea/state.rs", "TODO");
    write(r, "build.log", "TODO");
    write(r, "generated/gen.rs", "// TODO: generated");
    write(r, "sub/.ignore", "secret.rs\n");
    write(r, "sub/secret.rs", "TODO");
    write(r, "sub/public.rs", "// TODO: public");
    dir
}

fn rels(index: &FileIndex) -> Vec<String> {
    index.snapshot().files().iter().map(|e| e.rel().to_string()).collect()
}

#[test]
fn index_respects_ignore_files_and_hidden_folders() {
    let dir = tree();
    let index = FileIndex::scan_blocking(dir.path());
    assert_eq!(
        rels(&index),
        [
            ".gitignore",
            "Cargo.toml",
            "src/demo/Cargo.toml",
            "src/demo/generated.rs",
            "src/demo/order.rs",
            "src/web/handler.rs",
            "src/web/notes.txt",
            "sub/.ignore",
            "sub/public.rs",
        ]
    );
    let snap = index.snapshot();
    assert_eq!(snap.get("Cargo.toml").unwrap().kind(), FileKind::Other);
    assert_eq!(snap.get("src/demo/Cargo.toml").unwrap().kind(), FileKind::Other);
}

#[test]
fn background_scan_wakes_the_ui() {
    let dir = tree();
    let woke = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = woke.clone();
    let index = FileIndex::new(dir.path());
    assert!(!index.is_ready());
    index.start_scan(Waker::new(move || flag.store(true, std::sync::atomic::Ordering::SeqCst))).wait();
    assert!(index.is_ready());
    assert!(woke.load(std::sync::atomic::Ordering::SeqCst));
    assert_eq!(index.snapshot().len(), 9);
}

#[test]
fn excluded_folders_and_files_stay_out_of_the_index_and_find_in_files() {
    let dir = tree();
    let r = dir.path();
    let excluded = vec!["src/demo".to_string(), "sub/public.rs".to_string()];
    let index = FileIndex::with_excluded(r, excluded.clone());
    index.start_scan(Waker::noop()).wait();
    assert_eq!(rels(&index), [".gitignore", "Cargo.toml", "src/web/handler.rs", "src/web/notes.txt", "sub/.ignore"]);
    // Files created later inside an excluded folder stay out too.
    write(r, "src/demo/new.rs", "");
    write(r, "src/demo/Deep/more.rs", "");
    write(r, "src/web/new.rs", "");
    index.apply(&[FsEvent::Created(r.join("src/demo/new.rs")), FsEvent::Created(r.join("src/demo/Deep")), FsEvent::Created(r.join("src/web/new.rs"))]);
    assert!(!rels(&index).iter().any(|p| p.starts_with("src/demo")), "{:?}", rels(&index));
    assert!(rels(&index).contains(&"src/web/new.rs".to_string()));

    let (files, _) = run_find(r, FindOptions { excluded, ..opts("todo") });
    let rels: Vec<&str> = files.iter().map(|f| f.rel.as_str()).collect();
    assert_eq!(rels, ["src/web/notes.txt"]);
}

#[cfg(unix)]
#[test]
fn symlink_loops_are_skipped() {
    let dir = tree();
    std::os::unix::fs::symlink(dir.path().join("src"), dir.path().join("src/web/loop")).unwrap();
    let index = FileIndex::scan_blocking(dir.path());
    // The loop is detected; files reached through it once are fine, but the walk ends.
    assert!(index.snapshot().len() >= 9);
}

#[test]
fn incremental_updates() {
    let dir = tree();
    let r = dir.path();
    let index = FileIndex::scan_blocking(r);

    write(r, "src/demo/Customer.rs", "");
    write(r, "src/demo/debug.log", "");
    write(r, "src/demo/target/release/x.rs", "");
    write(r, "src/web/Program.rs", "");
    write(r, "src/web/Controllers/HomeController.rs", "");
    write(r, "generated/more.rs", "");
    write(r, "sub/secret2.rs", "");
    index.apply(&[
        FsEvent::Created(r.join("src/demo/Customer.rs")),
        FsEvent::Created(r.join("src/demo/debug.log")),
        FsEvent::Created(r.join("src/demo/target/release/x.rs")),
        FsEvent::Created(r.join("src/web")),
        FsEvent::Created(r.join("generated/more.rs")),
        FsEvent::Created(r.join("sub/secret2.rs")),
    ]);
    let snap = index.snapshot();
    assert!(snap.get("src/demo/Customer.rs").is_some());
    assert!(snap.get("src/web/Program.rs").is_some());
    assert!(snap.get("src/web/Controllers/HomeController.rs").is_some());
    assert!(snap.get("sub/secret2.rs").is_some());
    assert!(snap.get("src/demo/debug.log").is_none());
    assert!(snap.get("src/demo/target/release/x.rs").is_none());
    assert!(snap.get("generated/more.rs").is_none());

    // Rename a file, then remove a whole folder.
    fs::rename(r.join("src/demo/Customer.rs"), r.join("src/demo/Client.rs")).unwrap();
    index.rename(&r.join("src/demo/Customer.rs"), &r.join("src/demo/Client.rs"));
    fs::remove_dir_all(r.join("src/web")).unwrap();
    index.apply(&[FsEvent::Removed(r.join("src/web"))]);
    let snap = index.snapshot();
    assert!(snap.get("src/demo/Customer.rs").is_none());
    assert!(snap.get("src/demo/Client.rs").is_some());
    assert!(!snap.files().iter().any(|e| e.rel().starts_with("src/web/")));

    // A canonical path from the watcher (macOS reports /private/var/...) maps to the same root.
    let canon = r.canonicalize().unwrap();
    write(r, "Canon.rs", "");
    index.apply(&[FsEvent::Created(canon.join("Canon.rs"))]);
    assert!(index.snapshot().get("Canon.rs").is_some());
    // An event for a path that no longer exists counts as a removal.
    index.apply(&[FsEvent::Changed(r.join("src/web/notes.txt"))]);
    assert!(index.snapshot().get("src/web/notes.txt").is_none());
    index.apply(&[FsEvent::Changed(r.join("src/web/notes.txt"))]);
    assert!(index.snapshot().get("src/web/notes.txt").is_none());
}

#[test]
fn events_during_a_scan_are_replayed() {
    let dir = tree();
    let r = dir.path();
    let index = FileIndex::new(r);
    let scan = index.start_scan(Waker::noop());
    write(r, "Late.rs", "");
    index.apply(&[FsEvent::Created(r.join("Late.rs"))]);
    scan.wait();
    assert!(index.snapshot().get("Late.rs").is_some());
}

#[test]
fn go_to_file_over_a_scanned_tree() {
    let dir = tree();
    let index = FileIndex::scan_blocking(dir.path());
    let r = search_files(&index.snapshot(), &FileQuery::parse("ord:12"), &|_| 0, 10);
    assert_eq!(r.matches[0].entry.rel(), "src/demo/order.rs");
    assert_eq!(r.query.line, Some(12));

    let inbox = Inbox::new(Waker::noop());
    let worker = FileSearchWorker::new(index.clone(), inbox.sender());
    worker.search(FileSearchRequest { text: "zzz".into(), limit: 5, boost: None });
    let id = worker.search(FileSearchRequest { text: "web/hand".into(), limit: 5, boost: None });
    let mut last = None;
    while let Some(res) = inbox.recv_timeout(Duration::from_secs(5)) {
        if res.generation == id {
            last = Some(res);
            break;
        }
    }
    let res = last.expect("worker answered the latest query");
    assert_eq!(res.matches.len(), 1);
    assert_eq!(res.matches[0].entry.rel(), "src/web/handler.rs");
}

fn run_find(root: &Path, options: FindOptions) -> (Vec<FileMatches>, FindSummary) {
    let inbox = Inbox::new(Waker::noop());
    let handle = find_in_files(root, options, inbox.sender()).unwrap();
    let id = handle.id();
    handle.wait();
    let mut files = Vec::new();
    let mut summary = None;
    for ev in inbox.drain() {
        assert_eq!(ev.search(), id);
        match ev {
            FindEvent::File { file, .. } => files.push(file),
            FindEvent::Done { summary: s, .. } => summary = Some(s),
        }
    }
    files.sort_by(|a, b| a.rel.cmp(&b.rel));
    (files, summary.expect("done event"))
}

fn opts(pattern: &str) -> FindOptions {
    FindOptions { query: QueryOptions::text(pattern), ..Default::default() }
}

#[test]
fn find_in_files_respects_ignores_mask_and_scope() {
    let dir = tree();
    let (files, summary) = run_find(dir.path(), opts("todo"));
    let rels: Vec<&str> = files.iter().map(|f| f.rel.as_str()).collect();
    assert_eq!(rels, ["src/demo/generated.rs", "src/demo/order.rs", "src/web/notes.txt", "sub/public.rs"]);
    assert_eq!(summary.matches, 4);
    assert!(!summary.truncated && !summary.cancelled);

    let order = &files[1];
    assert_eq!(order.path, dir.path().join("src/demo/order.rs"));
    assert_eq!(order.lines.len(), 1);
    let line = &order.lines[0];
    assert_eq!((line.line, line.text.as_str()), (1, "    // TODO: order"));
    assert_eq!(line.ranges, vec![7..11]);

    let mask = FileMask::parse("*.rs, !generated.rs").unwrap();
    let (files, _) = run_find(dir.path(), FindOptions { mask, ..opts("todo") });
    let rels: Vec<&str> = files.iter().map(|f| f.rel.as_str()).collect();
    assert_eq!(rels, ["src/demo/order.rs", "sub/public.rs"]);

    let (files, _) = run_find(dir.path(), FindOptions { scope: Some("src/web".into()), ..opts("todo") });
    let rels: Vec<&str> = files.iter().map(|f| f.rel.as_str()).collect();
    assert_eq!(rels, ["src/web/notes.txt"]);
}

#[test]
fn find_options_whole_word_case_regex_and_context() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "a.rs", "int order = 1;\nOrder o;\nvar orders = 2;\nreorder();\n");
    let count = |q: QueryOptions| run_find(dir.path(), FindOptions { query: q, ..Default::default() }).1.matches;
    assert_eq!(count(QueryOptions::text("order")), 4);
    assert_eq!(count(QueryOptions { pattern: "order".into(), whole_word: true, ..Default::default() }), 2);
    assert_eq!(count(QueryOptions { pattern: "Order".into(), case_sensitive: true, ..Default::default() }), 1);
    assert_eq!(count(QueryOptions { pattern: r"\w+ = \d".into(), regex: true, ..Default::default() }), 2);

    let (files, _) = run_find(dir.path(), FindOptions { query: QueryOptions { pattern: "Order o".into(), case_sensitive: true, ..Default::default() }, context_lines: 1, ..Default::default() });
    let l = &files[0].lines[0];
    assert_eq!(l.before, ["int order = 1;"]);
    assert_eq!(l.after, ["var orders = 2;"]);

    // Several matches on one line, columns in chars.
    write(dir.path(), "b.txt", "ñx x\n");
    let (files, _) = run_find(dir.path(), opts("x"));
    let b = files.iter().find(|f| f.rel == "b.txt").unwrap();
    assert_eq!(b.lines[0].ranges, [1..2, 3..4]);
}

#[test]
fn multi_line_regex_matches() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "a.rs", "class A\r\n{\r\n}\r\nclass B\n{\n}\n");
    let q = QueryOptions { pattern: r"class \w+\s*\{".into(), regex: true, ..Default::default() };
    let (files, summary) = run_find(dir.path(), FindOptions { query: q, ..Default::default() });
    assert_eq!(summary.matches, 2);
    let lines: Vec<u32> = files[0].lines.iter().map(|l| l.line).collect();
    assert_eq!(lines, [0, 3]);
    assert_eq!(files[0].lines[0].text, "class A");
    assert_eq!(files[0].lines[0].ranges, vec![0..7]);
}

#[test]
fn binary_and_huge_files_are_skipped() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "text.rs", "needle\n");
    fs::write(dir.path().join("bin.dat"), b"needle\0\x01\x02 needle").unwrap();
    write(dir.path(), "big.rs", &"needle\n".repeat(2000));
    let (files, _) = run_find(dir.path(), FindOptions { max_file_size: 1000, ..opts("needle") });
    let rels: Vec<&str> = files.iter().map(|f| f.rel.as_str()).collect();
    assert_eq!(rels, ["text.rs"]);
}

#[test]
fn result_cap_truncates() {
    let dir = tempfile::tempdir().unwrap();
    for i in 0..20 {
        write(dir.path(), &format!("f{i}.txt"), "hit hit hit\n");
    }
    let (files, summary) = run_find(dir.path(), FindOptions { max_results: 10, ..opts("hit") });
    assert!(summary.truncated);
    assert_eq!(summary.matches, 10);
    assert!(files.iter().map(FileMatches::match_count).sum::<usize>() <= 10);
}

#[test]
fn a_new_search_cancels_the_running_one() {
    let dir = tempfile::tempdir().unwrap();
    for i in 0..4000 {
        write(dir.path(), &format!("d{}/f{i}.txt", i % 40), "some text with a needle in it\n");
    }
    let inbox = Inbox::new(Waker::noop());
    let mut finder = Finder::new(dir.path(), inbox.sender());
    let first = finder.start(opts("needle")).unwrap();
    let second = finder.start(opts("needle")).unwrap();
    assert_ne!(first, second);
    assert_eq!(finder.current(), Some(second));
    let mut done = std::collections::HashMap::new();
    while done.len() < 2 {
        match inbox.recv_timeout(Duration::from_secs(30)).expect("search events") {
            FindEvent::Done { search, summary } => {
                done.insert(search, summary);
            }
            FindEvent::File { .. } => {}
        }
    }
    let (a, b) = (&done[&first], &done[&second]);
    assert!(a.cancelled);
    assert!(a.files_searched < 4000);
    assert!(!b.cancelled);
    assert_eq!(b.files_matched, 4000);

    // Dropping a handle cancels too.
    let h = find_in_files(dir.path(), opts("needle"), inbox.sender()).unwrap();
    drop(h);
}

#[test]
fn bad_patterns_fail_up_front() {
    let dir = tempfile::tempdir().unwrap();
    let inbox = Inbox::new(Waker::noop());
    let bad = FindOptions { query: QueryOptions { pattern: "(".into(), regex: true, ..Default::default() }, ..Default::default() };
    assert!(matches!(find_in_files(dir.path(), bad, inbox.sender()), Err(QueryError::InvalidRegex(_))));
}

#[test]
fn replace_on_disk_keeps_bom_and_crlf_and_buffers_get_edits() {
    let dir = tempfile::tempdir().unwrap();
    let r = dir.path();
    fs::write(r.join("a.rs"), b"\xEF\xBB\xBFvar x = GetName();\r\nvar y = GetAge();\r\n").unwrap();
    write(r, "b.rs", "nothing here\n");
    write(r, "open.rs", "GetName();\n");
    let q = Query::new(QueryOptions { pattern: r"Get(\w+)\(\)".into(), regex: true, case_sensitive: true, ..Default::default() }).unwrap();
    let out = replace_in_files(
        [
            (r.join("a.rs"), ReplaceTarget::Disk),
            (r.join("b.rs"), ReplaceTarget::Disk),
            (r.join("open.rs"), ReplaceTarget::Buffer("x\nGetName();\n".into())),
            (r.join("missing.rs"), ReplaceTarget::Disk),
        ],
        &q,
        r"$1\n",
    );
    assert!(matches!(out[0].1, ReplaceOutcome::Written(2)));
    assert!(matches!(out[1].1, ReplaceOutcome::Written(0)));
    match &out[2].1 {
        ReplaceOutcome::Edits(e) => assert_eq!(e, &[TextEdit { range: 2..11, text: "Name\n".into() }]),
        o => panic!("{o:?}"),
    }
    assert!(matches!(out[3].1, ReplaceOutcome::Failed(ReplaceError::Io(_))));
    assert_eq!(fs::read(r.join("a.rs")).unwrap(), b"\xEF\xBB\xBFvar x = Name\r\n;\r\nvar y = Age\r\n;\r\n");
    // Open buffers are not written.
    assert_eq!(fs::read_to_string(r.join("open.rs")).unwrap(), "GetName();\n");
    // No temporary files left behind.
    assert_eq!(fs::read_dir(r).unwrap().count(), 3);
}

#[test]
fn replace_refuses_non_utf8() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("latin1.txt");
    fs::write(&p, b"caf\xE9 cafe").unwrap();
    let q = Query::new(QueryOptions::text("cafe")).unwrap();
    assert!(matches!(replace_in_file(&p, &q, "x"), Err(ReplaceError::NotUtf8)));
    assert_eq!(fs::read(&p).unwrap(), b"caf\xE9 cafe");
}
