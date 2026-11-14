//! Share in Playground (N-18): the selection, or the whole Rust file, goes to the Rust
//! Playground after the user confirms; the link is copied and shown with an Open in Browser
//! action. The upload runs on a background thread with a timeout.

use rc_cargo::playground::{Channel, EDITIONS, GIST_ENDPOINT, PlaygroundError};
use rc_core::{Inbox, Waker};
use rc_kit::i18n::{tr, trf};
use std::time::Duration;

/// How long the upload may take.
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(20);
/// Lines shown in the confirmation's preview.
pub const PREVIEW_LINES: usize = 8;
/// Characters kept of each preview line.
const PREVIEW_COLUMNS: usize = 100;

/// What the confirmation dialog shows and lets the user pick.
#[derive(Debug, Clone)]
pub struct Share {
    pub code: String,
    pub lines: usize,
    /// The first lines, shortened.
    pub preview: String,
    /// More lines follow the preview.
    pub more: bool,
    pub channel: Channel,
    pub edition: String,
}

impl Share {
    /// The dialog's content for `code`; `edition` is the crate's, if known.
    pub fn new(code: String, edition: Option<&str>) -> Share {
        let lines = code.lines().count().max(1);
        let preview = code
            .lines()
            .take(PREVIEW_LINES)
            .map(|l| {
                let l = l.replace('\t', "    ");
                if l.chars().count() > PREVIEW_COLUMNS { format!("{}…", l.chars().take(PREVIEW_COLUMNS).collect::<String>()) } else { l }
            })
            .collect::<Vec<_>>()
            .join("\n");
        let edition = edition.filter(|e| EDITIONS.contains(e)).unwrap_or(EDITIONS[EDITIONS.len() - 1]).to_string();
        Share { more: lines > PREVIEW_LINES, code, lines, preview, channel: Channel::Stable, edition }
    }
}

/// The uploads in flight and their answers.
pub struct Playground {
    inbox: Inbox<Result<String, PlaygroundError>>,
    running: usize,
    /// Where gists are created (a local server in tests).
    pub endpoint: String,
    /// The last link made.
    pub last_url: Option<String>,
}

impl Playground {
    pub fn new(waker: Waker) -> Self {
        Playground { inbox: Inbox::new(waker), running: 0, endpoint: GIST_ENDPOINT.to_string(), last_url: None }
    }

    pub fn running(&self) -> bool {
        self.running > 0
    }

    /// Uploads the confirmed code in the background.
    pub fn start(&mut self, share: &Share) {
        let tx = self.inbox.sender();
        let endpoint = self.endpoint.clone();
        let (code, channel, edition) = (share.code.clone(), share.channel, share.edition.clone());
        self.running += 1;
        std::thread::spawn(move || {
            let result = rc_cargo::playground::upload_gist(&endpoint, &code, UPLOAD_TIMEOUT).map(|id| rc_cargo::playground::share_url(channel, &edition, &id));
            tx.send(result);
        });
    }

    /// The finished uploads: the share link, or the message for what went wrong.
    pub fn poll(&mut self) -> Vec<Result<String, String>> {
        let done = self.inbox.drain();
        self.running = self.running.saturating_sub(done.len());
        done.into_iter()
            .map(|r| match r {
                Ok(url) => {
                    self.last_url = Some(url.clone());
                    Ok(url)
                }
                Err(e) => Err(error_text(&e)),
            })
            .collect()
    }
}

/// The user-facing text of an upload error.
pub fn error_text(e: &PlaygroundError) -> String {
    match e {
        PlaygroundError::TooLarge => tr("The code is too large to share on the Rust Playground").to_string(),
        PlaygroundError::Status(code) => trf("The Rust Playground refused the code (HTTP {code})", &[("code", code)]),
        PlaygroundError::ReplyTooLarge | PlaygroundError::BadReply => tr("The Rust Playground sent an unexpected reply").to_string(),
        PlaygroundError::TimedOut => tr("The Rust Playground did not answer in time").to_string(),
        PlaygroundError::Network(m) => trf("Could not reach the Rust Playground: {error}", &[("error", m)]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_confirmation_counts_lines_and_previews_the_first_ones() {
        let code = (1..=12).map(|i| format!("let x{i} = {i};")).collect::<Vec<_>>().join("\n");
        let s = Share::new(code, Some("2021"));
        assert_eq!(s.lines, 12);
        assert!(s.more);
        assert_eq!(s.preview.lines().count(), PREVIEW_LINES);
        assert!(s.preview.starts_with("let x1 = 1;\nlet x2 = 2;"));
        assert_eq!(s.edition, "2021");
        assert_eq!(s.channel, Channel::Stable);

        let s = Share::new(format!("{}\n", "y".repeat(300)), Some("2099"));
        assert_eq!((s.lines, s.more), (1, false));
        assert_eq!(s.preview.chars().count(), PREVIEW_COLUMNS + 1);
        assert_eq!(s.edition, "2024", "an unknown edition falls back to the newest");
        assert_eq!(Share::new(String::new(), None).edition, "2024");
    }
}
