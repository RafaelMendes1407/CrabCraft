//! Attaching the debugger automatically (D-15): while watching, a thread reads the process list
//! every [`POLL`] and reports processes that started after the watch began and whose name
//! matches one of the user's patterns. The app attaches GDB to the first one when no debug
//! session runs.

use rc_core::processes::{self, Process};
use rc_core::{Inbox, Waker};
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// How often the process list is read.
pub const POLL: Duration = Duration::from_millis(400);
/// At most this many patterns, of at most [`MAX_PATTERN_CHARS`] each.
pub const MAX_PATTERNS: usize = 16;
pub const MAX_PATTERN_CHARS: usize = 120;

/// The patterns in a comma-separated text: trimmed, without blanks and duplicates, bounded.
pub fn parse_patterns(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for p in text.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let p: String = p.chars().take(MAX_PATTERN_CHARS).collect();
        if !out.contains(&p) && out.len() < MAX_PATTERNS {
            out.push(p);
        }
    }
    out
}

/// Whether `name` matches `pattern`: `*` is any text, `?` one character; case matters, as in
/// process names.
pub fn glob(pattern: &str, name: &str) -> bool {
    let (p, n): (Vec<char>, Vec<char>) = (pattern.chars().collect(), name.chars().collect());
    // Iterative matching with one backtrack point: linear in practice, no recursion.
    let (mut i, mut j, mut star, mut mark) = (0, 0, None, 0);
    while j < n.len() {
        if i < p.len() && (p[i] == '?' || p[i] == n[j]) {
            i += 1;
            j += 1;
        } else if i < p.len() && p[i] == '*' {
            star = Some(i);
            i += 1;
            mark = j;
        } else if let Some(s) = star {
            i = s + 1;
            mark += 1;
            j = mark;
        } else {
            return false;
        }
    }
    p[i..].iter().all(|c| *c == '*')
}

/// Whether a process matches one of the patterns, by its name or its executable's.
pub fn matches(patterns: &[String], p: &Process) -> bool {
    patterns.iter().any(|pat| glob(pat, &p.name) || (!p.exe.is_empty() && glob(pat, &p.exe)))
}

/// A running watch; dropping it stops the thread.
pub struct Watch {
    pub patterns: Vec<String>,
    stop: Arc<AtomicBool>,
    found: Inbox<Process>,
}

impl Watch {
    /// Starts watching for new processes that match `patterns` (the ones running now are not
    /// reported, nor this program's own).
    pub fn start(patterns: Vec<String>, waker: Waker) -> Watch {
        let stop = Arc::new(AtomicBool::new(false));
        let found = Inbox::new(waker);
        let out = found.sender();
        let (flag, wanted) = (stop.clone(), patterns.clone());
        std::thread::spawn(move || {
            let mut seen: HashSet<u32> = processes::list().into_iter().map(|p| p.pid).collect();
            while !flag.load(Ordering::Relaxed) {
                std::thread::sleep(POLL);
                let now = processes::list();
                for p in &now {
                    if seen.insert(p.pid) && matches(&wanted, p) && !out.send(p.clone()) {
                        return;
                    }
                }
                // Forget the ones that ended, so a reused pid counts as new.
                let alive: HashSet<u32> = now.iter().map(|p| p.pid).collect();
                seen.retain(|pid| alive.contains(pid));
            }
        });
        Watch { patterns, stop, found }
    }

    /// The matching processes found since the last call.
    pub fn found(&self) -> Vec<Process> {
        self.found.drain()
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl crate::app::CrabCraftApp {
    /// Starts watching for the patterns typed in `auto_attach_text`; `false` when there are
    /// none or no Rust project is open.
    pub fn start_auto_attach(&mut self) -> bool {
        let patterns = parse_patterns(&self.auto_attach_text);
        if patterns.is_empty() || self.runner.cargo_workspace.is_none() {
            return false;
        }
        self.auto_attach_text = patterns.join(", ");
        self.session_dirty = true;
        self.auto_attach = Some(Watch::start(patterns, self.debugger.waker()));
        true
    }

    pub fn stop_auto_attach(&mut self) {
        self.auto_attach = None;
    }

    /// Attaches to the first matching process found, unless a debug session runs (those found
    /// meanwhile are let go).
    pub fn poll_auto_attach(&mut self) {
        let Some(found) = self.auto_attach.as_ref().map(Watch::found) else { return };
        let busy = self.debugger.session.as_ref().is_some_and(|s| !s.state().is_ended());
        if busy {
            return;
        }
        if let Some(p) = found.into_iter().next() {
            self.notify(rc_kit::i18n::trf("Attached automatically to {name} ({pid})", &[("name", &p.name), ("pid", &p.pid)]), false);
            let outs = self.debugger.attach_gdb(p.pid, &p.name, &self.docs);
            self.handle_debug(outs);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs_match_names() {
        assert!(glob("server", "server"));
        assert!(!glob("server", "server2"));
        assert!(glob("serv*", "server"));
        assert!(glob("*-worker", "shop-worker"));
        assert!(glob("a*b*c", "aXbYbZc"));
        assert!(glob("w?rker", "worker"));
        assert!(!glob("w?rker", "wrker"));
        assert!(glob("*", ""));
        assert!(!glob("", "x"));
        assert!(!glob("Server", "server"));
        assert!(!glob("*a*a*a*a*a*b", &"a".repeat(200)));
    }

    #[test]
    fn patterns_are_cleaned() {
        assert_eq!(parse_patterns(" api*, ,worker,api*,"), ["api*", "worker"]);
        assert_eq!(parse_patterns(&"x,".repeat(40)).len(), 1);
        let many: String = (0..40).map(|i| format!("p{i},")).collect();
        assert_eq!(parse_patterns(&many).len(), MAX_PATTERNS);
        assert_eq!(parse_patterns(&"y".repeat(500))[0].chars().count(), MAX_PATTERN_CHARS);
    }

    #[test]
    fn a_process_matches_by_its_name_or_executable() {
        let p = Process { pid: 1, name: "very-long-serve".into(), exe: "very-long-server-name".into(), command: String::new() };
        assert!(matches(&["*-server-name".into()], &p));
        assert!(matches(&["very-long-serve".into()], &p));
        assert!(!matches(&["other".into()], &p));
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn reports_a_matching_process_that_starts_after_the_watch() {
        let old = std::process::Command::new("sleep").arg("30").spawn().unwrap();
        std::thread::sleep(Duration::from_millis(100));
        let watch = Watch::start(vec!["sleep".into()], Waker::noop());
        std::thread::sleep(POLL * 2);
        let mut new = std::process::Command::new("sleep").arg("31").spawn().unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let mut found = Vec::new();
        while found.is_empty() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
            found.extend(watch.found());
        }
        let mut old = old;
        for c in [&mut old, &mut new] {
            let _ = c.kill();
            let _ = c.wait();
        }
        assert!(found.iter().any(|p| p.pid == new.id()), "{found:?}");
        assert!(!found.iter().any(|p| p.pid == old.id()));
    }
}
