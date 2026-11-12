# Source

All of Hufu's code lives in this directory, including its manifests and tests. Documents live in
[`doc/`](../doc/README.md).

**There is no code yet.** The project is in its design phase (milestone M0). The first code lands
with milestone M1; see the [roadmap](../doc/roadmap.md).

## Planned layout

Hufu is written in Rust ([ADR-0007](../doc/adr/0007-implement-in-rust.md)). This directory will be
a Cargo workspace. The published crate is `hufu-cli`; the command it installs is `hufu`.

```text
src/
├── Cargo.toml              workspace manifest
├── Cargo.lock
├── rust-toolchain.toml
├── crates/
│   ├── hufu-cli/           the `hufu` binary: commands, output, exit codes
│   ├── hufu-core/          spec, registry, store, runner, scheduler, workspace, tally, mission
│   ├── hufu-contract/      the Adapter trait and the types shared with adapters
│   ├── hufu-adapters/      acp, codex_cli, agy_cli, cursor_cli, kimi_cli, pi, claude_cli
│   ├── hufu-mcp/           MCP shell (M4)
│   ├── hufu-daemon/        daemon (M4)
│   └── hufu-testing/       fake agents, recorded transcripts, conformance suite
└── integrations/
    └── claude-code/        skill and CLAUDE.md snippet
```

## Rules for this directory

- `hufu-core` and `hufu-adapters` each depend on `hufu-contract` and not on each other. The
  binaries depend on `hufu-core` and wire the adapters in. The crate graph enforces this.
- Every adapter passes the shared conformance suite before it is merged.
- No test in the default suite needs a real agent, a login or the network.
- `cargo fmt` and `cargo clippy` are clean on every commit.

Design references: [technical design](../doc/technical/technical-design.md),
[adapter contract](../doc/technical/adapter-contract.md).
