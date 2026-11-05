//! Headless execution of one explicitly selected HTTP request. Responses are JSON so remote
//! terminal escape sequences cannot become terminal commands.
use crate::{cli::{CliError, Command}, http_client::{self, HttpError}};
use rc_kit::i18n::tr;
use std::{ffi::OsString, io::Read, path::{Path, PathBuf}, sync::atomic::AtomicBool};

#[derive(Debug, PartialEq, Eq)]
pub struct HttpCommand { pub file: PathBuf, pub request: Option<usize> }

pub fn parse(args: impl Iterator<Item = OsString>, cwd: &Path) -> Result<Command, CliError> {
    let (mut file, mut request, mut paths_only) = (None, None, false);
    let mut args = args;
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--") if !paths_only => paths_only = true,
            Some("-h" | "--help") if !paths_only => return Ok(Command::Help),
            Some("--request") if !paths_only => request = Some(crate::cli::number("--request", args.next())?),
            Some(text) if !paths_only && text.starts_with('-') => return Err(CliError::UnknownOption(text.to_string())),
            _ if file.is_none() => {
                let path = PathBuf::from(arg);
                file = Some(if path.is_absolute() { path } else { cwd.join(path) });
            }
            _ => return Err(CliError::HttpFile),
        }
    }
    Ok(Command::Http(HttpCommand { file: file.ok_or(CliError::HttpFile)?, request }))
}

#[derive(Debug)]
pub enum Error { Io(std::io::Error), Json(serde_json::Error), TooLarge, NotFile, SelectRequest, NoRequest, Http(HttpError) }
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => e.fmt(f), Self::Json(e) => e.fmt(f),
            Self::TooLarge => f.write_str(tr("The HTTP request file exceeds 8 MB.")),
            Self::NotFile => f.write_str(tr("The HTTP request path must be a regular file.")),
            Self::SelectRequest => f.write_str(tr("This file contains multiple requests. Select one with --request N.")),
            Self::NoRequest => f.write_str(tr("No HTTP request exists at this index.")),
            Self::Http(e) => f.write_str(&e.text()),
        }
    }
}
impl std::error::Error for Error {}
impl HttpCommand {
    /// No project model or project code is loaded. Output is streamed only after a bounded
    /// response has arrived; Ctrl+C terminates this foreground process.
    pub fn execute(&self) -> Result<(serde_json::Value, i32), Error> {
        let metadata = std::fs::metadata(&self.file).map_err(Error::Io)?;
        if !metadata.is_file() { return Err(Error::NotFile); }
        if metadata.len() > http_client::MAX_BODY_BYTES as u64 { return Err(Error::TooLarge); }
        let mut options = std::fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        { use std::os::unix::fs::OpenOptionsExt; options.custom_flags(libc::O_NONBLOCK); }
        let file = options.open(&self.file).map_err(Error::Io)?;
        if !file.metadata().map_err(Error::Io)?.is_file() { return Err(Error::NotFile); }
        let mut text = String::new();
        file.take((http_client::MAX_BODY_BYTES + 1) as u64).read_to_string(&mut text).map_err(Error::Io)?;
        if text.len() > http_client::MAX_BODY_BYTES { return Err(Error::TooLarge); }
        let requests = rc_syntax::http::parse_requests(&text);
        if self.request.is_none() && requests.len() > 1 { return Err(Error::SelectRequest); }
        let index = self.request.unwrap_or(1).checked_sub(1).ok_or(Error::NoRequest)?;
        let request = requests.get(index).ok_or(Error::NoRequest)?;
        let prepared = http_client::prepare(request).map_err(Error::Http)?;
        let response = http_client::send(&prepared, http_client::TIMEOUT, http_client::MAX_BODY_BYTES, &AtomicBool::new(false)).map_err(Error::Http)?;
        let warnings: Vec<_> = request.unsupported.iter().filter(|item| !matches!(item.kind, rc_syntax::http::UnsupportedKind::Variable(_) | rc_syntax::http::UnsupportedKind::FileVariable(_))).map(|item| http_client::unsupported_text(&item.kind)).collect();
        let body = std::str::from_utf8(&response.body).ok();
        let json = serde_json::json!({
            "request": index + 1, "method": prepared.method, "url": prepared.url,
            "status": response.status, "headers": response.headers,
            "body": body, "bodyBytes": if body.is_none() { Some(&response.body) } else { None },
            "truncated": response.truncated, "elapsedMs": response.elapsed.as_millis(), "warnings": warnings,
        });
        let exit = if response.status >= 400 || response.truncated { 1 } else { 0 };
        Ok((json, exit))
    }

    pub fn run(&self) -> Result<i32, Error> {
        use std::io::Write;
        let (json, code) = self.execute()?;
        let mut stdout = std::io::stdout().lock();
        serde_json::to_writer(&mut stdout, &json).map_err(Error::Json)?;
        stdout.write_all(b"\n").map_err(Error::Io)?;
        Ok(code)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn http_arguments_and_ambiguous_files_do_not_send_requests() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(crate::cli::parse(["http".into()], dir.path()), Err(CliError::HttpFile)));
        assert!(crate::cli::parse(["http".into(), "x".into(), "--request".into(), "0".into()], dir.path()).is_err());
        let file = dir.path().join("requests.http");
        std::fs::write(&file, "GET http://127.0.0.1:1/\n###\nGET http://127.0.0.1:1/other\n").unwrap();
        assert!(matches!(HttpCommand { file: file.clone(), request: None }.execute(), Err(Error::SelectRequest)));
        assert!(matches!(HttpCommand { file: file.clone(), request: Some(3) }.execute(), Err(Error::NoRequest)));
        std::fs::write(&file, "GET http://{{host}}/\n").unwrap();
        assert!(matches!(HttpCommand { file, request: None }.execute(), Err(Error::Http(HttpError::Variable(_)))));
    }

    #[test]
    fn http_cli_sends_the_selected_request_and_returns_safe_json_and_exit_status() {
        use std::io::{Read, Write};
        let server = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = server.local_addr().unwrap();
        let thread = std::thread::spawn(move || {
            let (mut stream, _) = server.accept().unwrap();
            stream.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
            let mut bytes = [0; 4096];
            let count = stream.read(&mut bytes).unwrap();
            let request = String::from_utf8_lossy(&bytes[..count]);
            assert!(request.starts_with("GET /selected HTTP/1.1"), "{request}");
            stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 7\r\nConnection: close\r\n\r\n\x1b[2Jbad").unwrap();
        });
        let project = tempfile::tempdir().unwrap();
        std::fs::write(project.path().join("Cargo.toml"), "[package]\nname=\"http_fixture\"\nversion=\"0.1.0\"\n").unwrap();
        std::fs::create_dir(project.path().join("src")).unwrap();
        std::fs::write(project.path().join("src/main.rs"), "fn main() {}\n").unwrap();
        std::fs::write(project.path().join("build.rs"), "compile_error!(\"HTTP CLI must never build this project\");").unwrap();
        let file = project.path().join("requests.http");
        std::fs::write(&file, format!("@host = {address}\n@base = http://{{{{host}}}}\n###\nGET {{{{base}}}}/unused\n###\nGET {{{{base}}}}/selected\n")).unwrap();
        let (result, code) = HttpCommand { file, request: Some(2) }.execute().unwrap();
        thread.join().unwrap();
        assert_eq!(code, 1);
        assert_eq!(result["status"], 404);
        assert_eq!(result["body"], "\x1b[2Jbad");
        assert!(!serde_json::to_string(&result).unwrap().contains('\x1b'));
        assert!(!project.path().join("target").exists());
    }
}
