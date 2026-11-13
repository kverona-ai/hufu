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
| [0006](0006-file-based-job-store.md) | Jobs are stored as plain files; a database index is deferred | Accepted |
| [0007](0007-implement-in-rust.md) | Implement in Rust; the crate is `hufu-cli`, the command is `hufu` | Accepted; library choices replaced by 0009 |
| [0008](0008-computer-use-is-a-routable-capability.md) | Computer use is a routable capability with an explicit grant | Accepted |
| [0009](0009-synchronous-core-and-own-protocol-clients.md) | A synchronous core, and protocol clients written for Hufu | Accepted |
| [0010](0010-accounts-are-profiles-pools-group-them.md) | An account is a profile; pools group profiles that can stand in for each other | Accepted |
| [0011](0011-the-daemon-is-optional.md) | The daemon is optional; limits are enforced by the runners themselves | Accepted |
| [0012](0012-connected-agents-may-configure-hufu.md) | Setup is one command, and connected agents may configure Hufu | Accepted |
| [0013](0013-self-update-with-system-tools.md) | Hufu updates itself with the system's download and archive tools, and checks the result itself | Accepted |

**Status values:** *Proposed* (written, waiting for the project owner), *Accepted*, *Superseded*.
Records 0001, 0002, 0004 and 0005 restate conclusions of the scoping discussion that started the
project. Records 0007, 0010 and 0012 follow decisions or requirements of the project owner. Records
0006 and 0008 were proposals; they became accepted when the project owner asked for the whole plan
to be implemented, and the system was built on them. Records 0009 and 0011 were made during that
implementation and change parts of earlier documents; each says which. Record 0013 was made when
`hufu update` was added at the project owner's request.

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
