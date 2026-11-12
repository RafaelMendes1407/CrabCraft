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

    let mut app = CrabCraftApp::with_settings(Settings::default(), Waker::noop(), None);
    app.open_workspace_and_wait(&root);
    app.trust_project();
    pump(&mut app, "status", |a| a.vcs.status.is_some());
    assert_eq!(app.vcs.branch_name().as_deref(), Some("main"));

    // Edit the file: a modified line and an added one show in the gutter.
    app.open_file_and_wait(&root.join("Program.cs"));
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
    app.save_all_and_wait();
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
    let mut app = CrabCraftApp::with_settings(Settings::default(), Waker::noop(), None);
    app.open_workspace_and_wait(root);
    app.trust_project();
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

#[test]
fn selected_files_are_stashed_and_restored_on_a_new_branch_from_the_app() {
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = tempfile::tempdir().unwrap();
    // SAFETY: serialized by HOME_LOCK.
    unsafe { std::env::set_var("CRABCRAFT_HOME", home.path()) };
    let (_repo, _remote, root) = repo_with_remote();
    let mut app = open_app(&root);
    std::fs::write(root.join("lib.rs"), "pub fn selected() {}\n").unwrap();
    std::fs::write(root.join("keep.rs"), "// unselected\n").unwrap();
    app.vcs.refresh();
    pump(&mut app, "dirty", |a| a.vcs.status.as_ref().is_some_and(|s| s.entries.len() == 2));
    app.vcs.stash_push_paths("selected", false, false, vec!["lib.rs".into()]);
    assert!(app.vcs.sequence_running());
    pump(&mut app, "selected stash", |a| a.vcs.busy.is_none() && a.vcs.stashes.len() == 1);
    assert_eq!(std::fs::read_to_string(root.join("lib.rs")).unwrap(), "pub fn a() {}\n");
    assert_eq!(std::fs::read_to_string(root.join("keep.rs")).unwrap(), "// unselected\n");
    let stash = app.vcs.stashes[0].clone();
    app.vcs.stash_branch(&stash, "restore-selected");
    pump(&mut app, "branch restore", |a| a.vcs.busy.is_none() && a.vcs.branch_name().as_deref() == Some("restore-selected"));
    assert_eq!(std::fs::read_to_string(root.join("lib.rs")).unwrap(), "pub fn selected() {}\n");
    assert_eq!(app.vcs.stashes[0].hash, stash.hash);
}

#[test]
fn changelists_limit_commits_and_keep_per_list_messages_in_the_project_session() {
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = tempfile::tempdir().unwrap();
    // SAFETY: serialized by HOME_LOCK.
    unsafe { std::env::set_var("CRABCRAFT_HOME", home.path()) };
    let (_repo, _remote, root) = repo_with_remote();
    std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"changelist_fixture\"\nversion = \"0.1.0\"\n[lib]\npath = \"lib.rs\"\n").unwrap();
    git(&root, &["add", "Cargo.toml"]);
    git(&root, &["commit", "-qm", "Add Cargo manifest"]);
    let mut app = open_app(&root);
    std::fs::write(root.join("lib.rs"), "pub fn assigned() {}\n").unwrap();
    std::fs::write(root.join("other.rs"), "pub fn unrelated() {}\n").unwrap();
    app.vcs.refresh();
    pump(&mut app, "two changes", |a| a.vcs.status.as_ref().is_some_and(|s| s.entries.len() == 2));
    app.vcs.changelists.create("Library fix").unwrap();
    app.vcs.changelists.move_file("lib.rs", "Library fix").unwrap();
    assert_eq!(app.vcs.included(), vec!["other.rs"]);
    app.vcs.message = "Default draft".into();
    app.vcs.changelists.select("Library fix", &mut app.vcs.message).unwrap();
    assert_eq!(app.vcs.included(), vec!["lib.rs"]);
    app.vcs.message = "Commit assigned library".into();
    app.session().save(&root).unwrap();
    let saved = rc_core::Session::load(&root).changelists.clean();
    assert_eq!(saved.active, "Library fix");
    assert_eq!(saved.list_of("lib.rs"), "Library fix");
    assert_eq!(saved.lists.get("Library fix").map(String::as_str), Some("Commit assigned library"));
    assert_eq!(saved.lists.get("").map(String::as_str), Some("Default draft"));
    app.vcs.commit(false, None);
    pump(&mut app, "assigned commit", |a| a.vcs.busy.is_none() && a.vcs.status.as_ref().is_some_and(|s| s.entries.len() == 1));
    assert_eq!(git_out(&root, &["show", "--format=", "--name-only", "HEAD"]).trim(), "lib.rs");
    assert!(root.join("other.rs").exists());
    assert_eq!(app.vcs.changelists.list_of("lib.rs"), "");
    app.vcs.changelists.select("", &mut app.vcs.message).unwrap();
    assert_eq!(app.vcs.message, "Default draft");
    assert_eq!(app.vcs.included(), vec!["other.rs"]);
    // Exercise the native controls as well as the Git selection behavior.
    use egui_kittest::kittest::Queryable;
    use rc_kit::i18n::tr;
    app.dock.show(crate::toolwindow::ToolWindow::Commit);
    app.dock.sizes.insert(crate::toolwindow::ToolWindow::Commit, 540.0);
    app.vcs.changelist_name = "UI list".into();
    let mut h = crate::screens::harness(app, egui::vec2(1440.0, 900.0));
    h.get_by_label(tr("Manage Changelists")).click();
    h.run_steps(2);
    h.get_by_label(tr("New Changelist")).click();
    h.run_steps(2);
    assert!(h.state().vcs.changelists.lists.contains_key("UI list"));
    h.get_by(|node| node.value().as_deref() == Some(tr("Default Changelist"))).click();
    h.run_steps(2);
    h.get_by_label("UI list (0)").click();
    h.run_steps(2);
    assert_eq!(h.state().vcs.changelists.active, "UI list");
    assert!(h.state().vcs.included().is_empty());
}

#[test]
fn patches_are_created_and_applied_between_repositories_from_the_app() {
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = tempfile::tempdir().unwrap();
    // SAFETY: serialized by HOME_LOCK.
    unsafe { std::env::set_var("CRABCRAFT_HOME", home.path()) };
    let (_repo, _remote, root) = repo_with_remote();
    let other = tempfile::tempdir().unwrap();
    let b = other.path().canonicalize().unwrap();
    git(&b, &["clone", "-q", &root.display().to_string(), "."]);
    for (k, v) in [("user.name", "Other"), ("user.email", "other@example.com"), ("commit.gpgsign", "false"), ("core.hooksPath", "no-hooks")] {
        git(&b, &["config", k, v]);
    }
    let out_dir = tempfile::tempdir().unwrap();

    // Two commits in A, and local changes on top.
    let mut app = open_app(&root);
    std::fs::write(root.join("lib.rs"), "pub fn a() {}\npub fn b() {}\n").unwrap();
    git(&root, &["commit", "-qam", "Add b"]);
    std::fs::write(root.join("README.md"), "# Shop\n").unwrap();
    git(&root, &["add", "README.md"]);
    git(&root, &["commit", "-qm", "Add readme"]);
    std::fs::write(root.join("lib.rs"), "pub fn a() {}\npub fn b() {}\npub fn c() {}\n").unwrap();
    std::fs::write(root.join("notes.txt"), "untracked notes\n").unwrap();
    app.vcs.refresh();
    pump(&mut app, "log", |a| a.vcs.log.len() == 3 && a.vcs.changes().len() == 2);

    // Ctrl-click marks the two newest commits; the patch covers both, newest first.
    let (newest, older) = (app.vcs.log[0].hash.clone(), app.vcs.log[1].hash.clone());
    app.vcs.mark_commit(&newest, false, false);
    app.vcs.select_commit(&newest);
    app.vcs.mark_commit(&older, true, false);
    assert_eq!(app.vcs.patch_commits(&older), [newest.clone(), older.clone()]);
    let first = app.vcs.log[2].hash.clone();
    assert_eq!(app.vcs.patch_commits(&first).len(), 1);
    let commits_file = out_dir.path().join("commits.patch");
    app.vcs.create_patch(crate::vcs::PatchSource::Commits(app.vcs.patch_commits(&newest)), Some(commits_file.clone()));
    pump(&mut app, "commit patch saved", |a| a.vcs.busy.is_none());
    assert!(app.notices.iter().any(|n| n.text == rc_kit::i18n::tr("Patch saved")), "{:?}", app.vcs.error);
    let text = std::fs::read_to_string(&commits_file).unwrap();
    assert!(text.find("Add b").unwrap() < text.find("Add readme").unwrap());

    // The checked local changes, saved and copied.
    let changes_file = out_dir.path().join("changes.patch");
    app.vcs.create_patch(crate::vcs::PatchSource::Changes(app.vcs.included()), Some(changes_file.clone()));
    pump(&mut app, "changes patch saved", |a| a.vcs.busy.is_none());
    app.vcs.create_patch(crate::vcs::PatchSource::Changes(vec!["lib.rs".into()]), None);
    pump(&mut app, "copied", |a| a.vcs.busy.is_none() && a.vcs.clipboard.is_some());
    let copied = app.vcs.clipboard.take().unwrap();
    assert!(copied.contains("+pub fn c() {}") && !copied.contains("notes.txt"), "{copied}");

    // In B: the commits become commits, then the changes go to the work tree.
    let mut app = open_app(&b);
    app.vcs.load_patch_file(commits_file.clone());
    pump(&mut app, "commit patch checked", |a| a.vcs.patch.as_ref().is_some_and(|p| p.preview.is_some()));
    let pv = app.vcs.patch.as_ref().unwrap().preview.clone().unwrap().unwrap();
    assert_eq!((pv.kind, pv.commits, pv.problem.clone()), (rc_vcs::PatchKind::Mail, 2, None));
    let mut files: Vec<_> = pv.files.iter().map(|f| (f.path.clone(), f.status)).collect();
    files.sort();
    assert_eq!(files, [("README.md".to_string(), 'A'), ("lib.rs".to_string(), 'M')]);
    app.vcs.apply_patch();
    pump(&mut app, "commits applied", |a| a.vcs.busy.is_none() && a.vcs.patch.as_ref().is_some_and(|p| p.result.is_some()));
    assert!(!app.vcs.patch.as_ref().unwrap().result.as_ref().unwrap().error);
    assert_eq!(git_out(&b, &["log", "--format=%s"]).lines().collect::<Vec<_>>(), ["Add readme", "Add b", "First"]);

    app.vcs.load_patch_file(changes_file.clone());
    pump(&mut app, "changes checked", |a| a.vcs.patch.as_ref().is_some_and(|p| p.preview.is_some()));
    assert_eq!(app.vcs.patch.as_ref().unwrap().preview.clone().unwrap().unwrap().kind, rc_vcs::PatchKind::Diff);
    app.vcs.apply_patch();
    pump(&mut app, "changes applied", |a| a.vcs.busy.is_none() && a.vcs.patch.as_ref().is_some_and(|p| p.result.is_some()));
    assert_eq!(std::fs::read_to_string(b.join("lib.rs")).unwrap(), "pub fn a() {}\npub fn b() {}\npub fn c() {}\n");
    assert_eq!(std::fs::read_to_string(b.join("notes.txt")).unwrap(), "untracked notes\n");
    assert!(app.notices.iter().any(|n| n.text == rc_kit::i18n::tr("Patch applied")));

    // Pasted again it no longer applies, and says why; applying shows git's message.
    app.vcs.load_patch_text(copied);
    pump(&mut app, "pasted checked", |a| a.vcs.patch.as_ref().is_some_and(|p| p.preview.is_some()));
    let pv = app.vcs.patch.as_ref().unwrap().preview.clone().unwrap().unwrap();
    assert!(pv.problem.is_some(), "{pv:?}");
    app.vcs.apply_patch();
    pump(&mut app, "failed apply", |a| a.vcs.busy.is_none() && a.vcs.patch.as_ref().is_some_and(|p| p.result.is_some()));
    let r = app.vcs.patch.as_ref().unwrap().result.clone().unwrap();
    assert!(r.error && r.text.contains("lib.rs"), "{r:?}");

    // Not a patch, a missing file and an oversized file are refused with a message.
    app.vcs.load_patch_text("hello".into());
    pump(&mut app, "not a patch", |a| a.vcs.patch.as_ref().is_some_and(|p| matches!(p.preview, Some(Err(_)))));
    app.vcs.load_patch_file(out_dir.path().join("missing.patch"));
    pump(&mut app, "missing", |a| !a.vcs.patch_loading && a.vcs.patch_error.is_some());
    let big = out_dir.path().join("big.patch");
    std::fs::File::create(&big).unwrap().set_len(rc_vcs::MAX_PATCH_BYTES as u64 + 1).unwrap();
    app.vcs.load_patch_file(big);
    pump(&mut app, "too large", |a| !a.vcs.patch_loading && a.vcs.patch_error.is_some());
    assert!(app.vcs.patch_error.as_ref().unwrap().contains("32"), "{:?}", app.vcs.patch_error);
    app.vcs.close_patch();
    assert!(app.vcs.patch.is_none() && app.vcs.patch_error.is_none());
}

#[test]
fn worktrees_are_added_locked_removed_pruned_and_opened_from_the_git_window() {
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = tempfile::tempdir().unwrap();
    // SAFETY: serialized by HOME_LOCK.
    unsafe { std::env::set_var("CRABCRAFT_HOME", home.path()) };
    let (_repo, _remote, root) = repo_with_remote();
    git(&root, &["branch", "dev"]);
    let mut app = open_app(&root);
    pump(&mut app, "worktrees", |a| a.vcs.worktrees.len() == 1);
    assert_eq!(app.vcs.current_worktree().map(|w| w.main), Some(true));
    let place = tempfile::tempdir().unwrap();
    let place = place.path().canonicalize().unwrap();

    // The default folder sits next to the repository, named after it and the branch.
    let suggested = crate::ui::dialogs::suggested_worktree_path(&root, "feature/login");
    assert_eq!(suggested.parent(), root.parent());
    assert!(suggested.file_name().unwrap().to_string_lossy().ends_with("-feature-login"), "{suggested:?}");
    assert!(crate::ui::dialogs::plausible_branch_name("feature/login") && !crate::ui::dialogs::plausible_branch_name("HEAD"));

    // A new branch and an existing one.
    let (feat, dev) = (place.join("feat"), place.join("dev"));
    app.vcs.add_worktree(feat.clone(), rc_vcs::WorktreeCheckout::NewBranch { name: "feat".into(), start: None });
    pump(&mut app, "new branch worktree", |a| a.vcs.busy.is_none() && a.vcs.worktrees.iter().any(|w| w.path == feat));
    assert!(app.notices.iter().any(|n| n.text == rc_kit::i18n::tr("Worktree created")));
    assert_eq!(app.vcs.selected_worktree.as_deref(), Some(feat.as_path()));
    app.vcs.add_worktree(dev.clone(), rc_vcs::WorktreeCheckout::Branch("dev".into()));
    pump(&mut app, "existing branch worktree", |a| a.vcs.busy.is_none() && a.vcs.worktrees.iter().any(|w| w.path == dev));
    let w = app.vcs.worktrees.iter().find(|w| w.path == dev).unwrap();
    assert_eq!(w.branch.as_deref(), Some("dev"));
    assert!(root.join("lib.rs").is_file() && dev.join("lib.rs").is_file());
    // A branch already checked out elsewhere is refused with git's reason.
    app.vcs.add_worktree(place.join("again"), rc_vcs::WorktreeCheckout::Branch("dev".into()));
    pump(&mut app, "branch in use", |a| a.vcs.busy.is_none() && a.vcs.error.is_some());
    assert!(!place.join("again").exists());

    // Lock with a reason, unlock.
    app.vcs.lock_worktree(&dev, "external disk");
    pump(&mut app, "locked", |a| a.vcs.busy.is_none() && a.vcs.worktrees.iter().any(|w| w.path == dev && w.locked.as_deref() == Some("external disk")));
    app.vcs.unlock_worktree(&dev);
    pump(&mut app, "unlocked", |a| a.vcs.busy.is_none() && a.vcs.worktrees.iter().any(|w| w.path == dev && w.locked.is_none()));

    // Removing a worktree with local changes is refused and offered with force.
    std::fs::write(feat.join("scratch.txt"), "draft").unwrap();
    app.vcs.remove_worktree(&feat, false);
    pump(&mut app, "refused removal", |a| a.vcs.busy.is_none() && a.vcs.force_remove.is_some());
    assert_eq!(app.vcs.force_remove.as_deref(), Some(feat.as_path()));
    assert!(feat.join("scratch.txt").exists());
    app.vcs.force_remove = None;
    app.vcs.remove_worktree(&feat, true);
    pump(&mut app, "forced removal", |a| a.vcs.busy.is_none() && !a.vcs.worktrees.iter().any(|w| w.path == feat));
    assert!(!feat.exists());
    assert!(git_out(&root, &["branch", "--list", "feat"]).contains("feat"), "the branch stays");

    // A worktree whose folder vanished is pruned; git's report is shown.
    std::fs::remove_dir_all(&dev).unwrap();
    app.vcs.refresh();
    pump(&mut app, "prunable", |a| a.vcs.worktrees.iter().any(|w| w.path == dev && w.prunable.is_some()));
    app.vcs.prune_worktrees();
    pump(&mut app, "pruned", |a| a.vcs.busy.is_none() && a.vcs.worktrees.len() == 1 && a.vcs.output.is_some());
    assert!(!app.vcs.output.as_ref().unwrap().error);

    // Opening a worktree makes it the project of this window.
    let other = place.join("other");
    app.vcs.add_worktree(other.clone(), rc_vcs::WorktreeCheckout::Detached("HEAD".into()));
    pump(&mut app, "detached worktree", |a| a.vcs.busy.is_none() && a.vcs.worktrees.iter().any(|w| w.path == other));
    app.open_workspace_and_wait(&other);
    app.trust_project();
    pump(&mut app, "worktree opened", |a| a.vcs.repo.as_ref().is_some_and(|r| r.root() == other) && a.vcs.worktrees.len() == 2);
    let cur = app.vcs.current_worktree().unwrap();
    assert!(!cur.main && cur.detached);
}

/// V-14: the Commit window's "Sign with GPG" starts as `commit.gpgsign` says, signs with the
/// user's key, and the Log shows the signature git verified; a failing signing program is
/// reported and commits nothing.
#[test]
fn commits_are_signed_with_gpg_and_the_log_shows_the_signature() {
    if std::process::Command::new("gpg").arg("--version").output().is_err() {
        eprintln!("skipped: gpg is not installed");
        return;
    }
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = tempfile::tempdir().unwrap();
    // SAFETY: serialized by HOME_LOCK.
    unsafe { std::env::set_var("CRABCRAFT_HOME", home.path()) };
    let gnupg = home.path().join("gnupg");
    std::fs::create_dir(&gnupg).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&gnupg, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let made = std::process::Command::new("gpg")
        .env("GNUPGHOME", &gnupg)
        .args(["--batch", "--passphrase", "", "--quick-generate-key", "Signer <signer@example.com>", "ed25519", "sign", "never"])
        .output()
        .unwrap();
    assert!(made.status.success(), "{}", String::from_utf8_lossy(&made.stderr));
    let wrapper = home.path().join("gpg.sh");
    std::fs::write(&wrapper, format!("#!/bin/sh\nGNUPGHOME='{}' exec gpg \"$@\"\n", gnupg.display())).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let repo = tempfile::tempdir().unwrap();
    let root = repo.path().canonicalize().unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    for (k, v) in [("user.name", "Signer"), ("user.email", "signer@example.com"), ("core.hooksPath", "no-hooks"), ("gpg.program", &wrapper.display().to_string()), ("commit.gpgsign", "true")] {
        git(&root, &["config", k, v]);
    }
    std::fs::write(root.join("a.txt"), "a\n").unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-q", "--no-gpg-sign", "-m", "First"]);

    let mut app = CrabCraftApp::with_settings(Settings::default(), Waker::noop(), None);
    app.open_workspace_and_wait(&root);
    app.trust_project();
    pump(&mut app, "the signing settings", |a| a.vcs.signing.is_some() && a.vcs.log.len() == 1);
    assert!(app.vcs.sign, "commit.gpgsign turns it on");

    // A signed commit, from the Commit window.
    std::fs::write(root.join("a.txt"), "b\n").unwrap();
    app.vcs.refresh();
    pump(&mut app, "the change", |a| a.vcs.changes().len() == 1);
    app.vcs.message = "Signed".into();
    app.commit_changes(false);
    pump(&mut app, "the commit", |a| a.vcs.busy.is_none() && a.vcs.log.len() == 2);
    let newest = app.vcs.log[0].hash.clone();
    app.vcs.select_commit(&newest);
    pump(&mut app, "the signature", |a| a.vcs.signature.is_some());
    let sig = app.vcs.signature.clone().unwrap().1;
    let (text, good) = crate::ui::git::signature_text(&sig).unwrap();
    assert!(good && text.starts_with("Signed by Signer <signer@example.com> (key "), "{text}");
    // The first commit is not signed: nothing is said.
    let first = app.vcs.log[1].hash.clone();
    app.vcs.select_commit(&first);
    pump(&mut app, "no signature", |a| a.vcs.signature.is_some());
    assert_eq!(crate::ui::git::signature_text(&app.vcs.signature.clone().unwrap().1), None);

    // Turned off by the user, the next commit is not signed even with commit.gpgsign.
    app.vcs.sign = false;
    app.vcs.sign_touched = true;
    std::fs::write(root.join("a.txt"), "c\n").unwrap();
    app.vcs.refresh();
    pump(&mut app, "the change", |a| a.vcs.changes().len() == 1 && a.vcs.signing.is_some());
    assert!(!app.vcs.sign, "the user's choice stays");
    app.vcs.message = "Plain".into();
    app.commit_changes(false);
    pump(&mut app, "the plain commit", |a| a.vcs.busy.is_none() && a.vcs.log.len() == 3);
    let out = std::process::Command::new("git").args(["log", "-1", "--format=%G?"]).current_dir(&root).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "N");

    // A signing program that fails: the error says so, nothing is committed.
    git(&root, &["config", "gpg.program", "false"]);
    app.vcs.sign = true;
    std::fs::write(root.join("a.txt"), "d\n").unwrap();
    app.vcs.refresh();
    pump(&mut app, "the change", |a| a.vcs.changes().len() == 1);
    app.vcs.message = "Fails".into();
    app.commit_changes(false);
    pump(&mut app, "the failure", |a| a.vcs.busy.is_none() && a.vcs.error.is_some());
    assert!(app.vcs.error.as_deref().unwrap().contains("could not be signed"), "{:?}", app.vcs.error);
    assert_eq!(app.vcs.log.len(), 3);
}

/// V-06: the Log filters by message text (literal, not a pattern), author and branch; a file's
/// history follows its renames; older commits come with Load More.
#[test]
fn the_log_is_filtered_and_shows_the_history_of_a_file() {
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = tempfile::tempdir().unwrap();
    // SAFETY: serialized by HOME_LOCK.
    unsafe { std::env::set_var("CRABCRAFT_HOME", home.path()) };
    let repo = tempfile::tempdir().unwrap();
    let root = repo.path().canonicalize().unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    for (k, v) in [("user.name", "Ana"), ("user.email", "ana@example.com"), ("core.hooksPath", "no-hooks"), ("commit.gpgsign", "false")] {
        git(&root, &["config", k, v]);
    }
    let commit = |msg: &str, author: Option<&str>| {
        git(&root, &["add", "-A"]);
        let mut args = vec!["commit", "-q", "--allow-empty", "-m", msg];
        if let Some(a) = author {
            args.push("--author");
            args.push(a);
        }
        git(&root, &args);
    };
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/old.rs"), "fn a() {}\n").unwrap();
    commit("Add old.rs", None);
    std::fs::write(root.join("README.md"), "# r\n").unwrap();
    commit("Add readme (v1.0)", Some("Bruno <bruno@example.com>"));
    git(&root, &["mv", "src/old.rs", "src/new.rs"]);
    commit("Rename old to new", None);
    std::fs::write(root.join("src/new.rs"), "fn a() {}\nfn b() {}\n").unwrap();
    commit("Add b to new.rs", Some("Bruno <bruno@example.com>"));
    git(&root, &["checkout", "-q", "-b", "dev"]);
    commit("Work on dev", None);
    git(&root, &["checkout", "-q", "main"]);

    let mut app = CrabCraftApp::with_settings(Settings::default(), Waker::noop(), None);
    app.open_workspace_and_wait(&root);
    app.trust_project();
    pump(&mut app, "the log", |a| a.vcs.log.len() == 5 && !a.vcs.log_loading);
    assert!(!app.vcs.log_more);
    let subjects = |a: &CrabCraftApp| a.vcs.log.iter().map(|c| c.subject.clone()).collect::<Vec<_>>();
    let apply = |app: &mut CrabCraftApp, f: crate::vcs::LogFilter, what: &str, n: usize| {
        app.vcs.set_log_filter(f);
        pump(app, what, |a| !a.vcs.log_loading && a.vcs.log.len() == n);
    };

    // Text: literal, case-insensitive; `(v1.0)` is not a pattern.
    apply(&mut app, crate::vcs::LogFilter { text: "(V1.0)".into(), ..Default::default() }, "text filter", 1);
    assert_eq!(subjects(&app), ["Add readme (v1.0)"]);
    // Author.
    apply(&mut app, crate::vcs::LogFilter { author: "bruno".into(), ..Default::default() }, "author filter", 2);
    assert_eq!(subjects(&app), ["Add b to new.rs", "Add readme (v1.0)"]);
    // Branch: main has no dev commit.
    apply(&mut app, crate::vcs::LogFilter { branch: Some("main".into()), ..Default::default() }, "branch filter", 4);
    assert!(!subjects(&app).contains(&"Work on dev".to_string()));
    // Nothing matches: an empty log.
    apply(&mut app, crate::vcs::LogFilter { text: "no such words".into(), ..Default::default() }, "no match", 0);

    // The history of new.rs reaches back past its rename; the menu opens it in the Git window.
    app.show_git_history(&root.join("src/new.rs"));
    pump(&mut app, "file history", |a| !a.vcs.log_loading && a.vcs.log.len() == 3);
    assert_eq!(app.vcs.log_filter.path.as_deref(), Some("src/new.rs"));
    assert_eq!(app.vcs.tab, crate::vcs::GitTab::Log);
    assert_eq!(subjects(&app), ["Add b to new.rs", "Rename old to new", "Add old.rs"]);
    // A folder's history.
    app.show_git_history(&root.join("src"));
    pump(&mut app, "folder history", |a| !a.vcs.log_loading && a.vcs.log_filter.path.as_deref() == Some("src/") && a.vcs.log.len() == 3);

    // More than a page: Load More brings the rest, once.
    for i in 0..crate::vcs::LOG_PAGE {
        git(&root, &["commit", "-q", "--allow-empty", "-m", &format!("Empty {i}")]);
    }
    apply(&mut app, crate::vcs::LogFilter::default(), "first page", crate::vcs::LOG_PAGE);
    assert!(app.vcs.log_more);
    app.vcs.load_more_log();
    pump(&mut app, "second page", |a| !a.vcs.log_loading && a.vcs.log.len() == crate::vcs::LOG_PAGE + 5);
    assert!(!app.vcs.log_more);
    assert_eq!(app.vcs.log.last().unwrap().subject, "Add old.rs");
}

/// V-07: merge, rebase and cherry-pick from the app. Each stops on its conflicts, which are
/// listed and resolved, then continued, skipped or aborted; a running merge can be cancelled.
#[test]
fn merge_rebase_and_cherry_pick_stop_on_conflicts_and_go_on() {
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = tempfile::tempdir().unwrap();
    // SAFETY: serialized by HOME_LOCK.
    unsafe { std::env::set_var("CRABCRAFT_HOME", home.path()) };
    let (_repo, _remote, root) = repo_with_remote();
    git(&root, &["switch", "-q", "-c", "side"]);
    std::fs::write(root.join("lib.rs"), "pub fn side() {}\n").unwrap();
    git(&root, &["commit", "-qam", "Side change"]);
    std::fs::write(root.join("extra.rs"), "pub fn extra() {}\n").unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-qm", "Side extra"]);
    let side_extra = git_out(&root, &["rev-parse", "HEAD"]).trim().to_string();
    let side_change = git_out(&root, &["rev-parse", "HEAD~1"]).trim().to_string();
    git(&root, &["switch", "-q", "main"]);
    std::fs::write(root.join("lib.rs"), "pub fn main_change() {}\n").unwrap();
    git(&root, &["commit", "-qam", "Main change"]);
    let main_head = git_out(&root, &["rev-parse", "HEAD"]).trim().to_string();
    let mut app = open_app(&root);
    pump(&mut app, "branches", |a| a.vcs.branches.len() == 2);
    let state = |a: &CrabCraftApp| a.vcs.operation.state;

    // Merge stops on lib.rs.
    app.vcs.merge("side", rc_vcs::MergeOptions::default());
    pump(&mut app, "merge conflict", |a| a.vcs.busy.is_none() && state(a) == Some(rc_vcs::RepoState::Merging) && !a.vcs.conflicts().is_empty());
    assert_eq!(app.vcs.error.as_deref(), Some(rc_kit::i18n::tr("Stopped on conflicts: resolve them, then continue")));
    assert!(app.vcs.op_output.as_ref().is_some_and(|o| o.error && o.text.contains("CONFLICT")), "{:?}", app.vcs.op_output);
    assert_eq!(app.vcs.conflicts(), ["lib.rs"]);
    assert_eq!(crate::ui::git::operation_text(&app.vcs), "Merging side into main");
    assert_eq!(crate::ui::git::side_names(&app.vcs), ("Yours (main)".to_string(), "Theirs (side)".to_string()));
    // The file still has its markers: marking it resolved is refused.
    app.vcs.mark_resolved(vec!["lib.rs".into()]);
    pump(&mut app, "refused", |a| a.vcs.busy.is_none() && a.vcs.error.is_some());
    assert!(app.vcs.error.as_deref().unwrap().contains("conflict markers"), "{:?}", app.vcs.error);
    app.vcs.resolve_conflicts(vec!["lib.rs".into()], true);
    pump(&mut app, "resolved", |a| a.vcs.busy.is_none() && a.vcs.conflicts().is_empty() && a.vcs.status.is_some());
    app.vcs.continue_operation();
    pump(&mut app, "merge done", |a| a.vcs.busy.is_none() && state(a).is_none() && a.vcs.log.first().is_some_and(|c| c.is_merge()));
    assert_eq!(std::fs::read_to_string(root.join("lib.rs")).unwrap(), "pub fn side() {}\n");
    git(&root, &["reset", "-q", "--hard", &main_head]);
    app.vcs.refresh();

    // Rebase main onto side stops at its only commit; abort puts main back.
    app.vcs.rebase("side", false);
    // A status read while git was still writing its state may lack the step: wait for the next.
    pump(&mut app, "rebase conflict", |a| a.vcs.busy.is_none() && state(a) == Some(rc_vcs::RepoState::Rebasing) && a.vcs.operation.step.is_some() && !a.vcs.conflicts().is_empty());
    assert_eq!(app.vcs.operation.step, Some((1, 1)));
    assert_eq!(app.vcs.operation.commit.as_deref(), Some(main_head.as_str()));
    assert!(crate::ui::git::operation_text(&app.vcs).starts_with("Rebasing main onto side"), "{}", crate::ui::git::operation_text(&app.vcs));
    app.vcs.abort_operation();
    pump(&mut app, "rebase aborted", |a| a.vcs.busy.is_none() && state(a).is_none() && a.notices.iter().any(|n| n.text == "Aborted"));
    assert_eq!(git_out(&root, &["rev-parse", "HEAD"]).trim(), main_head);

    // Cherry-pick: one commit applies; one conflicts, keeps our side, is empty and skipped.
    app.vcs.cherry_pick(vec![side_extra]);
    pump(&mut app, "cherry-pick", |a| a.vcs.busy.is_none() && a.notices.iter().any(|n| n.text == "Cherry-picked"));
    assert!(root.join("extra.rs").exists());
    app.vcs.cherry_pick(vec![side_change]);
    pump(&mut app, "cherry-pick conflict", |a| a.vcs.busy.is_none() && state(a) == Some(rc_vcs::RepoState::CherryPicking) && !a.vcs.conflicts().is_empty());
    app.vcs.resolve_conflicts(vec!["lib.rs".into()], false);
    pump(&mut app, "kept ours", |a| a.vcs.busy.is_none() && a.vcs.conflicts().is_empty());
    app.vcs.continue_operation();
    pump(&mut app, "empty commit", |a| a.vcs.busy.is_none() && a.vcs.error.is_some());
    assert_eq!(app.vcs.error.as_deref(), Some("The commit is now empty; skip it or abort"));
    app.vcs.skip_operation();
    pump(&mut app, "skipped", |a| a.vcs.busy.is_none() && state(a).is_none() && a.notices.iter().any(|n| n.text == "Commit skipped"));
    assert_eq!(std::fs::read_to_string(root.join("lib.rs")).unwrap(), "pub fn main_change() {}\n");

    // A merge held up by a slow hook is cancelled: git is killed.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let hooks = root.join("slow-hooks");
        std::fs::create_dir_all(&hooks).unwrap();
        let hook = hooks.join("pre-merge-commit");
        std::fs::write(&hook, "#!/bin/sh\nsleep 20\n").unwrap();
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        git(&root, &["config", "core.hooksPath", hooks.to_str().unwrap()]);
        git(&root, &["switch", "-q", "-c", "other", "HEAD~2"]);
        std::fs::write(root.join("other.rs"), "pub fn other() {}\n").unwrap();
        git(&root, &["add", "other.rs"]);
        git(&root, &["commit", "-qm", "Other"]);
        git(&root, &["switch", "-q", "main"]);
        app.vcs.merge("other", rc_vcs::MergeOptions::default());
        assert!(app.vcs.sequence_running());
        std::thread::sleep(Duration::from_millis(300));
        let started = Instant::now();
        app.vcs.cancel_operation();
        pump(&mut app, "cancelled", |a| a.vcs.busy.is_none());
        assert!(started.elapsed() < Duration::from_secs(10));
        assert_eq!(app.vcs.error.as_deref(), Some(rc_kit::i18n::tr("Cancelled")));
        assert!(!app.vcs.sequence_running());
    }
}

/// V-08: a conflicted file opens in the three-pane merge tool with the one-sided changes taken;
/// one conflict takes a side, the other is edited by hand, and saving writes the result and
/// marks the file resolved. A file deleted on one side is refused.
#[test]
fn the_merge_tool_resolves_a_conflicted_file() {
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = tempfile::tempdir().unwrap();
    // SAFETY: serialized by HOME_LOCK.
    unsafe { std::env::set_var("CRABCRAFT_HOME", home.path()) };
    let (_repo, _remote, root) = repo_with_remote();
    let base = "fn a() {}\n\nfn b() {}\n\nfn c() {}\n\nfn d() {}\n";
    std::fs::write(root.join("lib.rs"), base).unwrap();
    std::fs::write(root.join("gone.rs"), "x\n").unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-qm", "Base"]);
    git(&root, &["switch", "-q", "-c", "side"]);
    // Theirs: changes a (conflict), c (conflict) and d (theirs only); edits gone.rs.
    std::fs::write(root.join("lib.rs"), "fn a() { theirs }\n\nfn b() {}\n\nfn c() { theirs }\n\nfn d() { theirs }\n").unwrap();
    std::fs::write(root.join("gone.rs"), "y\n").unwrap();
    git(&root, &["commit", "-qam", "Theirs"]);
    git(&root, &["switch", "-q", "main"]);
    // Ours: changes a and c differently, b only here; deletes gone.rs.
    std::fs::write(root.join("lib.rs"), "fn a() { ours }\n\nfn b() { ours }\n\nfn c() { ours }\n\nfn d() {}\n").unwrap();
    git(&root, &["rm", "-q", "gone.rs"]);
    git(&root, &["commit", "-qam", "Ours"]);
    let mut app = open_app(&root);
    app.vcs.merge("side", rc_vcs::MergeOptions::default());
    pump(&mut app, "conflicts", |a| a.vcs.busy.is_none() && a.vcs.conflicts().len() == 2 && a.vcs.operation.state.is_some());

    // A file one side deleted cannot be merged line by line.
    app.vcs.open_merge_tool("gone.rs");
    pump(&mut app, "refusal", |a| a.vcs.merge_loading.is_none() && a.vcs.error.is_some());
    assert_eq!(app.vcs.error.as_deref(), Some("One side deleted the file; accept one side instead"));
    assert!(app.vcs.merge_tool.is_none());

    app.vcs.open_merge_tool("lib.rs");
    pump(&mut app, "merge tool", |a| a.vcs.merge_tool.is_some());
    let tool = app.vcs.merge_tool.as_mut().unwrap();
    assert_eq!(tool.names, ("Yours (main)".to_string(), "Theirs (side)".to_string()));
    let kinds: Vec<_> = tool.model.chunks.iter().map(|c| (c.kind, c.resolved)).collect();
    use rc_vcs::ChunkKind::*;
    assert_eq!(kinds, [(Conflict, false), (Ours, true), (Conflict, false), (Theirs, true)]);
    assert_eq!(tool.center.buffer.rope().to_string(), "fn a() {}\n\nfn b() { ours }\n\nfn c() {}\n\nfn d() { theirs }\n");
    assert_eq!(tool.current, Some(0));
    assert_eq!(tool.left.buffer.rope().to_string(), std::fs::read_to_string(root.join("lib.rs")).ok().map(|_| "fn a() { ours }\n\nfn b() { ours }\n\nfn c() { ours }\n\nfn d() {}\n".to_string()).unwrap());

    // Both sides for the first conflict; then the second is typed by hand.
    tool.take(0, rc_vcs::Take::Both);
    assert_eq!(tool.model.conflicts_left(), 1);
    let line = tool.model.chunks[2].result.start;
    let start = tool.center.buffer.pos_at(line, 0);
    let end = tool.center.buffer.pos_at(line + 1, 0);
    tool.center.replace(start, end, "fn c() { both }\n");
    tool.sync();
    assert_eq!(tool.model.unresolved(), 0);
    let expected = "fn a() { ours }\nfn a() { theirs }\n\nfn b() { ours }\n\nfn c() { both }\n\nfn d() { theirs }\n";
    assert_eq!(tool.center.buffer.rope().to_string(), expected);
    // Undo works on the result like in any editor.
    tool.center.undo();
    tool.sync();
    assert_eq!(tool.model.conflicts_left(), 1);
    tool.center.redo();
    tool.sync();
    assert_eq!(tool.model.unresolved(), 0);

    app.vcs.save_merge_tool();
    pump(&mut app, "saved", |a| a.vcs.merge_tool.is_none() && a.vcs.conflicts() == ["gone.rs"]);
    assert_eq!(std::fs::read_to_string(root.join("lib.rs")).unwrap(), expected);
    assert!(app.notices.iter().any(|n| n.text == "lib.rs resolved"));
    app.vcs.resolve_conflicts(vec!["gone.rs".into()], false);
    pump(&mut app, "all resolved", |a| a.vcs.busy.is_none() && a.vcs.conflicts().is_empty());
    app.vcs.continue_operation();
    pump(&mut app, "merged", |a| a.vcs.busy.is_none() && a.vcs.operation.state.is_none() && a.vcs.log.first().is_some_and(|c| c.is_merge()));
    assert_eq!(git_out(&root, &["show", "HEAD:lib.rs"]), expected);
}


#[test]
fn rust_git_changes_commit_branches_blame_and_remote_operations_work_together() {
    let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = tempfile::tempdir().unwrap();
    // SAFETY: serialized by HOME_LOCK.
    unsafe { std::env::set_var("CRABCRAFT_HOME", home.path()) };
    let (_repo, remote, root) = repo_with_remote();
    std::fs::write(root.join("Cargo.toml"), "[package]\nname=\"git_rust_fixture\"\nversion=\"0.1.0\"\nedition=\"2024\"\n[lib]\npath=\"lib.rs\"\n[workspace]\n").unwrap();
    std::fs::write(root.join("lib.rs"), "pub fn answer() -> u32 {\n    1\n}\n").unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-qm", "Rust project"]);
    let mut app = open_app(&root);
    app.open_file_and_wait(&root.join("lib.rs"));
    let doc = app.docs.active_doc_mut().unwrap();
    let at = doc.editor.buffer.pos_at(1, 4);
    doc.editor.replace(at, at + 1, "2");
    pump(&mut app, "Rust gutter mark", |a| a.docs.active_doc().unwrap().editor.line_changes.get(&1) == Some(&rc_editor::LineChange::Modified));
    app.save_all_and_wait();
    app.vcs.refresh();
    pump(&mut app, "Rust changed file", |a| a.vcs.changes().iter().any(|c| c.path == "lib.rs"));
    app.vcs.open_diff("lib.rs");
    pump(&mut app, "Rust diff", |a| a.vcs.diff.is_some());
    let diff = app.vcs.diff.as_ref().unwrap();
    assert_eq!(diff.left.lang(), rc_syntax::Lang::Rust);
    assert_eq!(diff.right.lang(), rc_syntax::Lang::Rust);
    assert!(!diff.right.line_backgrounds.is_empty());
    app.vcs.message = "Rust answer edit".into();
    app.vcs.commit(false, None);
    pump(&mut app, "Rust commit", |a| a.vcs.busy.is_none() && a.vcs.log.iter().any(|c| c.subject == "Rust answer edit"));
    app.vcs.new_branch("feature/rust");
    pump(&mut app, "Rust branch", |a| a.vcs.busy.is_none() && a.vcs.branch_name().as_deref() == Some("feature/rust"));
    let text = app.docs.active_doc().unwrap().editor.buffer.rope().to_string();
    app.vcs.toggle_blame(&root.join("lib.rs"), text, &mut app.docs);
    pump(&mut app, "Rust blame", |a| a.docs.active_doc().unwrap().editor.annotations.is_some());
    assert!(app.docs.active_doc().unwrap().editor.annotations.as_ref().unwrap().iter().all(|line| line.contains("Test")));
    app.vcs.push();
    assert!(app.vcs.sequence_running(), "push must expose cancellation");
    pump(&mut app, "Rust push", |a| a.vcs.busy.is_none());
    assert!(app.vcs.error.is_none(), "{:?}", app.vcs.error);
    assert_eq!(git_out(remote.path(), &["rev-parse", "refs/heads/feature/rust"]).trim(), git_out(&root, &["rev-parse", "HEAD"]).trim());
    let peer = tempfile::tempdir().unwrap();
    git(peer.path(), &["clone", "-q", "--branch", "feature/rust", &format!("file://{}", remote.path().display()), "."]);
    git(peer.path(), &["config", "user.name", "Peer"]);
    git(peer.path(), &["config", "user.email", "peer@example.com"]);
    git(peer.path(), &["config", "commit.gpgsign", "false"]);
    std::fs::write(peer.path().join("lib.rs"), "pub fn answer() -> u32 {\n    3\n}\n").unwrap();
    git(peer.path(), &["commit", "-qam", "Remote Rust edit"]);
    git(peer.path(), &["push", "-q"]);
    let before = git_out(&root, &["rev-parse", "HEAD"]);
    app.vcs.fetch();
    assert!(app.vcs.sequence_running());
    pump(&mut app, "Rust fetch", |a| a.vcs.busy.is_none());
    assert!(app.vcs.error.is_none(), "{:?}", app.vcs.error);
    assert_eq!(git_out(&root, &["rev-parse", "HEAD"]), before, "fetch must preserve the current checkout");
    app.vcs.pull();
    assert!(app.vcs.sequence_running());
    pump(&mut app, "Rust pull", |a| a.vcs.busy.is_none());
    assert!(app.vcs.error.is_none(), "{:?}", app.vcs.error);
    assert_eq!(std::fs::read_to_string(root.join("lib.rs")).unwrap(), "pub fn answer() -> u32 {\n    3\n}\n");
    // Network entry points keep the same cancellation contract as merge operations.
    for action in [crate::vcs::Vcs::fetch, crate::vcs::Vcs::pull, crate::vcs::Vcs::push] {
        action(&mut app.vcs);
        app.vcs.cancel_operation();
        pump(&mut app, "cancelled Git remote operation", |a| a.vcs.busy.is_none());
        assert_eq!(app.vcs.error.as_deref(), Some(rc_kit::i18n::tr("Cancelled")));
    }
    let checked = std::process::Command::new("cargo").args(["test", "--offline"]).current_dir(&root).output().unwrap();
    assert!(checked.status.success(), "{}", String::from_utf8_lossy(&checked.stderr));
}
