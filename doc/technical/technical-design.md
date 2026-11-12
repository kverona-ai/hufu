# Technical Design

| | |
|---|---|
| **Status** | Draft for review. Nothing here is implemented yet |
| **Last updated** | 2026-11-12 |
| **Related** | [Architecture](../architecture/architecture.md), [Adapter contract](adapter-contract.md), [Requirements](../requirements/requirements.md) |

This document turns the [architecture](../architecture/architecture.md) into concrete formats and
behaviours: the source layout, configuration, the task spec, the CLI, the on-disk store, the event
and result shapes, the mission protocol and the test strategy. How individual agents are driven is
in the [adapter contract](adapter-contract.md).

## 1. Stack

| Concern | Choice | Notes |
|---|---|---|
| Language | TypeScript, strict mode | [ADR-0003](../adr/0003-typescript-on-node.md) |
| Runtime | Node.js ≥ 22.19 | The floor is set by pi, the strictest of the target agents |
| Package manager | pnpm | |
| Schemas | Zod, exported as JSON Schema | One definition validates config, task specs and CLI JSON output |
| CLI framework | A small, dependency-light parser (to be picked in M1) | |
| ACP client | `@agentclientprotocol/sdk` | |
| MCP server | `@modelcontextprotocol/sdk` | M4 |
| Tests | Vitest | |
| Build | `tsc` to ESM; single `hufu` bin | |
| Pseudo-terminal | Optional dependency, only if the `agy` adapter needs it | See [adapter contract](adapter-contract.md#52-antigravity-agy-cli) |

## 2. Source layout

All code, including its manifests and tests, lives under [`src/`](../../src/README.md), which is the
package root. The repository root holds only documentation and repository metadata.

```text
src/
├── package.json            # package "hufu", bin: hufu
├── tsconfig.json
├── cli/                    # command definitions, human and JSON output, exit codes
├── core/
│   ├── spec/               # task spec, config and result schemas; validation
│   ├── registry/           # profiles, adapter lookup, capability cache
│   ├── store/              # job and mission directories, atomic writes
│   ├── runner/             # detached per-job runner: lifecycle, heartbeat, timeout, cancel
│   ├── scheduler/          # queue, concurrency slots, dependency release
│   ├── workspace/          # cwd resolution, git worktrees, change detection
│   ├── tally/              # acceptance checks
│   └── mission/            # advisor loop, plan and verdict handling, budgets
├── adapters/
│   ├── contract.ts         # the Adapter interface and shared types
│   ├── acp/                # generic ACP client adapter
│   ├── codex-cli/
│   ├── agy-cli/
│   ├── cursor-cli/
│   ├── kimi-cli/
│   ├── pi/
│   └── claude-cli/
├── mcp/                    # MCP shell (M4)
├── daemon/                 # daemon (M4)
├── integrations/
│   └── claude-code/        # skill and CLAUDE.md snippet shipped with the package
└── testing/                # fake agents, recorded transcripts, conformance suite
```

Dependency rule: `cli`, `mcp` and `daemon` depend on `core`; `core` depends on
`adapters/contract.ts` only; concrete adapters depend on the contract and nothing in `core`.

## 3. Configuration

Two files, merged with the project file winning:

| File | Scope |
|---|---|
| `~/.hufu/config.yaml` (or `$HUFU_HOME/config.yaml`) | User-wide profiles and defaults |
| `<project>/.hufu/config.yaml` | Project overrides; safe to commit because it holds no secrets |

A full sample is in [`doc/examples/config.yaml`](../examples/config.yaml).

```yaml
version: 1

defaults:
  permission: read-only     # least privilege unless a task says otherwise
  effort: default           # do not pass any effort flag
  timeout: 30m
  inactivity_timeout: 10m
  isolation: none           # or: worktree

limits:
  max_concurrent_jobs: 4
  per_profile:
    codex-high: 2

profiles:
  codex-high:
    adapter: codex-cli
    model: gpt-6.1-sol
    effort: high

  kimi:
    adapter: acp
    command: [kimi, acp]

  or-reasoner:
    adapter: pi
    provider: openrouter
    model: vendor/model-slug
    effort: high
    env: [OPENROUTER_API_KEY]   # names of variables to pass through, never values
```

Profile fields:

| Field | Applies to | Meaning |
|---|---|---|
| `description` | all | One line shown in listings and given to a managed advisor as part of the roster |
| `adapter` | all | Adapter kind: `acp`, `codex-cli`, `agy-cli`, `cursor-cli`, `kimi-cli`, `pi`, `claude-cli` |
| `command` | `acp` | Argv that starts the agent in ACP mode. For CLI adapters, an optional override of the binary path |
| `model` | all | Default model id, passed through verbatim |
| `provider` | `pi` | Provider name as pi knows it |
| `effort` | all | Default effort level |
| `effort_fallback` | all | `error` (default) or `nearest`. See [adapter contract, section 4](adapter-contract.md#4-effort-mapping) |
| `permission` | all | Default tier for this profile. Cannot be `full-access` unless `allow_full_access: true` is also set |
| `env` | all | Names of environment variables the agent process may inherit |
| `permission_requests` | `acp` | `unverified` (default) or `verified`. Tiers below `full-access` are advertised only when `verified`. See [adapter contract, section 3.1](adapter-contract.md#31-mapping) |
| `args` | all | Extra arguments appended verbatim. An escape hatch; adapters reject arguments that would change the permission tier |

A top-level `missions` block sets mission defaults: the review policy, the highest tier any
mission task may request, and the commands an advisor-written acceptance check may run. See
[section 9](#9-missions-the-managed-advisor-loop).

## 4. Task spec

A task can be given as flags, as a file (`-f task.yaml`), or both, with flags overriding the file.
Samples are in [`doc/examples/`](../examples/).

```yaml
version: 1
title: Refactor token refresh
prompt: |
  Refactor the token refresh logic under src/auth so that concurrent refreshes
  share one in-flight request. Keep the public API unchanged. Run the unit tests.
agent: codex-high
cwd: .
permission: workspace-write
effort: high
timeout: 45m
isolation: worktree
output:
  schema: ./schemas/change-summary.json
acceptance:
  - command: npm test -- auth
  - paths:
      allow: ["src/auth/**", "test/auth/**"]
```

| Field | Required | Default | Meaning |
|---|---|---|---|
| `version` | yes | | Spec version. Currently `1` |
| `prompt` | yes | | The instruction given to the agent |
| `agent` | yes | | Profile name |
| `title` | no | First line of the prompt | Shown in listings |
| `cwd` | no | Caller's directory | Workspace root for the agent |
| `permission` | no | Profile, then global default | `read-only`, `workspace-write`, `full-access` |
| `effort` | no | Profile, then global default | `off`, `low`, `medium`, `high`, `max`, `default` |
| `model` | no | Profile | Overrides the profile's model |
| `timeout` | no | Global default | Wall-clock limit enforced by Hufu |
| `inactivity_timeout` | no | Global default | Limit on silence from the agent |
| `isolation` | no | `none` | `worktree` runs the task on a new branch in a new git worktree |
| `output.schema` | no | | JSON Schema for the final answer; requires a profile that supports structured output |
| `acceptance` | no | | Tally checks. See [section 10](#10-tally-checks) |
| `strict_permissions` | no | `true` | When `true`, any denied action fails the job |
| `depends_on` | no | | Job ids that must succeed first. Used by missions |
| `labels` | no | | Free-form key/value pairs for filtering |

**Validation order.** Schema, then profile resolution, then capability checks (tier, effort, model,
structured output, resume), then workspace checks (directory exists; a git repository if
`isolation: worktree`). Any failure rejects the submission with exit code `2` and creates no job.

## 5. Command-line interface

```text
hufu run [-f task.yaml] [--agent P] [--permission T] [--effort E] [--model M]
         [--cwd DIR] [--isolation worktree] [--timeout D] [--wait] [--json] [PROMPT]
hufu status [JOB...] [--all] [--json]
hufu wait JOB... [--timeout D] [--any] [--json]
hufu result JOB [--full] [--json]
hufu logs JOB [--follow] [--raw]
hufu cancel JOB...
hufu resume JOB PROMPT [--wait] [--json]
hufu agents [PROFILE] [--json]
hufu doctor [--json]
hufu prune [--older-than D] [--keep N] [--worktrees]

hufu mission start --advisor P [--executors P,P] [--budget ...] GOAL      # M3
hufu mission status|report|pause|resume|abort MISSION                     # M3
hufu mcp                                                                  # M4
hufu daemon start|stop|status                                             # M4
```

Behaviour that matters to agent callers:

- `hufu run` prints the job id and returns as soon as the job is recorded. `--wait` blocks until a
  terminal state and prints the result.
- `hufu result` is compact by default: state, verdict, final message trimmed to a configurable
  length, changed files, denied actions, warnings. `--full` returns everything.
- `--json` output is a single object with a `v` field. Human output goes to stdout; diagnostics go
  to stderr.

### Exit codes

| Code | Meaning |
|---|---|
| `0` | Success. For `run --wait`, `wait` and `result`: the job succeeded and was not rejected |
| `1` | Internal error in Hufu |
| `2` | Usage or validation error. Nothing was run |
| `3` | Job failed |
| `4` | Job cancelled |
| `5` | Job timed out |
| `6` | `wait` timed out; the job is still running |
| `7` | Job lost |
| `8` | Job succeeded but was rejected by its tally check |
| `10` | Agent unavailable or not authenticated |

## 6. Job store

`$HUFU_HOME` defaults to `~/.hufu`.

```text
~/.hufu/
├── config.yaml
├── jobs/
│   └── j_7k2m9x4qpd/
│       ├── spec.json          # normalized task spec, written once
│       ├── state.json         # current state, rewritten atomically
│       ├── events.ndjson      # normalized events, append-only
│       ├── result.json        # written once, at the terminal state
│       ├── tally.json         # acceptance check outcomes, if any
│       └── raw/
│           ├── stdout.log     # the agent's output, untouched
│           └── stderr.log
├── missions/
│   └── m_3f8h1c5tzn/
│       ├── mission.json       # goal, advisor, roster, budgets
│       ├── plan.json          # the validated plan
│       ├── ledger.ndjson      # every dispatch, check outcome and verdict
│       └── report.md
└── run/
    └── slots/                 # one file per live runner; used for advisory limits before M4
```

- **Ids** are short and time-sortable, prefixed `j_` for jobs and `m_` for missions.
- **Atomicity.** `state.json` and `result.json` are written to a temporary file and renamed.
  `events.ndjson` is appended one complete line at a time.
- **`state.json`** holds the state, runner pid, start time, heartbeat time, attempt number and the
  resolved agent version.
- **No secrets.** `spec.json` records the names of passed-through environment variables, not values.

## 7. Events and results

### 7.1 Normalized events

Every adapter emits the same event types. Each line of `events.ndjson` is one event:

```json
{"v":1,"seq":42,"ts":"2026-11-12T17:03:11.120Z","type":"tool.call","id":"c7","kind":"edit","title":"src/auth/refresh.ts","status":"completed"}
```

| Type | Meaning |
|---|---|
| `job.state` | The job entered a new state |
| `agent.message` | Text from the agent; `final: true` marks the final answer |
| `agent.thought` | Reasoning text, when the agent exposes it |
| `tool.call` | A tool call started or changed status. `kind` uses ACP's vocabulary: `read`, `edit`, `delete`, `move`, `search`, `execute`, `think`, `fetch`, `other` |
| `permission.request` | The agent asked for permission; includes Hufu's decision |
| `permission.denied` | An action was skipped for lack of permission, whether Hufu denied it or the agent denied itself |
| `plan` | The agent published or updated a plan |
| `usage` | Token or cost figures reported by the agent |
| `warning`, `error` | Problems that did or did not stop the job |

### 7.2 Result envelope

```json
{
  "v": 1,
  "job": "j_7k2m9x4qpd",
  "state": "succeeded",
  "verdict": "accepted",
  "agent": {
    "profile": "codex-high", "adapter": "codex-cli", "version": "0.167.2",
    "model": "gpt-6.1-sol", "effort": "high", "permission": "workspace-write"
  },
  "final_message": "Refactored refresh() to share one in-flight promise. 14 tests pass.",
  "structured_output": null,
  "changes": {
    "files": ["src/auth/refresh.ts", "test/auth/refresh.test.ts"],
    "branch": "hufu/j_7k2m9x4qpd", "base": "4c1e9a2"
  },
  "denied_actions": [],
  "warnings": [],
  "usage": {"input_tokens": 48211, "output_tokens": 3907},
  "session": {"id": "…", "resumable": true},
  "timing": {"queued_at": "…", "started_at": "…", "finished_at": "…", "duration_s": 312}
}
```

`changes.files` comes from comparing the git state of the workspace before and after the job when
the workspace is a repository, and from `edit`, `delete` and `move` tool events otherwise.

## 8. Runner mechanics

1. **Detach.** `hufu run` spawns `hufu __runner <job>` as a detached process in its own session,
   with stdio redirected to files, and does not wait for it.
2. **Slot.** The runner takes a slot file. If the configured limit is reached it stays `queued` and
   polls. From M4 the daemon's queue replaces this.
3. **Launch.** The runner resolves the workspace, builds the run request and calls the adapter.
   The agent's environment is a small base set (`PATH`, `HOME`, locale, temp directory, proxy
   variables) plus the variables the profile lists.
4. **Heartbeat.** Every five seconds the runner rewrites `state.json` with a fresh timestamp.
5. **Timeouts.** A wall-clock timer and an inactivity timer run in the runner. Either one firing
   cancels the agent and ends the job as `timed_out`.
6. **Cancel.** `hufu cancel` writes a cancel marker and signals the runner. The runner asks the
   adapter to stop (ACP `session/cancel`, or `SIGTERM` to the agent's process group), waits ten
   seconds, then sends `SIGKILL` to the group.
7. **Finish.** The runner computes changes, writes `result.json`, runs tally checks if the task has
   any, writes the terminal state and releases its slot.
8. **Lost jobs.** Any command that reads a non-terminal job with a heartbeat older than thirty
   seconds and no live process at the recorded pid rewrites it as `lost`.

## 9. Missions: the managed advisor loop

A mission runs an advisor profile through the ordinary adapter layer, at `read-only`, and asks it
for JSON.

### 9.1 What the advisor is given

- The goal, in the user's words.
- The **roster**: each executor profile's name, a one-line description from configuration, and its
  capabilities (tiers, effort levels, whether it can resume).
- The plan schema and the rules: use only roster profiles, request the lowest tier that works,
  give every write task an acceptance check.

### 9.2 Plan

```json
{
  "v": 1,
  "summary": "Split the refresh logic, then update callers, then document.",
  "tasks": [
    {
      "id": "t1",
      "title": "Refactor refresh()",
      "prompt": "…",
      "agent": "codex-high",
      "permission": "workspace-write",
      "effort": "high",
      "depends_on": [],
      "acceptance": [{"command": "npm test -- auth"}]
    },
    {
      "id": "t2",
      "title": "Update callers",
      "prompt": "…",
      "agent": "kimi",
      "permission": "workspace-write",
      "depends_on": ["t1"],
      "acceptance": [{"command": "npm run typecheck"}]
    }
  ]
}
```

Hufu validates the plan the same way it validates a task spec, plus: ids are unique, the dependency
graph has no cycle, every agent is on the roster, and no task exceeds the mission's maximum tier.
An invalid plan is sent back once with the errors; a second invalid plan fails the mission.

### 9.3 Getting JSON out of an agent

In order of preference: the agent's own structured-output feature (Codex `--output-schema`,
Antigravity `--json-schema`); otherwise an instruction to answer with a single fenced JSON block,
which Hufu extracts and validates.

### 9.4 Review

After each job, the advisor receives the task, the compact result and the tally outcomes, and
returns:

```json
{"v": 1, "task": "t1", "verdict": "revise", "reason": "Tests pass but the public type changed.",
 "follow_up": "Restore the original signature of refresh() and keep the shared-promise behaviour."}
```

| Verdict | Effect |
|---|---|
| `accept` | The task is done. Dependants become ready |
| `revise` | The follow-up is sent to the same session if the agent can resume, otherwise as a new job with the earlier result attached. Counts against the task's round limit |
| `abort` | The mission stops and reports |

Whether the advisor reviews every result or only failures is configurable per mission
(`review: always | on_failure`); see open question Q4 in the
[requirements](../requirements/requirements.md#11-open-questions).

### 9.5 Budgets

`max_rounds_per_task`, `max_tasks`, `max_wall_clock`, and `max_tokens` or `max_cost` where usage is
reported. The first one exhausted stops the mission. Finished work is kept.

## 10. Tally checks

Checks run in the job's workspace after the agent finishes, in the order listed. All must pass for
the verdict to be `accepted`.

| Check | Passes when |
|---|---|
| `command: <shell>` | The command exits `0` within its timeout |
| `paths: {allow: [...], deny: [...]}` | Every changed file matches an `allow` glob and none matches a `deny` glob |
| `schema: <file>` | The structured output validates against the schema |
| `no_denied_actions: true` | The agent reported no skipped actions (implied by `strict_permissions`) |
| `review: advisor` | The mission's advisor returns `accept`. Only valid inside a mission |

Check commands run with the caller's privileges, not the agent's tier, so they are part of the
trusted task spec. In a mission, checks written by the advisor are limited to an allow-list of
commands defined in configuration.

## 11. MCP shell (M4)

`hufu mcp` is a stdio MCP server and a thin client of the daemon.

| Tool | Returns |
|---|---|
| `hufu_submit` | A job id, immediately |
| `hufu_status` | State of one or more jobs |
| `hufu_wait` | Blocks for at most a bounded time (default 60 s), then returns the current state |
| `hufu_result` | The compact result; `full: true` for everything |
| `hufu_cancel` | Confirmation |
| `hufu_agents` | Profiles and capabilities |

No tool call ever lasts as long as a job. That keeps Hufu inside every host's tool-call timeout.

## 12. Testing

| Layer | What | How |
|---|---|---|
| Unit | Spec validation, capability checks, tier and effort mapping, store atomicity | Vitest, no processes |
| Fake agents | Runner and adapters against scripted agents | A fake ACP agent and fake CLI binaries in `src/testing/` that replay recorded transcripts, including the bad cases: exit 0 with no output, soft-denied actions, a hang, a crash mid-stream, malformed JSON |
| Conformance | Every adapter passes one shared suite | Same assertions for all adapters: explicit tier flags, refusal of unsupported tiers and effort levels, cancel within the grace period, timeout enforcement, final message present |
| Durability | Jobs outlive callers; lost jobs are detected | Kill the caller; kill the runner; check the store |
| Live smoke | Real agents | Opt-in (`HUFU_LIVE=1`), run on a machine where the agents are installed and logged in. Never in default CI |

Recorded transcripts are tagged with the agent version they came from. When `hufu doctor` sees a
version newer than any recorded one, it says so.

## 13. Versioning

- The task spec, configuration and every JSON output carry `version` or `v`.
- The CLI follows semantic versioning from 1.0. Before that, breaking changes are listed in the
  changelog.
- Adapters record the range of agent versions they were tested against.
