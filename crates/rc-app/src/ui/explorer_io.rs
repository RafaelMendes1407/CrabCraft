//! Bounded static project-tree reads. Workers never invoke Cargo/MSBuild or evaluate code.
use rc_core::{
    Waker,
    workspace::{FileNode, is_hidden},
};
use rc_dotnet::ProjectInfo;
use std::{
    collections::HashMap,
    io::Read,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, SyncSender},
    },
};
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    Directory(PathBuf),
    Project(PathBuf),
}
impl Key {
    pub fn path(&self) -> &Path {
        match self {
            Self::Directory(path) | Self::Project(path) => path,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Io(std::io::ErrorKind),
    Unsupported,
    Limit,
    Cancelled,
    Changed,
    Busy,
}
#[derive(Debug)]
pub enum Value {
    Directory(Vec<FileNode>),
    Project(ProjectInfo),
}
struct Reply {
    key: Key,
    id: u64,
    result: Result<Value, Error>,
}
struct Work {
    key: Key,
    id: u64,
    cancel: Arc<AtomicBool>,
    out: mpsc::Sender<Reply>,
    waker: Waker,
}
fn check(cancel: &AtomicBool) -> Result<(), Error> {
    if cancel.load(Ordering::Acquire) {
        Err(Error::Cancelled)
    } else {
        Ok(())
    }
}
fn io(error: std::io::Error) -> Error {
    Error::Io(error.kind())
}
fn load(key: &Key, cancel: &AtomicBool) -> Result<Value, Error> {
    check(cancel)?;
    match key {
        Key::Directory(path) => {
            let before = std::fs::metadata(path).map_err(io)?.modified().ok();
            let mut rows = Vec::new();
            let mut bytes = 0;
            for (index, entry) in std::fs::read_dir(path).map_err(io)?.enumerate() {
                check(cancel)?;
                if index >= 20_000 {
                    return Err(Error::Limit);
                }
                let entry = entry.map_err(io)?;
                let name = entry.file_name().to_string_lossy().into_owned();
                let kind = entry.file_type().map_err(io)?;
                if is_hidden(&name, kind.is_dir()) {
                    continue;
                }
                bytes += name.len() + entry.path().as_os_str().len();
                if rows.len() >= 5000 || bytes > 8 * 1024 * 1024 {
                    return Err(Error::Limit);
                }
                rows.push(FileNode {
                    path: entry.path(),
                    name,
                    is_dir: kind.is_dir(),
                });
            }
            check(cancel)?;
            rows.sort_by_cached_key(|node| (!node.is_dir, node.name.to_lowercase()));
            if before != std::fs::metadata(path).map_err(io)?.modified().ok() {
                return Err(Error::Changed);
            }
            Ok(Value::Directory(rows))
        }
        Key::Project(path) => {
            let mut options = std::fs::OpenOptions::new();
            options.read(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
            }
            let mut file = options.open(path).map_err(io)?;
            let before = file.metadata().map_err(io)?;
            if !before.is_file() {
                return Err(Error::Unsupported);
            }
            if before.len() > 2 * 1024 * 1024 {
                return Err(Error::Limit);
            }
            let mut bytes = Vec::new();
            let mut chunk = [0u8; 8192];
            loop {
                check(cancel)?;
                let count = file.read(&mut chunk).map_err(io)?;
                if count == 0 {
                    break;
                }
                if bytes.len() + count > 2 * 1024 * 1024 {
                    return Err(Error::Limit);
                }
                bytes.extend_from_slice(&chunk[..count]);
            }
            let after = file.metadata().map_err(io)?;
            if before.modified().ok() != after.modified().ok() || before.len() != after.len() {
                return Err(Error::Changed);
            }
            let text = String::from_utf8(bytes).map_err(|_| Error::Unsupported)?;
            let project = ProjectInfo::parse(&text, path).map_err(|_| Error::Unsupported)?;
            if project.target_frameworks.len() > 32
                || project.packages.len() > 2048
                || project.project_refs.len() > 512
                || project
                    .target_frameworks
                    .iter()
                    .any(|name| name.len() > 128)
                || project
                    .packages
                    .iter()
                    .any(|package| package.name.len() > 1024 || package.version.len() > 1024)
                || project.sdk.len() > 1024
                || project.output_type.len() > 128
            {
                return Err(Error::Limit);
            }
            check(cancel)?;
            Ok(Value::Project(project))
        }
    }
}
fn pool() -> Option<&'static SyncSender<Work>> {
    static POOL: OnceLock<Option<SyncSender<Work>>> = OnceLock::new();
    POOL.get_or_init(|| {
        let (sender, receiver) = mpsc::sync_channel::<Work>(64);
        let receiver = Arc::new(Mutex::new(receiver));
        let mut started = 0;
        for index in 0..2 {
            let receiver = receiver.clone();
            if std::thread::Builder::new()
                .name(format!("rc-explorer-{index}"))
                .spawn(move || {
                    loop {
                        let work = match receiver.lock() {
                            Ok(receiver) => match receiver.recv() {
                                Ok(work) => work,
                                Err(_) => break,
                            },
                            Err(_) => break,
                        };
                        let result = load(&work.key, &work.cancel);
                        let _ = work.out.send(Reply {
                            key: work.key,
                            id: work.id,
                            result,
                        });
                        work.waker.wake();
                    }
                })
                .is_ok()
            {
                started += 1;
            }
        }
        (started > 0).then_some(sender)
    })
    .as_ref()
}
struct Job {
    id: u64,
    cancel: Arc<AtomicBool>,
}
impl Drop for Job {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}
pub struct Io {
    tx: mpsc::Sender<Reply>,
    rx: mpsc::Receiver<Reply>,
    overloaded: bool,
    pending: HashMap<Key, Job>,
    pub errors: HashMap<Key, Error>,
    next: u64,
    root: Option<PathBuf>,
    waker: Waker,
}
impl Default for Io {
    fn default() -> Self {
        let waker = Waker::noop();
        let (tx, rx) = mpsc::channel();
        Self {
            tx,
            rx,
            overloaded: false,
            pending: HashMap::new(),
            errors: HashMap::new(),
            next: 0,
            root: None,
            waker,
        }
    }
}
impl Io {
    pub fn attach(&mut self, ctx: &egui::Context) {
        let ctx = ctx.clone();
        self.waker = Waker::new(move || ctx.request_repaint());
    }
    pub fn root(&mut self, root: &Path) -> bool {
        if self.root.as_deref() == Some(root) {
            return false;
        }
        self.reset();
        self.root = Some(root.into());
        true
    }
    pub fn reset(&mut self) {
        self.pending.clear();
        self.errors.clear();
        self.overloaded = false;
        while self.rx.try_recv().is_ok() {}
    }
    pub fn cancel(&mut self) {
        let keys = self.pending.drain().map(|(key, _)| key).collect::<Vec<_>>();
        for key in keys {
            self.record_error(key, Error::Cancelled);
        }
    }
    pub fn invalidate(&mut self, path: &Path) {
        self.pending.retain(|key, _| key.path() != path);
        self.errors.retain(|key, _| key.path() != path);
    }
    pub fn record_error(&mut self, key: Key, error: Error) {
        if self.errors.len() >= 256 && !self.errors.contains_key(&key) {
            self.overloaded = true;
            return;
        }
        self.errors.insert(key, error);
    }
    pub fn error(&self, key: &Key) -> Option<Error> {
        self.errors.get(key).copied()
    }
    pub fn busy(&self) -> bool {
        !self.pending.is_empty()
    }
    pub fn request(&mut self, key: Key) {
        if self.overloaded || self.pending.contains_key(&key) || self.errors.contains_key(&key) {
            return;
        }
        if self.pending.len() >= 64 {
            self.record_error(key, Error::Busy);
            return;
        }
        self.next = self.next.wrapping_add(1);
        let cancel = Arc::new(AtomicBool::new(false));
        let id = self.next;
        let work = Work {
            key: key.clone(),
            id,
            cancel: cancel.clone(),
            out: self.tx.clone(),
            waker: self.waker.clone(),
        };
        let sent = pool().is_some_and(|pool| pool.try_send(work).is_ok());
        if sent {
            self.pending.insert(key, Job { id, cancel });
        } else {
            self.record_error(key, Error::Busy);
        }
    }
    pub fn drain(&mut self) -> Vec<(Key, Value)> {
        let mut values = Vec::new();
        for _ in 0..2 {
            let Ok(reply) = self.rx.try_recv() else {
                break;
            };
            if self
                .pending
                .get(&reply.key)
                .is_none_or(|job| job.id != reply.id)
            {
                continue;
            }
            self.pending.remove(&reply.key);
            match reply.result {
                Ok(value) => values.push((reply.key, value)),
                Err(error) => self.record_error(reply.key, error),
            }
        }
        if !self.pending.is_empty() {
            self.waker.wake();
        }
        values
    }
}
