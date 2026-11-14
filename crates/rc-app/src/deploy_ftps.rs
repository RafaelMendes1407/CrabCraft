//! Native explicit FTPS: mandatory authenticated TLS for control/data and real STOU creation.
use crate::deployment::{self, Error, MAX, Preview, Receipt, RemoteImage};
use rc_dotnet::process::CancelToken;
use std::{
    future::Future,
    net::IpAddr,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio_rustls::{
    TlsConnector,
    client::TlsStream,
    rustls::{
        self,
        pki_types::{CertificateDer, ServerName, pem::PemObject},
    },
};
#[derive(Clone, PartialEq, Eq)]
pub struct Config {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: String,
    pub ca: Option<PathBuf>,
    pub pin: String,
    #[cfg(test)]
    pub rate: Option<u32>,
}
impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FtpsConfig").field("host", &self.host).field("port", &self.port).field("credentials", &"<redacted>").finish()
    }
}
impl Config {
    pub fn new(host: String, port: u16, user: String, password: String, ca: Option<PathBuf>, pin: String) -> Self {
        Self {
            host,
            port,
            user,
            password,
            ca,
            pin,
            #[cfg(test)]
            rate: None,
        }
    }
    fn validate(&self) -> Result<(), Error> {
        if self.host.is_empty()
            || self.host.len() > 253
            || self.host.starts_with('-')
            || !self.host.bytes().all(|b| b.is_ascii_alphanumeric() || b".-_".contains(&b))
            || self.port == 0
            || self.user.is_empty()
            || self.user.len() > 512
            || self.user.chars().any(char::is_control)
            || self.password.is_empty()
            || self.password.len() > 4096
            || self.password.chars().any(char::is_control)
        {
            return Err(Error::Input);
        }
        if !self.pin.is_empty() {
            let pin = self.pin.strip_prefix("sha256//").ok_or(Error::Input)?;
            if pin.len() != 44
                || !pin.ends_with('=')
                || !pin.as_bytes().get(..43).ok_or(Error::Input)?.iter().all(|b| b.is_ascii_alphanumeric() || b"+/".contains(b))
            {
                return Err(Error::Input);
            }
        }
        Ok(())
    }
}
struct Reply {
    code: u16,
    text: String,
}
async fn wait<T>(cancel: &CancelToken, deadline: Instant, future: impl Future<Output = Result<T, Error>>) -> Result<T, Error> {
    if cancel.is_cancelled() {
        return Err(Error::Cancelled);
    }
    if Instant::now() >= deadline {
        return Err(Error::Timeout);
    }
    tokio::select! {result=future=>result,()=cancel.cancelled()=>Err(Error::Cancelled),()=tokio::time::sleep(deadline.saturating_duration_since(Instant::now()))=>Err(Error::Timeout)}
}
async fn reply(stream: &mut (impl AsyncBufRead + Unpin)) -> Result<Reply, Error> {
    let mut text = String::new();
    let mut expected = None;
    loop {
        let mut bytes = Vec::new();
        stream.take(8193).read_until(b'\n', &mut bytes).await?;
        if bytes.is_empty() || bytes.len() > 8192 || text.len().saturating_add(bytes.len()) > 65536 {
            return Err(Error::Protocol);
        }
        let line = std::str::from_utf8(&bytes).map_err(|_| Error::Protocol)?;
        let prefix = line.get(..3).and_then(|s| s.parse::<u16>().ok()).filter(|n| *n >= 100 && *n <= 599);
        if expected.is_none() {
            let code = prefix.ok_or(Error::Protocol)?;
            if line.as_bytes().get(3) == Some(&b'-') {
                expected = Some(code);
            } else if line.as_bytes().get(3) == Some(&b' ') {
                text.push_str(line);
                return Ok(Reply { code, text });
            } else {
                return Err(Error::Protocol);
            }
        } else if prefix == expected && line.as_bytes().get(3) == Some(&b' ') {
            text.push_str(line);
            return Ok(Reply { code: expected.ok_or(Error::Protocol)?, text });
        }
        text.push_str(line);
    }
}
// Only extracts the already PKI-verified certificate's SPKI DER, bounded and without permissive ASN.1.
fn der<'a>(bytes: &'a [u8], position: &mut usize) -> Result<(u8, &'a [u8], &'a [u8]), Error> {
    let start = *position;
    let tag = *bytes.get(*position).ok_or(Error::Tls)?;
    *position = position.checked_add(1).ok_or(Error::Tls)?;
    let first = *bytes.get(*position).ok_or(Error::Tls)?;
    *position = position.checked_add(1).ok_or(Error::Tls)?;
    let len = if first & 128 == 0 {
        first as usize
    } else {
        let count = (first & 127) as usize;
        if count == 0 || count > 4 {
            return Err(Error::Tls);
        }
        let mut len = 0usize;
        for _ in 0..count {
            let byte = *bytes.get(*position).ok_or(Error::Tls)?;
            *position = position.checked_add(1).ok_or(Error::Tls)?;
            len = len.checked_mul(256).and_then(|n| n.checked_add(byte as usize)).ok_or(Error::Tls)?;
        }
        len
    };
    let end = position.checked_add(len).ok_or(Error::Tls)?;
    let body = bytes.get(*position..end).ok_or(Error::Tls)?;
    *position = end;
    Ok((tag, body, bytes.get(start..end).ok_or(Error::Tls)?))
}
fn spki(cert: &[u8]) -> Result<&[u8], Error> {
    if cert.len() > 128 * 1024 {
        return Err(Error::Limit);
    }
    let mut position = 0;
    let (tag, body, _) = der(cert, &mut position)?;
    if tag != 0x30 || position != cert.len() {
        return Err(Error::Tls);
    }
    let mut position = 0;
    let (tag, tbs, _) = der(body, &mut position)?;
    if tag != 0x30 {
        return Err(Error::Tls);
    }
    let mut position = 0;
    if tbs.first() == Some(&0xa0) {
        der(tbs, &mut position)?;
    }
    for tag in [0x02, 0x30, 0x30, 0x30, 0x30] {
        if der(tbs, &mut position)?.0 != tag {
            return Err(Error::Tls);
        }
    }
    let (tag, _, full) = der(tbs, &mut position)?;
    if tag != 0x30 {
        return Err(Error::Tls);
    }
    Ok(full)
}
fn base64(bytes: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut text = String::new();
    for chunk in bytes.chunks(3) {
        let a = chunk.first().copied().unwrap_or(0) as u32;
        let b = chunk.get(1).copied().unwrap_or(0) as u32;
        let c = chunk.get(2).copied().unwrap_or(0) as u32;
        let word = (a << 16) | (b << 8) | c;
        for shift in [18, 12, 6, 0] {
            let index = ((word >> shift) & 63) as usize;
            text.push(ABC[index] as char);
        }
        if chunk.len() < 3 {
            text.pop();
            text.push('=');
        }
        if chunk.len() < 2 {
            let position = text.len().saturating_sub(2);
            text.replace_range(position..position + 1, "=");
        }
    }
    text
}
fn pinned(config: &Config, stream: &TlsStream<tokio::net::TcpStream>) -> Result<(), Error> {
    if config.pin.is_empty() {
        return Ok(());
    }
    use sha2::Digest;
    let cert = stream.get_ref().1.peer_certificates().and_then(|certs| certs.first()).ok_or(Error::Tls)?;
    let digest = sha2::Sha256::digest(spki(cert.as_ref())?);
    let actual = format!("sha256//{}", base64(&digest));
    if actual == config.pin { Ok(()) } else { Err(Error::Tls) }
}
#[derive(Clone)]
struct Entry {
    name: String,
    kind: String,
    size: Option<u64>,
    stamp: String,
    mode: Option<u32>,
}
fn listing(bytes: &[u8]) -> Result<Vec<Entry>, Error> {
    if bytes.len() > 2 * 1024 * 1024 {
        return Err(Error::Limit);
    }
    let text = std::str::from_utf8(bytes).map_err(|_| Error::Protocol)?;
    let mut entries = Vec::new();
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            continue;
        }
        if line.len() > 8192 || entries.len() >= 4096 {
            return Err(Error::Limit);
        }
        let (facts, name) = line.split_once(' ').ok_or(Error::Protocol)?;
        if name.is_empty() || name.len() > 4096 || name.contains('/') || name.chars().any(char::is_control) {
            return Err(Error::Protocol);
        }
        let mut values = std::collections::BTreeMap::new();
        for fact in facts.split(';').filter(|p| !p.is_empty()) {
            let (key, value) = fact.split_once('=').ok_or(Error::Protocol)?;
            if values.insert(key.to_ascii_lowercase(), value.to_owned()).is_some() {
                return Err(Error::Protocol);
            }
        }
        let kind = values.get("type").ok_or(Error::Protocol)?.to_ascii_lowercase();
        if kind == "cdir" || kind == "pdir" {
            continue;
        }
        let size = values.get("size").map(|s| s.parse::<u64>().map_err(|_| Error::Protocol)).transpose()?;
        let mode = values.get("unix.mode").map(|s| u32::from_str_radix(s, 8).map_err(|_| Error::Protocol)).transpose()?;
        let stamp = format!("{:?}", values);
        entries.push(Entry { name: name.to_owned(), kind, size, stamp, mode });
    }
    Ok(entries)
}
struct Client {
    config: Config,
    control: BufReader<TlsStream<tokio::net::TcpStream>>,
    connector: TlsConnector,
    peer: IpAddr,
    cancel: CancelToken,
    deadline: Instant,
}
impl Client {
    async fn connect(config: &Config, cancel: CancelToken) -> Result<Self, Error> {
        config.validate()?;
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let mut roots = rustls::RootCertStore::empty();
        if let Some(path) = &config.ca {
            let source = deployment::source_limited(path, &cancel, 256 * 1024)?;
            if source.bytes.len() > 256 * 1024 {
                return Err(Error::Limit);
            }
            let mut count = 0;
            for cert in CertificateDer::pem_slice_iter(&source.bytes) {
                count += 1;
                if count > 64 {
                    return Err(Error::Limit);
                }
                roots.add(cert.map_err(|_| Error::Tls)?).map_err(|_| Error::Tls)?;
            }
            if count == 0 {
                return Err(Error::Tls);
            }
        } else {
            let certs = rustls_native_certs::load_native_certs();
            for cert in certs.certs {
                let _ = roots.add(cert);
            }
        }
        let tls = rustls::ClientConfig::builder_with_provider(rustls::crypto::aws_lc_rs::default_provider().into())
            .with_protocol_versions(&[&rustls::version::TLS13, &rustls::version::TLS12])
            .map_err(|_| Error::Tls)?
            .with_root_certificates(roots)
            .with_no_client_auth();
        let connector = TlsConnector::from(Arc::new(tls));
        let deadline = Instant::now() + Duration::from_secs(120);
        let socket = wait(&cancel, Instant::now() + Duration::from_secs(5), async {
            tokio::net::TcpStream::connect((config.host.as_str(), config.port)).await.map_err(Error::from)
        })
        .await?;
        let peer = socket.peer_addr()?.ip();
        let mut plain = BufReader::new(socket);
        let greeting = wait(&cancel, deadline, reply(&mut plain)).await?;
        if greeting.code != 220 {
            return Err(Error::Remote(greeting.code as u32));
        }
        wait(&cancel, deadline, async {
            plain.get_mut().write_all(b"AUTH TLS\r\n").await?;
            plain.get_mut().flush().await?;
            Ok(())
        })
        .await?;
        if wait(&cancel, deadline, reply(&mut plain)).await?.code != 234 {
            return Err(Error::Tls);
        }
        if !plain.buffer().is_empty() {
            return Err(Error::Protocol);
        }
        let name = ServerName::try_from(config.host.clone()).map_err(|_| Error::Input)?;
        let tls = wait(&cancel, deadline, async { connector.connect(name, plain.into_inner()).await.map_err(|_| Error::Tls) }).await?;
        pinned(config, &tls)?;
        let mut client = Self { config: config.clone(), control: BufReader::new(tls), connector, peer, cancel, deadline };
        let user = client.command(&format!("USER {}", config.user)).await?;
        if user.code == 331 {
            if client.command(&format!("PASS {}", config.password)).await?.code != 230 {
                return Err(Error::Authentication);
            }
        } else if user.code != 230 {
            return Err(Error::Authentication);
        }
        for command in ["PBSZ 0", "PROT P"] {
            if client.command(command).await?.code != 200 {
                return Err(Error::Tls);
            }
        }
        if client.command("TYPE I").await?.code != 200 {
            return Err(Error::Protocol);
        }
        Ok(client)
    }
    async fn command(&mut self, command: &str) -> Result<Reply, Error> {
        if command.len() > 8192 || command.chars().any(char::is_control) {
            return Err(Error::Input);
        }
        let mut bytes = command.as_bytes().to_vec();
        bytes.extend(b"\r\n");
        wait(&self.cancel, self.deadline, async {
            self.control.get_mut().write_all(&bytes).await?;
            self.control.get_mut().flush().await?;
            reply(&mut self.control).await
        })
        .await
    }
    async fn data(&mut self, command: &str, path: &str, upload: Option<&[u8]>, limit: usize) -> Result<(Vec<u8>, String), Error> {
        if path != "/" {
            deployment::path(path.trim_end_matches('/'))?;
        }
        let passive = self.command("EPSV").await?;
        if passive.code != 229 {
            return Err(Error::Unsupported);
        }
        let body = passive.text.split_once('(').and_then(|(_, s)| s.split_once(')').map(|p| p.0)).ok_or(Error::Protocol)?;
        let delimiter = body.chars().next().filter(|c| c.is_ascii_punctuation()).ok_or(Error::Protocol)?;
        let fields: Vec<_> = body.split(delimiter).collect();
        if fields.len() != 5 || fields.first() != Some(&"") || fields.get(1) != Some(&"") || fields.get(2) != Some(&"") || fields.get(4) != Some(&"") {
            return Err(Error::Protocol);
        }
        let port = fields.get(3).ok_or(Error::Protocol)?.parse::<u16>().ok().filter(|p| *p > 0).ok_or(Error::Protocol)?;
        let socket = wait(&self.cancel, self.deadline, async { tokio::net::TcpStream::connect((self.peer, port)).await.map_err(Error::from) }).await?;
        let first = self.command(&format!("{command} {path}")).await?;
        if first.code != 125 && first.code != 150 {
            return Err(Error::Remote(first.code as u32));
        }
        let name = ServerName::try_from(self.config.host.clone()).map_err(|_| Error::Input)?;
        let mut stream = wait(&self.cancel, self.deadline, async { self.connector.connect(name, socket).await.map_err(|_| Error::Tls) }).await?;
        pinned(&self.config, &stream)?;
        let mut bytes = Vec::new();
        if let Some(upload) = upload {
            for chunk in upload.chunks(32768) {
                wait(&self.cancel, self.deadline, async { stream.write_all(chunk).await.map_err(Error::from) }).await?;
                #[cfg(test)]
                if let Some(rate) = self.config.rate {
                    wait(&self.cancel, self.deadline, async {
                        tokio::time::sleep(Duration::from_secs_f64(chunk.len() as f64 / f64::from(rate.max(1)))).await;
                        Ok(())
                    })
                    .await?;
                }
            }
            wait(&self.cancel, self.deadline, async {
                stream.flush().await?;
                stream.shutdown().await?;
                // Drain the peer's TLS close_notify before closing TCP. Dropping an unread
                // TLS peer alert can reset the socket while upload records are in flight.
                let mut byte = [0; 1];
                if stream.read(&mut byte).await? != 0 {
                    return Err(Error::Protocol);
                }
                Ok(())
            })
            .await?;
        } else {
            let mut chunk = [0; 32768];
            loop {
                let size = wait(&self.cancel, self.deadline, async { stream.read(&mut chunk).await.map_err(Error::from) }).await?;
                if size == 0 {
                    break;
                }
                if bytes.len().saturating_add(size) > limit {
                    return Err(Error::Limit);
                }
                bytes.extend_from_slice(&chunk[..size]);
            }
        }
        drop(stream);
        let final_reply = wait(&self.cancel, self.deadline, reply(&mut self.control)).await?;
        if final_reply.code != 226 && final_reply.code != 250 {
            return Err(Error::Remote(final_reply.code as u32));
        }
        Ok((bytes, first.text + &final_reply.text))
    }
    async fn list(&mut self, parent: &str) -> Result<Vec<Entry>, Error> {
        let path = if parent.is_empty() { "/" } else { parent };
        let (bytes, _) = self.data("MLSD", path, None, 2 * 1024 * 1024).await?;
        listing(&bytes)
    }
    async fn image(&mut self, path: &str) -> Result<RemoteImage, Error> {
        deployment::path(path)?;
        let (parent, name) = path.rsplit_once('/').ok_or(Error::Input)?;
        let entries = self.list(parent).await?;
        let matching: Vec<_> = entries.iter().filter(|e| e.name == name).collect();
        if matching.len() > 1 {
            return Err(Error::Protocol);
        }
        let Some(entry) = matching.first() else {
            if entries.iter().any(|e| e.name.to_lowercase() == name.to_lowercase()) {
                return Err(Error::Unsupported);
            }
            return Ok(RemoteImage::Missing);
        };
        if entry.kind != "file" {
            return Err(Error::Unsupported);
        }
        if entry.size.is_none_or(|n| n > MAX as u64) {
            return Err(Error::Limit);
        }
        let (bytes, _) = self.data("RETR", path, None, MAX).await?;
        if bytes.len() as u64 != entry.size.ok_or(Error::Protocol)? {
            #[cfg(test)]
            eprintln!("FTPS size changed: read={} listed={:?}", bytes.len(), entry.size);
            return Err(Error::Stale);
        }
        let after = self.list(parent).await?;
        if after.iter().find(|e| e.name == name).is_none_or(|e| e.stamp != entry.stamp) {
            #[cfg(test)]
            eprintln!("FTPS metadata changed during read");
            return Err(Error::Stale);
        }
        Ok(RemoteImage::Bytes { hash: deployment::digest(&bytes), bytes: Arc::from(bytes), stamp: entry.stamp.clone(), mode: entry.mode })
    }
    async fn unique(&mut self, path: &str, bytes: &[u8]) -> Result<String, Error> {
        let (_, response) = self.data("STOU", path, Some(bytes), MAX).await?;
        let (parent, _) = path.rsplit_once('/').ok_or(Error::Input)?;
        for line in response.lines() {
            if let Some((_, name)) = line.split_once("FILE: ") {
                let name = name.trim().trim_matches('"');
                let result = if name.starts_with('/') { name.into() } else { format!("{parent}/{name}") };
                deployment::path(&result)?;
                if result.rsplit_once('/').map(|p| p.0) != Some(parent) {
                    return Err(Error::Protocol);
                }
                return Ok(result);
            }
        }
        Err(Error::Unsupported)
    }
    async fn rename(&mut self, temp: &str, path: &str) -> Result<(), Error> {
        deployment::path(temp)?;
        deployment::path(path)?;
        if self.command(&format!("RNFR {temp}")).await?.code != 350 {
            return Err(Error::Protocol);
        }
        let result = self.command(&format!("RNTO {path}")).await?;
        if result.code != 250 {
            return Err(Error::Remote(result.code as u32));
        }
        Ok(())
    }
}
pub fn image(config: &Config, path: &str, cancel: &CancelToken) -> Result<RemoteImage, Error> {
    rc_core::Runtime::block_on(async { Client::connect(config, cancel.clone()).await?.image(path).await })
}
pub fn apply(config: &Config, preview: &Preview, cancel: &CancelToken) -> Result<Receipt, Error> {
    rc_core::Runtime::block_on(async {
        let mut client = Client::connect(config, cancel.clone()).await?;
        if !client.image(&preview.remote).await?.same(&preview.before) {
            return Err(Error::Stale);
        }
        let remote = if preview.create {
            deployment::local_current(preview, cancel)?;
            client.unique(&preview.remote, &preview.source.bytes).await?
        } else {
            let parent = preview.remote.rsplit_once('/').map(|p| p.0).ok_or(Error::Input)?;
            let temporary = client.unique(&format!("{parent}/.crabcraft-deploy"), &preview.source.bytes).await?;
            let image = client.image(&temporary).await?;
            if image.hash() != Some(preview.source.hash.as_str()) || image.size() != Some(preview.source.bytes.len()) {
                return Err(Error::Stale);
            }
            deployment::local_current(preview, cancel)?;
            if !client.image(&preview.remote).await?.same(&preview.before) {
                return Err(Error::Stale);
            }
            client.rename(&temporary, &preview.remote).await?;
            preview.remote.clone()
        };
        let image = client.image(&remote).await?;
        if image.hash() != Some(preview.source.hash.as_str()) || image.size() != Some(preview.source.bytes.len()) {
            #[cfg(test)]
            eprintln!(
                "FTPS verification mismatch: actual_size={:?} wanted={} actual_hash={:?} wanted_hash={}",
                image.size(),
                preview.source.bytes.len(),
                image.hash(),
                preview.source.hash
            );
            return Err(Error::Stale);
        }
        Ok(Receipt { remote, hash: preview.source.hash.clone(), size: preview.source.bytes.len() })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn response_and_der_parsers_bound_untrusted_fields_and_redact_authentication() {
        let config = Config::new("localhost".into(), 21, "private-user".into(), "private-password".into(), None, String::new());
        let debug = format!("{config:?}");
        assert!(!debug.contains("private-user") && !debug.contains("private-password"));
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        for bytes in [&b""[..], &b"\x30\x80"[..], &b"\x30\xff"[..], &b"\x30\x82\xff\xff"[..]] {
            assert!(spki(bytes).is_err());
        }
        rc_core::Runtime::block_on(async {
            let mut input = BufReader::new(&b"220-first\r\ntext\r\n220 ready\r\n"[..]);
            assert_eq!(reply(&mut input).await.unwrap().code, 220);
            let huge = vec![b'x'; 9000];
            let mut input = BufReader::new(huge.as_slice());
            assert!(reply(&mut input).await.is_err());
        });
        assert!(listing(b"type=file;size=7; x\n").is_ok());
        assert!(listing(b"not facts\n").is_err());
    }
    #[test]
    #[ignore = "Requires private verified FTPS fixture; STOU must not clobber concurrent targets and cancellation must close data I/O"]
    fn real_ftps_unique_creation_preserves_competitor_and_cancellation_is_prompt() {
        let _lock = deployment::tests::FTPS_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (config, root) = deployment::tests::ftps();
        let dir = tempfile::Builder::new().prefix("ftps-race-").tempdir_in(&root).unwrap();
        let dirname = dir.path().file_name().unwrap().to_str().unwrap();
        let target = format!("/{dirname}/new.bin");
        std::fs::write(dir.path().join("new.bin"), b"concurrent winner").unwrap();
        rc_core::Runtime::block_on(async {
            let mut client = Client::connect(&config, CancelToken::new()).await.unwrap();
            let actual = client.unique(&target, b"reviewed upload").await.unwrap();
            assert_ne!(actual, target);
            assert_eq!(std::fs::read(dir.path().join("new.bin")).unwrap(), b"concurrent winner");
            assert_eq!(std::fs::read(root.join(actual.trim_start_matches('/'))).unwrap(), b"reviewed upload");
        });
        let local = tempfile::tempdir().unwrap();
        let source = local.path().join("artifact.bin");
        std::fs::write(&source, vec![0x42; 1024 * 1024]).unwrap();
        let mut slow = config;
        slow.rate = Some(1024);
        let path = format!("/{dirname}/slow.bin");
        let plan = deployment::preview(deployment::Config::Ftps(slow.clone()), &source, path, true, true, CancelToken::new()).unwrap();
        let cancel = CancelToken::new();
        let token = cancel.clone();
        let handle = std::thread::spawn(move || deployment::apply(deployment::Config::Ftps(slow), plan, true, token));
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if std::fs::read_dir(dir.path()).unwrap().flatten().any(|e| e.file_name().to_string_lossy().starts_with("slow.bin.")) {
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        let start = Instant::now();
        cancel.cancel();
        assert!(matches!(handle.join().unwrap(), Err(Error::Cancelled)));
        assert!(start.elapsed() < Duration::from_secs(3));
    }
}
