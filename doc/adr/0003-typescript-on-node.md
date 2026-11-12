# ADR-0003: Implement in TypeScript on Node.js

| | |
|---|---|
| **Status** | **Superseded** by [ADR-0007](0007-implement-in-rust.md) before it was accepted. Kept for the comparison of options |
| **Date** | 2026-11-12 |

## Context

Hufu is a local command-line tool that spawns subprocesses, speaks JSON-RPC over pipes, parses
JSON streams and writes files. It will also be an ACP client and, later, an MCP server. No language
was fixed during scoping.

Facts that bear on the choice (checked 2026-11-12):

- The reference ACP SDK is TypeScript (`@agentclientprotocol/sdk`, 2.6.0, Apache-2.0). Rust and
  Python SDKs are also official.
- The MCP TypeScript SDK (`@modelcontextprotocol/sdk`) is the most widely used implementation.
- Most target agents are themselves distributed through npm or require Node.js: Codex, pi, the
  Claude, Codex and pi ACP adapters. A user of Hufu almost certainly has Node.js installed.
- The package name `hufu` is free on npm and already taken on PyPI and crates.io
  ([prior art](../research/prior-art.md#42-name-availability)).

## Decision

Implement Hufu in **TypeScript (strict), targeting Node.js ≥ 22.19**, published to npm as `hufu`.

## Alternatives considered

| Option | For | Against |
|---|---|---|
| **TypeScript / Node.js** | Both protocol SDKs first-class; same runtime users already have; fast iteration; free package name | Needs a runtime; process and signal handling needs care; a PTY, if ever needed, is a native add-on |
| **Rust** | Single static binary; strong process control; official ACP SDK | Slower to iterate while the design is still moving; `hufu` is taken on crates.io; contributors less likely |
| **Go** | Single binary; simple concurrency; used by the closest prior art (Sortie) | No official ACP SDK found; MCP SDK less mature |
| **Python** | Official ACP SDK; quick to write | Packaging and startup time are weak for a CLI called many times per session; `hufu` is taken on PyPI |

## Consequences

- Hufu's runtime floor is Node.js 22.19, set by pi, the strictest target.
- Start-up cost of a Node.js CLI matters because agents call `hufu` often. Commands must load only
  what they need; submission should return in well under a second
  ([requirements](../requirements/requirements.md), NFR-08).
- Subprocess handling needs explicit care: detached process groups, signal forwarding and no
  inherited terminals. This is covered by the durability tests.
- If a single-binary distribution is wanted later, the same code can be compiled with Bun or
  Node.js single-executable packaging without a rewrite.
- **If this proposal is rejected**, only this record, the stack tables in the architecture and
  technical design, and the layout under `src/` change. The contracts, formats and adapter rules
  are language-neutral.
