# Roadmap

| | |
|---|---|
| **Status** | Plan |
| **Last updated** | 2026-11-12 |
| **Related** | [Requirements](requirements/requirements.md), [Architecture](architecture/architecture.md) |

Milestones are ordered so that each one is useful on its own and none requires rewriting the one
before. Requirement ids (FR-xx) refer to the [requirements](requirements/requirements.md).

```mermaid
flowchart LR
    M0["M0<br/>Foundation<br/>docs and layout"] --> M1["M1<br/>First dispatch<br/>core, CLI, 2 adapters"]
    M1 --> M2["M2<br/>Many agents<br/>ACP, pi, worktrees"]
    M2 --> M3["M3<br/>Pluggable commander<br/>missions, tally checks"]
    M3 --> M4["M4<br/>Always on<br/>daemon, quotas, MCP"]
```

## M0. Foundation

**State: in progress (this is the current milestone).**

| Deliverable | Status |
|---|---|
| Requirements, research, architecture, technical design, deployment plan | Done in draft |
| Architecture decision records | Done in draft |
| Repository layout: `doc/` for documents, `src/` for code | Done |
| README in English and Chinese | Done |
| Decisions on the open questions in the requirements (Q1, Q2) | Waiting for the project owner |

**Exit:** the documents are reviewed and Q1 (implementation language) and Q2 (first adapter) are
answered.

## M1. First dispatch

**Goal:** from Claude Code, hand a task to another agent, keep working, and collect the result
later, even from a different session.

| Deliverable | Requirements |
|---|---|
| Package scaffold under `src/`, CI, the `hufu` environment for cloud sessions | |
| Task spec and configuration schemas with validation | FR-01, FR-21, FR-47 |
| Job store, detached runner, heartbeat, timeouts, cancel, lost-job detection | FR-08, FR-11 to FR-13 |
| CLI: `run`, `status`, `wait`, `result`, `logs`, `cancel`, `resume`, `agents`, `doctor` | FR-02 to FR-07, FR-22, FR-43, FR-44 |
| Adapter contract and conformance suite with fake agents | FR-20 |
| Codex CLI adapter | FR-15 |
| Antigravity CLI adapter | FR-15 |
| Permission tiers and effort mapping for both adapters | FR-24 to FR-29 |
| Claude Code skill and `CLAUDE.md` snippet | FR-34 |

**Exit:** the durability test passes (a job outlives its caller and its result is read from a new
session), both adapters pass the conformance suite, and a live smoke test succeeds on a real
install of each agent.

**Risks:** the `agy` non-TTY output bug and the open terms-of-use question. If either blocks, Codex
ships first and `agy` follows.

## M2. Many agents

**Goal:** one task spec reaches at least six agents, including a model on an open API.

| Deliverable | Requirements |
|---|---|
| Generic ACP adapter | FR-16 |
| Verified ACP profiles: Cursor, Kimi Code, pi (through `pi-acp`) | FR-17 |
| pi adapter with provider and model profiles for OpenRouter, DeepSeek, Z.ai and Kimi | FR-18 |
| Claude Code adapter | FR-19 |
| Git worktree isolation and same-directory write protection | FR-31, FR-33 |
| Structured output with schema validation | FR-09 |
| Job retention and pruning | FR-14 |

**Exit:** adding an unlisted ACP agent works from configuration alone, demonstrated with one agent
not named in this plan.

## M3. Pluggable commander

**Goal:** any profile can be the advisor. A mission runs from goal to report without a person in
the loop.

| Deliverable | Requirements |
|---|---|
| Mission engine: roster, plan request, plan validation, dependency-ordered dispatch | FR-35 to FR-37, FR-41 |
| Tally checks: command, paths, schema, advisor review | FR-38 |
| Review loop with accept, revise and abort; follow-ups on resumed sessions | FR-39 |
| Budgets: rounds, wall clock, tokens or cost | FR-40 |
| Mission inspection, pause and resume; mission report | FR-42 |
| Usage accounting per job and per mission | FR-49 |

**Exit:** the same goal completes with two different advisor profiles, one of them a model reached
through pi, with no code change between the runs.

## M4. Always on

**Goal:** Hufu becomes a service other tools can rely on.

| Deliverable | Requirements |
|---|---|
| Daemon with enforced concurrency limits, launchd and systemd units | FR-32, FR-46 |
| MCP shell | FR-45 |
| Retry policies | FR-10 |
| Plug-in adapters loaded from outside the package | FR-23 |
| Decision on Windows support | |

**Exit:** an MCP host other than Claude Code submits a job and reads its result.

## After M4

Ideas that are deliberately not scheduled:

- Result delivery by push, for hosts that cannot poll.
- A read-only web view of jobs and missions.
- Sharing a roster and budgets across a team.
- Remote executors: dispatching to agents on another machine once ACP has a stable remote
  transport.
