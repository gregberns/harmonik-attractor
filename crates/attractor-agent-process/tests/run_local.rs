//! `run_local`: the child gets exactly the given env, workdir and a null
//! stdin; its stdout streams to the Transcript; a timeout kills its group.
#![cfg(unix)]

use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, Instant};

use attractor_agent_process::{run_local, LocalRun, StreamedOutput};

const LONG: Duration = Duration::from_secs(30);

fn argv(args: &[&str]) -> Vec<String> {
    args.iter().map(|a| a.to_string()).collect()
}

fn env(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn path_env() -> BTreeMap<String, String> {
    env(&[("PATH", "/usr/bin:/bin")])
}

fn exited(run: LocalRun) -> StreamedOutput {
    match run {
        LocalRun::Exited(out) => out,
        LocalRun::TimedOut => panic!("unexpected timeout"),
        LocalRun::LaunchFailed(e) => panic!("unexpected launch failure: {e}"),
        LocalRun::WaitFailed(e) => panic!("unexpected wait failure: {e}"),
    }
}

/// True once `pid` is gone or a zombie (killed, not yet reaped).
fn is_dead(pid: &str) -> bool {
    let out = std::process::Command::new("ps")
        .args(["-o", "stat=", "-p", pid])
        .output()
        .unwrap();
    let stat = String::from_utf8_lossy(&out.stdout);
    let stat = stat.trim();
    stat.is_empty() || stat.starts_with('Z')
}

/// SIGKILL delivery to another process is asynchronous, and an orphaned
/// grandchild is reaped by init; nothing here can be waited on, so poll
/// with a bounded deadline.
fn wait_dead(pid: &str) -> bool {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if is_dead(pid) {
            return true;
        }
        std::thread::yield_now();
    }
    is_dead(pid)
}

#[tokio::test]
async fn env_is_exactly_the_given_map() {
    let tmp = tempfile::tempdir().unwrap();
    let vars = env(&[("PAS_TEST_VAR", "hello world"), ("PATH", "/usr/bin:/bin")]);
    let out = exited(run_local(&argv(&["/usr/bin/env"]), &vars, tmp.path(), None, LONG).await);
    assert!(out.status.success());
    let stdout = String::from_utf8(out.stdout).unwrap();
    let mut lines: Vec<&str> = stdout.lines().collect();
    lines.sort();
    assert_eq!(lines, ["PAS_TEST_VAR=hello world", "PATH=/usr/bin:/bin"]);
    assert!(!stdout.contains("HOME="));
}

#[tokio::test]
async fn cwd_is_workdir() {
    let tmp = tempfile::tempdir().unwrap();
    let out = exited(
        run_local(
            &argv(&["/bin/sh", "-c", "pwd -P"]),
            &path_env(),
            tmp.path(),
            None,
            LONG,
        )
        .await,
    );
    let expected = tmp.path().canonicalize().unwrap();
    assert_eq!(
        String::from_utf8(out.stdout).unwrap().trim_end(),
        expected.to_str().unwrap()
    );
}

#[tokio::test]
async fn stdin_is_null() {
    let tmp = tempfile::tempdir().unwrap();
    let out = exited(
        run_local(
            &argv(&["/bin/cat"]),
            &path_env(),
            tmp.path(),
            None,
            Duration::from_secs(10),
        )
        .await,
    );
    assert!(out.status.success());
    assert!(out.stdout.is_empty());
}

#[tokio::test]
async fn transcript_equals_stdout() {
    let tmp = tempfile::tempdir().unwrap();
    let transcript = tmp.path().join("transcripts").join("x.jsonl");
    let out = exited(
        run_local(
            &argv(&[
                "/bin/sh",
                "-c",
                r#"printf '{"a":1}\n\303\251\377\nlast'; printf 'err' >&2"#,
            ]),
            &path_env(),
            tmp.path(),
            Some(&transcript),
            LONG,
        )
        .await,
    );
    assert_eq!(out.stdout, b"{\"a\":1}\n\xc3\xa9\xff\nlast");
    assert_eq!(out.stderr, b"err");
    assert_eq!(std::fs::read(&transcript).unwrap(), out.stdout);
}

#[tokio::test]
async fn missing_program_is_launch_failed_without_transcript() {
    let tmp = tempfile::tempdir().unwrap();
    let transcript = tmp.path().join("transcripts").join("x.jsonl");
    let program = tmp.path().join("no-such-program");
    let run = run_local(
        &[program.to_string_lossy().into_owned()],
        &path_env(),
        tmp.path(),
        Some(&transcript),
        LONG,
    )
    .await;
    match run {
        LocalRun::LaunchFailed(e) => assert_eq!(e.kind(), std::io::ErrorKind::NotFound),
        _ => panic!("expected LaunchFailed"),
    }
    assert!(!transcript.exists());
    assert!(!transcript.parent().is_some_and(Path::exists));
}

#[tokio::test]
async fn empty_argv_is_launch_failed() {
    let tmp = tempfile::tempdir().unwrap();
    match run_local(&[], &path_env(), tmp.path(), None, LONG).await {
        LocalRun::LaunchFailed(e) => assert_eq!(e.kind(), std::io::ErrorKind::InvalidInput),
        _ => panic!("expected LaunchFailed"),
    }
}

#[tokio::test]
async fn timeout_kills_the_process_group() {
    let tmp = tempfile::tempdir().unwrap();
    let transcript = tmp.path().join("t.jsonl");
    // The shell prints its own pid and a background sleeper's pid; the
    // sleeper only dies if the whole group is killed.
    let run = run_local(
        &argv(&["/bin/sh", "-c", "sleep 30 & echo $$ $!; wait"]),
        &path_env(),
        tmp.path(),
        Some(&transcript),
        Duration::from_millis(300),
    )
    .await;
    assert!(matches!(run, LocalRun::TimedOut));
    let pids = std::fs::read_to_string(&transcript).unwrap();
    let pids: Vec<&str> = pids.split_whitespace().collect();
    assert_eq!(pids.len(), 2, "pids: {pids:?}");
    for pid in pids {
        assert!(wait_dead(pid), "process {pid} still alive");
    }
}

#[tokio::test]
async fn exec_sleeper_is_gone_after_timeout() {
    let tmp = tempfile::tempdir().unwrap();
    let transcript = tmp.path().join("t.jsonl");
    let run = run_local(
        &argv(&["/bin/sh", "-c", "echo $$; exec sleep 30"]),
        &path_env(),
        tmp.path(),
        Some(&transcript),
        Duration::from_millis(300),
    )
    .await;
    assert!(matches!(run, LocalRun::TimedOut));
    let pid = std::fs::read_to_string(&transcript).unwrap();
    assert!(wait_dead(pid.trim()), "sleeper {pid} still alive");
}

#[tokio::test]
async fn non_zero_exit_is_exited_with_stderr() {
    let tmp = tempfile::tempdir().unwrap();
    let out = exited(
        run_local(
            &argv(&["/bin/sh", "-c", "echo partial; echo oops >&2; exit 3"]),
            &path_env(),
            tmp.path(),
            None,
            LONG,
        )
        .await,
    );
    assert_eq!(out.status.code(), Some(3));
    assert_eq!(out.stdout, b"partial\n");
    assert_eq!(out.stderr, b"oops\n");
}
