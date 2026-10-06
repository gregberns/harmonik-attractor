//! `run_local`: the child gets exactly the given env, workdir and a null
//! stdin; its stdout streams to the Transcript and its stderr to the stderr
//! file; a timeout or a cancel sends TERM, waits the grace, then KILLs its
//! group; a dropped future KILLs its group.
#![cfg(unix)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use attractor_agent_process::{run_local, CancellationToken, LocalRun, Spawn, StreamedOutput};

const LONG: Duration = Duration::from_secs(30);
const GRACE: Duration = Duration::from_secs(10);

/// `run_local` with no stderr file, the default grace, no cancel and no
/// spawn report: the shape the tests written before 02b need.
async fn run(
    argv: &[String],
    env: &BTreeMap<String, String>,
    workdir: &Path,
    transcript: Option<&Path>,
    timeout: Duration,
) -> LocalRun {
    run_local(
        argv,
        env,
        workdir,
        transcript,
        None,
        timeout,
        GRACE,
        &CancellationToken::new(),
        &|_| {},
    )
    .await
}

/// Poll `cond` with a bound; the run under test makes progress meanwhile.
async fn poll_until(what: &str, mut cond: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !cond() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn read_pid(path: &Path) -> i32 {
    std::fs::read_to_string(path)
        .unwrap()
        .trim()
        .parse()
        .unwrap()
}

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
        LocalRun::Cancelled => panic!("unexpected cancel"),
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
    let out = exited(run(&argv(&["/usr/bin/env"]), &vars, tmp.path(), None, LONG).await);
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
        run(
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
        run(
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
        run(
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
    let stderr = tmp.path().join("transcripts").join("x.stderr.log");
    let calls = Mutex::new(0);
    let run = run_local(
        &[program.to_string_lossy().into_owned()],
        &path_env(),
        tmp.path(),
        Some(&transcript),
        Some(&stderr),
        LONG,
        GRACE,
        &CancellationToken::new(),
        &|_| *calls.lock().unwrap() += 1,
    )
    .await;
    match run {
        LocalRun::LaunchFailed(e) => assert_eq!(e.kind(), std::io::ErrorKind::NotFound),
        _ => panic!("expected LaunchFailed"),
    }
    assert!(!transcript.exists());
    assert!(!stderr.exists());
    assert!(!transcript.parent().is_some_and(Path::exists));
    assert_eq!(
        *calls.lock().unwrap(),
        0,
        "spawned called without a process"
    );
}

#[tokio::test]
async fn empty_argv_is_launch_failed() {
    let tmp = tempfile::tempdir().unwrap();
    match run(&[], &path_env(), tmp.path(), None, LONG).await {
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
    let run = run(
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
    let run = run(
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
        run(
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

#[tokio::test]
async fn stderr_is_written_live_and_kept_in_memory() {
    let tmp = tempfile::tempdir().unwrap();
    let stderr = tmp.path().join("transcripts").join("x.stderr.log");
    let go = tmp.path().join("go");
    // The child writes one stderr line, then waits (bounded) for `go`.
    let script = r#"echo working >&2
i=0
while [ ! -f go ] && [ $i -lt 1000 ]; do sleep 0.01; i=$((i+1)); done
echo done >&2"#;
    let cancel = CancellationToken::new();
    let argv = argv(&["/bin/sh", "-c", script]);
    let vars = path_env();
    let running = run_local(
        &argv,
        &vars,
        tmp.path(),
        None,
        Some(&stderr),
        LONG,
        GRACE,
        &cancel,
        &|_| {},
    );
    let release = async {
        poll_until("the first stderr line in the file", || {
            std::fs::read(&stderr).is_ok_and(|b| b == b"working\n")
        })
        .await;
        std::fs::write(&go, "").unwrap();
    };
    let (run, ()) = tokio::join!(running, release);
    let out = exited(run);
    assert!(out.status.success());
    assert_eq!(out.stderr, b"working\ndone\n");
    assert_eq!(std::fs::read(&stderr).unwrap(), out.stderr);
}

#[tokio::test]
async fn spawned_is_called_once_with_the_pid_before_any_output() {
    let tmp = tempfile::tempdir().unwrap();
    let transcript = tmp.path().join("transcripts").join("x.jsonl");
    let stderr = tmp.path().join("transcripts").join("x.stderr.log");
    // (spawn, transcript length, stderr file exists) at each call.
    let calls: Mutex<Vec<(Spawn, Option<u64>, bool)>> = Mutex::new(Vec::new());
    let out = exited(
        run_local(
            &argv(&["/bin/sh", "-c", "echo $$; echo err >&2"]),
            &path_env(),
            tmp.path(),
            Some(&transcript),
            Some(&stderr),
            LONG,
            GRACE,
            &CancellationToken::new(),
            &|spawn| {
                let len = std::fs::metadata(&transcript).ok().map(|m| m.len());
                calls.lock().unwrap().push((spawn, len, stderr.exists()));
            },
        )
        .await,
    );
    let calls = calls.into_inner().unwrap();
    assert_eq!(calls.len(), 1);
    let (spawn, transcript_len, stderr_existed) = &calls[0];
    let pid: u32 = String::from_utf8(out.stdout)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert_eq!(spawn.pid, pid);
    assert_eq!(spawn.pgid, pid);
    assert_eq!(spawn.host, attractor_agent_process::host_name());
    assert_eq!(
        *transcript_len,
        Some(0),
        "transcript must exist and be empty"
    );
    assert!(stderr_existed, "stderr file must exist");
    assert_eq!(
        std::fs::read(&transcript).unwrap(),
        format!("{pid}\n").as_bytes()
    );
}

#[test]
fn host_name_is_the_machine_name() {
    let out = std::process::Command::new("hostname").output().unwrap();
    let expected = String::from_utf8(out.stdout).unwrap();
    assert_eq!(
        attractor_agent_process::host_name().as_deref(),
        Some(expected.trim())
    );
}

/// A shell that writes `term` to `marker` on TERM and then exits; it writes
/// its pid to `pid` once the trap is set.
const HONOURS_TERM: &str =
    r#"trap 'echo term > marker; exit 143' TERM; echo $$ > pid; sleep 30 & wait $!"#;

/// A shell that writes `term` to `marker` on TERM and keeps going, so only
/// a KILL ends it.
const IGNORES_TERM: &str = r#"trap 'echo term >> marker' TERM; echo $$ > pid
i=0
while [ $i -lt 60 ]; do sleep 1 & wait $!; i=$((i+1)); done"#;

#[tokio::test]
async fn timeout_sends_term_first_and_is_timed_out() {
    let tmp = tempfile::tempdir().unwrap();
    let run = run_local(
        &argv(&["/bin/sh", "-c", HONOURS_TERM]),
        &path_env(),
        tmp.path(),
        None,
        None,
        Duration::from_secs(1),
        GRACE,
        &CancellationToken::new(),
        &|_| {},
    )
    .await;
    assert!(matches!(run, LocalRun::TimedOut), "got {run:?}");
    assert_eq!(
        std::fs::read_to_string(tmp.path().join("marker")).unwrap(),
        "term\n"
    );
}

#[tokio::test]
async fn timeout_kills_after_the_grace_when_term_is_ignored() {
    let tmp = tempfile::tempdir().unwrap();
    let started = Instant::now();
    let run = run_local(
        &argv(&["/bin/sh", "-c", IGNORES_TERM]),
        &path_env(),
        tmp.path(),
        None,
        None,
        Duration::from_secs(1),
        Duration::from_millis(300),
        &CancellationToken::new(),
        &|_| {},
    )
    .await;
    assert!(matches!(run, LocalRun::TimedOut), "got {run:?}");
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "grace not bounded"
    );
    assert!(std::fs::read_to_string(tmp.path().join("marker"))
        .unwrap()
        .starts_with("term\n"));
    let pid = read_pid(&tmp.path().join("pid"));
    // SAFETY: signal 0 only checks that `pid` exists; nothing is sent.
    let alive = unsafe { libc::kill(pid, 0) } == 0;
    assert!(!alive, "child {pid} was not reaped");
}

#[tokio::test]
async fn cancel_sends_term_first_and_is_cancelled() {
    let tmp = tempfile::tempdir().unwrap();
    let pid_file = tmp.path().join("pid");
    let cancel = CancellationToken::new();
    let argv = argv(&["/bin/sh", "-c", HONOURS_TERM]);
    let vars = path_env();
    let running = run_local(
        &argv,
        &vars,
        tmp.path(),
        None,
        None,
        LONG,
        GRACE,
        &cancel,
        &|_| {},
    );
    let stop = async {
        poll_until("the child's pid file", || pid_file.exists()).await;
        cancel.cancel();
    };
    let (run, ()) = tokio::join!(running, stop);
    assert!(matches!(run, LocalRun::Cancelled), "got {run:?}");
    assert_eq!(
        std::fs::read_to_string(tmp.path().join("marker")).unwrap(),
        "term\n"
    );
}

#[tokio::test]
async fn dropping_the_future_kills_the_group() {
    let tmp = tempfile::tempdir().unwrap();
    let pids: PathBuf = tmp.path().join("pids");
    let argv = argv(&[
        "/bin/sh",
        "-c",
        "sleep 30 & echo $$ $! > pids.tmp; mv pids.tmp pids; wait",
    ]);
    let vars = path_env();
    let cancel = CancellationToken::new();
    {
        let running = run_local(
            &argv,
            &vars,
            tmp.path(),
            None,
            None,
            LONG,
            GRACE,
            &cancel,
            &|_| {},
        );
        tokio::pin!(running);
        tokio::select! {
            run = &mut running => panic!("run ended early: {run:?}"),
            () = poll_until("the pids file", || pids.exists()) => {}
        }
    }
    let pids = std::fs::read_to_string(&pids).unwrap();
    let pids: Vec<&str> = pids.split_whitespace().collect();
    assert_eq!(pids.len(), 2, "pids: {pids:?}");
    for pid in pids {
        assert!(wait_dead(pid), "process {pid} still alive");
    }
}
