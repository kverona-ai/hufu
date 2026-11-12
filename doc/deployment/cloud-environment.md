# Cloud Development Environment

| | |
|---|---|
| **Status** | Assessment and recommendation |
| **Assessed on** | 2026-11-12 |
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
| Tooling found in the container | Node.js 22.23, npm 10.9, pnpm, Bun, Go, Rust (cargo), Python 3, Git, Chromium with Playwright |

Other projects in the same account already have their own environments. `hufu` does not.

## 2. What each phase needs

| Phase | Needs from the environment | Does `Default` cover it? |
|---|---|---|
| **M0: documentation** (now) | Git and Markdown only | **Yes.** Vendor documentation sites are blocked by the limited network, but research can go through web search and fetch tools, which do not depend on the container's network |
| **M1 to M4: building Hufu** | Node.js ≥ 22.19 and pnpm (both present); dependencies installed before the session starts; npm registry access (allowed) | Works, but every session would reinstall dependencies by hand. A setup script fixes that, and a repository's setup script does not belong in a shared environment |
| **Fake-agent and conformance tests** | Nothing beyond the above. They run against scripted fakes | Yes |
| **Live tests with API-key agents** (pi with OpenRouter or DeepSeek, Cursor or Codex with an API key) | Provider API keys as secrets; the providers' API hosts on the allow-list; the agent CLIs installed | **No.** `Default` has no secrets and allows no extra hosts, and these should not be added to an environment every repository shares |
| **Live tests with subscription logins** (`agy`, Kimi Code, Codex or Claude on a subscription) | An interactive browser login stored in the user's home directory | **Not possible in any cloud container.** These run on the developer's own machine |

## 3. Recommendation

**Not needed yet. Create a dedicated `hufu` environment when M1 starts.**

- While the repository holds only documentation, `Default` is enough, and changing nothing is the
  right call.
- Once code lands, a dedicated environment is worth having for three reasons:
  1. **Setup script.** Installing dependencies at container start is specific to this repository.
     `Default` is shared, so a repository's setup script should never be put there.
  2. **Secrets and hosts.** API keys and provider hosts for live tests should be visible to this
     project only.
  3. **Consistency.** The account already keeps one environment per project.
- Do not edit `Default` for this project.

### Proposed `hufu` environment (for M1)

| Setting | Value | When |
|---|---|---|
| Name | `hufu` | M1 |
| Network | Limited, package managers allowed | M1 |
| Setup script | See below | M1 |
| Allowed domains | None at first. Add provider API hosts only when live tests for that provider are written, for example `openrouter.ai`, `api.deepseek.com` | M2 |
| Secrets | Provider API keys for live tests, for example an OpenRouter key | M2 |
| Environment variables | `HUFU_HOME=/tmp/hufu-home` so tests never touch a real home directory | M1 |

Setup script sketch:

```bash
#!/usr/bin/env bash
set -euo pipefail
cd "$(git rev-parse --show-toplevel)/src"
corepack enable
if [ -f pnpm-lock.yaml ]; then
  pnpm install --frozen-lockfile
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

This split is also why the test strategy puts most of the weight on fake agents with recorded
transcripts: they are the only tests that run everywhere.

## 5. Revisit when

- The first `package.json` lands under `src/`: create the environment and its setup script.
- The first live test against an API-key agent is written: add that provider's host and secret.
- A vendor documentation host is needed inside the container for an automated check: add that one
  host, not a wider policy.
