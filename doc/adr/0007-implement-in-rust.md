# ADR-0007: Implement in Rust

| | |
|---|---|
| **Status** | Accepted. Supersedes the proposal in [ADR-0003](0003-typescript-on-node.md) |
| **Date** | 2026-11-12 |
| **Decided by** | Project owner |

## Context

[ADR-0003](0003-typescript-on-node.md) proposed TypeScript on Node.js and listed Rust as the main
alternative. The project owner asked for Rust.

Facts checked on 2026-11-12:

- ACP has an official Rust SDK, the `agent-client-protocol` crate (5.2.0, updated 2026-11-10), in
  the protocol's own GitHub organisation.
- MCP has an official Rust SDK, the `rmcp` crate (3.9.0).
- The crate name `hufu` is taken on crates.io by an unrelated project. `hufu-cli` is free. The npm
  name `hufu` is also free.
- Hufu's hard problems are process problems: jobs that outlive their caller, process groups,
  signals, timeouts, and possibly a pseudo-terminal for one agent.

## Decision

Implement Hufu in **Rust** (stable toolchain, edition 2024), as a Cargo workspace under `src/`.

- The published crate is **`hufu-cli`**. The installed command is **`hufu`**.
- Tokio for async I/O and subprocesses; `serde` for all formats; `schemars` for JSON Schema;
  `clap` for the command line; the official `agent-client-protocol` and `rmcp` crates for the two
  protocols.
- Releases are prebuilt binaries per platform. `cargo install hufu-cli` builds from source.

## Why Rust fits

| Concern | Effect of choosing Rust |
|---|---|
| Distribution | One static binary. Users need no runtime, and Hufu does not depend on the Node.js version an agent happens to require |
| Start-up time | A few milliseconds. Agents call `hufu` many times per session, so this is felt directly |
| Process control | Direct access to sessions, process groups and signals; a maintained cross-platform PTY crate if the `agy` adapter needs one |
| Long-running daemon (M4) | Low, steady memory use |
| Layering | The rule "adapters must not depend on the core" becomes a crate boundary the compiler enforces |
| Protocol support | Both protocol SDKs are official |

## Costs accepted

- Iteration is slower than in a scripting language while formats are still moving. Mitigation: the
  formats are specified in the documents first, and adapters are developed against recorded
  transcripts.
- Parsing several vendors' loosely specified JSON streams needs tolerant types. Unknown fields are
  ignored and unknown event types are logged, never fatal.
- Fewer casual contributors can write an adapter. Mitigation: the adapter contract is small, and
  the generic ACP adapter means most agents need configuration, not code.
- Release engineering is heavier: cross-compiled binaries for macOS and Linux on two architectures.
  A release tool such as `cargo-dist` is the candidate for this.

## Consequences

- `src/` is a Cargo workspace: `hufu-cli`, `hufu-core`, `hufu-contract`, `hufu-adapters`,
  `hufu-testing`, and later `hufu-mcp` and `hufu-daemon`
  ([technical design, section 2](../technical/technical-design.md#2-source-layout)).
- Node.js is no longer a requirement of Hufu. Individual agents may still need it.
- The crate name `hufu-cli` should be reserved on crates.io, and `hufu` on npm for an optional
  installer package, before the first public release.
- The formats, contracts and adapter rules in the other documents are unchanged; they were written
  to be language-neutral.
