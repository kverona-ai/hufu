# Hufu Documentation

All project documents live in this directory. Code lives in [`src/`](../src/README.md).

## Start here

| If you want to know… | Read |
|---|---|
| What Hufu is for and what it must do | [Requirements](requirements/requirements.md) |
| How the system is put together | [Architecture](architecture/architecture.md) |
| What is built when | [Roadmap](roadmap.md) |

## Map

```text
doc/
├── README.md                     this index
├── roadmap.md                    milestones M0 to M4
├── requirements/
│   └── requirements.md           goals, users, scenarios, FR and NFR lists, open questions
├── research/
│   ├── agent-landscape.md        how each agent is driven headlessly; ACP; verified facts
│   └── prior-art.md              existing orchestrators, positioning, the name
├── architecture/
│   └── architecture.md           components, roles, job and mission lifecycles, security
├── technical/
│   ├── technical-design.md       stack, source layout, config, task spec, CLI, store, missions, tests
│   └── adapter-contract.md       adapter interface, permission and effort mapping, per-agent notes
├── deployment/
│   ├── deployment.md             install, configure, connect hosts, operate, troubleshoot
│   └── cloud-environment.md      what cloud development sessions need
├── adr/                          architecture decision records
├── examples/                     sample configuration, task and mission files
└── assets/                       images used by the README and documents
```

## By category

### Requirements
- [Requirements](requirements/requirements.md)

### Research
- [Agent landscape](research/agent-landscape.md): what Codex, Cursor, Antigravity, Kimi Code, pi and
  others offer for non-interactive use, with the evidence level of each claim
- [Prior art, positioning and naming](research/prior-art.md)

### Architecture
- [Architecture](architecture/architecture.md)
- [Decision records](adr/README.md)

### Technical design
- [Technical design](technical/technical-design.md)
- [Adapter contract](technical/adapter-contract.md)
- [Examples](examples/): [configuration](examples/config.yaml), [task](examples/task.yaml),
  [mission](examples/mission.yaml)

### Deployment
- [Deployment and operations](deployment/deployment.md)
- [Cloud development environment](deployment/cloud-environment.md)

## Conventions

- **Language.** Documents are written in English. The README also has a
  [Chinese version](../README.zh-CN.md).
- **Status header.** Every document starts with a small table giving its status and the date it was
  last updated.
- **Evidence.** Claims about third-party tools carry an evidence mark and a source. The marks are
  defined in the [agent landscape](research/agent-landscape.md).
- **Diagrams** are written in Mermaid inside the Markdown files, so they render on GitHub and are
  reviewed as text.
- **Decisions** that shape the system get a record under [`adr/`](adr/README.md). Documents link to
  the record instead of repeating the argument.
- **New documents** go into the matching category folder and are added to this index.
