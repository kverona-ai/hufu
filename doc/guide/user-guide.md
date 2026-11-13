# Hufu User Guide

| | |
|---|---|
| **Status** | Describes Hufu 0.1 as implemented. The examples in it are checked by the test suite where they are files under `doc/examples/` |
| **Last updated** | 2026-11-13 (model and effort, who gets a task) |
| **Related** | [Agent install guide](agent-install.md), [Deployment](../deployment/deployment.md), [Technical design](../technical/technical-design.md), [Adapter contract](../technical/adapter-contract.md) |

Everything you can do with Hufu, from the first command to missions and computer use.
For why it works this way, see the [architecture](../architecture/architecture.md); for how it is
built, the [technical design](../technical/technical-design.md).

- [1. Install and set up](#1-install-and-set-up)
- [2. The ideas in one minute](#2-the-ideas-in-one-minute)
- [3. Jobs](#3-jobs)
- [4. Agents, accounts and pools](#4-agents-accounts-and-pools)
- [5. Permissions and effort](#5-permissions-and-effort) (and the [model](#model-and-effort))
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

The usual way is to let the coding agent you already use do it: give it the
[agent install guide](agent-install.md) and it installs, sets up and checks Hufu, then tells you
what is left for you (normally a login and a restart of the agent). The same steps by hand:

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
| 1 | `hufu init` | Looks for agents on your `PATH` and writes `~/.hufu/config.yaml` naming them, with a default advisor and default executors. An existing file is kept. With no agent found, no file is written |
| 2 | `hufu setup` | Registers Hufu's MCP server with the agents it found, so that each of them can dispatch through Hufu. Skip with `--no-setup` |
| 3 | `hufu daemon start` | Starts the optional daemon. Skip with `--no-daemon` |
| 4 | `hufu doctor` | Checks installation, logins and configuration, and lists what to do next |

Run `hufu up` again at any time: it changes nothing that is already in place.

`hufu up` asks no questions, and its last line says where you stand:

- **`ready. Try: hufu run -a <profile> ...`** Everything is set up.
- **`hufu: not ready: no supported agent is installed`** (exit code 10). Hufu found no agent to
  hand work to, so it stopped before changing anything: no configuration file, no registration, no
  daemon. The output lists how to install and log in to each supported agent. Install one, then
  run `hufu up` again.
- **`only one agent is in use`**, under "next steps". With a single agent, that agent is both the
  advisor and the only executor: it plans, does and reviews the work by itself. That works, but
  Hufu is built to hand work to other agents. Install a second agent, or add a second account of
  the same one ([section 4](#4-agents-accounts-and-pools)), and name it as an executor:
  `hufu config set roles.executors "[codex, claude]"`.
- **`no agent that could host Hufu was found`**. None of the agents that can call Hufu over MCP is
  installed. The entry to add to any other MCP-capable agent is printed with it.

`hufu doctor --self-test` goes one step further and runs real jobs against agents that are built
into the binary. It needs no account and no network, and it proves that background jobs, permission
enforcement and cancellation work on your machine.

Other ways to install, and how to uninstall, are in the [deployment guide](../deployment/deployment.md).

### Updating

```console
$ hufu update              # move to the newest release
$ hufu update --check      # only say whether there is a newer one
```

`hufu update` replaces the installed program with the newest release:

1. It reads the release's manifest, `hufu-manifest.json`, to learn the newest version. When that
   is the installed one, it says so and stops.
2. It downloads the archive for this platform and compares its SHA-256 sum with the manifest.
3. It starts the new program once, to make sure that it runs on this machine and is the version
   the release names.
4. It puts the new program in the place of the old one, with a rename.
5. It restarts the daemon if one was running, and runs `hufu doctor` with the new program.

When step 1, 2 or 3 fails, nothing was changed, and the message says so.

| Flag | Meaning |
|---|---|
| `--check` | Report the installed and the newest version. Nothing is downloaded or changed |
| `--version <v>` | Install that release, written `v0.2.0` or `0.2.0`, instead of the newest one. This is also how to go back to an older release |
| `--from-source` | Build with cargo instead of downloading: the main branch, or the tag given with `--version`. Needs Rust 1.89 or newer |
| `--force` | Install even when that version is already installed |

What an update leaves alone:

- **Configuration and job records.** Only the program file changes.
- **Running jobs.** They finish with the program they started with.
- **Agents that have Hufu connected.** Each keeps its old MCP server until that agent is
  restarted. Jobs it submits in the meantime already run with the new program. Restart the agent
  to get new tools.

When there is no release to download (none is published where Hufu looks, or it has no binary for
this platform), `hufu update` builds from source with cargo instead and says that it does.

The download uses `curl` (or `wget`) and `tar` from your system; Hufu checks the result itself.
Windows has both since Windows 10 version 1803; on an older Windows, run the install script again
instead. On a network that cannot reach GitHub, set `HUFU_DOWNLOAD_BASE` to a URL or a local
directory that holds the files of one release, the manifest and the archives side by side.

`hufu --json update --check` prints `current`, `latest`, `update_available` and `release_found`.
`hufu --json update` prints `updated`, `from`, `to`, `method` (`release` or `source`), the state
of the daemon and the `doctor` report. The exit code is `0` also when there was nothing to do, `1`
when the download or a check failed, and `2` for a version that does not exist. When the update
went in but `hufu doctor` then reports a problem, the document says `"updated": true,
"doctor_ok": false` and the exit code is the one of `hufu doctor`.

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
| `-e, --effort <level>` | `default`, `off`, `low`, `medium`, `high`, `max`, or `native:<value>`. See [Model and effort](#model-and-effort) |
| `-m, --model <id>` | A model id, passed to the agent as it is. Default: the profile's model |
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

A pool is for profiles that can stand in for each other. When a task names a pool, Hufu picks the
member, in three steps:

1. **Order the members**, at submission, by the pool's strategy.

   | Strategy | Order |
   |---|---|
   | `failover` (default) | As listed in the pool |
   | `round-robin` | As listed, starting one member further with every job |
   | `least-busy` | The member with the fewest running jobs first; ties keep the listed order |

2. **Leave out members that cannot take the task as asked**: a permission tier or an effort level
   the member does not have, or a tier different from the one the first usable member would run
   at. Each one left out is a warning on the job. When no member is left, the task is refused
   before anything starts.
3. **Run the job on the first member left.** The job moves on to the next one only when the member
   running it is not installed, not logged in or rate limited. A job that failed because the agent
   got the task wrong is not repeated on another account.

A `model` or `effort` named on the task goes with the job to whichever member runs it; otherwise
each member uses its own. The job's status and result name the profile that really ran, and a
switch to another member is recorded as a warning.

### Roles

```console
$ hufu config set roles.advisor claude
$ hufu config set roles.executors "[codex-any, agy, kimi-acp]"
```

There is one advisor, and it is a profile, not a pool. Executors can be any number of profiles and
pools. Both are only defaults: `hufu run -a` and `hufu mission start --advisor/--executors`
override them.

### Who gets a task

With several executors configured, Hufu does not spread tasks over them by itself. Every task
names one profile or one pool, and who writes that name depends on who is in command:

| Who is in command | Who picks the executor | How |
|---|---|---|
| You, on the command line | You | `hufu run -a <profile-or-pool>`. Without `-a`: the first entry of `roles.executors` |
| An agent that has Hufu connected, one task at a time | That agent | It reads `hufu_agents` and names a profile or a pool in the `agent` argument of `hufu_submit`, which is required. See [section 9](#9-using-hufu-from-another-agent-mcp) |
| A mission | The advisor | It gets the roster and writes a plan in which every task names one entry. See [section 7](#how-a-mission-hands-out-tasks) |
| Any of them, when the name is a pool | Hufu | By the pool's strategy, see [Pools](#pools) |

In every case the limits hold: at most `limits.max_concurrent_jobs` jobs run at once (default 4),
and `limits.per_profile.<name>` caps one profile. A job over a limit stays `queued` until a slot
is free; it is not moved to another executor.

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

### Model and effort

Every job runs with a model and a reasoning effort. Each can be set in three places, and the first
one that says something wins:

| | Where | Applies to |
|---|---|---|
| 1 | The task: `-m` and `-e` on `hufu run`, `model:` and `effort:` in a [task file](#12-task-files), `model` and `effort` on `hufu_submit` | That one job |
| 2 | The profile: `model` and `effort` (`hufu profile add/set --model --effort`) | Every job of that profile |
| 3 | `defaults.effort` in the configuration (effort only; there is no default model) | Everything else |

When none of them says anything, the agent uses its own default model and effort.

```console
$ hufu run -a codex -m gpt-6.1-sol -e high "Review src/auth for race conditions"
$ hufu profile add codex-deep --adapter codex-cli --model gpt-6.1-sol --effort max \
    --description "Hard design and debugging work"
$ hufu config set defaults.effort medium
```

**Effort** is one scale for every agent: `off`, `low`, `medium`, `high`, `max`, plus `default`
(pass nothing and let the agent decide) and `native:<value>` (pass the agent's own value through,
for example `native:xhigh` for Codex).

| Agent | Levels it has | What Hufu passes for `high` | Model |
|---|---|---|---|
| Codex | `low` to `max` | `-c model_reasoning_effort="high"` | `-m <id>` |
| Claude Code | `low` to `max` | `--effort high` | `--model <id>` |
| Antigravity | `low` to `high` | `--effort high` | `--model <id>` |
| Kimi Code | `low` to `max` | the variable `KIMI_MODEL_THINKING_EFFORT=high` | `-m <id>` |
| pi | `off` to `max` | `--thinking high` | `--model <id>` |
| Cursor | none | nothing: pick reasoning strength through the model id | `--model <id>` |
| ACP agents | what the agent offers when the session opens | a session option | a session option |

`hufu agents` lists the levels of each of your profiles. The rules:

- **A level asked for on the task or the profile that the agent does not have is an error**,
  raised before anything starts. `hufu profile set <name> --effort-fallback nearest` makes that
  profile use the closest level it has instead; the substitution is a warning on the job.
- **`defaults.effort` is milder.** For an agent without that level, the agent's own default is
  used, with a warning.
- **For an ACP agent** the levels and models are only known once its session is open, so one it
  does not offer fails the job at its start, not at submission.
- **A model id is passed on as it is.** Hufu keeps no list of models; the agent decides whether
  the id exists.

`--dry-run` prints the exact command line a combination leads to, see [section 3](#3-jobs). After
a job, `hufu result` names the profile, model and effort that really ran.

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

### How a mission hands out tasks

1. **The roster.** It is `--executors`, or `roles.executors` when that is left out: profiles and
   pools. For every entry the advisor is told its description, adapter, permission tiers, effort
   levels, model and account, whether it can take follow-ups and whether it can operate desktop
   applications. It is told about nothing outside the roster.
2. **The plan.** The advisor answers with a list of tasks. Each task has a prompt, names one roster
   entry as its agent, lists the tasks it depends on, and may set a permission tier and an effort
   level. It cannot set a model. To let the advisor choose between models, make each one a
   profile and say in its `description` what it is good at:

   ```console
   $ hufu profile add codex-deep --adapter codex-cli --model gpt-6.1-sol --effort max \
       --description "Hard design and debugging work"
   $ hufu mission start --advisor claude --executors codex-deep,codex-any,agy "..."
   ```

3. **The check.** Hufu validates every task the way it validates `hufu run`, within the mission's
   bounds. A plan that names an agent outside the roster, asks for a tier above `max_tier`, or for
   an effort level the chosen agent does not have, goes back to the advisor once with the list of
   problems. A second invalid plan fails the mission with `plan_invalid`.
4. **The order.** Tasks start in the order of the plan, each as soon as the tasks it depends on are
   accepted. `missions.max_parallel` (default 1) says how many run side by side.
5. **Pools.** A task assigned to a pool goes to one of its members by the rules in
   [Pools](#pools).
6. **Rounds.** A task the advisor sends back for changes returns to the same executor, in the same
   session where the agent can resume one, up to `max_rounds_per_task` rounds. It is not handed
   to a different executor. When a task fails, the tasks that depend on it are skipped.

`hufu mission status <id>` shows which executor each task went to, and `hufu mission report <id>`
keeps that on record.

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

### What to say, and the call it becomes

You talk to the agent; the agent calls the tools. These are the calls behind some common requests.

**One task, with a model and an effort.** "Have Codex review src/auth for race conditions. Use
gpt-6.1-sol at high effort."

```jsonc
// hufu_submit
{
  "agent": "codex",
  "prompt": "Review src/auth for race conditions. Give the file and line of each finding.",
  "model": "gpt-6.1-sol",
  "effort": "high"
}
```

`agent` and `prompt` are required. `model` and `effort` override the profile for this job only;
see [Model and effort](#model-and-effort). With `"dry_run": true` the call returns the command
line it would run (`launch`), the profile, tier and effort it resolved to, and any warnings, and
starts nothing. The answer is otherwise a job id; the agent follows it with `hufu_wait` or
`hufu_status` and reads `hufu_result`, whose `agent` block names the profile, model and effort
that really ran.

**Two tasks side by side, on different executors.** "Have Codex fix the failing auth tests in a
worktree, and at the same time have Antigravity update the API docs."

```jsonc
// hufu_submit, twice
{ "agent": "codex", "prompt": "...", "permission": "workspace-write", "isolation": "worktree" }
{ "agent": "agy",   "prompt": "...", "permission": "workspace-write", "isolation": "worktree" }
// then one hufu_wait
{ "jobs": ["j_...", "j_..."], "timeout_s": 50 }
```

The agent is the commander here: it chose who gets which task, from what `hufu_agents` told it.

**A named combination.** "Add a Hufu profile codex-deep: Codex with gpt-6.1-sol at max effort, for
hard design and debugging work."

```jsonc
// hufu_profile_set
{
  "name": "codex-deep",
  "profile": {
    "adapter": "codex-cli",
    "model": "gpt-6.1-sol",
    "effort": "max",
    "description": "Hard design and debugging work"
  }
}
```

Fields that are not named are kept (`"merge": false` replaces the profile). To change one field of
a profile that exists only as a preset, name its `adapter` too, as in
`{"name": "agy", "profile": {"adapter": "agy-cli", "effort_fallback": "nearest"}}`.

**A default.** "Make medium the default effort."

```jsonc
// hufu_config_set
{ "key": "defaults.effort", "value": "medium" }
```

**A pool.** "Make a pool of my two Codex accounts that takes turns."

```jsonc
// hufu_pool_set
{ "name": "codex-any", "members": ["codex", "codex-work"], "strategy": "round-robin" }
```

**A mission.** "Start a mission: migrate the HTTP client to async/await and keep the tests green.
Claude plans and reviews; codex-deep, codex-any and agy do the work."

```jsonc
// hufu_mission_start
{
  "goal": "Migrate the HTTP client from callbacks to async/await; keep the tests green",
  "advisor": "claude",
  "executors": ["codex-deep", "codex-any", "agy"]
}
```

Now the advisor is the commander, see
[How a mission hands out tasks](#how-a-mission-hands-out-tasks). The agent follows the mission
with `hufu_mission_status`.

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
| `model`, `provider` | The default model, passed to the agent as it is; a task can override it. `provider` is for `pi` |
| `effort`, `effort_fallback` | Default effort; `effort_fallback: nearest` uses the closest level the agent has instead of an error (`--effort-fallback`) |
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
| 10 | The agent is not installed or not logged in. From `hufu up` and `hufu init`: no supported agent is installed at all |

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
| `hufu up` ends with `not ready: no supported agent is installed` (exit 10) | No agent is installed, so nothing was set up. Install and log in to one of the agents the output lists, then run `hufu up` again |
| `only one agent is in use` | Not an error: your one agent is both advisor and only executor. Add a second agent or account, then `hufu config set roles.executors "[<first>, <second>]"` |
| `configuration kept`, but the file names no agent | The file was written before any agent was installed. `hufu init --force` rewrites it from the agents found |
| Exit 10, `agent_unavailable` | The agent's program is not on `PATH`. `hufu agents --probe` shows which program is looked for |
| Exit 10, `auth_required` | Log the agent in, or export the variable named in the message |
| `cannot enforce permission ...` | That agent has no such tier. Pick another tier or another profile; `hufu agents` lists them |
| `effort ... has no mapping` | Use a level `hufu agents` lists for that profile, or `hufu profile set <name> --effort-fallback nearest` |
| `tier_violated` | A read-only job changed files. The changes are listed in the result |
| A job stays `queued` | It waits for a free slot (`hufu status <job>` says so; see `limits`), for a job it depends on, or for the display |
| `lost` | The runner process died, for example at a reboot. `hufu resume` or run it again; with the daemon and `--retry` this is automatic |
| `hufu update` says `cannot write next to ...` | The program was installed by another user or by a package manager. Run the update as that user, or run the install script again |
| `hufu update` says `checksum mismatch` | The download is damaged or is not what the release published. Nothing was changed. Try again; if it repeats, do not install the file |
| `hufu update` builds from source, or says `building from source needs Rust` | There is no release with a binary for this platform where Hufu looked. Install Rust from <https://rustup.rs>, or check `HUFU_DOWNLOAD_BASE` |
| After `hufu update`, an agent still shows the old tools | Its MCP server was started before the update. Restart that agent |
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
