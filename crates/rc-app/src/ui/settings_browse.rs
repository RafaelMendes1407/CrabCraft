//! File chooser work belongs to one Settings draft and is discarded with that draft.
use rc_core::{Settings, Waker};
use std::{future::Future, path::PathBuf, sync::mpsc};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolPath {
    Analyzer,
    Html,
    Css,
    Typescript,
    Tsserver,
    Gdb,
    Lldb,
}

impl ToolPath {
    pub fn value(self, settings: &mut Settings) -> &mut Option<PathBuf> {
        match self {
            Self::Analyzer => &mut settings.rust_analyzer_path,
            Self::Html => &mut settings.web_html_server,
            Self::Css => &mut settings.web_css_server,
            Self::Typescript => &mut settings.web_typescript_server,
            Self::Tsserver => &mut settings.web_tsserver_path,
            Self::Gdb => &mut settings.gdb_path,
            Self::Lldb => &mut settings.lldb_dap_path,
        }
    }
}

struct Pending {
    target: ToolPath,
    receiver: mpsc::Receiver<Option<PathBuf>>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Pending {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[derive(Default)]
pub struct SettingsBrowse {
    pending: Option<Pending>,
}
impl SettingsBrowse {
    pub fn busy(&self) -> bool {
        self.pending.is_some()
    }
    pub fn cancel(&mut self) {
        self.pending = None;
    }
    pub fn start(&mut self, target: ToolPath, current: Option<PathBuf>, waker: Waker) {
        self.start_with(target, waker, async move {
            let mut chooser = rfd::AsyncFileDialog::new();
            if let Some(parent) = current.as_deref().and_then(|path| path.parent()) {
                chooser = chooser.set_directory(parent);
            }
            chooser.pick_file().await.map(|file| file.path().to_path_buf())
        });
    }
    fn start_with(&mut self, target: ToolPath, waker: Waker, future: impl Future<Output = Option<PathBuf>> + Send + 'static) {
        self.cancel();
        let (sender, receiver) = mpsc::channel();
        let task = rc_core::Runtime::spawn(async move {
            let result = future.await;
            let _ = sender.send(result);
            waker.wake();
        });
        self.pending = Some(Pending { target, receiver, task });
    }
    pub fn poll(&mut self, settings: &mut Settings) {
        let Some(pending) = &self.pending else { return };
        match pending.receiver.try_recv() {
            Ok(path) => {
                if let Some(path) = path {
                    *pending.target.value(settings) = Some(path);
                }
                self.pending = None;
            }
            Err(mpsc::TryRecvError::Disconnected) => self.pending = None,
            Err(mpsc::TryRecvError::Empty) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn finish(browse: &mut SettingsBrowse, settings: &mut Settings) {
        let started = std::time::Instant::now();
        while browse.busy() && started.elapsed() < std::time::Duration::from_secs(2) {
            browse.poll(settings);
            std::thread::yield_now();
        }
        assert!(!browse.busy());
    }
    #[test]
    fn chooser_updates_only_its_target_and_only_its_own_draft() {
        let mut old = SettingsBrowse::default();
        let (send, receive) = tokio::sync::oneshot::channel();
        old.start_with(ToolPath::Gdb, Waker::noop(), async move { receive.await.ok() });
        drop(old);
        let mut new = SettingsBrowse::default();
        let mut settings = Settings::default();
        let original = settings.gdb_path.clone();
        let _ = send.send(PathBuf::from("discarded"));
        new.poll(&mut settings);
        assert_eq!(settings.gdb_path, original);
        new.start_with(ToolPath::Analyzer, Waker::noop(), async { Some(PathBuf::from("chosen-analyzer")) });
        finish(&mut new, &mut settings);
        assert_eq!(settings.rust_analyzer_path, Some(PathBuf::from("chosen-analyzer")));
        assert_eq!(settings.gdb_path, original);
    }
    #[test]
    fn cancelled_chooser_never_overwrites_the_draft() {
        let mut browse = SettingsBrowse::default();
        let (send, receive) = tokio::sync::oneshot::channel();
        browse.start_with(ToolPath::Gdb, Waker::noop(), async move { receive.await.ok() });
        let mut settings = Settings::default();
        let original = settings.gdb_path.clone();
        browse.cancel();
        let _ = send.send(PathBuf::from("discarded"));
        browse.poll(&mut settings);
        assert_eq!(settings.gdb_path, original);
        assert!(!browse.busy());
    }
}
