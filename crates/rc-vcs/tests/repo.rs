//! Integration tests against the real `git` in temporary repositories.

mod common;

use common::{TempRepo, git, numbered};
use rc_vcs::*;
use std::sync::{Arc, Mutex};

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| x.to_string()).collect()
}

#[tokio::test]
async fn discovers_repository_from_nested_paths() {
    let t = TempRepo::new().await;
    t.write("src/deep/file.cs", "x\n");
    let from_file = Repository::discover(&t.path("src/deep/file.cs")).await.unwrap();
    assert_eq!(from_file.root(), t.repo.root());
    assert!(from_file.git_dir().ends_with(".git"));
    assert_eq!(from_file.rel_path(&t.path("src/deep/file.cs")).as_deref(), Some("src/deep/file.cs"));
    assert!(Git::detect().await.unwrap().version() >= git::MIN_VERSION);

    let outside = tempfile::tempdir().unwrap();
    match Repository::discover(outside.path()).await {
        Err(GitError::NotARepository(_)) => {}
        other => panic!("expected NotARepository, got {other:?}"),
    }
}

#[tokio::test]
async fn status_reports_branch_files_renames_and_untracked() {
    let t = TempRepo::new().await;
    let st = t.repo.status(&StatusOptions::default()).await.unwrap();
    assert_eq!(st.branch.head.as_deref(), Some("main"));
    assert_eq!(st.branch.oid, None);

    t.commit_files(&[("a.txt", "a\n"), ("b.txt", &numbered(20)), ("gone.txt", "g\n")], "init");
    t.write("a.txt", "a changed\n");
    t.write("new dir/new file.txt", "n\n");
    t.git(&["mv", "b.txt", "c.txt"]);
    t.git(&["rm", "-q", "gone.txt"]);
    t.write(".gitignore", "*.log\n");
    t.write("x.log", "ignored\n");

    let st = t.repo.status(&StatusOptions { ignored: true, ..Default::default() }).await.unwrap();
    assert!(st.branch.oid.is_some());
    let a = st.entry("a.txt").unwrap();
    assert_eq!((a.index, a.worktree), (FileState::Unmodified, FileState::Modified));
    let c = st.entry("c.txt").unwrap();
    assert_eq!((c.index, c.orig_path.as_deref()), (FileState::Renamed, Some("b.txt")));
    assert_eq!(st.entry("gone.txt").unwrap().index, FileState::Deleted);
    assert!(st.entry("new dir/new file.txt").unwrap().is_untracked());
    assert!(st.ignored().any(|e| e.path == "x.log"));
    assert_eq!(st.staged().count(), 2);
    assert_eq!(st.unstaged().count(), 1);
}

#[tokio::test]
async fn diffs_parse_for_every_spec() {
    let t = TempRepo::new().await;
    t.commit_files(&[("f.txt", &numbered(30))], "one");
    let first = t.repo.resolve("HEAD").await.unwrap();
    let mut text = numbered(30).replace("line 3\n", "line three\n");
    t.write("f.txt", &text);
    t.git(&["commit", "-qam", "two"]);
    text = text.replace("line 20\n", "line twenty\n");
    t.write("f.txt", &text);
    t.git(&["add", "f.txt"]);
    text.push_str("line 31\n");
    t.write("f.txt", &text);

    let opts = DiffOptions::default();
    let unstaged = t.repo.diff(&DiffSpec::WorktreeVsIndex, &opts).await.unwrap();
    assert_eq!(unstaged.len(), 1);
    let h = &unstaged[0].hunks[0];
    assert_eq!(h.lines.iter().filter(|l| l.kind == DiffLineKind::Added).count(), 1);
    assert_eq!(h.lines.last().unwrap().new_line, Some(31));

    let staged = t.repo.diff(&DiffSpec::IndexVsHead, &opts).await.unwrap();
    assert_eq!(staged[0].hunks.len(), 1);
    assert!(staged[0].hunks[0].lines.iter().any(|l| l.text == "line twenty" && l.new_line == Some(20)));

    let all = t.repo.diff(&DiffSpec::WorktreeVsHead, &opts).await.unwrap();
    assert_eq!(all[0].hunks.len(), 2);

    let between = t.repo.diff(&DiffSpec::Revisions { from: first.clone(), to: "HEAD".into() }, &opts).await.unwrap();
    assert!(between[0].hunks[0].lines.iter().any(|l| l.text == "line three"));

    let root = t.repo.diff(&DiffSpec::Commit(first.clone()), &opts).await.unwrap();
    assert_eq!(root[0].status, DiffStatus::Added);
    assert_eq!(root[0].hunks[0].lines.len(), 30);
    let second = t.repo.diff(&DiffSpec::Commit("HEAD".into()), &opts).await.unwrap();
    assert_eq!(second[0].hunks[0].lines.iter().filter(|l| l.kind != DiffLineKind::Context).count(), 2);

    let one_file = t.repo.diff(&DiffSpec::WorktreeVsHead, &DiffOptions { paths: s(&["nope.txt"]), ..Default::default() }).await.unwrap();
    assert!(one_file.is_empty());

    t.write("untracked.txt", "u1\nu2\n");
    let u = t.repo.diff_untracked("untracked.txt", 3).await.unwrap().unwrap();
    assert_eq!((u.status, u.path(), u.hunks[0].lines.len()), (DiffStatus::Added, "untracked.txt", 2));
}

#[tokio::test]
async fn renames_binaries_and_file_contents() {
    let t = TempRepo::new().await;
    t.commit_files(&[("old.txt", &numbered(10)), ("crlf.txt", "a\r\nb\r\n")], "init");
    std::fs::write(t.path("bin.dat"), [0u8, 1, 2, 3, 0, 255]).unwrap();
    t.git(&["mv", "old.txt", "new.txt"]);
    t.git(&["add", "bin.dat"]);
    let d = t.repo.diff(&DiffSpec::IndexVsHead, &DiffOptions::default()).await.unwrap();
    let bin = d.iter().find(|f| f.path() == "bin.dat").unwrap();
    assert!(bin.binary);
    let ren = d.iter().find(|f| f.status == DiffStatus::Renamed).unwrap();
    assert_eq!((ren.old_path.as_deref(), ren.new_path.as_deref(), ren.similarity), (Some("old.txt"), Some("new.txt"), Some(100)));

    let head = t.repo.show_file(&Revision::Head, "old.txt").await.unwrap().unwrap();
    assert_eq!(head.text(), numbered(10));
    assert!(t.repo.show_file(&Revision::Head, "new.txt").await.unwrap().is_none());
    assert!(t.repo.show_file(&Revision::Index, "new.txt").await.unwrap().is_some());
    assert!(t.repo.show_file(&Revision::Index, "bin.dat").await.unwrap().unwrap().binary);
    assert_eq!(t.repo.show_file(&Revision::Head, "crlf.txt").await.unwrap().unwrap().text(), "a\r\nb\r\n");
    assert!(t.repo.show_file(&Revision::Commit("HEAD".into()), "missing").await.unwrap().is_none());
}

#[tokio::test]
async fn stage_unstage_and_discard_hunks_and_lines() {
    let t = TempRepo::new().await;
    let base = numbered(40);
    t.commit_files(&[("f.txt", &base)], "init");
    let edited = base.replace("line 2\n", "line TWO\n").replace("line 30\n", "line THIRTY\nline 30b\n");
    t.write("f.txt", &edited);

    // Stage only the second hunk.
    let d = t.repo.diff(&DiffSpec::WorktreeVsIndex, &DiffOptions::default()).await.unwrap();
    assert_eq!(d[0].hunks.len(), 2);
    t.repo.stage_hunk(&d[0], 1).await.unwrap();
    let staged = t.repo.show_file(&Revision::Index, "f.txt").await.unwrap().unwrap();
    assert_eq!(staged.text(), base.replace("line 30\n", "line THIRTY\nline 30b\n"));

    // Unstage it again.
    let sd = t.repo.diff(&DiffSpec::IndexVsHead, &DiffOptions::default()).await.unwrap();
    t.repo.unstage_hunk(&sd[0], 0).await.unwrap();
    assert_eq!(t.repo.show_file(&Revision::Index, "f.txt").await.unwrap().unwrap().text(), base);

    // Stage a single added line of the second hunk ("line 30b").
    let d = t.repo.diff(&DiffSpec::WorktreeVsIndex, &DiffOptions::default()).await.unwrap();
    let h = &d[0].hunks[1];
    let idx = h.lines.iter().position(|l| l.text == "line 30b").unwrap();
    t.repo.stage_lines(&d[0], 1, &[idx]).await.unwrap();
    assert_eq!(t.repo.show_file(&Revision::Index, "f.txt").await.unwrap().unwrap().text(), base.replace("line 30\n", "line 30\nline 30b\n"));

    // Unstage that line through the staged diff.
    let sd = t.repo.diff(&DiffSpec::IndexVsHead, &DiffOptions::default()).await.unwrap();
    let idx = sd[0].hunks[0].lines.iter().position(|l| l.kind == DiffLineKind::Added).unwrap();
    t.repo.unstage_lines(&sd[0], 0, &[idx]).await.unwrap();
    assert_eq!(t.repo.show_file(&Revision::Index, "f.txt").await.unwrap().unwrap().text(), base);

    // Discard the first hunk in the work tree, keep the second.
    let d = t.repo.diff(&DiffSpec::WorktreeVsIndex, &DiffOptions::default()).await.unwrap();
    t.repo.discard_hunk(&d[0], 0).await.unwrap();
    assert_eq!(t.read("f.txt"), base.replace("line 30\n", "line THIRTY\nline 30b\n"));

    // Discard just the "line THIRTY" replacement (its removed and added lines).
    let d = t.repo.diff(&DiffSpec::WorktreeVsIndex, &DiffOptions::default()).await.unwrap();
    let h = &d[0].hunks[0];
    let sel: Vec<usize> =
        h.lines.iter().enumerate().filter(|(_, l)| l.text == "line 30" && l.kind == DiffLineKind::Removed || l.text == "line THIRTY").map(|(i, _)| i).collect();
    t.repo.discard_lines(&d[0], 0, &sel).await.unwrap();
    assert_eq!(t.read("f.txt"), base.replace("line 30\n", "line 30\nline 30b\n"));

    // Whole-file discard and rollback.
    t.repo.discard_unstaged(&s(&["f.txt"])).await.unwrap();
    assert_eq!(t.read("f.txt"), base);
}

#[tokio::test]
async fn stage_files_rollback_and_delete_untracked() {
    let t = TempRepo::new().await;
    t.commit_files(&[("a.txt", "a\n"), ("b.txt", "b\n")], "init");
    t.write("a.txt", "A\n");
    std::fs::remove_file(t.path("b.txt")).unwrap();
    t.write("n.txt", "new\n");
    t.repo.stage(&s(&["a.txt", "b.txt", "n.txt"])).await.unwrap();
    let st = t.repo.status(&StatusOptions::default()).await.unwrap();
    assert_eq!(st.staged().count(), 3);

    t.repo.unstage(&s(&["a.txt"])).await.unwrap();
    let st = t.repo.status(&StatusOptions::default()).await.unwrap();
    assert!(st.entry("a.txt").unwrap().is_unstaged() && !st.entry("a.txt").unwrap().is_staged());

    t.repo.rollback(&s(&["a.txt", "b.txt", "n.txt"])).await.unwrap();
    assert_eq!(t.read("a.txt"), "a\n");
    assert_eq!(t.read("b.txt"), "b\n");
    // The added file is unstaged, not deleted.
    assert_eq!(t.read("n.txt"), "new\n");
    let st = t.repo.status(&StatusOptions::default()).await.unwrap();
    assert_eq!(st.entries.len(), 1);
    assert!(st.entry("n.txt").unwrap().is_untracked());

    t.repo.delete_untracked(&s(&["n.txt"])).await.unwrap();
    assert!(!t.path("n.txt").exists());
}

#[tokio::test]
async fn commit_amend_signoff_and_only_selected_files() {
    let t = TempRepo::new().await;
    match t.repo.commit(&CommitOptions { message: "empty".into(), ..Default::default() }).await {
        Err(GitError::NothingToCommit) => {}
        other => panic!("expected NothingToCommit, got {other:?}"),
    }
    t.write("a.txt", "a\n");
    t.repo.stage(&s(&["a.txt"])).await.unwrap();
    let h1 = t.repo.commit(&CommitOptions { message: "First\n\nBody".into(), signoff: true, ..Default::default() }).await.unwrap();
    let msg = t.repo.commit_message("HEAD").await.unwrap();
    assert!(msg.starts_with("First\n\nBody") && msg.contains("Signed-off-by: Test User <test@example.com>"), "{msg}");

    let h2 = t.repo.commit(&CommitOptions { message: "First, amended".into(), amend: true, ..Default::default() }).await.unwrap();
    assert_ne!(h1, h2);
    assert_eq!(t.repo.log(&LogOptions::default()).await.unwrap().len(), 1);

    // Only selected files: b (tracked, modified) and c (untracked); a stays staged.
    t.commit_files(&[("b.txt", "b\n")], "b");
    t.write("a.txt", "a2\n");
    t.repo.stage(&s(&["a.txt"])).await.unwrap();
    t.write("b.txt", "b2\n");
    t.write("c.txt", "c\n");
    t.repo.commit(&CommitOptions { message: "b and c".into(), only: s(&["b.txt", "c.txt"]), ..Default::default() }).await.unwrap();
    let files = t.repo.commit_files("HEAD").await.unwrap();
    let mut names: Vec<_> = files.iter().map(|f| (f.status, f.path.as_str())).collect();
    names.sort();
    assert_eq!(names, vec![('A', "c.txt"), ('M', "b.txt")]);
    let st = t.repo.status(&StatusOptions::default()).await.unwrap();
    assert_eq!(st.staged().map(|e| e.path.as_str()).collect::<Vec<_>>(), vec!["a.txt"]);
}

#[tokio::test]
async fn branches_create_checkout_rename_delete() {
    let t = TempRepo::new().await;
    t.commit_files(&[("a.txt", "a\n")], "init");
    t.repo.create_branch("feature", None, true).await.unwrap();
    assert_eq!(t.repo.current_branch().await.unwrap().as_deref(), Some("feature"));
    t.commit_files(&[("f.txt", "f\n")], "feature work");
    match t.repo.create_branch("feature", None, false).await {
        Err(GitError::BranchExists(b)) => assert_eq!(b, "feature"),
        other => panic!("expected BranchExists, got {other:?}"),
    }
    t.repo.checkout_branch("main").await.unwrap();
    match t.repo.delete_branch("feature", false).await {
        Err(GitError::BranchNotFullyMerged(b)) => assert_eq!(b, "feature"),
        other => panic!("expected BranchNotFullyMerged, got {other:?}"),
    }
    t.repo.rename_branch("feature", "topic").await.unwrap();
    let branches = t.repo.branches().await.unwrap();
    let names: Vec<_> = branches.iter().map(|b| (b.name.as_str(), b.is_head, b.kind)).collect();
    assert_eq!(names, vec![("main", true, BranchKind::Local), ("topic", false, BranchKind::Local)]);
    t.repo.delete_branch("topic", true).await.unwrap();
    assert_eq!(t.repo.branches().await.unwrap().len(), 1);
    assert!(matches!(t.repo.checkout_branch("nope").await, Err(GitError::NotFound(_))));

    // Detached HEAD.
    let head = t.repo.resolve("HEAD").await.unwrap();
    t.repo.checkout_detached(&head).await.unwrap();
    assert_eq!(t.repo.current_branch().await.unwrap(), None);
    let st = t.repo.status(&StatusOptions::default()).await.unwrap();
    assert_eq!(st.branch.head, None);

    // Local changes that a checkout would overwrite.
    t.repo.checkout_branch("main").await.unwrap();
    t.repo.create_branch("other", None, true).await.unwrap();
    t.commit_files(&[("a.txt", "other\n")], "other");
    t.repo.checkout_branch("main").await.unwrap();
    t.write("a.txt", "dirty\n");
    match t.repo.checkout_branch("other").await {
        Err(GitError::LocalChangesWouldBeOverwritten(files)) => assert_eq!(files, vec!["a.txt".to_string()]),
        other => panic!("expected LocalChangesWouldBeOverwritten, got {other:?}"),
    }
}

#[tokio::test]
async fn stash_push_list_apply_pop_drop() {
    let t = TempRepo::new().await;
    t.commit_files(&[("a.txt", "a\n")], "init");
    assert!(matches!(t.repo.stash_push(&StashOptions::default()).await, Err(GitError::NothingToStash)));
    t.write("a.txt", "one\n");
    t.repo.stash_push(&StashOptions { message: Some("first".into()), ..Default::default() }).await.unwrap();
    t.write("a.txt", "two\n");
    t.write("u.txt", "untracked\n");
    t.repo.stash_push(&StashOptions { message: Some("second".into()), include_untracked: true, ..Default::default() }).await.unwrap();
    assert_eq!(t.read("a.txt"), "a\n");
    assert!(!t.path("u.txt").exists());

    let list = t.repo.stashes().await.unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!((list[0].index, list[0].message.as_str(), list[0].branch.as_deref()), (0, "On main: second", Some("main")));
    assert_eq!(t.repo.status(&StatusOptions::default()).await.unwrap().stash_count, 2);

    // The files of each stash: tracked changes, and untracked files saved with them.
    let files = |f: Vec<rc_vcs::ChangedFile>| f.into_iter().map(|f| (f.status, f.path)).collect::<Vec<_>>();
    assert_eq!(files(t.repo.stash_files(&list[1].hash).await.unwrap()), [('M', "a.txt".to_string())]);
    assert_eq!(files(t.repo.stash_files(&list[0].hash).await.unwrap()), [('M', "a.txt".to_string()), ('?', "u.txt".to_string())]);
    t.repo.check_stash(1, &list[1].hash).await.unwrap();
    assert!(matches!(t.repo.check_stash(0, &list[1].hash).await, Err(GitError::StashChanged)));
    assert!(matches!(t.repo.check_stash(7, &list[1].hash).await, Err(GitError::StashChanged)));

    t.repo.stash_apply(1, false).await.unwrap();
    assert_eq!(t.read("a.txt"), "one\n");
    t.repo.rollback(&s(&["a.txt"])).await.unwrap();
    t.repo.stash_pop(0, false).await.unwrap();
    assert_eq!(t.read("a.txt"), "two\n");
    assert!(t.path("u.txt").exists());
    t.repo.stash_drop(0).await.unwrap();
    assert!(t.repo.stashes().await.unwrap().is_empty());
}

#[tokio::test]
async fn stash_selected_literal_paths_and_restore_on_original_base_branch() {
    let t = TempRepo::new().await;
    t.commit_files(
        &[
            ("Cargo.toml", "[package]\nname = \"stash_fixture\"\nversion = \"0.1.0\"\n"),
            ("src/lib.rs", "pub fn original() {}\n"),
            ("other.rs", "// original\n"),
            ("a[1].rs", "// original\n"),
            ("a1.rs", "// original\n"),
        ],
        "init",
    );
    let base = t.repo.resolve("HEAD").await.unwrap();
    t.write("src/lib.rs", "pub fn changed() {}\n");
    t.write("other.rs", "// keep local\n");
    t.write("a[1].rs", "// selected literal\n");
    t.write("a1.rs", "// keep local too\n");
    t.write("new file.rs", "// untracked selected\n");
    let opts = StashOptions { include_untracked: true, paths: s(&["src/lib.rs", "a[1].rs", "new file.rs"]), ..Default::default() };
    let cancel = CancelToken::new();
    cancel.cancel();
    assert!(matches!(t.repo.stash_push_cancellable(&opts, Some(&cancel)).await, Err(GitError::Cancelled)));
    assert_eq!(t.read("src/lib.rs"), "pub fn changed() {}\n");
    t.repo.stash_push_cancellable(&opts, None).await.unwrap();
    assert_eq!(t.read("src/lib.rs"), "pub fn original() {}\n");
    assert_eq!(t.read("a[1].rs"), "// original\n");
    assert_eq!(t.read("other.rs"), "// keep local\n");
    assert_eq!(t.read("a1.rs"), "// keep local too\n");
    assert!(!t.path("new file.rs").exists());
    let stash = t.repo.stashes().await.unwrap()[0].clone();
    t.git(&["add", "."]);
    t.git(&["commit", "-qm", "advance main"]);
    assert!(matches!(t.repo.stash_branch(&stash, "--bad", None).await, Err(GitError::InvalidBranchName(_))));
    assert!(matches!(t.repo.stash_branch(&stash, "cancelled", Some(&cancel)).await, Err(GitError::Cancelled)));
    t.repo.stash_branch(&stash, "restored", None).await.unwrap();
    assert_eq!(t.repo.resolve("HEAD").await.unwrap(), base);
    assert_eq!(t.read("src/lib.rs"), "pub fn changed() {}\n");
    assert_eq!(t.read("a[1].rs"), "// selected literal\n");
    assert_eq!(t.read("other.rs"), "// original\n");
    assert_eq!(t.read("new file.rs"), "// untracked selected\n");
    assert_eq!(t.repo.stashes().await.unwrap()[0].hash, stash.hash);
    let mut stale = stash.clone();
    stale.index += 1;
    assert!(matches!(t.repo.stash_branch(&stale, "stale", None).await, Err(GitError::StashChanged)));
}

/// Two repositories with the same first commit (`b` is a clone of `a`).
async fn twin_repos() -> (TempRepo, TempRepo) {
    let a = TempRepo::new().await;
    a.commit_files(&[("a.txt", &numbered(5)), ("src/lib.rs", "pub fn f() {}\n"), ("old.txt", "old\n")], "init");
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["clone", "-q", &a.repo.root().display().to_string(), "."]);
    common::configure(dir.path());
    let repo = Repository::discover(dir.path()).await.unwrap().with_trust(true);
    (a, TempRepo { dir, repo })
}

#[tokio::test]
async fn patches_of_local_changes_round_trip_between_repositories() {
    let (a, b) = twin_repos().await;
    // Staged and unstaged edits, a deletion, a rename, a binary and an untracked file.
    a.write("a.txt", &numbered(5).replace("line 3", "line three"));
    a.git(&["rm", "-q", "old.txt"]);
    a.git(&["mv", "src/lib.rs", "src/core.rs"]);
    std::fs::write(a.path("logo.bin"), [0u8, 159, 146, 150, 0, 1]).unwrap();
    a.write("new.txt", "brand new\n");
    a.write("skip.txt", "not selected\n");
    let paths = s(&["a.txt", "old.txt", "src/lib.rs", "src/core.rs", "logo.bin", "new.txt"]);
    let patch = a.repo.diff_patch(&paths).await.unwrap();
    assert_eq!(patch_kind(&patch), PatchKind::Diff);
    let text = String::from_utf8_lossy(&patch);
    assert!(!text.contains("skip.txt"), "{text}");
    assert!(matches!(a.repo.diff_patch(&s(&["skip-nothing.txt"])).await, Err(GitError::NothingToCommit)));

    let preview = b.repo.preview_patch(&patch, false).await.unwrap();
    assert_eq!(preview.problem, None);
    let mut files: Vec<_> = preview.files.iter().map(|f| (f.path.as_str(), f.status)).collect();
    files.sort();
    assert_eq!(files, [("a.txt", 'M'), ("logo.bin", 'A'), ("new.txt", 'A'), ("old.txt", 'D'), ("src/core.rs", 'R')]);
    assert!(preview.files.iter().any(|f| f.path == "logo.bin" && f.added.is_none()));

    let report = b.repo.apply_patch(&patch, false).await.unwrap();
    assert!(report.contains("a.txt"), "{report}");
    assert_eq!(b.read("a.txt"), a.read("a.txt"));
    assert_eq!(b.read("new.txt"), "brand new\n");
    assert_eq!(std::fs::read(b.path("logo.bin")).unwrap(), [0u8, 159, 146, 150, 0, 1]);
    assert!(!b.path("old.txt").exists() && !b.path("src/lib.rs").exists() && b.path("src/core.rs").exists());

    // Applied again, the check fails and says why; applying fails with git's message.
    let again = b.repo.preview_patch(&patch, false).await.unwrap();
    assert!(again.problem.as_deref().is_some_and(|p| !p.is_empty()), "{again:?}");
    assert!(matches!(b.repo.apply_patch(&patch, false).await, Err(GitError::PatchFailed(m)) if !m.is_empty()));
    // Text that is not a patch.
    assert!(matches!(b.repo.preview_patch(b"hello\nworld\n", false).await, Err(GitError::PatchFailed(_))));
    assert!(b.repo.apply_patch(b"hello\n", false).await.is_err());
}

#[tokio::test]
async fn three_way_apply_leaves_conflicts_to_resolve() {
    let (a, b) = twin_repos().await;
    a.write("a.txt", &numbered(5).replace("line 3", "ours"));
    let patch = a.repo.diff_patch(&s(&["a.txt"])).await.unwrap();
    b.commit_files(&[("a.txt", &numbered(5).replace("line 3", "theirs"))], "diverge");
    assert!(b.repo.preview_patch(&patch, false).await.unwrap().problem.is_some());
    assert!(matches!(b.repo.apply_patch(&patch, true).await, Err(GitError::Conflicts(_))));
    assert_eq!(b.repo.conflicted_paths().await.unwrap(), ["a.txt"]);
    assert!(b.read("a.txt").contains("<<<<<<<"));
}

#[tokio::test]
async fn commit_patches_become_commits_with_am() {
    let (a, b) = twin_repos().await;
    a.commit_files(&[("a.txt", &numbered(6))], "Add line 6");
    a.commit_files(&[("a.txt", &numbered(7)), ("b.txt", "b\n")], "Add line 7 and b");
    let log = a.repo.log(&LogOptions::default()).await.unwrap();
    let (newest, older) = (log[0].hash.clone(), log[1].hash.clone());
    // Given newest first, like the log; written oldest first.
    let patch = a.repo.format_patch(&[newest, older]).await.unwrap();
    assert_eq!(patch_kind(&patch), PatchKind::Mail);
    let text = String::from_utf8_lossy(&patch);
    assert!(text.find("Add line 6").unwrap() < text.find("Add line 7").unwrap());
    let preview = b.repo.preview_patch(&patch, false).await.unwrap();
    assert_eq!((preview.kind, preview.commits, preview.problem.clone()), (PatchKind::Mail, 2, None));
    let a_file = preview.files.iter().find(|f| f.path == "a.txt").unwrap();
    assert_eq!((a_file.added, a_file.removed), (Some(2), Some(0)));

    b.repo.apply_patch(&patch, false).await.unwrap();
    let blog = b.repo.log(&LogOptions::default()).await.unwrap();
    assert_eq!((blog[0].subject.as_str(), blog[1].subject.as_str()), ("Add line 7 and b", "Add line 6"));
    assert_eq!(blog[0].author_email, "test@example.com");
    assert_eq!(b.read("a.txt"), numbered(7));

    // A failing series is aborted: no commit, no `am` in progress.
    let head = b.repo.resolve("HEAD").await.unwrap();
    assert!(matches!(b.repo.apply_patch(&patch, false).await, Err(GitError::PatchFailed(_))));
    assert_eq!(b.repo.state().await, RepoState::Clean);
    assert_eq!(b.repo.resolve("HEAD").await.unwrap(), head);
    assert!(matches!(a.repo.format_patch(&s(&["no-such-commit"])).await, Err(GitError::NotFound(_))));
}

#[tokio::test]
async fn log_paging_file_history_and_graph() {
    let t = TempRepo::new().await;
    assert!(t.repo.log(&LogOptions::default()).await.unwrap().is_empty());
    t.commit_files(&[("a.txt", "1\n")], "c1");
    t.commit_files(&[("b.txt", "1\n")], "c2");
    t.git(&["tag", "v1"]);
    t.git(&["switch", "-q", "-c", "side"]);
    t.commit_files(&[("a.txt", "side\n")], "side work");
    t.git(&["switch", "-q", "main"]);
    t.commit_files(&[("c.txt", "1\n")], "c3");
    t.git(&["merge", "-q", "--no-edit", "--no-ff", "side"]);
    t.git(&["mv", "a.txt", "renamed.txt"]);
    t.git(&["commit", "-q", "-m", "rename\n\nwith body"]);

    let all = t.repo.log(&LogOptions::default()).await.unwrap();
    assert_eq!(all.len(), 6);
    assert_eq!(all[0].subject, "rename");
    assert_eq!(all[0].body, "with body");
    assert_eq!(all[0].head_branch.as_deref(), Some("main"));
    assert!(all.iter().any(|c| c.is_merge()));
    assert!(all.iter().any(|c| c.refs.contains(&RefLabel::Tag("v1".into()))));
    assert_eq!(all[0].author_email, "test@example.com");

    let page = t.repo.log(&LogOptions { skip: 2, max_count: 2, ..Default::default() }).await.unwrap();
    assert_eq!(page.iter().map(|c| &c.hash).collect::<Vec<_>>(), all[2..4].iter().map(|c| &c.hash).collect::<Vec<_>>());

    let history = t.repo.log(&LogOptions { paths: s(&["renamed.txt"]), follow: true, ..Default::default() }).await.unwrap();
    let subjects: Vec<_> = history.iter().map(|c| c.subject.as_str()).collect();
    assert_eq!(subjects, vec!["rename", "side work", "c1"]);

    let grep = t.repo.log(&LogOptions { grep: Some("SIDE WORK".into()), ..Default::default() }).await.unwrap();
    assert_eq!(grep.len(), 1);

    let mut graph = Graph::new();
    let rows = graph.rows(&all);
    assert_eq!(rows.len(), all.len());
    assert!(rows.iter().any(|r| r.column == 1));
    assert!(rows.iter().all(|r| r.width <= 2));
    assert_eq!(rows.last().unwrap().column, 0);
}

#[tokio::test]
async fn blame_matches_unsaved_buffer() {
    let t = TempRepo::new().await;
    t.commit_files(&[("f.txt", "one\ntwo\n")], "first");
    t.commit_files(&[("f.txt", "one\ntwo\nthree\n")], "second");
    let b = t.repo.blame("f.txt", None, None, None).await.unwrap();
    assert_eq!(b.lines.len(), 3);
    assert_eq!(b.commit_of(0).unwrap().summary, "first");
    assert_eq!(b.commit_of(2).unwrap().summary, "second");
    assert_eq!(b.commit_of(2).unwrap().author, "Test User");

    // The buffer has an inserted line that is not on disk.
    let buffer = "zero\none\ntwo\nthree\n";
    let b = t.repo.blame("f.txt", Some(buffer), None, None).await.unwrap();
    assert_eq!(b.lines.len(), 4);
    assert!(b.commit_of(0).unwrap().is_uncommitted());
    assert_eq!(b.commit_of(1).unwrap().summary, "first");
    assert_eq!(b.lines[1].orig_line, 1);
    assert_eq!(b.commit_of(3).unwrap().summary, "second");

    let old = t.repo.blame("f.txt", None, Some("HEAD~1"), None).await.unwrap();
    assert_eq!(old.lines.len(), 2);
}

#[tokio::test]
async fn merge_conflicts_are_listed_and_resolved() {
    let t = TempRepo::new().await;
    t.commit_files(&[("c.txt", "base\n"), ("ok.txt", "x\n")], "base");
    t.git(&["switch", "-q", "-c", "theirs"]);
    t.commit_files(&[("c.txt", "theirs\n")], "theirs");
    t.git(&["switch", "-q", "main"]);
    t.commit_files(&[("c.txt", "ours\n")], "ours");

    match t.repo.merge("theirs", false).await {
        Err(GitError::Conflicts(_)) => {}
        other => panic!("expected Conflicts, got {other:?}"),
    }
    assert_eq!(t.repo.state().await, RepoState::Merging);
    assert_eq!(t.repo.conflicted_paths().await.unwrap(), vec!["c.txt".to_string()]);
    let st = t.repo.status(&StatusOptions::default()).await.unwrap();
    let c: Vec<_> = st.conflicts().collect();
    assert_eq!((c.len(), c[0].conflict), (1, Some(ConflictKind::BothModified)));
    let v = t.repo.conflict_versions("c.txt").await.unwrap();
    assert_eq!(v.base.unwrap().text(), "base\n");
    assert_eq!(v.ours.unwrap().text(), "ours\n");
    assert_eq!(v.theirs.unwrap().text(), "theirs\n");
    match t.repo.commit(&CommitOptions { message: "m".into(), ..Default::default() }).await {
        Err(GitError::Conflicts(_)) => {}
        other => panic!("expected Conflicts on commit, got {other:?}"),
    }

    t.repo.resolve_with(&s(&["c.txt"]), true).await.unwrap();
    assert_eq!(t.read("c.txt"), "theirs\n");
    assert!(t.repo.conflicted_paths().await.unwrap().is_empty());
    t.repo.commit(&CommitOptions { message: "merged".into(), ..Default::default() }).await.unwrap();
    assert_eq!(t.repo.state().await, RepoState::Clean);

    // Abort a second conflicting merge.
    t.git(&["switch", "-q", "theirs"]);
    t.commit_files(&[("c.txt", "theirs 2\n")], "theirs 2");
    t.git(&["switch", "-q", "main"]);
    t.commit_files(&[("c.txt", "ours 2\n")], "ours 2");
    assert!(t.repo.merge("theirs", false).await.is_err());
    t.repo.abort_merge().await.unwrap();
    assert_eq!(t.repo.state().await, RepoState::Clean);
    assert_eq!(t.read("c.txt"), "ours 2\n");
}

/// V-07: merge (fast-forward, merge commit, ff-only refused), rebase stopped on a conflict and
/// continued, skipped or aborted, cherry-picks with conflicts, and the operation in progress.
#[tokio::test]
async fn merge_rebase_and_cherry_pick_with_conflicts() {
    let t = TempRepo::new().await;
    t.commit_files(&[("f.txt", "a\nb\nc\n")], "base");
    t.git(&["switch", "-q", "-c", "side"]);
    t.commit_files(&[("f.txt", "a\nB\nc\n")], "side 1");
    t.commit_files(&[("g.txt", "g\n")], "side 2");
    let side2 = t.repo.resolve("side").await.unwrap();
    let side1 = t.repo.resolve("side~1").await.unwrap();
    t.git(&["switch", "-q", "main"]);

    // Fast-forward, then nothing left to merge.
    let ff = t.repo.merge_with("side", &MergeOptions::default(), None, None).await.unwrap();
    assert!(ff.contains("Fast-forward"), "{ff}");
    assert_eq!(t.repo.resolve("HEAD").await.unwrap(), side2);
    let up = t.repo.merge_with("side", &MergeOptions::default(), None, None).await.unwrap();
    assert!(up.contains("Already up to date"), "{up}");
    t.git(&["reset", "-q", "--hard", "HEAD~2"]);

    // main diverges: --ff-only is refused, a merge commit is made, a conflict stops.
    t.commit_files(&[("h.txt", "h\n")], "main 1");
    let only = MergeOptions { fast_forward: FastForward::Only, ..Default::default() };
    assert!(matches!(t.repo.merge_with("side", &only, None, None).await, Err(GitError::NotFastForward)));
    t.repo.merge_with("side", &MergeOptions { fast_forward: FastForward::Never, ..Default::default() }, None, None).await.unwrap();
    assert!(t.git(&["log", "-1", "--format=%P"]).split_whitespace().count() == 2);
    t.git(&["reset", "-q", "--hard", "HEAD~1"]);
    t.commit_files(&[("f.txt", "a\nM\nc\n")], "main 2");
    assert!(matches!(t.repo.merge_with("side", &MergeOptions::default(), None, None).await, Err(GitError::Conflicts(_))));
    let op = t.repo.operation().await;
    assert_eq!((op.state, op.target.as_deref()), (Some(RepoState::Merging), Some(side2.as_str())));
    // Another operation cannot start meanwhile.
    assert!(matches!(t.repo.rebase("side", false, None, None).await, Err(GitError::OperationInProgress)));
    assert!(matches!(t.repo.continue_operation(None).await, Err(GitError::Conflicts(_))));
    t.repo.abort_operation().await.unwrap();
    assert_eq!(t.repo.operation().await, Operation::default());
    assert_eq!(t.read("f.txt"), "a\nM\nc\n");
    assert!(matches!(t.repo.abort_operation().await, Err(GitError::NoOperation)));

    // Rebase side onto main: the first commit conflicts; resolve, continue to the end.
    t.git(&["switch", "-q", "side"]);
    t.write("f.txt", "dirty\n");
    assert!(matches!(t.repo.rebase("main", false, None, None).await, Err(GitError::UncommittedChanges(_))));
    t.git(&["checkout", "--", "f.txt"]);
    let lines = Arc::new(Mutex::new(Vec::new()));
    let sink: ProgressSink = {
        let lines = lines.clone();
        Arc::new(move |p: Progress| lines.lock().unwrap().push(p.text))
    };
    assert!(matches!(t.repo.rebase("main", false, None, Some(sink)).await, Err(GitError::Conflicts(_))));
    assert!(lines.lock().unwrap().iter().any(|l| l.contains("Rebasing (1/2)")), "{:?}", lines.lock().unwrap());
    let op = t.repo.operation().await;
    assert_eq!((op.state, op.branch.as_deref(), op.step), (Some(RepoState::Rebasing), Some("side"), Some((1, 2))));
    assert_eq!(op.target, Some(t.repo.resolve("main").await.unwrap()));
    t.write("f.txt", "a\nM and B\nc\n");
    t.repo.mark_resolved(&s(&["f.txt"])).await.unwrap();
    t.repo.continue_operation(None).await.unwrap();
    assert_eq!(t.repo.state().await, RepoState::Clean);
    assert_eq!(t.git(&["log", "--format=%s", "-3"]).lines().collect::<Vec<_>>(), ["side 2", "side 1", "main 2"]);

    // Rebase again from the original side: skip the conflicting commit, or abort.
    t.git(&["reset", "-q", "--hard", &side2]);
    assert!(t.repo.rebase("main", false, None, None).await.is_err());
    t.repo.skip_operation(None).await.unwrap();
    assert_eq!(t.git(&["log", "--format=%s", "-2"]).lines().collect::<Vec<_>>(), ["side 2", "main 2"]);
    t.git(&["reset", "-q", "--hard", &side2]);
    assert!(t.repo.rebase("main", false, None, None).await.is_err());
    t.repo.abort_operation().await.unwrap();
    assert_eq!(t.repo.resolve("HEAD").await.unwrap(), side2);
    // Local changes are carried over with autostash.
    t.git(&["switch", "-q", "-c", "other", "side~2"]);
    t.commit_files(&[("z.txt", "z\n")], "other");
    t.git(&["switch", "-q", "main"]);
    t.write("h.txt", "local\n");
    t.repo.rebase("other", true, None, None).await.unwrap();
    assert_eq!(t.read("h.txt"), "local\n");
    assert_eq!(t.git(&["log", "--format=%s", "-3"]).lines().collect::<Vec<_>>(), ["main 2", "main 1", "other"]);
    t.git(&["checkout", "--", "h.txt"]);

    // Cherry-pick onto main: one applies cleanly, one conflicts and is resolved and
    // continued; a commit whose change is already there stops as empty and is skipped.
    t.repo.cherry_pick(&s(&[&side2]), None).await.unwrap();
    assert_eq!(t.read("g.txt"), "g\n");
    assert!(matches!(t.repo.cherry_pick(&s(&[&side1]), None).await, Err(GitError::Conflicts(_))));
    let op = t.repo.operation().await;
    assert_eq!((op.state, op.target.as_deref()), (Some(RepoState::CherryPicking), Some(side1.as_str())));
    t.repo.resolve_with(&s(&["f.txt"]), false).await.unwrap();
    assert!(matches!(t.repo.continue_operation(None).await, Err(GitError::EmptyCommit(_))));
    t.repo.skip_operation(None).await.unwrap();
    assert_eq!(t.repo.state().await, RepoState::Clean);
    assert!(matches!(t.repo.cherry_pick(&s(&[&side1]), None).await, Err(GitError::Conflicts(_))));
    t.write("f.txt", "a\nB\nc\n");
    t.repo.mark_resolved(&s(&["f.txt"])).await.unwrap();
    t.repo.continue_operation(None).await.unwrap();
    assert_eq!(t.git(&["log", "-1", "--format=%s"]).trim(), "side 1");
    assert_eq!(t.repo.state().await, RepoState::Clean);
}

#[tokio::test]
async fn tags_are_created_listed_pushed_checked_out_and_deleted() {
    let remote_dir = tempfile::tempdir().unwrap();
    git(remote_dir.path(), &["init", "-q", "--bare", "-b", "main"]);
    let url = format!("file://{}", remote_dir.path().display());
    let t = TempRepo::new().await;
    t.git(&["remote", "add", "origin", &url]);
    t.commit_files(&[("a.txt", "1\n")], "first");
    let first = t.repo.resolve("HEAD").await.unwrap();
    t.commit_files(&[("a.txt", "2\n")], "second");
    let second = t.repo.resolve("HEAD").await.unwrap();

    // Invalid names never reach git as options or refs.
    for bad in ["", "-d", "a..b", "with space", "end.lock", "x~1", "a:b", " v1"] {
        assert!(matches!(t.repo.create_tag(bad, None, None).await, Err(GitError::InvalidRefName(_))), "{bad:?}");
    }
    t.repo.create_tag("v1.0", Some(&first), None).await.unwrap();
    t.repo.create_tag("v2.0", None, Some("Release 2\n\nWith notes")).await.unwrap();
    assert!(matches!(t.repo.create_tag("v2.0", None, None).await, Err(GitError::TagExists(n)) if n == "v2.0"));

    let tags = t.repo.tags().await.unwrap();
    let v1 = tags.iter().find(|x| x.name == "v1.0").unwrap();
    let v2 = tags.iter().find(|x| x.name == "v2.0").unwrap();
    assert_eq!((v1.target.as_str(), v1.annotated, v1.message.as_str()), (first.as_str(), false, ""));
    assert_eq!((v2.target.as_str(), v2.annotated, v2.message.as_str()), (second.as_str(), true, "Release 2\n\nWith notes"));

    // Push one tag, then all of them, to a local bare remote.
    let report = t.repo.push_tags("origin", Some("v1.0"), None, None).await.unwrap();
    assert!(report.contains("refs/tags/v1.0"), "{report}");
    assert_eq!(git(remote_dir.path(), &["tag", "--list"]).trim(), "v1.0");
    t.repo.push_tags("origin", None, None, None).await.unwrap();
    assert_eq!(git(remote_dir.path(), &["tag", "--list"]).lines().collect::<Vec<_>>(), ["v1.0", "v2.0"]);
    assert!(matches!(t.repo.push_tags("nowhere", Some("v1.0"), None, None).await, Err(GitError::Network(_) | GitError::NotFound(_) | GitError::Failed { .. })));
    assert!(t.repo.push_tags("--all", None, None, None).await.is_err());

    // Checkout detaches HEAD at the tag.
    t.repo.checkout_tag("v1.0").await.unwrap();
    assert_eq!(t.repo.current_branch().await.unwrap(), None);
    assert_eq!(t.repo.resolve("HEAD").await.unwrap(), first);

    t.repo.delete_tag("v1.0").await.unwrap();
    assert!(t.repo.tags().await.unwrap().iter().all(|x| x.name != "v1.0"));
    assert!(t.repo.delete_tag("v1.0").await.is_err());
}

#[tokio::test]
async fn worktrees_are_added_listed_locked_removed_and_pruned() {
    let t = TempRepo::new().await;
    t.commit_files(&[("a.txt", "1\n")], "first");
    t.git(&["branch", "dev"]);
    let head = t.repo.resolve("HEAD").await.unwrap();
    let place = tempfile::tempdir().unwrap();
    let place = place.path().canonicalize().unwrap();
    let (feat, dev, detached, gone) = (place.join("feat wt"), place.join("dev"), place.join("-detached"), place.join("gone"));

    let list = t.repo.worktrees().await.unwrap();
    assert_eq!(list.len(), 1);
    assert!(list[0].main && list[0].path == t.repo.root() && list[0].branch.as_deref() == Some("main"));

    // A new branch, an existing one, a detached commit (a path that looks like an option).
    t.repo.add_worktree(&feat, &WorktreeCheckout::NewBranch { name: "feat/x".into(), start: None }).await.unwrap();
    t.repo.add_worktree(&dev, &WorktreeCheckout::Branch("dev".into())).await.unwrap();
    t.repo.add_worktree(&detached, &WorktreeCheckout::Detached("HEAD".into())).await.unwrap();
    t.repo.add_worktree(&gone, &WorktreeCheckout::Detached(head.clone())).await.unwrap();
    assert_eq!(std::fs::read_to_string(feat.join("a.txt")).unwrap(), "1\n");
    let list = t.repo.worktrees().await.unwrap();
    assert_eq!(list.len(), 5);
    let find = |p: &std::path::Path| list.iter().find(|w| w.path == p).cloned().unwrap();
    assert_eq!((find(&feat).branch.as_deref(), find(&feat).head.as_deref()), (Some("feat/x"), Some(head.as_str())));
    assert_eq!(find(&dev).branch.as_deref(), Some("dev"));
    assert!(find(&detached).detached && find(&detached).branch.is_none());

    // Refusals: bad or taken branch names, a branch checked out elsewhere, a used folder.
    for bad in ["", "-b", "a..b", "with space", "HEAD"] {
        let r = t.repo.add_worktree(&place.join("bad"), &WorktreeCheckout::NewBranch { name: bad.into(), start: None }).await;
        assert!(matches!(r, Err(GitError::InvalidBranchName(_))), "{bad:?}: {r:?}");
    }
    assert!(
        matches!(t.repo.add_worktree(&place.join("x"), &WorktreeCheckout::NewBranch { name: "dev".into(), start: None }).await, Err(GitError::BranchExists(b)) if b == "dev")
    );
    let r = t.repo.add_worktree(&place.join("x"), &WorktreeCheckout::Branch("dev".into())).await;
    assert!(matches!(&r, Err(GitError::BranchInUse(b)) if b == "dev"), "{r:?}");
    assert!(matches!(t.repo.add_worktree(&place.join("x"), &WorktreeCheckout::Branch("nope".into())).await, Err(GitError::NotFound(_))));
    std::fs::create_dir_all(place.join("full")).unwrap();
    std::fs::write(place.join("full/f"), "x").unwrap();
    let r = t.repo.add_worktree(&place.join("full"), &WorktreeCheckout::Detached("HEAD".into())).await;
    assert!(matches!(r, Err(GitError::WorktreePathExists(_))), "{r:?}");
    assert!(t.repo.add_worktree(std::path::Path::new("relative"), &WorktreeCheckout::Detached("HEAD".into())).await.is_err());

    // Lock and unlock.
    t.repo.lock_worktree(&dev, Some(" on a USB disk ")).await.unwrap();
    assert_eq!(t.repo.worktrees().await.unwrap().iter().find(|w| w.path == dev).unwrap().locked.as_deref(), Some("on a USB disk"));
    let r = t.repo.remove_worktree(&dev, true).await;
    assert!(matches!(r, Err(GitError::WorktreeLocked(_))), "{r:?}");
    t.repo.unlock_worktree(&dev).await.unwrap();
    assert!(t.repo.worktrees().await.unwrap().iter().find(|w| w.path == dev).unwrap().locked.is_none());

    // Remove: refused with local changes unless forced; the main worktree never.
    std::fs::write(feat.join("new.txt"), "x").unwrap();
    let r = t.repo.remove_worktree(&feat, false).await;
    assert!(matches!(r, Err(GitError::WorktreeDirty(_))), "{r:?}");
    assert!(feat.exists());
    t.repo.remove_worktree(&feat, true).await.unwrap();
    assert!(!feat.exists());
    t.repo.remove_worktree(&dev, false).await.unwrap();
    let r = t.repo.remove_worktree(t.repo.root(), true).await;
    assert!(matches!(r, Err(GitError::MainWorktree)), "{r:?}");
    let r = t.repo.remove_worktree(&place.join("never"), false).await;
    assert!(matches!(r, Err(GitError::NotFound(_))), "{r:?}");

    // A worktree whose folder vanished is prunable, unless locked.
    std::fs::remove_dir_all(&gone).unwrap();
    t.repo.lock_worktree(&detached, None).await.unwrap();
    std::fs::remove_dir_all(&detached).unwrap();
    let list = t.repo.worktrees().await.unwrap();
    assert!(list.iter().find(|w| w.path == gone).unwrap().prunable.is_some());
    assert_eq!(list.iter().find(|w| w.path == detached).unwrap().locked.as_deref(), Some(""));
    let report = t.repo.prune_worktrees().await.unwrap();
    assert!(report.contains("gone"), "{report}");
    let list = t.repo.worktrees().await.unwrap();
    assert_eq!(list.iter().map(|w| w.path.clone()).collect::<Vec<_>>(), [t.repo.root().to_path_buf(), detached.clone()]);
}

#[tokio::test]
async fn fetch_pull_push_with_progress() {
    let remote_dir = tempfile::tempdir().unwrap();
    git(remote_dir.path(), &["init", "-q", "--bare", "-b", "main"]);
    let url = format!("file://{}", remote_dir.path().display());

    let a = TempRepo::new().await;
    a.git(&["remote", "add", "origin", &url]);
    a.commit_files(&[("f.txt", &numbered(200))], "init");
    match a.repo.push(&PushOptions::default(), None, None).await {
        Err(GitError::NoUpstream) => {}
        other => panic!("expected NoUpstream, got {other:?}"),
    }
    let lines = Arc::new(Mutex::new(Vec::<Progress>::new()));
    let sink: ProgressSink = {
        let lines = lines.clone();
        Arc::new(move |p| lines.lock().unwrap().push(p))
    };
    a.repo
        .push(&PushOptions { remote: Some("origin".into()), refspec: Some("main".into()), set_upstream: true, ..Default::default() }, Some(sink.clone()), None)
        .await
        .unwrap();
    assert!(!lines.lock().unwrap().is_empty());

    // A second clone pushes a commit; the first one fetches and pulls it.
    let b_dir = tempfile::tempdir().unwrap();
    git(b_dir.path(), &["clone", "-q", &url, "."]);
    common::configure(b_dir.path());
    let b = Repository::discover(b_dir.path()).await.unwrap().with_trust(true);
    std::fs::write(b.root().join("g.txt"), "g\n").unwrap();
    b.stage(&s(&["g.txt"])).await.unwrap();
    b.commit(&CommitOptions { message: "from b".into(), ..Default::default() }).await.unwrap();
    b.push(&PushOptions::default(), None, None).await.unwrap();

    a.repo.fetch(&FetchOptions { remote: Some("origin".into()), prune: true, ..Default::default() }, Some(sink), None).await.unwrap();
    let st = a.repo.status(&StatusOptions::default()).await.unwrap();
    assert_eq!((st.branch.upstream.as_deref(), st.branch.behind, st.branch.ahead), (Some("origin/main"), 1, 0));
    let branches = a.repo.branches().await.unwrap();
    assert!(branches.iter().any(|br| br.kind == BranchKind::Remote && br.name == "origin/main"));

    // Diverge, then a push is rejected until we pull.
    a.commit_files(&[("h.txt", "h\n")], "from a");
    match a.repo.push(&PushOptions::default(), None, None).await {
        Err(GitError::PushRejected(_)) => {}
        other => panic!("expected PushRejected, got {other:?}"),
    }
    a.repo.pull(&PullOptions { rebase: Some(true), ..Default::default() }, None, None).await.unwrap();
    a.repo.push(&PushOptions::default(), None, None).await.unwrap();
    assert!(a.path("g.txt").exists());
    assert_eq!(a.repo.remotes().await.unwrap()[0].name, "origin");

    // Remote branch checkout and deletion on the remote.
    b.create_branch("topic", None, false).await.unwrap();
    b.push(&PushOptions { remote: Some("origin".into()), refspec: Some("topic".into()), ..Default::default() }, None, None).await.unwrap();
    a.repo.fetch(&FetchOptions::default(), None, None).await.unwrap();
    a.repo.checkout_remote_branch("origin/topic", None).await.unwrap();
    assert_eq!(a.repo.current_branch().await.unwrap().as_deref(), Some("topic"));
    let topic = a.repo.branches().await.unwrap().into_iter().find(|br| br.name == "topic").unwrap();
    assert_eq!(topic.upstream.as_deref(), Some("origin/topic"));
    a.repo.checkout_branch("main").await.unwrap();
    a.repo.push(&PushOptions { remote: Some("origin".into()), refspec: Some("topic".into()), delete: true, ..Default::default() }, None, None).await.unwrap();
    a.repo.fetch(&FetchOptions { prune: true, ..Default::default() }, None, None).await.unwrap();
    let topic = a.repo.branches().await.unwrap().into_iter().find(|br| br.name == "topic").unwrap();
    assert!(topic.upstream_gone);
}

#[tokio::test]
async fn cancelled_operations_stop() {
    let t = TempRepo::new().await;
    let token = CancelToken::new();
    token.cancel();
    assert!(matches!(t.repo.fetch(&FetchOptions::default(), None, Some(&token)).await, Err(GitError::Cancelled)));
}

#[cfg(unix)]
#[tokio::test]
async fn cancel_kills_a_hanging_fetch() {
    let t = TempRepo::new().await;
    t.git(&["config", "protocol.ext.allow", "always"]);
    t.git(&["remote", "add", "slow", "ext::sh -c sleep% 30"]);
    let token = CancelToken::new();
    let canceller = token.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        canceller.cancel();
    });
    let started = std::time::Instant::now();
    let r = t.repo.fetch(&FetchOptions { remote: Some("slow".into()), ..Default::default() }, None, Some(&token)).await;
    assert!(matches!(r, Err(GitError::Cancelled)), "{r:?}");
    assert!(started.elapsed() < std::time::Duration::from_secs(10));
}

#[tokio::test]
async fn gutter_marks_against_head_content() {
    let t = TempRepo::new().await;
    t.commit_files(&[("f.cs", &numbered(10))], "init");
    let head = t.repo.show_file(&Revision::Head, "f.cs").await.unwrap().unwrap();
    let buffer = numbered(10).replace("line 2\n", "line 2 edited\n").replace("line 5\n", "").replace("line 8\n", "line 8\nnew\n");
    let lc = LineChanges::compute(&head.text(), &buffer);
    let marks = lc.marks(10);
    assert_eq!(marks[1], Some(GutterMark::Modified));
    assert_eq!(marks[3], Some(GutterMark::DeletedAfter));
    assert_eq!(marks[7], Some(GutterMark::Added));
    assert_eq!(lc.change_at(1).unwrap().original_text(&head.text()), "line 2\n");
}

#[test]
fn errors_have_translatable_messages() {
    let (template, args) = GitError::BranchExists("dev".into()).user_message();
    assert_eq!(template, "A branch named {branch} already exists");
    assert_eq!(args, vec![("branch", "dev".to_string())]);
    let tsv = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../scripts/i18n/rc-vcs.tsv")).unwrap();
    let errors = [
        GitError::GitNotFound,
        GitError::UnsupportedVersion { found: String::new(), required: String::new() },
        GitError::NotARepository(Default::default()),
        GitError::NothingToCommit,
        GitError::NothingToStash,
        GitError::StashChanged,
        GitError::Conflicts(String::new()),
        GitError::AuthenticationRequired(String::new()),
        GitError::DetachedHead,
        GitError::LocalChangesWouldBeOverwritten(vec![]),
        GitError::BranchExists(String::new()),
        GitError::BranchNotFullyMerged(String::new()),
        GitError::NoUpstream,
        GitError::PushRejected(String::new()),
        GitError::PatchFailed(String::new()),
        GitError::PatchTooLarge(32),
        GitError::OperationInProgress,
        GitError::UncommittedChanges(String::new()),
        GitError::EmptyCommit(String::new()),
        GitError::NoOperation,
        GitError::UnrelatedHistories,
        GitError::NotFastForward,
        GitError::NotFound(String::new()),
        GitError::IdentityUnknown,
        GitError::Locked(String::new()),
        GitError::Network(String::new()),
        GitError::Cancelled,
        GitError::Parse(String::new()),
    ];
    for e in errors {
        let (template, _) = e.user_message();
        assert!(tsv.lines().any(|l| l.split('\t').next() == Some(template)), "missing translation of {template:?}");
    }
}

/// V-14: commits are signed with a GPG key (a throwaway one, in its own GnuPG home reached
/// through `gpg.program`), git verifies them, and a signing program that fails is reported.
#[tokio::test]
async fn commits_are_signed_and_their_signatures_verified() {
    if std::process::Command::new("gpg").arg("--version").output().is_err() {
        eprintln!("skipped: gpg is not installed");
        return;
    }
    let t = TempRepo::new().await;
    let home = tempfile::tempdir().unwrap();
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
    t.git(&["config", "gpg.program", &wrapper.display().to_string()]);
    t.git(&["config", "user.signingkey", "signer@example.com"]);

    let cfg = t.repo.signing_config().await.unwrap();
    assert_eq!((cfg.sign_by_default, cfg.key.as_deref(), cfg.format.as_str()), (false, Some("signer@example.com"), "openpgp"));

    t.write("a.txt", "a\n");
    let signed = t.repo.commit(&CommitOptions { message: "Signed".into(), only: s(&["a.txt"]), sign: Some(true), ..Default::default() }).await.unwrap();
    let sig = t.repo.signature(&signed).await.unwrap();
    assert!(matches!(sig.status, SignatureStatus::Good | SignatureStatus::GoodUntrusted), "{sig:?}");
    assert_eq!(sig.signer, "Signer <signer@example.com>");
    assert!(!sig.key.is_empty());

    // `--no-gpg-sign` wins over `commit.gpgsign`.
    t.git(&["config", "commit.gpgsign", "true"]);
    assert!(t.repo.signing_config().await.unwrap().sign_by_default);
    t.write("a.txt", "b\n");
    let plain = t.repo.commit(&CommitOptions { message: "Plain".into(), only: s(&["a.txt"]), sign: Some(false), ..Default::default() }).await.unwrap();
    assert_eq!(t.repo.signature(&plain).await.unwrap().status, SignatureStatus::Unsigned);
    // `None` follows it.
    t.write("a.txt", "c\n");
    let default = t.repo.commit(&CommitOptions { message: "Default".into(), only: s(&["a.txt"]), ..Default::default() }).await.unwrap();
    assert_ne!(t.repo.signature(&default).await.unwrap().status, SignatureStatus::Unsigned);

    // A signing program that fails: nothing is committed, and the error says why.
    t.git(&["config", "gpg.program", "false"]);
    t.write("a.txt", "d\n");
    let err = t.repo.commit(&CommitOptions { message: "Fails".into(), only: s(&["a.txt"]), sign: Some(true), ..Default::default() }).await.unwrap_err();
    assert!(matches!(err, GitError::SigningFailed(_)), "{err:?}");
    assert_eq!(t.repo.resolve("HEAD").await.unwrap(), default);
}
