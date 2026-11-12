# ADR-0009: A synchronous core, and protocol clients written for Hufu

| | |
|---|---|
| **Status** | Accepted. Replaces the library choices listed under "Decision" in [ADR-0007](0007-implement-in-rust.md); the choice of Rust stands |
| **Date** | 2026-11-12 |

## Context

[ADR-0007](0007-implement-in-rust.md) chose Rust and, with it, named a stack: Tokio, the official
`agent-client-protocol` and `rmcp` crates, `schemars`, and a pseudo-terminal crate if needed. That
list was written before any code. When the whole plan was implemented in one step, for Linux,
macOS and Windows, each item was looked at again against what Hufu actually does:

- A runner drives **one** agent process: it writes a prompt, reads two pipes line by line, watches
  a clock and a cancel marker. A mission driver waits for a handful of jobs. Nothing in Hufu
  serves many connections.
- Hufu uses a small part of each protocol. Of ACP: the handshake, one session, one prompt,
  permission answers, cancel. Of MCP: the handshake, a tool list and tool calls over stdio.
- The agents on the other end of ACP differ in small ways (which optional methods exist, how
  answers are chunked, what an auth error looks like). A client has to be tolerant of that.
- Every dependency has to build on three operating systems and two architectures, and into a
  static Linux binary.

## Decision

1. **No async runtime.** Hufu uses threads and blocking I/O from the standard library: one thread
   per pipe, a watchdog thread per runner, channels between them.
2. **The ACP client and the MCP server are written for Hufu**, on `serde_json`, as newline-delimited
   JSON-RPC 2.0. Each is a few hundred lines.
3. **JSON Schema validation is written for Hufu**, covering the subset used for structured output
   and plans. Schemas for Hufu's own formats are not generated.
4. **The dependency list is short and fixed:** `serde`, `serde_json`, `serde-saphyr` (YAML),
   `clap`, `getrandom`, and `libc` or `windows-sys` for process control.
5. **No pseudo-terminal.** If an agent prints nothing without a terminal, the job fails with
   `empty_output` and the answer is to upgrade that agent.

## Alternatives considered

| Option | Why not |
|---|---|
| Tokio with the official SDK crates, as first listed | Brings a runtime and a large dependency tree for a program that waits on one child process. The SDKs cover far more of each protocol than Hufu uses and move quickly: both published several major versions within a year. An agent that deviates slightly is harder to accommodate from behind a strict typed layer |
| Tokio with hand-written protocol code | Keeps the runtime cost without the benefit of the SDKs |
| Official SDKs behind a blocking wrapper | Two concurrency models in one small program |

## Consequences

- The binary is about 4 MB, builds in well under a minute, and links statically on Linux with the
  linker that ships with Rust, which keeps the release workflow simple.
- Process handling is plain enough to reason about per platform; all of it sits in one module.
- Hufu owns its protocol code. A new protocol revision is work for Hufu rather than a version bump.
  The parts in use are small and are covered by end-to-end tests with a client and a fake agent
  that live in the repository. The MCP server accepts several protocol revisions for that reason.
- The protocol code is tolerant by intent: unknown notifications are ignored, optional methods are
  tried and skipped. That favours working with many agents over strict conformance checking.
- If Hufu ever serves many sessions from one process, for example remote executors, this decision
  should be revisited.
