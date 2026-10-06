//! Runs `pas run` against the fake `claude` (`tests/agents/fake-claude`).
//!
//! Each [`FakeAgent`] owns a scratch folder: a git repo with one commit used
//! as the workdir, a `bin/` holding the fake as `claude` (first on `PATH`),
//! the fake's scenario folder, and the Run's logs and state folders. Git
//! ignores the developer's global and system config (hooks, templates).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;

/// The fake script in the repo's `tests/agents/`.
fn fake_claude() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/agents/fake-claude")
}

pub struct FakeAgent {
    dir: tempfile::TempDir,
}

impl FakeAgent {
    pub fn new() -> Self {
        let fake = Self {
            dir: tempfile::tempdir().unwrap(),
        };
        for dir in [fake.bin(), fake.scenarios(), fake.repo()] {
            fs::create_dir_all(dir).unwrap();
        }
        std::os::unix::fs::symlink(fake_claude(), fake.bin().join("claude")).unwrap();
        fake.git(&["init", "-q"]);
        fake.git(&["commit", "-q", "--allow-empty", "-m", "base"]);
        fake
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn bin(&self) -> PathBuf {
        self.path().join("bin")
    }

    /// `FAKE_AGENT_SCENARIOS`: where the fake keeps its state.
    pub fn scenarios(&self) -> PathBuf {
        self.path().join("scenarios")
    }

    /// The Run's workdir: a git repo with one commit.
    pub fn repo(&self) -> PathBuf {
        self.path().join("repo")
    }

    fn logs(&self) -> PathBuf {
        self.path().join("logs")
    }

    /// `git <args>` in the repo as a fixed test user; returns trimmed stdout.
    pub fn git(&self, args: &[&str]) -> String {
        let output = Command::new("git")
            .args([
                "-c",
                "user.name=pas-test",
                "-c",
                "user.email=pas-test@example.invalid",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .current_dir(self.repo())
            .env("GIT_CEILING_DIRECTORIES", self.path())
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .output()
            .unwrap();
        assert!(output.status.success(), "git {args:?}: {output:?}");
        String::from_utf8(output.stdout).unwrap().trim().to_string()
    }

    /// Writes `dot` and runs `pas run` on it with the fake first on `PATH`.
    pub fn run(&self, dot: &str) -> Output {
        let pipeline = self.path().join("p.dot");
        fs::write(&pipeline, dot).unwrap();
        let path = std::env::var_os("PATH").unwrap_or_default();
        let mut paths = vec![self.bin()];
        paths.extend(std::env::split_paths(&path));
        Command::new(env!("CARGO_BIN_EXE_pas"))
            .arg("run")
            .arg(&pipeline)
            .arg("--workdir")
            .arg(self.repo())
            .arg("--logs")
            .arg(self.logs())
            .env("PATH", std::env::join_paths(paths).unwrap())
            .env("FAKE_AGENT_SCENARIOS", self.scenarios())
            .env("PAS_STATE_DIR", self.path().join("state"))
            .env("GIT_CEILING_DIRECTORIES", self.path())
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("PAS_HEARTBEAT_INTERVAL_MS")
            .current_dir(self.path())
            .output()
            .unwrap()
    }

    /// The journal of the one Run made so far.
    pub fn events(&self) -> Vec<Value> {
        let runs: Vec<PathBuf> = fs::read_dir(self.logs().join("runs"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        assert_eq!(runs.len(), 1, "expected one Run folder: {runs:?}");
        fs::read_to_string(runs[0].join("events.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    /// The journal's Events of type `kind`, as their `data`.
    pub fn events_of(&self, kind: &str) -> Vec<Value> {
        self.events()
            .into_iter()
            .filter(|event| event["type"] == kind)
            .map(|event| event["data"].clone())
            .collect()
    }

    /// The node ids of every `StageStarted`, in order.
    pub fn stages_started(&self) -> Vec<String> {
        self.events_of("StageStarted")
            .iter()
            .map(|data| data["node_id"].as_str().unwrap().to_string())
            .collect()
    }

    /// How many times the fake started `scenario`.
    pub fn attempts(&self, scenario: &str) -> u32 {
        let counter = self.scenarios().join(format!("attempts.{scenario}"));
        match fs::read_to_string(counter) {
            Ok(text) => text.trim().parse().unwrap(),
            Err(_) => 0,
        }
    }

    /// The fake's argv per start, without the `-p` value.
    pub fn invocations(&self) -> Vec<Vec<String>> {
        self.blocks("invocations.log")
            .iter()
            .map(|block| block.lines().map(str::to_string).collect())
            .collect()
    }

    /// The fake's `-p` prompt per start.
    pub fn prompts(&self) -> Vec<String> {
        self.blocks("prompts.log")
    }

    /// The `--- start` blocks of one of the fake's logs.
    fn blocks(&self, log: &str) -> Vec<String> {
        let text = fs::read_to_string(self.scenarios().join(log)).unwrap_or_default();
        text.split("--- start\n")
            .skip(1)
            .map(str::to_string)
            .collect()
    }
}

/// `pas run`'s stderr as text.
pub fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}
