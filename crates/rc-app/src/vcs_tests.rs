//! Phase 6 closure from the app with the real `git`: the status lists changes, edited lines
//! get gutter marks, the diff view aligns both sides, a commit and a new branch work, and
//! blame annotates every line.

use crate::app::CrabCraftApp;
use rc_core::{Settings, Waker};
use std::path::Path;
use std::time::{Duration, Instant};

fn git(dir: &Path, args: &[&str]) {
    let out = std::process::Command::new("git").args(args).current_dir(dir).output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

fn pump(app: &mut CrabCraftApp, what: &str, mut done: impl FnMut(&mut CrabCraftApp) -> bool) {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(30) {
        let n = app.vcs.update(&mut app.docs, true);
        app.notices.extend(n.into_iter().map(|text| crate::app::Notice { text, error: false, at: 0.0, url: None }));
        if done(app) {
            return;
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    panic!("timed out waiting for {what}; error {:?}", app.vcs.error);
}

#[test]
fn git_changes_marks_diff_commit_branch_and_blame() {
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = tempfile::tempdir().unwrap();
    // SAFETY: serialized by HOME_LOCK.
    unsafe { std::env::set_var("CRABCRAFT_HOME", home.path()) };
    let repo = tempfile::tempdir().unwrap();
    let root = repo.path().canonicalize().unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    git(&root, &["config", "user.name", "Test"]);
    git(&root, &["config", "user.email", "test@example.com"]);
    std::fs::write(root.join("Program.cs"), "class A\n{\n    void M() { }\n}\n").unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-q", "-m", "First"]);

    let settings = Settings { roslyn_path: Some("/nonexistent".into()), ..Settings::default() };
    let mut app = CrabCraftApp::with_settings(settings, Waker::noop(), None);
    app.open_workspace(&root);
    pump(&mut app, "status", |a| a.vcs.status.is_some());
    assert_eq!(app.vcs.branch_name().as_deref(), Some("main"));

    // Edit the file: a modified line and an added one show in the gutter.
    app.open_file(&root.join("Program.cs"));
    let doc = app.docs.active_doc_mut().unwrap();
    let pos = doc.editor.buffer.pos_at(2, 4);
    doc.editor.replace(pos, pos + 8, "void N(int x) { }\n    int y;");
    pump(&mut app, "gutter marks", |a| !a.docs.active_doc().unwrap().editor.line_changes.is_empty());
    let marks = app.docs.active_doc().unwrap().editor.line_changes.clone();
    assert_eq!(marks.get(&2), Some(&rc_editor::LineChange::Modified), "{marks:?}");
    // One line replaced by two: IntelliJ marks the whole block as modified.
    assert!(matches!(marks.get(&3), Some(rc_editor::LineChange::Modified | rc_editor::LineChange::Added)), "{marks:?}");
    assert!(!marks.contains_key(&0) && !marks.contains_key(&4), "{marks:?}");

    // Save → the status lists the change; the diff view aligns HEAD and the working tree.
    app.save_all_docs();
    app.vcs.refresh();
    pump(&mut app, "changed status", |a| a.vcs.changes().iter().any(|c| c.path == "Program.cs"));
    app.vcs.open_diff("Program.cs");
    pump(&mut app, "diff", |a| a.vcs.diff.is_some());
    let d = app.vcs.diff.as_ref().unwrap();
    assert_eq!(d.left.buffer.len_lines(), d.right.buffer.len_lines());
    assert!(!d.right.line_backgrounds.is_empty());

    // Commit, then a new branch.
    app.vcs.message = "Second".into();
    app.vcs.commit(false, None);
    pump(&mut app, "commit", |a| a.vcs.busy.is_none() && a.notices.iter().any(|n| n.text == rc_kit::i18n::tr("Committed")));
    pump(&mut app, "clean status", |a| a.vcs.status.as_ref().is_some_and(|s| s.entries.is_empty()));
    app.vcs.new_branch("feature/x");
    pump(&mut app, "branch", |a| a.vcs.branch_name().as_deref() == Some("feature/x"));
    pump(&mut app, "log", |a| a.vcs.log.iter().any(|c| c.subject == "Second"));

    // Blame annotates every line.
    let (path, text) = (root.join("Program.cs"), app.docs.active_doc().unwrap().editor.buffer.rope().to_string());
    app.vcs.toggle_blame(&path, text, &mut app.docs);
    pump(&mut app, "blame", |a| a.docs.active_doc().unwrap().editor.annotations.is_some());
    let ann = app.docs.active_doc().unwrap().editor.annotations.clone().unwrap();
    assert_eq!(ann.len(), app.docs.active_doc().unwrap().editor.buffer.len_lines() - 1);
    assert!(ann.iter().all(|a| a.contains("Test")), "{ann:?}");
}

/// A repository with one commit and a local bare remote named `origin`.
fn repo_with_remote() -> (tempfile::TempDir, tempfile::TempDir, std::path::PathBuf) {
    let repo = tempfile::tempdir().unwrap();
    let remote = tempfile::tempdir().unwrap();
    let root = repo.path().canonicalize().unwrap();
    git(remote.path(), &["init", "-q", "--bare", "-b", "main"]);
    git(&root, &["init", "-q", "-b", "main"]);
    for (k, v) in [("user.name", "Test"), ("user.email", "test@example.com"), ("tag.gpgsign", "false"), ("commit.gpgsign", "false"), ("core.hooksPath", "no-hooks")] {
        git(&root, &["config", k, v]);
    }
    git(&root, &["remote", "add", "origin", &format!("file://{}", remote.path().display())]);
    std::fs::write(root.join("lib.rs"), "pub fn a() {}\n").unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-q", "-m", "First"]);
    (repo, remote, root)
}

fn open_app(root: &Path) -> CrabCraftApp {
    let settings = Settings { roslyn_path: Some("/nonexistent".into()), ..Settings::default() };
    let mut app = CrabCraftApp::with_settings(settings, Waker::noop(), None);
    app.open_workspace(root);
    pump(&mut app, "status", |a| a.vcs.status.is_some() && !a.vcs.log.is_empty());
    app
}

fn git_out(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git").args(args).current_dir(dir).output().unwrap();
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn tags_are_created_pushed_checked_out_and_deleted_from_the_git_window() {
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = tempfile::tempdir().unwrap();
    // SAFETY: serialized by HOME_LOCK.
    unsafe { std::env::set_var("CRABCRAFT_HOME", home.path()) };
    let (_repo, remote, root) = repo_with_remote();
    let mut app = open_app(&root);
    let first = app.vcs.log[0].hash.clone();
    std::fs::write(root.join("lib.rs"), "pub fn b() {}\n").unwrap();
    git(&root, &["commit", "-qam", "Second"]);
    app.vcs.refresh();
    pump(&mut app, "remotes and log", |a| a.vcs.remotes == ["origin"] && a.vcs.log.len() == 2);

    // A lightweight tag on an older commit, an annotated one on HEAD.
    app.vcs.create_tag("v0.1", Some(first.clone()), None);
    pump(&mut app, "lightweight tag", |a| a.vcs.busy.is_none() && a.vcs.tags.iter().any(|t| t.name == "v0.1"));
    app.vcs.create_tag("v0.2", None, Some("Second release".into()));
    pump(&mut app, "annotated tag", |a| a.vcs.busy.is_none() && a.vcs.tags.iter().any(|t| t.name == "v0.2"));
    let v1 = app.vcs.tags.iter().find(|t| t.name == "v0.1").unwrap().clone();
    let v2 = app.vcs.tags.iter().find(|t| t.name == "v0.2").unwrap().clone();
    assert_eq!((v1.target.as_str(), v1.annotated), (first.as_str(), false));
    assert!(v2.annotated && v2.message == "Second release");
    assert!(app.notices.iter().any(|n| n.text == rc_kit::i18n::tr("Tag created")));

    // A bad name is refused by git's own rules, before anything runs.
    app.vcs.create_tag("bad..name", None, None);
    pump(&mut app, "refused name", |a| a.vcs.busy.is_none() && a.vcs.error.is_some());
    assert!(!crate::ui::dialogs::plausible_tag_name("bad..name") && crate::ui::dialogs::plausible_tag_name("release/1.0"));

    // Push one tag, then all; the output shows what git did.
    app.vcs.push_tags("origin", Some("v0.1".into()));
    pump(&mut app, "push one", |a| a.vcs.busy.is_none() && a.vcs.output.is_some());
    let out = app.vcs.output.clone().unwrap();
    assert!(!out.error && out.text.contains("refs/tags/v0.1"), "{out:?}");
    assert_eq!(git_out(remote.path(), &["tag", "--list"]).trim(), "v0.1");
    app.vcs.push_tags("origin", None);
    pump(&mut app, "push all", |a| a.vcs.busy.is_none() && a.vcs.output.is_some());
    assert_eq!(git_out(remote.path(), &["tag", "--list"]).lines().collect::<Vec<_>>(), ["v0.1", "v0.2"]);
    // A failing push reports git's message as output.
    app.vcs.push_tags("nowhere", Some("v0.1".into()));
    pump(&mut app, "failed push", |a| a.vcs.busy.is_none() && a.vcs.output.as_ref().is_some_and(|o| o.error));

    // Checkout detaches HEAD at the tag; delete removes it locally only.
    app.vcs.checkout_tag("v0.1");
    pump(&mut app, "detached", |a| a.vcs.busy.is_none() && a.vcs.status.as_ref().is_some_and(|s| s.branch.head.is_none()));
    assert_eq!(git_out(&root, &["rev-parse", "HEAD"]).trim(), first);
    app.vcs.delete_tag("v0.1");
    pump(&mut app, "deleted", |a| a.vcs.busy.is_none() && !a.vcs.tags.iter().any(|t| t.name == "v0.1"));
    assert!(git_out(remote.path(), &["tag", "--list"]).contains("v0.1"));
}

#[test]
fn stashes_are_made_shown_applied_popped_and_dropped_from_the_git_window() {
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = tempfile::tempdir().unwrap();
    // SAFETY: serialized by HOME_LOCK.
    unsafe { std::env::set_var("CRABCRAFT_HOME", home.path()) };
    let (_repo, _remote, root) = repo_with_remote();
    let mut app = open_app(&root);
    let clean = |a: &CrabCraftApp| a.vcs.busy.is_none() && a.vcs.status.as_ref().is_some_and(|s| s.entries.is_empty());

    // Stash a modified file and an untracked one, with a message.
    std::fs::write(root.join("lib.rs"), "pub fn changed() {}\n").unwrap();
    std::fs::write(root.join("new.rs"), "pub fn new() {}\n").unwrap();
    app.vcs.refresh();
    pump(&mut app, "dirty", |a| a.vcs.status.as_ref().is_some_and(|s| s.entries.len() == 2));
    app.vcs.stash_push("  work in progress ", true, false);
    pump(&mut app, "stashed", |a| clean(a) && a.vcs.stashes.len() == 1);
    assert!(app.notices.iter().any(|n| n.text == rc_kit::i18n::tr("Changes stashed")));
    assert_eq!(std::fs::read_to_string(root.join("lib.rs")).unwrap(), "pub fn a() {}\n");
    assert!(!root.join("new.rs").exists());
    let st = app.vcs.stashes[0].clone();
    assert_eq!((st.index, st.message.as_str(), st.branch.as_deref()), (0, "On main: work in progress", Some("main")));

    // Its contents: the changed file, the untracked one, and a diff of each.
    app.vcs.select_stash(&st.hash);
    pump(&mut app, "stash files", |a| a.vcs.stash_files.len() == 2);
    let files: Vec<_> = app.vcs.stash_files.iter().map(|f| (f.status, f.path.as_str())).collect();
    assert_eq!(files, [('M', "lib.rs"), ('?', "new.rs")]);
    let changed = app.vcs.stash_files[0].clone();
    app.vcs.open_stash_diff(&st.hash, &changed);
    pump(&mut app, "stash diff", |a| a.vcs.diff.is_some());
    let untracked = app.vcs.stash_files[1].clone();
    app.vcs.diff = None;
    app.vcs.open_stash_diff(&st.hash, &untracked);
    pump(&mut app, "untracked diff", |a| a.vcs.diff.is_some());

    // Apply keeps the stash; Pop drops it.
    app.vcs.stash_apply(&st, false);
    pump(&mut app, "applied", |a| a.vcs.busy.is_none() && a.vcs.status.as_ref().is_some_and(|s| s.entries.len() == 2));
    assert_eq!(std::fs::read_to_string(root.join("lib.rs")).unwrap(), "pub fn changed() {}\n");
    assert_eq!(app.vcs.stashes.len(), 1);
    git(&root, &["checkout", "--", "lib.rs"]);
    std::fs::remove_file(root.join("new.rs")).unwrap();
    app.vcs.refresh();
    pump(&mut app, "clean again", |a| clean(a));
    app.vcs.stash_apply(&st, true);
    pump(&mut app, "popped", |a| a.vcs.busy.is_none() && a.vcs.stashes.is_empty() && a.vcs.status.as_ref().is_some_and(|s| s.entries.len() == 2));
    assert!(app.vcs.selected_stash.is_none() && app.vcs.stash_files.is_empty());

    // Drop, after a check that the entry is still the one shown.
    app.vcs.stash_push("", false, false);
    pump(&mut app, "stashed again", |a| a.vcs.busy.is_none() && a.vcs.stashes.len() == 1);
    let old = app.vcs.stashes[0].clone();
    assert!(old.message.starts_with("WIP on main"), "{old:?}");
    git(&root, &["stash", "push", "--include-untracked", "-m", "from outside"]);
    app.vcs.stash_drop(&old);
    pump(&mut app, "stale drop refused", |a| a.vcs.busy.is_none() && a.vcs.error.is_some());
    assert_eq!(git_out(&root, &["stash", "list"]).lines().count(), 2);
    pump(&mut app, "both listed", |a| a.vcs.stashes.len() == 2);
    let old = app.vcs.stashes.iter().find(|s| s.hash == old.hash).unwrap().clone();
    assert_eq!(old.index, 1);
    app.vcs.stash_drop(&old);
    pump(&mut app, "dropped", |a| a.vcs.busy.is_none() && a.vcs.stashes.len() == 1);
    assert!(git_out(&root, &["stash", "list"]).contains("from outside"));

    // Nothing to stash is reported, not a crash.
    app.vcs.stash_push("empty", false, false);
    pump(&mut app, "nothing to stash", |a| a.vcs.busy.is_none() && a.vcs.error.is_some());
}
