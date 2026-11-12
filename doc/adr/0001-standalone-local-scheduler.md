# ADR-0001: Standalone local scheduler, CLI first

| | |
|---|---|
| **Status** | Accepted |
| **Date** | 2026-11-12 |

## Context

The need started as "let Claude Code hand tasks to another agent". The lightest ways to do that are
a shell snippet in `CLAUDE.md`, a Claude Code plugin, or an MCP server that wraps one agent.
Community projects exist for each ([prior art](../research/prior-art.md)).

Those approaches share three weaknesses:

- A delegated task lives inside the host's session. If the session ends, is compacted or hits a
  tool-call timeout, the task is lost.
- The integration is tied to one host. Using a different agent as the planner means starting again.
- A synchronous MCP tool call is a poor fit for work that takes many minutes.

## Decision

Hufu is its own local program with its own on-disk state. Hosts are clients.

- The **CLI** is the primary interface. Every operation is a non-interactive command with JSON
  output and stable exit codes. Any agent that can run a shell command can use it.
- Jobs run in processes that do not depend on the caller and record everything on disk.
- The host side stays thin: a handful of commands plus a skill that explains when to use them.
- An **MCP server** is an optional outer shell added later. Its submit tool returns a job id
  immediately; no tool call lasts as long as a job.

## Alternatives considered

| Alternative | Why not |
|---|---|
| A Claude Code plugin or skill only | Bound to one host; jobs die with the session |
| An MCP server as the core | Tool-call timeouts; needs a running server before anything works; harder to script |
| Adopt an existing orchestrator | The closest ones are built around a fixed commander or a human UI ([prior art](../research/prior-art.md)) |

## Consequences

- Users install one more tool.
- A job can be inspected with ordinary file tools, which makes debugging simple.
- The CLI's JSON shapes and exit codes are a public contract from the first release.
- A daemon is not needed at first ([architecture, section 7](../architecture/architecture.md#7-process-model));
  it arrives when quotas need a single owner.
