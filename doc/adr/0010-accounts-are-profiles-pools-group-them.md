# ADR-0010: An account is a profile; pools group profiles that can stand in for each other

| | |
|---|---|
| **Status** | Accepted. The requirement is the project owner's |
| **Date** | 2026-11-12 |

## Context

The project owner asked that executors can be many agents or APIs, including **several accounts of
the same agent** (two Codex subscriptions, two Antigravity logins, two API keys), while the advisor
is normally one. People keep several accounts for quota, for separating work from personal use, or
for different prices.

Constraints from earlier decisions: Hufu never stores or reads a credential
([requirements](../requirements/requirements.md), NFR-02), and everything above the adapter layer
is agent-agnostic ([ADR-0004](0004-roles-are-configuration.md)).

How agents separate accounts was checked per agent: most headless CLIs keep their login in a
configuration directory that an environment variable can relocate; API-backed agents take a key
from a variable.

## Decision

1. **There is no separate "account" object.** A profile is one agent, configured one way, with one
   account. A second account is a second profile.
2. A profile selects its account by one of two means, neither of which gives Hufu a secret:
   - `home`: the agent's own configuration and login directory. The adapter knows which variable
     that is for its agent (`CODEX_HOME`, `CLAUDE_CONFIG_DIR`, ...).
   - `env` / `env_from`: the names of variables to pass through, or to pass under another name, so
     that two keys can live in the shell as `KEY` and `KEY_TEAM`.
   `account` is a label for people.
3. **A pool** is a named list of profiles with a strategy: `failover`, `round-robin` or
   `least-busy`. A pool can be named wherever an executor profile can.
4. A job moves to the next member of its pool only when the current one fails with
   `agent_unavailable`, `auth_required` or `rate_limited`. A task the agent got wrong is not
   repeated on another account.
5. **The default advisor is one profile and cannot be a pool.** A mission's advisor keeps a
   session across planning and review; moving it between accounts mid-mission would lose that.

## Alternatives considered

| Option | Why not |
|---|---|
| An `accounts` list inside a profile | Two levels to configure and to report on, and adapters would need to know about it. A flat list of profiles is what every other part of Hufu already understands |
| Hufu stores keys or tokens and injects them | Breaks NFR-02 and makes Hufu a credential manager |
| Fail over on any failure | Burns a second account's quota on a task that fails for its own reasons, and can apply a change twice |
| Let the advisor be a pool | Needs session hand-over between accounts, which no agent offers |

## Consequences

- Listings, limits, results and logs need no new concept: each account already has a name.
  `limits.per_profile` works per account.
- The result of a job records which member actually ran.
- Logging in a second account is still the user's job, done once with the agent's own command and
  the relocated directory. `hufu agents --probe` shows whether it worked.
- An agent that has no way to relocate its login can have only one interactive account; a second
  one needs a variable its vendor documents. This is the case for Antigravity today.
- Pools make it easy to spread load over subscriptions. Whether a vendor's terms allow that is the
  user's question to answer; see the [agent landscape](../research/agent-landscape.md).
