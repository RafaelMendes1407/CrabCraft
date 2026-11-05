//! Hostile-helper controls are executed deliberately only in throwaway test repositories.
#![cfg(unix)]
mod common;
use common::TempRepo;
use rc_vcs::{DiffOptions, DiffSpec, GitError, LogOptions, Repository, Revision, StatusOptions};
fn helper(repo: &TempRepo, name: &str, body: &str) -> String {
    use std::os::unix::fs::PermissionsExt;
    let path = repo.path(&format!(".git/{name}"));
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
    path.display().to_string()
}
fn clear(repo: &TempRepo, names: &[&str]) {
    for name in names {
        let path = repo.path(name);
        if path.exists() {
            std::fs::remove_file(path).unwrap();
        }
    }
}
fn absent(repo: &TempRepo, names: &[&str]) {
    for name in names {
        assert!(
            !repo.path(name).exists(),
            "unexpected helper execution: {name}"
        );
    }
}
#[tokio::test]
async fn static_hostile_rust_repository_reads_do_not_execute_configured_helpers() {
    let repo = TempRepo::new().await;
    repo.commit_files(
        &[
            (
                "Cargo.toml",
                "[package]\nname=\"git_safety\"\nversion=\"0.1.0\"\nedition=\"2021\"\n",
            ),
            ("src/lib.rs", "pub fn value()->u32 {1}\n"),
        ],
        "Rust fixture",
    );
    let fsmonitor = helper(&repo, "monitor", "printf x > marker-fsmonitor\nexit 1");
    repo.git(&["config", "core.fsmonitor", &fsmonitor]);
    repo.git(&["status", "--porcelain"]);
    assert!(repo.path("marker-fsmonitor").exists());
    clear(&repo, &["marker-fsmonitor"]);
    std::fs::create_dir_all(repo.path(".git/hostile-hooks")).unwrap();
    let hook = repo.path(".git/hostile-hooks/post-index-change");
    std::fs::write(&hook, "#!/bin/sh\nprintf x > marker-hook\n").unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    repo.git(&["config", "core.hooksPath", ".git/hostile-hooks"]);
    repo.write("src/lib.rs", "pub fn value()->u32 {2}\n");
    repo.git(&["add", "src/lib.rs"]);
    assert!(repo.path("marker-hook").exists());
    clear(&repo, &["marker-fsmonitor", "marker-hook"]);
    let clean = helper(&repo, "cleaner", "printf x > marker-clean\ncat");
    let smudge = helper(&repo, "smudger", "printf x > marker-smudge\ncat");
    let textconv = helper(&repo, "textconv", "printf x > marker-textconv\ncat \"$1\"");
    let external = helper(&repo, "external", "printf x > marker-external\nexit 0");
    repo.git(&["config", "filter.hostile.clean", &clean]);
    repo.git(&["config", "filter.hostile.smudge", &smudge]);
    repo.git(&["config", "filter.hostile.required", "true"]);
    repo.git(&["config", "diff.hostile.textconv", &textconv]);
    repo.git(&["config", "diff.external", &external]);
    repo.write(".gitattributes", "*.rs filter=hostile diff=hostile\n");
    repo.write("src/lib.rs", "pub fn value()->u32 {3}\n");
    repo.git(&["diff", "--no-ext-diff"]);
    assert!(repo.path("marker-clean").exists());
    assert!(repo.path("marker-textconv").exists());
    repo.git(&["diff"]);
    assert!(repo.path("marker-external").exists());
    repo.git(&["cat-file", "--filters", "HEAD:src/lib.rs"]);
    assert!(repo.path("marker-smudge").exists());
    let process = helper(&repo, "processor", "printf x > marker-process\nexit 1");
    repo.git(&["config", "filter.hostile.process", &process]);
    let control = std::process::Command::new("git")
        .current_dir(repo.repo.root())
        .args(["diff", "--no-ext-diff", "--no-textconv"])
        .output()
        .unwrap();
    assert!(!control.status.success());
    assert!(repo.path("marker-process").exists());
    let markers = [
        "marker-fsmonitor",
        "marker-hook",
        "marker-clean",
        "marker-smudge",
        "marker-textconv",
        "marker-external",
        "marker-process",
    ];
    clear(&repo, &markers);
    let safe = Repository::discover(repo.repo.root()).await.unwrap();
    assert!(
        !safe
            .status(&StatusOptions::default())
            .await
            .unwrap()
            .entries
            .is_empty()
    );
    safe.log(&LogOptions::default()).await.unwrap();
    safe.blame("src/lib.rs", None, None, None).await.unwrap();
    safe.diff(&DiffSpec::WorktreeVsHead, &DiffOptions::default())
        .await
        .unwrap();
    let content = safe
        .show_file(&Revision::Head, "src/lib.rs")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(content.text(), "pub fn value()->u32 {1}\n");
    safe.branches().await.unwrap();
    safe.stashes().await.unwrap();
    safe.tags().await.unwrap();
    safe.worktrees().await.unwrap();
    absent(&repo, &markers);
    assert!(matches!(
        safe.stage_all().await,
        Err(GitError::TrustRequired)
    ));
    absent(&repo, &markers);
    // Explicit trust restores the intended helper/hook workflow, never silently suppressing it.
    repo.git(&["config", "--unset", "filter.hostile.process"]);
    safe.with_trust(true).stage_all().await.unwrap();
    assert!(repo.path("marker-clean").exists());
    assert!(repo.path("marker-hook").exists());
}

#[tokio::test]
async fn safe_history_never_verifies_with_repository_gpg_or_fetches_missing_objects() {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let repo = TempRepo::new().await;
    repo.commit_files(
        &[("src/lib.rs", "pub fn value()->u32{1}\n")],
        "Rust fixture",
    );
    let tree = repo.git(&["rev-parse", "HEAD^{tree}"]);
    let parent = repo.git(&["rev-parse", "HEAD"]);
    let commit = format!(
        "tree {}\nparent {}\nauthor Safety <test@example.com> 1700000000 +0000\ncommitter Safety <test@example.com> 1700000000 +0000\ngpgsig -----BEGIN PGP SIGNATURE-----\n AAAA\n -----END PGP SIGNATURE-----\n\nSigned control\n",
        tree.trim(),
        parent.trim()
    );
    let mut child = Command::new("git")
        .current_dir(repo.repo.root())
        .args(["hash-object", "-t", "commit", "-w", "--stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(commit.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let hash = String::from_utf8(output.stdout).unwrap();
    repo.git(&["update-ref", "refs/heads/signed-control", hash.trim()]);
    let gpg = helper(&repo, "verifier", "printf x > marker-gpg\nexit 1");
    repo.git(&["config", "gpg.program", &gpg]);
    repo.git(&["config", "log.showSignature", "true"]);
    repo.git(&["log", "-1", "--format=%G?", "signed-control"]);
    assert!(repo.path("marker-gpg").exists());
    clear(&repo, &["marker-gpg"]);
    let safe = Repository::discover(repo.repo.root()).await.unwrap();
    safe.log(&LogOptions {
        revs: vec!["signed-control".into()],
        ..Default::default()
    })
    .await
    .unwrap();
    safe.signature("signed-control").await.unwrap();
    absent(&repo, &["marker-gpg"]);
    let transport = helper(&repo, "transport", "printf x > marker-transport\nexit 1");
    repo.git(&["config", "extensions.partialClone", "origin"]);
    repo.git(&["config", "remote.origin.promisor", "true"]);
    repo.git(&["config", "remote.origin.url", &format!("ext::{transport}")]);
    repo.git(&["config", "protocol.ext.allow", "always"]);
    let missing = "1111111111111111111111111111111111111111";
    let output = Command::new("git")
        .current_dir(repo.repo.root())
        .args(["cat-file", "blob", missing])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(repo.path("marker-transport").exists());
    clear(&repo, &["marker-transport"]);
    assert!(
        safe.cmd()
            .args(["cat-file", "blob", missing])
            .output()
            .await
            .is_err()
    );
    absent(&repo, &["marker-gpg", "marker-transport"]);
}

#[tokio::test]
async fn unsafe_config_reads_cancel_and_oversized_blob_output_is_bounded() {
    use rc_vcs::CancelToken;
    use std::time::Duration;
    let repo = TempRepo::new().await;
    let text = " ".repeat(33 * 1024 * 1024);
    repo.commit_files(&[("src/lib.rs", &text)], "Large Rust source");
    drop(text);
    let safe = Repository::discover(repo.repo.root()).await.unwrap();
    let error = tokio::time::timeout(
        Duration::from_secs(5),
        safe.show_file(&Revision::Head, "src/lib.rs"),
    )
    .await
    .unwrap()
    .unwrap_err();
    assert!(matches!(error, GitError::OutputLimit));
    let config = repo.path(".git/config");
    std::fs::remove_file(&config).unwrap();
    assert!(
        std::process::Command::new("mkfifo")
            .arg(&config)
            .status()
            .unwrap()
            .success()
    );
    let cancel = CancelToken::new();
    let stop = cancel.clone();
    let operation = safe
        .cmd()
        .args(["status", "--porcelain"])
        .cancel(Some(&cancel))
        .output();
    let trigger = async move {
        tokio::time::sleep(Duration::from_millis(30)).await;
        stop.cancel();
    };
    let (result, ()) = tokio::time::timeout(Duration::from_secs(2), async {
        tokio::join!(operation, trigger)
    })
    .await
    .unwrap();
    assert!(matches!(result, Err(GitError::Cancelled)));
}
