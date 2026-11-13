# Technical Design

| | |
|---|---|
| **Status** | Describes Hufu 0.1 as implemented |
| **Last updated** | 2026-11-12 (rewritten after implementation: synchronous core, own protocol clients, accounts and pools, setup, daemon, three platforms) |
| **Related** | [Architecture](../architecture/architecture.md), [Adapter contract](adapter-contract.md), [Requirements](../requirements/requirements.md), [User guide](../guide/user-guide.md) |

This document turns the [architecture](../architecture/architecture.md) into concrete formats and
behaviours: the source layout, configuration, the task spec, the on-disk store, the event and
result shapes, the runner, the mission protocol, the MCP server and the test strategy. How
individual agents are driven is in the [adapter contract](adapter-contract.md). How to use the
result is in the [user guide](../guide/user-guide.md).

## 1. Stack

| Concern | Choice | Notes |
|---|---|---|
| Language | Rust, edition 2024, minimum version 1.89 | [ADR-0007](../adr/0007-implement-in-rust.md) |
| Concurrency | Threads and blocking I/O. No async runtime | [ADR-0009](../adr/0009-synchronous-core-and-own-protocol-clients.md) |
| CLI | `clap` (derive) | |
| Serialization | `serde`, `serde_json`; `serde-saphyr` for YAML | YAML 1.2 rules: only `true` and `false` are booleans, so a profile named `y` or `no` stays a string |
| JSON Schema | A validator written for Hufu, covering the subset that agents' structured output uses | `hufu-core/src/schema.rs` |
| ACP client | Written for Hufu on `serde_json`: JSON-RPC 2.0 over the agent's stdio, protocol version 1 | [ADR-0009](../adr/0009-synchronous-core-and-own-protocol-clients.md) |
| MCP server | Written for Hufu on `serde_json`: newline-delimited JSON-RPC over stdio | Same record |
| Process control | `std::process`, plus `libc` on Unix and `windows-sys` on Windows | All platform differences are in `hufu-contract/src/proc.rs` |
| Randomness | `getrandom`, for ids | |
| Tests | `cargo test`. End-to-end tests run the built binary against fake agents that live in the binary itself | Section 13 |
| Distribution | One `hufu` binary per platform; static on Linux | [Deployment](../deployment/deployment.md#3-install) |

That is the whole dependency list. The release profile optimizes for size, uses thin LTO and strips
symbols; a release binary is about 4 MB.

## 2. Source layout

All code, including its manifests and tests, lives under [`src/`](../../src/README.md), which is a
Cargo workspace. The repository root holds only documentation and repository metadata.

```text
src/
├── Cargo.toml  Cargo.lock  rustfmt.toml
├── crates/
│   ├── hufu-contract/      # what adapters and the core share; depends on nothing of Hufu's
│   │   └── src/            # types (Adapter trait, Profile, Capabilities, RunRequest, RunOutcome),
│   │                       # event, proc (processes on three platforms), time, json
│   ├── hufu-adapters/      # one module per adapter
│   │   └── src/            # codex_cli, agy_cli, cursor_cli, claude_cli, kimi_cli, pi, acp,
│   │                       # external, common
│   ├── hufu-core/          # everything agent-agnostic
│   │   └── src/
│   │       ├── config      # config files, merge, validation, editing
│   │       ├── spec        # task spec and the normalized job spec
│   │       ├── submit      # validation, profile and pool resolution, launching runners
│   │       ├── runner      # one process per job: slot, attempts, heartbeat, timeouts, result
│   │       ├── slots       # concurrency limits as files
│   │       ├── store       # job directories, atomic writes, lost-job detection
│   │       ├── workspace   # git worktrees, change detection
│   │       ├── tally       # acceptance checks
│   │       ├── schema      # JSON Schema validation
│   │       ├── capability  # computer-use routing, grants, pre-flight
│   │       ├── mission     # advisor loop, plan and verdict handling, budgets
│   │       ├── setup       # registering with host agents
│   │       ├── doctor      # health check and self-test
│   │       ├── update      # `hufu update`: release manifest, SHA-256, replacing the program
│   │       └── ops         # configuration edits and pruning shared by CLI and MCP
│   ├── hufu-mcp/           # MCP server
│   ├── hufu-daemon/        # optional daemon
│   ├── hufu-testing/       # fake agents and the conformance suite
│   └── hufu-cli/           # the `hufu` binary: commands, human and JSON output, exit codes
│       └── tests/e2e/      # end-to-end tests
├── install/                # install.sh, install.ps1
└── integrations/
    └── claude-code/        # skill and CLAUDE.md snippet
```

Dependency rule, enforced by the crate graph: `hufu-core` and `hufu-adapters` each depend on
`hufu-contract` and not on each other. `hufu-mcp` and `hufu-daemon` depend on `hufu-core`.
`hufu-cli` depends on all of them and wires the adapters in through the `AdapterRegistry` trait.

## 3. Configuration

Two files, merged key by key with the project file winning:

| File | Scope |
|---|---|
| `~/.hufu/config.yaml` (or `$HUFU_HOME/config.yaml`) | User-wide profiles and defaults |
| `<project>/.hufu/config.yaml` | Project overrides; safe to commit because it holds no secrets. Found by walking up from the workspace |

Under both lie the **presets**: one profile per supported agent (`codex`, `agy`, `cursor`,
`claude`, `kimi`, `pi`, `cursor-acp`, `kimi-acp`), so that an installed agent works with an empty
configuration. A profile of the same name in a file replaces the preset.

A full, test-checked sample is [`doc/examples/config.yaml`](../examples/config.yaml); the key
reference is in the [user guide](../guide/user-guide.md#11-configuration-reference). The top-level
keys are `version`, `defaults`, `limits`, `roles`, `profiles`, `pools`, `capabilities`, `missions`
and `retention`.

Profile fields:

| Field | Applies to | Meaning |
|---|---|---|
| `description` | all | One line shown in listings and given to a managed advisor as part of the roster |
| `adapter` | all | `codex-cli`, `agy-cli`, `cursor-cli`, `claude-cli`, `kimi-cli`, `pi`, `acp`, `external` |
| `command` | `acp`, `external`; optional elsewhere | Argv that starts the agent. For CLI adapters, an override of the program |
| `model` | all | Default model id, passed through verbatim |
| `provider` | `pi` | Provider name as pi knows it |
| `effort`, `effort_fallback` | all | Default effort; `error` (default) or `nearest`. See [adapter contract, section 4](adapter-contract.md#4-effort-mapping) |
| `permission`, `allow_full_access` | all | Default tier for this profile. `full-access` as a default needs `allow_full_access: true` |
| `account` | all | A label for the account, shown in listings and in results |
| `home` | adapters with a home variable | The agent's own configuration and login directory. The adapter turns it into `CODEX_HOME`, `CLAUDE_CONFIG_DIR`, `CURSOR_CONFIG_DIR`, `KIMI_CODE_HOME` or `PI_CODING_AGENT_DIR` |
| `env` | all | Names of environment variables the agent process may inherit |
| `env_from` | all | `{AGENT_VAR: SHELL_VAR}`: the agent sees the first name with the value of the second |
| `vars` | all | Literal variables. A key or value that looks like a secret is refused |
| `args` | all | Extra arguments appended verbatim. An escape hatch; adapters reject arguments that would change the permission tier |
| `provides` | all | `{computer_use: {via: native \| lcu \| <name>, check: [argv]}}`. Section 12 |
| `permission_requests`, `allow_execute`, `auth_method` | `acp` | `unverified` (default) or `verified`; command prefixes allowed at `workspace-write`; the `authenticate` method to call. See [adapter contract, section 3.1](adapter-contract.md#31-mapping) |

**Validation** reports every problem at once: unknown adapters, roles and pools naming profiles
that do not exist, an advisor that is a pool, a computer-use agent that does not declare the
capability, unparseable durations, secrets in `vars`, `full-access` defaults without the opt-in.
Unknown keys and YAML errors are refused when the file is read. A configuration with problems
still serves the read-only commands (`agents`, `doctor`, `config`, `status`), so that it can be
repaired with Hufu itself, but no job or mission starts from it.

**Editing.** `hufu config set`, `hufu profile ...`, `hufu pool ...` and the MCP configuration tools
all go through `hufu-core/src/ops.rs`: the file is read as a document, the change is applied, the
result merged with the other file is validated, and only then is it written, atomically. An edit
to the project file is validated together with the user-wide file it builds on. Comments in a
hand-written file are not preserved by an edit.

### 3.1 Accounts and pools

- A **profile** is the unit of "one agent, one model, one account". Several profiles may use the
  same adapter. Accounts are told apart by `home` (a separate login directory) or by `env` /
  `env_from` (a separately named API key). Hufu holds names and paths, never credentials.
- A **pool** has `members` (profiles) and a `strategy`. Wherever a task names an agent, it may name
  a pool. At submission Hufu orders the members by strategy: `failover` keeps the listed order,
  `round-robin` rotates the starting point (a counter in `run/rr-<pool>`), `least-busy` sorts by
  running jobs. The runner then tries them in that order and moves to the next member only when
  one fails with `agent_unavailable`, `auth_required` or `rate_limited`. The profile that actually
  ran is recorded in the state and the result.
- `roles.advisor` is one profile. `roles.executors` is a list of profiles and pools; its first
  entry is the default agent of `hufu run`.

## 4. Task spec

A task can be given as flags, as a file (`-f task.yaml`, YAML or JSON), or both, with flags
overriding the file. A test-checked sample is [`doc/examples/task.yaml`](../examples/task.yaml).

| Field | Required | Default | Meaning |
|---|---|---|---|
| `prompt` | yes | | The instruction given to the agent |
| `agent` | no | First of `roles.executors` | A profile or pool |
| `version` | no | `1` | Spec version |
| `title` | no | First line of the prompt | Shown in listings |
| `cwd` | no | Caller's directory | Workspace root for the agent |
| `permission` | no | Profile, then global default | `read-only`, `workspace-write`, `full-access` |
| `effort` | no | Profile, then global default | `off`, `low`, `medium`, `high`, `max`, `default`, `native:<value>` |
| `model` | no | Profile | Overrides the profile's model |
| `timeout` | no | Global default | Wall-clock limit per attempt, enforced by Hufu |
| `inactivity_timeout` | no | Global default | Limit on silence from the agent |
| `isolation` | no | Global default (`none`) | `worktree` runs the task on a new branch in a new git worktree |
| `output.schema` | no | | JSON Schema for the final answer: inline, or a path |
| `acceptance` | no | | Tally checks. See [section 10](#10-tally-checks) |
| `strict_permissions` | no | `false` | When `true`, any denied action fails the job |
| `depends_on` | no | | Job ids that must succeed, and not be rejected, first |
| `retry` | no | Global default | `{max_attempts, on: [reasons], backoff}` |
| `labels` | no | | Free-form key/value pairs |
| `grants` | no | none | Capabilities beyond the workspace, each with its scope. Section 12 |
| `computer_use` | no | | `{route, agent}`: a per-task override of the routing |

**Validation order.** The configuration as a whole, then the spec, then profile or pool
resolution, then capability checks against what the adapter declares (tier, effort, resume, where
the tier may come from), then computer-use routing and pre-flight, then workspace checks (the
directory exists; a git repository if `isolation: worktree`). Any failure refuses the submission
with exit code `2` and creates no job. `--dry-run` runs all of it and prints the launch plan.

The validated task is written once as `spec.json`: every default applied, the resolved profile
embedded, the order of pool members fixed. A runner needs nothing but that file and the
configuration's limits.

## 5. Command-line interface

The command reference is in the [user guide](../guide/user-guide.md); `hufu <command> --help` is
generated from the same definitions. What matters to callers that are agents or scripts:

- `hufu run` prints the job id and returns as soon as the job is recorded and its runner started.
  `--wait` blocks until a terminal state and prints the result.
- `hufu result` is compact by default: state, verdict, final message clipped to
  `defaults.result_chars`, changed files, denied actions, warnings. `--full` returns everything.
- `--json` makes every command print one document with a `v` field. An error is then
  `{"v": 1, "error": {"kind", "message"}}` with the usual exit code.
- Every error message says what was wrong, where, and what would be accepted.

### Exit codes

| Code | Meaning |
|---|---|
| `0` | Success. For `run --wait`, `wait` and `result`: the job succeeded and was not rejected |
| `1` | Internal error in Hufu |
| `2` | Usage, validation or configuration error. Nothing was run |
| `3` | Job failed |
| `4` | Job cancelled |
| `5` | Job timed out |
| `6` | `wait` timed out; the job is still running |
| `7` | Job lost |
| `8` | Job succeeded but was rejected by its tally check |
| `10` | Agent unavailable or not authenticated. From `up` and `init`: no supported agent is installed |

## 6. Job store

`$HUFU_HOME` defaults to `~/.hufu`.

```text
~/.hufu/
├── config.yaml
├── jobs/
│   └── j_7k2m9x4qpd/
│       ├── spec.json          # normalized task, written once
│       ├── state.json         # current state, rewritten atomically
│       ├── events.ndjson      # normalized events, append-only
│       ├── result.json        # written once, just before the terminal state
│       ├── tally.json         # acceptance check outcomes, if any
│       ├── children.ndjson    # child jobs, for computer-use hand-overs
│       ├── cancel.request     # present once cancellation was asked for
│       ├── runner.log         # the runner's own stderr
│       └── raw/
│           ├── stdout.log     # the agent's output, untouched
│           └── stderr.log
├── missions/
│   └── m_3f8h1c5tzn/
│       ├── mission.json       # goal, advisor, roster, budgets, tasks and their state
│       ├── plan.json          # the validated plan
│       ├── ledger.ndjson      # every dispatch, check outcome and verdict
│       ├── report.md
│       ├── driver.log
│       └── pause.request, abort.request
├── worktrees/                 # git worktrees of isolated jobs and missions
└── run/
    ├── slots/                 # one file per live runner
    ├── rr-<pool>              # round-robin counters
    └── daemon.json, daemon.log, daemon.stop
```

- **Ids** are short and time-sortable, prefixed `j_` for jobs and `m_` for missions.
- **Atomicity.** JSON files are written to a temporary sibling and renamed over the target.
  `events.ndjson` is appended one complete line at a time. Reads retry briefly, because on Windows
  a rename can collide with a reader for a moment.
- **`state.json`** holds the state, verdict, reason, the profile that is running, runner and agent
  process ids, times, heartbeat, attempt number, last activity and the resolved agent version.
- **Ordering.** `result.json` is written before `state.json` turns terminal, so a waiter that sees
  a terminal state always finds the result.
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
  "title": "Share one in-flight token refresh",
  "agent": {
    "profile": "codex-work", "adapter": "codex-cli", "account": "work", "version": "0.167.2",
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
  "tally": [{"check": "command", "description": "npm test -- auth", "passed": true, "duration_ms": 8412}],
  "usage": {"input_tokens": 48211, "output_tokens": 3907},
  "session": {"id": "…", "resumable": true},
  "timing": {"queued_at": "…", "started_at": "…", "finished_at": "…", "duration_s": 312},
  "attempts": 1
}
```

A failed job also has `reason` (one of the reason codes in the
[adapter contract](adapter-contract.md#21-failure-reasons)) and `detail`. A job that handed over
computer-use steps lists them under `children`; a mission's job carries `mission`.

`changes.files` comes from comparing the workspace before and after the job: the git state when
the workspace is a repository; otherwise sizes and modification times of up to 20,000 files; and
beyond that only the paths the agent reported in `edit`, `delete` and `move` events. Paths are
relative to the workspace.

## 8. Runner mechanics

1. **Detach.** `hufu run` starts `hufu __runner <job>` as a detached process and does not wait for
   it: its own session on Unix, its own hidden console and process group on Windows, with output
   going to `runner.log`. On Windows the caller's standard handles are made non-inheritable first,
   because otherwise the runner would keep the caller's output pipe open and whoever reads
   `hufu run`'s output would wait for the whole job.
2. **Dependencies.** The runner waits until every job in `depends_on` has succeeded; one that
   failed or was rejected ends this job with `dependency_failed`.
3. **Slot.** The runner takes a slot file under a short-lived lock, subject to the global limit,
   the per-profile limit and, for computer use, the display limit. Until there is room the job
   stays `queued`. Slot files of dead processes are removed by whoever reads them. With the daemon
   running, jobs are left for it to start, in submission order (section 14).
4. **Launch.** The runner resolves the workspace (creating the worktree if asked), takes a snapshot
   of it, builds the run request and calls the adapter. The agent's environment is a small fixed
   set (`PATH`, home, locale, temp directory, proxy and certificate variables, the Windows system
   variables) plus what the profile names; a variable named in `env_from` that is not set fails
   the attempt with `auth_required`, naming the variable.
5. **Pool members and retries.** A member that fails with `agent_unavailable`, `auth_required` or
   `rate_limited` is followed by the next member. A failure whose reason is in the retry policy is
   retried after the backoff, up to `max_attempts`.
6. **Heartbeat.** Every five seconds a watchdog thread rewrites `state.json` with a fresh
   timestamp.
7. **Timeouts.** The watchdog also holds a wall-clock timer per attempt and an inactivity timer.
   Either one firing cancels the agent and ends the job as `timed_out`.
8. **Cancel.** `hufu cancel` writes `cancel.request`; the watchdog sees it within a second. The
   adapter is asked to stop: ACP `session/cancel`, or `SIGTERM` to the agent's process group. After
   the grace period (ten seconds) the whole tree is killed: the process group and, found through
   the process table, descendants that left it. On Windows there is no polite signal for a hidden
   console process, so the tree is ended at once with `taskkill /T /F`. `--force` kills from the
   outside without waiting.
9. **Finish.** The runner computes changes, enforces the tier (`tier_violated` for a read-only job
   that changed files), validates structured output, runs the tally checks, writes `result.json`,
   then the terminal state, and releases its slot.
10. **Lost jobs.** Any command that reads a non-terminal job whose heartbeat is older than thirty
    seconds and whose runner process is gone rewrites it as `lost` with reason `runner_died`. A
    zombie process counts as gone. The agent of a lost job is not killed automatically: its
    process id may have been reused, and a wrong kill is worse than a leftover.
11. **Bugs.** A panic in the runner is caught and ends the job as `failed` with `internal_error`,
    so that a bug in Hufu does not look like a crash of the machine.

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
graph has no cycle, every agent is on the roster (a profile or a pool), no task exceeds the
mission's maximum tier, every acceptance command starts with an allowed prefix, and every grant is
inside `allowed_grants`. An invalid plan, or an answer that is not JSON, is sent back once with the
errors; a second one fails the mission with `plan_invalid`.

Optional fields may be left out or given as `null`. Agents with native structured output are given
a schema in which every property is required but nullable, because that is the only form some of
them accept; Hufu fills in absent nullable properties before validating.

Tasks whose dependencies are met run side by side up to `max_parallel`. A goal the advisor answers
with an empty plan becomes a single task for the first executor.

### 9.3 Getting JSON out of an agent

In order of preference: the agent's own structured-output feature (Codex `--output-schema`,
Antigravity and Claude Code `--json-schema`); otherwise an instruction appended to the prompt to
answer with a single fenced JSON block, which Hufu extracts and validates. Either way the answer is
validated by Hufu's own JSON Schema validator, and a mismatch fails the job with `invalid_output`.

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

Whether the advisor reviews every result is configurable per mission: `review: always`,
`on_failure` (the default: only results whose checks failed or whose job failed) or `never`.
A task whose automatic checks fail is not accepted because the advisor said `accept`.

The review is a follow-up in the advisor's planning session where the agent can resume, so the
advisor still knows its own plan; the follow-up carries the review schema instead of the plan
schema.

### 9.5 Budgets

`max_rounds_per_task`, `max_tasks`, `max_wall_clock`, and `max_tokens` or `max_cost` where usage is
reported. The first one exhausted stops the mission. Finished work is kept.

### 9.6 The driver

A mission is driven by its own detached process, `hufu __mission <id>`, which holds no state that
is not also in `mission.json` and `ledger.ndjson`. That is what makes `pause`, `resume` and
recovery after a restart the same operation: a new driver reads the files and continues, and
finished tasks are not repeated. `pause` lets running jobs finish; `abort` cancels them. With
`isolation: worktree` the whole mission works in one worktree on branch `hufu/<mission>`. When the
mission ends, the driver writes `report.md`.

## 10. Tally checks

Checks run in the job's workspace after the agent finishes, in the order listed. All must pass for
the verdict to be `accepted`.

| Check | Passes when |
|---|---|
| `command: <shell>` | The command exits `0` within its timeout |
| `paths: {allow: [...], deny: [...]}` | Every changed file matches an `allow` glob and none matches a `deny` glob |
| `schema: <file>` | The structured output validates against the schema |
| `no_denied_actions: true` | The agent reported no skipped actions. `strict_permissions: true` goes further and fails the job itself |
| `review: advisor` | The mission's advisor returns `accept`. Only valid inside a mission |

Check commands run with the caller's privileges, not the agent's tier, so they are part of the
trusted task spec. In a mission, checks written by the advisor are limited to commands that start
with a prefix from `missions.acceptance_commands`. A command has a ten-minute limit unless the
check sets `timeout`. Outcomes are stored in `tally.json` and in the result.

Independently of any check, a `read-only` job that changed files fails with `tier_violated`.

## 11. MCP server

`hufu mcp` is a stdio MCP server. It is not a client of the daemon: it calls the same functions as
the CLI, so it works with or without one.

| Group | Tools |
|---|---|
| Dispatch | `hufu_agents`, `hufu_submit`, `hufu_status`, `hufu_wait`, `hufu_result`, `hufu_logs`, `hufu_cancel`, `hufu_resume` |
| Missions | `hufu_mission_start`, `hufu_mission_status`, `hufu_mission_pause`, `hufu_mission_resume`, `hufu_mission_abort` |
| Configuration | `hufu_config_get`, `hufu_config_set`, `hufu_profile_set`, `hufu_profile_remove`, `hufu_pool_set`, `hufu_pool_remove`, `hufu_init`, `hufu_setup` |
| Health | `hufu_doctor` |

- **No tool call lasts as long as a job.** `hufu_submit` returns the id at once. `hufu_wait` blocks
  for a bounded time (default 50 s, at most 600 s) and then answers `wait_timed_out: true`, which
  its description tells the caller to read as "still running".
- **Results are structured.** A tool answers with text and, for objects, the same value as
  `structuredContent`. Failures are tool results with `isError: true` and Hufu's error kind, so
  the calling model can read and react to them; protocol errors are reserved for the protocol.
- **Configuration tools** make the same validated edits as the CLI. `hufu_setup` without a list of
  agents registers with every agent found, exactly like `hufu setup`.
- **Protocol.** Newline-delimited JSON-RPC 2.0. Handshake revisions `2025-11-25`, `2025-06-18`,
  `2025-03-26` and `2024-11-05`; the handshake-free revision `2026-07-28`, where each request
  names its revision and results carry `resultType: "complete"`; `server/discover`; batches.
  Errors: `-32700` parse, `-32601` unknown method, `-32602` unknown tool or bad arguments, `-32022`
  unsupported revision.
- **Child mode.** `hufu mcp --child-of <job>` serves exactly one tool, `computer_use`, to the
  executor of that job (section 12).
- A panic inside one tool is answered as a tool error; the server and the host's session go on.

## 12. Capability routing: computer use

Some work needs more than code tools. **Computer use** means operating a desktop application
through its interface. It can be served by a separately configured agent or by the job's own agent
([requirements, section 7.9](../requirements/requirements.md#79-capability-routing),
[ADR-0008](../adr/0008-computer-use-is-a-routable-capability.md)). The design borrows from LCU
([study](../research/computer-use.md)).

### 12.1 Configuration

```yaml
profiles:
  claude:
    adapter: claude-cli
    provides:
      computer_use: {via: native}   # this executor can operate the desktop itself
  desk:
    adapter: codex-cli
    provides:
      computer_use: {via: lcu}      # a profile kept for desktop operation, through LCU
  codex:
    adapter: codex-cli              # provides no computer use

capabilities:
  computer_use:
    route: dedicated                # dedicated | executor
    agent: desk                     # any profile that declares the capability
    when_unavailable: error         # error | executor
    max_concurrent: 1               # one operator per display
```

| `route` | Meaning |
|---|---|
| `executor` | The job's own agent operates the desktop. Valid only for profiles that declare `provides.computer_use`; otherwise the task is refused |
| `dedicated` | Computer-use steps are carried out by the profile named in `agent`, whichever agent is executing the task. That profile can be one of the executors or a profile used for nothing else; either way it must declare the capability |

A task may override the route and the agent (`computer_use: {route, agent}`, or
`--computer-use-route`, `--computer-use-agent`). When the dedicated agent is the job's own profile,
the route collapses to `executor`. Hufu does not install or configure the runtime; `via` only
tells Hufu how to check readiness.

### 12.2 Granting

```yaml
grants:
  - capability: computer_use
    apps: ["Figma", "Notes"]        # the app scope
```

- Without the grant, no route is set up and the executor is not offered the tool, whatever its
  permission tier.
- The app scope is required and has no wildcards. Terminal applications and agent host
  applications are always refused. Browsers and password managers are allowed only by name and
  come with a warning in the result.
- In a mission, the advisor may request a grant only if `allowed_grants` includes the capability,
  and only for applications listed there.
- A grant does not carry over to a follow-up job.

### 12.3 Pre-flight

Before launch, without touching the screen:

1. The serving profile declares the capability. For `via: lcu` on a platform LCU does not support,
   the task is refused.
2. Where the runtime can list what it already allows (`lcu apps` for LCU, or the profile's
   `check` command), every application in the scope must be in that list. If one is missing, the
   task is refused with the command a person must run. Hufu never runs it, and never answers a
   runtime's approval prompt. Where the runtime cannot be asked, the job carries a warning instead.
3. The display slot is free, or the job queues for it.

`hufu doctor` reports the same per profile.

### 12.4 How a dedicated route works

1. For a task with the grant, the runner attaches a small MCP server to the executor's session:
   `hufu mcp --child-of <job>`. Over ACP this uses the `mcpServers` parameter of `session/new`;
   Codex and Claude Code take it through their own MCP configuration flags. An executor whose
   adapter cannot attach an MCP server cannot use the dedicated route, and the task is refused at
   validation.
2. The server offers one tool, `computer_use`, taking an instruction.
3. A call creates a **child job** on the dedicated profile, linked to the parent, carrying the
   parent's app scope, and waits for it within the parent's remaining time.
4. The child's final message is returned as the tool result. Its events and raw output stay in the
   child job's directory.
5. Child jobs do not take a slot of the global limit, since they are part of their parent, but the
   child holds the display slot while it runs. Cancelling the parent cancels its children.

### 12.5 Safety

- Computer use is scoped to one job and to the listed applications.
- Computer use acts outside the workspace. Worktree isolation and permission tiers do not contain
  it, and a grant should be read as wider than `full-access`.
- The recommended setup is a separate machine, VM or user account for the operating agent.

## 13. Testing

| Layer | What | How |
|---|---|---|
| Unit | Parsers for each agent's output, tier and effort mapping, configuration validation and merging, schema validation, routing, time and process helpers | `cargo test`, in each crate |
| Fake agents | One fake per adapter flavour, compiled into the `hufu` binary and started as `hufu __fake-agent <flavor>` | They print what the real agent's documentation says it prints, and take directives in the prompt (`[[fake:slow=1500]]`, `[[fake:deny]]`, `[[fake:hang]]`, `[[fake:auth]]`, `[[fake:malformed]]`, ...) to produce the bad cases: exit 0 with no output, soft-denied actions, a hang, a crash mid-stream, malformed JSON, a login error, a rate limit |
| Conformance | Every adapter passes one shared suite, C1 to C12 | Explicit tier flags, refusal of unsupported tiers and effort levels, cancel within the grace period, a final message or a failure, never an empty success |
| End to end | The built binary, in a throwaway home, against the fakes | `src/crates/hufu-cli/tests/e2e/`: jobs, missions, MCP (with a client of its own), computer use, the daemon, configuration and setup. Includes durability: kill the caller, kill the runner, kill the daemon, and check the store |
| Documentation | The files under `doc/examples/` | Loaded and used by an end-to-end test |
| Self-test | `hufu doctor --self-test` | A user-facing subset of the same idea: three real jobs against the fakes |
| Platforms | Linux, macOS, Windows | The whole suite runs on all three in CI |
| Live | Real agents | Not automated: it needs logins. Open item in the [roadmap](../roadmap.md#next) |

Because the fakes live in the shipped binary, the end-to-end tests exercise the same executable a
user installs, and need nothing else on the machine but `git`.

## 14. Daemon

The daemon is optional and stateless: everything it knows is in the job store.

- **Presence.** `run/daemon.json` holds its pid and a heartbeat refreshed every few seconds. A
  daemon counts as running only if the process is alive **and** the heartbeat is fresh, which also
  covers a daemon that died without cleaning up.
- **Queue.** While a daemon is running, `hufu run` records the job as queued for the daemon instead
  of starting a runner. The daemon starts runners oldest first while there is room. If the daemon
  disappears, any `status` or `wait` on such a job starts its runner directly.
- **Lost jobs.** A job that became `lost` with `runner_died` after the daemon started, and whose
  retry policy has attempts left, is put back in the queue.
- **Pruning** runs once an hour according to `retention`.
- **Stopping** uses a marker file, so it works the same on every platform; running jobs are not
  touched.

## 15. Setup and health

- **`hufu init`** looks for each preset's program on `PATH`, writes a commented `config.yaml`
  naming what it found, and picks default roles: the advisor is the first agent found, looking in
  the order `codex`, `claude`, `agy`, `cursor`, `kimi`, `pi`, and the executors are all the agents
  found. With no agent found it writes nothing (`action: skipped`, exit code `10`), so that the
  next run, after an agent is installed, writes a complete file; an existing file is always kept,
  and a file naming no agent would otherwise need `--force` to be refreshed. `--force` writes the
  file regardless. With one agent found the report carries a hint that it holds both roles.
- **`hufu setup`** registers the MCP server with host agents (table in the
  [deployment guide](../deployment/deployment.md#5-connect-host-agents)). JSON and TOML files of
  other tools are edited in place under a marker or a single key, with a one-time backup; for
  Claude Code the host's own `claude mcp add` is used, because its settings file is not a stable
  interface.
- **`hufu up`** is `init`, `setup`, `daemon start` and `doctor` in one, and safe to repeat. It
  never asks a question, because agents run it. It probes first: when no profile's agent is
  installed it stops before `setup` and `daemon start`, prints each supported agent's install and
  login command, and exits with `10`. Otherwise it reports `ready` only when the configuration is
  valid and at least one installed agent is not known to be logged out. When no host is found it
  returns the generic `mcpServers` entry as a `manual` action instead of registering nothing
  silently.
- **`hufu doctor`** probes each profile (installed, version, login where it can be seen without
  running a model), lists configuration problems, and prints next steps. Among them: no agent
  installed (with the command that applies, `hufu up` or `hufu init --force`), a configuration
  file that names no roles although an agent is installed, and a single profile that is both the
  advisor and the only executor. `--self-test` runs three
  jobs against the fakes in a temporary home: a background job, an ACP job with a permission
  decision, and a cancel that must leave no process behind.

- **`hufu update`** ([ADR-0013](../adr/0013-self-update-with-system-tools.md)) reads
  `hufu-manifest.json` of the newest or the named release, downloads the archive for the platform
  with the system's `curl` or `wget`, checks its SHA-256 sum with code of its own, unpacks it with
  the system's `tar`, and starts the new program once to see that it runs and reports the expected
  version. Only then is the program replaced, by a rename; on Windows the running program is
  renamed aside first and removed later by a short-lived helper. The daemon is restarted if one
  was running, and `doctor` is run by the new program. With no release to download it builds with
  `cargo install` into a scratch directory and installs the result the same way. A process that
  still runs the old program starts its runners from the new one.

## 16. Versioning

- The configuration, the task spec and every JSON output carry `version` or `v`, currently `1`.
- The CLI follows semantic versioning from 1.0. Before that, breaking changes are listed in the
  release notes. `hufu update` orders versions as Semantic Versioning does, so `0.1.0-alpha.2`
  comes before `0.1.0-rc.1`, and both come before `0.1.0`.
- The agent version an adapter saw is recorded in each job's state and result, so a report about a
  misbehaving agent always says which version it was.
