#![cfg(unix)]
mod common;
use common::TempRepo;
use rc_vcs::{CancelToken, GitError, Repository};
use std::time::Duration;

#[tokio::test]
async fn untrusted_read_commands_cannot_redirect_output_or_write_files() {
    let repo = TempRepo::new().await;
    repo.commit_files(&[("src/lib.rs", "pub fn value()->u8{1}\n")], "Rust fixture");
    let safe = Repository::discover(repo.dir.path()).await.unwrap();
    for command in ["diff", "log", "show", "rev-list", "stash"] {
        let mut args = vec![command];
        if command == "stash" {
            args.push("list");
        }
        args.extend(["--output=forbidden"]);
        assert!(matches!(safe.cmd().args(args).output().await, Err(GitError::TrustRequired)));
        assert!(!repo.path("forbidden").exists());
    }
    for option in ["--output", "--out=forbidden", "-o", "--write-bitmap-index"] {
        assert!(matches!(safe.cmd().args(["log", option, "forbidden"]).output().await, Err(GitError::TrustRequired)));
        assert!(!repo.path("forbidden").exists());
    }
}

#[tokio::test]
async fn cancellation_and_dropped_futures_kill_git_helper_descendants() {
    let repo = TempRepo::new().await;
    repo.commit_files(&[("src/lib.rs", "pub fn value()->u8{1}\n")], "Rust fixture");
    std::fs::write(repo.path(".git/delay"), "printf started > .git/started\n(sleep 1; printf late > .git/late) &\nwait\n").unwrap();
    repo.git(&["config", "alias.delay", "!sh .git/delay"]);
    let git = repo.repo.git().clone();
    let root = repo.dir.path().to_path_buf();
    let token = CancelToken::new();
    let child_token = token.clone();
    let task = tokio::spawn(async move { git.command(&root).trusted(true).arg("delay").cancel(Some(&child_token)).output().await });
    for _ in 0..100 {
        if repo.path(".git/started").exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(repo.path(".git/started").exists());
    token.cancel();
    assert!(matches!(task.await.unwrap(), Err(GitError::Cancelled)));
    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert!(!repo.path(".git/late").exists());
    std::fs::remove_file(repo.path(".git/started")).unwrap();
    let git = repo.repo.git().clone();
    let root = repo.dir.path().to_path_buf();
    let task = tokio::spawn(async move { git.command(&root).trusted(true).arg("delay").output().await });
    for _ in 0..100 {
        if repo.path(".git/started").exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(repo.path(".git/started").exists());
    task.abort();
    let _ = task.await;
    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert!(!repo.path(".git/late").exists());
}
