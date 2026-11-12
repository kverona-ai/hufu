# ADR-0004: Advisor and executor are roles assigned by configuration

| | |
|---|---|
| **Status** | Accepted |
| **Date** | 2026-11-12 |

## Context

The first design had a fixed shape: Claude Code plans, other agents execute. The project owner
then asked for one more level of abstraction: *any* agent or model should be able to act as the
advisor that dispatches the others.

Hard-coding a commander would make that impossible. Writing a separate "advisor integration" per
agent would double the adapter work.

## Decision

There is one kind of thing, an **agent profile**, resolved through one adapter layer. A profile has
no inherent role. A **mission** assigns roles:

- one profile is the **advisor**: it receives the goal and the roster, returns a plan, and reviews
  results;
- one or more profiles are **executors**: they run the tasks.

The same profile can advise one mission and execute in another. Nothing in an adapter knows which
role it is playing.

Two ways to fill the advisor role share the same core:

- **External advisor.** A host agent in the user's own session calls the Hufu CLI. Available from
  the first milestone.
- **Managed advisor.** Hufu runs the advisor profile itself and drives the plan, dispatch and
  review loop. Available from M3.

The contract between advisor and executor is the task spec with its acceptance criteria, checked
by the tally check. That contract is what the project's name refers to.

## Alternatives considered

| Alternative | Why not |
|---|---|
| Fixed commander (Claude Code) | Does not meet the requirement |
| A built-in planner model inside Hufu | Makes Hufu an agent, with prompts and model credentials of its own; a non-goal |
| Separate advisor plug-ins | Duplicates every adapter |

## Consequences

- The managed advisor needs only two things from an agent: answer a prompt, and produce JSON.
  Agents without schema-constrained output are asked for a fenced JSON block, with one repair
  attempt.
- Models on open APIs can advise, because the pi harness turns a model into an agent profile.
- Advisors run at the `read-only` tier by default. An advisor cannot grant an executor more than
  the mission's configured maximum tier.
- Plans come from a model and must be validated like untrusted input: schema, roster membership,
  tier ceiling, acyclic dependencies, and an allow-list for acceptance commands.
