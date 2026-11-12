# ADR-0012: Setup is one command, and connected agents may configure Hufu

| | |
|---|---|
| **Status** | Accepted. The requirement is the project owner's |
| **Date** | 2026-11-12 |

## Context

The project owner asked for the tool to be easy to adopt: one command to start, one command to
configure, installable into any agent, and configurable both from the command line and "by MCP, so
that any AI can configure Hufu".

Hufu's configuration decides which programs run, with which permissions and under which accounts.
Letting an agent edit it is a real grant of power, so it needed a decision rather than a default.

## Decision

1. **`hufu up` is the whole setup.** It detects installed agents, writes a first configuration with
   default roles, registers Hufu with those agents, starts the daemon and reports what is missing.
   It is safe to repeat.
2. **`hufu setup` writes into other agents' settings**, but only the one entry that registers the
   `hufu` MCP server: in place, leaving everything else alone, with a one-time backup, a preview
   (`--print`) and a way back (`--remove`). Where an agent has its own command for this, Hufu uses
   that command instead of editing the file.
3. **The MCP server exposes configuration tools** next to the dispatch tools: read and set any
   key, add and remove profiles and pools, write the first configuration, register with hosts.
4. **Every edit goes through one path**, shared by the CLI and MCP: apply the change to the
   document, validate the merged result as a whole, write atomically only if it is valid.
5. **Three things cannot be configured by anyone**, agent or person: `full-access` as the global
   default tier; a value that looks like a secret; a computer-use agent that does not declare the
   capability.

## Alternatives considered

| Option | Why not |
|---|---|
| Configuration tools off by default, enabled by a flag | The user would have to edit configuration by hand to allow configuration by conversation, which defeats the request. The host agent already runs with the user's shell rights in most setups, so the flag would protect little |
| Read-only configuration over MCP | Does not meet the requirement |
| `hufu setup` prints instructions only | That is `--print`. The request was for one command |
| Ask for confirmation on each MCP edit | MCP has no dependable way to ask a person, and an unattended host cannot answer |

## Consequences

- A host agent connected to Hufu can add a profile that runs an arbitrary program, by setting
  `command`. That is equivalent to what the same agent can usually do with a shell. Hosts that are
  not trusted that far should not be given the `hufu` server; the documents say so.
- Tier safety does not rest on the configuration being hard to change: `full-access` still has to
  be asked for per task or opted into per profile, and tiers are still validated per agent.
- Hufu now depends on where five agents keep their MCP settings. Those locations are in one module
  and each has a test; `--agent generic --print` is the fallback when one moves.
- Comments in a hand-written configuration file are lost when the file is edited by command.
