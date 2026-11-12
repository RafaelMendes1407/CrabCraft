//! Rust debug targets and DAP launch arguments for GDB and LLDB.

use rc_dotnet::LaunchPlan;
use serde_json::{Map, Value, json};
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaunchConfig {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: Vec<(String, String)>,
}

impl From<&LaunchPlan> for LaunchConfig {
    fn from(plan: &LaunchPlan) -> Self {
        Self { program: plan.program.clone(), args: plan.args.clone(), cwd: plan.working_dir.clone(), env: plan.env.clone() }
    }
}

#[derive(Clone, Debug)]
pub enum DebugTarget {
    Launch(LaunchConfig),
    Attach { pid: u32 },
    Remote(crate::remote::RemoteTarget),
    Cargo { cargo: rc_cargo::Cargo, target: rc_cargo::RunTarget },
}

impl DebugTarget {
    pub fn is_attach(&self) -> bool {
        matches!(self, Self::Attach { .. } | Self::Remote(_))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DebugOptions {
    pub stop_at_entry: bool,
    pub exception_filters: Option<Vec<String>>,
    pub top_frames: i64,
}

impl Default for DebugOptions {
    fn default() -> Self {
        Self { stop_at_entry: false, exception_filters: None, top_frames: 20 }
    }
}

pub fn gdb_launch_arguments(config: &LaunchConfig, options: &DebugOptions) -> Value {
    let env: Map<String, Value> = config.env.iter().map(|(key, value)| (key.clone(), Value::String(value.clone()))).collect();
    json!({
        "program": config.program,
        "args": config.args,
        "cwd": config.cwd,
        "env": env,
        "stopOnEntry": options.stop_at_entry,
    })
}

pub fn gdb_attach_arguments(pid: u32) -> Value {
    json!({ "pid": pid })
}

pub fn lldb_launch_arguments(config: &LaunchConfig, options: &DebugOptions) -> Value {
    let mut args = gdb_launch_arguments(config, options);
    args["type"] = json!("lldb");
    args["request"] = json!("launch");
    args["initCommands"] = json!(["settings set target.disable-aslr false"]);
    args
}
