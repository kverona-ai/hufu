# Prior Art, Positioning and Naming

| | |
|---|---|
| **Status** | Research snapshot |
| **Verified on** | 2026-11-12 |
| **Feeds into** | [Requirements](../requirements/requirements.md), [ADR-0001](../adr/0001-standalone-local-scheduler.md) |

Before building a scheduler for coding agents we looked at what already exists. This document
lists the projects found, says what Hufu can learn from each, and explains why Hufu is still worth
building. It ends with the result of the name check.

Everything here comes from project pages and package registries read on the date above. None of
these projects was installed or run.

## 1. Existing projects

### 1.1 Orchestrators and runners

| Project | What it is | Drives agents via | Shaped for | License, language |
|---|---|---|---|---|
| [Sortie](https://github.com/sortie-ai/sortie) | Daemon that turns tracker tickets (GitHub, GitLab, Linear, Jira) into agent sessions, each in its own workspace. Session metadata, retries and run history persist in SQLite. | Native CLI adapters for Claude Code, Codex, Copilot CLI and OpenCode, plus a generic ACP adapter | Ticket-driven automation | Apache-2.0, Go |
| [Tutti](https://github.com/nutthouse/tutti) | CLI (`tt`) that runs Claude Code, Codex and Aider in tmux sessions, one git worktree per agent. Has run records, checkpoints and a local HTTP control API. | tmux terminal sessions | A person steering a team of agents | MIT, Rust |
| [Jockey](https://github.com/recailai/jockey) | Desktop multi-agent orchestrator coordinating Claude Code, Gemini CLI and Codex | ACP | Desktop UI | Tauri, Rust, SolidJS |
| [Raven](https://github.com/EverMind-AI/Raven) | Host agent (CLI, TUI, web) that plans a task as a DAG and runs nodes on a built-in agent or a launched ACP agent | ACP | Its own host agent | Not checked |
| [Kronos](https://github.com/Reqeique/Kronos) | Self-hosted scheduler that runs ACP agent tasks on schedules | ACP | Scheduled jobs | Not checked |
| [Compozy](https://github.com/compozy/compozy) | Runs ACP agents as a team on loops and schedules | ACP | Team loops | Not checked |
| [OctoShell](https://github.com/savvasioakeim/OctoShell) | Runs ACP agents in git worktrees, one per task | ACP | Parallel tasks | Not checked |
| [acpx](https://github.com/openclaw/acpx) | Command-line ACP client | ACP | Scripting | Not checked |
| `acp-runner` (npm, 0.1.0-beta.6) | Local runner for multi-step agent runbooks (`agent.yaml`) over ACP | ACP | Runbooks | Not checked |

Two names from the earlier analysis, SpurCode and Orca, could not be located on the ACP clients
page or elsewhere, so they are not assessed here.

### 1.2 Bridges from Claude Code to one other agent

| Project | What it is |
|---|---|
| [clagy](https://glama.ai/mcp/servers/mateo-cuello/orchestrator-claude--agy) | MCP server by an individual developer (MIT, Node.js). Claude orchestrates; `agy -p --output-format stream-json` workers run in parallel. Offers dispatch, fan-out, background start and result tools. Listed as "activity slowing". |
| agy-bridge, claude-to-agy, agymcp, antigravity-for-claude-code | Community wrappers that expose `agy` to Claude Code as an MCP tool, a delegate tool or a plugin with a routing hook. Named in the earlier analysis; not re-read. |

These prove the demand: people already want one agent to hand work to another. Each is tied to one
host (Claude Code) and one worker (`agy`).

## 2. What Hufu takes from them

| Idea | Seen in | How Hufu uses it |
|---|---|---|
| A generic ACP adapter next to native CLI adapters | Sortie | The same two-surface adapter layer ([ADR-0002](../adr/0002-acp-first-cli-fallback.md)) |
| One git worktree per task | Tutti, OctoShell | Optional `isolation: worktree` on a task |
| Persisted run records and resumable runs | Sortie, Tutti | Jobs live on disk and outlive the caller ([ADR-0006](../adr/0006-file-based-job-store.md)) |
| Background start plus a separate result call | clagy | `hufu run` returns a job id at once; `hufu result` fetches later |
| Plan as a DAG, then dispatch | Raven | The mission plan format ([Technical design, section 9](../technical/technical-design.md#9-missions-the-managed-advisor-loop)) |

## 3. Where Hufu differs

No project found combines all four of these:

1. **The commander is pluggable.** In every project above the "brain" is fixed: a human at a UI,
   Claude Code as host, a built-in planner, or a ticket queue. In Hufu the advisor is a role that
   any configured agent or model can fill, and it can be swapped per mission.
2. **Built to be called by an agent, not watched by a person.** The primary interface is a
   non-interactive CLI with JSON output and stable exit codes, so any agent that can run a shell
   command can use it. No UI, no tmux.
3. **Explicit permission and reasoning mapping.** One permission tier and one reasoning level in the
   task spec, translated per agent, with a hard error when an agent cannot honour the request. None
   of the surveyed projects documents this as a contract.
4. **Open-API models as first-class executors.** Any model on OpenRouter, DeepSeek, Z.ai or Kimi's
   coding plan becomes an executor through the pi harness, using the same task spec.

**Build or adopt?** Adopt the patterns, build the core. Sortie is the closest in architecture and
is worth reading before the adapter layer is written, but its unit of work is a tracker ticket and
its control loop is its own. Hufu's unit of work is a task handed over by another agent.

## 4. Naming

### 4.1 Why "Hufu"

A hufu (虎符, tiger tally) was the token used in ancient China to authorise troop movements. It was
made in two halves: the ruler kept one and the commander in the field held the other. An order was
valid only when the halves were brought together and matched.

The metaphor maps onto the product directly:

- **Whoever holds the tally commands.** The name refers to the right to dispatch, not to a fixed
  commander. Any agent or model can be the advisor.
- **Matching halves make the order valid.** A task spec and its result must match before work is
  accepted. Hufu calls this acceptance step the *tally check*.
- **Short and usable.** Four letters, easy to type: `hufu run`, `hufu status`.

### 4.2 Name availability

| Registry | `hufu` | Detail |
|---|---|---|
| npm | **Free** | `hufu`, `hufu-cli` and the `@hufu` scope all returned 404 on 2026-11-12 |
| PyPI | Taken | `hufu` 0.5.4, "HuFu (虎符): neuro-symbolic data-quality verification", by another team |
| crates.io | `hufu` taken; **`hufu-cli` free** | `hufu` 0.3.0, "Credential-free, independently verifiable evidence trail for irreversible operations", by another author. `hufu-cli` returned no result on 2026-11-12 |
| GitHub | `kverona-ai/hufu` | This repository |

Other candidates considered earlier and rejected because same-field projects already use them:
Podium, Tutti, Muster, Sortie, Maestro, Conductor, Baton.

**Consequences**

- Hufu is written in Rust ([ADR-0007](../adr/0007-implement-in-rust.md)). Because `hufu` is taken
  on crates.io, the crate is published as **`hufu-cli`** and installs a command named `hufu`. This
  was confirmed by the project owner.
- The free npm name `hufu` is kept for an optional installer package. Both names should be reserved
  before the first public release.
- Two unrelated projects already use the same name and the same tiger-tally story in other
  ecosystems. They are in different fields (data quality, audit evidence), so confusion is unlikely,
  but public descriptions should always say "Hufu, the multi-agent dispatch layer".
- Not checked: trademarks and domain names.

## Sources

- ACP clients list: <https://agentclientprotocol.com/overview/clients>
- Sortie: <https://github.com/sortie-ai/sortie>, <https://docs.sortie-ai.com/>
- Tutti: <https://docs.rs/tutti>
- clagy: <https://glama.ai/mcp/servers/mateo-cuello/orchestrator-claude--agy>
- Registries: <https://registry.npmjs.org/hufu>, <https://pypi.org/project/hufu/>, <https://crates.io/crates/hufu>
