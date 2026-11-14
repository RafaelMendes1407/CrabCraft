//! Debugging Rust programs through the Debug Adapter Protocol with GDB or LLDB.
//!
//! - [`adapter`]: command for an installed Rust adapter.
//! - [`client`]: the DAP client over stdio, non-blocking ([`Pending`] polled per frame).
//! - [`session`]: [`DebugSession`], the state machine the UI owns: build, launch/attach,
//!   handshake, breakpoints, stops, frames, variables, watches, evaluation.
//! - [`launch`]: what to debug ([`DebugTarget`]) and the adapter's launch arguments.
//! - [`breakpoints`]: breakpoints by file and line, hit conditions and logpoints.
//! - [`format`]: integers in hexadecimal for adapters that cannot print them so.
//! - [`memory`]: `readMemory` answers decoded and laid out as a hex dump.
//! - [`disassembly`]: `disassemble` answers decoded, merged and laid out as rows.

pub mod adapter;
pub mod breakpoints;
pub mod client;
pub mod disassembly;
pub mod format;
pub mod gdb;
pub mod launch;
pub mod lldb;
pub mod memory;
pub mod protocol;
pub mod remote;
pub mod session;

pub use adapter::AdapterCommand;
pub use breakpoints::{BreakpointSpec, BreakpointState, Breakpoints};
pub use client::{DapClient, DapError, Incoming, Pending};
pub use launch::{DebugOptions, DebugTarget, LaunchConfig};
pub use memory::MemoryBlock;
pub use protocol::{EvalContext, ExceptionInfo, Scope, StackFrame, Thread, Variable};
pub use session::{
    DebugEvent, DebugSession, EvalValue, Failure, GotoError, OutputCategory, ParallelStack, SessionState, SetMethod, StopInfo, StopReason, ThreadFrames, Watch,
    WatchValue, set_method,
};
