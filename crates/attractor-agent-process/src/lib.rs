//! Shared local-process runner for agent and tool processes.
//!
//! Spawns a child in its own process group, streams its stdout into a
//! Transcript and its stderr into a stderr file, and stops the whole group
//! (TERM, grace, KILL) on timeout or cancellation.

pub mod process_group;
pub mod provider_stream;
mod run_local;
pub mod stderr_tail;

pub use process_group::ProcessGroupGuard;
pub use provider_stream::{run_streaming, StderrLog, StreamedOutput, Transcript};
pub use run_local::{host_name, run_local, LocalRun, Spawn};
pub use stderr_tail::stderr_tail;
pub use tokio_util::sync::CancellationToken;
