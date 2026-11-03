//! One running CrabCraft receives the files that later `crabcraft file.rs` calls ask to open.
//!
//! The first instance listens on a local endpoint in the data folder: a Unix socket (only the
//! user can reach it, as the folder is theirs), or on Windows a loopback TCP port written to a
//! file together with a random token that every request has to repeat. A later call sends its
//! requests there and exits; when nobody answers it becomes the window itself.
//!
//! What arrives is untrusted: the size is capped, the text must parse, and only absolute paths
//! are kept.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::cli::OpenRequest;

/// The most a request may weigh; more is refused.
const MAX_REQUEST: u64 = 1 << 20;
const TIMEOUT: Duration = Duration::from_secs(3);

/// Where the running instance listens. On Unix a socket path must fit in about 100 bytes, so
/// the user's private runtime folder is preferred, named after the data folder (each
/// `CRABCRAFT_HOME` has its own instance).
pub fn endpoint() -> PathBuf {
    let data = rc_core::paths::data_dir();
    if cfg!(windows) {
        return data.join("instance.json");
    }
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).filter(|d| d.is_absolute() && d.is_dir());
    socket_path(&data, runtime.as_deref())
}

fn socket_path(data: &Path, runtime: Option<&Path>) -> PathBuf {
    let in_data = data.join("instance.sock");
    match runtime {
        Some(dir) => {
            // FNV-1a: the same name from every CrabCraft version.
            let hash = data.as_os_str().as_encoded_bytes().iter().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3));
            dir.join(format!("crabcraft-{hash:016x}.sock"))
        }
        None => in_data,
    }
}

/// Hands `requests` to the instance listening at `endpoint`. `Ok(false)` when there is none.
pub fn forward(endpoint: &Path, requests: &[OpenRequest]) -> std::io::Result<bool> {
    let Some(mut stream) = imp::connect(endpoint)? else { return Ok(false) };
    stream.set_timeout(TIMEOUT)?;
    let mut body = serde_json::to_vec(requests).map_err(std::io::Error::other)?;
    body.push(b'\n');
    stream.write_all(&body)?;
    stream.flush()?;
    let mut reply = String::new();
    BufReader::new(&mut stream).take(16).read_line(&mut reply)?;
    Ok(reply.trim() == "ok")
}

/// Keeps listening at `endpoint` on a thread, calling `deliver` with each batch of requests.
/// `None` when the endpoint could not be opened (another instance has it, or the folder is
/// read-only): the app then simply works alone.
pub fn listen(endpoint: &Path, deliver: impl Fn(Vec<OpenRequest>) + Send + 'static) -> Option<Listener> {
    if let Some(dir) = endpoint.parent() {
        std::fs::create_dir_all(dir).ok()?;
    }
    let listener = imp::bind(endpoint).ok()?;
    let path = endpoint.to_path_buf();
    std::thread::Builder::new()
        .name("instance".into())
        .spawn(move || listener.serve(&deliver))
        .ok()?;
    Some(Listener { path })
}

/// Removes the endpoint when the app quits.
pub struct Listener {
    path: PathBuf,
}

impl Drop for Listener {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Reads one request; `None` when it is not a valid one.
fn read_request(stream: &mut impl Read) -> Option<Vec<OpenRequest>> {
    let mut line = Vec::new();
    BufReader::new(stream.take(MAX_REQUEST)).read_until(b'\n', &mut line).ok()?;
    if !line.ends_with(b"\n") {
        return None;
    }
    let list: Vec<OpenRequest> = serde_json::from_slice(&line).ok()?;
    Some(list.into_iter().filter(|r| r.path.is_absolute()).collect())
}

trait Stream: Read + Write {
    fn set_timeout(&self, timeout: Duration) -> std::io::Result<()>;
}

#[cfg(unix)]
mod imp {
    use super::*;
    use std::os::unix::net::{UnixListener, UnixStream};

    impl Stream for UnixStream {
        fn set_timeout(&self, timeout: Duration) -> std::io::Result<()> {
            self.set_read_timeout(Some(timeout))?;
            self.set_write_timeout(Some(timeout))
        }
    }

    pub fn connect(endpoint: &Path) -> std::io::Result<Option<UnixStream>> {
        match UnixStream::connect(endpoint) {
            Ok(s) => Ok(Some(s)),
            Err(e) if matches!(e.kind(), std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused) => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub struct Bound(UnixListener);

    pub fn bind(endpoint: &Path) -> std::io::Result<Bound> {
        if connect(endpoint)?.is_some() {
            return Err(std::io::ErrorKind::AddrInUse.into());
        }
        // Left behind by an instance that crashed.
        let _ = std::fs::remove_file(endpoint);
        Ok(Bound(UnixListener::bind(endpoint)?))
    }

    impl Bound {
        pub fn serve(self, deliver: &dyn Fn(Vec<OpenRequest>)) {
            for mut stream in self.0.incoming().flatten() {
                if stream.set_timeout(TIMEOUT).is_err() {
                    continue;
                }
                if let Some(list) = read_request(&mut stream) {
                    let _ = stream.write_all(b"ok\n");
                    deliver(list);
                }
            }
        }
    }
}

#[cfg(windows)]
mod imp {
    use super::*;
    use std::net::{Ipv4Addr, TcpListener, TcpStream};

    impl Stream for TcpStream {
        fn set_timeout(&self, timeout: Duration) -> std::io::Result<()> {
            self.set_read_timeout(Some(timeout))?;
            self.set_write_timeout(Some(timeout))
        }
    }

    #[derive(serde::Serialize, serde::Deserialize)]
    struct Address {
        port: u16,
        token: String,
    }

    pub fn connect(endpoint: &Path) -> std::io::Result<Option<TcpStream>> {
        let Ok(text) = std::fs::read_to_string(endpoint) else { return Ok(None) };
        let Ok(address) = serde_json::from_str::<Address>(&text) else { return Ok(None) };
        match TcpStream::connect_timeout(&(Ipv4Addr::LOCALHOST, address.port).into(), TIMEOUT) {
            Ok(mut s) => {
                s.write_all(format!("{}\n", address.token).as_bytes())?;
                Ok(Some(s))
            }
            Err(_) => Ok(None),
        }
    }

    pub struct Bound(TcpListener, String);

    pub fn bind(endpoint: &Path) -> std::io::Result<Bound> {
        if connect(endpoint)?.is_some() {
            return Err(std::io::ErrorKind::AddrInUse.into());
        }
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        let token = token();
        let address = Address { port: listener.local_addr()?.port(), token: token.clone() };
        std::fs::write(endpoint, serde_json::to_vec(&address).map_err(std::io::Error::other)?)?;
        Ok(Bound(listener, token))
    }

    /// Unpredictable enough for a local handshake: the hasher's random keys, twice.
    fn token() -> String {
        use std::hash::{BuildHasher, RandomState};
        let a = RandomState::new().hash_one(std::process::id());
        let b = RandomState::new().hash_one(std::time::SystemTime::now());
        format!("{a:016x}{b:016x}")
    }

    impl Bound {
        pub fn serve(self, deliver: &dyn Fn(Vec<OpenRequest>)) {
            for mut stream in self.0.incoming().flatten() {
                if stream.set_timeout(TIMEOUT).is_err() {
                    continue;
                }
                let mut token = Vec::new();
                if BufReader::new((&mut stream).take(128)).read_until(b'\n', &mut token).is_err() || token.trim_ascii() != self.1.as_bytes() {
                    continue;
                }
                if let Some(list) = read_request(&mut stream) {
                    let _ = stream.write_all(b"ok\n");
                    deliver(list);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn a_second_call_hands_its_files_to_the_first() {
        let dir = tempfile::tempdir().unwrap();
        let endpoint = dir.path().join("instance.sock");
        let file = dir.path().join("main.rs");
        assert!(!forward(&endpoint, &[OpenRequest::path(&file)]).unwrap(), "nobody listens yet");

        let (tx, rx) = mpsc::channel();
        let listener = listen(&endpoint, move |list| tx.send(list).unwrap()).expect("listens");
        assert!(listen(&endpoint, |_| {}).is_none(), "only one instance listens");

        let sent = vec![OpenRequest { path: file.clone(), line: Some(3), column: Some(2) }, OpenRequest::path("relative.rs")];
        assert!(forward(&endpoint, &sent).unwrap());
        let got = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(got, vec![sent[0].clone()], "relative paths are dropped");

        drop(listener);
        assert!(!endpoint.exists());
        assert!(!forward(&endpoint, &sent).unwrap());
    }

    #[test]
    fn garbage_and_oversized_requests_are_ignored() {
        assert_eq!(read_request(&mut &b"not json\n"[..]), None);
        assert_eq!(read_request(&mut &b"[{\"path\":\"/a\",\"line\":1,\"column\":null}]"[..]), None, "no end of line");
        let huge = vec![b' '; MAX_REQUEST as usize + 10];
        assert_eq!(read_request(&mut &huge[..]), None);
        #[cfg(unix)]
        assert_eq!(read_request(&mut &b"[{\"path\":\"/a\",\"line\":1,\"column\":null}]\n"[..]), Some(vec![OpenRequest { path: "/a".into(), line: Some(1), column: None }]));
    }

    #[test]
    fn each_data_folder_has_its_own_socket() {
        let a = socket_path(Path::new("/home/u/.local/share/CrabCraft"), Some(Path::new("/run/user/1000")));
        let b = socket_path(Path::new("/tmp/other"), Some(Path::new("/run/user/1000")));
        assert!(a.starts_with("/run/user/1000") && b.starts_with("/run/user/1000"));
        assert_ne!(a, b);
        assert_eq!(a, socket_path(Path::new("/home/u/.local/share/CrabCraft"), Some(Path::new("/run/user/1000"))), "stable");
        assert_eq!(socket_path(Path::new("/d"), None), Path::new("/d/instance.sock"));
    }

    #[cfg(unix)]
    #[test]
    fn a_stale_socket_left_by_a_crash_is_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let endpoint = dir.path().join("instance.sock");
        drop(std::os::unix::net::UnixListener::bind(&endpoint).unwrap());
        assert!(endpoint.exists());
        assert!(!forward(&endpoint, &[]).unwrap());
        let _listener = listen(&endpoint, |_| {}).expect("takes the endpoint over");
        assert!(forward(&endpoint, &[]).unwrap());
    }
}
