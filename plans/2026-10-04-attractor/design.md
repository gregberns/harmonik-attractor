# Attractor design: the PAS fork

A plan, not code. It covers decisions Q12-56 for the operator's fork of PAS
(`~/github/harmonik-attractor`, at `50945da`). Code changes come later, in
that repo. Terms follow [CONTEXT.md](CONTEXT.md); module, interface, seam,
adapter, depth, leverage and locality are used as in the codebase-design
skill.

Inputs: [decisions.md](https://github.com/gregberns/harmonik-v3/blob/main/plans/2026-10-04-attractor/decisions.md) Q12-56 and, in [research/](https://github.com/gregberns/harmonik-v3/blob/main/plans/2026-10-04-attractor/research/),
[impl-harmonik-attractor.md](https://github.com/gregberns/harmonik-v3/blob/main/plans/2026-10-04-attractor/research/impl-harmonik-attractor.md),
[spec-gaps-pas.md](https://github.com/gregberns/harmonik-v3/blob/main/plans/2026-10-04-attractor/research/spec-gaps-pas.md),
[test-and-output-pas.md](https://github.com/gregberns/harmonik-v3/blob/main/plans/2026-10-04-attractor/research/test-and-output-pas.md) and
[kilroy-git.md](https://github.com/gregberns/harmonik-v3/blob/main/plans/2026-10-04-attractor/research/kilroy-git.md). Paths below are under the fork's
`crates/` unless noted.

Every choice the operator hasn't made is in [Operator
questions](#operator-questions) at the end. Where this document shows one
option, it is the proposal those questions would confirm or change.

## Summary

- **Agent handlers.** A small public interface crate (`attractor-agent-handler`)
  holds the handler trait, the request and result types and the registry.
  Each handler is its own crate (Q31, Q35): `claude-p` and `pi` (the
  multi-model handler for DeepSeek, GLM and the hosted Qwen, Q34) are built
  new; Codex and Gemini are ported from today's code (Q5, Q32). A shared process-runner
  crate gives local handlers the transcript and stderr files, start event,
  env, timeout and graceful kill. The engine keeps prompt assembly,
  routing, retries and git. `run` never returns an error: every agent
  failure comes back classified; the engine routes a reported error and
  stops the run on a timeout, crash or launch failure, as today (Q47).
- **Agent config.** Named agent profiles (defaults embedded, overrides in
  `pas.toml`) replace
  the closed `LlmProvider` enum. A new model or flag, or a new agent that
  speaks an output format a handler already reads, is a config edit. An
  agent with a new output format (dsh, ZCode, Qwen Code) needs a new
  handler crate. A new model reachable through Pi (DeepSeek, GLM, the
  hosted Qwen, anything OpenAI-compatible) is a profile (Q33, Q34). Reasoning level becomes a profile field passed through (Claude:
  `--effort <level>`, present in `claude --help` 2.1.280).
- **Sessions.** A node that runs again (retry, or a loop back) continues
  its previous agent conversation by default (Q28), mapped onto the spec's
  `fidelity=full` and `thread_id`; session ids live in the checkpoint.
- **Worktree and attempt commits.** Each run gets its own worktree
  (default `<project-root>/.pas/worktrees/<run-id>`) and branch. The engine
  commits after every attempt, with `Pas-*` trailers, and never deletes
  partial work. Resume stays checkpoint-based for now (Q43). On success the
  worktree is removed; the run reports its branch and final commit and
  merges nothing (Q44).
- **Run folder.** PAS's existing run folder becomes a documented contract,
  plus an agent-start event, an on-disk stderr file, the prompt and a final
  result file.
- **Failures (Q47-51).** Never hide a failure; keep what exists. An
  agent-reported error stays routable; a timeout or crash stops the run, as
  today, with an error naming the node, class and transcript and stderr
  paths. Two failure-hiding bugs are fixed by stopping the run (the
  first-edge fallback, an exhausted Retry), and the Claude error-subtype
  bug is fixed. The spec's routing rework is deferred.
- **Digital twin.** A fake `claude -p` executable, selected through a test
  profile behind an explicit flag, drives the real production path in end
  to end tests.

## 1. Agent handlers (Q14, Q23, Q31-33)

### Today

`CodergenHandler` (`attractor-pipeline/src/handlers/codergen_handler.rs`)
mixes four jobs: prompt assembly; invocation bookkeeping (transcripts, the
`LlmInvoked` event); timeout and process-group kill; and a provider-specific
command builder and parser keyed on `LlmProvider { Claude, Codex, Gemini }`
(`execution_plan.rs:15-52`, `codergen_provider.rs:348-840`). The agent
process is a true external dependency; a fake executable is a local
stand-in for it.

Constraints:
- The engine owns git (Q15-17), prompts, routing and retries; handlers never
  touch them.
- Every way an agent can fail comes back as a classified result, not a Rust
  error, so the engine decides: route a reported error, stop the run on the
  rest (Q47).
- A start event with pid and file paths reaches the journal before the
  agent produces output (Q20).
- The fake agent goes through the same code as `claude` (Q19).
- Each handler is separate code in its own crate behind a public handler
  interface; expect about six over time (Q31, Q35). Built now: `claude-p`
  and `pi` (new), and `codex-exec` and `gemini` ported from today's code
  (Q5, Q32, Q34).
- ACP, tmux and a remote host must fit later without changing the engine
  (Q14, Q23).

### Crates

```
attractor-cli (pas) ──registers──▶ handler crates ──▶ attractor-agent-process
       │                                │                     │
       └──▶ attractor-pipeline ──▶ attractor-agent-handler (interface, registry, config) ◀┘
```

The interface crate is `attractor-agent-handler`: `attractor-agent` is the
existing agent-session-loop crate, so the name was changed in ticket 02a.

| Crate | Holds |
|---|---|
| `attractor-agent-handler` | The public interface: the `AgentHandler` trait; `AgentRequest`, `AgentResult`, `Session`, `FailureClass`, `Usage`, `InvocationFiles`; the `AgentObserver`; profile config loading; and `Agents`, the registry the engine calls. Depends on no other PAS crate |
| `attractor-agent-process` | Shared local-process runner: spawn with the given env, cwd and stdin; own process group; prompt, transcript and stderr files written live; `Started`; timeout and cancel with TERM, grace, KILL; drop guard. Handlers that run a local process use it; handlers that don't (remote, ACP over a socket) skip it |
| `attractor-handler-claude-p` | Claude `-p` stream-json: argv, parser, failure table, session id. Built new |
| `attractor-handler-codex-exec` | Today's Codex command and parser (`codergen_provider.rs:406-421, 715-757`), moved |
| `attractor-handler-gemini` | Today's Gemini command, `--help` format probe and parser (`codergen_provider.rs:270-335, 422-432`), moved |
| `attractor-handler-pi` | Pi (`@earendil-works/pi-coding-agent`, whatever `pi` is installed, Q52) in `--mode json`: the multi-model handler for DeepSeek, GLM, the hosted Qwen and any model Pi can reach (Q33, Q34). Built new |

`codergen_provider.rs`, `provider_stream.rs` and `process_group.rs` leave
`attractor-pipeline`: the process parts into `attractor-agent-process`, the
per-agent parts into the handler crates. `LlmProvider` is deleted.

### The interface

```rust
// attractor-agent-handler
#[async_trait]
pub trait AgentHandler: Send + Sync {
    /// The `mechanism` name profiles use, e.g. "claude-p".
    fn mechanism(&self) -> &'static str;
    /// Check the handler-specific fields of a profile (plan-compile time).
    fn check(&self, profile: &Profile) -> Result<(), ConfigError>;
    /// Run one invocation to its end. Never returns an error.
    async fn run(&self, inv: Invocation<'_>) -> AgentResult;
}

pub struct Agents;   // loaded profiles + registered handlers
impl Agents {
    pub fn new(handlers: Vec<Arc<dyn AgentHandler>>, layers: &[ConfigSource],
               allow_test_agents: bool) -> Result<Agents, ConfigError>;
    pub fn check(&self, sel: &Selection) -> Result<(), ConfigError>;   // plan compile
    pub async fn run(&self, req: AgentRequest<'_>) -> AgentResult;     // never Err
}

pub struct Selection { pub profile: String, pub model: Option<String>, pub reasoning: Option<String> }
pub struct AgentRequest<'a> {
    pub selection: Selection,
    pub prompt: String,            // assembled by the engine
    pub workdir: PathBuf,          // the run's worktree
    pub timeout: Option<Duration>, // node timeout, else profile timeout
    pub record: Record,            // run_id, node_id, attempt, invocation_id, output dir
    pub session: Session,          // New(uuid) | Continue(recorded id)
    pub observer: Option<&'a dyn AgentObserver>,   // Started, Finished -> journal
    pub cancel: CancellationToken, // pas stop / kill
}
/// What `Agents::run` hands a handler: the request plus everything resolved.
pub struct Invocation<'a> {
    pub profile: &'a ResolvedProfile,  // templates filled: model, reasoning, session_id
    pub env: BTreeMap<String, String>, // complete child env (see invariants)
    pub prompt: &'a str, pub workdir: &'a Path, pub timeout: Duration,
    pub files: InvocationFiles,        // fixed paths: prompt, transcript, stderr
    pub session: &'a Session,
    pub started: &'a dyn Fn(StartedInfo),  // call once after spawn
    pub cancel: CancellationToken,
}
pub struct AgentResult {
    pub invocation_id: String,
    pub status: AgentStatus,           // Completed | Failed(FailureClass) | Cancelled
    pub text: String, pub detail: String,
    pub usage: Usage, pub exit: Option<ExitInfo>, pub duration: Duration,
    pub files: InvocationFiles,
    pub agent_session_id: Option<String>,  // the id the agent actually used
    pub continued: bool,                   // an earlier session was continued
}
pub enum FailureClass { Reported, Timeout, Crash, NoResult, Launch }
```

### Who enforces which invariant

`Agents::run` wraps every handler, so the invariants that don't depend on
how an agent is driven are enforced once, outside the handlers:

| Invariant | Enforced by |
|---|---|
| Profile resolved, overrides valid, test-only profiles refused without `--allow-test-agents` | `Agents` |
| The complete child env: parent env minus `remove`, then `set`, plus `PAS_RUN_ID`, `PAS_NODE_ID`, `PAS_ATTEMPT`, `PAS_INVOCATION_ID`, `PAS_SESSION_ID` | `Agents` computes it; the handler must use exactly this map |
| `<id>.prompt.txt` written before the handler is called; file paths fixed | `Agents` |
| `Finished` sent exactly once and equal to the return value | `Agents` sends it from the return value; handlers never send it |
| `Started` once per spawn; a handler spawns more than once only inside the rate-limit window (Q48), each spawn with its own pid, transcript and stderr file | `Agents` journals each as its own `LlmStarted` |
| A handler that panics returns `Failed(Crash)` with "handler panicked" | `Agents` (catch-unwind around the future) |
| A hard deadline (timeout plus grace plus a margin) in case a handler ignores its timeout: `Failed(Timeout)` | `Agents` |
| cwd = workdir, stdin never inherited, own process group, transcript and stderr written live, `Started` after spawn and before the first output, TERM-grace-KILL on timeout or cancel, drop guard | The handler; `attractor-agent-process` gives all of them to handlers that run a local process |
| Classify every end into a status and failure class; record the agent's session id; continue, or fail if it can't (Q50) | The handler (its own failure table) |
| Never touch git, never retry except inside the rate-limit window, never write outside the output directory | Every handler (review rule; tests with the fake) |

`claude-p` failure table (first match wins):

| Observed | Status |
|---|---|
| cancelled | `Cancelled` |
| timeout fired | `Failed(Timeout)` |
| final `result` line, `is_error` or `subtype` starting `error` | `Failed(Reported)` (fixes `codergen_provider.rs:707`) |
| final `result` line otherwise | `Completed`, even after a non-zero exit (today's rule) |
| no result line, non-zero exit or signal | `Failed(Crash)` |
| no result line, exit 0 | `Failed(NoResult)` |
| could not start | `Failed(Launch)` |

The `claude-p` handler adds only the flags its parser depends on (`-p
<prompt> --output-format stream-json --verbose`); everything else comes
from the profile. `codex-exec` and `gemini` keep today's parsing rules,
moved into the same table shape.

### The Pi handler (Q34)

From [research/multi-model-handler.md](https://github.com/gregberns/harmonik-v3/blob/main/plans/2026-10-04-attractor/research/multi-model-handler.md)
(Pi 1.0.3, smoke-tested against a local Qwen3.8-27B; `pi-smoke/evidence.txt`):
- **Command:** `pi --mode json --model <provider/id> [--thinking <level>]
  --session-dir <dir> --session-id <id> <prompt>`, with an exact
  `provider/id` (`--model` also fuzzy-matches). Pi has no `--cwd`; it runs
  in the process cwd (the worktree). The profile runs whatever `pi` is
  installed; no pinned version and no per-upgrade smoke test (Q52).
- **Stdin `/dev/null`:** Pi reads stdin to EOF when it isn't a TTY.
- **Per-invocation config dir:** the handler writes a fresh
  `PI_CODING_AGENT_DIR` under the run folder (never in the worktree;
  removed when the invocation ends, since `models.json` holds an API key)
  with:
  - `models.json`: the profile's provider, `baseUrl`, `api:
    openai-completions`, the model id and its limits (an unknown id would
    otherwise inherit the provider default's limits,
    `model-resolver.ts:570-596`), and the API key read from the env var the
    profile names;
  - `settings.json`: Pi's own retry (429, 5xx, network) set to cover the
    profile's rate-limit window (Q48, section 5).
  Plus `PI_TELEMETRY=0`, `PI_OFFLINE=1` and `PI_SKIP_VERSION_CHECK=1`. Pi
  never reads or writes `~/.pi`. The key goes in the 0600 `models.json`
  rather than `--api-key` on argv, which `ps` would show (operator
  question).
- **Sessions:** `--session-dir <run folder>/pi-sessions --session-id <id>`
  creates the session on first use and resumes it after, under the id the
  engine chose (verified headless). So `New(id)` and `Continue(id)` are the
  same command, and `agent_session_id` is the id passed.
- **Failure table** (json mode exits 0 even when the provider fails):

  | Observed | Status |
  |---|---|
  | cancelled / timeout fired | `Cancelled` / `Failed(Timeout)` |
  | last assistant `message_end` has `stopReason` `error` or `aborted` | `Failed(Reported)`, detail = `errorMessage` |
  | last assistant `message_end` has `stopReason` `stop` | `Completed`, text = that message |
  | no final assistant message, non-zero exit or signal | `Failed(Crash)` |
  | no final assistant message, exit 0 | `Failed(NoResult)` |
  | could not start | `Failed(Launch)` |

  This follows Pi's own text mode (`modes/print-mode.ts:145` at v1.0.3).
- **Trade-offs:** one generic system prompt for every model, not a
  model-tuned harness (the trade Q33 chose); `read`, `bash`, `edit` and
  `write` run unprompted with no sandbox (acceptable under Q7); Node 22.19+
  is required.

### Registration

- `pas` registers handlers in one compile-time list in `attractor-cli`;
  all are always compiled in (no cargo features, Q51):

  ```rust
  fn handlers() -> Vec<Arc<dyn AgentHandler>> { vec![
      Arc::new(ClaudeP::default()),
      Arc::new(CodexExec::default()),
      Arc::new(Gemini::default()),
      Arc::new(Pi::default()),
  ]}
  ```

- A profile's `mechanism` picks the handler by name; an unknown name fails
  `pas validate`.
- **Adding handler #7:** a new crate implementing `AgentHandler` (using
  `attractor-agent-process` if it runs a local process), one workspace
  entry and one line in `handlers()`, and a profile (in the embedded
  defaults or a project's `pas.toml`). No engine or interface change. Dynamic loading (dylib or
  WASM plugins) is not proposed.

### The three shapes, revisited

The parallel designs (A: one call; B: sessions; C: a runner with public
launcher and protocol ports) still inform this, with Q31 settling the
question of how public the inside is:
- **A** becomes the public trait: one `run`, never an error, failure
  classes. Its leverage stays: the engine calls `Agents::run` and handles
  three statuses.
- **C**'s runner becomes `attractor-agent-process`, and its protocols
  become handler crates. Its launcher port is not built: a remote or tmux
  handler is its own crate that doesn't use the local runner, so no
  launcher trait is needed yet ("one adapter means a hypothetical seam").
- **B**'s session interface stays rejected as the trait: all six expected
  handlers are one-shot per invocation, and mid-run control was declined
  (Q26). B's idea of journaling a handle survives as the process-group id in
  `Started` (section 4).
- Cost of the split: six crates instead of one, and each handler re-states
  its argv and failure table. Gain: a handler can be added, removed or
  removed without touching the others, and the shared invariants
  still live in one place (`Agents` and the runner).

### Sessions: new or continue (Q25, Q27, Q28, Q30)

**Request.** `AgentRequest.session` is `New(uuid)` or `Continue(id)`.
- `New(id)`: the engine mints a UUID. Profiles see it as `{session_id}` and
  `PAS_SESSION_ID`; the default `claude` profile passes `--session-id
  {session_id}` (`claude --help` 2.1.280: "Use a specific session ID").
- `Continue(id)`: `id` is the agent session id recorded from an earlier
  invocation. The profile's resume form replaces its normal form:
  - `claude`: `resume_args = ["--resume", "{session_id}"]`, without
    `--session-id`. Verified on the operator's Max login
    (research/claude-resume-check.md, Q62): `--session-id <uuid>` sets the
    id, and `-p --resume <id>` keeps the same id and remembers the context.
    `--session-id` with `--resume` is untested, so they aren't combined.
  - `codex`: `codex exec resume <id> <prompt>` is a subcommand, so the
    profile has a whole `resume_command`. `exec resume --help` (0.156.1)
    lists `--json`, `-m`, `--skip-git-repo-check` and
    `--dangerously-bypass-approvals-and-sandbox`, not `--yolo`, and no `-C`,
    so cwd comes from the process.
  - `gemini`: resume support unchecked; no resume form is configured.
  - `pi`: `--session-dir` plus `--session-id` handles both cases (above).
  - **A profile with no resume form** (gemini today) can't continue. So that
    nothing is hidden, a node using such a profile gets `fidelity=fresh` as
    its default, shown by `pas validate`; setting `fidelity=full` on it is a
    validation error. Existing gemini pipelines keep working.
- **Recorded:** `agent_session_id` and `continued` in `AgentResult`,
  `LlmStarted` (once known) and `LlmInvoked`. Claude: `session_id` from
  `system/init`. Codex: `thread_id` from `thread.started`, if present.
- **Persistence (Q27):** the default profiles drop
  `--no-session-persistence` (Claude) and `--ephemeral` (Codex), so session
  files stay under `~/.claude` and `~/.codex`. No setting.
- **Resume failure (Q50):** if continuing fails because the session is
  gone, the attempt fails; no fallback to a new session. For Claude this is
  a result line with `is_error: true`, `subtype: error_during_execution`
  and an `errors` array ("No conversation found with session ID"), so the
  `claude-p` table reports `Failed(Reported)` with that reason (Q62).

**Engine: which session a node uses (fidelity, spec §5.4).**
- `fidelity=full` (the default, Q30, for profiles that can resume):
  continue the session for the node's thread key. `fidelity=fresh` (a PAS
  extension) starts a new one. The spec's default is `compact` (spec lines
  142, 1162-1163), so full-by-default is itself a deliberate divergence
  (Q28, Q30).
- `thread_id="<key>"` makes several nodes share one session (Q30). Default
  thread key: the node id, so a node that runs again (retry, or a loop such
  as implement → review → implement) continues its own session.
- The spec's `truncate`, `compact` and `summary:*` stay rejected at compile
  time; they would need the engine to edit or summarise a transcript.
- The engine keeps a per-run map thread key → agent session id in
  `checkpoint.json`, so it survives a `pas` resume (Q43). First use of a key:
  `New`; later uses: `Continue(last recorded id)`.
- **Deliberate divergence from the spec:** spec §5.3 (strongdm/attractor
  `fb57a55`) degrades `full` fidelity to `summary:high` on resume, because
  in-memory sessions can't be serialized. Our agents' sessions are on disk,
  so a resumed run continues them.
- Two nodes sharing a thread must not run at once; PAS runs sequentially,
  so this matters only if fan-out is added.

**Prompt on a re-run (the engine's job).** The engine assembles the prompt,
so it adds what the agent needs:
- on a retry: why the last attempt failed (`failure_class` and
  `failure_reason`, e.g. "the previous attempt timed out after 600 s");
- on a loop back: the new inputs, e.g. the review's result, through the
  existing context in the prompt (`codergen_handler.rs:240-256`);
- after an interrupted attempt (section 3): "your previous attempt was
  interrupted (<class>); its changes since `<last attempt commit>` are kept in
  the worktree and recorded in commit `<interrupted sha>`; review `git diff
  <last attempt commit>` before continuing".
Handlers pass the prompt through unchanged.

### Engine side

- `CodergenHandler` keeps prompt assembly (`codergen_handler.rs:237-276`),
  label extraction and context updates. It calls `Agents::run` and maps the
  result: `Completed` → Success; `Failed(Reported)` → a routable Fail with
  `failure_reason`; `Failed(Timeout)` → today's retryable timeout error;
  `Failed(Crash | NoResult | Launch)` → an error that stops the run, with
  the clear message of section 5; `Cancelled` → the engine's stop path.
- Plan compilation calls `Agents::check` in place of the `llm_provider`
  checks (`execution_plan.rs:970, 1024-1043`). A node names
  `agent="<profile>"`; `llm_model` and `reasoning_effort` are overrides.
  `reasoning_effort` is no longer rejected
  (`docs/execution-capabilities.md:22`).
- **Compatibility (Q51):** `llm_provider="claude"`, `"codex"` and
  `"gemini"` stay accepted as aliases for the default profiles of the same
  name, with no deprecation, so existing `.dot` files run unchanged.
  `[codergen.claude]` in `pas.toml` keeps working, read into the `claude`
  profile's `args`.

### How other handlers fit (not built)

| Handler | Fits as |
|---|---|
| ACP | its own crate; over stdio it uses `attractor-agent-process` with stdin as a JSON-RPC pipe (`initialize`, `session/new`, `session/prompt`; `session/cancel` before TERM; permission answers from a profile policy); over a socket it skips the runner |
| tmux | its own crate that runs the agent in a pane, streams `pipe-pane` output into the transcript, reads exit status from a file; no structured result unless the profile defines one |
| remote (Q23) | its own crate that runs the command on another host with the env map sent explicitly, streams lines back into the local files, and maps signals over the connection. The host checks out the previous attempt commit (or `base_sha`) and pushes its work back. "Host unreachable" is a `Launch` failure |
| ACP via OpenCode or Qwen Code | the runner-up multi-model route (multi-model-handler.md), if an ACP handler is built for other reasons. No generic run-any-program `exec` handler for now (Q33); Pi is the built multi-model handler (Q34) |


## 2. Agent and model config (Q13)

- **Files:** the default profiles live in an `agents.toml` in the PAS
  source, embedded in the binary, so a fresh install works. A project
  overrides or adds profiles in its existing `pas.toml` under `[agents.*]`
  (pre-answered question 4); a later layer replaces whole profiles by name.
- **Profile fields:** `mechanism`, `command`, `args`, `model`,
  `model_args`, `reasoning`, `reasoning_args`, `timeout`, `kill_grace`,
  `env` (`remove`, `set`; `set` applied after `remove`), `test_only`,
  `inherit_from`, and for sessions: `session_args` (used for a new session,
  e.g. `--session-id {session_id}`), `resume_args` (replace only
  `session_args` when continuing; `args` stay, so `--safe-mode` and the
  other flags are kept) or `resume_command` (a whole command form, used
  instead of `command` plus `args`; `model_args` and `reasoning_args` are
  appended to it, which `codex exec resume -m` accepts). Pi profiles
  add `provider`, `base_url`, `api_key_env` and `limits`; the handler
  checks them (`AgentHandler::check`). Templates: `{model}`, `{reasoning}`, `{session_id}`. An allowlist env mode
  is added only if the operator chooses it (question 5).
- **Limit of "config, not code" (Q13 vs Q14):** a profile can point at any
  binary, but a handler must read its output. Agents that print Claude
  stream-json, Codex JSON or Gemini JSON need no rebuild; an agent with
  another format (dsh's NDJSON, ZCode, Qwen Code) needs a new handler crate.
  A generic `exec` handler is not built (Q33); the way around the limit is
  the Pi handler, which reaches most models from a profile (section 1).
- **Example (default file):**

  ```toml
  [defaults]
  timeout = "10m"
  kill_grace = "10s"
  [defaults.env]
  remove  = ["ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_BASE_URL",
             "OPENAI_API_KEY", "CLAUDE_CODE_USE_BEDROCK", "CLAUDE_CODE_USE_VERTEX"]

  [profiles.claude]
  mechanism = "claude-p"
  command   = ["claude"]
  args      = ["--safe-mode", "--dangerously-skip-permissions",
               "--strict-mcp-config", "--disable-slash-commands"]
  session_args   = ["--session-id", "{session_id}"]
  resume_args    = ["--resume", "{session_id}"]   # replaces session_args only
  model_args     = ["--model", "{model}"]
  reasoning_args = ["--effort", "{reasoning}"

  [profiles.codex]
  mechanism  = "codex-exec"
  command    = ["codex"]
  args       = ["exec", "--json", "--yolo", "--skip-git-repo-check"]
  model_args = ["--model", "{model}"]
  resume_command = ["codex", "exec", "resume", "{session_id}", "--json",
                    "--skip-git-repo-check", "--dangerously-bypass-approvals-and-sandbox"]

  [profiles.gemini]
  mechanism  = "gemini"
  command    = ["gemini"]
  args       = ["--approval-mode", "yolo"]   # output format chosen by the handler's --help probe
  model_args = ["--model", "{model}"]

  # Pi profiles (Q34). The handler turns provider/base_url/api_key_env into
  # Pi's models.json; models.json is never edited by hand.
  [profiles.deepseek]
  mechanism   = "pi"
  command     = ["pi"]                 # whatever pi is installed (Q52)
  provider    = "deepseek"             # built into Pi
  model       = "deepseek-v4-pro"
  api_key_env = "DEEPSEEK_API_KEY"
  reasoning   = "high"                 # passed as --thinking

  [profiles.glm]
  mechanism   = "pi"
  command     = ["pi"]
  provider    = "zai"
  base_url    = "https://api.z.ai/api/paas/v4"   # pay-as-you-go, not the Coding Plan URL
  model       = "glm-5.3"
  api_key_env = "ZAI_API_KEY"

  [profiles.qwen]
  mechanism   = "pi"
  command     = ["pi"]
  provider    = "qwen-lan"
  base_url    = "http://<host>:<port>/v1"          # the operator's hosted Qwen; endpoint TBD
  model       = "qwen3.8"                          # id as the server names it
  api_key_env = "QWEN_API_KEY"                     # or none if the server needs no key
  limits      = { context = 262144, max_output = 32768 }

  [profiles.claude-opus]
  inherit_from = "claude"
  model = "opus"
  reasoning = "high"
  ```

- Removing `ANTHROPIC_API_KEY` by default keeps a stray key from silently
  moving a node from the Max subscription to API billing
  (impl-harmonik-attractor.md §7). A profile that routes Claude Code to
  GLM or DeepSeek (harnesses.md) sets `ANTHROPIC_BASE_URL` and a key in its
  own `set`, which is applied after `remove`.
- The `[codergen.claude]` section of `pas.toml`
  (`attractor-quality/src/manifest.rs:43-59`) folds into profile `args`.
- Validation: `pas validate` resolves every profile a pipeline uses
  (`check`); an unknown profile or an override the profile can't take
  (`reasoning` without `reasoning_args`) fails before the run starts.

## 3. Worktree and attempt commits (Q15-17, Q36-45)

Attempt commits with trailers are the record of what each attempt did
(Q38, Q40). Deriving a run's state and resume from git is **deferred**
(Q43): resume keeps using PAS's checkpoint, as today. The journal and run
folder are observability (Q20).

### Worktree and branch

- **Branch:** `pas/run/<run-id>` from `--base <ref>` (default `HEAD`) of the
  source checkout (Q37). If the source checkout has uncommitted changes, the
  run starts anyway; the engine records a warning (journal `RunStarted` and
  `run.json`) and the changes are not carried into the worktree.
- **Location (Q36):** default `<project-root>/.pas/worktrees/<run-id>`,
  mirroring Claude Code's `<project-root>/.claude/worktrees/<name>`.
  Configurable as `worktree_root` in the project config and with
  `--worktree-root`.
- **Keeping `.pas/` out of git:**
  - On first use, pas writes `.pas/.gitignore` containing `*`. That ignores
    everything under `.pas/` (worktrees, run folders) in the main checkout
    without touching a tracked file, and works whether or not `pas init`
    ran. `pas init` (question 17) does the same, nothing more.
  - The engine's commit never stages `.pas/`, whatever a repo's ignore
    rules say: it runs `git add -A -- . ':(exclude).pas'` in the worktree.
- **Ordering in `prepare_run`** (`attractor-cli/src/commands/run.rs:781+`),
  locks first so a refused run leaves no trace (PAS rule C5,
  `run.rs:838-843`):
  1. resolve the pipeline folder from `--logs` and take the Pipeline lock
     (`<logs_dir>/run.lock`, `run.rs:377-392`), which needs no run id;
  2. read the checkpoint, which gives the run id on resume, or mint one
     (or take `--run-id`), as today;
  3. create or reuse `<worktree_root>/<run-id>` and its branch;
  4. take the Worktree lock (`<git-dir>/pas-run.lock`);
  5. record `worktree`, `branch`, `base_sha` and the dirty-source warning in
     `run.json` (observability).
- In a linked worktree `--absolute-git-dir` is `.git/worktrees/<name>`, so
  with one worktree per run the Worktree lock is per run; it remains as a
  guard against two processes resuming the same run. `--allow-shared-workdir`
  loses its meaning.

### Attempt commits (Q16, Q40)

- **One commit per attempt**, whatever its status: success, fail,
  interrupted. Message `pas(<run-id>): <node> attempt <n> (<status>)` with
  trailers:

  ```
  Pas-Run: <run-id>
  Pas-Node: <node-id>
  Pas-Attempt: <n>
  Pas-Status: success | fail | interrupted
  Pas-Failure-Class: timeout | crash | reported | no_result | launch   (on failure)
  Pas-Session: <agent session id>                                     (agent nodes)
  ```

- `git add -A -- . ':(exclude).pas'`, then `git commit --allow-empty
  --no-verify` (Q40: always skip the project repo's hooks; the graph enforces
  lint and tests). If the repo has no `user.name`/`user.email`, the engine
  uses `-c user.name=PAS -c user.email=pas@localhost`, as Kilroy does
  (`git.go:140-169`).
- The commit is made in the attempt loop right after the handler returns,
  before the result is matched, next to `emit_run_commits`
  (`engine.rs:575-585`), so an attempt that ends in the `Err` path
  (`:635-640`) also gets its commit.
- If the agent committed during the attempt, the engine's commit stacks on
  top (often empty). `CommitsCreated` (`engine.rs:575-585, 653-693`) keeps
  reporting agent commits.
- The trailers make `git log` readable on its own and give a later
  git-derived resume its history (Q43).

### Resume (Q43)

- Resume stays PAS's checkpoint-based resume: re-running the same `pas run`
  continues from `checkpoint.json` under the same run id
  (impl-harmonik-attractor.md §6). The checkpoint also stores the thread
  key → session id map.
- **No reset today:** production code never resets, cleans or checks out
  the workdir on resume; the two `git checkout` calls in `crates/` are in
  tests (`attractor-pipeline/src/run_commits.rs:255`, under `#[cfg(test)]`
  at :130, and `beads_close_tests.rs:563`), and there is no
  `remove_dir_all` in the pipeline or CLI. The
  in-progress node re-runs in the same files. With a worktree per run,
  resume reuses `<worktree_root>/<run-id>` (ordering above), so partial
  work survives; nothing to fix.

### Interrupted attempts (Q29)

An interrupted attempt is one `pas` never finished, because `pas` crashed
or was killed mid-attempt; a timed-out attempt is not interrupted (it gets
its normal attempt commit with `Pas-Status: fail` and class `timeout`).
Partial work is never deleted. On resume, the checkpoint already says which
node and attempt were in progress (it is saved before each attempt,
`engine.rs:541`, with `active_node_id` and `active_node_attempts`,
`checkpoint.rs:16-56`). Before re-running that node, if the worktree has
changes, the engine commits them as the interrupted attempt (`Pas-Status:
interrupted`) and tells the agent: "your previous attempt was interrupted;
its changes since `<last attempt commit>` are recorded in `<interrupted
sha>`; review `git diff <last attempt commit>` before continuing". This is
kept because it is cheap on top of the existing resume (about half a day:
one check of the checkpoint, one commit, one prompt line) and keeps the work
safe even if the re-run agent runs `git checkout .` or `git reset`.

### Cleanup (Q39)

- **Success:** `git worktree remove` the run worktree (it is clean after the
  last attempt commit); keep the branch.
- **Failure:** keep both, for inspection and resume.
- No prune command; later, or an agent cleans up.

### End of a run (Q41, Q44)

The engine merges nothing. At the end it reports `branch`, `base` (ref and
SHA), `final_commit` and `status` in `pas run --json` output and in
`final.json`. The agent or the user merges, opens a PR, or works off an
integration branch. Merge support is listed under section 7.

## 4. Run folder as the central location (Q20)

The run folder is **observability** (Q20): another process reads it to
watch a run, spot a hung agent and read results. The record of what each
attempt did is its attempt commit (section 3); resume uses the checkpoint.

PAS already writes a versioned run folder (test-and-output-pas.md §2). The
design keeps its layout and adds what a reader needs to spot a hung agent
and read results:

```
<logs>/<stem>-<hash>/
  checkpoint.json
  run.lock
  runs/<run-id>/
    run.json                    + worktree, branch, base_sha
    events.jsonl                + LlmStarted, failure_class, commit SHAs
    transcripts/<inv>.jsonl     stdout, live (unchanged)
    transcripts/<inv>.stderr.log  NEW, live (a rate-limit re-spawn adds <inv>.<n>.jsonl / .stderr.log)
    transcripts/<inv>.prompt.txt  NEW, prompt + argv + env names (no values)
    final.json                  NEW, run status, last node, branch, base, final commit, cost
    answers/, control/          unchanged
```

- **New journal event `LlmStarted`** `{invocation_id, spawn, node_id,
  attempt, profile, model, host, pid, pgid, transcript, stderr}`, one per
  spawn, written from the handler's
  `Started` before the agent's first output. With it, a reader can map a
  growing transcript to its node and see the agent's pid.
- **Agent progress:** a reader sees whether a running agent is silent from
  its transcript's modification time and size, matched to `LlmStarted`. No
  new heartbeat field (Q51).
- **`final.json`** marks the end and carries the end-of-run report (section
  3) and, on failure, the clear error (section 5). Per-node results stay in
  the journal (`StageCompleted`/`StageFailed`) and the attempt commits.
- **Contract:** a `docs/run-folder.md` in the fork lists every file, its
  format, when it is written and its version field. New files and events
  carry `v`; readers ignore unknown event types (already true,
  `attractor-journal/src/event.rs:9-11`).
- **Orphans:** `LlmStarted` records the agent's pgid and host, so a person
  or agent can find an agent left running after `pas` was SIGKILLed.
  Automatic reaping is deferred (section 7).

## 5. Failures (Q22, Q47-51)

Rule (Q51): keep what exists if it is good enough; never hide a failure.
The spec's routing rework (crash and timeout as routable FAILs,
`retry_target` for FAIL, conditions on failure class) is deferred to
section 7.

**What stays as today:**
- **Agent-reported error** (`is_error`, or a `subtype` starting `error`,
  section 1): a routable Fail outcome, as today (`codergen_handler.rs:470-527`).
- **Agent timeout, crash, no result or launch failure:** the run stops with
  an error, as today's `Err` path (`engine.rs:635-640`). The engine maps
  `Failed(Timeout | Crash | NoResult | Launch)` from `Agents::run` to that
  path. Timeouts stay retryable up to `max_retries`, as today
  (`attractor-types/src/error.rs:100-110`, `engine.rs:609-625`); the others
  are not retried.
- **Tool nodes:** unchanged. A non-zero exit is a routable Fail with
  `<node>.exit_code` and `failure_reason` "Command exited with code N"
  (`handlers/tool_handler.rs:170-236`); a timeout (default 300 s, `:147-163`)
  or a spawn failure (`:142`) stops the run. That gives success and failure
  routes for commands.

**Clearer errors, only where today's error can't be understood (Q56):**
today a timeout ends with `Command timed out after 120000ms`, naming
neither the node nor where to look, and a crash's stderr now goes to a
file instead of the message. So the run-ending error names the node, the
attempt, the failure class, the handler's `detail` (exit code or signal,
the last stderr lines) and the transcript and stderr paths, for example: `node 'build' attempt 2 failed: timeout after 600 s;
transcript .pas/logs/.../transcripts/<inv>.jsonl; stderr .../<inv>.stderr.log`.
It goes to stderr, `StageFailed`, `PipelineFailed` and `final.json`.

**Two failure-hiding bugs, fixed minimally** (fail the run with a clear
error; no rerouting):
- **(a) First-edge fallback** (`edge_selection.rs:65-75`): when every
  outgoing edge is conditional and none matches, PAS takes the first edge,
  so a Fail can follow a `condition="outcome=success"` edge and the run
  carries on. Fix: return no edge, and in the engine's no-edge branch
  (`engine.rs:1150-1162`) stop the run when the node has outgoing edges but
  none matched, whatever the outcome (for a non-FAIL outcome this
  deliberately diverges from spec lines 390-392, which end the run
  normally; edges that match nothing are a graph mistake to report): `node 'X' outcome fail matched no
  outgoing edge (conditions: …)`. Today that branch errors only for Fail
  and otherwise ends the run as complete. About 0.5 day with tests.
- **(b) Exhausted RETRY** (`engine.rs:589, 627-634`): when a handler returns
  Retry on its last attempt, the outcome leaves the loop still Retry, then
  either ends the run as complete (no edge) or follows an edge. Fix: in the
  attempt loop, an exhausted Retry stops the run with `node 'X' still
  retrying after N attempts`. About 0.25 day with tests.
- **(c) Claude `error_*` subtypes counted as success:** fixed by the
  `claude-p` failure table (section 1).

Agree with the expectation: (a) and (b) are small, both hide failures, and
both should stop the run rather than reroute. Existing tests that assert the
first-edge fallback change; none asserts it directly, so any dependants
show up when the suite runs.

**Rate limits (Q48):** handled inside the handler, not the engine. A
rate-limited attempt is retried by the handler for a short window, set per
profile (`rate_limit_window`, default 2 minutes), then reported as
`Failed(Reported)` with "rate limited" in `detail`. No new failure class.
- `claude-p`: a result with `is_error` and a `rate_limit_event` whose
  `status` isn't `allowed` (or a rate-limit error text) waits until
  `resetsAt` or a backoff, within the window, then re-invokes continuing the
  same session. Each wait and re-spawn is visible: a journalled
  `LlmRateLimited {invocation_id, wait_s}` (through the observer), then a
  new `LlmStarted` with its own pid and its own transcript and stderr
  files.
- `pi`: Pi's own retry (429, 5xx and network errors, 3 tries by default)
  is configured in the engine-written `settings.json` to cover the window
  (exact keys unverified), instead of being turned off.
- `codex-exec` and `gemini`: unchanged from today.

**Sessions (Q50):** a session that can't be continued fails the attempt,
with no fallback to a new session; the error says the session id wasn't
found.

**No idle timeout (Q49):** none exists today (no "idle" in
`attractor-pipeline/src`); the node timeout is the only limit.

Behaviour changes across this design:
- The first-edge fallback is gone (a), and an exhausted Retry stops the run
  (b).
- `llm_provider` keeps working as an alias for `agent=` (section 1).
- Agents run in a new worktree, not the caller's `--workdir`, and the
  worktree is removed on success.
- Every attempt leaves a commit on `pas/run/<run-id>`.
- `.pas/.gitignore` is written into the project.
- Retries and loop-backs continue the agent's session by default; today
  every attempt is fresh (`--no-session-persistence`, `--ephemeral`).
- Session files now persist under `~/.claude` and `~/.codex` (Q27).
- API-key variables are stripped from the agent's environment by default
  (an operator question below); today the environment is inherited
  unchanged (impl-harmonik-attractor.md §7).
- A retry prompt names the previous attempt's failure class and reason.

## 6. Digital twin test harness (Q19)

- **The fake:** `tests/agents/fake-claude`, an executable that accepts
  every `claude` flag the profile passes, reads a scenario from
  `FAKE_AGENT_SCENARIOS` keyed by `PAS_NODE_ID` and `PAS_ATTEMPT`, and
  prints stream-json. Scenarios (test-and-output-pas.md §1): success,
  reported failure, `error_max_turns` with `is_error:false`, label routing,
  crash, crash after result, garbage, silent, hang, slow stream, budget,
  edit and commit, flaky (counter file), and continue: the twin accepts
  `--resume <id>`, echoes that id in its init line, and can fail with
  "session not found" to check the attempt fails (Q50); and rate limited,
  a few times then success, to check the handler's retry window (Q48).
- **A Pi twin:** `tests/agents/fake-pi` prints Pi's `--mode json` JSONL
  (`message_end` with `stopReason`), always exiting 0, with scenarios for
  `stop`, `error` with `errorMessage`, `aborted`, no final message, and a
  session resumed by `--session-id`. It checks that the handler wrote
  `PI_CODING_AGENT_DIR/models.json` and set the `PI_*` env vars.
- **Selection:** a test profile `fake` with `test_only = true`, pointing
  `command` at the script. `Agents::new` refuses it unless `pas run
  --allow-test-agents` is given (Kilroy's `--allow-test-shim` guard,
  `provider_exec_policy.go:41-49`), so it can't run by accident.
- **Path under test:** the fake goes through the real `claude-p` handler and `Agents` (argv, env,
  files, timers, parser) and the real engine (routing, retries, commits,
  resume). Only `argv[0]` differs from production.
- **Failure scenarios (Q47):** a reported error routes on a fail edge; a
  timeout is retried then stops the run; a crash stops the run, and the
  error names the node, attempt, class and transcript and stderr paths; a
  Fail with only non-matching conditional edges stops the run (fix a); a
  Retry on the last attempt stops the run (fix b).
- **Git scenarios:** a crash mid-attempt, then resume, leaves an
  interrupted commit with the partial edits and re-runs on top; every
  attempt has a commit with the right trailers; `.pas/` is never committed;
  on success the worktree is gone and `final.json` reports branch, base and
  final commit.
- **Seams (proposed, to confirm):**
  1. `Agents::run` with the fake: start event, files, failure table, kill.
  2. `pas run` end to end with the fake: the failure scenarios above,
     retries, attempt commits in the worktree, stop and resume, run-folder
     contents.
- **Loop:** vertical slices, one failing test then the code for it,
  starting with "a one-node pipeline with the fake succeeds and leaves an
  attempt commit", then one failure scenario per slice.
- Existing in-process `NodeHandler` doubles stay for engine unit tests.

## 7. What we deliberately don't build

- Handlers beyond `claude-p`, `pi` and the ported `codex-exec` and `gemini`:
  ACP, tmux and remote fit later as their own crates (section 1).
- A generic run-any-program `exec` handler (Q33).
- An ACP handler (OpenCode or Qwen Code over ACP is the multi-model
  runner-up to Pi; multi-model-handler.md).
- Dynamic handler loading (plugins); handlers are compiled in.
- `pas runs prune` worktree cleanup.
- Parallel fan-out and fan-in and manager loops: still rejected at compile
  time. (Fidelity `full` and `thread_id` are built, section 1; only
  `truncate`, `compact` and `summary:*` stay rejected.)
- An HTTP or JSON driver API; the CLI plus the run folder is the driver
  interface (Q20). The Monitor stays as it is.
- Merging the run's work into another branch, `pas merge`, pull requests
  or pushing run branches (Q44). Later; the end-of-run report (section 3)
  gives what a merge needs.
- Git-derived state and resume (Q43, later): an engine-owned ref
  `refs/pas/runs/<run-id>` with a `state.json` (Q42), resume from git
  instead of the checkpoint, detection of history rewritten by an agent
  (the last recorded attempt commit no longer an ancestor of the branch
  tip, saving the rewritten tip and stopping as `diverged`), and a
  `--resume-from-record` repair. The attempt commits' trailers, written now,
  are the history it would start from.
- Sandboxing (decision Q7).
- A queue or one process per work item (Q20, later).
- Planning for `--bare` becoming the `-p` default (Q21).
- Steering or mid-run interaction with an agent: the one-call interface
  has none, which the operator accepted for now (Q26); a single added call
  could provide it later.
- The spec's failure-routing rework (Q47): agent timeouts and crashes as
  routable FAILs, `retry_target`/`fallback_retry_target` for FAIL,
  `failure_class` in conditions. Today they stop the run with a clear error.
- An idle (no-output) timeout (Q49); the node timeout is the only limit.
- Falling back to a new session when one can't be continued (Q50).
- Automatic reaping of agents orphaned by a SIGKILLed `pas`, per-node result
  files, tool-output files, and a `last_output_at` heartbeat field (Q51).
- Spec fidelity modes `truncate`, `compact` and `summary:*`; continuing a
  session (`full`) is built, the others stay rejected (section 1).

## On-disk names (Q45)

The project may be renamed from "pas" later. These on-disk names would need
migrating; nothing changes now:
- the `.pas/` directory (run folders, worktrees, `.pas/.gitignore`);
- the `pas/run/<run-id>` branch prefix (and `refs/pas/runs/` if Q42's ref is
  built later);
- the `Pas-*` commit trailers;
- the `PAS_*` environment variables (`PAS_RUN_ID`, `PAS_NODE_ID`,
  `PAS_ATTEMPT`, `PAS_INVOCATION_ID`, `PAS_SESSION_ID`, `PAS_STATE_DIR`);
- `pas.toml`, `agents.toml` and the `pas-run.lock` file;
- the `pas` binary name and the `attractor-*` crate names.

## Priority (Q55, Q56)

Q56: get work done; refine later. Everything above is needed for that
except these, which are **low priority** and go last in the ticket order:
- the rate-limit retry window inside `claude-p` (Q48); until it lands, a
  rate limit is a reported error, as today;
- the failure class and reason in a retry prompt (Q55);
- the `<inv>.prompt.txt` file;
- `docs/run-folder.md`.

## Rough effort

| Piece | Days |
|---|---|
| Interface crate and registry, process-runner crate, `claude-p` handler, Codex and Gemini ported as handler crates, config loader, aliases, engine wiring, `LlmProvider` removal | 7-10 |
| Worktree, branch, attempt commits with trailers, prepare_run reorder, cleanup, interrupted-attempt commit | 2.5-3.5 |
| Run-folder additions (`LlmStarted`, stderr and prompt files, `final.json`) and `docs/run-folder.md` | 1.5-2 |
| Failures: clear error messages, fixes (a) and (b), rate-limit window in `claude-p` | 1-1.5 |
| Sessions: new/continue, fidelity and thread keys in the checkpoint, retry prompt | 1.5-2.5 |
| Pi handler (config dir, models.json, failure table) | 2 |
| Digital twins (claude and Pi) and end-to-end tests | 2-4 |
| **Total** | **17.5-25.5** |

Estimates come from the research files' per-item figures plus the handler-crate
extraction; none is measured. Q51 cut orphan reaping, per-node and
tool-output files, the heartbeat field, the session fallback and the
routing rework.

## Operator questions

Q51's rule (keep what exists if it is good enough; do less now) settles
most of the remaining questions. They are listed as pre-answered; the
operator only needs to object.

**Answered:** 1-3 (Q31-35), 8-13 (Q29, Q36-40), 14 (Q47), 15 (Q47-48),
16 (Q49), 18-19 (Q27-28), 21 (Q50), 22 in part (Q30), 27 (Q46), 28 (Q43),
29 (Q44).

**Pre-answered by Q51** (object to change):
4. **Config location:** profiles in the existing `pas.toml` (`[agents.*]`),
   on top of the defaults embedded in the binary; no separate file. A
   `--agents <file>` flag only if a driver needs it later.
5. **Safety flags:** profile `args` are editable (Q13: config, passed
   through); the default profiles keep today's flags
   (`--dangerously-skip-permissions`, `--safe-mode`, `--yolo`).
6. **Environment:** inherit the caller's environment, with no allowlist
   mode (as today). *Still open:* the strip list of API-key variables is new
   behaviour. It stops a stray key from switching a subscription to API
   billing, but someone who deliberately runs Claude on
   `ANTHROPIC_API_KEY` would silently move to the subscription unless their
   profile sets the key back (`set`). Strip by default, or inherit
   unchanged as today?
7. **Compatibility:** `llm_provider` and `[codergen.claude]` keep working,
   with no deprecation (section 1).
15. **Retry defaults:** as today: timeouts retry up to `max_retries`;
    crashes and no-result don't retry; rate limits are retried inside the
    handler (Q48).
17. **Run folder location:** as today, `.pas/logs/...` relative to the
    `pas` process's directory; a driver passes `--logs` to pin it.
    Worktrees anchor to the project root instead, so a run started from a
    subdirectory creates two `.pas/` folders; pas writes `.pas/.gitignore`
    with `*` in each `.pas/` it creates, so both stay ignored.
20. **Test seams and fake:** the two seams in section 6; the fakes are
    shell scripts, as PAS's existing stub tests are
    (`codergen_handler_tests.rs:564-570`).
22. **Opt-out name:** `fidelity=fresh`.
23. **Thread key:** each node continues its own session; nodes share one
    only when the graph sets `thread_id`.
24. **Retry prompt:** the engine adds the failure class and reason, nothing
    more (new behaviour, but minor); the graph's own prompt text covers the
    rest.
25. **Handler registration:** all handlers always compiled in; no cargo
    features.
26. *Answered (Q52): no pinning; the API key goes in the per-invocation
    `models.json` (0600).*

**Answered at sign-off:** 6 (Q54: strip API keys by default; a token is
supplied in the profile on purpose), 20 (Q53: the two seams, shell-script
fakes), 24 (Q55: fine, low priority). Q56 trimmed the scope (below).

**Still open:** none. Section 6's test seams follow the tdd rule of
agreeing seams first; the captain may confirm them with the operator.
