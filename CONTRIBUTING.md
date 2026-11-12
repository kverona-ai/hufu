# Contributing to Hufu

Thank you for your interest. Hufu is in its design phase: the documents under [`doc/`](doc/README.md)
are the product right now, and feedback on them is the most useful contribution.

## Where things go

| Kind of change | Location |
|---|---|
| Documents of any kind | [`doc/`](doc/README.md), in the matching category folder, and listed in `doc/README.md` |
| Code, tests, package manifests | [`src/`](src/README.md) |
| Sample configuration and specs | `doc/examples/` |
| Images | `doc/assets/` |

The repository root holds only the READMEs, the licence and repository metadata.

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

## Writing code (from M1)

- Hufu is written in Rust. Follow the crate dependency rules in [`src/README.md`](src/README.md),
  and keep `cargo fmt` and `cargo clippy` clean.
- A new adapter must pass the conformance suite described in the
  [adapter contract](doc/technical/adapter-contract.md#6-conformance-suite).
- Tests in the default suite must not need a real agent, a login or the network.
- Never read, copy or log an agent's stored credentials.

## Commit messages

Use a short imperative subject with a type prefix, for example `docs: add adapter contract` or
`feat(adapters): add codex-cli adapter`.

## Licence

By contributing you agree that your contributions are licensed under the
[Apache License 2.0](LICENSE).
