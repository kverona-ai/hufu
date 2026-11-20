# Adapter Contract

| | |
|---|---|
| **Status** | Describes Hufu 1.0 as implemented. Per-agent details follow each vendor's documentation as read on 2026-11-20, checked against the installed Codex, Claude Code and Antigravity CLIs on one machine (help and model lists only); no job has been run against real accounts by this project |
| **Last updated** | 2026-11-20 (`xhigh`; `models()` and `hufu models`; `cursor-agent`; `agy acp` and `--mode accept-edits`; Claude Code `--permission-prompts none`; Kimi print-mode resume) |
| **Related** | [Agent landscape](../research/agent-landscape.md), [Technical design](technical-design.md), [ADR-0002](../adr/0002-acp-first-cli-fallback.md), [ADR-0005](../adr/0005-explicit-permission-and-effort-mapping.md), [ADR-0009](../adr/0009-synchronous-core-and-own-protocol-clients.md) |

An adapter is the only code that knows how one kind of agent is driven. This document defines what
an adapter must do, how Hufu's uniform permission tiers and effort levels map onto each agent, and
the per-agent notes.

The evidence marks (✅ ◐ ⚠️ ❓) are those defined in the
[agent landscape](../research/agent-landscape.md). The quickest way to see what Hufu will run for
your own profile is `hufu run -a <profile> -p <tier> -e <effort> --dry-run "x"`, and the quickest
way to see which models and levels the agent itself offers is `hufu models <profile>`.

## 1. Interface

The contract lives in the `hufu-contract` crate (`src/crates/hufu-contract/src/types.rs`). It is
synchronous: `run` blocks its thread until the agent is done.

```rust
pub trait Adapter: Send + Sync {
    /// The adapter kind used in configuration, for example "codex-cli".
    fn kind(&self) -> &'static str;

    /// The command used when the profile gives none.
    fn default_command(&self) -> Vec<String>;

    /// The variable that points the agent at its configuration and login directory, if it has
    /// one. A profile's `home` is passed through it, which is how accounts are kept apart.
    fn home_var(&self) -> Option<&'static str> { None }

    /// Is the agent installed? Which version? Is it logged in, if that can be told without
    /// starting a model? Must not prompt.
    fn probe(&self, profile: &Profile) -> ProbeResult;

    /// What this profile can do. Static: must not start the agent.
    fn capabilities(&self, profile: &Profile) -> Capabilities;

    /// Builds the invocation without running it. This is what `--dry-run` prints and what the
    /// conformance suite inspects.
    fn plan(&self, profile: &Profile, req: &RunRequest) -> Result<LaunchPlan, AdapterError>;

    /// Runs one prompt to completion, emitting normalized events to `sink` as they happen.
    /// Must stop the agent when `cancel` fires. Agent-side failures are not errors of this
    /// call; they are returned in the outcome.
    fn run(&self, profile: &Profile, req: &RunRequest, sink: &dyn EventSink, cancel: &CancelToken)
        -> RunOutcome;

    /// The models the agent offers this profile, asked from the agent itself where it can say
    /// (`hufu models`). Runs only a listing command or an ACP handshake, never a prompt. The
    /// default answer is an error saying the adapter cannot list models.
    fn models(&self, profile: &Profile, env: &BTreeMap<String, String>, cwd: &Path)
        -> Result<ModelList, AdapterError>;
}

pub struct Capabilities {
    pub permission_tiers: Vec<Tier>,          // tiers this profile can truly enforce
    pub efforts: Vec<Effort>,                 // levels with a native mapping; `default` always works
    pub native_effort: bool,                  // is `native:<value>` passed through?
    pub checked_at_runtime: bool,             // effort and model only known once a session is open (ACP)
    pub models: Vec<String>,                  // known ids; empty means "not known ahead of time"
    pub resume: bool,                         // can a later prompt continue this session?
    pub structured_output: StructuredOutput,  // Native | Instructed | None
    pub reports_usage: bool,
    pub reports_denials: bool,                // can skipped actions be detected?
    pub accepts_mcp_servers: bool,            // can Hufu attach an MCP server to one run?
    pub provides: Vec<String>,                // capabilities beyond code tools
    pub notes: Vec<String>,                   // caveats shown by `hufu profile show`
}

pub struct RunRequest {
    pub job_id: String,
    pub prompt: String,
    pub cwd: PathBuf,
    pub permission: Tier,                     // always explicit
    pub effort: Effort,                       // Effort::Default means: pass nothing
    pub model: Option<String>,
    pub output_schema: Option<serde_json::Value>,
    pub resume_session: Option<String>,
    pub env: BTreeMap<String, String>,        // already filtered by the runner
    pub mcp_servers: Vec<McpServerSpec>,      // servers Hufu attaches, if any
    pub scratch_dir: PathBuf,                 // the job's directory, for files the agent needs
}

pub struct RunOutcome {
    pub status: RunStatus,                    // Completed | Failed | Cancelled
    pub reason: Option<String>,               // a reason code, section 2.1
    pub detail: Option<String>,
    pub final_message: Option<String>,
    pub structured_output: Option<serde_json::Value>,
    pub denied_actions: Vec<DeniedAction>,
    pub session: Option<SessionRef>,          // id and whether it can be resumed
    pub usage: Option<Usage>,
    pub exit_code: Option<i32>,
    pub retryable: bool,
    pub touched_paths: Vec<String>,           // paths the agent said it changed
}

pub struct ModelList {
    pub source: String,                       // agent | config | documentation
    pub from: String,                         // the command run, file read or page copied
    pub models: Vec<ModelInfo>,               // id, name, efforts, default_effort, default, description
    pub efforts: Vec<String>,                 // agent values for every model, when stated once
    pub notes: Vec<String>,
}
```

`hufu-adapters/src/common.rs` holds what the CLI adapters share: resolving the program, building
the environment, running the process with line-by-line output, collecting an outcome, recognising
login and rate-limit messages, and the prompt suffix that asks an agent for JSON.

### 1.1 Third-party adapters

An adapter can also be a program outside Hufu (`adapter: external`, `command: [my-adapter]`). It
is called with one of three subcommands and reads one JSON document from stdin:

| Call | stdin | stdout |
|---|---|---|
| `<cmd> probe` | `{"v":1,"profile":{...}}` | one `ProbeResult` object |
| `<cmd> capabilities` | `{"v":1,"profile":{...}}` | one `Capabilities` object |
| `<cmd> run` | `{"v":1,"profile":{...},"request":{...}}` | JSON Lines: normalized events, then one `{"type":"outcome", ...}` line |

The shapes are the serialized forms of the types above. Hufu still validates the task against the
declared capabilities, stops the program's process tree on cancel, and applies rule 4 below. The
fake agent `hufu __fake-agent external` is a complete example.

## 2. Rules every adapter follows

1. **State the tier explicitly.** Build the invocation so that the requested tier is spelled out in
   flags or in ACP permission answers. Never rely on what the agent does by default.
2. **Refuse what cannot be honoured.** If the tier, effort level or resume cannot be delivered,
   `capabilities()` must not advertise it and `plan()` must return an error. Hufu refuses the task
   before launch.
3. **Never widen.** No adapter may run an agent at a higher tier than requested, for any reason.
   `args` from the profile that would change the tier are refused.
4. **Do not trust exit codes alone.** A run counts as completed only if a final message was
   produced. An exit code of `0` with no message is `failed` with reason `empty_output`.
5. **Report denials.** Where the agent says it skipped an action for lack of permission, emit
   `permission.denied` and list it in the outcome.
6. **Close stdin** unless the protocol needs it, and never attach the caller's terminal.
7. **Set the working directory** by flag where the agent has one, and always as the child process's
   cwd as well.
8. **Record the version.** `probe()` reports the agent version, and it is stored with every job.
9. **Stop cleanly.** On cancel: ask the agent to stop, then let the process group be terminated.
10. **Keep the raw output.** Everything the agent wrote goes to the job's `raw/` directory
    untouched, in addition to normalized events. A line that cannot be parsed is skipped, not fatal.
11. **Never touch credentials.** An adapter may check that a login file exists or ask the agent's
    own status command; it never reads, copies or logs a credential.

### 2.1 Failure reasons

| Reason | Meaning | Retried or failed over |
|---|---|---|
| `agent_unavailable` | The program is not installed or could not be started | Next pool member |
| `auth_required` | Not logged in, or a required variable is not set | Next pool member |
| `rate_limited` | The agent reported a rate or quota limit | Next pool member; retried by default |
| `agent_error` | The agent reported an error or exited non-zero | Only if listed in `retry.on` |
| `empty_output` | The agent ended without a final message | 〃 |
| `invalid_output` | The answer does not match the requested schema | 〃 |
| `protocol_error` | An ACP or adapter-protocol exchange went wrong | 〃 |
| `refusal`, `max_tokens`, `max_turn_requests` | ACP stop reasons other than a normal end | 〃 |
| `unsupported` | The request cannot be expressed for this agent (caught at validation in normal use) | No |
| `permission_denied` | `strict_permissions` and an action was denied | No |
| `tier_violated` | A read-only job changed files | No |
| `timeout`, `inactivity_timeout` | Hufu's timers; the job ends `timed_out` | No |
| `cancelled` | Asked to stop; the job ends `cancelled` | No |
| `dependency_failed`, `workspace_error`, `runner_died`, `internal_error` | Set by the runner, not by adapters | `runner_died` by the daemon, see the technical design |

## 3. Permission tiers

| Tier | Intent |
|---|---|
| `read-only` | Read and search the workspace. No file changes, no commands with side effects |
| `workspace-write` | Also create, edit and delete files inside the workspace. Commands limited to what the agent's own sandbox or allow-list permits |
| `full-access` | Everything the agent can do, with no approval prompts |

### 3.1 Mapping

| Agent (adapter) | `read-only` | `workspace-write` | `full-access` |
|---|---|---|---|
| **Codex** (`codex-cli`) ◐ | `--sandbox read-only` | `--sandbox workspace-write` | `--dangerously-bypass-approvals-and-sandbox` |
| **Antigravity** (`agy-cli`) ✅ | `--mode plan --sandbox` | `--mode accept-edits --sandbox`: workspace edits are applied, commands need an allow rule in agy's own settings or are soft-denied | `--dangerously-skip-permissions` |
| **Cursor** (`cursor-cli`) ✅ | `--mode plan` | Print mode without `--force`: workspace edits applied, other actions rejected. Refused when `cli-config.json` sets `approvalMode: unrestricted` | `--force` |
| **Claude Code** (`claude-cli`) ✅ | `--permission-mode plan --permission-prompts none` | `--permission-mode acceptEdits --permission-prompts none`: file edits allowed, other tools need Claude Code's own allow rules | `--dangerously-skip-permissions` |
| **Kimi Code** (`kimi-cli`) ✅ | Not available | Not available | Print mode, which approves every tool call that `[[permission.rules]]` do not deny |
| **pi** (`pi`) ✅ | `--tools read --no-mcp`: only pi's `read` tool is offered | Not available: pi cannot confine edits to the workspace | No restriction: pi has no permission system |
| **ACP agent** (`acp`), `verified` | Allow `read`, `search`, `think`; reject the rest | Also allow `edit`, `delete`, `move` for paths inside the workspace, and `execute` for commands starting with a prefix in `allow_execute` | Allow every request |
| **ACP agent** (`acp`), not verified | Not available | Not available | Allow every request |

Notes:

- **"Not available" is a hard stop.** A task asking Kimi's print mode for `read-only` is refused
  with a message naming the tiers that profile does support. Use `kimi-acp`, worktree isolation or
  a container.
- **Hufu checks afterwards, too.** Whatever the agent's own controls did, a `read-only` job whose
  workspace changed fails with `tier_violated`. That check does not depend on the agent.
- **ACP answers are only as good as the questions.** An ACP agent decides which actions to ask
  about. If it never asks before editing, Hufu cannot stop it. A generic ACP profile therefore
  advertises `read-only` and `workspace-write` only when it is marked
  `permission_requests: verified`, after someone has checked that agent. Every permission request
  is answered with a once-only option, never "always", and an `execute` request whose command text
  is not given is rejected at `workspace-write`.
- **The middle tier of Cursor, Antigravity and Claude Code is the agent's own boundary**, not
  Hufu's. Rejections inside it surface as denied actions.

### 3.2 Denied actions

| Agent | How denials are detected |
|---|---|
| Codex ◐ | Failed or declined command items in the `--json` event stream |
| Cursor ✅ | `tool_call` events that complete as rejected |
| Antigravity ✅ | Step events whose tool error is a permission denial; the process still exits `0`, so this check is mandatory |
| Claude Code ◐ | `permission_denials` in the result event |
| ACP | Hufu's own `reject` answers |
| Kimi print mode, pi | Not applicable: nothing is ever denied |

Denied actions are listed in the result. With `strict_permissions: true` any of them fails the job
with reason `permission_denied`; the default is `false`.

## 4. Effort mapping

Hufu's scale is `off`, `low`, `medium`, `high`, `xhigh`, `max`, plus `default`, which means "pass
nothing and let the agent decide". `xhigh` joined the scale in 1.0, when Codex, Claude Code,
Antigravity, Kimi Code and pi had all added it between `high` and `max`.

| Hufu | Codex ✅ `-c model_reasoning_effort=` | Antigravity ✅ `--effort` | Claude Code ✅ `--effort` | Kimi ✅ `KIMI_MODEL_THINKING_EFFORT` | pi ✅ `--thinking` | Cursor | ACP |
|---|---|---|---|---|---|---|---|
| `off` | n/a | n/a | n/a | n/a | `off` | n/a | The session's `thought_level` option, if advertised, matched by value |
| `low` | `low` | `low` | `low` | `low` | `low` | n/a | 〃 |
| `medium` | `medium` | `medium` | `medium` | `medium` | `medium` | n/a | 〃 |
| `high` | `high` | `high` | `high` | `high` | `high` | n/a | 〃 |
| `xhigh` | `xhigh` | `xhigh` | `xhigh` | `xhigh` | `xhigh` | n/a | 〃 (also `extra-high`, `extra_high`) |
| `max` | `max` | `max` | `max` | `max` | `max` | n/a | 〃 (also `maximum`) |
| `default` | (omit) | (omit) | (omit) | (unset) | (omit) | (omit) | (leave unset) |

- **n/a means an error**, not a silent skip. A task asking Codex for `off` fails validation.
- A profile may set `effort_fallback: nearest` to accept the closest supported level instead (a tie
  goes to the lower level). The substitution is then recorded as a warning in the result.
- **The agent's levels differ per model.** The table says what Hufu passes; whether the model
  accepts it is the agent's business. Codex refuses a level its model lacks; Claude Code, pi and
  Antigravity fall back to a level the model has. `hufu models <profile>` shows each model's
  levels where the agent lists them.
- Native levels beyond Hufu's scale (Codex `ultra`, which hands the work to subagents; Claude
  Code `ultracode`; pi `minimal`) are reached with `effort: native:<value>`, passed through verbatim
  and validated only by the agent.
- Kimi's variable applies only to a model the profile defines with `KIMI_MODEL_NAME`. Other Kimi
  print-mode profiles have no per-run effort: the level comes from `[thinking] effort` in Kimi's
  `config.toml`, or use `kimi-acp`, which sets it per session.
- Cursor has no effort control; reasoning strength is selected through the model id.
- For ACP the levels are only known once a session is open, so a level the agent does not offer
  fails the job at start rather than at submission.

## 5. Per-agent notes

### 5.1 Codex CLI

Evidence: ◐ vendor documentation and `codex-rs/exec` sources.

```text
codex exec --cd <cwd> --sandbox <tier> [-m <model>] [-c model_reasoning_effort="<level>"] \
  --json --color never [--skip-git-repo-check] [--output-schema <file>] -o <last-message file> -
```

- The prompt is written to stdin (`-`), which is then closed.
- With `--json`, stdout is JSON Lines: `thread.started`, `turn.*`, `item.*`, `error`. The final
  message is also read from the `-o` file, which is removed before each run so that a stale one is
  never mistaken for an answer. A turn that failed fails the job even when the exit code is `0`.
- The default sandbox for `codex exec` is read-only. The adapter still passes the flag every time.
- Requires a Git repository unless `--skip-git-repo-check` is given; the adapter passes that flag
  only when the workspace is not a repository and says so in a note.
- Resume: `codex exec resume <SESSION_ID>`. Structured output: `--output-schema`.
- An attached MCP server is passed as `-c mcp_servers.<name>.command=...` overrides.
- Since Codex 0.169, `codex exec` keeps project configuration and project MCP servers off in a
  directory it does not trust. Trust it once with `codex exec --trust`, or add `--trust` to the
  profile's `args`.
- Models: `hufu models` runs `codex debug models` and lists the catalog's visible models with their
  effort levels and default level. `gpt-6.2-sol` is Codex's default; `gpt-6.2-astra` is meant for
  demanding work.
- `--approve-for-me` and `--not-so-yolo` change approvals and are refused in a profile's `args`.
- Account: `home` becomes `CODEX_HOME`. Auth: saved login, or `CODEX_API_KEY` if the profile passes
  it through.

### 5.2 Antigravity (`agy`) CLI

Evidence: ✅ vendor documentation.

```text
agy -p <prompt> --output-format stream-json [--mode plan|accept-edits --sandbox | --dangerously-skip-permissions] \
  [--model <slug>] [--effort <level>] [--json-schema <file>] [--conversation <id>]
agy acp                                                      # preset `agy-acp`
```

- Two surfaces: the headless CLI (preset `agy`) and, since `agy acp` appeared, ACP (preset
  `agy-acp`) when permission answers per action are wanted.
- Every tier names its mode, because the mode a run gets without `--mode` follows the user's
  `agentMode` setting. `--settings` can widen permissions per run and is refused in `args`.
- Models: `hufu models` runs `agy models` (one `slug<TAB>name` line each). A slug that ends in a
  level (`gemini-4-neon-high`) already picks the effort; `--effort` switches the current model to
  its variant, `xhigh` and `max` only where the model publishes them, and Orchestrator models take
  none.
- **No cwd flag.** The adapter sets the child process's working directory.
- **stdin is closed.** An open stdin pipe is reported to hang some versions.
- **Empty stdout without a TTY** is a reported bug
  ([#76](https://github.com/google-antigravity/antigravity-cli/issues/76)) ⚠️. The adapter treats
  "exit 0, no result event" as `empty_output`. Hufu does not run `agy` under a pseudo-terminal; if
  an installed version is affected, the job fails clearly and upgrading `agy` is the fix.
- **Soft denial.** Without `--dangerously-skip-permissions`, actions needing approval are skipped
  and the process exits `0`. Denial detection is mandatory.
- **No default timeout.** Hufu's own timers apply.
- Unknown model ids exit non-zero; they do not fall back.
- Resume: `--conversation <id>` using `conversation_id` from the result.
- Auth: cached login from an interactive session. If absent, the run fails with `auth_required`.
- `agy` has no documented variable for its configuration directory, so `home` is not available;
  a second account needs a variable the vendor documents, passed with `vars` or `env_from`.
- Open question on terms of use for subscription accounts: see the
  [agent landscape](../research/agent-landscape.md#34-antigravity-cli-agy).

### 5.3 Cursor

Evidence: ✅ vendor documentation.

```text
cursor-agent -p --output-format stream-json --workspace <cwd> --trust [--mode plan | --force] \
  [--model <id>] [--resume <chatId>] <prompt>
```

- **The program is `cursor-agent`.** Cursor installs the same binary as `agent` too, but other
  CLIs (the Grok CLI) also install an `agent`. Without a `command` in the profile, Hufu runs
  `cursor-agent` when it is on PATH and otherwise `agent`, and `probe` reports an `agent` whose
  `--version` names another CLI as not installed.
- Both surfaces work: the headless CLI (preset `cursor`), and ACP (`cursor-agent acp`, preset
  `cursor-acp`) when permission answers per action are wanted.
- `workspace-write` is refused when Cursor's `cli-config.json` sets `approvalMode: unrestricted`,
  which makes print mode run everything as `--force` does.
- Stream events: `system`, `user`, `assistant`, `tool_call`, `result`. Thinking is not emitted.
  The `result` event's `usage` (with `reasoningTokens`) is reported.
- Structured output is by instruction. Models: `hufu models` runs `cursor-agent models`.
- Account: `home` becomes `CURSOR_CONFIG_DIR`. Auth: `CURSOR_API_KEY` if passed through, or a saved
  `cursor-agent login`.

### 5.4 Kimi Code

Evidence: ✅ vendor documentation.

```text
kimi -p <prompt> --output-format stream-json [-m <model>]      # preset `kimi`, full-access only
kimi acp                                                       # preset `kimi-acp`
```

- Print mode always approves, so it supports only `full-access`. ACP is the way to lower tiers.
- Effort is set through `KIMI_MODEL_THINKING_EFFORT`, which Kimi reads only for a model defined by
  `KIMI_MODEL_NAME`. A profile that does not define its model that way offers no effort levels:
  Kimi takes the level from `[thinking] effort` in its `config.toml`.
- The stream ends with a `session.resume_hint` line carrying the session id; a follow-up runs
  `kimi -S <id> -p ...`. Exit code `4` (session held by another process) fails the job with a
  clear message.
- Print mode reads MCP servers only from its own settings file; over ACP Hufu attaches them.
- Models: `hufu models` reads the `[models."<alias>"]` tables and `default_model` of Kimi's
  `config.toml`, with each model's `support_efforts`.
- `kimi acp` not logged in answers `AUTH_REQUIRED` (`-32000`), mapped to `auth_required` with the
  hint `kimi login`.
- Account: `home` becomes `KIMI_CODE_HOME`.

### 5.5 pi

Evidence: ✅ vendor documentation.

```text
pi --mode json [--tools read --no-mcp] [--provider <name>] [--model <id>] [--thinking <level>] \
  [--session <id>] <prompt>
```

- pi is Hufu's route to open-API models. A profile names a provider and a model; the API key
  reaches pi through an environment variable the profile lists in `env` or renames with `env_from`.
  `hufu doctor` knows the usual variable of each common provider and says when it is not set.
- `full-access` is unrestricted; pi's own advice is to run it in a container. `read-only` limits pi
  to its `read` tool.
- Non-interactive modes show no project trust prompt.
- `--thinking` with a level the model lacks is clamped by pi to one it has.
- Models: `hufu models` runs `pi --list-models <provider>`.
- Resume: `--session <id>`. Account: `home` becomes `PI_CODING_AGENT_DIR`.
- pi itself requires Node.js ≥ 22.19. Hufu does not.

### 5.6 Claude Code

Evidence: ✅ vendor documentation and `claude --help` (2.1.328).

```text
claude -p --output-format stream-json --verbose \
  (--permission-mode <plan|acceptEdits> --permission-prompts none | --dangerously-skip-permissions) \
  [--model <alias or id>] [--effort <level>] [--json-schema <schema>] [--resume <id>] [--mcp-config <json>]
```

- `--permission-prompts none` needs Claude Code 2.1.259 or later; it makes a call that would need
  approval a denial instead of a prompt, whatever mode a print-mode run would otherwise start in.
- Models: an alias (`best`, `fable`, `opus`, `sonnet`, `haiku`, `opus[1m]`, `sonnet[1m]`,
  `opusplan`) or a full id. Claude Code cannot list models, so `hufu models` shows Hufu's copy of
  its model configuration page.

- The prompt is written to stdin. Present so that Claude Code can be an executor, or the advisor of
  a mission, under any caller.
- Login is checked with `claude auth status`. Account: `home` becomes `CLAUDE_CONFIG_DIR`. Auth:
  saved login, or `ANTHROPIC_API_KEY` if the profile passes it through.
- Denials come from the result event; usage and cost are reported.

### 5.7 Generic ACP

```yaml
profiles:
  my-agent:
    adapter: acp
    command: [my-agent, acp]        # whatever starts the agent in ACP mode
    permission_requests: unverified # tiers below full-access are not advertised until verified
    allow_execute: ["npm test"]     # commands allowed at workspace-write, by prefix
```

The client is written for Hufu ([ADR-0009](../adr/0009-synchronous-core-and-own-protocol-clients.md)).
Flow:

1. Spawn `command` with pipes; speak newline-delimited JSON-RPC 2.0.
2. `initialize` with `protocolVersion: 1`. Advertise no file-system or terminal client
   capabilities, so the agent uses its own tools.
3. `authenticate` only with a non-interactive method named by the profile's `auth_method`. Hufu
   never performs an interactive login; an agent that needs one fails with `auth_required`.
4. `session/new` with the workspace as `cwd` and any MCP servers Hufu attaches, or `session/load`
   / `session/resume` for a follow-up, depending on what the agent advertises.
5. Read `configOptions`. Set the option of category `model` and the option of category
   `thought_level` with `session/set_config_option` when the task asks for them. An option without
   a category is matched by its id (`model`; `thinking`, `effort`, `reasoning_effort` or
   `reasoning` for the thought level). Where the agent advertises a plan-like mode, select it for
   `read-only` as a second control.
6. `session/prompt`. Translate `session/update` notifications into normalized events; chunks of the
   answer are joined into the final message.
7. Answer every `session/request_permission` from the tier policy in [section 3.1](#31-mapping).
8. Map `stopReason`: `end_turn` is completed; `cancelled` is cancelled; `refusal`, `max_tokens` and
   `max_turn_requests` are failed with the reason recorded.
9. On cancel: send `session/cancel`, give the agent the grace period, then stop the process tree.

`hufu models` runs steps 1 to 4 without a prompt and lists the values of the `model` option (or the
older `models` field of the `session/new` answer) and of the `thought_level` option, then ends the
agent.

Agents expected to work through this profile with no adapter code: Gemini CLI, OpenCode, Goose,
Qwen Code, Cursor, Kimi Code, Antigravity (`agy acp`), pi (through `pi-acp`), Codex (through
`codex-acp`), Claude (through `claude-agent-acp`). None of them has been run against this client
yet.

## 6. Conformance suite

Every adapter must pass the same checks, run against a fake of its agent
(`hufu-testing/src/conformance.rs`; the fakes are in the same crate):

| # | Assertion |
|---|---|
| C1 | The invocation contains an explicit tier flag or policy for every supported tier |
| C2 | A tier not in `capabilities().permission_tiers` is never launched |
| C3 | An effort level not in `capabilities().efforts` is never launched, unless fallback is enabled, in which case a warning is emitted |
| C4 | Exit `0` with no final message yields `failed / empty_output` |
| C5 | A transcript containing a skipped action yields a `permission.denied` event and a non-empty `denied_actions` |
| C6 | Abort stops the fake agent within the grace period and yields `cancelled` |
| C7 | A hanging agent is ended by the runner's timeout and no child process survives |
| C8 | Missing login yields `failed / auth_required` with a hint, and no hang |
| C9 | Malformed output lines are logged as warnings and do not crash the adapter |
| C10 | Raw output is preserved byte for byte |
| C11 | `resume_session` reaches the agent as its native resume mechanism, or the capability is absent |
| C12 | No environment variable outside the request's `env` reaches the agent |
