//! Running processes of the current user, to pick one to attach a debugger to.

/// One running program.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Process {
    pub pid: u32,
    pub name: String,
    /// The file name of its executable (`name` is cut to 15 bytes by Linux).
    pub exe: String,
    /// Its command line.
    pub command: String,
}

/// The user's processes, newest first. Empty where the system does not expose them this way.
pub fn list() -> Vec<Process> {
    let me = std::process::id();
    let Ok(entries) = std::fs::read_dir("/proc") else { return Vec::new() };
    let mut out: Vec<Process> = entries
        .flatten()
        .filter_map(|e| e.file_name().to_str()?.parse::<u32>().ok())
        .filter(|pid| *pid != me)
        .filter_map(read)
        .collect();
    out.sort_by_key(|p| std::cmp::Reverse(p.pid));
    out
}

fn read(pid: u32) -> Option<Process> {
    let dir = std::path::PathBuf::from(format!("/proc/{pid}"));
    // Other users' processes hide their executable, and could not be attached to.
    let exe = std::fs::read_link(dir.join("exe")).ok()?;
    let exe = exe.file_name().map(|n| n.to_string_lossy().trim_end_matches(" (deleted)").to_string()).unwrap_or_default();
    let name = std::fs::read_to_string(dir.join("comm")).ok()?.trim().to_string();
    let raw = std::fs::read(dir.join("cmdline")).ok()?;
    Some(Process { pid, name, exe, command: command_line(&raw) })
}

/// `/proc/<pid>/cmdline` separates arguments with zero bytes.
fn command_line(raw: &[u8]) -> String {
    String::from_utf8_lossy(raw).trim_end_matches('\0').replace('\0', " ")
}

/// Linux process identity, retained across a terminal picker refresh to reject PID reuse.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalProcess { pub process: Process, pub started: u64 }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalProcessError { Unsupported, Gone, Cancelled, Limit }

fn limited(path: &std::path::Path, limit: usize) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut bytes = Vec::new();
    std::fs::File::open(path).ok()?.take(limit as u64 + 1).read_to_end(&mut bytes).ok()?;
    (bytes.len() <= limit).then_some(bytes)
}

fn identity(pid: u32) -> Option<(u32,u64)> {
    let bytes = limited(&std::path::PathBuf::from(format!("/proc/{pid}/stat")), 8192)?;
    let text = std::str::from_utf8(&bytes).ok()?;
    let (_, tail) = text.rsplit_once(") ")?;
    let fields: Vec<_> = tail.split_whitespace().take(20).collect();
    Some((fields.get(1)?.parse().ok()?, fields.get(19)?.parse().ok()?))
}

/// Snapshot only the selected terminal process and its descendants. No shell or project
/// command is executed. Work is bounded and intended for a background worker.
pub fn terminal_processes(root: u32, cancel: &std::sync::atomic::AtomicBool) -> Result<Vec<TerminalProcess>, TerminalProcessError> {
    use std::sync::atomic::Ordering;
    if !cfg!(target_os="linux") { return Err(TerminalProcessError::Unsupported); }
    let root_identity = identity(root).ok_or(TerminalProcessError::Gone)?;
    let entries = std::fs::read_dir("/proc").map_err(|_|TerminalProcessError::Unsupported)?;
    let mut parents = std::collections::HashMap::new();
    for (count, entry) in entries.enumerate() {
        if cancel.load(Ordering::Relaxed) { return Err(TerminalProcessError::Cancelled); }
        if count >= 32768 { return Err(TerminalProcessError::Limit); }
        let Ok(entry) = entry else { continue };
        let Some(pid) = entry.file_name().to_str().and_then(|s|s.parse::<u32>().ok()) else { continue };
        if let Some(value) = identity(pid) { parents.insert(pid,value); }
    }
    let mut found = Vec::new();
    for (&pid,&(_,started)) in &parents {
        if cancel.load(Ordering::Relaxed) { return Err(TerminalProcessError::Cancelled); }
        let mut ancestor = pid;
        for _ in 0..128 {
            if ancestor == root { break; }
            let Some(&(parent,_)) = parents.get(&ancestor) else { break };
            if parent == ancestor { break; }
            ancestor = parent;
        }
        if ancestor != root || pid == std::process::id() { continue; }
        let dir = std::path::PathBuf::from(format!("/proc/{pid}"));
        let Some(name) = limited(&dir.join("comm"), 256).map(|b|String::from_utf8_lossy(&b).trim().to_string()) else { continue };
        let Some(raw) = limited(&dir.join("cmdline"), 8192) else { continue };
        let Ok(exe) = std::fs::read_link(dir.join("exe")) else { continue };
        if identity(pid).is_none_or(|(_,now)| now != started) { continue; }
        found.push(TerminalProcess {process:Process {pid,name,exe:exe.file_name().map(|s|s.to_string_lossy().into_owned()).unwrap_or_default(),command:command_line(&raw)},started});
        if found.len() > 256 { return Err(TerminalProcessError::Limit); }
    }
    if identity(root) != Some(root_identity) { return Err(TerminalProcessError::Gone); }
    found.sort_by_key(|p|std::cmp::Reverse(p.process.pid));
    Ok(found)
}

pub fn validate_terminal_process(root: u32, selected: &TerminalProcess, cancel: &std::sync::atomic::AtomicBool) -> Result<(), TerminalProcessError> {
    terminal_processes(root,cancel)?.into_iter().any(|p|p == *selected).then_some(()).ok_or(TerminalProcessError::Gone)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joins_the_arguments() {
        assert_eq!(command_line(b"a\0--b\0c d\0"), "a --b c d");
        assert_eq!(command_line(b""), "");
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn lists_a_running_child_and_not_itself() {
        let mut child = std::process::Command::new("sleep").arg("30").spawn().expect("sleep");
        let is_child = |p: &Process| p.pid == child.id() && p.name == "sleep" && p.command == "sleep 30";
        // Right after the fork the child still runs as the test binary until it calls exec.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut found = list();
        while !found.iter().any(is_child) && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(20));
            found = list();
        }
        let seen = found.iter().any(is_child);
        let _ = child.kill();
        let _ = child.wait();
        assert!(seen, "{}", found.len());
        assert!(!found.iter().any(|p| p.pid == std::process::id()));
    }

    #[test]
    #[cfg(target_os="linux")]
    fn terminal_snapshot_excludes_unrelated_processes_and_rejects_reused_identity() {
        use std::sync::atomic::AtomicBool;
        let mut terminal = std::process::Command::new("sh").args(["-c","sleep 1 & wait"]).spawn().unwrap();
        let mut unrelated = std::process::Command::new("sleep").arg("1").spawn().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(30));
        let result = terminal_processes(terminal.id(),&AtomicBool::new(false)).unwrap();
        assert!(result.iter().any(|p|p.process.pid==terminal.id()));
        assert!(result.iter().any(|p|p.process.name=="sleep"));
        assert!(!result.iter().any(|p|p.process.pid==unrelated.id()));
        let mut selected = result[0].clone(); selected.started=selected.started.saturating_add(1);
        assert_eq!(validate_terminal_process(terminal.id(),&selected,&AtomicBool::new(false)),Err(TerminalProcessError::Gone));
        assert_eq!(terminal_processes(terminal.id(),&AtomicBool::new(true)),Err(TerminalProcessError::Cancelled));
        let _=terminal.wait();let _=unrelated.wait();
    }
}
