# Agent Landscape: How Coding Agents Can Be Driven Headlessly

| | |
|---|---|
| **Status** | Research snapshot |
| **Verified on** | 2026-11-12 |
| **Feeds into** | [Requirements](../requirements/requirements.md), [Adapter contract](../technical/adapter-contract.md), [ADR-0002](../adr/0002-acp-first-cli-fallback.md) |

Hufu drives other coding agents as subprocesses. This document records what each target agent
actually offers for non-interactive use, which claims were checked against primary sources, and
which are still open. Agent CLIs change quickly, so every table below carries its evidence level.

**Evidence levels used in this document**

| Mark | Meaning |
|---|---|
| ✅ | Read in the vendor's own documentation or package registry on the date above |
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
| Version | `protocolVersion` is an integer naming the major version. Stable is `1`. A v2 exists as a draft. |
| Governance | Jointly governed by Zed and JetBrains, with a stated goal of moving to an independent foundation. Apache-2.0. |
| Baseline methods | `initialize`, `session/new`, `session/prompt`, `session/cancel` (notification), `session/update` (notification from the agent). |
| Optional, capability-gated | `authenticate`, `session/load` (replays history), `session/resume` (no replay), `session/list`, `session/close`, `session/delete`, `fs/read_text_file`, `fs/write_text_file`, `terminal/*`, `elicitation/create`. |
| Model and reasoning selection | `session/set_config_option` with agent-advertised `configOptions`. Reserved categories include `model` and `thought_level`. Stabilised on 2026-02-04. There is no `session/set_model` in v1. |
| Session modes | `session/set_mode` still exists but is marked legacy and slated for removal in favour of config options. |
| Permissions | The agent calls `session/request_permission` with options of kind `allow_once`, `allow_always`, `reject_once`, `reject_always`. The client answers `selected` or `cancelled`. Clients may answer automatically from policy. |
| Turn result | `session/prompt` returns a `stopReason`: `end_turn`, `max_tokens`, `max_turn_requests`, `refusal` or `cancelled`. |
| Streaming | `session/update` variants include `agent_message_chunk`, `agent_thought_chunk`, `tool_call`, `tool_call_update`, `plan`, `usage_update`, `config_option_update`. |
| Tool kinds | `read`, `edit`, `delete`, `move`, `search`, `execute`, `think`, `fetch`, `switch_mode`, `other`. |

**SDKs** (✅ registries and library pages)

| Language | Package | Note |
|---|---|---|
| TypeScript | `@agentclientprotocol/sdk` (npm, 2.6.0, Apache-2.0) | Reference implementation; Gemini CLI's ACP code is built on it |
| Rust | `agent-client-protocol` (crates.io) | |
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
| **Codex** (OpenAI) | `codex exec` ◐ | Via `codex-acp` adapter ✅ | `-m/--model` ◐ | `model_reasoning_effort` config key ◐ | Read-only sandbox ◐ |
| **Cursor** | `agent -p` ✅ | `agent acp`, a hidden command ✅ | `--model` ✅ | ❓ none found | Edits inside the workspace are applied; other actions rejected ✅ |
| **Antigravity** (Google) | `agy -p` ✅ | ❓ no `agy acp`; a registry entry exists but points at IDE docs | `--model <slug>` ✅ | `--effort low\|medium\|high` ✅ | Ask-only actions are soft-denied and the run still exits `0` ✅ |
| **Kimi Code** (Moonshot) | `kimi --print` ✅ | `kimi acp` ✅ | `-m/--model` ✅ | `--thinking` / `--no-thinking` (boolean) ✅ | Auto-approves everything (`--print` implies `--afk`) ✅ |
| **pi** | `pi -p`, `--mode json`, `--mode rpc`, SDK ✅ | Via `pi-acp` adapter ✅ | `--provider`, `--model provider/id` ✅ | `--thinking off…max` ✅ | No permission system at all ✅ |
| **Claude Code** (Anthropic) | `claude -p` ⚠️ not re-verified in this pass | Via `claude-agent-acp` adapter ✅ | `--model` ⚠️ | ⚠️ | ⚠️ |
| **Gemini CLI**, **OpenCode**, **Goose**, **Qwen Code** | Not checked | Listed as ACP agents ✅ | Through ACP config options | Through ACP config options | Through ACP permission requests |
| **DeepSeek Harness** (`dsh`) | ❓ | Reported to run an ACP server ⚠️ | ❓ | ❓ | ❓ |
| **ZCode** (Z.ai) | ❓ no CLI documented | ❓ | In-app only | ❓ | ❓ |

### 3.2 Codex CLI

Source: `learn.chatgpt.com/docs/non-interactive-mode`, `…/developer-commands`, `…/config-reference`
(the former `developers.openai.com/codex/*` pages redirect here). Evidence ◐: the pages were long
and partly summarised by the fetcher, and two of them disagreed on details. Confirm with
`codex exec --help` when the adapter is written.

- `codex exec "<prompt>"`; `codex exec -` reads the prompt from stdin.
- Progress goes to stderr and only the final agent message goes to stdout. With `--json`, stdout
  becomes JSON Lines with events such as `thread.started`, `turn.started`, `turn.completed`,
  `turn.failed`, `item.*` and `error`.
- `--output-schema <file>` requests a final message matching a JSON Schema.
  `-o/--output-last-message <file>` also writes the final message to a file.
- `--sandbox read-only | workspace-write | danger-full-access`. **`codex exec` defaults to
  read-only**, so an executor that should change files must be given `workspace-write` explicitly.
- `--ask-for-approval on-request | never`. `--dangerously-bypass-approvals-and-sandbox` (alias
  `--yolo`) removes both. `--full-auto` is reported as no longer supported.
- `--model/-m`; reasoning effort through the `model_reasoning_effort` config key. Values listed:
  `low`, `medium`, `high`, `xhigh`, `max`, `ultra`.
- `codex exec resume <SESSION_ID>` and `codex exec resume --last`.
- `--cd/-C` sets the workspace root. `--skip-git-repo-check` lifts the "must be a Git repo" rule.
- Auth: reuses the saved CLI login; `CODEX_API_KEY` can be set per run.
- `codex app-server` exists and is marked experimental. `codex mcp` manages MCP servers that Codex
  consumes; a "Codex as MCP server" command was not found.

### 3.3 Cursor CLI

Source: `cursor.com/docs/cli/headless.md`, `…/reference/parameters.md`, `…/acp.md`,
`…/reference/output-format.md`. Evidence ✅.

- Binary is `agent`. `-p/--print` is the non-interactive mode and "has access to all tools,
  including write and shell".
- **Print mode writes to files in the workspace even without `--force`.** It rejects writes outside
  the workspace or to protected paths, and shell commands, MCP tools and web fetches that are not
  allow-listed. `--force` (alias `--yolo`) runs everything except what `permissions.deny` and team
  policy block.
- `--output-format text | json | stream-json`. Stream events: `system` (init), `user`, `assistant`,
  `tool_call` (started/completed), `result`. Thinking events are suppressed in print mode.
- `--mode plan | ask`, `--resume [chatId]`, `--continue`, `--workspace <path>`,
  `--sandbox enabled | disabled`, `--trust`.
- Auth: `CURSOR_API_KEY` or `--api-key`, or an interactive `agent login`.
- ACP: `agent acp`, documented as an advanced command hidden from default help. If the client does
  not answer permission requests, tool execution blocks. Team-level MCP servers are not available
  in ACP mode.

### 3.4 Antigravity CLI (`agy`)

Source: `antigravity.google/docs/cli/headless` ✅, plus the project issue tracker and Google's
developer forum ⚠️.

- `agy -p "<prompt>"` (aliases `--print`, `--prompt`). Runs are stateless by default;
  `--continue/-c` and `--conversation <id>` resume.
- `--output-format text | json | stream-json`. The JSON envelope carries `conversation_id`,
  `status`, `response`, `error`, `duration_seconds`, `num_turns`, `structured_output` and `usage`.
  `--json-schema` requests structured output.
- `--model <slug>` (list with `agy models`); an unknown model exits non-zero instead of falling
  back. `--effort low | medium | high`.
- Permissions: `--dangerously-skip-permissions` approves everything. Otherwise, actions that would
  need a prompt are **soft-denied, and the run still exits `0`** with a notice on stderr.
  Fine-grained allow rules live in settings (`permissions.allow`, for example `command(git)` or
  `write_file(src/)`). `--sandbox` exists.
- `--print-timeout` defaults to `0s`, meaning no limit.
- Auth: headless runs reuse cached credentials from an earlier interactive login. Without them, the
  run exits with an "authentication required" error instead of hanging.
- No working-directory flag was found; the adapter must set the child process's cwd.
- No ACP entry point was found on the CLI.

**Known problem: empty stdout without a TTY** ⚠️. Issue
[google-antigravity/antigravity-cli#76](https://github.com/google-antigravity/antigravity-cli/issues/76)
reports that `agy -p` can complete a full round trip and print nothing when stdout is a pipe, and
can hang when stdin is an open pipe. Reports span versions 1.0.0 to 1.0.16 on Windows, macOS and
Linux. Commenters say later versions fix it, but we did not confirm the issue's final state.
Workarounds reported: close stdin (`< /dev/null`), wrap the call in a pseudo-terminal, or read the
answer from the CLI's own transcript file. The Hufu adapter must close stdin and must treat
"exit 0 with empty output" as a failure.

**Terms of use** ⚠️. Two forum threads ask whether an external orchestrator may invoke `agy -p`
under a personal subscription. One received a "yes, fully supported" reply from a poster whose staff
status was not visible; the other received no policy answer. Treat this as unresolved and re-check
before recommending unattended use on subscription accounts.

### 3.5 Kimi Code CLI

Source: `moonshotai.github.io/kimi-cli` reference pages. Evidence ✅.

- `--print` is the non-interactive mode and **implicitly enables `--afk`**, which auto-approves all
  tool calls and auto-dismisses questions. In print mode there is no way to run with tighter
  permissions.
- `-p/--prompt`, `--output-format text | stream-json`, `--final-message-only`, `--quiet`.
- `-m/--model`, `--thinking` / `--no-thinking`.
- `--continue`, `--session [ID]`, `-w/--work-dir`, `--add-dir`.
- `kimi acp` starts a multi-session ACP server. Not logged in returns `AUTH_REQUIRED` (`-32000`).
- Auth: `kimi login` (browser OAuth). An API-key environment variable was not found.

### 3.6 pi

Source: the `@earendil-works/pi-coding-agent` README and npm metadata. Evidence ✅. The README read was
that of version 0.83.0; the latest version on npm on the same day was 1.5.2, so flags should be
re-checked against the installed release.

- Install: `npm install -g --ignore-scripts @earendil-works/pi-coding-agent`; binary `pi`. The
  current release requires Node.js ≥ 22.19.
- Four integration modes: `-p/--print`, `--mode json` (JSON Lines events), `--mode rpc` (JSONL over
  stdin/stdout for process integration) and an embeddable SDK.
- `--provider <name>`, `--model <provider/id[:thinking]>`,
  `--thinking off | minimal | low | medium | high | xhigh | max`.
- Providers listed include Anthropic, OpenAI, Google Gemini, **OpenRouter**, **DeepSeek**,
  **ZAI Coding Plan**, and **Kimi For Coding**. Exact environment-variable names for each
  provider's key were not on the page read.
- By design it has no permission prompts, no sub-agents, no plan mode and no MCP. The README's
  advice is to run it in a container.
- ACP is available through the community `pi-acp` adapter.

**Why pi matters to Hufu.** OpenRouter and similar APIs are model endpoints, not agents: they have
no loop that reads files or runs commands. pi is a small, scriptable harness that turns "a model
slug on some provider" into an agent. One pi adapter therefore covers "any model on OpenRouter",
DeepSeek's API, Z.ai's coding plan and Kimi's coding plan.

### 3.7 ZCode and DeepSeek Harness

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
| `agy` takes `--mode accept-edits\|plan` or `--approve` | The official headless page documents only `--dangerously-skip-permissions`, settings-based allow rules and `--sandbox` | A middle permission tier on `agy` needs settings rules; verify on a real install |
| `agy` headless times out after 5 minutes by default | `--print-timeout` defaults to `0s` (no limit) | Hufu must enforce its own timeout on every job |
| Antigravity has an ACP entry | No `agy acp` command; registry entry points at IDE documentation | `agy` is CLI-adapter only for now |
| Kimi `--print` turns on yolo | It turns on `--afk`, a stronger auto-approve that also dismisses questions | Same practical effect; tighter tiers require the ACP path |
| ZCode has `zcode --prompt` and `--mode build\|plan` | Not found in vendor documentation | ZCode is out of scope until a headless entry is confirmed |
| DeepSeek Harness has a headless mode | Not confirmed | Candidate for the generic ACP adapter only |

## 5. Implications for Hufu

1. **Never rely on an agent's default permissions.** Defaults range from read-only (Codex) to
   approve-everything (Kimi print mode, pi). Each adapter must translate a Hufu permission tier into
   explicit flags and must refuse a tier it cannot honour.
2. **Exit code `0` does not mean the work was done.** `agy` exits `0` after soft-denying actions and
   can exit `0` with empty output. Adapters must inspect the event stream, and a job needs an
   acceptance step that is separate from process success.
3. **Hufu owns timeouts and cancellation.** At least one agent has no default timeout.
4. **Reasoning controls are not uniform.** They range from six named levels to a boolean to nothing.
   A task spec uses an abstract level and each adapter declares what it can map.
5. **Prefer the surface that gives more control.** For Kimi, ACP is the only path with permission
   control. For Codex, the CLI is better documented than its adapter. The preferred surface is a
   per-agent decision recorded in the [adapter contract](../technical/adapter-contract.md).
6. **Open-API models go through pi.** No separate "model adapter" is needed.
7. **Subscription terms are a risk to track**, not something code can solve. Hufu only ever invokes
   official binaries and never reads or reuses their stored credentials.

## 6. Open verification items

These need a machine with the real CLIs installed and logged in:

- [ ] `codex exec --help`: confirm sandbox, approval and reasoning-effort flags and values.
- [ ] `agy --help`: confirm whether a non-interactive middle permission mode exists; confirm the
      state of the non-TTY stdout bug on the current version.
- [ ] Cursor: confirm whether any reasoning-effort control exists (for example model-id suffixes).
- [ ] `kimi acp`: confirm that permission requests are sent to the client and honoured.
- [ ] pi: record provider environment-variable names from `docs/providers.md`.
- [ ] `dsh`: confirm publisher, headless mode and ACP server.
- [ ] Claude Code: re-verify `claude -p` flags when its adapter is written.

## Sources

- Agent Client Protocol: <https://agentclientprotocol.com/llms.txt> and the protocol v1 pages it indexes
- ACP agents and registry: <https://agentclientprotocol.com/get-started/agents>, <https://agentclientprotocol.com/get-started/registry>
- Codex: <https://learn.chatgpt.com/docs/non-interactive-mode>, <https://learn.chatgpt.com/docs/developer-commands?surface=cli>, <https://learn.chatgpt.com/docs/config-file/config-reference>
- Cursor: <https://cursor.com/docs/cli/headless>, <https://cursor.com/docs/cli/reference/parameters>, <https://cursor.com/docs/cli/acp>, <https://cursor.com/docs/cli/reference/output-format>
- Antigravity: <https://antigravity.google/docs/cli/headless>, <https://github.com/google-antigravity/antigravity-cli/issues/76>, <https://discuss.ai.google.dev/t/183051>, <https://discuss.ai.google.dev/t/179013>
- Kimi Code: <https://moonshotai.github.io/kimi-cli/en/reference/kimi-command.html>, <https://moonshotai.github.io/kimi-cli/en/reference/kimi-acp.html>
- pi: <https://www.npmjs.com/package/@earendil-works/pi-coding-agent>
- ZCode: <https://zcode.z.ai>
- Zed external agents: <https://zed.dev/docs/ai/external-agents>
