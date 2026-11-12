# Hufu User Guide

| | |
|---|---|
| **Status** | Describes Hufu 0.1 as implemented. The examples in it are checked by the test suite where they are files under `doc/examples/` |
| **Last updated** | 2026-11-12 |
| **Related** | [Deployment](../deployment/deployment.md), [Technical design](../technical/technical-design.md), [Adapter contract](../technical/adapter-contract.md) |

Everything you can do with Hufu, from the first command to missions and computer use.
For why it works this way, see the [architecture](../architecture/architecture.md); for how it is
built, the [technical design](../technical/technical-design.md).

- [1. Install and set up](#1-install-and-set-up)
- [2. The ideas in one minute](#2-the-ideas-in-one-minute)
- [3. Jobs](#3-jobs)
- [4. Agents, accounts and pools](#4-agents-accounts-and-pools)
- [5. Permissions and effort](#5-permissions-and-effort)
- [6. Acceptance checks](#6-acceptance-checks)
- [7. Missions](#7-missions)
- [8. Computer use](#8-computer-use)
- [9. Using Hufu from another agent (MCP)](#9-using-hufu-from-another-agent-mcp)
- [10. The daemon](#10-the-daemon)
- [11. Configuration reference](#11-configuration-reference)
- [12. Task files](#12-task-files)
- [13. Exit codes and JSON output](#13-exit-codes-and-json-output)
- [14. Where things are stored](#14-where-things-are-stored)
- [15. Troubleshooting](#15-troubleshooting)
- [16. Known limits](#16-known-limits)

## 1. Install and set up

```bash
# macOS and Linux
curl -fsSL https://raw.githubusercontent.com/kverona-ai/hufu/main/src/install/install.sh | sh
```

```powershell
# Windows
irm https://raw.githubusercontent.com/kverona-ai/hufu/main/src/install/install.ps1 | iex
```

Then one command sets everything up:

```console
$ hufu up
```

`hufu up` does four things, and each of them is also a command of its own:

| Step | Command | What it does |
|---|---|---|
| 1 | `hufu init` | Looks for agents on your `PATH` and writes `~/.hufu/config.yaml` naming them, with a default advisor and default executors. An existing file is kept |
| 2 | `hufu setup` | Registers Hufu's MCP server with the agents it found, so that each of them can dispatch through Hufu. Skip with `--no-setup` |
| 3 | `hufu daemon start` | Starts the optional daemon. Skip with `--no-daemon` |
| 4 | `hufu doctor` | Checks installation, logins and configuration, and lists what to do next |

Run `hufu up` again at any time: it changes nothing that is already in place.

`hufu doctor --self-test` goes one step further and runs real jobs against agents that are built
into the binary. It needs no account and no network, and it proves that background jobs, permission
enforcement and cancellation work on your machine.

Other ways to install, and how to uninstall, are in the [deployment guide](../deployment/deployment.md).

## 2. The ideas in one minute

- A **profile** is one agent configured one way: an adapter, perhaps a model, an effort level, an
  account. Presets exist for every supported agent (`codex`, `agy`, `cursor`, `claude`, `kimi`,
  `pi`, `cursor-acp`, `kimi-acp`), so an installed agent works without writing anything.
- A **pool** is a set of profiles that can stand in for each other, typically several accounts.
- A **task** is what you ask for. Once submitted it is a **job**, which runs in the background and
  is recorded on disk.
- **Roles** say who does what by default: one `advisor` and any number of `executors`.
- A **mission** is a goal the advisor breaks into tasks, hands to executors and reviews.
- A **tally check** decides whether finished work is accepted.

## 3. Jobs

```console
$ hufu run -a codex "Explain what src/auth/refresh.ts does"
j_7k2m9x4qpd
```

`hufu run` returns at once with a job id. The job keeps running if you close the terminal.

| Command | What it does |
|---|---|
| `hufu run [options] "<prompt>"` | Start a job. `-` reads the prompt from stdin, `--prompt-file` from a file, `-f` takes a [task file](#12-task-files) |
| `hufu status [job]` | One job, or the active jobs plus the most recent ones. `--all`, `--state <s>`, `--mission <id>`, `--limit <n>` |
| `hufu wait <job>...` | Block until the jobs have ended. `--timeout 10m` gives up waiting (the jobs go on), `--any` returns on the first one |
| `hufu result <job>` | The final message, changed files, denied actions, check outcomes, usage. `--full` does not clip the message |
| `hufu logs <job>` | Normalized events. `-f` follows, `--raw` prints the agent's own output, `--raw --stderr` its stderr |
| `hufu cancel <job>...` | Stop the agent and everything it started. `--force` skips the grace period |
| `hufu resume <job> "<prompt>"` | A follow-up in the same agent session, as a new job. It keeps the workspace, the tier and the checks of the first job |
| `hufu prune` | Remove old finished jobs. `--older-than 7d`, `--keep 100`, `--worktrees`, `--dry-run` |

The most used options of `hufu run`:

| Option | Meaning |
|---|---|
| `-a, --agent <name>` | A profile or a pool. Default: the first of `roles.executors` |
| `-p, --permission <tier>` | `read-only` (default), `workspace-write`, `full-access` |
| `-e, --effort <level>` | `default`, `off`, `low`, `medium`, `high`, `max`, or `native:<value>` |
| `-m, --model <id>` | A model id, passed to the agent as it is |
| `-C, --cwd <dir>` | The workspace. Default: the current directory |
| `-t, --timeout <d>`, `--inactivity-timeout <d>` | Limits such as `30m`, `2h`. Defaults: 30 minutes, and 10 minutes of silence |
| `--isolation worktree` | Run in a git worktree of its own, on branch `hufu/<job>` |
| `--accept "<command>"`, `--allow-path <glob>`, `--deny-path <glob>`, `--strict` | [Acceptance checks](#6-acceptance-checks) |
| `--schema <file>` | A JSON Schema the final answer must match; the parsed answer is in the result |
| `--depends-on <job>` | Start only after that job has succeeded |
| `--retry <n>` | Total attempts for retryable failures (rate limits by default) |
| `--computer-use <apps>` | Grant desktop control of these applications, see [computer use](#8-computer-use) |
| `-w, --wait`, `--follow`, `--wait-timeout <d>` | Wait for the result, print events meanwhile |
| `--dry-run` | Validate and print the exact command line that would run. Nothing is started |

`--dry-run` is the quickest way to see what Hufu will really do with an agent:

```console
$ hufu run -a codex -p workspace-write -e high --dry-run "do it"
would run profile `codex` (adapter codex-cli) at workspace-write in /work/app

codex exec --cd /work/app --sandbox workspace-write -c model_reasoning_effort="high" --json ...
(the prompt is sent on stdin)
```

A job is in one of these **states**: `queued`, `starting`, `running`, then one of `succeeded`,
`failed`, `cancelled`, `timed_out`, `lost`. A finished job also has a **verdict**: `unchecked`,
`accepted` or `rejected`. A failed job carries a **reason** such as `auth_required`,
`agent_unavailable`, `rate_limited`, `agent_error`, `empty_output`, `invalid_output`,
`tier_violated`, `dependency_failed` or `runner_died`.

## 4. Agents, accounts and pools

```console
$ hufu agents            # profiles and pools, their tiers and effort levels, and the roles
$ hufu agents --probe    # also: is it installed, is it logged in
```

### Adding an agent

```console
$ hufu profile add codex-high --adapter codex-cli --model gpt-6.1-sol --effort high
$ hufu profile add or-reasoner --adapter pi --provider openrouter --model vendor/model-slug \
    --env OPENROUTER_API_KEY
$ hufu profile add my-agent --adapter acp --command my-agent,acp    # any ACP agent
```

`hufu profile set <name> ...` changes some fields, `hufu profile show <name>` prints one profile
with its capabilities, `hufu profile remove <name>` takes it out. Add `--project` to write to
`.hufu/config.yaml` of the current project instead of your user-wide file.

Hufu stores the **names** of environment variables, never their values. A setting that looks like
a secret is refused.

### Several accounts of one agent

A second account is a second profile. There are two ways to keep the accounts apart:

```console
# An agent that logs in interactively: give the account its own login directory.
$ hufu profile add codex-work --adapter codex-cli --account work --home ~/.codex-work
$ CODEX_HOME=~/.codex-work codex login          # log that account in once

# An agent that takes an API key: read the key from a differently named variable.
$ hufu profile add or-team --adapter pi --provider openrouter --model vendor/model-slug \
    --account team --env-from OPENROUTER_API_KEY=OPENROUTER_API_KEY_TEAM
```

`--home` becomes the agent's own variable for its configuration directory:

| Adapter | Variable set from `home` |
|---|---|
| `codex-cli` | `CODEX_HOME` |
| `claude-cli` | `CLAUDE_CONFIG_DIR` |
| `cursor-cli` | `CURSOR_CONFIG_DIR` |
| `kimi-cli` | `KIMI_CODE_HOME` |
| `pi` | `PI_CODING_AGENT_DIR` |
| `agy-cli`, `acp`, `external` | none; use `env_from` or `vars` with the variable that agent documents |

### Pools

```console
$ hufu pool add codex-any --member codex,codex-work --strategy failover
$ hufu run -a codex-any "..."
```

| Strategy | Which member runs |
|---|---|
| `failover` (default) | The first member; the next one when it is not logged in, not installed or rate limited |
| `round-robin` | Members in turn; the same fallback applies |
| `least-busy` | The member with the fewest running jobs |

Failover happens only for those three reasons. A job that failed because the agent got the task
wrong is not repeated on another account.

### Roles

```console
$ hufu config set roles.advisor claude
$ hufu config set roles.executors "[codex-any, agy, kimi-acp]"
```

There is one advisor, and it is a profile, not a pool. Executors can be any number of profiles and
pools. Both are only defaults: `hufu run -a` and `hufu mission start --advisor/--executors`
override them.

## 5. Permissions and effort

Every job runs at one of three tiers, and Hufu passes the matching flags to the agent explicitly.

| Tier | The agent may |
|---|---|
| `read-only` | Read the workspace. Nothing else |
| `workspace-write` | Also change files inside the workspace |
| `full-access` | Do anything your user account can do |

Rules that hold for every agent:

- The default is `read-only`.
- A tier the agent cannot enforce is **refused** before anything starts. Hufu never widens or
  narrows a tier silently. `hufu agents` shows what each profile supports.
- `full-access` has to be asked for on the task (`-p full-access`). It can be a profile's default
  only together with `allow_full_access: true`, and never the global default.
- After a `read-only` job Hufu compares the workspace with how it was before. If files changed, the
  job fails with `tier_violated`, whatever the agent said.
- Actions the agent skipped for lack of permission are listed in the result as denied actions.
  `--strict` turns any of them into a failure.

ACP agents ask Hufu before each action, and Hufu answers by tier. Because that only protects you if
the agent really asks, an ACP profile supports `full-access` only until you mark it as checked:
`hufu profile set <name> --verified`. At `workspace-write`, an ACP agent may run only the commands
listed under the profile's `allow_execute`.

**Effort** is one scale for every agent: `off`, `low`, `medium`, `high`, `max`, plus `default`
(pass nothing) and `native:<value>` (pass the agent's own value through). A level an agent has no
equivalent for is an error. Set `effort_fallback: nearest` on a profile to use the closest level
instead.

## 6. Acceptance checks

Exit code `0` from an agent is not proof of anything. A task can carry checks, and the job is
`accepted` only when all of them pass. A `rejected` job exits with code 8.

| Check | Flag | In a task file |
|---|---|---|
| A command exits 0, run in the job's workspace | `--accept "cargo test"` | `- command: cargo test` (optional `timeout: 5m`) |
| Only these paths changed | `--allow-path "src/**"` | `- paths: {allow: ["src/**"]}` |
| These paths did not change | `--deny-path "**/*.lock"` | `- paths: {deny: ["**/*.lock"]}` |
| The answer matches a JSON Schema | `--schema out.json` | `output: {schema: {...}}` |
| Nothing was denied | `--strict` | `- no_denied_actions: true` |

## 7. Missions

A mission is the managed loop: the advisor plans, executors do the work, the advisor reviews.

```console
$ hufu mission start "Migrate the HTTP client to async/await; keep the tests green"
m_4h8s2k9d

$ hufu mission status m_4h8s2k9d
$ hufu mission wait m_4h8s2k9d
$ hufu mission report m_4h8s2k9d
```

| Command | What it does |
|---|---|
| `hufu mission start "<goal>"` | Start. `--advisor`, `--executors a,b`, `-C`, `--isolation worktree`, `--review always\|on_failure\|never`, `--max-tier`, `--max-tasks`, `--allow-command "<prefix>"`, `-f mission.yaml`, `-w` |
| `hufu mission status [id]`, `hufu mission list` | State and tasks |
| `hufu mission report <id>` | What was done, task by task |
| `hufu mission pause <id>` | Let running jobs finish, start no new ones |
| `hufu mission resume <id>` | Continue a paused mission, or one interrupted by a restart. Finished tasks are not repeated |
| `hufu mission abort <id>` | Stop now and cancel running jobs. Finished work is kept |

What keeps a mission in bounds:

- **The roster.** The advisor can only dispatch to the executors you named.
- **`max_tier`.** No task may ask for more than this tier (default `workspace-write`).
- **Acceptance commands.** A check the advisor writes may only run commands that start with one of
  `missions.acceptance_commands` (or `--allow-command`).
- **Grants.** The advisor can request computer use for a task only for the applications listed in
  `missions.allowed_grants`.
- **Budgets.** `max_tasks`, `max_rounds_per_task`, `max_wall_clock`, and `max_tokens` / `max_cost`
  where agents report usage.
- **Checks outrank the advisor.** A task whose checks fail is not accepted because the advisor
  liked it.

A mission file holds the same settings; see [`doc/examples/mission.yaml`](../examples/mission.yaml).

## 8. Computer use

A task can be granted control of named desktop applications. Nothing touches the desktop without a
grant, and the grant lists exactly which applications.

```console
$ hufu run -a codex -p workspace-write --computer-use "Figma,Notes" \
    "Export the 'Login' frame from Figma as PNG into assets/"
```

Who operates the desktop is a matter of configuration:

1. **Say which profiles can do it.** Any profile whose agent supports computer use, an executor you
   already use or a profile kept only for this:

   ```console
   $ hufu profile set claude --computer-use native    # the agent's own computer use
   $ hufu profile add desk --adapter codex-cli --computer-use lcu   # through the LCU runtime
   ```

2. **Choose the route.**

   ```console
   $ hufu config set capabilities.computer_use.route dedicated
   $ hufu config set capabilities.computer_use.agent desk
   ```

   | Route | Who operates the desktop |
   |---|---|
   | `dedicated` | The profile in `capabilities.computer_use.agent`. The job's own agent hands over one step at a time and gets back what was done and seen |
   | `executor` | The job's own agent, which must itself declare computer use |

   A profile that does not declare computer use is refused in either position.
   `when_unavailable: executor` falls back to the job's own agent when the dedicated one cannot be
   used; the default is to fail.

3. **Override per task** with `--computer-use-agent <profile>` or `--computer-use-route <route>`.

Only one job at a time holds the display (`capabilities.computer_use.max_concurrent`, default 1);
others wait in the queue. For `via: lcu`, Hufu reads which applications LCU already allows and
refuses a grant that goes beyond them, telling you what to allow first. Hufu never widens what the
desktop runtime permits; a person does that.

## 9. Using Hufu from another agent (MCP)

```console
$ hufu setup                     # every agent found on this machine
$ hufu setup --agent codex       # one of: claude, codex, cursor, kimi, agy, generic
$ hufu setup --agent claude --project    # only for the current project
$ hufu setup --print             # show what would be written, change nothing
$ hufu setup --remove            # take the registration out again
```

| Host | What `hufu setup` changes |
|---|---|
| Claude Code | Runs `claude mcp add --scope user hufu`, and installs a `hufu` skill. With `--project`: `.mcp.json` and `.claude/skills/hufu` in the project |
| Codex | A marked block in `~/.codex/config.toml` |
| Cursor | `~/.cursor/mcp.json` |
| Kimi Code | `~/.kimi-code/mcp.json` |
| Antigravity | `~/.gemini/config/mcp_config.json` |
| Any other | `--agent generic --print` prints the snippet: command `hufu`, argument `mcp` |

Existing files are edited in place, other entries are left alone, and a backup of the original is
kept next to it.

The connected agent gets these tools:

| Group | Tools |
|---|---|
| Dispatch | `hufu_agents`, `hufu_submit`, `hufu_status`, `hufu_wait`, `hufu_result`, `hufu_logs`, `hufu_cancel`, `hufu_resume` |
| Missions | `hufu_mission_start`, `hufu_mission_status`, `hufu_mission_pause`, `hufu_mission_resume`, `hufu_mission_abort` |
| Configuration | `hufu_config_get`, `hufu_config_set`, `hufu_profile_set`, `hufu_profile_remove`, `hufu_pool_set`, `hufu_pool_remove`, `hufu_init`, `hufu_setup` |
| Health | `hufu_doctor` |

So configuration can be a conversation: "add my work Codex account under `~/.codex-work`, pool it
with the personal one, and make that pool the first executor". Every change is validated before it
is written; an invalid one is refused and nothing changes.

`hufu mcp` speaks MCP over stdio and accepts the protocol revisions `2025-11-25`, `2025-06-18`,
`2025-03-26` and `2024-11-05`, as well as the handshake-free revision `2026-07-28`.

## 10. The daemon

Hufu works without a daemon: every job has its own background runner, and the limits on concurrent
jobs hold either way. The daemon adds three things:

- queued jobs start strictly in the order they were submitted,
- a job whose runner died is started again when its retry policy has attempts left,
- old jobs are pruned once an hour according to `retention`.

```console
$ hufu daemon start | stop | status
$ hufu daemon run          # in the foreground, for a service manager
```

Stopping the daemon does not touch running jobs. If the daemon dies, jobs left in its queue start
themselves the next time anyone asks for their status.

## 11. Configuration reference

Hufu reads `~/.hufu/config.yaml` and then `.hufu/config.yaml` of the project you are in, which
overrides it key by key. `hufu config path` prints both, `hufu config get [key]` the merged result,
`hufu config validate` every problem at once.

```console
$ hufu config set limits.max_concurrent_jobs 6
$ hufu config set defaults.timeout 45m --project
$ hufu config unset limits.per_profile.codex
```

Values are read as YAML: `4`, `true`, `[a, b]`, `{k: v}`.

| Key | Default | Meaning |
|---|---|---|
| `defaults.permission` | `read-only` | `read-only` or `workspace-write` |
| `defaults.effort` | `default` | |
| `defaults.timeout` | `30m` | Per attempt |
| `defaults.inactivity_timeout` | `10m` | Stop a job whose agent has been silent this long |
| `defaults.isolation` | `none` | Or `worktree` |
| `defaults.result_chars` | `4000` | How much of the final message `hufu result` prints without `--full` |
| `defaults.retry` | `{max_attempts: 1, on: [rate_limited], backoff: 15s}` | |
| `limits.max_concurrent_jobs` | `4` | Across all agents |
| `limits.per_profile.<name>` | none | Jobs of one profile at a time |
| `roles.advisor` | none | A profile |
| `roles.executors` | none | Profiles and pools; the first is the default for `hufu run` |
| `profiles.<name>` | presets | See below |
| `pools.<name>` | none | `members`, `strategy`, `description` |
| `capabilities.computer_use` | `{route: dedicated, when_unavailable: error, max_concurrent: 1}` | Plus `agent` |
| `missions.review` | `on_failure` | `always`, `on_failure`, `never` |
| `missions.max_tier` | `workspace-write` | |
| `missions.max_parallel` | `1` | Independent tasks side by side |
| `missions.acceptance_commands` | none | Command prefixes for advisor-written checks |
| `missions.allowed_grants` | none | For example `[{capability: computer_use, apps: [Figma]}]` |
| `missions.budgets` | `{max_tasks: 12, max_rounds_per_task: 3, max_wall_clock: 3h}` | Plus `max_tokens`, `max_cost` |
| `retention` | `{max_age: 30d, max_jobs: 2000}` | What `hufu prune` and the daemon keep |

A profile:

| Field | Meaning |
|---|---|
| `adapter` | `codex-cli`, `agy-cli`, `cursor-cli`, `claude-cli`, `kimi-cli`, `pi`, `acp`, `external` |
| `command` | The argv that starts the agent. Needed for `acp` and `external`; optional elsewhere |
| `model`, `provider` | Passed to the agent. `provider` is for `pi` |
| `effort`, `effort_fallback` | Default effort; `effort_fallback: nearest` instead of an error |
| `permission`, `allow_full_access` | Default tier; `full-access` needs `allow_full_access: true` |
| `account` | A label shown in listings |
| `home` | The agent's login directory for this profile |
| `env` | Names of variables to pass through |
| `env_from` | `{AGENT_VAR: SHELL_VAR}`: pass a variable under another name |
| `vars` | Literal, non-secret variables |
| `args` | Extra arguments for the agent |
| `provides` | `{computer_use: {via: native}}` or `{via: lcu}` |
| `permission_requests`, `allow_execute`, `auth_method` | ACP only: `verified` once checked; commands allowed at `workspace-write`; the login method to call |
| `description` | Shown in listings, and to an advisor choosing an executor |

A full sample: [`doc/examples/config.yaml`](../examples/config.yaml).

Agent processes get a small, fixed set of environment variables (such as `PATH`, `HOME`, locale and
proxy settings) plus the ones a profile names. Nothing else from your shell reaches them.

## 12. Task files

Anything `hufu run` takes as flags can be written down. Flags override the file.

```yaml
version: 1
title: Share one in-flight token refresh
prompt: |
  Refactor the token refresh logic under src/auth so that concurrent callers
  share a single in-flight refresh request.
agent: codex-any
permission: workspace-write
effort: high
timeout: 45m
isolation: worktree
acceptance:
  - command: npm test -- auth
  - paths: {allow: ["src/auth/**", "test/auth/**"], deny: ["**/*.lock"]}
strict_permissions: false
depends_on: []
retry: {max_attempts: 2}
grants:
  - {capability: computer_use, apps: [Figma]}
computer_use: {route: dedicated, agent: desk}
labels: {area: auth}
```

Other keys: `cwd`, `model`, `inactivity_timeout`, `output: {schema: ...}`.

## 13. Exit codes and JSON output

| Code | Meaning |
|---|---|
| 0 | Success |
| 1 | Internal error |
| 2 | Usage or configuration error; nothing was started |
| 3 | The job failed |
| 4 | Cancelled |
| 5 | Timed out |
| 6 | `wait` gave up; the job is still running |
| 7 | Lost: the runner died |
| 8 | Rejected by an acceptance check |
| 10 | The agent is not installed or not logged in |

`--json`, before the command, prints one JSON document with `"v": 1` instead of text:
`hufu --json status j_...`. An error is then a document too: `{"v": 1, "error": {"kind": ..., "message": ...}}`.

## 14. Where things are stored

Everything is under `~/.hufu` (or `$HUFU_HOME`):

```text
~/.hufu/
├── config.yaml
├── jobs/j_<id>/        spec.json, state.json, events.ndjson, result.json, raw/stdout.log, raw/stderr.log
├── missions/m_<id>/    mission.json, plan.json, ledger.ndjson, report.md
├── worktrees/          git worktrees of isolated jobs and missions
└── run/                slots of running jobs, the daemon's pid file and log
```

Files are plain JSON and text, so that you can read them, and so that any session can pick up a
job another one started. Prompts and agent output are stored here unencrypted; keep the directory
private.

## 15. Troubleshooting

| What you see | What to do |
|---|---|
| Exit 10, `agent_unavailable` | The agent's program is not on `PATH`. `hufu agents --probe` shows which program is looked for |
| Exit 10, `auth_required` | Log the agent in, or export the variable named in the message |
| `cannot enforce permission ...` | That agent has no such tier. Pick another tier or another profile; `hufu agents` lists them |
| `effort ... has no mapping` | Use a listed level, or set `effort_fallback: nearest` on the profile |
| `tier_violated` | A read-only job changed files. The changes are listed in the result |
| A job stays `queued` | It waits for a free slot (`hufu status <job>` says so; see `limits`), for a job it depends on, or for the display |
| `lost` | The runner process died, for example at a reboot. `hufu resume` or run it again; with the daemon and `--retry` this is automatic |
| Nothing works | `hufu doctor`, then `hufu doctor --self-test` |

## 16. Known limits

- **Not yet run against real accounts in CI.** The adapters follow each agent's documentation as
  of November 2026 and are tested against fakes that speak the same formats.
- **Kimi Code print mode and pi cannot be sandboxed** by their own means: Kimi supports only
  `full-access`, pi only `read-only` and `full-access`. Use `kimi-acp` to get all tiers for Kimi.
- **Timeouts are per attempt.** A job with `--retry 3` can run three times its timeout.
- **A lost job's agent process is not killed automatically.** Hufu will not kill by a process id
  that may have been reused. `hufu cancel --force <job>` does it on request.
- **Cancelling during an acceptance command** takes effect when that command ends.
- **`hufu_wait` over MCP blocks that connection** until it returns; keep its timeout short, as its
  description tells the calling agent.
- **Windows.** A cancelled agent is stopped by ending its process tree at once, since there is no
  polite signal to send. Processes that outlived their parent are not found by that.
