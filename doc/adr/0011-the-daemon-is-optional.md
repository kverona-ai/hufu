# ADR-0011: The daemon is optional; limits are enforced by the runners themselves

| | |
|---|---|
| **Status** | Accepted. Changes the process model sketched for milestone M4 in the first architecture draft |
| **Date** | 2026-11-12 |

## Context

The first design had two stages: no daemon at first, with advisory limits, and later a daemon that
would own every runner, enforce quotas, and be required by the MCP server. The reason given was
that queues and quotas need a single owner.

Building it showed that they do not. Each runner can take a slot file under a short lock before it
starts its agent, and remove files left by dead processes on the way. That enforces the global
limit, per-profile limits and the display slot without anyone in charge.

Hufu also has to behave the same on Linux, macOS and Windows. A required service means a launchd
unit, a systemd unit and a Windows service, each with its own way of failing.

## Decision

1. **Every job has its own runner process, with or without a daemon.** The daemon never owns a
   runner; it only starts them.
2. **Limits are enforced by runners** through slot files in `run/slots/`.
3. **The MCP server calls the core directly** and does not need the daemon.
4. **The daemon is an optional helper** that adds what a single observer is good for: starting
   queued jobs strictly in submission order, restarting jobs whose runner died when their retry
   policy allows, and pruning on a schedule.
5. **The daemon holds no state.** Everything it knows is in the job store, so it can be stopped,
   started or killed at any time. Its absence is detected by a heartbeat, and jobs it left queued
   start themselves when next looked at.
6. Hufu does not install service units. `hufu daemon run` is a plain foreground program for any
   service manager.

## Alternatives considered

| Option | Why not |
|---|---|
| A required daemon that owns runners | A single point of failure for work that otherwise survives anything; three service integrations; and nothing would run after an install until the service does |
| No daemon at all | Without an observer, start order among waiting jobs is "whoever polls first", and a job whose runner died stays lost until a person retries it |
| A database with row locks instead of slot files | See [ADR-0006](0006-file-based-job-store.md) |

## Consequences

- `hufu run` works straight after installation, and the first-run experience has no service step
  that can fail.
- Without the daemon, waiting jobs start in no guaranteed order. People who care run the daemon.
- A crashed runner leaves a slot file; it is cleaned up by the next reader that sees the process
  is gone. Limits therefore depend on a working process-liveness check, which is one function per
  platform.
- The daemon can be upgraded or restarted while jobs run.
