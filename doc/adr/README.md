# Architecture Decision Records

Each record captures one decision: the context, what was decided, and what follows from it.
Records are numbered in the order they were written and are never deleted. A decision that is
later reversed is marked *Superseded* and links to its replacement.

| # | Decision | Status |
|---|---|---|
| [0001](0001-standalone-local-scheduler.md) | Hufu is a standalone local scheduler; the CLI is the primary interface | Accepted |
| [0002](0002-acp-first-cli-fallback.md) | ACP is the main integration path; headless CLI adapters are kept alongside it | Accepted |
| [0003](0003-typescript-on-node.md) | Implement in TypeScript on Node.js | **Proposed** |
| [0004](0004-roles-are-configuration.md) | Advisor and executor are roles assigned by configuration | Accepted |
| [0005](0005-explicit-permission-and-effort-mapping.md) | Permissions and reasoning effort are always explicit and mapped per adapter | Accepted |
| [0006](0006-file-based-job-store.md) | Jobs are stored as plain files; a database index is deferred | **Proposed** |

**Status values:** *Proposed* (written, waiting for the project owner), *Accepted*, *Superseded*.
Records 0001, 0002, 0004 and 0005 restate conclusions of the scoping discussion that started the
project. Records 0003 and 0006 are new proposals from this design pass.

## Template

```markdown
# ADR-NNNN: Title

| | |
|---|---|
| **Status** | Proposed |
| **Date** | YYYY-MM-DD |

## Context
## Decision
## Alternatives considered
## Consequences
```
