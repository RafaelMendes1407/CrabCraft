//! Reviewed, explicitly trusted response scripts through a separately installed Node runtime.
//! Node has the user's OS permissions; VM timeouts are resource bounds, never a sandbox claim.
use super::{Exchange, Outcome};
use rc_core::{Inbox, Waker};
use rc_kit::i18n::tr;
use std::{path::PathBuf, process::Stdio, time::Duration};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
const SOURCE: usize = 64 * 1024;
const RESPONSE: usize = 1024 * 1024;
const OUTPUT: usize = 128 * 1024;
const DRIVER: &str = r#"
'use strict';
const fs = require('node:fs');
const vm = require('node:vm');
const exit = process.exit.bind(process);
const write = process.stdout.write.bind(process.stdout);
const input = JSON.parse(fs.readFileSync(0, 'utf8'));
const reports = [], logs = [];
const text = (v,n) => String(v).slice(0,n);
let assertions = 0;
const client = Object.freeze({
  test(name, fn) {
    if (reports.length >= 128) throw Error(input.messages.tests);
    try { fn(); reports.push({name:text(name,256),passed:true,message:''}); }
    catch(error) { reports.push({name:text(name,256),passed:false,message:text(error && error.message,2048)}); }
  },
  assert(value, message) { assertions++; if(assertions>4096) throw Error(input.messages.assertions); if(!value) throw Error(text(message || input.messages.failed,2048)); },
  log(value) { if(logs.length>=64) throw Error(input.messages.logs); logs.push(text(value,2048)); }
});
let body = input.response.body;
if(input.response.json) { try { body = JSON.parse(body); } catch(_) {} }
globalThis.response = Object.freeze({ status:input.response.status, body,
  contentType:input.response.contentType,
  headers:Object.freeze({ valueOf(name) { const key=String(name).toLowerCase(); return input.response.headers.filter(([n])=>n.toLowerCase()===key).map(([,v])=>v).join(', '); } }) });
globalThis.client = client;
try { vm.runInThisContext(input.script, {timeout:5000,filename:'reviewed-response.js'}); }
catch(error) { reports.push({name:input.messages.script,passed:false,message:text(error && error.message,2048)}); }
write(JSON.stringify({reports,logs})); exit(0);
"#;
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    Trust,
    Input,
    Process,
    Output,
    Cancelled,
    Limit,
}
impl Error {
    fn text(&self) -> String {
        tr(match self {
    Self::Trust=>"Trust the project and explicitly approve the response script before running it.",
    Self::Input=>"The response script or response is missing, stale, binary, or exceeds its limit.",
    Self::Process=>"The response script runtime failed; install Node.js separately and select its executable.",
    Self::Output=>"The response script returned invalid or oversized results.",
    Self::Cancelled=>"Cancelled",
    Self::Limit=>"The response script exceeded its time or output limit.",
}).to_string()
    }
}
#[derive(Clone, serde::Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct Report {
    pub name: String,
    pub passed: bool,
    pub message: String,
}
#[derive(Clone, serde::Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct Results {
    pub reports: Vec<Report>,
    pub logs: Vec<String>,
}
fn input(exchange: &Exchange) -> Result<Vec<u8>, Error> {
    let script = exchange
        .response_script
        .as_ref()
        .filter(|s| s.len() <= SOURCE)
        .ok_or(Error::Input)?;
    let Outcome::Done { response, .. } = &exchange.outcome else {
        return Err(Error::Input);
    };
    if response.truncated
        || response.body.len() > RESPONSE
        || response
            .headers
            .iter()
            .map(|(k, v)| k.len() + v.len())
            .sum::<usize>()
            > 64 * 1024
    {
        return Err(Error::Input);
    }
    let body = std::str::from_utf8(&response.body).map_err(|_| Error::Input)?;
    let content = response.header("content-type").unwrap_or_default();
    serde_json::to_vec(&serde_json::json!({"script":script,"messages":{"tests":tr("HTTP script test limit reached"),"assertions":tr("HTTP script assertion limit reached"),"failed":tr("HTTP script assertion failed"),"logs":tr("HTTP script log limit reached"),"script":tr("Response script")},"response":{"status":response.status,"body":body,"contentType":content,"json":content.to_ascii_lowercase().contains("json"),"headers":response.headers}})).map_err(|_|Error::Input)
}
struct Group(Option<u32>);
impl Drop for Group {
    fn drop(&mut self) {
        if let Some(pid) = self.0 {
            rc_dotnet::kill_tree(pid);
        }
    }
}
async fn limited<R: AsyncRead + Unpin>(reader: R) -> Result<Vec<u8>, Error> {
    let mut bytes = Vec::new();
    reader
        .take(OUTPUT as u64 + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|_| Error::Process)?;
    if bytes.len() > OUTPUT {
        return Err(Error::Limit);
    }
    Ok(bytes)
}
async fn run(
    program: PathBuf,
    input: Vec<u8>,
    trusted: bool,
    approved: bool,
    stop: rc_dotnet::CancelToken,
) -> Result<Results, Error> {
    if !trusted || !approved {
        return Err(Error::Trust);
    }
    if !program.is_absolute() || input.len() > 2 * RESPONSE || stop.is_cancelled() {
        return Err(if stop.is_cancelled() {
            Error::Cancelled
        } else {
            Error::Input
        });
    }
    let dir = tempfile::Builder::new()
        .prefix("crabcraft-http-script-")
        .tempdir()
        .map_err(|_| Error::Process)?;
    let driver = dir.path().join("driver.cjs");
    std::fs::write(&driver, DRIVER).map_err(|_| Error::Process)?;
    let mut command = tokio::process::Command::new(program);
    command
        .arg("--max-old-space-size=128")
        .arg(driver)
        .current_dir(dir.path())
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    let mut child = command.spawn().map_err(|_| Error::Process)?;
    let group = Group(child.id());
    let mut stdin = child.stdin.take().ok_or(Error::Process)?;
    let stdout = child.stdout.take().ok_or(Error::Process)?;
    let stderr = child.stderr.take().ok_or(Error::Process)?;
    let work = async {
        let write = async move {
            stdin.write_all(&input).await.map_err(|_| Error::Process)?;
            stdin.shutdown().await.map_err(|_| Error::Process)?;
            drop(stdin);
            Ok::<_, Error>(())
        };
        let (_, bytes, _) = tokio::try_join!(write, limited(stdout), limited(stderr))?;
        if !child.wait().await.map_err(|_| Error::Process)?.success() {
            return Err(Error::Process);
        }
        let results: Results = serde_json::from_slice(&bytes).map_err(|_| Error::Output)?;
        if results.reports.len() > 129
            || results.logs.len() > 64
            || results
                .reports
                .iter()
                .any(|r| r.name.len() > 1024 || r.message.len() > 8192)
            || results.logs.iter().any(|s| s.len() > 8192)
        {
            return Err(Error::Output);
        }
        Ok(results)
    };
    let result = tokio::select! {result=tokio::time::timeout(Duration::from_secs(10),work)=>result.map_err(|_|Error::Limit)?,()=stop.cancelled()=>Err(Error::Cancelled)};
    drop(group);
    let _ = child.start_kill();
    let _ = child.wait().await;
    result
}
pub struct State {
    pub program: String,
    pub approved: bool,
    selected: Option<u64>,
    inbox: Inbox<(u64, Result<Results, Error>)>,
    generation: u64,
    stop: Option<rc_dotnet::CancelToken>,
    results: Option<Results>,
    error: Option<Error>,
}
impl State {
    pub fn new(waker: Waker) -> Self {
        Self {
            program: if cfg!(unix) {
                "/usr/bin/node".into()
            } else {
                String::new()
            },
            approved: false,
            selected: None,
            inbox: Inbox::new(waker),
            generation: 0,
            stop: None,
            results: None,
            error: None,
        }
    }
    pub fn enforce_trust(&mut self, trusted: bool) {
        if !trusted {
            self.cancel();
            self.approved = false;
            self.results = None;
            self.error = None;
        }
    }
    pub fn cancel(&mut self) {
        if let Some(stop) = self.stop.take() {
            stop.cancel();
        }
    }
    pub fn poll(&mut self) -> bool {
        let events = self.inbox.drain();
        let changed = !events.is_empty();
        for (generation, result) in events {
            if generation != self.generation || self.stop.is_none() {
                continue;
            }
            self.stop = None;
            match result {
                Ok(results) => self.results = Some(results),
                Err(error) => self.error = Some(error),
            }
        }
        changed
    }
    fn start(&mut self, exchange: &Exchange, trusted: bool) {
        self.cancel();
        self.results = None;
        self.error = None;
        if !trusted || !self.approved {
            self.approved = false;
            self.error = Some(Error::Trust);
            return;
        }
        let bytes = match input(exchange) {
            Ok(bytes) => bytes,
            Err(error) => {
                self.error = Some(error);
                return;
            }
        };
        let program = PathBuf::from(&self.program);
        let approved = self.approved;
        self.approved = false;
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        let tx = self.inbox.sender();
        let stop = rc_dotnet::CancelToken::default();
        self.stop = Some(stop.clone());
        rc_core::Runtime::spawn(async move {
            tx.send((
                generation,
                run(program, bytes, trusted, approved, stop).await,
            ));
        });
    }
    pub fn show(&mut self, ui: &mut egui::Ui, exchange: Option<&Exchange>, trusted: bool) {
        let selected = exchange.map(|e| e.id);
        if selected != self.selected || !trusted {
            self.cancel();
            self.approved = false;
            self.results = None;
            self.error = None;
            self.selected = selected;
        }
        egui::CollapsingHeader::new(tr("Reviewed response script")).show(ui,|ui|{
            ui.label(tr("Inline scripts never run on send. Node.js runs with your user permissions and can access files and network; this is not an OS sandbox."));
            ui.horizontal(|ui|{ui.label(tr("Node.js executable"));if ui.add(egui::TextEdit::singleline(&mut self.program).char_limit(4096)).changed(){self.cancel();self.approved=false;self.results=None;self.error=None;}});
            let Some(exchange)=exchange else{ui.label(tr("Send a request with one inline response script first."));return;};
            let Some(script)=&exchange.response_script else{ui.label(tr("Only one complete inline response script is supported; external scripts are unavailable."));return;};
            ui.label(tr("Review the complete script before approving execution."));
            egui::ScrollArea::both().id_salt("http_script_preview").max_height(160.0).show(ui,|ui|{ui.add(egui::Label::new(egui::RichText::new(script).monospace()).selectable(true));});
            ui.add_enabled(trusted && self.stop.is_none(),egui::Checkbox::new(&mut self.approved,tr("I trust this script and approve one execution with my user permissions")));
            ui.horizontal(|ui|{if ui.add_enabled(trusted && self.approved && self.stop.is_none(),egui::Button::new(tr("Run reviewed response script"))).clicked(){self.start(exchange,trusted);}if ui.add_enabled(self.stop.is_some(),egui::Button::new(tr("Cancel"))).clicked(){self.cancel();self.error=Some(Error::Cancelled);}});
            if !trusted{ui.label(tr("Trust the project and explicitly approve the response script before running it."));}
            if let Some(results)=&self.results{for report in &results.reports{ui.label(format!("{} {} {}",if report.passed{"✓"}else{"✗"},report.name,report.message));}for log in &results.logs{ui.label(log);}}
            if let Some(error)=&self.error{ui.colored_label(rc_kit::pal(ui).error,error.text());}
        });
    }
}
impl Drop for State {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn exchange(script: &str) -> Exchange {
        let request = rc_syntax::http::parse_requests(&format!(
            "GET http://localhost/\n\n> {{%\n{script}\n%}}\n"
        ))
        .remove(0);
        Exchange {
            id: 1,
            title: "fixture".into(),
            method: "GET".into(),
            url: "http://localhost/".into(),
            file: PathBuf::from("api.http"),
            line: 0,
            notes: Vec::new(),
            outcome: Outcome::Done {
                response: super::super::HttpResponse {
                    status: 200,
                    reason: "OK".into(),
                    version: "HTTP/1.1".into(),
                    headers: vec![("Content-Type".into(), "application/json".into())],
                    body: br#"{"value":42}"#.to_vec(),
                    truncated: false,
                    to_headers: Duration::ZERO,
                    elapsed: Duration::ZERO,
                },
                body: super::super::BodyView::Empty,
            },
            started: std::time::Instant::now(),
            response_script: request.response_script,
        }
    }
    #[test]
    fn inline_extraction_is_bounded_and_never_treats_handlers_as_request_bodies() {
        let parse = |text: &str| rc_syntax::http::parse_requests(text).remove(0);
        let request = parse(
            "POST http://localhost/\n\n{\"a\":1}\n> {% client.assert(response.status === 200); %}\n",
        );
        assert_eq!(request.body, "{\"a\":1}");
        assert!(request.response_script.unwrap().contains("client.assert"));
        for text in [
            "GET http://localhost/\n\n> external.js\n".to_string(),
            "GET http://localhost/\n\n> {% unclosed".into(),
            "GET http://localhost/\n\n> {% a(); %}\n> {% b(); %}\n".into(),
            format!(
                "GET http://localhost/\n\n> {{% {} %}}\n",
                "x".repeat(SOURCE + 1)
            ),
        ] {
            assert!(parse(&text).response_script.is_none());
        }
        let mut response = exchange("client.assert(true);");
        if let Outcome::Done { response, .. } = &mut response.outcome {
            response.body = vec![0xff];
        }
        assert!(matches!(input(&response), Err(Error::Input)));
    }
    #[test]
    #[ignore = "Requires separately installed Node.js at /usr/bin/node"]
    fn real_node_checks_json_assertions_and_cancellation_without_implicit_execution() {
        rc_core::Runtime::block_on(async {
            let node = PathBuf::from("/usr/bin/node");
            assert!(
                node.is_file(),
                "This proof requires Node.js installed separately"
            );
            let fixture = exchange(
                "client.test('status', () => client.assert(response.status === 200)); client.test('json', () => client.assert(response.body.value === 42)); client.test('failure', () => client.assert(false, 'expected failure')); client.log(response.headers.valueOf('Content-Type'));",
            );
            let bytes = input(&fixture).unwrap();
            assert!(matches!(
                run(
                    node.clone(),
                    bytes.clone(),
                    false,
                    true,
                    rc_dotnet::CancelToken::default()
                )
                .await,
                Err(Error::Trust)
            ));
            assert!(matches!(
                run(
                    node.clone(),
                    bytes.clone(),
                    true,
                    false,
                    rc_dotnet::CancelToken::default()
                )
                .await,
                Err(Error::Trust)
            ));
            let results = run(
                node.clone(),
                bytes,
                true,
                true,
                rc_dotnet::CancelToken::default(),
            )
            .await
            .unwrap();
            assert_eq!(
                results.reports.iter().map(|r| r.passed).collect::<Vec<_>>(),
                [true, true, false]
            );
            assert_eq!(results.logs, ["application/json"]);
            let stop = rc_dotnet::CancelToken::default();
            let flag = stop.clone();
            let job = tokio::spawn(run(
                node,
                input(&exchange("while(true){};")).unwrap(),
                true,
                true,
                stop,
            ));
            tokio::time::sleep(Duration::from_millis(100)).await;
            flag.cancel();
            let start = std::time::Instant::now();
            assert!(matches!(job.await.unwrap(), Err(Error::Cancelled)));
            assert!(start.elapsed() < Duration::from_secs(3));
        });
    }
    #[test]
    #[ignore = "Requires separately installed Node.js at /usr/bin/node"]
    fn native_response_script_requires_trust_review_and_one_run_in_a_real_rust_project() {
        use egui_kittest::kittest::Queryable;
        let _home = crate::screens::HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        std::fs::create_dir(project.path().join("src")).unwrap();
        std::fs::write(project.path().join("Cargo.toml"),"[package]\nname=\"response_script_proof\"\nversion=\"0.1.0\"\nedition=\"2024\"\n[workspace]\n").unwrap();
        std::fs::write(
            project.path().join("src/main.rs"),
            "fn main(){println!(\"script proof\");}\n",
        )
        .unwrap();
        std::fs::write(
            project.path().join("build.rs"),
            "fn main(){std::fs::write(\"build-ran\",\"yes\").unwrap();}",
        )
        .unwrap();
        let marker = project.path().join("script-ran");
        let quoted = serde_json::to_string(&marker.to_string_lossy()).unwrap();
        let source = format!(
            "client.test('json', () => client.assert(response.body.value === 42)); process.mainModule.require('node:fs').writeFileSync({quoted}, 'approved');"
        );
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut peer, _) = listener.accept().unwrap();
            peer.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut bytes = [0; 4096];
            assert!(peer.read(&mut bytes).unwrap() > 0);
            peer.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 12\r\nConnection: close\r\n\r\n{\"value\":42}").unwrap();
        });
        let request =
            rc_syntax::http::parse_requests(&format!("GET {endpoint}/\n\n> {{%\n{source}\n%}}\n"))
                .remove(0);
        let mut app = crate::screens::test_app(home.path(), true);
        app.open_workspace_and_wait(project.path());
        app.http.start(&request, project.path().join("api.http"));
        app.toggle_tool(crate::toolwindow::ToolWindow::Http);
        app.dock
            .sizes
            .insert(crate::toolwindow::ToolWindow::Http, 680.0);
        let mut h = crate::screens::harness(app, egui::vec2(1280.0, 1000.0));
        let response_deadline = std::time::Instant::now() + Duration::from_secs(10);
        while h.state().http.running() {
            h.run_steps(1);
            assert!(std::time::Instant::now() < response_deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        server.join().unwrap();
        assert!(!marker.exists());
        h.get_by_label(tr("Reviewed response script")).click();
        h.run_steps(3);
        assert!(!marker.exists());
        h.state_mut().http.scripts.approved = true;
        let fixture = exchange(&source);
        h.state_mut().http.scripts.start(&fixture, false);
        assert!(matches!(h.state().http.scripts.error, Some(Error::Trust)));
        assert!(!marker.exists());
        h.state_mut().trust_project_tools_for_test();
        h.run_steps(3);
        assert!(!marker.exists());
        assert!(!project.path().join("build-ran").exists());
        h.get_by_label(tr(
            "I trust this script and approve one execution with my user permissions",
        ))
        .click();
        h.run_steps(2);
        h.get_by_label(tr("Run reviewed response script")).click();
        h.run_steps(2);
        let until = std::time::Instant::now() + Duration::from_secs(10);
        while h.state().http.scripts.stop.is_some() {
            h.run_steps(1);
            assert!(std::time::Instant::now() < until);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(marker.exists(), "{:?}", h.state().http.scripts.error);
        assert!(!h.state().http.scripts.approved);
        assert!(h.state().http.scripts.results.as_ref().unwrap().reports[0].passed);
        assert!(!project.path().join("build-ran").exists());
        h.state_mut().http.scripts.approved = true;
        h.state_mut().http.selected = 1;
        h.run_steps(2);
        assert!(!h.state().http.scripts.approved);
        let output = std::process::Command::new("cargo")
            .args(["run", "--offline", "--quiet"])
            .current_dir(project.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
