# Source

All of Hufu's code lives in this directory, including its manifests, tests, install scripts and
agent integrations. Documents live in [`doc/`](../doc/README.md).

Hufu is written in Rust ([ADR-0007](../doc/adr/0007-implement-in-rust.md)). This directory is a
Cargo workspace. The package that holds the binary is `hufu-cli`; the command it installs is `hufu`.

## Build, test, run

```bash
cd src
cargo build                     # debug build: target/debug/hufu
cargo test                      # unit tests and the end-to-end suite
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo run -p hufu-cli -- doctor --self-test
cargo build --release -p hufu-cli
```

Rust 1.89 or newer. No other tool is needed: the tests bring their own agents (see below), and
`git` is the only program they call.

## Layout

```text
src/
├── Cargo.toml              workspace manifest: versions, dependencies, lints, release profile
├── Cargo.lock
├── rustfmt.toml
├── crates/
│   ├── hufu-contract/      the Adapter trait and everything adapters share: types, events,
│   │                       process handling, time, JSON helpers. Depends on nothing of Hufu's
│   ├── hufu-adapters/      codex_cli, agy_cli, cursor_cli, claude_cli, kimi_cli, pi, acp, external
│   ├── hufu-core/          config, spec, submit, runner, slots, store, workspace, tally, schema,
│   │                       capability (computer-use routing), mission, setup, doctor, ops
│   ├── hufu-mcp/           the MCP server: protocol loop and tools
│   ├── hufu-daemon/        the optional daemon
│   ├── hufu-testing/       fake agents (one per adapter) and the adapter conformance suite
│   └── hufu-cli/           the `hufu` binary: argument parsing, commands, rendering, exit codes
│       └── tests/e2e/      end-to-end tests that drive the built binary
├── install/                install.sh (macOS, Linux) and install.ps1 (Windows)
└── integrations/
    └── claude-code/        the skill and the CLAUDE.md snippet that `hufu setup` installs
```

## Rules for this directory

- `hufu-core` and `hufu-adapters` each depend on `hufu-contract` and not on each other. The binary
  wires the adapters in. The crate graph enforces this.
- Every adapter passes the shared conformance suite (`hufu-testing::conformance`, checks C1 to C12).
- No test needs a real agent, a login or the network. The binary contains fake agents
  (`hufu __fake-agent <flavor>`) that speak each real agent's output format, and the end-to-end
  tests point profiles at them. A prompt can carry a directive such as `[[fake:slow=1500]]` or
  `[[fake:deny]]` to make the fake misbehave in a chosen way.
- Synchronous code and threads; no async runtime. Few dependencies: `serde`, `serde_json`,
  `serde-saphyr`, `clap`, `getrandom`, and `libc` or `windows-sys` for process control
  ([ADR-0009](../doc/adr/0009-synchronous-core-and-own-protocol-clients.md)).
- Platform differences live in `hufu-contract/src/proc.rs`. Code elsewhere does not use `cfg`
  for operating systems unless it has to.
- `cargo fmt` and `cargo clippy -- -D warnings` are clean on every commit. CI checks both, runs the
  tests on Linux, macOS and Windows, and compiles with the minimum supported Rust version.

Design references: [technical design](../doc/technical/technical-design.md),
[adapter contract](../doc/technical/adapter-contract.md).
