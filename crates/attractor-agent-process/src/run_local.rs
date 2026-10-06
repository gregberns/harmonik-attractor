//! Run one local process to its end, or until a timeout kills its group.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use crate::process_group::{self, ProcessGroupGuard};
use crate::provider_stream::{run_streaming, StreamedOutput, Transcript};

/// How a local process run ended.
#[derive(Debug)]
pub enum LocalRun {
    /// The process ran to its end; stdout/stderr/status as run_streaming collected them.
    Exited(StreamedOutput),
    /// The timeout fired; the process group was killed (SIGKILL by the drop guard).
    TimedOut,
    /// The process could not be started.
    LaunchFailed(std::io::Error),
    /// The process started, but reading its output or waiting for it failed;
    /// the process group was killed.
    WaitFailed(std::io::Error),
}

/// Run `argv` in `workdir` with exactly `env`, stdin from `/dev/null`, in
/// its own process group. stdout streams to `transcript` (created only once
/// the process has started); a timeout or a dropped future kills the group.
pub async fn run_local(
    argv: &[String],
    env: &BTreeMap<String, String>,
    workdir: &Path,
    transcript: Option<&Path>,
    timeout: Duration,
) -> LocalRun {
    let Some((program, args)) = argv.split_first() else {
        return LocalRun::LaunchFailed(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "empty argv",
        ));
    };
    let mut command = tokio::process::Command::new(program);
    command
        .args(args)
        .env_clear()
        .envs(env)
        .current_dir(workdir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    process_group::configure(&mut command);

    let child = match command.spawn() {
        Ok(child) => child,
        Err(error) => return LocalRun::LaunchFailed(error),
    };
    // Armed from here on: a timeout, a wait error or a dropped future kills
    // the whole group.
    let mut group = ProcessGroupGuard::new(child.id());
    let transcript = match transcript {
        Some(path) => Transcript::create(path.to_path_buf()).await,
        None => None,
    };
    match tokio::time::timeout(timeout, run_streaming(child, transcript)).await {
        Ok(Ok(output)) => {
            group.disarm();
            LocalRun::Exited(output)
        }
        Ok(Err(error)) => LocalRun::WaitFailed(error),
        Err(_elapsed) => LocalRun::TimedOut,
    }
}
