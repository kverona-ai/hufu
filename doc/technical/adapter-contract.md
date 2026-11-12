# Adapter Contract

| | |
|---|---|
| **Status** | Draft for review. Nothing here is implemented yet |
| **Last updated** | 2026-11-12 (interface rewritten as a Rust trait) |
| **Related** | [Agent landscape](../research/agent-landscape.md), [Technical design](technical-design.md), [ADR-0002](../adr/0002-acp-first-cli-fallback.md), [ADR-0005](../adr/0005-explicit-permission-and-effort-mapping.md) |

An adapter is the only code that knows how one kind of agent is driven. This document defines what
an adapter must do, how Hufu's uniform permission tiers and effort levels map onto each agent, and
the per-agent notes an implementer needs.

The evidence marks (✅ ◐ ⚠️ ❓) are those defined in the
[agent landscape](../research/agent-landscape.md). Anything not marked ✅ must be confirmed against
the installed CLI before its adapter ships.

## 1. Interface

The contract lives in the `hufu-contract` crate. Sketch:

```rust
#[async_trait]
pub trait Adapter: Send + Sync {
    /// Adapter kind, e.g. "acp", "codex-cli".
    fn kind(&self) -> &'static str;

    /// Is the agent installed? Which version? Is it logged in, if that can be told cheaply?
    async fn probe(&self, profile: &ResolvedProfile) -> ProbeResult;

    /// What this profile can do. Used to validate a task before anything runs.
    async fn capabilities(&self, profile: &ResolvedProfile) -> Result<Capabilities, AdapterError>;

    /// Run one prompt to completion. Emit normalized events to `sink` as they happen.
    /// Must stop the agent when `cancel` fires. Agent-side failures are not errors of
    /// this call; they are returned in the outcome.
    async fn run(
        &self,
        request: RunRequest,
        sink: &dyn EventSink,
        cancel: CancellationToken,
    ) -> RunOutcome;
}

pub struct Capabilities {
    pub permission_tiers: Vec<PermissionTier>, // tiers this profile can truly honour
    pub efforts: Vec<Effort>,                  // levels with an exact native mapping
    pub models: ModelSupport,                  // List(ids) | Any (passed through) | Unknown
    pub resume: bool,                          // can a later prompt continue this session?
    pub structured_output: bool,               // can the agent be held to a JSON Schema?
    pub reports_usage: bool,
    pub reports_denials: bool,                 // can skipped actions be detected reliably?
    pub accepts_mcp_servers: bool,             // can Hufu attach an MCP server to the session?
    pub provides: Vec<Capability>,             // e.g. ComputerUse (planned)
}

pub struct RunRequest {
    pub prompt: String,
    pub cwd: PathBuf,
    pub permission: PermissionTier,            // always explicit
    pub effort: Effort,                        // Effort::Default means: pass nothing
    pub model: Option<String>,
    pub output_schema: Option<serde_json::Value>,
    pub resume_session: Option<String>,
    pub env: BTreeMap<String, String>,         // already filtered by the runner
    pub mcp_servers: Vec<McpServerSpec>,       // servers Hufu attaches, if any
}

pub struct RunOutcome {
    pub status: RunStatus,                     // Completed | Failed | Cancelled
    pub reason: Option<FailureReason>,         // e.g. AuthRequired, EmptyOutput
    pub final_message: Option<String>,
    pub structured_output: Option<serde_json::Value>,
    pub denied_actions: Vec<DeniedAction>,
    pub session: Option<SessionRef>,           // id and whether it can be resumed
    pub usage: Option<Usage>,
}
```

## 2. Rules every adapter follows

1. **State the tier explicitly.** Build the invocation so that the requested tier is spelled out in
   flags or in ACP permission answers. Never rely on what the agent does by default.
2. **Refuse what cannot be honoured.** If the tier, effort level, model or structured output cannot
   be delivered, `capabilities()` must not advertise it. Hufu then rejects the task before launch.
3. **Never widen.** No adapter may run an agent at a higher tier than requested, for any reason.
4. **Do not trust exit codes alone.** A run counts as completed only if a final message was
   produced. An exit code of `0` with no message is `failed` with reason `empty_output`.
5. **Report denials.** Where the agent says it skipped an action for lack of permission, emit
   `permission.denied` and list it in the outcome.
6. **Close stdin** unless the protocol needs it, and never attach the caller's terminal.
7. **Set the working directory** by flag where the agent has one, and always as the child process's
   cwd as well.
8. **Check the version.** `probe()` records the agent version. An adapter may refuse versions it is
   known not to work with.
9. **Stop cleanly.** On abort: ask the agent to stop, then let the runner terminate the process
   group.
10. **Keep the raw output.** Everything the agent wrote goes to the job's `raw/` directory
    untouched, in addition to normalized events.

## 3. Permission tiers

| Tier | Intent |
|---|---|
| `read-only` | Read and search the workspace. No file changes, no commands with side effects |
| `workspace-write` | Also create, edit and delete files inside the workspace. Commands limited to what the agent's own sandbox or allow-list permits |
| `full-access` | Everything the agent can do, with no approval prompts |

### 3.1 Mapping

| Agent (surface) | `read-only` | `workspace-write` | `full-access` |
|---|---|---|---|
| **Codex** (CLI) ◐ | `--sandbox read-only` | `--sandbox workspace-write` | `--dangerously-bypass-approvals-and-sandbox` |
| **Cursor** (CLI) ✅ | `--mode plan` | Print mode without `--force`: workspace edits applied, other actions rejected | `--force` |
| **Antigravity** (CLI) ✅ | No allow rules, `--sandbox`; actions needing approval are soft-denied | Conditional: needs `permissions.allow` rules in settings. Advertised only when the profile declares them | `--dangerously-skip-permissions` |
| **Kimi Code** (CLI) ✅ | Not available | Not available | `--print` (implies `--afk`) |
| **Kimi Code** (ACP) ⚠️ | By permission answers; to be verified | By permission answers; to be verified | Allow all |
| **pi** (CLI, RPC) ✅ | Not available | Not available | Only mode: pi has no permission system |
| **Generic ACP agent** | Allow `read`, `search`, `think`; reject the rest | Also allow `edit`, `delete`, `move` for paths inside the workspace; `execute` and `fetch` by allow-list | Allow every request |

Notes:

- **"Not available" is a hard stop.** A task asking Kimi's print mode or pi for `read-only` is
  rejected with a message naming the tiers that profile does support. Use worktree isolation or a
  container when running these agents.
- **ACP answers are only as good as the questions.** An ACP agent decides which actions to ask
  about. If it never asks before editing, Hufu cannot stop it. `capabilities()` for a generic ACP
  profile therefore advertises `read-only` and `workspace-write` only when the profile is marked
  `permission_requests: verified` in configuration, after someone has checked that agent.
- **ACP modes.** Where an ACP agent advertises a plan-like mode or config option, the adapter also
  selects it for `read-only`, as a second control.
- **The Cursor middle tier is Cursor's own boundary**, not Hufu's: workspace edits are applied,
  shell commands outside its allow-list are rejected. Those rejections surface as denied actions.

### 3.2 Denied actions

| Agent | How denials are detected |
|---|---|
| Codex ◐ | Failed or declined command items in the `--json` event stream |
| Cursor ✅ | `tool_call` events that complete as rejected |
| Antigravity ✅ | Notice on stderr and step events; the process still exits `0`, so this check is mandatory |
| ACP | Hufu's own `reject` answers, plus tool calls that end with status `failed` after a rejection |
| Kimi print mode, pi | Not applicable: nothing is ever denied |

With `strict_permissions: true`, the default, any denied action fails the job with reason
`permission_denied`.

## 4. Effort mapping

Hufu's scale is `off`, `low`, `medium`, `high`, `max`, plus `default`, which means "pass nothing and
let the agent decide".

| Hufu | Codex ◐ `model_reasoning_effort` | Antigravity ✅ `--effort` | Kimi ✅ | pi ✅ `--thinking` | Cursor | ACP |
|---|---|---|---|---|---|---|
| `off` | n/a | n/a | `--no-thinking` | `off` | n/a | Option of category `thought_level`, if advertised, matched by value |
| `low` | `low` | `low` | n/a | `low` | n/a | 〃 |
| `medium` | `medium` | `medium` | n/a | `medium` | n/a | 〃 |
| `high` | `high` | `high` | `--thinking` | `high` | n/a | 〃 |
| `max` | `max` | n/a | n/a | `max` | n/a | 〃 |
| `default` | (omit) | (omit) | (omit) | (omit) | (omit) | (leave unset) |

- **n/a means an error**, not a silent skip. A task asking Antigravity for `max` fails validation.
- A profile may set `effort_fallback: nearest` to accept the closest supported level instead. The
  substitution is then recorded as a warning in the result.
- Agents with more native levels than Hufu's scale (Codex `xhigh` and `ultra`, pi `minimal` and
  `xhigh`) can be reached with `effort: native:<value>`, which is passed through verbatim and
  validated only by the agent.
- Kimi's control is a boolean, so only two levels map: `off` to `--no-thinking` and `high` to
  `--thinking`.
- Cursor has no known effort control ❓. If reasoning strength is encoded in its model ids, it is
  selected through `model`.

## 5. Per-agent notes

### 5.1 Codex CLI

Evidence: ◐ vendor documentation, summarised; confirm with `--help`.

```text
codex exec --json --cd <cwd> --sandbox <tier> --ask-for-approval never \
  [-m <model>] [-c model_reasoning_effort=<level>] [--output-schema <file>] -
```

- Preferred surface: CLI. An ACP adapter (`codex-acp`) exists and can be used through the generic
  ACP profile instead.
- The prompt is written to stdin (`-`), which is then closed.
- With `--json`, stdout is JSON Lines: `thread.started`, `turn.*`, `item.*`, `error`.
- The default sandbox for `codex exec` is read-only. The adapter still passes the flag every time.
- Resume: `codex exec resume <SESSION_ID>`.
- Requires a Git repository unless `--skip-git-repo-check` is given; the adapter passes that flag
  only when the workspace is not a repository and says so in a warning.
- Auth: saved login, or `CODEX_API_KEY` if the profile passes it through.

### 5.2 Antigravity (`agy`) CLI

Evidence: ✅ vendor documentation.

```text
agy -p <prompt> --output-format stream-json [--model <slug>] [--effort <level>] \
  [--json-schema <schema>] [--sandbox] [--dangerously-skip-permissions] < /dev/null
```

- Only surface: CLI. No ACP entry point was found.
- **No cwd flag.** The adapter sets the child process's working directory.
- **stdin must be closed.** An open stdin pipe is reported to hang some versions.
- **Empty stdout without a TTY** is a reported bug
  ([#76](https://github.com/google-antigravity/antigravity-cli/issues/76)) ⚠️. The adapter treats
  "exit 0, no result event" as `empty_output`. If the installed version is affected, the fallback
  is to run `agy` under a pseudo-terminal; the `portable-pty` crate covers this, and whether it is
  needed is decided during M1 testing on a real install.
- **Soft denial.** Without `--dangerously-skip-permissions`, actions needing approval are skipped
  and the process exits `0`. Denial detection is mandatory.
- **No default timeout** (`--print-timeout` defaults to no limit). Hufu's own timers apply.
- Unknown model ids exit non-zero; they do not fall back.
- Resume: `--conversation <id>` using `conversation_id` from the result.
- Auth: cached login from an interactive session. If absent, the run exits with an
  "authentication required" error, mapped to `auth_required`.
- Open question on terms of use for subscription accounts: see the
  [agent landscape](../research/agent-landscape.md#34-antigravity-cli-agy).

### 5.3 Cursor

Evidence: ✅ vendor documentation.

```text
agent -p <prompt> --output-format stream-json --workspace <cwd> --trust \
  [--model <id>] [--mode plan] [--force]
```

- Both surfaces work. Default: CLI for one-shot jobs; ACP (`agent acp`, a hidden command) when
  permission answers per action are wanted.
- Stream events: `system`, `user`, `assistant`, `tool_call`, `result`. Thinking is not emitted.
- Over ACP, an unanswered permission request blocks the agent; the adapter always answers.
- Resume: `--resume <chatId>`.
- Auth: `CURSOR_API_KEY` if passed through, or a saved `agent login`.

### 5.4 Kimi Code

Evidence: ✅ vendor documentation.

```text
kimi acp                                   # preferred
kimi --print --output-format stream-json -p <prompt> -w <cwd> [-m <model>] [--thinking]
```

- Preferred surface: ACP, because print mode always auto-approves.
- `kimi acp` is a multi-session server. Not logged in returns `AUTH_REQUIRED` (`-32000`), mapped to
  `auth_required` with the hint `kimi login`.
- Resume in print mode: `--session <ID>`.

### 5.5 pi

Evidence: ✅ vendor documentation.

```text
pi --mode json -p <prompt> --provider <name> --model <provider/id> [--thinking <level>]
```

- pi is Hufu's route to open-API models. A profile names a provider and a model; the API key
  reaches pi through an environment variable the profile lists in `env`.
- Surfaces: `--mode json` for one-shot jobs; `--mode rpc` when a session must stay open; the
  community `pi-acp` adapter is an alternative through the generic ACP profile.
- `full-access` is the only tier. pi's own advice is to run it in a container.
- Non-interactive modes show no project trust prompt.
- Resume: `--session <id>`.
- pi itself requires Node.js ≥ 22.19. Hufu does not.

### 5.6 Claude Code

Evidence: ⚠️ not yet verified.

To be verified when the adapter is written. Expected surface: `claude -p` with a streaming JSON
output format and an explicit permission mode, or the `claude-agent-acp` adapter through the generic
ACP profile. Needed so that Claude Code can be an executor under a different advisor.

### 5.7 Generic ACP

```yaml
profiles:
  gemini:
    adapter: acp
    command: [gemini, --acp]        # whatever starts the agent in ACP mode
    permission_requests: unverified # tiers below full-access are not advertised until verified
```

Flow:

1. Spawn `command` with pipes; speak newline-delimited JSON-RPC 2.0.
2. `initialize` with `protocolVersion: 1`. Advertise no file-system or terminal client capabilities
   by default, so the agent uses its own tools.
3. `authenticate` if the agent advertises methods and reports that it is not logged in. Hufu never
   performs an interactive login; it fails with `auth_required` and a hint.
4. `session/new` with the workspace as `cwd`, or `session/resume` / `session/load` for a follow-up.
5. Read `configOptions`. Set the option of category `model` and the option of category
   `thought_level` with `session/set_config_option` when the task asks for them. If the agent
   advertises no such option, the capability is absent and the task was already rejected.
6. `session/prompt`. Translate `session/update` notifications into normalized events.
7. Answer every `session/request_permission` from the tier policy in [section 3.1](#31-mapping),
   always with a once-only option, never "always".
8. Map `stopReason`: `end_turn` is completed; `cancelled` is cancelled; `refusal`, `max_tokens` and
   `max_turn_requests` are failed with the reason recorded.
9. On abort: send `session/cancel`, answer any pending permission request with `cancelled`.

Agents expected to work through this profile with no adapter code: Gemini CLI, OpenCode, Goose,
Qwen Code, Cursor, Kimi Code, pi (through `pi-acp`), Codex (through `codex-acp`), Claude (through
`claude-agent-acp`), and DeepSeek Harness if its ACP server is confirmed.

## 6. Conformance suite

Every adapter must pass the same tests, run against a fake of its agent that replays recorded
transcripts:

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
