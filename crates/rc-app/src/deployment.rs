//! Reviewed file deployments. Snapshots and hashes are produced only on cancellable workers.
use crate::{deploy_ftps, deploy_sftp, remote_transfer};
use rc_dotnet::process::CancelToken;
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::SystemTime,
};
pub const MAX: usize = 32 * 1024 * 1024;
#[derive(Clone, Debug)]
pub enum Error {
    Input,
    Trust,
    Limit,
    Cancelled,
    Timeout,
    Stale,
    Protocol,
    Unsupported,
    Remote(u32),
    Tls,
    Authentication,
    Transport(i32),
    Io,
}
impl From<std::io::Error> for Error {
    fn from(_: std::io::Error) -> Self {
        Self::Io
    }
}
impl From<remote_transfer::Error> for Error {
    fn from(e: remote_transfer::Error) -> Self {
        match e {
            remote_transfer::Error::Input => Self::Input,
            remote_transfer::Error::Limit => Self::Limit,
            remote_transfer::Error::Cancelled => Self::Cancelled,
            remote_transfer::Error::Timeout => Self::Timeout,
            remote_transfer::Error::Stale => Self::Stale,
            remote_transfer::Error::Io(_) => Self::Io,
            remote_transfer::Error::Process(_) => Self::Transport(-1),
        }
    }
}
#[derive(Clone, PartialEq, Eq)]
pub enum Config {
    Sftp(remote_transfer::Config),
    Ftps(deploy_ftps::Config),
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct Stamp {
    size: u64,
    modified: SystemTime,
    mode: u32,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}
impl Stamp {
    fn new(meta: &std::fs::Metadata) -> Result<Self, Error> {
        if !meta.is_file() {
            return Err(Error::Input);
        }
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        Ok(Self {
            size: meta.len(),
            modified: meta.modified()?,
            #[cfg(unix)]
            mode: meta.mode() & 0o777,
            #[cfg(not(unix))]
            mode: 0o644,
            #[cfg(unix)]
            device: meta.dev(),
            #[cfg(unix)]
            inode: meta.ino(),
        })
    }
}
#[derive(Clone)]
pub struct Source {
    pub path: PathBuf,
    pub bytes: Arc<[u8]>,
    pub hash: String,
    pub mode: u32,
    stamp: Stamp,
}
pub fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
pub fn source(path: &Path, cancel: &CancelToken) -> Result<Source, Error> {
    source_limited(path, cancel, MAX)
}
pub(crate) fn source_limited(
    path: &Path,
    cancel: &CancelToken,
    limit: usize,
) -> Result<Source, Error> {
    use std::io::Read;
    if cancel.is_cancelled() {
        return Err(Error::Cancelled);
    }
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let before_path = std::fs::symlink_metadata(&path)?;
    if !before_path.is_file() || before_path.file_type().is_symlink() {
        return Err(Error::Input);
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let mut file = options.open(&path)?;
    let stamp = Stamp::new(&file.metadata()?)?;
    if stamp.size > limit as u64 {
        return Err(Error::Limit);
    }
    if stamp != Stamp::new(&before_path)? {
        return Err(Error::Stale);
    }
    let mut bytes = Vec::with_capacity(stamp.size as usize);
    let mut hash = Sha256::new();
    let mut chunk = [0; 65536];
    loop {
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let n = file.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        if bytes.len().saturating_add(n) > limit {
            return Err(Error::Limit);
        }
        hash.update(&chunk[..n]);
        bytes.extend_from_slice(&chunk[..n]);
    }
    if stamp != Stamp::new(&file.metadata()?)?
        || stamp != Stamp::new(&std::fs::symlink_metadata(&path)?)?
        || bytes.len() as u64 != stamp.size
    {
        return Err(Error::Stale);
    }
    Ok(Source {
        path,
        bytes: Arc::from(bytes),
        hash: format!("{:x}", hash.finalize()),
        mode: stamp.mode,
        stamp,
    })
}
#[derive(Clone)]
pub enum RemoteImage {
    Missing,
    Bytes {
        bytes: Arc<[u8]>,
        hash: String,
        stamp: String,
        mode: Option<u32>,
    },
}
impl RemoteImage {
    pub fn size(&self) -> Option<usize> {
        match self {
            Self::Missing => None,
            Self::Bytes { bytes, .. } => Some(bytes.len()),
        }
    }
    pub fn hash(&self) -> Option<&str> {
        match self {
            Self::Missing => None,
            Self::Bytes { hash, .. } => Some(hash),
        }
    }
    pub fn same(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Missing, Self::Missing) => true,
            (
                Self::Bytes {
                    hash: a,
                    stamp: sa,
                    bytes: ba,
                    ..
                },
                Self::Bytes {
                    hash: b,
                    stamp: sb,
                    bytes: bb,
                    ..
                },
            ) => a == b && sa == sb && ba.len() == bb.len(),
            _ => false,
        }
    }
}
#[derive(Clone)]
pub struct Preview {
    pub config: Config,
    pub source: Source,
    pub remote: String,
    pub before: RemoteImage,
    pub create: bool,
    pub source_text: Option<String>,
    pub remote_text: Option<String>,
}
pub struct Receipt {
    pub remote: String,
    pub hash: String,
    pub size: usize,
}
pub fn path(value: &str) -> Result<(), Error> {
    if !value.starts_with('/')
        || value.len() > 4096
        || value.ends_with('/')
        || value.chars().any(char::is_control)
        || value.split('/').any(|p| p == ".." || p == ".")
    {
        return Err(Error::Input);
    }
    Ok(())
}
fn snippet(bytes: &[u8]) -> Option<String> {
    if bytes.len() > 65536 || bytes.contains(&0) {
        return None;
    }
    let text = std::str::from_utf8(bytes).ok()?;
    Some(text.chars().take(8192).collect())
}
pub fn preview(
    config: Config,
    local: &Path,
    remote: String,
    create: bool,
    trusted: bool,
    cancel: CancelToken,
) -> Result<Preview, Error> {
    if !trusted {
        return Err(Error::Trust);
    }
    path(&remote)?;
    let source = source(local, &cancel)?;
    let before = match &config {
        Config::Sftp(config) => rc_core::Runtime::block_on(async {
            let mut client = deploy_sftp::Client::connect(config, cancel.clone()).await?;
            client.image(&remote).await
        })?,
        Config::Ftps(config) => deploy_ftps::image(config, &remote, &cancel)?,
    };
    if matches!(before, RemoteImage::Missing) != create {
        return Err(Error::Stale);
    }
    let source_text = snippet(&source.bytes);
    let remote_text = match &before {
        RemoteImage::Missing => None,
        RemoteImage::Bytes { bytes, .. } => snippet(bytes),
    };
    Ok(Preview {
        config,
        source,
        remote,
        before,
        create,
        source_text,
        remote_text,
    })
}
pub(crate) fn local_current(preview: &Preview, cancel: &CancelToken) -> Result<(), Error> {
    let now = source(&preview.source.path, cancel)?;
    if now.hash != preview.source.hash || now.stamp != preview.source.stamp {
        return Err(Error::Stale);
    }
    Ok(())
}
pub fn apply(
    config: Config,
    preview: Preview,
    trusted: bool,
    cancel: CancelToken,
) -> Result<Receipt, Error> {
    if !trusted {
        return Err(Error::Trust);
    }
    if config != preview.config {
        return Err(Error::Stale);
    }
    path(&preview.remote)?;
    local_current(&preview, &cancel)?;
    match &config {
        Config::Ftps(config) => deploy_ftps::apply(config, &preview, &cancel),
        Config::Sftp(config) => rc_core::Runtime::block_on(async {
            let mut client = deploy_sftp::Client::connect(config, cancel.clone()).await?;
            let now = client.image(&preview.remote).await?;
            if !now.same(&preview.before) {
                return Err(Error::Stale);
            }
            let dir = tempfile::Builder::new()
                .prefix("crabcraft-deploy-")
                .rand_bytes(16)
                .tempdir()?;
            let nonce = dir
                .path()
                .file_name()
                .and_then(|s| s.to_str())
                .ok_or(Error::Input)?;
            let parent = preview
                .remote
                .rsplit_once('/')
                .map(|p| p.0)
                .ok_or(Error::Input)?;
            let temporary = format!("{parent}/.{nonce}");
            let mode = match &preview.before {
                RemoteImage::Bytes {
                    mode: Some(mode), ..
                } => *mode,
                _ => preview.source.mode,
            };
            client
                .upload(&temporary, &preview.source.bytes, mode)
                .await?;
            let result = async {
                let image = client.image(&temporary).await?;
                if image.hash() != Some(preview.source.hash.as_str())
                    || image.size() != Some(preview.source.bytes.len())
                {
                    return Err(Error::Stale);
                }
                local_current(&preview, &cancel)?;
                if !client.image(&preview.remote).await?.same(&preview.before) {
                    return Err(Error::Stale);
                }
                client
                    .publish(&temporary, &preview.remote, !preview.create)
                    .await?;
                Ok(Receipt {
                    remote: preview.remote.clone(),
                    hash: preview.source.hash.clone(),
                    size: preview.source.bytes.len(),
                })
            }
            .await;
            if result.is_err() {
                let _ = client.remove_temporary(&temporary).await;
            }
            result
        }),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    pub(crate) static FTPS_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    pub(crate) fn ftps() -> (deploy_ftps::Config, PathBuf) {
        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read("/tmp/crabcraft-ftps/connection.json").unwrap())
                .unwrap();
        let config = deploy_ftps::Config::new(
            value["host"].as_str().unwrap().into(),
            value["port"].as_u64().unwrap() as u16,
            value["user"].as_str().unwrap().into(),
            value["password"].as_str().unwrap().into(),
            Some(value["ca"].as_str().unwrap().into()),
            "sha256//xZetbpyqKJd7ccYSVQfMKBN+6Zgr880bWPqJTOrPi/U=".into(),
        );
        (config, value["root"].as_str().unwrap().into())
    }
    #[test]
    fn trust_and_regular_file_guards_precede_any_transport() {
        let config = Config::Sftp(crate::remote_transfer::tests::config());
        assert!(matches!(
            preview(
                config.clone(),
                Path::new("/missing/source"),
                "/missing/remote".into(),
                true,
                false,
                CancelToken::new()
            ),
            Err(Error::Trust)
        ));
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(
            source(dir.path(), &CancelToken::new()),
            Err(Error::Input)
        ));
        let source_path = dir.path().join("file.rs");
        std::fs::write(&source_path, "fn main(){}\n").unwrap();
        let cancel = CancelToken::new();
        cancel.cancel();
        assert!(matches!(
            source(&source_path, &cancel),
            Err(Error::Cancelled)
        ));
        for bad in ["relative", "/tmp/../other", "/tmp/a\nb", "/tmp/a/"] {
            assert!(path(bad).is_err());
        }
    }
    #[cfg(unix)]
    #[test]
    fn local_fifo_and_symlink_never_block_or_become_deployment_sources() {
        let dir = tempfile::tempdir().unwrap();
        let fifo = dir.path().join("fifo");
        let name = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        let start = std::time::Instant::now();
        assert!(source(&fifo, &CancelToken::new()).is_err());
        assert!(start.elapsed() < std::time::Duration::from_secs(1));
        let file = dir.path().join("file");
        std::fs::write(&file, b"\0binary\xff").unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(file, &link).unwrap();
        assert!(source(&link, &CancelToken::new()).is_err());
    }
    #[test]
    #[ignore = "Requires the private OpenSSH fixture; exercises raw SFTP v3 binary/new/replace semantics"]
    fn real_sftp_deployment_freezes_binary_new_targets_refuses_stale_and_preserves_concurrent_destinations()
     {
        let _lock = crate::remote_transfer::tests::SSH_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let local = dir.path().join("hello.rs");
        std::fs::write(&local, "fn main(){println!(\"deployed Rust 🦀\");}\n").unwrap();
        let binary = dir.path().join("local-bin");
        let output = std::process::Command::new("rustc")
            .arg(&local)
            .arg("-o")
            .arg(&binary)
            .output()
            .unwrap();
        assert!(output.status.success());
        let destination = dir.path().join("deployed-bin");
        let config = Config::Sftp(crate::remote_transfer::tests::config());
        let plan = preview(
            config.clone(),
            &binary,
            destination.display().to_string(),
            true,
            true,
            CancelToken::new(),
        )
        .unwrap();
        assert!(!destination.exists());
        let size = plan.source.bytes.len();
        let hash = plan.source.hash.clone();
        let receipt = apply(config.clone(), plan, true, CancelToken::new()).unwrap();
        assert_eq!(receipt.hash, hash);
        assert_eq!(receipt.size, size);
        assert_eq!(digest(&std::fs::read(&destination).unwrap()), hash);
        assert!(
            std::process::Command::new(&destination)
                .status()
                .unwrap()
                .success()
        );
        std::fs::File::open(&destination)
            .unwrap()
            .set_times(
                std::fs::FileTimes::new()
                    .set_accessed(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1)),
            )
            .unwrap();
        let plan = preview(
            config.clone(),
            &local,
            destination.display().to_string(),
            false,
            true,
            CancelToken::new(),
        )
        .unwrap();
        std::fs::write(&local, "fn main(){println!(\"changed\");}\n").unwrap();
        assert!(matches!(
            apply(config.clone(), plan, true, CancelToken::new()),
            Err(Error::Stale)
        ));
        assert_eq!(digest(&std::fs::read(&destination).unwrap()), hash);
        let plan = preview(
            config.clone(),
            &local,
            destination.display().to_string(),
            false,
            true,
            CancelToken::new(),
        )
        .unwrap();
        std::fs::write(&destination, b"external winner").unwrap();
        assert!(matches!(
            apply(config.clone(), plan, true, CancelToken::new()),
            Err(Error::Stale)
        ));
        assert_eq!(std::fs::read(&destination).unwrap(), b"external winner");
        let plan = preview(
            config.clone(),
            &local,
            destination.display().to_string(),
            false,
            true,
            CancelToken::new(),
        )
        .unwrap();
        apply(config, plan, true, CancelToken::new()).unwrap();
        assert_eq!(
            std::fs::read(&destination).unwrap(),
            std::fs::read(&local).unwrap()
        );
        rc_core::Runtime::block_on(async {
            let mut client = deploy_sftp::Client::connect(
                &crate::remote_transfer::tests::config(),
                CancelToken::new(),
            )
            .await
            .unwrap();
            let temporary = dir.path().join("owned-temp");
            client
                .upload(temporary.to_str().unwrap(), b"our upload", 0o644)
                .await
                .unwrap();
            let target = dir.path().join("new-race.bin");
            std::fs::write(&target, b"concurrent winner").unwrap();
            assert!(
                client
                    .publish(temporary.to_str().unwrap(), target.to_str().unwrap(), false)
                    .await
                    .is_err()
            );
            assert_eq!(std::fs::read(&target).unwrap(), b"concurrent winner");
            client
                .remove_temporary(temporary.to_str().unwrap())
                .await
                .unwrap();
        });
        let mut bad = crate::remote_transfer::tests::config();
        let hosts = dir.path().join("bad-hosts");
        std::fs::write(&hosts, "").unwrap();
        bad.known_hosts = hosts;
        assert!(
            preview(
                Config::Sftp(bad),
                &local,
                dir.path().join("bad.bin").display().to_string(),
                true,
                true,
                CancelToken::new()
            )
            .is_err()
        );
    }
    #[test]
    #[ignore = "Requires private explicit-FTPS fixture with TLS data/control verification"]
    fn real_ftps_deployment_reviews_text_binary_new_names_tls_pinning_and_stale() {
        let _lock = FTPS_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (config, root) = ftps();
        let local = tempfile::tempdir().unwrap();
        let prefix = tempfile::Builder::new()
            .prefix("ftps-evidence-")
            .tempdir_in(&root)
            .unwrap();
        let name = prefix.path().file_name().unwrap().to_str().unwrap();
        let path = format!("/{name}/new.rs");
        let source_path = local.path().join("main.rs");
        std::fs::write(&source_path, "fn main(){println!(\"FTPS Rust 🦀\");}\n").unwrap();
        let plan = preview(
            Config::Ftps(config.clone()),
            &source_path,
            path.clone(),
            true,
            true,
            CancelToken::new(),
        )
        .unwrap();
        assert!(!prefix.path().join("new.rs").exists());
        let receipt = apply(Config::Ftps(config.clone()), plan, true, CancelToken::new()).unwrap();
        assert_eq!(
            digest(&std::fs::read(root.join(receipt.remote.trim_start_matches('/'))).unwrap()),
            receipt.hash
        );
        let plan = preview(
            Config::Ftps(config.clone()),
            &source_path,
            receipt.remote.clone(),
            false,
            true,
            CancelToken::new(),
        )
        .unwrap();
        std::fs::write(
            root.join(receipt.remote.trim_start_matches('/')),
            b"external",
        )
        .unwrap();
        assert!(matches!(
            apply(Config::Ftps(config.clone()), plan, true, CancelToken::new()),
            Err(Error::Stale)
        ));
        let plan = preview(
            Config::Ftps(config.clone()),
            &source_path,
            receipt.remote.clone(),
            false,
            true,
            CancelToken::new(),
        )
        .unwrap();
        apply(Config::Ftps(config.clone()), plan, true, CancelToken::new()).unwrap();
        assert_eq!(
            std::fs::read(root.join(receipt.remote.trim_start_matches('/'))).unwrap(),
            std::fs::read(&source_path).unwrap()
        );
        let binary = local.path().join("program");
        let result = std::process::Command::new("rustc")
            .arg(&source_path)
            .arg("-o")
            .arg(&binary)
            .output()
            .unwrap();
        assert!(result.status.success());
        let binary_path = format!("/{name}/program.bin");
        let plan = preview(
            Config::Ftps(config.clone()),
            &binary,
            binary_path,
            true,
            true,
            CancelToken::new(),
        )
        .unwrap();
        let expected = plan.source.hash.clone();
        let receipt = apply(Config::Ftps(config.clone()), plan, true, CancelToken::new()).unwrap();
        assert_eq!(
            digest(&std::fs::read(root.join(receipt.remote.trim_start_matches('/'))).unwrap()),
            expected
        );
        let mut bad = config.clone();
        bad.ca = None;
        assert!(matches!(
            preview(
                Config::Ftps(bad),
                &source_path,
                format!("/{name}/untrusted.rs"),
                true,
                true,
                CancelToken::new()
            ),
            Err(Error::Tls)
        ));
        let mut bad = config.clone();
        bad.pin = "sha256//AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=".into();
        assert!(matches!(
            preview(
                Config::Ftps(bad),
                &source_path,
                format!("/{name}/bad-pin.rs"),
                true,
                true,
                CancelToken::new()
            ),
            Err(Error::Tls)
        ));
        let mut bad = config;
        bad.password = "wrong-fixture-password".into();
        assert!(matches!(
            preview(
                Config::Ftps(bad),
                &source_path,
                format!("/{name}/bad-auth.rs"),
                true,
                true,
                CancelToken::new()
            ),
            Err(Error::Authentication)
        ));
    }
}
