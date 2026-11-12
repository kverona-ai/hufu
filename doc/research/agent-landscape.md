# Agent Landscape: How Coding Agents Can Be Driven Headlessly

| | |
|---|---|
| **Status** | Research snapshot. Refreshed when the adapters were written (Hufu 0.1); the flags each adapter passes are in the [adapter contract](../technical/adapter-contract.md) |
| **Verified on** | 2026-11-12, against vendor documentation. No agent was run with a real account |
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
| **Codex** (OpenAI) | `codex exec` ✅ | Via `codex-acp` adapter ✅ | `-m/--model` ◐ | `-c model_reasoning_effort="<level>"` ◐ | Read-only sandbox ◐ |
| **Cursor** | `agent -p` ✅ | `agent acp`, a hidden command ✅ | `--model` ✅ | ❓ none found | Edits inside the workspace are applied; other actions rejected ✅ |
| **Antigravity** (Google) | `agy -p` ✅ | ❓ no `agy acp`; a registry entry exists but points at IDE docs | `--model <slug>` ✅ | `--effort low\|medium\|high` ✅ (the reference also lists `max` ◐) | Ask-only actions are soft-denied and the run still exits `0` ✅ |
| **Kimi Code** (Moonshot) | `kimi -p` ✅ | `kimi acp` ✅ | `-m/--model` ✅ | Environment variable `KIMI_MODEL_THINKING_EFFORT=low…max` ✅; cannot be switched off | Runs every tool without asking; only deny rules apply ✅ |
| **pi** | `pi -p`, `--mode json`, `--mode rpc`, SDK ✅ | Via `pi-acp` adapter ✅ | `--provider`, `--model provider/id` ✅ | `--thinking off…max` ✅ | No permission system at all ✅ |
| **Claude Code** (Anthropic) | `claude -p` ✅ | Via `claude-agent-acp` adapter ✅ | `--model` ✅ | `--effort low…max` ✅ | Calls that need approval are denied ✅ |
| **Gemini CLI**, **OpenCode**, **Goose**, **Qwen Code** | Not checked | Listed as ACP agents ✅ | Through ACP config options | Through ACP config options | Through ACP permission requests |
| **DeepSeek Harness** (`dsh`) | ❓ | Reported to run an ACP server ⚠️ | ❓ | ❓ | ❓ |
| **ZCode** (Z.ai) | ❓ no CLI documented | ❓ | In-app only | ❓ | ❓ |

### 3.2 Codex CLI

Source: `learn.chatgpt.com/docs/non-interactive-mode`, `…/developer-commands`, `…/config-reference`
(the former `developers.openai.com/codex/*` pages redirect here), and the option list in the
`codex exec` source. Evidence: ✅ for the options of `exec` itself and for the event stream, ◐ for
the options shared with the interactive command (`-m`, `--sandbox`, `-C`, `-c`), which were read in
documentation only.

- `codex exec "<prompt>"`; `codex exec -` reads the prompt from stdin.
- Progress goes to stderr and only the final agent message goes to stdout. With `--json`, stdout
  becomes JSON Lines with events such as `thread.started`, `turn.started`, `turn.completed`,
  `turn.failed`, `item.*` and `error`.
- `--output-schema <file>` requests a final message matching a JSON Schema.
  `-o/--output-last-message <file>` also writes the final message to a file.
- `--sandbox read-only | workspace-write | danger-full-access`. **`codex exec` defaults to
  read-only**, so an executor that should change files must be given `workspace-write` explicitly.
- `--dangerously-bypass-approvals-and-sandbox` (alias `--yolo`) removes the sandbox and all
  approvals. `--full-auto` is reported as no longer supported. `--ask-for-approval` is documented
  for the interactive command; we could not confirm that `codex exec` accepts it, so the adapter
  does not pass it.
- `--model/-m`; reasoning effort through `-c model_reasoning_effort="<level>"`. The value is parsed
  as TOML, so a string needs its quotes. Values listed: `low`, `medium`, `high`, `xhigh`, `max`,
  `ultra`; which of them a model accepts depends on the model.
- `codex exec resume <SESSION_ID>` and `codex exec resume --last`. Options that are not global
  (`--sandbox`, `--cd`, `-c`, `--add-dir`) must come before the word `resume`.
- Event items seen in the stream: `agent_message`, `reasoning`, `command_execution` (with a status
  that can be `declined`), `file_change`, `mcp_tool_call`, `web_search`, `todo_list`, `error`.
  `turn.completed` carries token usage.
- MCP servers are declared in `config.toml` under `[mcp_servers.<name>]`; the same keys can be given
  per run with `-c`. The configuration directory is `CODEX_HOME`, default `~/.codex`.
- Exit codes are not documented ❓. The adapter decides success from the event stream.
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
  `write_file(src/)`). `--sandbox` exists. The reference lists `--mode accept-edits | auto | plan`,
  but not on the headless page ◐; the adapter uses `--mode plan --sandbox` for read-only jobs and
  backs it with Hufu's own before-and-after comparison of the workspace.
- `stream-json` prints one object per line with an `event` of `init`, `step_update` or `result`;
  the last one carries the same envelope as `--output-format json`.
- MCP servers are read from `~/.gemini/config/mcp_config.json`, or `.agents/mcp_config.json` in the
  workspace. There is no flag to attach one for a single run, and no flag or environment variable
  that moves the configuration directory, so a second `agy` account needs a second operating-system
  user or home directory.
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

Source: the Kimi Code CLI documentation at `code.kimi.com`, and the reference pages of its
predecessor at `moonshotai.github.io/kimi-cli`. Evidence ✅ unless marked.

The first pass of this research read the predecessor, `kimi-cli`, which is now archived. Its
successor keeps the binary name `kimi` and changes several flags. The points below describe the
successor (npm package `@moonshot-ai/kimi-code`, Node.js ≥ 22.19, data in `~/.kimi-code` or
`KIMI_CODE_HOME`).

- `kimi -p "<prompt>"` is the non-interactive mode. It cannot be combined with `--yolo`, `--auto`
  or `--plan`: print mode **always runs tools without asking**, and only deny rules still apply.
  There is no read-only print mode.
- `--output-format text | stream-json`. The stream holds assistant messages, with their tool calls,
  and tool results, one per line. It has no final "result" line, no usage and no session id ◐, so a
  print-mode job cannot be resumed by Hufu.
- `-m/--model`. There is no thinking flag; the level comes from the environment variable
  `KIMI_MODEL_THINKING_EFFORT=low | medium | high | xhigh | max`, and thinking cannot be turned
  off. There is no working-directory flag; the process's directory is used.
- `kimi acp` starts an ACP server over stdio. Not logged in returns `AUTH_REQUIRED` (`-32000`). It
  supports `session/set_mode`, a `model` config option, `session/load` and `session/resume`.
- MCP servers are read from `mcp.json` in the data directory, or `.kimi-code/mcp.json` in the
  project. Headless runs ignore the project file unless `KIMI_CODE_TRUST_WORKSPACE=1`.
- Auth: `kimi login` (device code). Provider keys are read from `config.toml`, not from the shell
  environment.
- Exit codes of print mode are not documented ❓ (the predecessor used `0`, `1` and `75` for
  "retry later").

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
  **ZAI Coding Plan**, and **Kimi For Coding**.
- By design it has no permission prompts, no sub-agents and no plan mode. The README's advice is
  to run it in a container. What a run may do is limited by its tool list instead:
  `--tools read,bash,edit,write` is the default, and `--tools read --no-mcp` leaves an agent that
  can only read. That is how the adapter offers a read-only tier; there is nothing between
  read-only and everything.
- `--mode json` starts with a `session` line that carries the session id and ends with `agent_end`;
  messages carry usage with a cost. `pi auth check --provider <name> --json` reports whether a
  provider is ready.
- Provider keys come from the environment: `OPENROUTER_API_KEY`, `DEEPSEEK_API_KEY`, `ZAI_API_KEY`,
  `KIMI_API_KEY`, `MOONSHOT_API_KEY`, `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `GEMINI_API_KEY` and
  others. The configuration directory is `PI_CODING_AGENT_DIR`.
- ACP is available through the community `pi-acp` adapter.

**Why pi matters to Hufu.** OpenRouter and similar APIs are model endpoints, not agents: they have
no loop that reads files or runs commands. pi is a small, scriptable harness that turns "a model
slug on some provider" into an agent. One pi adapter therefore covers "any model on OpenRouter",
DeepSeek's API, Z.ai's coding plan and Kimi's coding plan.

### 3.7 Claude Code

Source: the Claude Code CLI reference and headless documentation at `code.claude.com/docs`.
Evidence ✅.

- `claude -p` is the non-interactive mode; the prompt can be piped on stdin.
  `--output-format text | json | stream-json`; `stream-json` needs `--verbose`.
- `--permission-mode default | acceptEdits | plan | auto | dontAsk | bypassPermissions`, and
  `--dangerously-skip-permissions`. Since version 2.1.302 a print-mode run with no mode denies
  every call that would need approval. `plan` gives a read-only run and `acceptEdits` one that may
  edit files; shell commands outside the allow rules are still denied in `acceptEdits`.
- `--model`, `--effort low | medium | high | xhigh | max`, `--resume <id>`, `--json-schema`,
  `--mcp-config <file or JSON>`, `--add-dir`. There is no working-directory flag.
- The final `result` message carries `session_id`, `total_cost_usd`, `usage`, `structured_output`
  and `permission_denials`, so denied actions can be read without guessing from text.
- Not logged in prints "Not logged in" as the result. `claude auth status` exits `0` or `1`. The
  configuration directory is `CLAUDE_CONFIG_DIR`.
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
| Antigravity has an ACP entry | No `agy acp` command; registry entry points at IDE documentation | `agy` is CLI-adapter only for now |
| Kimi `--print` turns on yolo | Print mode runs every tool without asking and cannot be combined with `--plan` | Same practical effect; tighter tiers require the ACP path |
| Kimi takes `--thinking`, `--print` and `-w` | Those belong to the archived `kimi-cli`. Kimi Code uses `-p`, the process's directory and `KIMI_MODEL_THINKING_EFFORT` | The adapter targets Kimi Code |
| pi has no MCP and no way to limit a run | It has `--tools` and `--no-mcp` | pi gets a read-only tier |
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
4. **Reasoning controls are not uniform.** They range from seven named levels, through a
   configuration key and an environment variable, to nothing.
   A task spec uses an abstract level and each adapter declares what it can map.
5. **Prefer the surface that gives more control.** For Kimi, ACP is the only path with permission
   control. For Codex, the CLI is better documented than its adapter. The preferred surface is a
   per-agent decision recorded in the [adapter contract](../technical/adapter-contract.md).
6. **Open-API models go through pi.** No separate "model adapter" is needed.
7. **Subscription terms are a risk to track**, not something code can solve. Hufu only ever invokes
   official binaries and never reads or reuses their stored credentials.

## 6. Open verification items

The adapters were written from the documentation above and are tested against fake agents that
print what the documentation says. The items below need a machine with the real CLIs installed and
logged in. `hufu doctor` reports the installed version of each agent; record it here when an item
is closed.

- [ ] Every adapter: one read-only job, one writing job, one cancel and one resume against the real
      agent (the first item of [what is next](../roadmap.md#next)).
- [ ] `codex exec --help`: confirm the sandbox and reasoning-effort options and their values, and
      that options before `resume` are accepted as documented.
- [ ] `agy --help`: confirm that `--mode plan` and `--sandbox` apply to `-p`; confirm the state of
      the non-TTY stdout bug on the current version; record the text of the soft-denial notice.
- [ ] Cursor: confirm whether any reasoning-effort control exists (for example model-id suffixes).
- [ ] `kimi acp` and `agent acp`: confirm that permission requests are sent to the client and
      honoured. Until then Hufu offers these profiles `full-access` only, unless the profile is
      marked `permission_requests: verified`.
- [ ] Kimi Code print mode: record the exact keys of the stream and the exit codes.
- [ ] `dsh`: confirm publisher, headless mode and ACP server.

Closed in this pass: Claude Code's flags, pi's provider environment variables.

## Sources

- Agent Client Protocol: <https://agentclientprotocol.com/llms.txt> and the protocol v1 pages it indexes
- ACP agents and registry: <https://agentclientprotocol.com/get-started/agents>, <https://agentclientprotocol.com/get-started/registry>
- Codex: <https://learn.chatgpt.com/docs/non-interactive-mode>, <https://learn.chatgpt.com/docs/developer-commands?surface=cli>, <https://learn.chatgpt.com/docs/config-file/config-reference>
- Cursor: <https://cursor.com/docs/cli/headless>, <https://cursor.com/docs/cli/reference/parameters>, <https://cursor.com/docs/cli/acp>, <https://cursor.com/docs/cli/reference/output-format>
- Antigravity: <https://antigravity.google/docs/cli/headless>, <https://github.com/google-antigravity/antigravity-cli/issues/76>, <https://discuss.ai.google.dev/t/183051>, <https://discuss.ai.google.dev/t/179013>
- Kimi Code: <https://code.kimi.com>, and for the predecessor <https://moonshotai.github.io/kimi-cli/en/reference/kimi-command.html>, <https://moonshotai.github.io/kimi-cli/en/reference/kimi-acp.html>
- Claude Code: <https://code.claude.com/docs/en/cli-reference>, <https://code.claude.com/docs/en/headless>
- pi: <https://www.npmjs.com/package/@earendil-works/pi-coding-agent>
- ZCode: <https://zcode.z.ai>
- Zed external agents: <https://zed.dev/docs/ai/external-agents>
