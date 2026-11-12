# Cloud Development Environment

| | |
|---|---|
| **Status** | Assessment and recommendation |
| **Assessed on** | 2026-11-12 (updated after Hufu 0.1 was implemented) |
| **Related** | [Deployment](deployment.md), [Roadmap](../roadmap.md) |

This repository is developed partly from Claude Code cloud sessions. Each session runs in a
container built from a saved **environment**: a network policy, environment variables and a setup
script. This document records what the project needs from that environment and whether it should
have its own instead of sharing `Default`.

## 1. Current state

| Setting | `Default` environment today |
|---|---|
| Scope | Personal; shared by every repository that does not name another environment |
| Network | Limited. Package managers allowed; no additional allowed domains |
| Environment variables | None |
| Setup script | None |
| Tooling found in the container | Rust 1.97 (rustc and cargo), Node.js 22.23, npm, pnpm, Go, Python 3, Git, Chromium with Playwright |

Other projects in the same account already have their own environments. `hufu` does not.

## 2. What each phase needs

| Phase | Needs from the environment | Does `Default` cover it? |
|---|---|---|
| **Documentation** | Git and Markdown only | **Yes.** Vendor documentation sites are blocked by the limited network, but research can go through web search and fetch tools, which do not depend on the container's network |
| **Building Hufu** | A Rust toolchain (present); crates fetched and ideally a first build done before the session starts; crates.io access (allowed as a package manager) | **Works, and is how Hufu 0.1 was built.** Every session downloads and compiles the dependencies from scratch: a first build and test run takes a few minutes, because the dependency list is short. A setup script would remove that wait, and a repository's setup script does not belong in a shared environment |
| **Fake-agent and conformance tests** | Nothing beyond the above. The fake agents are part of the `hufu` binary | Yes |
| **macOS and Windows** | Runners for those systems | **Not in a cloud container**, which is Linux. GitHub Actions runs the full test suite on the three systems for every pull request; the container can only type-check and lint the other targets |
| **Live tests with API-key agents** (pi with OpenRouter or DeepSeek, Cursor or Codex with an API key) | Provider API keys as secrets; the providers' API hosts on the allow-list; the agent CLIs installed | **No.** `Default` has no secrets and allows no extra hosts, and these should not be added to an environment every repository shares |
| **Live tests with subscription logins** (`agy`, Kimi Code, Codex or Claude on a subscription) | An interactive browser login stored in the user's home directory | **Not possible in any cloud container.** These run on the developer's own machine |
| **Computer-use tests** | A real desktop session, a computer-use runtime and applications approved by a person | **Not possible in a cloud container.** These run on a supported machine with someone present |

## 3. Recommendation

**Optional so far; needed for live tests with API keys.** Hufu 0.1 was built and tested in
`Default` without changing it. The `hufu` environment below has not been created.

- For building and for the fake-agent tests, `Default` is enough.
- A dedicated environment is worth having for three reasons:
  1. **Setup script.** Installing dependencies at container start is specific to this repository.
     `Default` is shared, so a repository's setup script should never be put there.
  2. **Secrets and hosts.** API keys and provider hosts for live tests should be visible to this
     project only.
  3. **Consistency.** The account already keeps one environment per project.
- Do not edit `Default` for this project.

### Proposed `hufu` environment

| Setting | Value | When |
|---|---|---|
| Name | `hufu` | When the environment is created |
| Network | Limited, package managers allowed | From the start |
| Setup script | See below | From the start |
| Allowed domains | None at first. Add provider API hosts only when live tests for that provider are written, for example `openrouter.ai`, `api.deepseek.com` | With the first live test |
| Secrets | Provider API keys for live tests, for example an OpenRouter key | With the first live test |
| Environment variables | `HUFU_HOME=/tmp/hufu-home` so tests never touch a real home directory; `CARGO_TERM_COLOR=never` for readable logs | From the start |

Setup script sketch:

```bash
#!/usr/bin/env bash
set -euo pipefail
cd "$(git rev-parse --show-toplevel)/src"
if [ -f Cargo.lock ]; then
  cargo fetch --locked
  cargo build --workspace --all-targets --locked   # warm the build cache
fi
```

Notes:

- A setup script cannot read the environment's saved variables or secrets, so it must not depend on
  API keys. Agent CLIs needed for live tests are installed in the test job itself.
- Hosts are added one at a time, each tied to a test that needs it. A broad allow-list is not
  wanted here.

## 4. Where live agent tests run

The agents that matter most to Hufu authenticate with subscription logins. Those logins live on the
developer's machine and cannot be moved. So:

| Test kind | Where |
|---|---|
| Unit, fake-agent, conformance, durability | Cloud sessions and CI |
| Live tests with API keys | Cloud sessions in the `hufu` environment, or CI with repository secrets |
| Live tests with subscription logins | The developer's own machine, through a local session or a remote-control session from the cloud thread |

This split is also why the test strategy puts most of the weight on fake agents: they are the only
tests that run everywhere.

One caution for cloud sessions: the container has a real `claude` program on its `PATH`. `hufu
setup` and `hufu up` register Hufu with the hosts they find, so in a session run them with
`--print`, `--project` or `--no-setup`.

## 5. Revisit when

- Session start-up time becomes a nuisance: create the environment and its setup script.
- The first live test against an API-key agent is written: add that provider's host and secret.
- A vendor documentation host is needed inside the container for an automated check: add that one
  host, not a wider policy.
