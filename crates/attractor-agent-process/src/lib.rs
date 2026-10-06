//! Shared local-process runner for agent and tool processes.
//!
//! Spawns a child in its own process group, streams its stdout into a
//! Transcript, and kills the whole group on timeout or cancellation.

pub mod process_group;
pub mod provider_stream;
mod run_local;

pub use process_group::ProcessGroupGuard;
pub use provider_stream::{run_streaming, StreamedOutput, Transcript};
pub use run_local::{run_local, LocalRun};
