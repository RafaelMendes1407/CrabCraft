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
}
