//! Validated TCP endpoint for an explicit GDB remote connection (never a shell command).
use std::{net::IpAddr, path::PathBuf};

#[derive(Clone, Debug)]
pub struct RemoteTarget { endpoint: String, program: PathBuf }

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteError { Host, Port, Program }
impl std::fmt::Display for RemoteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self { Self::Host => "invalid remote host", Self::Port => "invalid remote port", Self::Program => "invalid local symbol program path" })
    }
}
impl std::error::Error for RemoteError {}

impl RemoteTarget {
    pub fn new(host: &str, port: u16, program: PathBuf) -> Result<Self, RemoteError> {
        if port == 0 { return Err(RemoteError::Port); }
        if host.is_empty() || host.len() > 253 || (!host.parse::<IpAddr>().is_ok() &&
            !host.split('.').all(|part| !part.is_empty() && part.len() <= 63 && !part.starts_with('-') && !part.ends_with('-') && part.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-'))) { return Err(RemoteError::Host); }
        let path = program.to_str().ok_or(RemoteError::Program)?;
        if !program.is_absolute() || path.chars().any(|c| c.is_control()) { return Err(RemoteError::Program); }
        let endpoint = if host.contains(':') { format!("[{host}]:{port}") } else { format!("{host}:{port}") };
        Ok(Self { endpoint, program })
    }
    pub fn endpoint(&self) -> &str { &self.endpoint }
    pub fn arguments(&self) -> serde_json::Value { serde_json::json!({ "target": self.endpoint, "program": self.program }) }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn remote_endpoints_reject_gdb_command_injection_and_zero_ports() {
        let program = std::env::current_dir().unwrap().join("symbols with spaces");
        for host in ["|sh", "localhost\nquit", "host;quit", "-option", "a..b", "host:1234", ""] {
            assert_eq!(RemoteTarget::new(host, 1234, program.clone()).unwrap_err(), RemoteError::Host);
        }
        assert_eq!(RemoteTarget::new("127.0.0.1", 0, program.clone()).unwrap_err(), RemoteError::Port);
        assert!(RemoteTarget::new("::1", 1234, program.clone()).unwrap().endpoint().contains("[::1]:1234"));
        assert!(RemoteTarget::new("localhost", 1234, "relative".into()).is_err());
        assert!(RemoteTarget::new("localhost", 1234, program).is_ok());
    }
}
