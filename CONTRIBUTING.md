# Contributing to Hufu

Thank you for your interest. Hufu 0.1 is implemented and tested against built-in fake agents. The
most useful contribution right now is a report from a run against a real agent: what you ran
(`hufu run ... --dry-run` shows the exact command line), what the agent printed
(`hufu logs <job> --raw`), and what Hufu made of it (`hufu --json result <job>`). Remove anything
private from the output first.

## Where things go

| Kind of change | Location |
|---|---|
| Documents of any kind | [`doc/`](doc/README.md), in the matching category folder, and listed in `doc/README.md` |
| Code, tests, package manifests, install scripts | [`src/`](src/README.md) |
| CI and release workflows | `.github/workflows/` |
| Sample configuration and specs | `doc/examples/` |
| Images | `doc/assets/` |

The repository root holds only the READMEs, the licence and repository metadata. That includes
`.github/`, because GitHub reads workflows from nowhere else, and `AGENTS.md` and `llms.txt`,
because agents look for them in the root. Both only point to documents under `doc/`; keep them in
step with the [agent install guide](doc/guide/agent-install.md).

## Proposing a change

1. Open an issue describing the problem before a large change.
2. Branch from `main` and keep each pull request to one topic.
3. A change that alters a decision recorded under [`doc/adr/`](doc/adr/README.md) adds a new record
   that supersedes the old one. Records are not edited after they are accepted.
4. Update both `README.md` and `README.zh-CN.md` when the README changes.

## Writing documents

- English, plain and specific. Define a term the first time it is used.
- Start each document with the status table used by the existing ones.
- A claim about a third-party tool needs a source and an evidence mark; the marks are defined in
  [`doc/research/agent-landscape.md`](doc/research/agent-landscape.md).
- Diagrams are Mermaid blocks inside the Markdown file.
- Check that relative links resolve before opening a pull request.

## Writing code

- Hufu is written in Rust. Build and test commands, the layout and the crate dependency rules are in
  [`src/README.md`](src/README.md). Keep `cargo fmt --all --check` and
  `cargo clippy --all-targets -- -D warnings` clean.
- A new adapter must pass the conformance suite described in the
  [adapter contract](doc/technical/adapter-contract.md#6-conformance-suite), and comes with a fake
  agent that speaks the real agent's output format.
- A change in behaviour comes with an end-to-end test under `src/crates/hufu-cli/tests/e2e/`.
- Tests must not need a real agent, a login or the network, and must pass on Linux, macOS and
  Windows. CI runs all three.
- Never read, copy or log an agent's stored credentials.
- A change that users can see updates the [user guide](doc/guide/user-guide.md) and, where it
  applies, both READMEs, in the same pull request.

## Commit messages

Use a short imperative subject with a type prefix, for example `docs: add adapter contract` or
`feat(adapters): add codex-cli adapter`.

## Licence

By contributing you agree that your contributions are licensed under the
[Apache License 2.0](LICENSE).
