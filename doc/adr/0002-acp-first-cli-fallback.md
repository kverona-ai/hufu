# ADR-0002: ACP first, headless CLI adapters alongside

| | |
|---|---|
| **Status** | Accepted |
| **Date** | 2026-11-12 |

## Context

There are two ways to drive a coding agent from another program
([agent landscape](../research/agent-landscape.md)):

- **Agent Client Protocol.** One JSON-RPC protocol over stdio, with sessions, streaming updates,
  permission requests and, since February 2026, a stable way to select model and reasoning level.
  Dozens of agents implement it natively or through an adapter.
- **Headless CLI modes.** Every agent has one, and every one is different.

Neither is enough alone. Antigravity's CLI has no ACP entry point. Codex's ACP support is a
separate adapter, while its own CLI is richer and better documented. Kimi's print mode approves
everything, so ACP is the only way to run it with tighter permissions.

## Decision

Hufu has two adapter families behind one contract:

1. A **generic ACP adapter**, driven entirely by configuration. This is the default way to add an
   agent.
2. **Per-agent CLI adapters**, written where ACP is missing, weaker, or less direct.

Each agent has a *preferred surface*, recorded in the
[adapter contract](../technical/adapter-contract.md). A profile can choose the other surface when
both exist.

The order of work follows risk, not preference: CLI adapters for the first two agents come first
(M1), because they are needed regardless and they exercise the contract; the ACP adapter follows
(M2) and brings most other agents with it.

## Alternatives considered

| Alternative | Why not |
|---|---|
| ACP only | Leaves out Antigravity today, and loses control wherever an adapter lags the vendor's CLI |
| CLI adapters only | One adapter per agent forever; ignores a standard that already covers most of the field |
| MCP as the agent-side protocol | MCP exposes tools to a model. It does not model an agent session with prompts, streaming turns and permission requests |

## Consequences

- Two code paths must produce the same normalized events. The conformance suite exists to hold
  them to that.
- ACP standardises the conversation, not the sandbox. Permission tiers over ACP depend on what each
  agent asks about, so a generic ACP profile advertises restricted tiers only after that agent has
  been checked.
- ACP v2 is in draft. The adapter targets v1 and negotiates the version at `initialize`.
- A remote ACP transport is still a draft, so executors are local subprocesses for now.
