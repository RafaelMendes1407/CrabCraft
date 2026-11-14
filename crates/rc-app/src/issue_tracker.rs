//! Explicit read-only GitHub/GitLab issue lookup; project configuration never starts HTTP.
pub use rc_core::session::IssueTrackerConfig as Config;
use rc_core::{Inbox, Waker};
use rc_kit::i18n::{tr, trf};
use std::time::Duration;
#[derive(Clone, Debug)]
pub struct Issue {
    pub number: u64,
    pub title: String,
    pub body: String,
    pub state: String,
    pub author: String,
    pub url: String,
}
#[derive(Debug)]
pub enum Error {
    Config,
    Token,
    Network,
    Status(u16),
    Limit,
    Payload,
    Cancelled,
}
impl Error {
    fn message(&self) -> String {
        match self {
            Self::Config => tr("Choose GitHub or GitLab, a valid HTTPS API base and a namespace/project path").into(),
            Self::Token => tr("The tracker token contains invalid header characters").into(),
            Self::Network => tr("Could not reach the issue tracker; check its address and connection").into(),
            Self::Status(status) => trf("Issue tracker returned HTTP {status}", &[("status", status)]),
            Self::Limit => tr("Issue tracker response exceeded its safety limits").into(),
            Self::Payload => tr("Issue tracker returned an unsupported response").into(),
            Self::Cancelled => tr("Issue lookup cancelled").into(),
        }
    }
}
fn default_config() -> Config {
    Config { provider: "github".into(), base_url: "https://api.github.com".into(), project: String::new() }
}
fn checked(config: &Config) -> Result<reqwest::Url, Error> {
    if !matches!(config.provider.as_str(), "github" | "gitlab") || config.base_url.len() > 2048 || config.project.len() > 512 {
        return Err(Error::Config);
    }
    let base = reqwest::Url::parse(config.base_url.trim()).map_err(|_| Error::Config)?;
    let loopback = base.host_str().is_some_and(|host| host.trim_matches(['[', ']']).parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback()));
    if !(base.scheme() == "https" || (base.scheme() == "http" && loopback))
        || base.host_str().is_none()
        || !base.username().is_empty()
        || base.password().is_some()
        || base.query().is_some()
        || base.fragment().is_some()
    {
        return Err(Error::Config);
    }
    let parts: Vec<_> = config.project.split('/').collect();
    if parts.len() < 2
        || (config.provider == "github" && parts.len() != 2)
        || parts
            .iter()
            .any(|part| part.is_empty() || matches!(*part, "." | "..") || !part.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.')))
    {
        return Err(Error::Config);
    }
    Ok(base)
}
fn endpoint(config: &Config, state: &str, page: usize) -> Result<reqwest::Url, Error> {
    if !matches!(state, "open" | "closed" | "all") || !(1..=100).contains(&page) {
        return Err(Error::Config);
    }
    let mut url = checked(config)?;
    {
        let mut path = url.path_segments_mut().map_err(|_| Error::Config)?;
        path.pop_if_empty();
        if config.provider == "github" {
            path.push("repos");
            for part in config.project.split('/') {
                path.push(part);
            }
            path.push("issues");
        } else {
            path.push("projects").push(&config.project).push("issues");
        }
    }
    let state = if config.provider == "gitlab" && state == "open" { "opened" } else { state };
    url.query_pairs_mut().append_pair("state", state).append_pair("per_page", "100").append_pair("page", &page.to_string());
    if config.provider == "gitlab" {
        url.query_pairs_mut().append_pair("scope", "all");
    }
    Ok(url)
}
fn issue_url(config: &Config, number: u64) -> Result<String, Error> {
    let mut url = checked(config)?;
    if config.provider == "github" && url.host_str() == Some("api.github.com") {
        url.set_host(Some("github.com")).map_err(|_| Error::Config)?;
        url.set_path("");
    } else {
        let mut path = url.path().trim_end_matches('/').to_string();
        for suffix in ["/api/v3", "/api/v4"] {
            if let Some(prefix) = path.strip_suffix(suffix) {
                path = prefix.into();
                break;
            }
        }
        url.set_path(&path);
    }
    {
        let mut path = url.path_segments_mut().map_err(|_| Error::Config)?;
        path.pop_if_empty();
        for part in config.project.split('/') {
            path.push(part);
        }
        if config.provider == "gitlab" {
            path.push("-");
        }
        path.push("issues").push(&number.to_string());
    }
    Ok(url.to_string())
}
fn parse(config: &Config, bytes: &[u8]) -> Result<Vec<Issue>, Error> {
    let value: serde_json::Value = serde_json::from_slice(bytes).map_err(|_| Error::Payload)?;
    let rows = value.as_array().ok_or(Error::Payload)?;
    if rows.len() > 100 {
        return Err(Error::Limit);
    }
    let mut result = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for row in rows {
        if config.provider == "github" && row.get("pull_request").is_some() {
            continue;
        }
        let number = row.get(if config.provider == "github" { "number" } else { "iid" }).and_then(|n| n.as_u64()).filter(|n| *n > 0).ok_or(Error::Payload)?;
        if !seen.insert(number) {
            continue;
        }
        let string = |key: &str, limit: usize| -> Result<String, Error> {
            let text = row.get(key).and_then(|s| s.as_str()).unwrap_or("");
            if text.len() > limit {
                return Err(Error::Limit);
            }
            Ok(text.chars().filter(|c| !c.is_control() || matches!(c, '\n' | '\t' | '\r')).collect())
        };
        let title = string("title", 1024)?;
        if title.is_empty() {
            return Err(Error::Payload);
        }
        let body = string(if config.provider == "github" { "body" } else { "description" }, 64 * 1024)?;
        let state = string("state", 32)?;
        let author = row
            .get(if config.provider == "github" { "user" } else { "author" })
            .and_then(|a| a.get(if config.provider == "github" { "login" } else { "username" }))
            .and_then(|a| a.as_str())
            .unwrap_or("")
            .chars()
            .take(128)
            .collect();
        result.push(Issue { number, title, body, state, author, url: issue_url(config, number)? });
    }
    Ok(result)
}
pub async fn fetch(config: &Config, token: &str, state: &str, page: usize, cancel: &rc_dotnet::CancelToken) -> Result<Vec<Issue>, Error> {
    if cancel.is_cancelled() {
        return Err(Error::Cancelled);
    }
    let url = endpoint(config, state, page)?;
    if token.len() > 4096 {
        return Err(Error::Token);
    }
    let mut builder = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .connect_timeout(Duration::from_secs(8))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent("CrabCraft issue lookup");
    if url.host_str().is_some_and(|host| host.trim_matches(['[', ']']).parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback())) {
        builder = builder.no_proxy();
    }
    let client = builder.build().map_err(|_| Error::Network)?;
    let mut request = client.get(url).header("Accept", "application/json");
    if !token.is_empty() {
        let value = if config.provider == "github" { format!("Bearer {token}") } else { token.to_string() };
        let mut header = reqwest::header::HeaderValue::from_str(&value).map_err(|_| Error::Token)?;
        header.set_sensitive(true);
        request = if config.provider == "github" { request.header(reqwest::header::AUTHORIZATION, header) } else { request.header("PRIVATE-TOKEN", header) };
    }
    let work = async {
        let mut response = request.send().await.map_err(|_| Error::Network)?;
        if response.status() != reqwest::StatusCode::OK {
            return Err(Error::Status(response.status().as_u16()));
        }
        if response.content_length().is_some_and(|n| n > 2 * 1024 * 1024) {
            return Err(Error::Limit);
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| Error::Network)? {
            if bytes.len() + chunk.len() > 2 * 1024 * 1024 {
                return Err(Error::Limit);
            }
            bytes.extend_from_slice(&chunk);
        }
        parse(config, &bytes)
    };
    tokio::select! {result=work=>result,()=cancel.cancelled()=>Err(Error::Cancelled)}
}
fn reference(config: &Config, issue: &Issue) -> String {
    format!("{}#{}", config.project, issue.number)
}
fn branch(issue: &Issue) -> String {
    let slug: String = issue.title.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect();
    let slug = slug.split('-').filter(|p| !p.is_empty()).collect::<Vec<_>>().join("-");
    format!("issue-{}-{}", issue.number, slug.chars().take(48).collect::<String>()).trim_end_matches('-').into()
}
pub enum UiAction {
    Commit(String),
    Branch(String),
}
struct Event {
    result: Result<Vec<Issue>, Error>,
    page: usize,
}
pub struct Tracker {
    pub config: Config,
    pub token: String,
    pub issues: Vec<Issue>,
    pub state: String,
    pub page: usize,
    pub selected: Option<usize>,
    inbox: Inbox<Event>,
    cancel: Option<rc_dotnet::CancelToken>,
    error: Option<String>,
}
impl Tracker {
    pub fn new(waker: Waker) -> Self {
        Self {
            config: default_config(),
            token: String::new(),
            issues: Vec::new(),
            state: "open".into(),
            page: 1,
            selected: None,
            inbox: Inbox::new(waker),
            cancel: None,
            error: None,
        }
    }
    pub fn restore(&mut self, config: Option<Config>) {
        self.token.clear();
        if let Some(config) = config
            && matches!(config.provider.as_str(), "github" | "gitlab")
            && config.base_url.len() <= 2048
            && config.project.len() <= 512
        {
            self.config = config;
        }
    }
    pub fn cancel(&self) {
        if let Some(cancel) = &self.cancel {
            cancel.cancel();
        }
    }
    fn start(&mut self, page: usize) {
        if self.cancel.is_some() {
            return;
        }
        self.error = None;
        let cancel = rc_dotnet::CancelToken::new();
        self.cancel = Some(cancel.clone());
        let out = self.inbox.sender();
        let config = self.config.clone();
        let token = self.token.clone();
        let state = self.state.clone();
        rc_core::Runtime::spawn(async move {
            out.send(Event { result: fetch(&config, &token, &state, page, &cancel).await, page });
        });
    }
    pub fn poll(&mut self) {
        for event in self.inbox.drain() {
            self.cancel = None;
            match event.result {
                Ok(issues) => {
                    self.issues = issues;
                    self.page = event.page;
                    self.selected = None;
                }
                Err(error) => self.error = Some(error.message()),
            }
        }
    }
    pub fn show(&mut self, ui: &mut egui::Ui) -> (Option<UiAction>, bool) {
        let mut action = None;
        let mut changed = false;
        let busy = self.cancel.is_some();
        egui::ScrollArea::vertical().id_salt("issue_tracker").show(ui, |ui| {
            ui.label(tr("Read-only issue lookup. Requests run only after Fetch, Previous or Next. Tokens stay in memory and are never saved."));
            ui.add_enabled_ui(!busy, |ui| {
                ui.horizontal(|ui| {
                    ui.label(tr("Issue tracker"));
                    egui::ComboBox::from_id_salt("tracker_provider").selected_text(if self.config.provider == "gitlab" { "GitLab" } else { "GitHub" }).show_ui(
                        ui,
                        |ui| {
                            for (provider, title, base) in [("github", "GitHub", "https://api.github.com"), ("gitlab", "GitLab", "https://gitlab.com/api/v4")] {
                                if ui.selectable_value(&mut self.config.provider, provider.into(), title).changed() {
                                    self.config.base_url = base.into();
                                    changed = true;
                                }
                            }
                        },
                    );
                });
                ui.horizontal(|ui| {
                    ui.label(tr("API base URL"));
                    changed |= ui.add(egui::TextEdit::singleline(&mut self.config.base_url).char_limit(2048).desired_width(400.0)).changed();
                });
                ui.horizontal(|ui| {
                    ui.label(tr("Namespace/project"));
                    changed |= ui.add(egui::TextEdit::singleline(&mut self.config.project).char_limit(512).desired_width(400.0)).changed();
                });
                if changed {
                    self.token.clear();
                    self.issues.clear();
                    self.selected = None;
                    self.page = 1;
                }
                ui.horizontal(|ui| {
                    ui.label(tr("Token (memory only)"));
                    ui.add(egui::TextEdit::singleline(&mut self.token).password(true).char_limit(4096));
                });
                ui.horizontal(|ui| {
                    egui::ComboBox::from_id_salt("tracker_state")
                        .selected_text(match self.state.as_str() {
                            "closed" => tr("Closed"),
                            "all" => tr("All"),
                            _ => tr("Open"),
                        })
                        .show_ui(ui, |ui| {
                            for (value, title) in [("open", tr("Open")), ("closed", tr("Closed")), ("all", tr("All"))] {
                                ui.selectable_value(&mut self.state, value.into(), title);
                            }
                        });
                    if ui.button(tr("Fetch Issues")).clicked() {
                        self.start(1);
                    }
                    if ui.add_enabled(self.page > 1, egui::Button::new(tr("Previous"))).clicked() {
                        self.start(self.page - 1);
                    }
                    if ui.add_enabled(self.page < 100, egui::Button::new(tr("Next"))).clicked() {
                        self.start(self.page + 1);
                    }
                    ui.label(format!("{} {}", tr("Page"), self.page));
                });
            });
            if busy {
                ui.horizontal(|ui| {
                    ui.spinner();
                    if ui.button(tr("Cancel")).clicked() {
                        self.cancel();
                    }
                });
            }
            if let Some(error) = &self.error {
                ui.colored_label(rc_kit::pal(ui).error, error);
            }
            egui::ScrollArea::vertical().id_salt("tracker_rows").max_height(180.0).show(ui, |ui| {
                for (i, issue) in self.issues.iter().enumerate() {
                    if ui.selectable_label(self.selected == Some(i), format!("#{} {}", issue.number, issue.title)).clicked() {
                        self.selected = Some(i);
                    }
                }
            });
            if let Some(issue) = self.selected.and_then(|i| self.issues.get(i)) {
                ui.label(format!("{} · {}", issue.state, issue.author));
                ui.hyperlink_to(tr("Open Issue"), &issue.url);
                ui.horizontal(|ui| {
                    if ui.button(tr("Copy Issue Reference")).clicked() {
                        ui.ctx().copy_text(reference(&self.config, issue));
                    }
                    if ui.button(tr("Use in Commit Message")).clicked() {
                        action = Some(UiAction::Commit(format!(
                            "Refs {} — {}",
                            reference(&self.config, issue),
                            issue.title.lines().next().unwrap_or("").chars().take(200).collect::<String>()
                        )));
                    }
                    if ui.button(tr("Create Issue Branch…")).clicked() {
                        action = Some(UiAction::Branch(branch(issue)));
                    }
                });
                let mut body = issue.body.clone();
                ui.add(egui::TextEdit::multiline(&mut body).interactive(false).desired_rows(8));
            }
        });
        (action, changed)
    }
}
impl Drop for Tracker {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn config(provider: &str, base: &str, project: &str) -> Config {
        Config { provider: provider.into(), base_url: base.into(), project: project.into() }
    }
    #[test]
    fn api_urls_payload_limits_and_remote_issue_links_are_validated() {
        let github = config("github", "https://api.github.com", "acme/rust");
        assert_eq!(endpoint(&github, "open", 2).unwrap().as_str(), "https://api.github.com/repos/acme/rust/issues?state=open&per_page=100&page=2");
        let gitlab = config("gitlab", "https://gitlab.example/api/v4", "group/subgroup/rust");
        assert!(endpoint(&gitlab, "open", 1).unwrap().as_str().contains("projects/group%2Fsubgroup%2Frust/issues?state=opened"));
        assert_eq!(issue_url(&gitlab, 42).unwrap(), "https://gitlab.example/group/subgroup/rust/-/issues/42");
        for base in ["http://example.test", "https://user:pass@example.test", "https://example.test?token=secret", "file:///tmp"] {
            assert!(checked(&config("github", base, "acme/rust")).is_err());
        }
        for project in ["../rust", "owner/../repo", "owner/repo/extra", "owner/repo?token=secret"] {
            assert!(checked(&config("github", "https://api.github.com", project)).is_err());
        }
        let data=br#"[{"number":42,"title":"Fix Rust borrowing","body":"Plain <script> text","state":"open","html_url":"javascript:evil()","user":{"login":"dev"}},{"number":43,"title":"A pull request","pull_request":{}}]"#;
        let issues = parse(&github, data).unwrap();
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].url, "https://github.com/acme/rust/issues/42");
        assert_eq!(reference(&github, &issues[0]), "acme/rust#42");
        assert_eq!(branch(&issues[0]), "issue-42-fix-rust-borrowing");
        assert!(issues[0].body.contains("<script>"));
        let large = format!("[{{\"number\":1,\"title\":\"{}\"}}]", "x".repeat(1025));
        assert!(matches!(parse(&github, large.as_bytes()), Err(Error::Limit)));
        let gl = parse(&gitlab, br#"[{"iid":7,"title":"Rust issue","description":"Body","state":"opened","author":{"username":"tester"}}]"#).unwrap();
        assert_eq!(gl[0].number, 7);
        assert!(gl[0].url.ends_with("/-/issues/7"));
    }
    #[tokio::test]
    async fn invalid_tokens_and_pre_cancelled_lookups_never_send_requests() {
        let cfg = config("github", "http://127.0.0.1:1", "acme/rust");
        let token = rc_dotnet::CancelToken::new();
        assert!(matches!(fetch(&cfg, "secret\r\nInjected: bad", "open", 1, &token).await, Err(Error::Token)));
        token.cancel();
        assert!(matches!(fetch(&cfg, "", "open", 1, &token).await, Err(Error::Cancelled)));
    }
    fn fixture(root: &std::path::Path) {
        std::fs::create_dir(root.join("src")).unwrap();
        std::fs::write(root.join("Cargo.toml"), "[package]\nname='issue-proof'\nversion='0.1.0'\nedition='2021'\n[workspace]\n").unwrap();
        std::fs::write(root.join("src/lib.rs"), "pub fn value()->u8{42}\n").unwrap();
        std::fs::write(root.join("build.rs"), "fn main(){std::fs::write(\"build-ran\",\"yes\").unwrap();}\n").unwrap();
        std::fs::write(root.join(".gitignore"), "/target/\n/Cargo.lock\n/build-ran\n").unwrap();
        for args in [
            vec!["init", "-q", "-b", "main"],
            vec!["config", "user.name", "Issues"],
            vec!["config", "user.email", "issues@example.test"],
            vec!["add", "."],
            vec!["commit", "-qm", "Rust issue fixture"],
        ] {
            let output = std::process::Command::new("git").args(args).current_dir(root).output().unwrap();
            assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        }
    }
    #[test]
    #[ignore = "Native read-only HTTP integration on a real Rust/Git project; binds a loopback API fixture"]
    fn native_issue_lookup_is_explicit_keeps_tokens_private_and_links_commits_and_branches() {
        use egui_kittest::kittest::Queryable;
        use std::{
            io::{Read, Write},
            time::Instant,
        };
        let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let root = project.path().canonicalize().unwrap();
        fixture(&root);
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let mut app = crate::screens::test_app(home.path(), true);
        app.open_workspace_and_wait(&root);
        app.trust_project_tools_for_test();
        app.issue_tracker.config = config("github", &format!("http://{address}"), "acme/rust");
        app.issue_tracker.token = "MEMORY_ONLY_SECRET".into();
        app.dock.sizes.insert(crate::toolwindow::ToolWindow::Git, 900.0);
        app.dock.show(crate::toolwindow::ToolWindow::Git);
        let mut h = crate::screens::harness(app, egui::vec2(1650.0, 1200.0));
        let start = Instant::now();
        while (h.state().vcs.status.is_none() || h.state().runner.cargo_workspace.is_none()) && start.elapsed() < Duration::from_secs(15) {
            h.run_steps(2);
            std::thread::sleep(Duration::from_millis(10));
        }
        h.get_by_label(tr("Issues")).click();
        h.run_steps(3);
        assert_eq!(listener.accept().unwrap_err().kind(), std::io::ErrorKind::WouldBlock);
        assert!(!root.join("build-ran").exists());
        listener.set_nonblocking(false).unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
            let mut bytes = [0u8; 8192];
            let count = stream.read(&mut bytes).unwrap();
            let request = String::from_utf8_lossy(&bytes[..count]).to_string();
            let body=br#"[{"number":42,"title":"Fix Rust borrowing","body":"Native issue body","state":"open","user":{"login":"dev"},"html_url":"javascript:ignored"},{"number":43,"title":"PR","pull_request":{}}]"#;
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).unwrap();
            stream.write_all(body).unwrap();
            request
        });
        h.get_by_label(tr("Fetch Issues")).click();
        h.run_steps(2);
        while h.state().issue_tracker.cancel.is_some() && start.elapsed() < Duration::from_secs(15) {
            h.run_steps(2);
            std::thread::sleep(Duration::from_millis(10));
        }
        let request = server.join().unwrap();
        assert!(request.starts_with("GET /repos/acme/rust/issues?state=open&per_page=100&page=1 HTTP/1.1"));
        assert!(request.to_lowercase().contains("authorization: bearer memory_only_secret"));
        assert_eq!(h.state().issue_tracker.issues.len(), 1, "{:?}", h.state().issue_tracker.error);
        assert!(!root.join("build-ran").exists());
        h.get_by_label("#42 Fix Rust borrowing").click();
        h.run_steps(2);
        h.state_mut().vcs.message = "Existing draft".into();
        h.get_by_label(tr("Use in Commit Message")).click();
        h.run_steps(2);
        assert!(h.state().vcs.message.starts_with("Existing draft\nRefs acme/rust#42"));
        h.state_mut().dock.show(crate::toolwindow::ToolWindow::Git);
        h.run_steps(2);
        h.get_by_label(tr("Create Issue Branch…")).click();
        h.run_steps(2);
        assert!(matches!(&h.state().dialog,Some(crate::ui::dialogs::Dialog::NewBranch{name})if name=="issue-42-fix-rust-borrowing"));
        h.get_by_label(tr("Create")).click();
        h.run_steps(2);
        while h.state().vcs.branch_name().as_deref() != Some("issue-42-fix-rust-borrowing") && start.elapsed() < Duration::from_secs(15) {
            h.run_steps(2);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(h.state().vcs.branch_name().as_deref(), Some("issue-42-fix-rust-borrowing"), "{:?}", h.state().vcs.error);
        let session = h.state().session();
        assert!(!serde_json::to_string(&session).unwrap().contains("MEMORY_ONLY_SECRET"));
        let mut fresh = Tracker::new(Waker::noop());
        fresh.restore(session.issue_tracker);
        assert!(fresh.token.is_empty());
        assert_eq!(fresh.config.project, "acme/rust");
        let output = rc_cargo::Cargo::locate().unwrap().std_command().args(["test", "--offline"]).current_dir(&root).output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        assert!(root.join("build-ran").exists());
    }
    #[tokio::test]
    #[ignore = "Read-only public GitHub API smoke test; no token and no remote writes"]
    async fn live_public_github_issues_follow_the_documented_contract() {
        let config = config("github", "https://api.github.com", "rust-lang/rust");
        let issues = fetch(&config, "", "open", 1, &rc_dotnet::CancelToken::new()).await.unwrap();
        assert!(!issues.is_empty());
        assert!(issues.iter().all(|issue| issue.url.starts_with("https://github.com/rust-lang/rust/issues/") && !issue.title.is_empty()));
    }
    #[tokio::test]
    #[ignore = "Loopback HTTP cancellation, redirect and response-size safety"]
    async fn pending_tracker_requests_cancel_and_redirects_never_receive_tokens() {
        use std::io::{Read, Write};
        let target = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        target.set_nonblocking(true).unwrap();
        let target_addr = target.local_addr().unwrap();
        let source = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let cfg = config("github", &format!("http://{}", source.local_addr().unwrap()), "acme/rust");
        let server = std::thread::spawn(move || {
            let (mut stream, _) = source.accept().unwrap();
            let mut bytes = [0u8; 4096];
            assert!(stream.read(&mut bytes).unwrap() > 0);
            write!(stream, "HTTP/1.1 302 Found\r\nLocation: http://{target_addr}/steal\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
        });
        assert!(matches!(fetch(&cfg, "secret", "open", 1, &rc_dotnet::CancelToken::new()).await, Err(Error::Status(302))));
        server.join().unwrap();
        assert_eq!(target.accept().unwrap_err().kind(), std::io::ErrorKind::WouldBlock);
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let cfg = config("gitlab", &format!("http://{}/api/v4", listener.local_addr().unwrap()), "group/rust");
        let (tx, rx) = std::sync::mpsc::channel();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut bytes = [0u8; 4096];
            let n = stream.read(&mut bytes).unwrap();
            assert!(String::from_utf8_lossy(&bytes[..n]).to_lowercase().contains("private-token: secret"));
            tx.send(()).unwrap();
            let result = stream.read(&mut bytes);
            assert!(matches!(result, Ok(0)));
        });
        let token = rc_dotnet::CancelToken::new();
        let task_token = token.clone();
        let task = tokio::spawn(async move { fetch(&cfg, "secret", "open", 1, &task_token).await });
        for _ in 0..200 {
            if rx.try_recv().is_ok() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        token.cancel();
        assert!(matches!(task.await.unwrap(), Err(Error::Cancelled)));
        server.join().unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let cfg = config("github", &format!("http://{}", listener.local_addr().unwrap()), "acme/rust");
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut bytes = [0u8; 4096];
            assert!(stream.read(&mut bytes).unwrap() > 0);
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: 2097153\r\nConnection: close\r\n\r\n").unwrap();
        });
        assert!(matches!(fetch(&cfg, "", "open", 1, &rc_dotnet::CancelToken::new()).await, Err(Error::Limit)));
        server.join().unwrap();
    }
}
