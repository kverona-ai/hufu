# ADR-0006: Jobs are stored as plain files; a database index is deferred

| | |
|---|---|
| **Status** | Accepted. Proposed during design; built as proposed when the project owner asked for the whole plan to be implemented (2026-11-12) |
| **Date** | 2026-11-12 |

## Context

Jobs must outlive the session that submitted them, and any later process must be able to read
their state, logs and results. Before the daemon exists (M1 to M3), several unrelated processes
touch the store at once: the submitting CLI, one runner per job, and any number of readers.

## Decision

Each job is a directory under `~/.hufu/jobs/` containing JSON and NDJSON files
([technical design, section 6](../technical/technical-design.md#6-job-store)).

- `spec.json` and `result.json` are written once.
- `state.json` is replaced atomically by write-then-rename. Only the job's runner writes it while
  the job is live.
- `events.ndjson` is append-only, one complete line per write.
- Listing jobs means listing directories; ids sort by time.

No database is used until the daemon arrives. If listing becomes slow with thousands of jobs, the
daemon may keep an index, with the files remaining the source of truth.

## Alternatives considered

| Alternative | Why not now |
|---|---|
| SQLite from the start | Solves a problem not yet present; concurrent writers from many short-lived processes need care; job logs still need files |
| An embedded key-value store | Same concerns, less tooling |
| A daemon that owns all state from M1 | Makes the first milestone much larger and the tool harder to debug |

## Consequences

- A job can be inspected, copied or deleted with ordinary tools. An agent can read a result file
  directly.
- One writer per job means no locking is needed for job state. Concurrency slots use simple lock
  files and are advisory until the daemon enforces them.
- Queries are limited to lookup, list and filter by a few fields. That is all the requirements ask
  for.
- Pruning is deleting directories.
- The format is versioned, so a later index or a move to a database can be done by migration.
