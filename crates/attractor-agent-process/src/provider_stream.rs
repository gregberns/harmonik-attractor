//! Stream a provider process's stdout into a Transcript as it is produced.
//!
//! Each raw stdout line (bytes up to and including `\n`, or a final line with
//! no `\n`) is appended to the Transcript and flushed before the next line is
//! read, so a reader tailing the file sees it grow while the provider runs.
//! The Transcript is the provider's stdout byte for byte; stderr is collected
//! separately and never written to it.
//!
//! A Transcript that cannot be created or written is logged and dropped: a
//! Transcript problem never fails the stage.

use std::path::{Path, PathBuf};
use std::process::ExitStatus;

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Child;

/// An append-only file written as output arrives. A write error logs a
/// warning and stops writing the file; it never fails the run.
struct LiveFile {
    /// What the file is, for log messages ("Transcript", "stderr file").
    kind: &'static str,
    path: PathBuf,
    file: Option<tokio::fs::File>,
}

impl LiveFile {
    /// Create an empty file at `path`, making its folder if needed.
    /// Returns `None` (after a warning) when the file cannot be created.
    async fn create(kind: &'static str, path: PathBuf) -> Option<Self> {
        let result = async {
            if let Some(dir) = path.parent() {
                tokio::fs::create_dir_all(dir).await?;
            }
            tokio::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .await
        }
        .await;
        match result {
            Ok(file) => Some(Self {
                kind,
                path,
                file: Some(file),
            }),
            Err(error) => {
                tracing::warn!(path = %path.display(), %error, "Cannot create {kind}");
                None
            }
        }
    }

    async fn discard(mut self) {
        self.file = None;
        if let Err(error) = tokio::fs::remove_file(&self.path).await {
            tracing::warn!(path = %self.path.display(), %error, "Cannot remove {}", self.kind);
        }
    }

    async fn append(&mut self, bytes: &[u8]) {
        let Some(file) = self.file.as_mut() else {
            return;
        };
        let result = async {
            file.write_all(bytes).await?;
            file.flush().await
        }
        .await;
        if let Err(error) = result {
            tracing::warn!(
                path = %self.path.display(),
                %error,
                "Cannot write {}; continuing without it",
                self.kind
            );
            self.file = None;
        }
    }
}

/// An open Transcript file (`transcripts/<invocation-id>.jsonl`).
pub struct Transcript(LiveFile);

impl Transcript {
    /// Create an empty Transcript at `path`, making its folder if needed.
    /// Returns `None` (after a warning) when the file cannot be created.
    pub async fn create(path: PathBuf) -> Option<Self> {
        LiveFile::create("Transcript", path).await.map(Self)
    }

    pub fn path(&self) -> &Path {
        &self.0.path
    }

    /// Remove the Transcript; used when the provider never started.
    pub async fn discard(self) {
        self.0.discard().await;
    }
}

/// An open stderr file (`transcripts/<invocation-id>.stderr.log`): the
/// process's stderr byte for byte, written as it arrives.
pub struct StderrLog(LiveFile);

impl StderrLog {
    /// Create an empty stderr file at `path`, making its folder if needed.
    /// Returns `None` (after a warning) when the file cannot be created.
    pub async fn create(path: PathBuf) -> Option<Self> {
        LiveFile::create("stderr file", path).await.map(Self)
    }

    pub fn path(&self) -> &Path {
        &self.0.path
    }
}

/// What the provider process produced.
#[derive(Debug)]
pub struct StreamedOutput {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// Wait for `child`, copying each stdout line to `transcript` and each
/// stderr chunk to `stderr_log` as it arrives.
///
/// stdout and stderr are drained concurrently so a provider that writes a lot
/// to stderr cannot block on a full pipe. The child is borrowed, not owned,
/// so the caller keeps the handle to signal and reap it if this future is
/// dropped (e.g. on timeout); everything already flushed stays in the files.
pub async fn run_streaming(
    child: &mut Child,
    mut transcript: Option<Transcript>,
    mut stderr_log: Option<StderrLog>,
) -> std::io::Result<StreamedOutput> {
    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();

    let read_stdout = async {
        let mut stdout = Vec::new();
        if let Some(pipe) = stdout_pipe {
            let mut reader = BufReader::new(pipe);
            let mut line = Vec::new();
            loop {
                line.clear();
                if reader.read_until(b'\n', &mut line).await? == 0 {
                    break;
                }
                if let Some(transcript) = transcript.as_mut() {
                    transcript.0.append(&line).await;
                }
                stdout.extend_from_slice(&line);
            }
        }
        Ok::<_, std::io::Error>(stdout)
    };
    let read_stderr = async {
        let mut stderr = Vec::new();
        if let Some(mut pipe) = stderr_pipe {
            let mut chunk = [0u8; 8192];
            loop {
                let n = pipe.read(&mut chunk).await?;
                let Some(bytes) = chunk.get(..n).filter(|bytes| !bytes.is_empty()) else {
                    break;
                };
                if let Some(log) = stderr_log.as_mut() {
                    log.0.append(bytes).await;
                }
                stderr.extend_from_slice(bytes);
            }
        }
        Ok::<_, std::io::Error>(stderr)
    };

    let (stdout, stderr, status) = tokio::try_join!(read_stdout, read_stderr, child.wait())?;
    Ok(StreamedOutput {
        status,
        stdout,
        stderr,
    })
}

#[cfg(all(test, unix))]
mod tests {
    use std::process::Stdio;

    use super::*;

    fn sh(script: &str) -> Child {
        tokio::process::Command::new("sh")
            .arg("-c")
            .arg(script)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap()
    }

    #[tokio::test]
    async fn transcript_equals_stdout_including_unterminated_last_line() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("t").join("x.jsonl");
        let transcript = Transcript::create(path.clone()).await.unwrap();
        let out = run_streaming(
            &mut sh(r"printf 'a\r\n\303\251\tb\n\377\nlast'; printf 'err' >&2"),
            Some(transcript),
            None,
        )
        .await
        .unwrap();
        let expected = b"a\r\n\xc3\xa9\tb\n\xff\nlast".to_vec();
        assert!(out.status.success());
        assert_eq!(out.stdout, expected);
        assert_eq!(out.stderr, b"err");
        assert_eq!(std::fs::read(&path).unwrap(), expected);
    }

    #[tokio::test]
    async fn large_stderr_does_not_block() {
        let out = run_streaming(
            &mut sh("head -c 1048576 /dev/zero >&2; echo done; exit 4"),
            None,
            None,
        )
        .await
        .unwrap();
        assert_eq!(out.status.code(), Some(4));
        assert_eq!(out.stdout, b"done\n");
        assert_eq!(out.stderr.len(), 1_048_576);
    }

    #[tokio::test]
    async fn create_fails_softly_and_does_not_overwrite() {
        let tmp = tempfile::tempdir().unwrap();
        let existing = tmp.path().join("x.jsonl");
        std::fs::write(&existing, "keep").unwrap();
        assert!(Transcript::create(existing.clone()).await.is_none());
        assert_eq!(std::fs::read(&existing).unwrap(), b"keep");

        let blocker = tmp.path().join("file");
        std::fs::write(&blocker, "").unwrap();
        assert!(Transcript::create(blocker.join("y.jsonl")).await.is_none());
    }

    #[tokio::test]
    async fn discard_removes_the_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("x.jsonl");
        let transcript = Transcript::create(path.clone()).await.unwrap();
        assert!(path.exists());
        transcript.discard().await;
        assert!(!path.exists());
    }
}
