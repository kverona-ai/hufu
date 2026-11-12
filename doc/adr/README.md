# Architecture Decision Records

Each record captures one decision: the context, what was decided, and what follows from it.
Records are numbered in the order they were written and are never deleted. A decision that is
later reversed is marked *Superseded* and links to its replacement.

| # | Decision | Status |
|---|---|---|
| [0001](0001-standalone-local-scheduler.md) | Hufu is a standalone local scheduler; the CLI is the primary interface | Accepted |
| [0002](0002-acp-first-cli-fallback.md) | ACP is the main integration path; headless CLI adapters are kept alongside it | Accepted |
| [0003](0003-typescript-on-node.md) | Implement in TypeScript on Node.js | Superseded by 0007 |
| [0004](0004-roles-are-configuration.md) | Advisor and executor are roles assigned by configuration | Accepted |
| [0005](0005-explicit-permission-and-effort-mapping.md) | Permissions and reasoning effort are always explicit and mapped per adapter | Accepted |
| [0006](0006-file-based-job-store.md) | Jobs are stored as plain files; a database index is deferred | **Proposed** |
| [0007](0007-implement-in-rust.md) | Implement in Rust; the crate is `hufu-cli`, the command is `hufu` | Accepted |
| [0008](0008-computer-use-is-a-routable-capability.md) | Computer use is a routable capability with an explicit grant | **Proposed** |

**Status values:** *Proposed* (written, waiting for the project owner), *Accepted*, *Superseded*.
Records 0001, 0002, 0004 and 0005 restate conclusions of the scoping discussion that started the
project. Record 0007 was decided by the project owner. Records 0006 and 0008 are proposals from
the design work and still need confirmation.

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
