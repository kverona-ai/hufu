# Source

All of Hufu's code lives in this directory, including its package manifest and tests. Documents
live in [`doc/`](../doc/README.md).

**There is no code yet.** The project is in its design phase (milestone M0). The first code lands
with milestone M1; see the [roadmap](../doc/roadmap.md).

## Planned layout

This directory will be the package root (`package.json` here, published to npm as `hufu`).

```text
src/
├── package.json
├── tsconfig.json
├── cli/                    command definitions, output, exit codes
├── core/
│   ├── spec/               task spec, config and result schemas; validation
│   ├── registry/           profiles, adapter lookup, capability cache
│   ├── store/              job and mission directories, atomic writes
│   ├── runner/             detached per-job runner
│   ├── scheduler/          queue, concurrency slots, dependency release
│   ├── workspace/          working directories, git worktrees, change detection
│   ├── tally/              acceptance checks
│   └── mission/            advisor loop, plans, verdicts, budgets
├── adapters/
│   ├── contract.ts         the Adapter interface
│   ├── acp/                generic Agent Client Protocol adapter
│   ├── codex-cli/
│   ├── agy-cli/
│   ├── cursor-cli/
│   ├── kimi-cli/
│   ├── pi/
│   └── claude-cli/
├── mcp/                    MCP shell (M4)
├── daemon/                 daemon (M4)
├── integrations/
│   └── claude-code/        skill and CLAUDE.md snippet
└── testing/                fake agents, recorded transcripts, conformance suite
```

## Rules for this directory

- `cli`, `mcp` and `daemon` depend on `core`. `core` depends only on `adapters/contract.ts`.
  Concrete adapters depend on the contract and on nothing in `core`.
- Every adapter passes the shared conformance suite before it is merged.
- No test in the default suite needs a real agent, a login or the network.

Design references: [technical design](../doc/technical/technical-design.md),
[adapter contract](../doc/technical/adapter-contract.md).
