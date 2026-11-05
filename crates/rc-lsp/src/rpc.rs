//! JSON-RPC 2.0 over a child process's stdio with LSP's `Content-Length` framing.
//!
//! The client is usable from the UI thread without blocking: [`Client::request`] returns a
//! [`Pending`] the UI polls every frame (the response wakes it), dropping or cancelling it
//! sends `$/cancelRequest`. Server notifications and server→client requests arrive as
//! [`Incoming`] messages through an `rc_core::Outbox`.

use parking_lot::Mutex;
use rc_core::inbox::Outbox;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Instant;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{mpsc, oneshot};

#[derive(Clone, Debug, PartialEq)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.message, self.code)
    }
}

impl std::error::Error for RpcError {}

/// `RequestCancelled` / `ContentModified`: the answer was not needed anymore.
pub const REQUEST_CANCELLED: i64 = -32800;
pub const CONTENT_MODIFIED: i64 = -32801;

/// A message from the server that is not a response to one of our requests.
#[derive(Clone, Debug)]
pub enum Incoming {
    Notification { method: String, params: Value },
    /// A request from the server: answer it with [`Client::reply`].
    Request { id: Value, method: String, params: Value },
    /// A line the server wrote to stderr (logs).
    Stderr(String),
    /// The server process exited (or its pipes closed).
    Exited(Option<i32>),
}

type Responder = oneshot::Sender<Result<Value, RpcError>>;

struct Shared {
    pending: Mutex<HashMap<i64, Responder>>,
    out: mpsc::UnboundedSender<String>,
    next_id: AtomicI64,
}

/// Handle to a running server. Cheap to clone.
#[derive(Clone)]
pub struct Client {
    shared: Arc<Shared>,
    child: Arc<Mutex<Option<Child>>>,
}

/// The response of a request, polled without blocking.
pub struct Pending<T> {
    rx: Option<oneshot::Receiver<Result<Value, RpcError>>>,
    id: i64,
    client: Client,
    pub started: Instant,
    parse: fn(Value) -> Result<T, RpcError>,
}

impl<T> Pending<T> {
    /// The response if it arrived: `None` while waiting.
    pub fn poll(&mut self) -> Option<Result<T, RpcError>> {
        let rx = self.rx.as_mut()?;
        match rx.try_recv() {
            Ok(r) => {
                self.rx = None;
                Some(r.and_then(self.parse))
            }
            Err(oneshot::error::TryRecvError::Empty) => None,
            Err(oneshot::error::TryRecvError::Closed) => {
                self.rx = None;
                Some(Err(RpcError { code: -1, message: "the language server stopped".into() }))
            }
        }
    }

    /// Waits for the response (tests and background tasks only — never the UI thread).
    pub async fn wait(mut self) -> Result<T, RpcError> {
        let rx = self.rx.take().expect("already taken");
        match rx.await {
            Ok(r) => r.and_then(self.parse),
            Err(_) => Err(RpcError { code: -1, message: "the language server stopped".into() }),
        }
    }

    /// Asks the server to stop working on it; the response will not be delivered.
    pub fn cancel(mut self) {
        if self.rx.take().is_some() {
            self.client.cancel(self.id);
        }
    }

    pub fn is_done(&self) -> bool {
        self.rx.is_none()
    }

    pub fn id(&self) -> i64 {
        self.id
    }
}

impl<T> Drop for Pending<T> {
    fn drop(&mut self) {
        if self.rx.is_some() {
            self.client.cancel(self.id);
        }
    }
}

/// Frames a JSON body with its `Content-Length` header.
pub fn frame(body: &str) -> String {
    format!("Content-Length: {}\r\n\r\n{body}", body.len())
}

impl Client {
    /// Whether two handles still address the same language-server connection.
    pub fn same_connection(&self, other: &Self) -> bool { Arc::ptr_eq(&self.shared, &other.shared) }

    /// Starts `program` and talks LSP over its stdin/stdout. Must be called inside the tokio
    /// runtime (`rc_core::Runtime::handle().enter()` or from a task).
    pub fn spawn(program: &Path, args: &[String], cwd: &Path, env: &[(String, String)], incoming: Outbox<Incoming>) -> std::io::Result<Client> {
        Self::spawn_with_env_removals(program,args,cwd,env,&[],incoming)
    }
    /// Starts a transport without inheriting explicitly disallowed host configuration.
    pub fn spawn_with_env_removals(program:&Path,args:&[String],cwd:&Path,env:&[(String,String)],remove:&[&str],incoming:Outbox<Incoming>)->std::io::Result<Client>{
        let mut cmd = Command::new(program);
        cmd.args(args)
            .current_dir(cwd)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        for name in remove {cmd.env_remove(name);}
        for (k, v) in env {
            cmd.env(k, v);
        }
        #[cfg(unix)]
        cmd.process_group(0);
        #[cfg(windows)]
        cmd.creation_flags(0x0800_0000 | 0x0000_0200);
        let mut child = cmd.spawn()?;
        let stdin = child.stdin.take().expect("piped");
        let stdout = child.stdout.take().expect("piped");
        let stderr = child.stderr.take().expect("piped");
        let (out_tx, out_rx) = mpsc::unbounded_channel::<String>();
        let shared = Arc::new(Shared { pending: Mutex::new(HashMap::new()), out: out_tx, next_id: AtomicI64::new(1) });
        tokio::spawn(write_loop(stdin, out_rx));
        let reader_shared = shared.clone();
        let reader_incoming = incoming.clone();
        let child = Arc::new(Mutex::new(Some(child)));
        let exit_child = child.clone();
        tokio::spawn(async move {
            read_loop(stdout, reader_shared.clone(), reader_incoming.clone()).await;
            // Pipes closed: fail every pending request and report the exit code.
            for (_, tx) in reader_shared.pending.lock().drain() {
                let _ = tx.send(Err(RpcError { code: -1, message: "the language server stopped".into() }));
            }
            let code = exit_child.lock().as_mut().and_then(|c| c.try_wait().ok().flatten()).and_then(|s| s.code());
            if code.is_none() { Client { shared: reader_shared, child: exit_child }.kill(); }
            reader_incoming.send(Incoming::Exited(code));
        });
        let stderr_client=Client { shared: shared.clone(), child: child.clone() };
        tokio::spawn(async move {
            if let Err(error)=stderr_loop(stderr,&incoming,2*1024*1024).await {
                for (_,tx) in stderr_client.shared.pending.lock().drain() { let _=tx.send(Err(error.clone())); }
                incoming.send(Incoming::Stderr(error.message));
                stderr_client.kill();
            }
        });
        Ok(Client { shared, child })
    }

    /// Sends a request; the response is parsed with `serde` into `T`.
    pub fn request_raw<T: serde::de::DeserializeOwned>(&self, method: &str, params: Value) -> Pending<T> {
        let id = self.shared.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.shared.pending.lock().insert(id, tx);
        let body = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string();
        let _ = self.shared.out.send(frame(&body));
        fn parse<T: serde::de::DeserializeOwned>(v: Value) -> Result<T, RpcError> {
            serde_json::from_value(v).map_err(|e| RpcError { code: -2, message: format!("unexpected response: {e}") })
        }
        Pending { rx: Some(rx), id, client: self.clone(), started: Instant::now(), parse: parse::<T> }
    }

    /// A typed LSP request.
    pub fn request<R: lsp_types::request::Request>(&self, params: R::Params) -> Pending<R::Result>
    where
        R::Result: serde::de::DeserializeOwned,
    {
        self.request_raw(R::METHOD, serde_json::to_value(params).unwrap_or(Value::Null))
    }

    pub fn notify_raw(&self, method: &str, params: Value) {
        let body = json!({"jsonrpc": "2.0", "method": method, "params": params}).to_string();
        let _ = self.shared.out.send(frame(&body));
    }

    pub fn notify<N: lsp_types::notification::Notification>(&self, params: N::Params) {
        self.notify_raw(N::METHOD, serde_json::to_value(params).unwrap_or(Value::Null));
    }

    /// Answers a server→client request.
    pub fn reply(&self, id: Value, result: Result<Value, RpcError>) {
        let body = match result {
            Ok(v) => json!({"jsonrpc": "2.0", "id": id, "result": v}),
            Err(e) => json!({"jsonrpc": "2.0", "id": id, "error": {"code": e.code, "message": e.message}}),
        };
        let _ = self.shared.out.send(frame(&body.to_string()));
    }

    fn cancel(&self, id: i64) {
        if self.shared.pending.lock().remove(&id).is_some() {
            self.notify_raw("$/cancelRequest", json!({"id": id}));
        }
    }

    /// Kills the server process.
    pub fn kill(&self) {
        if let Some(mut c) = self.child.lock().take() {
            #[cfg(unix)]
            if let Some(pid) = c.id() {
                // SAFETY: spawn created this child's own group; never signal the IDE group.
                unsafe { libc::killpg(pid as libc::pid_t, libc::SIGKILL); }
            }
            #[cfg(windows)]
            {
                rc_core::Runtime::spawn(async move {
                    if let Some(pid) = c.id() {
                        let root = std::env::var_os("SystemRoot").map(std::path::PathBuf::from).unwrap_or_else(|| "C:\\Windows".into());
                        let mut command = tokio::process::Command::new(root.join("System32/taskkill.exe"));
                        command.args(["/PID", &pid.to_string(), "/T", "/F"]).creation_flags(0x0800_0000)
                            .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).kill_on_drop(true);
                        let _ = tokio::time::timeout(std::time::Duration::from_secs(2), command.status()).await;
                    }
                    let _ = c.start_kill(); let _ = c.wait().await;
                });
                return;
            }
            #[cfg(not(windows))]
            {
                let _ = c.start_kill();
                // Reap a killed child asynchronously instead of leaving it as a zombie.
                rc_core::Runtime::spawn(async move { let _ = c.wait().await; });
            }
        }
    }

    pub fn pending_count(&self) -> usize {
        self.shared.pending.lock().len()
    }
}

async fn write_loop(mut stdin: ChildStdin, mut rx: mpsc::UnboundedReceiver<String>) {
    while let Some(msg) = rx.recv().await {
        if stdin.write_all(msg.as_bytes()).await.is_err() || stdin.flush().await.is_err() {
            return;
        }
    }
}

/// Transport limits are reported to pending requests rather than allocating unbounded log lines.
pub const STDERR_LIMIT_ERROR:i64=-32097;
async fn stderr_loop(stderr:impl tokio::io::AsyncRead+Unpin,incoming:&Outbox<Incoming>,budget:usize)->Result<(),RpcError>{
 let mut reader=BufReader::new(stderr);let mut total=0usize;let mut lines=0usize;
 loop {let mut line=String::new();let count=(&mut reader).take(8193).read_line(&mut line).await.map_err(|_|RpcError{code:STDERR_LIMIT_ERROR,message:"language server stderr is invalid or unreadable".into()})?;if count==0{return Ok(());}total=total.saturating_add(count);lines=lines.saturating_add(1);if count>8192||total>budget||lines>8192{return Err(RpcError{code:STDERR_LIMIT_ERROR,message:"language server stderr exceeded its line or session limit".into()});}let text=line.trim_end_matches(['\r','\n']).to_owned();if !incoming.send(Incoming::Stderr(text)){return Ok(());}}
}

/// Reads framed messages until the pipe closes.
async fn read_loop(stdout: tokio::process::ChildStdout, shared: Arc<Shared>, incoming: Outbox<Incoming>) {
    let mut reader = BufReader::new(stdout);
    loop {
        let mut len: Option<usize> = None;
        let mut header_bytes = 0usize;
        loop {
            let mut line = String::new();
            match (&mut reader).take(8193).read_line(&mut line).await {
                Ok(0) | Err(_) => return,
                Ok(_) => {}
            }
            header_bytes += line.len();
            if line.len() > 8192 || header_bytes > 32 * 1024 { return; }
            let line = line.trim_end();
            if line.is_empty() {
                break;
            }
            if let Some(v) = line.strip_prefix("Content-Length:") {
                len = v.trim().parse().ok();
            }
        }
        let Some(len) = len else { return };
        if len > 32 * 1024 * 1024 { return; }
        let mut body = vec![0u8; len];
        if reader.read_exact(&mut body).await.is_err() {
            return;
        }
        let Ok(msg) = serde_json::from_slice::<Value>(&body) else { continue };
        dispatch(msg, &shared, &incoming);
    }
}

fn dispatch(msg: Value, shared: &Shared, incoming: &Outbox<Incoming>) {
    let method = msg.get("method").and_then(Value::as_str).map(str::to_string);
    let id = msg.get("id").cloned();
    match (method, id) {
        (Some(method), Some(id)) => {
            incoming.send(Incoming::Request { id, method, params: msg.get("params").cloned().unwrap_or(Value::Null) });
        }
        (Some(method), None) => {
            incoming.send(Incoming::Notification { method, params: msg.get("params").cloned().unwrap_or(Value::Null) });
        }
        (None, Some(id)) => {
            let Some(id) = id.as_i64() else { return };
            let Some(tx) = shared.pending.lock().remove(&id) else { return };
            let result = if let Some(err) = msg.get("error") {
                Err(RpcError {
                    code: err.get("code").and_then(Value::as_i64).unwrap_or(-1),
                    message: err.get("message").and_then(Value::as_str).unwrap_or("").to_string(),
                })
            } else {
                Ok(msg.get("result").cloned().unwrap_or(Value::Null))
            };
            let _ = tx.send(result);
        }
        (None, None) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rc_core::{Inbox, Runtime, Waker};

    /// A tiny "server": a shell script that answers one request with the same id.
    #[test]
    fn requests_get_their_responses_and_notifications_arrive() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("echo-server.py");
        std::fs::write(
            &script,
            r#"import sys, json
def read():
    n = 0
    while True:
        line = sys.stdin.buffer.readline()
        if not line: sys.exit(0)
        line = line.decode().strip()
        if not line: break
        if line.startswith('Content-Length:'): n = int(line.split(':')[1])
    return json.loads(sys.stdin.buffer.read(n))
def send(obj):
    b = json.dumps(obj).encode()
    sys.stdout.buffer.write(b'Content-Length: %d\r\n\r\n' % len(b) + b); sys.stdout.buffer.flush()
while True:
    m = read()
    if 'id' in m:
        send({'jsonrpc': '2.0', 'method': 'log', 'params': {'saw': m['method']}})
        send({'jsonrpc': '2.0', 'id': m['id'], 'result': {'echo': m['params']}})
"#,
        )
        .unwrap();
        let inbox: Inbox<Incoming> = Inbox::new(Waker::noop());
        let out = inbox.sender();
        let _guard = Runtime::handle().enter();
        let client = Client::spawn(Path::new("python3"), &["-u".into(), script.display().to_string()], dir.path(), &[], out).unwrap();
        let pending = client.request_raw::<Value>("test/echo", json!({"x": 1}));
        let result = Runtime::block_on(pending.wait()).unwrap();
        assert_eq!(result, json!({"echo": {"x": 1}}));
        let note = inbox.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        assert!(matches!(note, Incoming::Notification { method, .. } if method == "log"));
        client.kill();
    }
    #[test]
    fn oversized_or_malformed_lsp_frames_fail_pending_requests_without_allocating_the_body() {
        use std::time::{Duration, Instant};
        for header in ["Content-Length: 9999999999999999999\r\n\r\n".to_string(), format!("X: {}\r\n\r\n", "a".repeat(9000))] {
            let dir = tempfile::tempdir().unwrap(); let script = dir.path().join("bad_frame.py");
            let header = serde_json::to_string(&header).unwrap();
            std::fs::write(&script, format!("import sys,time\nsys.stdin.buffer.readline()\nsys.stdout.buffer.write(({header}).encode());sys.stdout.buffer.flush()\ntime.sleep(30)\n")).unwrap();
            let inbox = Inbox::new(Waker::noop());
            let _runtime = Runtime::handle().enter();
            let client = Client::spawn(Path::new("python3"), &[script.display().to_string()], dir.path(), &[], inbox.sender()).unwrap();
            let mut request = client.request_raw::<Value>("test", json!({}));
            let end = Instant::now() + Duration::from_secs(3);
            loop {
                if let Some(reply) = request.poll() { assert!(reply.is_err()); break; }
                assert!(Instant::now() < end); std::thread::sleep(Duration::from_millis(10));
            }
            client.kill();
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn killing_a_language_server_stops_its_owned_helper_process_group() {
        use std::time::{Duration, Instant};
        let dir = tempfile::tempdir().unwrap(); let script = dir.path().join("server.py");
        let pid_file = dir.path().join("helper.pid");
        let pid_json = serde_json::to_string(&pid_file.to_string_lossy()).unwrap();
        std::fs::write(&script, format!("import subprocess,time\nchild=subprocess.Popen(['sleep','30'])\nwith open({pid_json},'w') as f: f.write(str(child.pid))\ntime.sleep(30)\n")).unwrap();
        let _runtime = Runtime::handle().enter(); let inbox = Inbox::new(Waker::noop());
        let client = Client::spawn(Path::new("python3"), &[script.display().to_string()], dir.path(), &[], inbox.sender()).unwrap();
        let end = Instant::now() + Duration::from_secs(3);
        let pid = loop {
            if let Some(pid) = std::fs::read_to_string(&pid_file).ok().and_then(|text| text.parse::<u32>().ok()) { break pid; }
            assert!(Instant::now() < end); std::thread::sleep(Duration::from_millis(10));
        };
        client.kill();
        loop {
            let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok();
            let running = stat.as_deref().and_then(|stat| stat.rsplit_once(") ")).is_some_and(|(_, state)| !state.starts_with('Z'));
            if !running { break; }
            assert!(Instant::now() < end, "the killed server's helper is still running");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

}

#[cfg(test)]
mod stderr_budget_tests {
 use super::*;
 #[test]
 fn fragmented_utf8_logs_are_preserved_and_both_budgets_are_enforced(){rc_core::Runtime::block_on(async{
  let inbox=rc_core::Inbox::new(rc_core::Waker::noop());let(mut writer,reader)=tokio::io::duplex(8);let task=tokio::spawn(async move{for byte in "log 🦀\r\n".as_bytes(){writer.write_all(&[*byte]).await.unwrap();tokio::task::yield_now().await;} });stderr_loop(reader,&inbox.sender(),64).await.unwrap();task.await.unwrap();let lines=inbox.drain();assert!(matches!(&lines[..],[Incoming::Stderr(line)]if line=="log 🦀"));
  let error=stderr_loop(&b"123456789\n"[..],&inbox.sender(),8).await.unwrap_err();assert_eq!(error.code,STDERR_LIMIT_ERROR);assert!(inbox.drain().is_empty());let bytes=vec![b'x';8193];let error=stderr_loop(bytes.as_slice(),&inbox.sender(),20000).await.unwrap_err();assert_eq!(error.code,STDERR_LIMIT_ERROR);assert!(inbox.drain().is_empty());
 });}
 #[cfg(unix)]
 #[test]
 fn oversized_stderr_fails_pending_requests_and_terminates_the_server(){rc_core::Runtime::block_on(async{
  let inbox=rc_core::Inbox::new(rc_core::Waker::noop());let client=Client::spawn(Path::new("/bin/sh"),&["-c".into(),"sleep 0.1; printf '%8193s' '' >&2; sleep 60".into()],Path::new("/tmp"),&[],inbox.sender()).unwrap();let request=client.request_raw::<Value>("initialize",json!({}));let error=tokio::time::timeout(std::time::Duration::from_secs(5),request.wait()).await.unwrap().unwrap_err();assert_eq!(error.code,STDERR_LIMIT_ERROR);assert!(client.child.lock().is_none());
 });}
}
