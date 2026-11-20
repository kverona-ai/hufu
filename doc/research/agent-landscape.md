# Agent Landscape: How Coding Agents Can Be Driven Headlessly

| | |
|---|---|
| **Status** | Research snapshot, refreshed for Hufu 1.0; the flags each adapter passes are in the [adapter contract](../technical/adapter-contract.md) |
| **Verified on** | 2026-11-20, against vendor documentation and changelogs. On one macOS machine the installed CLIs were asked for `--version`, `--help` and their model lists only: Codex 0.170.0, Claude Code 2.1.328, Antigravity 1.5.12. Kimi Code, pi and Cursor were not installed there. No job was run with a real account |
| **Feeds into** | [Requirements](../requirements/requirements.md), [Adapter contract](../technical/adapter-contract.md), [ADR-0002](../adr/0002-acp-first-cli-fallback.md) |

Hufu drives other coding agents as subprocesses. This document records what each target agent
actually offers for non-interactive use, which claims were checked against primary sources, and
which are still open. Agent CLIs change quickly, so every table below carries its evidence level.

**Evidence levels used in this document**

| Mark | Meaning |
|---|---|
| ✅ | Read in the vendor's own documentation or package registry on the date above |
| 🖥 | Seen in the output of the installed CLI on the date above |
| ◐ | Read in vendor documentation through a summarised fetch; confirm against `--help` before relying on it |
| ⚠️ | Only third-party or community sources; treat as a lead, not a fact |
| ❓ | Looked for and not found |

## 1. Two integration surfaces

Every agent we care about exposes at least one of these:

1. **Agent Client Protocol (ACP)**. A client launches the agent as a subprocess and speaks
   newline-delimited JSON-RPC 2.0 over stdin/stdout. One client implementation covers every
   compliant agent.
2. **Headless CLI**. A one-shot, non-interactive invocation such as `codex exec` or `agy -p`.
   Flags, output formats, permission defaults and exit-code semantics differ per vendor, so each
   one needs a thin adapter.

Hufu uses both: ACP where it is solid, a CLI adapter where ACP is missing or less capable. The
rationale is recorded in [ADR-0002](../adr/0002-acp-first-cli-fallback.md).

## 2. Agent Client Protocol

All facts in this section are ✅ from `agentclientprotocol.com` (protocol v1 pages).

| Topic | Finding |
|---|---|
| Transport | stdio is the only defined transport. The client launches the agent as a subprocess. A Streamable HTTP / WebSocket transport exists only as an RFD (draft). |
| Framing | JSON-RPC 2.0, UTF-8, one message per line, no embedded newlines. The agent may log to stderr but must not write non-protocol data to stdout. |
| Version | `protocolVersion` is an integer naming the major version. Stable is `1`. A v2 has been a draft since 2026-07-20 and "can, and will, change before stabilization": prompt responses return a message id instead of ending the turn, diffs become structured file changes, and permission requests gain a `title` and a `subject`. |
| Governance | Jointly governed by Zed and JetBrains, with a stated goal of moving to an independent foundation. Apache-2.0. |
| Baseline methods | `initialize`, `session/new`, `session/prompt`, `session/cancel` (notification), `session/update` (notification from the agent). |
| Optional, capability-gated | `authenticate`, `session/load` (replays history), `session/resume` (no replay), `session/list`, `session/close`, `session/delete`, `fs/read_text_file`, `fs/write_text_file`, `terminal/*`, `elicitation/create`. |
| Model and reasoning selection | `session/set_config_option` with agent-advertised `configOptions`. The reserved categories are `mode`, `model`, `model_config` (context size or speed, stabilised 2026-06-24) and `thought_level`; names starting with `_` are custom. Some agents leave `category` out, so Hufu also matches an option by its id (`model`, or `thinking`, `effort`, `reasoning_effort` for the thought level). Older agents answer `session/new` with a `models` field (`availableModels`, `currentModelId`) instead. There is no `session/set_model` in v1. |
| Session modes | `session/set_mode` still exists but is marked legacy and slated for removal in favour of config options. |
| Permissions | The agent calls `session/request_permission` with options of kind `allow_once`, `allow_always`, `reject_once`, `reject_always`. The client answers `selected` or `cancelled`. Clients may answer automatically from policy. |
| Turn result | `session/prompt` returns a `stopReason`: `end_turn`, `max_tokens`, `max_turn_requests`, `refusal` or `cancelled`. |
| Streaming | `session/update` variants include `agent_message_chunk`, `agent_thought_chunk`, `tool_call`, `tool_call_update`, `plan`, `usage_update`, `config_option_update`. `usage_update` carries `used` and `size` (tokens) and an optional `cost` (`amount`, `currency`). |
| Recent stable additions | Boolean config options (2026-07-06), elicitation (2026-07-22), tool-call names (2026-09-17), client-provided system prompts (2026-11-06), several plans per session (2026-11-20). None changes what Hufu sends. |
| Tool kinds | `read`, `edit`, `delete`, `move`, `search`, `execute`, `think`, `fetch`, `switch_mode`, `other`. |

**SDKs** (✅ registries and library pages)

| Language | Package | Note |
|---|---|---|
| TypeScript | `@agentclientprotocol/sdk` (npm, 2.6.0, Apache-2.0) | Reference implementation; Gemini CLI's ACP code is built on it |
| Rust | `agent-client-protocol` (crates.io, 5.2.0) | Not used: Hufu has a small client of its own for the subset it needs ([ADR-0009](../adr/0009-synchronous-core-and-own-protocol-clients.md)) |
| Python | `agent-client-protocol` (PyPI) | |
| Kotlin, Java | Library pages exist | Package names not checked |
| Go | ❓ | No official page found |

**Registry.** The ACP Registry is a curated list of agents that support authentication, published as
JSON at `cdn.agentclientprotocol.com/registry/v1/latest/registry.json`. We counted about 45 entries
on the registry page; the page itself states no total.

**What ACP does not give us.** It standardises the conversation, not the sandbox. An agent decides
which actions it asks permission for, and it owns "remember this decision". A client can only say
yes or no to what it is asked. Hufu therefore treats ACP permission answers as one control among
several, alongside workspace isolation (see [Architecture, section 8](../architecture/architecture.md#8-security-model)).

## 3. Per-agent findings

### 3.1 Summary matrix

| Agent | Headless CLI | ACP | Model selection | Reasoning control | Permission default when headless |
|---|---|---|---|---|---|
| **Codex** (OpenAI) | `codex exec` ✅ 🖥 | Via `codex-acp` adapter ✅ | `-m/--model` ✅; `codex debug models` prints the catalog 🖥 | `-c model_reasoning_effort="low…max"`, plus `ultra` (subagents) ✅ | Read-only sandbox ✅ |
| **Cursor** | `cursor-agent -p` (also installed as `agent`) ✅ | `cursor-agent acp` ✅ | `--model`; `cursor-agent models` lists ids ✅ | ❓ none in the CLI | Edits inside the workspace are applied; other actions rejected, unless `approvalMode: unrestricted` ✅ |
| **Antigravity** (Google) | `agy -p` ✅ 🖥 | `agy acp` 🖥 | `--model <slug>`; `agy models` lists slugs ✅ 🖥 | `--effort low…max`, per model ✅ | Ask-only actions are soft-denied and the run still exits `0` ✅ |
| **Kimi Code** (Moonshot) | `kimi -p` ✅ | `kimi acp` ✅ | `-m/--model <alias>` from `config.toml` ✅ | `[thinking] effort` in `config.toml`; `KIMI_MODEL_THINKING_EFFORT` only for a model defined by `KIMI_MODEL_NAME` ✅ | Runs tools without asking; `[[permission.rules]]` apply ✅ |
| **pi** | `pi -p`, `--mode json`, `--mode rpc`, SDK ✅ | Via `pi-acp` adapter ✅ | `--provider`, `--model provider/id`; `--list-models` ✅ | `--thinking off…max`, clamped to the model ✅ | No permission system at all ✅ |
| **Claude Code** (Anthropic) | `claude -p` ✅ 🖥 | Via `claude-agent-acp` adapter ✅ | `--model <alias or id>` ✅ | `--effort low…max`, `ultracode` ✅ 🖥 | Calls that need approval are denied; `--permission-prompts none` makes it explicit ✅ |
| **Gemini CLI**, **OpenCode**, **Goose**, **Qwen Code** | Not checked | Listed as ACP agents ✅ | Through ACP config options | Through ACP config options | Through ACP permission requests |
| **DeepSeek Harness** (`dsh`) | ❓ | Reported to run an ACP server ⚠️ | ❓ | ❓ | ❓ |
| **ZCode** (Z.ai) | ❓ no CLI documented | ❓ | In-app only | ❓ | ❓ |

### 3.2 Codex CLI

Source: `learn.chatgpt.com/docs/non-interactive-mode`, `…/models`, `…/developer-commands`,
`…/config-file/config-reference` and `…/changelog?type=codex-cli` (the former
`developers.openai.com/codex/*` pages redirect here). Latest release 0.171.0 (2026-11-20); the
macOS check ran 0.170.0. Evidence ✅ unless marked.

- `codex exec "<prompt>"`; `codex exec -` reads the prompt from stdin. Progress goes to stderr and
  only the final agent message goes to stdout. With `--json`, stdout becomes JSON Lines with events
  `thread.started`, `turn.started`, `turn.completed`, `turn.failed`, `item.*` and `error`.
  `turn.completed` carries `input_tokens`, `cached_input_tokens`, `output_tokens` and
  `reasoning_output_tokens`. Only `agent_message` and `command_execution` are named as item types in
  the documentation; the adapter also reads `reasoning`, `file_change`, `mcp_tool_call`,
  `web_search` and `todo_list` from the `codex-rs/exec` sources ◐.
- `--output-schema <file>`, `-o/--output-last-message <file>`, `--cd/-C`, `--skip-git-repo-check`,
  `--profile/-p`, `--ephemeral`, `--ignore-user-config`, `--trust`, and since 0.171 `--remote
  <ADDR>` (run on an existing app server). `--approve-for-me` and `--not-so-yolo` are new approval
  shortcuts; Hufu forbids them in a profile's `args` because they change the tier.
- `--sandbox read-only | workspace-write | danger-full-access`. **`codex exec` defaults to
  read-only.** `--dangerously-bypass-approvals-and-sandbox` (alias `--yolo`) removes the sandbox and
  all approvals. `--full-auto` is no longer supported.
- **Untrusted directories (0.169).** `codex exec` keeps project configuration and project MCP
  servers disabled in a directory Codex does not trust, even with `--sandbox workspace-write`.
  `codex exec --trust` saves the trust. Whether servers given with `-c mcp_servers.…` are affected
  is not stated ❓.
- **Models.** `gpt-6.2-sol` is the default catalog selection since 0.171 ("fast, go-to model for
  coding and everyday work"); `gpt-6.2-astra` is recommended for demanding work. Also listed:
  `gpt-6.1-sol`, `gpt-6.1-terra`, `gpt-6-astra`, `gpt-6-luna`. `gpt-5.5` left Codex for ChatGPT
  sign-in on 2026-10-14 and `gpt-5.3-codex-spark` on 2026-09-14; API-key sessions are not affected.
- **`codex debug models`** 🖥 prints the model catalog of the current login as JSON:
  `{"models": [{"slug", "display_name", "description", "default_reasoning_level",
  "supported_reasoning_levels": [{"effort", "description"}], "visibility": "list" | "hide",
  "priority", …}]}`. On the macOS check it listed 12 models; `--bundled` skips the refresh. There is
  no top-level `codex models` command. `hufu models` reads this catalog.
- **Effort.** `-c model_reasoning_effort="<level>"` (TOML, so the string needs its quotes). Values:
  `low`, `medium`, `high`, `xhigh`, `max`, `ultra`; which ones a model accepts depends on the
  model. `ultra` is not a deeper single-agent level: it "uses subagents to handle separate parts of
  a complex task in parallel". GPT-6 Luna goes up to `max`; GPT-6.2 Sol's catalog entry lists all
  six and defaults to `low`. `plan_mode_reasoning_effort` overrides the level in plan mode.
- `codex exec resume <SESSION_ID>` and `codex exec resume --last`. Options that are not global
  (`--sandbox`, `--cd`, `-c`, `--add-dir`) must come before the word `resume`.
- MCP servers: `[mcp_servers.<name>]` in `config.toml`, or the same keys per run with `-c`. The
  configuration directory is `CODEX_HOME`, default `~/.codex`. `default_permissions` (`:read-only`,
  `:workspace`, `:danger-full-access`) is a new config key; how it interacts with `--sandbox` is not
  documented ❓.
- Exit codes are not documented ❓. The adapter decides success from the event stream.
- Auth: the saved CLI login, or `CODEX_API_KEY` per run.

### 3.3 Cursor CLI

Source: `cursor.com/docs/cli/headless`, `…/reference/parameters`, `…/acp`, `…/changelog`
(latest v2026.11.17), and Cursor's forum. Evidence ✅ unless marked.

- **The program has two names.** The installer puts the same binary on PATH as `agent` and as
  `cursor-agent`. Other tools install an `agent` too: the Grok CLI does, and on the macOS check
  `agent` was Grok 1.0.89 🖥, so a dispatcher that ran `agent` would have started Grok. Cursor's
  staff recommend `cursor-agent` for that reason (forum, July 2026 ⚠️). Hufu runs `cursor-agent`
  when it exists and otherwise checks that `agent --version` is not Grok's.
- `-p/--print` is the non-interactive mode. **Print mode writes to files in the workspace without
  `--force`**, and rejects writes outside it, shell commands, MCP tools and web fetches that are
  not allow-listed. `--force` (alias `--yolo`) runs everything except what `permissions.deny` and
  team policy block. `approvalMode: unrestricted` in `cli-config.json` makes print mode behave like
  `--force`; Hufu reads that setting and then refuses `workspace-write`. A team can disable
  headless mode altogether.
- `--output-format text | json | stream-json`. Stream events: `system` (init), `user`,
  `assistant`, `tool_call` (started/completed), `result`. Since v2026.10.28 `result` carries
  `usage`, including `reasoningTokens` when the model reasoned.
- `--mode plan | ask` (`plan` blocks code changes; hardened in v2026.11.04), `--resume [chatId]`,
  `--continue`, `--workspace <path>`, `--sandbox enabled | disabled`, `--trust` (headless only),
  `--approve-mcps`, `-w/--worktree [name]`.
- **Models.** `cursor-agent models` ("available models for this account") and `--list-models`.
  The documentation gives display names only (Composer 2.5, GPT-5.6 Sol, Claude Opus 5, Grok 4.7,
  Gemini Pro and others), not ids. There is **no effort control** in the CLI; effort exists only in
  the IDE's model picker.
- Auth: `CURSOR_API_KEY` or `--api-key`, `CURSOR_AUTH_TOKEN`, or an interactive
  `cursor-agent login`. The configuration directory is `CURSOR_CONFIG_DIR`, default `~/.cursor`.
- ACP: `cursor-agent acp`, auth method `cursor_login`, modes `agent`, `plan`, `ask`. Dashboard MCP
  servers are not available in ACP mode.

### 3.4 Antigravity CLI (`agy`)

Source: `antigravity.google/docs/cli/headless`, `…/docs/models?app=cli`,
`…/docs/cli/commands/effort`, and `agy --help` 🖥 (version 1.5.12). Evidence ✅ unless marked.

- `agy -p "<prompt>"` (aliases `--print`, `--prompt`). Runs are stateless by default;
  `--continue/-c` and `--conversation <id>` resume.
- `--output-format text | json | stream-json`. The JSON envelope carries `conversation_id`,
  `status`, `response`, `error`, `duration_seconds`, `num_turns`, `structured_output` and `usage`.
  `--json-schema` requests structured output. `stream-json` prints one object per line with an
  `event` of `init`, `step_update` or `result`.
- **Permissions.** `--dangerously-skip-permissions` approves everything. Otherwise actions that
  would need a prompt are **soft-denied: the run continues, exits `0` and prints a notice on
  stderr**. Allow rules live in `~/.gemini/antigravity-cli/settings.json` (`permissions.allow`, for
  example `command(git)` or `write_file(src/)`). `--sandbox` turns on terminal sandbox
  restrictions. `--mode accept-edits | auto | plan` 🖥: the mode a run gets without it depends on
  the user's `agentMode` setting and the version, so Hufu always names one. `--settings` takes
  per-run settings, including permissions, so Hufu forbids it in a profile's `args`.
- **Models.** `agy models` 🖥 prints one `slug<TAB>display name` line per model on stdout and
  "Fetching available models..." on stderr, exit `0`. The macOS check listed 27 models: Gemini 4
  Neon, Carbon, Argon and Hydrogen in level variants (`gemini-4-neon-high`, `gemini-4-neon-xhigh`,
  …), three Orchestrator models, Gemini 3.8 Flash, Claude Opus 5.5 and Sonnet 5.5 variants, and
  GPT-OSS 120B. Gemini 4 Neon is the default for new conversations. An unknown model exits `1`.
- **Effort.** `--effort low | medium | high | xhigh | max`, applied to the current model by
  switching to that model's variant. `xhigh` and `max` exist only for models that publish them:
  Gemini 4 Neon and Hydrogen have `xhigh`; no model lists `max` yet. Orchestrator models take no
  effort. `--effort` lasts for one invocation.
- **ACP.** `agy acp` 🖥 starts an ACP server over stdio (with `--dangerously-skip-permissions`,
  `--add-dir`, `--agent` and a `--login` that logs in interactively and exits). The documentation
  page for it was not read. Hufu ships it as the `agy-acp` preset.
- MCP servers are read from `~/.gemini/config/mcp_config.json`, or `.agents/mcp_config.json` in the
  workspace; there is no per-run flag. No flag or variable moves the configuration directory, so a
  second account needs a second operating-system user or home directory.
- `--print-timeout` defaults to `0s` (no limit). No working-directory flag; the adapter sets the
  child process's cwd. Without cached credentials the run exits with "authentication required"
  instead of hanging.
- Exit codes: `0` success; non-zero when no response was produced (`1` for an unknown model); `2`
  for stream-input control events and CLI-handled slash commands.

**Known problem: empty stdout without a TTY** ⚠️. Issue
[google-antigravity/antigravity-cli#76](https://github.com/google-antigravity/antigravity-cli/issues/76)
reports that `agy -p` can complete a full round trip and print nothing when stdout is a pipe, and
can hang when stdin is an open pipe (versions 1.0.0 to 1.0.16). The Hufu adapter closes stdin and
treats "exit 0 with empty output" as a failure.

**Terms of use** ⚠️. Whether an external orchestrator may invoke `agy -p` under a personal
subscription is still not answered by Google. Re-check before recommending unattended use on
subscription accounts.

### 3.5 Kimi Code CLI

Source: the Kimi Code documentation under `www.kimi.com/code/docs/en/` (command reference,
configuration files, environment variables, models, changelog). Latest release 2.7.2
(2026-11-20). Evidence ✅ unless marked.

- `kimi -p "<prompt>"` is the non-interactive mode. It cannot be combined with `--yolo`, `--auto`
  or `--plan`. Print mode **runs file operations and shell commands without asking**; since 2.6.0
  the `[[permission.rules]]` of `config.toml` (`deny`, `ask`, `allow`, with `deny` winning) apply in
  print mode too. There is no read-only print mode, so Hufu offers print mode `full-access` only
  and points lower tiers at `kimi acp`.
- `--output-format text | stream-json`. Each stdout line is an assistant reply, tool call or tool
  result; thinking and progress go to stderr. **The stream ends with a
  `{"type": "session.resume_hint", "session_id": …}` line**, and `kimi -S <id> -p "…"` continues
  that session, so print-mode jobs can be resumed. There is no result line and no usage.
- Exit codes: `0` done; `1` invalid arguments, model not ready or an execution error; `4` the
  session is held by another process (`KIMI_CODE_SESSION_LOCK=0` turns the lock off).
- **Models.** `-m/--model` takes an alias defined in `config.toml` (`[models."kimi-code/k3"]`
  with `model = "k3"`, `support_efforts` and `default_effort`), and `default_model` names the
  default. The model table lists `k3.1-pro`, `k3.1-pro-highspeed`, `k3`, `k3-256k`,
  `kimi-for-coding` and `kimi-for-coding-highspeed`. `hufu models` reads the aliases from
  `config.toml`.
- **Effort.** There is no flag. `[thinking] effort = "low" | "medium" | "high" | "xhigh" | "max"`
  in `config.toml`, per-model `support_efforts`, and `/effort` interactively. K3.1 Pro, K3 and
  `kimi-for-coding` accept `low`, `high` and `max`; thinking cannot be turned off on K3.1 Pro.
  **`KIMI_MODEL_THINKING_EFFORT` only applies to a model defined in the environment with
  `KIMI_MODEL_NAME`** (and `KIMI_MODEL_API_KEY`, `KIMI_MODEL_BASE_URL`), so Hufu offers effort to a
  print-mode profile only when the profile defines its model that way.
- `kimi acp` starts an ACP server over stdio. Not logged in returns `AUTH_REQUIRED` (`-32000`).
  Swarms are not available in print mode or ACP (2.3.2).
- MCP servers: `mcp.json` in the data directory, or `.kimi-code/mcp.json` in a trusted project
  (`KIMI_CODE_TRUST_WORKSPACE=1` trusts it for one process). The data directory is
  `KIMI_CODE_HOME`, default `~/.kimi-code`.
- Auth: `kimi login`. Provider keys are read from `config.toml`, not from shell variables, except
  `KIMI_MODEL_*` and `api_key_env`.

### 3.6 pi

Source: `docs.pi.dev/latest` (CLI, providers, JSON events, environment variables, quickstart) and
`pi.dev/changelog`. Latest release 1.6.0 (2026-11-20). Evidence ✅.

- Install: `curl -fsSL https://pi.dev/install.sh | sh` (macOS, Linux),
  `powershell -c "irm https://pi.dev/install.ps1 | iex"` (Windows), or
  `npm install -g --ignore-scripts @earendil-works/pi-coding-agent` with Node.js 22.19 or newer.
- Modes: `-p/--print`, `--mode json` (JSON Lines events), `--mode rpc` and an SDK. The JSON stream
  starts with `{"type": "session", "id": …}` and runs through `agent_start`, `turn_*`, `message_*`
  (with `usage` and `cost`), `tool_execution_*`, `agent_end` and `agent_settled`.
- `--provider <name>` (needs `--model`), `--model <provider/id[:thinking]>`,
  `--thinking off | minimal | low | medium | high | xhigh | max`. **A level the model lacks is
  clamped to one it has**, not refused; GPT-6.2 models have no `off` or `minimal`.
  `--list-models [search]` lists models, optionally filtered; its output format is not documented.
- By design it has no permission prompts and no plan mode. A run is limited by its tool list
  instead: `--tools read --no-mcp` leaves an agent that can only read. `-a/--approve` trusts
  project-local configuration for one process.
- `--session <path|id>` resumes; `--no-session`, `--fork`, `--session-dir` also exist.
- Provider keys come from the environment, for example `OPENROUTER_API_KEY`, `DEEPSEEK_API_KEY`,
  `ZAI_API_KEY`, `ZAI_CODING_CN_API_KEY`, `KIMI_API_KEY`, `MOONSHOT_API_KEY`,
  `MINIMAX_API_KEY`/`MINIMAX_CN_API_KEY`, `XAI_API_KEY`, `GEMINI_API_KEY`, `GOOGLE_CLOUD_API_KEY`
  (Vertex AI), `XIAOMI_API_KEY`, `OPENCODE_API_KEY`, `BASETEN_API_KEY`, `RADIUS_API_KEY`,
  `TYPESAFE_API_KEY` and the `QWEN_TOKEN_PLAN_*` family. The configuration directory is
  `PI_CODING_AGENT_DIR`, default `~/.pi/agent`.
- Default models moved to GPT-6.2 Sol (OpenAI, Azure, Codex) and `claude-opus-5-5` (Anthropic).

**Why pi matters to Hufu.** OpenRouter and similar APIs are model endpoints, not agents: they have
no loop that reads files or runs commands. pi turns "a model slug on some provider" into an agent,
so one adapter covers OpenRouter, DeepSeek, Z.ai's and Kimi's coding plans, and more.

### 3.7 Claude Code

Source: `code.claude.com/docs/en/cli-reference`, `…/headless`, `…/model-config`, `…/changelog`,
and `claude --help` 🖥 (version 2.1.328). Evidence ✅ unless marked.

- `claude -p` is the non-interactive mode; the prompt can be piped on stdin.
  `--output-format text | json | stream-json`; `stream-json` needs `--verbose`.
- **Permissions.** `--permission-mode default | acceptEdits | plan | auto | dontAsk |
  bypassPermissions | manual` (`manual` is an alias of `default`), and
  `--dangerously-skip-permissions`. `--permission-prompts host | none` (2.1.259 or later): with
  `none`, a call that would need approval is denied and Claude is told not to retry. The
  documentation disagrees on the mode a print-mode run gets when none is set (2.1.302), so Hufu
  always names the mode and passes `--permission-prompts none` for the lower tiers.
- **Models.** `--model` takes an alias (`default`, `best`, `fable`, `opus`, `sonnet`, `haiku`,
  `opus[1m]`, `sonnet[1m]`, `opusplan`) or a full id such as `claude-opus-5-5`,
  `claude-haiku-5-5` or `claude-fable-5-1`. On the Anthropic API `opus`, `sonnet` and `haiku` are
  the 5.5 models and `default` is Opus 5.5 on every plan; `best` is Fable where the account has it.
  On Bedrock, Vertex AI or Foundry the aliases resolve to older models. Claude Code has **no
  command that lists models**, so `hufu models` shows Hufu's copy of the model configuration page.
  `--fallback-model` takes a comma-separated list.
- **Effort.** `--effort low | medium | high | xhigh | max | ultracode` (`ultracode` is `xhigh` with
  ultracode on, 2.1.203 or later). Fable 5.1, Opus 5.5, Sonnet 5.5, Haiku 5.5, Opus 5, Sonnet 5,
  Opus 4.8 and 4.7 have all five levels; Opus 4.6 and Sonnet 4.6 have no `xhigh`. A missing level
  falls back to the highest one below it. Defaults: `medium` for the 5.5 models, `high` for most
  others.
- `--resume <id>`, `--json-schema`, `--mcp-config`, `--strict-mcp-config`, `--add-dir`,
  `--max-turns`, `--max-budget-usd`, `--no-session-persistence`, `--worktree`, `--bare`. There is
  no working-directory flag.
- The final `result` message carries `session_id`, `total_cost_usd`, `usage`, `modelUsage`,
  `structured_output` and `permission_denials`; `permission_denied` system messages arrive during
  the run.
- `claude auth status` prints JSON and exits `0` or `1`. The configuration directory is
  `CLAUDE_CONFIG_DIR`. `claude -p` exits non-zero on failure; SIGTERM gives `143`.
- As a host: `claude mcp add [--scope local|project|user] <name> -- <command>` registers an MCP
  server, and skills live in `~/.claude/skills/<name>/SKILL.md`.

### 3.8 ZCode and DeepSeek Harness

- **ZCode** ❓. `zcode.z.ai` describes an Electron desktop development environment (version 3.17.2
  at the time of reading). We found no documented command-line or headless entry point, and no ACP
  support. Until one is confirmed, Hufu reaches GLM models through pi's ZAI provider or through the
  community `glm-acp-agent` listed in the ACP registry.
- **DeepSeek Harness** ⚠️. The only evidence found is indirect: a third-party ACP client's README
  refers to a `dsh` command installed from `@deepseek-ai/dsh` and says it launches DSH's ACP server.
  Whether this is an official DeepSeek product, and whether it has a one-shot headless mode, is
  unconfirmed. If the ACP server exists, the generic ACP adapter covers it by configuration.

## 4. Corrections to the earlier analysis

The project was scoped from an earlier conversation. Checking it against primary sources changed
several points that affect design:

| Earlier statement | What the sources say | Consequence |
|---|---|---|
| Cursor without `--force` only proposes edits | Print mode applies edits inside the workspace without `--force` | Cursor's safest write-free mode is `--mode plan`, not the default |
| `agy` takes `--mode accept-edits\|plan` or `--approve` | The headless page documents only `--dangerously-skip-permissions`, settings-based allow rules and `--sandbox`. The reference lists `--mode accept-edits\|auto\|plan` without saying that it applies to `-p`; `--approve` was not found | The adapter uses `--mode plan --sandbox` for read-only and `--sandbox` for workspace writes, and Hufu checks the workspace afterwards. Verify on a real install |
| `agy` headless times out after 5 minutes by default | `--print-timeout` defaults to `0s` (no limit) | Hufu must enforce its own timeout on every job |
| Antigravity has an ACP entry | No `agy acp` command; registry entry points at IDE documentation | Superseded in 2026-11: `agy acp` exists (see below) |
| Kimi `--print` turns on yolo | Print mode runs every tool without asking and cannot be combined with `--plan` | Same practical effect; tighter tiers require the ACP path |
| Kimi takes `--thinking`, `--print` and `-w` | Those belong to the archived `kimi-cli`. Kimi Code uses `-p`, the process's directory and `KIMI_MODEL_THINKING_EFFORT` | The adapter targets Kimi Code |
| pi has no MCP and no way to limit a run | It has `--tools` and `--no-mcp` | pi gets a read-only tier |
| ZCode has `zcode --prompt` and `--mode build\|plan` | Not found in vendor documentation | ZCode is out of scope until a headless entry is confirmed |
| DeepSeek Harness has a headless mode | Not confirmed | Candidate for the generic ACP adapter only |

The 2026-11-20 refresh corrected these points of the 0.1 research:

| Earlier statement | What the sources say now | Consequence |
|---|---|---|
| `agy` has no ACP entry point | `agy acp` exists in 1.5.12 🖥 | New `agy-acp` preset; ACP gives agy per-request permission answers |
| Cursor's binary is `agent` | The installer also provides `cursor-agent`, and another CLI (Grok) installs `agent` 🖥 | Hufu runs `cursor-agent` first and checks the identity of `agent` |
| `agy` workspace writes need only `--sandbox` | The default mode follows the user's `agentMode` setting | `workspace-write` passes `--mode accept-edits --sandbox` explicitly |
| Kimi print mode cannot be resumed | The stream ends with `session.resume_hint`; `-S <id>` resumes | Kimi print-mode jobs are resumable |
| `KIMI_MODEL_THINKING_EFFORT` sets Kimi's effort | Only for a model defined by `KIMI_MODEL_NAME` | Effort is offered only to such profiles; others use `config.toml` or `kimi-acp` |
| Effort tops out at `max` with `high` below it | Codex, Claude Code, Antigravity, Kimi and pi all have `xhigh` between them | Hufu's scale gains `xhigh` |
| Claude Code denies approval calls in print mode | The default mode is documented two ways; `--permission-prompts none` is explicit | Hufu passes `--permission-prompts none` for read-only and workspace-write |

## 5. Implications for Hufu

1. **Never rely on an agent's default permissions.** Defaults range from read-only (Codex) to
   approve-everything (Kimi print mode, pi). Each adapter must translate a Hufu permission tier into
   explicit flags and must refuse a tier it cannot honour.
2. **Exit code `0` does not mean the work was done.** `agy` exits `0` after soft-denying actions and
   can exit `0` with empty output. Adapters must inspect the event stream, and a job needs an
   acceptance step that is separate from process success.
3. **Hufu owns timeouts and cancellation.** At least one agent has no default timeout.
4. **Reasoning controls are not uniform.** They range from seven named levels, through a
   configuration key, an environment variable and a model variant, to nothing, and the levels a
   model accepts differ per model. A task spec uses an abstract level, each adapter declares what
   it can map, and `hufu models` asks the agent which models and levels it has.
5. **Prefer the surface that gives more control.** For Kimi, ACP is the only path with permission
   control. For Codex, the CLI is better documented than its adapter. The preferred surface is a
   per-agent decision recorded in the [adapter contract](../technical/adapter-contract.md).
6. **Open-API models go through pi.** No separate "model adapter" is needed.
7. **Subscription terms are a risk to track**, not something code can solve. Hufu only ever invokes
   official binaries and never reads or reuses their stored credentials.

## 6. Open verification items

The adapters were written from the documentation above and are tested against fake agents that
print what the documentation says. The items below need a machine with the real CLIs installed and
logged in. `hufu doctor` reports the installed version of each agent, and `hufu models <profile>`
shows what the agent itself lists; record the version here when an item is closed.

- [ ] Every adapter: one read-only job, one writing job, one cancel and one resume against the real
      agent.
- [ ] Codex: whether `--trust` or the trust rule of 0.169 affects MCP servers given with `-c`; how
      `default_permissions` in the user's `config.toml` interacts with `--sandbox`.
- [ ] `agy`: the state of the non-TTY stdout bug on 1.5.x; the text of the soft-denial notice;
      whether `agy acp` sends permission requests and offers `model` and `thought_level` options.
- [ ] Cursor: the output format of `cursor-agent models` (Hufu reads it leniently, one model per
      line).
- [ ] pi: the output format of `--list-models` (Hufu reads a `provider model …` table, or one model
      per line).
- [ ] `kimi acp`, `agy acp` and `cursor-agent acp`: confirm that permission requests are sent to
      the client and honoured. Until then Hufu offers these profiles `full-access` only, unless the
      profile is marked `permission_requests: verified`.
- [ ] `dsh`: confirm publisher, headless mode and ACP server.

Closed in the 1.0 refresh: `codex exec` options and effort values, `codex debug models`, `agy
--mode`, `agy models`, Kimi print-mode session ids and exit codes, Cursor's lack of an effort
control.

## Sources

- Agent Client Protocol: <https://agentclientprotocol.com/llms.txt>, <https://agentclientprotocol.com/updates.md>, <https://agentclientprotocol.com/protocol/v1/session-config-options.md>, <https://agentclientprotocol.com/protocol/v1/prompt-turn.md>, <https://agentclientprotocol.com/announcements/acp-v2-draft.md>
- ACP agents and registry: <https://agentclientprotocol.com/get-started/agents>, <https://agentclientprotocol.com/get-started/registry>
- Codex: <https://learn.chatgpt.com/docs/non-interactive-mode>, <https://learn.chatgpt.com/docs/models>, <https://learn.chatgpt.com/docs/developer-commands?surface=cli>, <https://learn.chatgpt.com/docs/config-file/config-reference>, <https://learn.chatgpt.com/docs/changelog?type=codex-cli>
- Cursor: <https://cursor.com/docs/cli/headless>, <https://cursor.com/docs/cli/reference/parameters>, <https://cursor.com/docs/cli/acp>, <https://cursor.com/docs/cli/changelog>, <https://cursor.com/help/models-and-usage/available-models>
- Antigravity: <https://antigravity.google/docs/cli/headless>, <https://antigravity.google/docs/models?app=cli>, <https://antigravity.google/docs/cli/commands/effort/>, <https://github.com/google-antigravity/antigravity-cli/issues/76>
- Kimi Code: <https://www.kimi.com/code/docs/en/kimi-code-cli/reference/kimi-command.html>, <https://www.kimi.com/code/docs/en/kimi-code-cli/configuration/configuration-files.html>, <https://www.kimi.com/code/docs/en/kimi-code-cli/configuration/env-vars.html>, <https://www.kimi.com/code/docs/en/kimi-code/models.html>, <https://www.kimi.com/code/docs/en/kimi-code-cli/changelog.html>
- Claude Code: <https://code.claude.com/docs/en/cli-reference>, <https://code.claude.com/docs/en/headless>, <https://code.claude.com/docs/en/model-config>, <https://code.claude.com/docs/en/changelog>
- pi: <https://docs.pi.dev/latest/cli>, <https://docs.pi.dev/latest/providers>, <https://docs.pi.dev/latest/json>, <https://docs.pi.dev/latest/quickstart>, <https://pi.dev/changelog>
- ZCode: <https://zcode.z.ai>
- Zed external agents: <https://zed.dev/docs/ai/external-agents>
