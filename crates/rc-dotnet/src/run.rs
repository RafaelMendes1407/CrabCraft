//! Shared process launch plan and run events used by Cargo.

use crate::build::BuildEvent;
use crate::process::{OpHandle, OutputLine};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;
use tokio::sync::mpsc;

/// The resolved process to start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaunchPlan {
    pub program: PathBuf,
    pub args: Vec<String>,
    /// Variables set on top of the IDE's environment.
    pub env: Vec<(String, String)>,
    pub working_dir: PathBuf,
    /// URLs reported by a web application.
    pub urls: Vec<String>,
}

impl LaunchPlan {
    /// A readable command line for the console header.
    pub fn command_line(&self) -> String {
        let quote = |s: &str| if s.contains(' ') || s.is_empty() { format!("\"{s}\"") } else { s.to_string() };
        std::iter::once(quote(&self.program.to_string_lossy())).chain(self.args.iter().map(|a| quote(a))).collect::<Vec<_>>().join(" ")
    }

    pub fn env_var(&self, key: &str) -> Option<&str> {
        self.env.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunEvent {
    /// Events of the build before the launch.
    Build(BuildEvent),
    Started {
        pid: u32,
        plan: LaunchPlan,
    },
    Output(OutputLine),
    /// Last event of a run that started.
    Exited {
        code: Option<i32>,
        cancelled: bool,
        duration: Duration,
    },
    /// Last event when the build failed or the process could not start.
    Failed(String),
}

/// A running (or finished) program. `stop()` or dropping it kills the process tree.
pub struct RunHandle {
    op: OpHandle,
    input: mpsc::UnboundedSender<Vec<u8>>,
    pid: Arc<AtomicU32>,
}

impl RunHandle {
    /// Wraps a program started by another backend (Cargo).
    pub fn new(op: OpHandle, input: mpsc::UnboundedSender<Vec<u8>>, pid: Arc<AtomicU32>) -> Self {
        RunHandle { op, input, pid }
    }

    /// Stops the program (graceful first, then killed after a few seconds).
    pub fn stop(&self) {
        self.op.cancel();
    }

    pub fn is_finished(&self) -> bool {
        self.op.is_finished()
    }

    /// Process id once started.
    pub fn pid(&self) -> Option<u32> {
        Some(self.pid.load(Ordering::SeqCst)).filter(|&p| p != 0)
    }

    /// Writes to the program's stdin; false when it is gone.
    pub fn send_input(&self, text: &str) -> bool {
        self.input.send(text.as_bytes().to_vec()).is_ok()
    }
}

/// Time between the graceful stop request and the kill.
pub const STOP_GRACE: Duration = Duration::from_secs(5);
