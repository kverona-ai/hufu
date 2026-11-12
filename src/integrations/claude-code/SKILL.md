---
name: hufu
description: Delegate work to other coding agents (Codex, Antigravity, Cursor, Kimi, pi, another Claude Code account) through Hufu. Use when a task can run in the background, when a second opinion from a different model helps, when several independent tasks can run side by side, or when the user asks to "dispatch", "delegate" or "hand off" work.
---

# Hufu: dispatch work to other agents

Hufu runs other agents as background **jobs** and gives you back compact, structured results.
You stay the commander: you decide what to hand off and you check what comes back.

Use the `hufu_*` MCP tools when they are available. The `hufu` command line does the same.

## The loop

1. `hufu_agents` – see which profiles and pools exist, what each can do, and which roles are set.
2. `hufu_submit` – hand off one task. Returns a job id at once. Give:
   - `prompt`: self-contained. The executor sees only this prompt and the workspace.
   - `agent`: a profile or pool name from step 1.
   - `permission`: `read-only` (default), `workspace-write`, or `full-access`. Ask for the lowest that works.
   - optional `effort`, `cwd`, `isolation: worktree` (own copy of the repository on its own branch),
     `acceptance` (commands that must exit 0, path rules), `output_schema`.
3. Keep working. Check with `hufu_status`, or block for a bounded time with `hufu_wait`.
4. `hufu_result` – the final message, changed files, skipped actions, tally outcomes, usage.
5. Judge the result yourself. If it needs changes, `hufu_resume` sends a follow-up to the same session.

## Rules

- Never ask for `full-access` unless the user said so for this task.
- A job can **succeed and still be rejected**: `state` is about the run, `verdict` is about the
  acceptance checks. Read both.
- `denied_actions` lists what the executor wanted to do but was not allowed to. A result with
  denied actions is usually incomplete: report it or resubmit with a higher tier if the user agrees.
- Two writers in one directory overwrite each other. For parallel write tasks use `isolation: worktree`.
- `hufu_wait` is bounded (60 s by default). A `wait_timed_out` answer means "still running", not failure.
- Desktop control (computer use) needs an explicit grant listing the applications. Never add one on your own.

## Larger pieces of work

`hufu_mission_start` runs a managed loop: an advisor profile plans, executors work, the advisor
reviews, within budgets. Use it when the user wants a goal carried out unattended. Follow it with
`hufu_mission_status` and read the report when it ends.

## Setting Hufu up for the user

`hufu_doctor` shows what is installed and logged in. `hufu_profile_set`, `hufu_pool_set` and
`hufu_config_set` change the configuration; every change is validated before it is written.
Several accounts of one agent are separate profiles with their own `home` directory; put them in a
pool to get failover between accounts.

## Command line equivalents

```
hufu run -a codex -p workspace-write --isolation worktree "Refactor the token refresh logic"
hufu status            hufu wait <job>            hufu result <job>
hufu logs <job> -f     hufu cancel <job>          hufu resume <job> "Also update the docs"
```
