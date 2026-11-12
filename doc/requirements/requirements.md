# Requirements

| | |
|---|---|
| **Status** | Draft for review |
| **Last updated** | 2026-11-12 |
| **Related** | [Research](../research/agent-landscape.md), [Architecture](../architecture/architecture.md), [Roadmap](../roadmap.md) |

## 1. Background

Developers now keep several coding agents installed side by side: Claude Code, Codex, Cursor,
Antigravity, Kimi Code, pi and others. Each has different strengths, quotas and prices. People want
one agent to plan and judge while others do the bulk work, and they want to choose that pairing
per task.

Today this is done with one-off glue: a shell snippet in `CLAUDE.md`, or a community bridge that
ties one host to one worker. That glue breaks in the same ways every time:

- **Each agent is driven differently.** Flags, output formats and exit codes all differ
  ([Agent landscape](../research/agent-landscape.md)).
- **Permission defaults are wildly different.** One agent defaults to read-only, another approves
  everything the moment it runs headless. Glue that relies on defaults is unsafe.
- **Long tasks die with the session.** A delegated task that takes twenty minutes is lost when the
  calling session ends, hits a tool timeout or is compacted.
- **The commander is hard-wired.** Existing bridges assume a fixed host. Swapping the planner for a
  different agent or model means starting over.

## 2. Product statement

> **Hufu is a multi-agent dispatch layer with a pluggable commander: any agent or model holds the
> tally, and dispatches any agent to do the work.**

It is a local scheduler. A caller hands it a task spec; Hufu runs the chosen agent with explicit
permissions, keeps the job alive and recorded on disk, and returns a result that can be checked
against the spec.

## 3. Goals and non-goals

### Goals

| ID | Goal |
|---|---|
| G1 | One task spec drives any supported agent, with the same commands and the same result format |
| G2 | Jobs outlive the caller: state and logs are on disk, and any later session can query them |
| G3 | The advisor role is configuration, not code: any agent or model can plan, dispatch and review |
| G4 | Permissions and reasoning effort are explicit in every run and never left to an agent's default |
| G5 | Adding an ACP-compliant agent is a configuration change, not a code change |
| G6 | Models on open APIs (OpenRouter, DeepSeek, Z.ai, Kimi) can be executors or advisors |

### Non-goals

- **Not an agent.** Hufu has no model loop, no prompt engineering of its own and no tools for
  reading or editing code. It dispatches agents that do.
- **Not an IDE or dashboard.** The interface is a CLI and, later, an MCP server. A UI may come from
  others.
- **Not a credential manager.** Hufu never stores, reads or forwards an agent's login. Each agent
  authenticates the way its vendor intends.
- **Not a hosted service.** Hufu runs on the user's machine, next to the agents it drives.
- **Not a ticket-to-PR pipeline.** Tracker integration is left to tools built for it.

## 4. Users

| Persona | Situation | What they need from Hufu |
|---|---|---|
| **Host-driven developer** | Works in Claude Code (or another agent) all day and wants it to hand heavy or parallel work to cheaper or faster agents | A few commands the host agent can call, results that come back compactly, jobs that survive session loss |
| **Mixer** | Wants a specific model as the planner, for example a strong reasoning model through OpenRouter, with Codex and Kimi as executors | To define an advisor and a roster in configuration and start a mission with one command |
| **Automator** | Runs agents from scripts, cron or CI | Stable exit codes, JSON output, timeouts, no prompts |
| **Adapter author** | Wants to add a new agent | A small, documented adapter contract and a conformance test suite |

## 5. Concepts

| Term | Meaning |
|---|---|
| **Agent** | An external coding agent that Hufu can launch, such as Codex or Kimi Code |
| **Adapter** | The code that knows how to drive one kind of agent. Two families: ACP and headless CLI |
| **Agent profile** | A named configuration: adapter, model, reasoning effort and default permission tier. For example `codex-high` or `or-deepseek` |
| **Role** | What a profile does in a given piece of work: **advisor** (plans, dispatches, reviews) or **executor** (does the task). A role belongs to the mission, not to the agent |
| **Task** | A unit of work described by a task spec: prompt, workspace, profile, permission tier, effort, timeout, acceptance criteria |
| **Job** | One execution of a task by one agent. It has an id, a state, an event log and a result |
| **Mission** | A goal handed to an advisor, which breaks it into tasks, dispatches them and reviews the results |
| **Tally check** | The acceptance step that compares a job's result with the task's acceptance criteria |
| **Permission tier** | Hufu's uniform permission level: `read-only`, `workspace-write` or `full-access` |
| **Effort** | Hufu's uniform reasoning level: `off`, `low`, `medium`, `high`, `max`, or `default` |

## 6. Scenarios

**S1. Delegate one task and keep working.** In Claude Code, the user asks for a large mechanical
refactor. Claude runs `hufu run --agent codex-high --permission workspace-write "<task>"`, gets a
job id back at once and continues the conversation. Later it runs `hufu result <id>` and reads a
short summary plus the list of changed files.

**S2. Fan out.** The host agent submits five independent tasks to three different agents, each in
its own git worktree, waits with `hufu wait`, and collects five results.

**S3. Survive a lost session.** A job is submitted, then the calling session crashes. A new session
runs `hufu status` and finds the job still running, or finished with its result intact.

**S4. Swap the commander.** The user defines an advisor profile that runs a reasoning model through
pi on OpenRouter, and a roster of Codex and Kimi. `hufu mission start --advisor planner "<goal>"`
makes the advisor produce a plan; Hufu dispatches the tasks; the advisor reviews each result and
either accepts it or sends a follow-up.

**S5. Refuse unsafe requests.** A task asks for `read-only` on an agent whose headless mode
approves everything. Hufu rejects the task before anything runs and says which tiers that profile
supports.

**S6. Add an agent by configuration.** A new ACP-compliant agent appears. The user adds six lines
to the configuration naming its launch command, runs `hufu doctor`, and uses it.

**S7. Unattended run.** A nightly script runs `hufu run --wait --json` with a timeout and uses the
exit code to decide whether to open a pull request.

## 7. Functional requirements

Priority: **P0** must have for the milestone named, **P1** should have, **P2** later.
Milestones are defined in the [roadmap](../roadmap.md).

### 7.1 Tasks and jobs

| ID | Requirement | Pri | Milestone |
|---|---|---|---|
| FR-01 | Submit a task from flags or from a YAML/JSON spec file. Submission validates the spec and returns a job id without waiting for the agent | P0 | M1 |
| FR-02 | Query one job or all jobs: state, profile, timings, last activity | P0 | M1 |
| FR-03 | Fetch a job's result: final message, structured output if requested, changed files, usage if reported, warnings | P0 | M1 |
| FR-04 | Read a job's event log, optionally following it live, in normalized or raw form | P0 | M1 |
| FR-05 | Cancel a running job. The agent is asked to stop, then terminated after a grace period | P0 | M1 |
| FR-06 | Block until one or more jobs finish, with a timeout (`hufu wait`, `hufu run --wait`) | P0 | M1 |
| FR-07 | Send a follow-up prompt to a finished job's session when the agent supports resuming | P1 | M1 |
| FR-08 | Enforce a per-job timeout in Hufu itself, independent of the agent | P0 | M1 |
| FR-09 | Request structured output against a JSON Schema where the agent supports it; validate the output | P1 | M2 |
| FR-10 | Retry policy per task: number of attempts and which failure classes are retried | P2 | M4 |

### 7.2 Durability

| ID | Requirement | Pri | Milestone |
|---|---|---|---|
| FR-11 | Job spec, state, events and result are written to disk as the job runs | P0 | M1 |
| FR-12 | A job keeps running after the process that submitted it exits | P0 | M1 |
| FR-13 | A job whose runner died is detected and reported as `lost`, never left as `running` forever | P0 | M1 |
| FR-14 | Retention: old jobs can be listed and pruned by age or count | P1 | M2 |

### 7.3 Agents and adapters

| ID | Requirement | Pri | Milestone |
|---|---|---|---|
| FR-15 | Headless CLI adapters for Antigravity (`agy`) and Codex | P0 | M1 |
| FR-16 | A generic ACP adapter that can drive any ACP agent from configuration | P0 | M2 |
| FR-17 | Verified ACP profiles for Cursor, Kimi Code and pi | P0 | M2 |
| FR-18 | A pi adapter that exposes open-API models (OpenRouter, DeepSeek, Z.ai, Kimi) as profiles | P0 | M2 |
| FR-19 | A Claude Code adapter, so that Claude Code can be an executor under another advisor | P1 | M2 |
| FR-20 | Each adapter declares its capabilities: models, effort levels, permission tiers, resume, structured output, usage reporting | P0 | M1 |
| FR-21 | A task that asks for something the profile cannot do fails validation with a specific message. Nothing is silently ignored or substituted | P0 | M1 |
| FR-22 | `hufu agents` lists profiles and their capabilities; `hufu doctor` checks that each agent is installed, reports its version and login state where it can be detected | P0 | M1 |
| FR-23 | Third-party adapters can be loaded without changing Hufu | P2 | M4 |

### 7.4 Permissions and reasoning

| ID | Requirement | Pri | Milestone |
|---|---|---|---|
| FR-24 | Three permission tiers, `read-only`, `workspace-write`, `full-access`, each translated by the adapter into explicit agent flags or ACP permission answers | P0 | M1 |
| FR-25 | Every run states its tier explicitly. No run depends on the agent's default | P0 | M1 |
| FR-26 | `full-access` is never chosen implicitly; it must appear in the task or profile | P0 | M1 |
| FR-27 | Actions an agent skipped because permission was denied are reported in the result, and by default make the job fail | P0 | M1 |
| FR-28 | One effort scale mapped per adapter. An unmappable level is an error unless the profile opts in to nearest-level fallback | P0 | M1 |
| FR-29 | Model selection per task or profile, validated against what the agent accepts where that can be known | P0 | M1 |

### 7.5 Workspaces and concurrency

| ID | Requirement | Pri | Milestone |
|---|---|---|---|
| FR-30 | A task runs in a named working directory | P0 | M1 |
| FR-31 | Optional isolation: run the task in a fresh git worktree on its own branch and report the branch in the result | P0 | M2 |
| FR-32 | Limit how many jobs run at once, globally and per profile | P1 | M4 |
| FR-33 | Refuse, or warn about, two write-capable jobs in the same directory without isolation | P1 | M2 |

### 7.6 Roles and missions

| ID | Requirement | Pri | Milestone |
|---|---|---|---|
| FR-34 | **External advisor.** Any agent that can run shell commands can act as the advisor by calling the CLI. A ready-made skill and a `CLAUDE.md` snippet are provided for Claude Code | P0 | M1 |
| FR-35 | **Managed advisor.** `hufu mission start --advisor <profile> "<goal>"` runs the advisor through the same adapter layer and asks it for a plan in a defined JSON format | P0 | M3 |
| FR-36 | The advisor is given the roster: which executor profiles exist and what each can do | P0 | M3 |
| FR-37 | Hufu validates the plan, then dispatches tasks in dependency order within the concurrency limit | P0 | M3 |
| FR-38 | **Tally check.** Each task may define acceptance checks: commands that must exit 0, limits on which paths may change, schema validation of structured output, and advisor review | P0 | M3 |
| FR-39 | The advisor receives each result with its check outcomes and returns a verdict: accept, revise with a follow-up, or abort | P0 | M3 |
| FR-40 | A mission has budgets: maximum rounds, wall-clock time and, where usage is reported, tokens or cost. It stops when one is exhausted | P0 | M3 |
| FR-41 | Any profile can be an advisor in one mission and an executor in another | P0 | M3 |
| FR-42 | A mission can be inspected, paused and resumed; its plan and every verdict are on disk | P1 | M3 |

### 7.7 Integration surfaces

| ID | Requirement | Pri | Milestone |
|---|---|---|---|
| FR-43 | Every command supports `--json` with a documented, versioned output shape | P0 | M1 |
| FR-44 | Exit codes are documented and stable | P0 | M1 |
| FR-45 | An MCP server (`hufu mcp`) exposing submit, status, result, wait and cancel. Submit returns a job id immediately | P1 | M4 |
| FR-46 | A long-running daemon that owns scheduling and quotas, managed by launchd or systemd | P1 | M4 |
| FR-47 | Configuration in a global file and an optional per-project file, with the project overriding the global | P0 | M1 |

### 7.8 Observability

| ID | Requirement | Pri | Milestone |
|---|---|---|---|
| FR-48 | A normalized event stream per job, the same shape for every adapter, alongside the raw agent output | P0 | M1 |
| FR-49 | Token usage and cost per job where the agent reports them, aggregated per mission | P1 | M3 |
| FR-50 | Summaries sized for an agent's context window: `hufu result` defaults to a compact form and offers the full form on request | P0 | M1 |

## 8. Non-functional requirements

| ID | Area | Requirement |
|---|---|---|
| NFR-01 | Local-first | Works with no network service of its own. No telemetry |
| NFR-02 | Security | Never reads, copies or forwards an agent's stored credentials. Only invokes official agent binaries. Passes through only the environment variables a profile lists |
| NFR-03 | Security | Secrets never appear in job records. Environment values are not logged |
| NFR-04 | Safety | Least privilege by default: the default tier is `read-only`. Write access is always an explicit choice |
| NFR-05 | Reliability | State changes are atomic on disk. A crash at any point leaves every job in a readable, truthful state |
| NFR-06 | Reliability | "Exit 0" from an agent is never taken as success on its own |
| NFR-07 | Portability | macOS and Linux from M1. Windows is a later goal; nothing in the design should rule it out |
| NFR-08 | Performance | Submitting a job returns in under one second. Hufu's own overhead per running job stays below 50 MB of memory |
| NFR-09 | Scale | Tens of concurrent jobs and thousands of stored jobs on one machine. Not designed for clusters |
| NFR-10 | Extensibility | A new CLI adapter is under about 300 lines and passes a shared conformance suite |
| NFR-11 | Compatibility | JSON outputs and the task spec carry a version. Breaking changes bump it |
| NFR-12 | Usability | Every validation error says what was wrong, which profile or field, and what would be accepted |
| NFR-13 | Licence | Apache-2.0. Dependencies must be compatible with it |

## 9. Constraints and assumptions

- The agents are installed and logged in by the user, on the same machine where Hufu runs.
  Subscription logins cannot be moved into CI or a cloud container.
- Agent CLIs change without notice. Adapters must check the installed version and fail clearly on
  one they do not understand.
- Whether a vendor permits its CLI to be driven by an outside orchestrator under a subscription
  plan is that vendor's decision. For Antigravity this is not yet clearly answered
  ([Agent landscape, section 3.4](../research/agent-landscape.md#34-antigravity-cli-agy)).
- ACP governs the conversation, not the sandbox. Hufu cannot make an agent safer than the agent's
  own controls allow.

## 10. Success measures

| Measure | Target |
|---|---|
| Agents reachable through one task spec at the end of M2 | At least six, including one open-API model through pi |
| Jobs that survive the death of their submitting session, in the durability test | 100% |
| Time for a new user to delegate a first task from Claude Code, with one agent already installed | Under ten minutes |
| Cases in the conformance suite where an unsupported tier or effort level is silently accepted | Zero |
| Adding a new ACP-compliant agent | Configuration only |

## 11. Open questions

| # | Question | Owner | Needed by |
|---|---|---|---|
| Q1 | Is TypeScript on Node.js the right implementation choice? Proposed in [ADR-0003](../adr/0003-typescript-on-node.md) | Project owner | Before M1 |
| Q2 | Should `agy` be the first adapter, given its non-TTY output bug and the open terms-of-use question? Codex first would be lower risk | Project owner | Before M1 |
| Q3 | When a permission request falls outside the tier, should Hufu always reject, or may it pause the job and ask the advisor? | Design | M3 |
| Q4 | Does the advisor review every result, or only those whose automatic checks fail? This trades cost against assurance | Design | M3 |
| Q5 | Is Windows support wanted before 1.0? | Project owner | M4 |
| Q6 | How are results delivered to a host that cannot poll, for example through a hook or a notification? | Design | M4 |
