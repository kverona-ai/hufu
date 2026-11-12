# ADR-0008: Computer use is a routable capability with an explicit grant

| | |
|---|---|
| **Status** | **Proposed.** The requirement is the project owner's; the design below is for review |
| **Date** | 2026-11-12 |

## Context

The project owner wants computer use, meaning an agent operating a desktop or browser through its
interface, to be configurable in two ways: served by a separately configured agent, or served by
the executor's own agent. LCU was named as the reference
([study](../research/computer-use.md)).

What the study showed:

- LCU makes computer use a **runtime attached to a harness**. The harness's own model operates the
  desktop. LCU has no way to hand the work to a different agent.
- Approval has two layers, and the per-application layer can only be answered by a person. When
  nobody can answer, the request fails closed.
- On Linux the runtime's code execution is not sandboxed, and the harness's sandbox mode does not
  apply to it.

## Decision

1. **Capability, not agent type.** A profile declares what it provides beyond code tools, starting
   with `computer_use`, and how: built in, or through an attached runtime such as LCU. Hufu does
   not bundle, install or host any computer-use runtime.
2. **Routing is configuration.** `capabilities.computer_use.route` is `executor` or `dedicated`,
   set globally and overridable per task.
   - `executor`: the executor's own agent does it. Valid only if its profile provides the
     capability.
   - `dedicated`: a named profile does it on the executor's behalf. Hufu gives the executor one
     tool, and each call becomes a **child job** on the dedicated profile.
3. **Explicit grant.** A task must grant `computer_use`, with an **app scope** naming the
   applications it may touch. No permission tier implies the grant. In a mission, the advisor can
   request it only if the mission allows it.
4. **Hufu never answers a runtime's approval prompt.** Applications must be allowed by the user in
   advance. Hufu checks this before launch where the runtime lets it, and otherwise the job fails
   closed.
5. **One operator per display.** At most one computer-use job runs per display at a time.
6. **Treated as wider than `full-access`.** Worktree isolation and permission tiers do not contain
   computer use, and the documents and command output say so.

## Alternatives considered

| Alternative | Why not |
|---|---|
| Treat computer use as just another executor profile | Covers the dedicated case only when the whole task is computer use. It cannot serve a coding task that needs one step on the desktop, and it ignores the "executor's own agent" case |
| Build computer use into Hufu | Makes Hufu an agent with a model loop and a screen-control stack. A non-goal |
| Embed LCU as a library | LCU's client is JavaScript, it depends on a third-party app, and its terms are not ours to accept on a user's behalf |
| Imply the grant from `full-access` | A task that needs to edit files freely should not thereby gain the screen, the browser and the network |

## Consequences

- The adapter contract gains two capability fields: what a profile `provides`, and whether a session
  can accept an MCP server from Hufu, which the dedicated route needs.
- Jobs gain a parent link, so that child jobs are budgeted and reported under the job that asked
  for them.
- Headless adapters that run with "never ask" policies probably cannot carry a computer-use
  runtime. Which surfaces work is an open item for the milestone.
- The general mechanism (declare, route, grant) can carry other capabilities later. Only computer
  use is specified now.
- Scheduled as milestone M5, after the daemon, because the display lock and child jobs are simplest
  with a single owner of scheduling.
