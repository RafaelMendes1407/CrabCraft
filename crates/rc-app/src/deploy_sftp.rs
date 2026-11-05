//! Bounded SFTP v3 over verified OpenSSH, including protocol no-overwrite rename for new files.
use crate::{
    deployment::{Error, MAX, RemoteImage},
    remote_transfer,
};
use rc_dotnet::process::CancelToken;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
#[derive(Clone, Debug, PartialEq, Eq)]
struct Attrs {
    size: Option<u64>,
    owner: Option<(u32, u32)>,
    mode: Option<u32>,
    modified: Option<u32>,
}
struct Cursor<'a> {
    bytes: &'a [u8],
    position: usize,
}
impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8], Error> {
        let end = self.position.checked_add(n).ok_or(Error::Protocol)?;
        let bytes = self.bytes.get(self.position..end).ok_or(Error::Protocol)?;
        self.position = end;
        Ok(bytes)
    }
    fn u32(&mut self) -> Result<u32, Error> {
        let mut bytes = [0; 4];
        bytes.copy_from_slice(self.take(4)?);
        Ok(u32::from_be_bytes(bytes))
    }
    fn u64(&mut self) -> Result<u64, Error> {
        let mut bytes = [0; 8];
        bytes.copy_from_slice(self.take(8)?);
        Ok(u64::from_be_bytes(bytes))
    }
    fn string(&mut self, max: usize) -> Result<&'a [u8], Error> {
        let len = self.u32()? as usize;
        if len > max {
            return Err(Error::Limit);
        }
        self.take(len)
    }
    fn done(&self) -> bool {
        self.position == self.bytes.len()
    }
}
fn string(out: &mut Vec<u8>, value: &[u8]) -> Result<(), Error> {
    let len = u32::try_from(value.len()).map_err(|_| Error::Limit)?;
    out.extend(len.to_be_bytes());
    out.extend(value);
    Ok(())
}
fn attrs(reader: &mut Cursor<'_>) -> Result<Attrs, Error> {
    let flags = reader.u32()?;
    if flags & !0x8000000f != 0 {
        return Err(Error::Protocol);
    }
    let size = if flags & 1 != 0 {
        Some(reader.u64()?)
    } else {
        None
    };
    let owner = if flags & 2 != 0 {
        Some((reader.u32()?, reader.u32()?))
    } else {
        None
    };
    let mode = if flags & 4 != 0 {
        Some(reader.u32()?)
    } else {
        None
    };
    let modified = if flags & 8 != 0 {
        let _accessed = reader.u32()?; // Our own reads may legitimately update access time.
        Some(reader.u32()?)
    } else {
        None
    };
    if flags & 0x80000000 != 0 {
        let count = reader.u32()?;
        if count > 16 {
            return Err(Error::Limit);
        }
        for _ in 0..count {
            reader.string(4096)?;
            reader.string(65536)?;
        }
    }
    Ok(Attrs {
        size,
        owner,
        mode,
        modified,
    })
}
pub struct Client {
    child: tokio::process::Child,
    input: tokio::process::ChildStdin,
    output: tokio::process::ChildStdout,
    id: u32,
    pid: u32,
    cancel: CancelToken,
    deadline: Instant,
    posix_rename: bool,
    fsync: bool,
    _credentials: tempfile::TempDir,
    stderr: tokio::task::JoinHandle<()>,
    overflow: std::sync::Arc<std::sync::atomic::AtomicBool>,
}
impl Drop for Client {
    fn drop(&mut self) {
        self.stderr.abort();
        rc_dotnet::process::kill_tree(self.pid);
        let _ = self.child.start_kill();
    }
}
impl Client {
    pub async fn connect(
        config: &remote_transfer::Config,
        cancel: CancelToken,
    ) -> Result<Self, Error> {
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let credentials = tempfile::tempdir()?;
        let mut args = remote_transfer::args(config, credentials.path()).map_err(Error::from)?;
        args.extend([
            "-p".into(),
            config.port.to_string(),
            "-T".into(),
            "-s".into(),
            format!("{}@{}", config.user, config.host),
            "sftp".into(),
        ]);
        let mut command = tokio::process::Command::new("ssh");
        command
            .args(args)
            .current_dir(credentials.path())
            .env_remove("SSH_ASKPASS")
            .env("SSH_ASKPASS_REQUIRE", "never")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        #[cfg(unix)]
        command.process_group(0);
        #[cfg(windows)]
        command.creation_flags(0x0800_0000);
        let mut child = command.spawn()?;
        let pid = child.id().ok_or(Error::Protocol)?;
        let input = child.stdin.take().ok_or(Error::Protocol)?;
        let output = child.stdout.take().ok_or(Error::Protocol)?;
        let mut errors = child.stderr.take().ok_or(Error::Protocol)?;
        let overflow = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = overflow.clone();
        let stderr = tokio::spawn(async move {
            let mut total = 0usize;
            let mut bytes = [0; 4096];
            loop {
                match errors.read(&mut bytes).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        total = total.saturating_add(n);
                        if total > 64 * 1024 {
                            flag.store(true, std::sync::atomic::Ordering::Release);
                            rc_dotnet::process::kill_tree(pid);
                            break;
                        }
                    }
                }
            }
        });
        let mut client = Self {
            child,
            input,
            output,
            id: 0,
            pid,
            cancel,
            deadline: Instant::now() + Duration::from_secs(120),
            posix_rename: false,
            fsync: false,
            _credentials: credentials,
            stderr,
            overflow,
        };
        client.write(&[1, 0, 0, 0, 3]).await?;
        let reply = client.read_packet().await?;
        if reply.first() != Some(&2) {
            return Err(Error::Protocol);
        }
        let mut reader = Cursor::new(reply.get(1..).ok_or(Error::Protocol)?);
        if reader.u32()? != 3 {
            return Err(Error::Unsupported);
        }
        let mut count = 0;
        while !reader.done() {
            count += 1;
            if count > 128 {
                return Err(Error::Limit);
            }
            let name = reader.string(4096)?;
            let value = reader.string(4096)?;
            if name == b"posix-rename@openssh.com" && value == b"1" {
                client.posix_rename = true;
            }
            if name == b"fsync@openssh.com" && value == b"1" {
                client.fsync = true;
            }
        }
        Ok(client)
    }
    fn check(&self) -> Result<(), Error> {
        if self.cancel.is_cancelled() {
            Err(Error::Cancelled)
        } else if self.overflow.load(std::sync::atomic::Ordering::Acquire) {
            Err(Error::Limit)
        } else if Instant::now() > self.deadline {
            Err(Error::Timeout)
        } else {
            Ok(())
        }
    }
    async fn write(&mut self, body: &[u8]) -> Result<(), Error> {
        self.check()?;
        if body.len() > 128 * 1024 {
            return Err(Error::Limit);
        }
        let len = u32::try_from(body.len())
            .map_err(|_| Error::Limit)?
            .to_be_bytes();
        let token = self.cancel.clone();
        let timeout = self.deadline.saturating_duration_since(Instant::now());
        tokio::select! {result=async{self.input.write_all(&len).await?;self.input.write_all(body).await?;self.input.flush().await}=>result.map_err(Error::from),()=token.cancelled()=>Err(Error::Cancelled),()=tokio::time::sleep(timeout)=>Err(Error::Timeout)}
    }
    async fn read_packet(&mut self) -> Result<Vec<u8>, Error> {
        self.check()?;
        let token = self.cancel.clone();
        let timeout = self.deadline.saturating_duration_since(Instant::now());
        tokio::select! {result=async{let len=self.output.read_u32().await?as usize;if len==0||len>128*1024{return Err(Error::Limit);}let mut bytes=vec![0;len];self.output.read_exact(&mut bytes).await?;Ok(bytes)}=>result,()=token.cancelled()=>Err(Error::Cancelled),()=tokio::time::sleep(timeout)=>Err(Error::Timeout)}
    }
    async fn request(&mut self, kind: u8, body: Vec<u8>) -> Result<(u8, Vec<u8>), Error> {
        self.check()?;
        self.id = self.id.checked_add(1).ok_or(Error::Limit)?;
        let mut packet = vec![kind];
        packet.extend(self.id.to_be_bytes());
        packet.extend(body);
        self.write(&packet).await?;
        let reply = self.read_packet().await?;
        let kind = *reply.first().ok_or(Error::Protocol)?;
        let mut reader = Cursor::new(reply.get(1..).ok_or(Error::Protocol)?);
        if reader.u32()? != self.id {
            return Err(Error::Protocol);
        }
        Ok((
            kind,
            reader
                .bytes
                .get(reader.position..)
                .ok_or(Error::Protocol)?
                .to_vec(),
        ))
    }
    fn status(kind: u8, body: &[u8]) -> Result<u32, Error> {
        if kind != 101 {
            return Err(Error::Protocol);
        }
        let mut reader = Cursor::new(body);
        let code = reader.u32()?;
        reader.string(65536)?;
        reader.string(4096)?;
        if !reader.done() {
            return Err(Error::Protocol);
        }
        Ok(code)
    }
    async fn ok(&mut self, kind: u8, body: Vec<u8>) -> Result<(), Error> {
        let (kind, body) = self.request(kind, body).await?;
        let code = Self::status(kind, &body)?;
        if code == 0 {
            Ok(())
        } else {
            Err(Error::Remote(code))
        }
    }
    async fn stat(&mut self, path: &str) -> Result<Option<Attrs>, Error> {
        let mut data = Vec::new();
        string(&mut data, path.as_bytes())?;
        let (kind, body) = self.request(7, data).await?;
        if kind == 101 {
            return match Self::status(kind, &body)? {
                2 => Ok(None),
                code => Err(Error::Remote(code)),
            };
        }
        if kind != 105 {
            return Err(Error::Protocol);
        }
        let mut reader = Cursor::new(&body);
        let attrs = attrs(&mut reader)?;
        if !reader.done() {
            return Err(Error::Protocol);
        }
        if attrs.mode.is_none_or(|mode| mode & 0o170000 != 0o100000) {
            return Err(Error::Unsupported);
        }
        if attrs.size.is_none_or(|n| n > MAX as u64) {
            return Err(Error::Limit);
        }
        Ok(Some(attrs))
    }
    async fn open(&mut self, path: &str, flags: u32, mode: Option<u32>) -> Result<Vec<u8>, Error> {
        let mut data = Vec::new();
        string(&mut data, path.as_bytes())?;
        data.extend(flags.to_be_bytes());
        if let Some(mode) = mode {
            data.extend(4u32.to_be_bytes());
            data.extend((mode & 0o777).to_be_bytes());
        } else {
            data.extend(0u32.to_be_bytes());
        }
        let (kind, body) = self.request(3, data).await?;
        if kind == 101 {
            return Err(Error::Remote(Self::status(kind, &body)?));
        }
        if kind != 102 {
            return Err(Error::Protocol);
        }
        let mut reader = Cursor::new(&body);
        let handle = reader.string(4096)?.to_vec();
        if !reader.done() {
            return Err(Error::Protocol);
        }
        Ok(handle)
    }
    async fn close(&mut self, handle: &[u8]) -> Result<(), Error> {
        let mut data = Vec::new();
        string(&mut data, handle)?;
        self.ok(4, data).await
    }
    pub async fn image(&mut self, path: &str) -> Result<RemoteImage, Error> {
        let Some(before) = self.stat(path).await? else {
            return Ok(RemoteImage::Missing);
        };
        let size =
            usize::try_from(before.size.ok_or(Error::Protocol)?).map_err(|_| Error::Limit)?;
        let handle = self.open(path, 1, None).await?;
        let mut bytes = Vec::with_capacity(size);
        while bytes.len() < size {
            let mut data = Vec::new();
            string(&mut data, &handle)?;
            data.extend((bytes.len() as u64).to_be_bytes());
            data.extend(32768u32.to_be_bytes());
            let (kind, body) = self.request(5, data).await?;
            if kind == 101 {
                return Err(Error::Remote(Self::status(kind, &body)?));
            }
            if kind != 103 {
                return Err(Error::Protocol);
            }
            let mut reader = Cursor::new(&body);
            let chunk = reader.string(32768)?;
            if chunk.is_empty() || bytes.len().saturating_add(chunk.len()) > size || !reader.done()
            {
                return Err(Error::Stale);
            }
            bytes.extend(chunk);
        }
        self.close(&handle).await?;
        if self.stat(path).await? != Some(before.clone()) {
            return Err(Error::Stale);
        }
        Ok(RemoteImage::Bytes {
            hash: crate::deployment::digest(&bytes),
            bytes: std::sync::Arc::from(bytes),
            stamp: format!("{:?}", before),
            mode: before.mode,
        })
    }
    pub async fn upload(&mut self, path: &str, bytes: &[u8], mode: u32) -> Result<(), Error> {
        if bytes.len() > MAX {
            return Err(Error::Limit);
        }
        let handle = self.open(path, 2 | 8 | 32, Some(mode)).await?;
        let mut offset = 0usize;
        for chunk in bytes.chunks(32768) {
            let mut data = Vec::new();
            string(&mut data, &handle)?;
            data.extend((offset as u64).to_be_bytes());
            string(&mut data, chunk)?;
            self.ok(6, data).await?;
            offset = offset.saturating_add(chunk.len());
        }
        if self.fsync {
            let mut data = Vec::new();
            string(&mut data, b"fsync@openssh.com")?;
            string(&mut data, &handle)?;
            self.ok(200, data).await?;
        }
        self.close(&handle).await
    }
    pub async fn publish(&mut self, temp: &str, path: &str, replace: bool) -> Result<(), Error> {
        let mut data = Vec::new();
        let kind = if replace {
            if !self.posix_rename {
                return Err(Error::Unsupported);
            }
            string(&mut data, b"posix-rename@openssh.com")?;
            200
        } else {
            18
        };
        string(&mut data, temp.as_bytes())?;
        string(&mut data, path.as_bytes())?;
        self.ok(kind, data).await
    }
    pub async fn remove_temporary(&mut self, path: &str) -> Result<(), Error> {
        let mut data = Vec::new();
        string(&mut data, path.as_bytes())?;
        self.ok(13, data).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn packet_fields_reject_truncation_lengths_and_invalid_attribute_flags() {
        let mut c = Cursor::new(&[0, 0, 0]);
        assert!(c.u32().is_err());
        let mut c = Cursor::new(&[0xff, 0xff, 0xff, 0xff]);
        assert!(matches!(c.string(4096), Err(Error::Limit)));
        let mut c = Cursor::new(&[0, 0, 0, 16]);
        assert!(attrs(&mut c).is_err());
        for length in 0..128 {
            let bytes = vec![0xff; length];
            let mut c = Cursor::new(&bytes);
            let _ = c.string(4096);
        }
    }
    #[test]
    #[ignore = "Binds a private loopback listener and starts OpenSSH to verify active handshake cancellation"]
    fn real_sftp_cancel_stalled_handshake() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let mut config = crate::remote_transfer::tests::config();
        config.port = listener.local_addr().unwrap().port();
        let cancel = CancelToken::new();
        let token = cancel.clone();
        let handle =
            std::thread::spawn(move || rc_core::Runtime::block_on(Client::connect(&config, token)));
        let (socket, _) = listener.accept().unwrap();
        let start = Instant::now();
        cancel.cancel();
        assert!(matches!(handle.join().unwrap(), Err(Error::Cancelled)));
        assert!(start.elapsed() < Duration::from_secs(3));
        drop(socket);
    }
}
