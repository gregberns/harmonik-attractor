//! Runs `pas run` against the fake `claude` (`tests/agents/fake-claude`).
//!
//! Each [`FakeAgent`] owns a scratch folder: a git repo used as the workdir,
//! whose first commit holds a `pas.toml` with the `fake` agent profile
//! (`test_only`, `command` = the fake script), the fake's scenario folder,
//! and the Run's logs and state folders. Pipelines select the fake with
//! `agent="fake"`, and every `pas run` passes `--allow-test-agents`. A
//! `bin/` first on `PATH` holds a fail-loud stub for every agent CLI
//! ([`BLOCKED_AGENTS`]), so a test can never reach a real agent from the
//! user's `PATH`; [`FakeAgent::shim_claude_on_path`] replaces the `claude`
//! stub with the fake (the `llm_provider="claude"` alias), and
//! `shim_codex_on_path`/`shim_gemini_on_path` do the same for `codex` and
//! `gemini`. The `pas.toml` also has `fake-codex`, `fake-gemini` and
//! `fake-claude` profiles (the built-in `codex`/`gemini`/`claude` profiles
//! with the fakes as command, `test_only`; `fake-claude` resumes sessions,
//! `fake` doesn't), and `fake-pi`: a `pi` profile run by
//! `tests/agents/fake-pi`, whose key variable `FAKE_PI_KEY` only the Pi tests
//! set (to the dummy [`FAKE_PI_KEY`]).
//! Git ignores the developer's global and system config (hooks, templates).

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;

/// The fake script `name` in the repo's `tests/agents/`.
fn fake_script(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../tests/agents/{name}"))
}

/// The fake script in the repo's `tests/agents/`.
fn fake_claude() -> PathBuf {
    fake_script("fake-claude")
}

/// Agent CLIs a test must never reach on the user's `PATH`: each gets a
/// stub in `bin/` that exits 2 unless a test shims the fake in its place.
pub const BLOCKED_AGENTS: [&str; 4] = ["claude", "codex", "gemini", "pi"];

/// The dummy API key the Pi tests give `pas` as `FAKE_PI_KEY`.
#[allow(dead_code)]
pub const FAKE_PI_KEY: &str = "sk-secret";

pub struct FakeAgent {
    dir: tempfile::TempDir,
}

impl FakeAgent {
    pub fn new() -> Self {
        let fake = Self {
            dir: tempfile::tempdir().unwrap(),
        };
        for dir in [fake.bin(), fake.scenarios(), fake.repo(), fake.home()] {
            fs::create_dir_all(dir).unwrap();
        }
        for agent in BLOCKED_AGENTS {
            fake.block_agent(agent);
        }
        fake.git(&["init", "-q"]);
        fake.write_pas_toml("");
        fake.git(&["add", "pas.toml"]);
        fake.git(&["commit", "-q", "-m", "base"]);
        fake
    }

    /// Writes the repo's `pas.toml`: the `fake` profile, then `agents`
    /// (more `[agents.<name>]` tables, or an override of `fake`'s fields
    /// when it starts with plain keys). Does not commit.
    pub fn write_pas_toml(&self, agents: &str) {
        let fake = fake_claude().canonicalize().unwrap();
        let codex = fake_script("fake-codex").canonicalize().unwrap();
        let gemini = fake_script("fake-gemini").canonicalize().unwrap();
        let pi = fake_script("fake-pi").canonicalize().unwrap();
        fs::write(
            self.repo().join("pas.toml"),
            format!(
                "[project]\nname = \"fake-test\"\n\n\
                 [agents.fake-codex]\ninherit_from = \"codex\"\ncommand = {codex:?}\ntest_only = true\n\n\
                 [agents.fake-gemini]\ninherit_from = \"gemini\"\ncommand = {gemini:?}\ntest_only = true\n\n\
                 [agents.fake-claude]\ninherit_from = \"claude\"\ncommand = {fake:?}\ntest_only = true\n\n\
                 [agents.fake-pi]\nmechanism = \"pi\"\ncommand = [{pi:?}]\nargs = [\"--mode\", \"json\"]\n\
                 reasoning_args = [\"--thinking\", \"{{reasoning}}\"]\nprovider = \"fakeprov\"\n\
                 base_url = \"http://127.0.0.1:9/v1\"\nmodel = \"fake-model\"\napi_key_env = \"FAKE_PI_KEY\"\n\
                 limits = {{ context = 1000, max_output = 100 }}\ntest_only = true\n\n\
                 [agents.fake]\nmechanism = \"claude-p\"\ncommand = {fake:?}\ntest_only = true\n{agents}\n"
            ),
        )
        .unwrap();
    }

    /// [`FakeAgent::write_pas_toml`], committed, so the Run's worktree and
    /// the dirty check see it.
    pub fn commit_pas_toml(&self, agents: &str) {
        self.write_pas_toml(agents);
        self.git(&["add", "pas.toml"]);
        self.git(&["commit", "-q", "-m", "pas.toml"]);
    }

    /// Puts the fake on `PATH` as `claude` in place of its stub, for
    /// `llm_provider="claude"`.
    #[allow(dead_code)]
    pub fn shim_claude_on_path(&self) {
        self.shim_on_path("claude", "fake-claude");
    }

    /// Puts `fake-codex` on `PATH` as `codex`, for `llm_provider="codex"`.
    #[allow(dead_code)]
    pub fn shim_codex_on_path(&self) {
        self.shim_on_path("codex", "fake-codex");
    }

    /// Puts `fake-gemini` on `PATH` as `gemini`, for `llm_provider="gemini"`.
    #[allow(dead_code)]
    pub fn shim_gemini_on_path(&self) {
        self.shim_on_path("gemini", "fake-gemini");
    }

    /// Replaces `bin/<agent>`'s stub with the fake script `fake`.
    fn shim_on_path(&self, agent: &str, fake: &str) {
        let path = self.bin().join(agent);
        fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink(fake_script(fake), path).unwrap();
    }

    /// Writes `bin/<agent>`: a stub that refuses to run, ahead of any real
    /// agent on the user's `PATH`.
    fn block_agent(&self, agent: &str) {
        use std::os::unix::fs::PermissionsExt;
        let stub = self.bin().join(agent);
        fs::write(
            &stub,
            format!(
                "#!/bin/sh\necho 'real agent blocked in tests: call shim_{agent}_on_path()' >&2\nexit 2\n"
            ),
        )
        .unwrap();
        fs::set_permissions(&stub, fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn bin(&self) -> PathBuf {
        self.path().join("bin")
    }

    /// `HOME` for `pas` and its agents: an empty folder in the test's.
    pub fn home(&self) -> PathBuf {
        self.path().join("home")
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

    /// Writes `dot` and runs `pas run` on it with test agents allowed.
    pub fn run(&self, dot: &str) -> Output {
        self.command(dot).output().unwrap()
    }

    /// Writes `dot` and returns the `pas run` command [`Self::run`] runs, for
    /// tests that add to its environment or stdin.
    pub fn command(&self, dot: &str) -> Command {
        let mut command = self.pas_run(dot);
        command
            .arg("--workdir")
            .arg(self.repo())
            .arg("--logs")
            .arg(self.logs());
        command
    }

    /// `pas` with the fake's environment, in the scratch folder.
    fn pas(&self) -> Command {
        let path = std::env::var_os("PATH").unwrap_or_default();
        let mut paths = vec![self.bin()];
        paths.extend(std::env::split_paths(&path));
        let mut command = Command::new(env!("CARGO_BIN_EXE_pas"));
        command
            .env("PATH", std::env::join_paths(paths).unwrap())
            .env("FAKE_AGENT_SCENARIOS", self.scenarios())
            .env("PAS_STATE_DIR", self.path().join("state"))
            // Agents' session files (`~/.claude`, `~/.codex`) and anything
            // else under home stay in the test's folder, never the real one.
            .env("HOME", self.home())
            .env("CODEX_HOME", self.home().join(".codex"))
            .env("GIT_CEILING_DIRECTORIES", self.path())
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("PAS_HEARTBEAT_INTERVAL_MS")
            .current_dir(self.path());
        command
    }

    /// Writes `dot` as the pipeline file and returns its path.
    fn pipeline(&self, dot: &str) -> PathBuf {
        let pipeline = self.path().join("p.dot");
        fs::write(&pipeline, dot).unwrap();
        pipeline
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

    /// The one Run's `run.json`.
    pub fn run_json(&self) -> Value {
        let runs: Vec<PathBuf> = fs::read_dir(self.logs().join("runs"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        assert_eq!(runs.len(), 1, "expected one Run folder: {runs:?}");
        serde_json::from_str(&fs::read_to_string(runs[0].join("run.json")).unwrap()).unwrap()
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

    /// The fake's `PAS_*`, `ANTHROPIC_*`, `OPENAI_*` and `CLAUDE_CODE_USE_*`
    /// environment per start.
    pub fn env_logs(&self) -> Vec<BTreeMap<String, String>> {
        self.blocks("env.log")
            .iter()
            .map(|block| {
                block
                    .lines()
                    .filter_map(|line| line.split_once('='))
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect()
            })
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

// --- Worktree per Run (ticket 06) ---

// Each test crate that includes this module uses a subset of these.
#[allow(dead_code)]
impl FakeAgent {
    /// The scratch folder holding the repo, logs and scenarios.
    pub fn root(&self) -> &Path {
        self.path()
    }

    /// The Pipeline's logs folder (`--logs`).
    pub fn logs_dir(&self) -> PathBuf {
        self.logs()
    }

    /// `pas run` like [`FakeAgent::run`], with `extra` arguments.
    pub fn run_with(&self, dot: &str, extra: &[&str]) -> Output {
        self.command_with(dot, extra).output().unwrap()
    }

    /// The `pas run` command [`FakeAgent::run_with`] runs, to spawn it.
    pub fn command_with(&self, dot: &str, extra: &[&str]) -> Command {
        self.command_in(dot, &self.repo(), extra)
    }

    /// [`FakeAgent::command_with`] with `--workdir <workdir>`.
    pub fn command_in(&self, dot: &str, workdir: &Path, extra: &[&str]) -> Command {
        let mut command = self.pas_run(dot);
        command
            .arg("--workdir")
            .arg(workdir)
            .arg("--logs")
            .arg(self.logs())
            .args(extra);
        command
    }

    /// `pas run <pipeline> --allow-test-agents` for `dot`, with no
    /// `--workdir` or `--logs`, in the scratch folder, with the fake's
    /// environment.
    pub fn pas_run(&self, dot: &str) -> Command {
        let mut command = self.pas();
        command
            .arg("run")
            .arg(self.pipeline(dot))
            .arg("--allow-test-agents");
        command
    }

    /// [`FakeAgent::command`] without `--allow-test-agents`.
    pub fn command_without_test_agents(&self, dot: &str) -> Command {
        let mut command = self.pas();
        command
            .arg("run")
            .arg(self.pipeline(dot))
            .arg("--workdir")
            .arg(self.repo())
            .arg("--logs")
            .arg(self.logs());
        command
    }

    /// `pas validate <pipeline> <extra>` for `dot`, run in the repo so it
    /// reads the repo's `pas.toml`.
    pub fn validate(&self, dot: &str, extra: &[&str]) -> Output {
        self.pas()
            .arg("validate")
            .arg(self.pipeline(dot))
            .args(extra)
            .current_dir(self.repo())
            .output()
            .unwrap()
    }

    /// `pas <args>` with the fake's environment (e.g. `stop`, `kill`).
    pub fn pas_cli(&self, args: &[&str]) -> Output {
        self.pas_cli_command(args).output().unwrap()
    }

    /// The command [`FakeAgent::pas_cli`] runs, to spawn it.
    pub fn pas_cli_command(&self, args: &[&str]) -> Command {
        let mut command = self.pas();
        command.args(args);
        command
    }

    /// The folders under `<logs>/runs`, sorted.
    pub fn run_dirs(&self) -> Vec<PathBuf> {
        let Ok(entries) = fs::read_dir(self.logs().join("runs")) else {
            return vec![];
        };
        let mut dirs: Vec<PathBuf> = entries.map(|entry| entry.unwrap().path()).collect();
        dirs.sort();
        dirs
    }

    /// The one Run's `run.json`.
    pub fn run_meta(&self) -> Value {
        let runs = self.run_dirs();
        assert_eq!(runs.len(), 1, "expected one Run folder: {runs:?}");
        serde_json::from_str(&fs::read_to_string(runs[0].join("run.json")).unwrap()).unwrap()
    }

    /// The one Run's worktree, from its `run.json`.
    pub fn worktree(&self) -> PathBuf {
        PathBuf::from(
            self.run_meta()["worktree"]
                .as_str()
                .expect("run.json worktree"),
        )
    }

    /// `git <args>` in `dir` as the fixed test user; returns trimmed stdout.
    pub fn git_in(&self, dir: &Path, args: &[&str]) -> String {
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
            .current_dir(dir)
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
}
