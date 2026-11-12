# ADR-0005: Permissions and reasoning effort are always explicit and mapped per adapter

| | |
|---|---|
| **Status** | Accepted |
| **Date** | 2026-11-12 |

## Context

Research into each agent's headless mode found that defaults disagree completely
([agent landscape](../research/agent-landscape.md)):

- Codex defaults to a read-only sandbox.
- Cursor applies edits inside the workspace and rejects other actions.
- Antigravity skips actions that need approval, and still exits `0`.
- Kimi's print mode approves everything.
- pi has no permission system.

Reasoning controls are just as uneven: six named levels, three levels, a boolean, or nothing.

A dispatcher that passes a prompt and trusts defaults would be read-only on one agent and
unrestricted on the next, with the same task spec.

## Decision

1. Hufu defines **three permission tiers** (`read-only`, `workspace-write`, `full-access`) and
   **one effort scale** (`off`, `low`, `medium`, `high`, `max`, plus `default`).
2. Every run carries an explicit tier. The global default is `read-only`. `full-access` is never
   implied.
3. Each adapter **declares** which tiers and effort levels it can honour and **translates** them
   into explicit flags or protocol answers.
4. A request that cannot be honoured is **rejected before launch**, with a message naming what is
   supported. Nothing is silently ignored, substituted or widened.
5. Actions an agent skipped for lack of permission are **reported**, and by default fail the job.

## Alternatives considered

| Alternative | Why not |
|---|---|
| Pass agent-native flags through from the task spec | The caller, often another model, would need to know every agent's flags; mistakes would be silent |
| Best effort: use the nearest supported tier | Widening permissions without being asked is the exact failure this project must prevent |
| Always run everything unrestricted inside a sandbox Hufu provides | Building a sandbox is out of scope, and it would remove useful read-only runs |

## Consequences

- Some combinations are simply unavailable, for example `read-only` on pi. Users must choose a
  supported tier and add isolation. This is intended.
- Nearest-level fallback for *effort* is allowed as an opt-in per profile, because a different
  reasoning level changes quality, not safety. There is no fallback for permission tiers.
- The mapping tables in the [adapter contract](../technical/adapter-contract.md) must be kept
  current as agent CLIs change; the conformance suite and version checks in `hufu doctor` are the
  guard.
- A tier is only as strong as the agent's own enforcement. The documentation says so plainly.
