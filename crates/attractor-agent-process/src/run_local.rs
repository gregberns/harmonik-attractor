//! Run one local process to its end, or until a timeout or a cancel stops it.

use std::collections::BTreeMap;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::process::Stdio;
use std::time::Duration;

use tokio::process::Child;
use tokio_util::sync::CancellationToken;

use crate::process_group::{self, ProcessGroupGuard};
use crate::provider_stream::{run_streaming, StderrLog, StreamedOutput, Transcript};

/// How long to wait for a SIGKILLed child to be reaped.
const REAP_BOUND: Duration = Duration::from_secs(5);

/// How a local process run ended.
#[derive(Debug)]
pub enum LocalRun {
    /// The process ran to its end; stdout/stderr/status as run_streaming collected them.
    Exited(StreamedOutput),
    /// The timeout fired; the group got TERM, then KILL after the grace.
    /// Reported even when the process exits cleanly on TERM.
    TimedOut,
    /// The cancel token fired; the group got TERM, then KILL after the grace.
    Cancelled,
    /// The process could not be started.
    LaunchFailed(std::io::Error),
    /// The process started, but reading its output or waiting for it failed;
    /// the process group was killed.
    WaitFailed(std::io::Error),
}

/// A started process, reported once right after it is spawned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spawn {
    pub pid: u32,
    /// The process group; the process leads its own group, so this is `pid`.
    pub pgid: u32,
    /// This machine's host name, when it can be read.
    pub host: Option<String>,
}

/// Why the run stopped before the process ended on its own.
enum Trigger {
    Ended(std::io::Result<StreamedOutput>),
    Timeout,
    Cancel,
}

/// Run `argv` in `workdir` with exactly `env`, stdin from `/dev/null`, in
/// its own process group.
///
/// Once the process has started (and only then) the `transcript` and
/// `stderr` files are created, `spawned` is called once, synchronously,
/// and then stdout streams to the transcript and stderr to the stderr file.
/// So `spawned` runs after both files exist and before any output is in
/// them.
///
/// A timeout or a cancel stops the group: SIGTERM, up to `kill_grace` for
/// the process to end, then SIGKILL, and the process is reaped. A dropped
/// future SIGKILLs the group (it cannot wait or reap).
#[allow(clippy::too_many_arguments)]
pub async fn run_local(
    argv: &[String],
    env: &BTreeMap<String, String>,
    workdir: &Path,
    transcript: Option<&Path>,
    stderr: Option<&Path>,
    timeout: Duration,
    kill_grace: Duration,
    cancel: &CancellationToken,
    spawned: &(dyn Fn(Spawn) + Sync),
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

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => return LocalRun::LaunchFailed(error),
    };
    // A child that has not been waited on always has an id.
    let Some(pid) = child.id() else {
        return LocalRun::WaitFailed(std::io::Error::other("started process has no pid"));
    };
    // Armed from here on: a wait error or a dropped future kills the whole
    // group; `terminate` disarms it once the process is reaped.
    let mut group = ProcessGroupGuard::new(Some(pid));
    let transcript = match transcript {
        Some(path) => Transcript::create(path.to_path_buf()).await,
        None => None,
    };
    let stderr_log = match stderr {
        Some(path) => StderrLog::create(path.to_path_buf()).await,
        None => None,
    };
    spawned(Spawn {
        pid,
        pgid: pid,
        host: host_name(),
    });

    let stopped = {
        let streaming = run_streaming(&mut child, transcript, stderr_log);
        tokio::pin!(streaming);
        let trigger = tokio::select! {
            biased;
            ended = &mut streaming => Trigger::Ended(ended),
            () = cancel.cancelled() => Trigger::Cancel,
            () = tokio::time::sleep(timeout) => Trigger::Timeout,
        };
        let stopped = match trigger {
            Trigger::Ended(Ok(output)) => {
                group.disarm();
                return LocalRun::Exited(output);
            }
            Trigger::Ended(Err(error)) => return LocalRun::WaitFailed(error),
            Trigger::Timeout => LocalRun::TimedOut,
            Trigger::Cancel => LocalRun::Cancelled,
        };
        terminate(pid, streaming, kill_grace).await;
        stopped
    };
    reap(&mut child, &mut group).await;
    stopped
}

/// The one stop for a timeout and a cancel: SIGTERM the group, keep
/// awaiting the same streaming future for up to `kill_grace`, and SIGKILL
/// the group if it has not finished. The caller then reaps the process with
/// [`reap`] (the streaming future borrows the child until it is dropped).
async fn terminate<F>(pgid: u32, mut streaming: Pin<&mut F>, kill_grace: Duration)
where
    F: Future<Output = std::io::Result<StreamedOutput>>,
{
    signal_group(pgid, Signal::Term);
    match tokio::time::timeout(kill_grace, streaming.as_mut()).await {
        // The process ended and was waited on: nothing left to kill.
        Ok(Ok(_)) => {}
        // Not reaped yet, so its pid (and the group id) can't have been
        // reused: KILL is safe.
        Ok(Err(_)) | Err(_) => signal_group(pgid, Signal::Kill),
    }
}

/// Wait (bounded) for the stopped process so no zombie stays, and disarm
/// the guard once it is reaped. A process already waited on returns its
/// status at once.
async fn reap(child: &mut Child, group: &mut ProcessGroupGuard) {
    match tokio::time::timeout(REAP_BOUND, child.wait()).await {
        Ok(Ok(_)) => group.disarm(),
        Ok(Err(error)) => tracing::warn!(%error, "Cannot reap stopped process"),
        Err(_) => tracing::warn!(
            bound_ms = REAP_BOUND.as_millis() as u64,
            "Stopped process was not reaped in time"
        ),
    }
}

#[derive(Clone, Copy)]
enum Signal {
    Term,
    Kill,
}

/// Send `signal` to the process group `pgid`; best-effort.
fn signal_group(pgid: u32, signal: Signal) {
    #[cfg(unix)]
    {
        let Ok(pgid) = libc::pid_t::try_from(pgid) else {
            return;
        };
        let signal = match signal {
            Signal::Term => libc::SIGTERM,
            Signal::Kill => libc::SIGKILL,
        };
        // SAFETY: the child was placed in a new process group whose ID is
        // its PID, and it has not been reaped, so the ID still names that
        // group. Delivery is best-effort; the group may already be gone.
        unsafe {
            libc::killpg(pgid, signal);
        }
    }

    #[cfg(not(unix))]
    {
        let _ = (pgid, signal);
    }
}

/// This machine's host name, or `None` when it can't be read, is
/// truncated, is empty or is not UTF-8.
pub fn host_name() -> Option<String> {
    #[cfg(unix)]
    {
        let mut buf = [0u8; 256];
        // SAFETY: `buf` is a live, writable buffer of `buf.len()` bytes, and
        // gethostname writes at most that many bytes into it.
        let rc = unsafe { libc::gethostname(buf.as_mut_ptr().cast::<libc::c_char>(), buf.len()) };
        if rc != 0 {
            return None;
        }
        // No NUL means the name may have been truncated.
        let end = buf.iter().position(|&b| b == 0)?;
        let name = std::str::from_utf8(buf.get(..end)?).ok()?;
        (!name.is_empty()).then(|| name.to_owned())
    }

    #[cfg(not(unix))]
    {
        None
    }
}
