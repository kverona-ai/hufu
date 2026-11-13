# Installing Hufu: a guide for agents

| | |
|---|---|
| **Status** | Describes Hufu 0.1 as implemented. No release has been tagged yet, so the install script builds from source until the first tag |
| **Last updated** | 2026-11-13 (first version) |
| **Audience** | A coding agent (Claude Code, Codex, Cursor, Kimi Code, Antigravity or any other agent with a shell) whose user asked it to install or configure Hufu |
| **Related** | [User guide](user-guide.md), [Deployment](../deployment/deployment.md), [sample configuration](../examples/config.yaml) |

Letting an agent do the installation is the main way to install Hufu. The user says "install Hufu"
and gives this page; the agent does the rest and reports back. Everything here is written for the
agent, as steps it can run and outputs it can check. A person can follow the same steps by hand.

**What you are installing.** Hufu is one self-contained program, `hufu`, that runs on the user's own
machine. It lets one coding agent hand work to other coding agents as background jobs, and lets any
agent that speaks MCP drive it. It has no server and no account of its own.

## Contents

- [1. Who does what](#1-who-does-what)
- [2. Check the machine](#2-check-the-machine)
- [3. Install and set up](#3-install-and-set-up)
- [4. Read the result](#4-read-the-result)
- [5. Connect the host agents](#5-connect-the-host-agents)
- [6. Configure](#6-configure)
- [7. Prove that it works](#7-prove-that-it-works)
- [8. Report to the user](#8-report-to-the-user)
- [9. Environment variables](#9-environment-variables)
- [10. Upgrade and uninstall](#10-upgrade-and-uninstall)
- [11. When something goes wrong](#11-when-something-goes-wrong)

## 1. Who does what

| You, the agent | The user |
|---|---|
| Check the machine | Install the agent programs they want Hufu to drive |
| Run the installer and `hufu up` | Log in to each of those agents once |
| Read `hufu doctor` and act on it | Export API keys in their own shell profile |
| Configure profiles, pools and roles | Restart their agent so that it sees the new tools |
| Report what changed and what is still missing | |

Rules that hold for every step:

1. **Never log an agent in for the user**, and never read, copy or print an agent's stored
   credentials. When a login is missing, tell the user which command to run.
2. **Never write a secret value** into a file or onto a command line. Hufu's configuration holds
   the **names** of environment variables; the values stay in the user's shell. Hufu refuses a
   setting that looks like a secret.
3. **No `sudo` and no administrator rights.** Everything installs under the user's home directory.
4. **Do not widen permissions.** Leave `defaults.permission` at `read-only`. Give a profile
   `full-access` only when the user asks for it in words.
5. **Do not spend the user's quota to test.** `hufu doctor --self-test` needs no account. Run a
   real job only when the user agrees.
6. **Do not work around a sandbox.** If your shell has no network or cannot write outside the
   workspace, ask the user to approve the command or to run it themselves.

## 2. Check the machine

| Requirement | How to check | If it is missing |
|---|---|---|
| Linux, macOS or Windows on x86-64 or ARM64 | `uname -sm`; on Windows `$env:PROCESSOR_ARCHITECTURE` | Stop: other platforms are not supported |
| At least one agent program, installed and logged in | See the table below | Stop and ask the user to install and log in to one. Hufu installs no agents and logs in for nobody |
| `curl` or `wget` (macOS, Linux) | `command -v curl wget` | Ask the user |
| Rust 1.89 or newer, **only while no release is tagged** | `cargo --version` | Tell the user that Hufu is built from source for now and that this needs Rust from <https://rustup.rs>. Install it only if they agree |
| Git, optional | `git --version` | Hufu works without it, but worktree isolation needs it |

The agents Hufu can drive, and the program each one puts on `PATH`:

| Agent | Program | The user logs in with |
|---|---|---|
| Codex | `codex` | `codex login` |
| Antigravity | `agy` | running `agy` once, interactively |
| Cursor | `agent` | `agent login`, or `CURSOR_API_KEY` |
| Kimi Code | `kimi` | `kimi login` |
| Claude Code | `claude` | running `claude` once, interactively |
| pi (OpenRouter, DeepSeek, Z.ai, Kimi and other model APIs) | `pi` | the provider's API key in the environment |

```bash
command -v codex agy agent kimi claude pi
```

You are probably one of these yourself, and that counts. An agent that runs only inside an editor,
with no program of its own on `PATH`, can still **call** Hufu over MCP, but Hufu needs at least one
of the programs above to hand work to.

## 3. Install and set up

One command installs the program and sets everything up. Building from source takes a minute or
more, so give the command a timeout of at least ten minutes.

```bash
# macOS and Linux
curl -fsSL https://raw.githubusercontent.com/kverona-ai/hufu/main/src/install/install.sh | sh -s -- --up
```

```powershell
# Windows: run this in PowerShell. If your shell tool is cmd or bash, start `powershell` first.
$env:HUFU_UP = "1"; irm https://raw.githubusercontent.com/kverona-ai/hufu/main/src/install/install.ps1 | iex
```

What the installer does:

1. Downloads the release archive for this platform and its SHA-256 file, and verifies the sum.
2. If no archive can be downloaded, which is the case until the first release is tagged, it prints
   `No prebuilt binary could be downloaded for this machine` and builds with `cargo install`
   instead. That needs Rust 1.89 or newer.
3. Puts one program on the machine: `~/.local/bin/hufu`, or `%LOCALAPPDATA%\hufu\bin\hufu.exe` on
   Windows. On Windows it also adds that directory to the user `PATH`.
4. With `--up` (or `HUFU_UP=1`), runs `hufu up`.

What `hufu up` does. Each step is also a command of its own, and running `hufu up` again changes
nothing that is already in place:

| Step | Same as | Result |
|---|---|---|
| Configuration | `hufu init` | `~/.hufu/config.yaml` with one profile per agent found, a default advisor and default executors. An existing file is kept |
| Host registration | `hufu setup` | Hufu's MCP server is registered with every agent found on the machine. This edits those agents' own settings; see [section 5](#5-connect-the-host-agents). `hufu up --no-setup` skips it |
| Daemon | `hufu daemon start` | The optional daemon runs. `hufu up --no-daemon` skips it |
| Check | `hufu doctor` | A table of agents with installation and login state |

Installer options. On macOS and Linux they go after `sh -s --`, for example
`curl ... | sh -s -- --up --dir "$HOME/bin"`. On Windows set the variable before the command.

| Option | Variable | Meaning |
|---|---|---|
| `--up` / `-Up` | `HUFU_UP=1` | Run `hufu up` afterwards |
| `--version <tag>` / `-Version` | `HUFU_VERSION` | Install this release instead of the latest one |
| `--dir <path>` / `-Dir` | `HUFU_INSTALL_DIR` | Where to put the program |
| `--from-source` / `-FromSource` | `HUFU_FROM_SOURCE=1` | Always build with cargo |
| | `HUFU_DOWNLOAD_BASE` | A mirror that holds the release files, for networks that cannot reach GitHub |

**If `hufu` is not found afterwards.** On macOS and Linux the installer does not touch the shell
profile. When it prints `... is not on your PATH`, call the program by its full path
(`~/.local/bin/hufu`) for the rest of this session, and pass the `export PATH=...` line it printed
on to the user. On Windows the new `PATH` reaches only terminals opened after the install, so use
the full path there too.

**If the install script cannot be fetched**, build directly, then set up:

```bash
cargo install --git https://github.com/kverona-ai/hufu hufu-cli --locked
hufu up
```

## 4. Read the result

```bash
hufu --json doctor
```

`hufu doctor` exits `0` when the configuration is valid and `1` when it is not. The JSON document
is the part to act on:

| Field | Meaning | What to do |
|---|---|---|
| `problems` | Configuration errors. Empty means valid | Fix each one; `hufu config validate` lists them again |
| `hints` | Next steps, most important first | Work through them as below |
| `profiles[].installed` | Is the agent's program on `PATH` | |
| `profiles[].authenticated` | `true`, `false`, or absent when it cannot be told without running the agent | |
| `profiles[].hint` | The vendor's install and login commands for that agent | Pass it on to the user |
| `profiles[].missing_env` | Variables the profile needs that are not set | Ask the user to export them |
| `advisor`, `executors` | The default roles | Report them |
| `hosts` | Agents that can be registered as MCP hosts | |

How to act on a hint:

| Hint | Action |
|---|---|
| `no supported agent is installed` | Stop and ask the user to install one and log in |
| `profile X: the agent is not installed` | It matters only if the user wants that agent. Give them the install command from the hint |
| `profile X: not logged in` | Ask the user to run the login command from the hint. Do not do it for them |
| `profile X: set NAME in the shell that runs hufu` | Ask the user to export `NAME` in their shell profile. Do not ask them to paste the value to you |
| `no default advisor` | `hufu config set roles.advisor <profile>` |

Profiles marked `(preset)` in `hufu agents` are built in. They need no entry in the configuration
file; they become usable as soon as their agent is installed and logged in.

## 5. Connect the host agents

A host is an agent that calls Hufu. `hufu up` has already run `hufu setup`, which registers the MCP
server `hufu` (command `hufu mcp`, over stdio) with every agent found:

| Host | What is changed |
|---|---|
| Claude Code | Runs `claude mcp add --scope user hufu`, and writes a skill to `~/.claude/skills/hufu/SKILL.md` |
| Codex | A marked block in `~/.codex/config.toml` |
| Cursor | `~/.cursor/mcp.json` |
| Kimi Code | `~/.kimi-code/mcp.json` |
| Antigravity | `~/.gemini/config/mcp_config.json` |

Files are edited in place: other servers stay, and the original is copied once to
`<file>.hufu-bak`. The agents' own variables for their configuration directory (`CODEX_HOME`,
`CLAUDE_CONFIG_DIR`, `CURSOR_CONFIG_DIR`, `KIMI_CODE_HOME`) are honoured.

```bash
hufu setup --print               # show what would be written, change nothing
hufu setup --agent codex         # one of: claude, codex, cursor, kimi, agy, generic
hufu setup --agent claude --project    # only for the current project (.mcp.json)
hufu setup --remove              # take the registration out again
```

**Any other host.** Print the entry and add it to that host's MCP configuration yourself:

```console
$ hufu setup --agent generic --print
{
  "mcpServers": {
    "hufu": {
      "args": [
        "mcp"
      ],
      "command": "/home/you/.local/bin/hufu"
    }
  }
}
```

The `command` is the full path of the installed program, so the host finds it whatever its `PATH`.

**A restart is needed.** A host reads its MCP servers when a session starts. You will not see the
`hufu_*` tools in the session that ran the installer. Tell the user to restart their agent, or to
open a new session. Until then, do the rest of the configuration with the `hufu` command line,
which does the same.

## 6. Configure

The configuration `hufu init` wrote is enough to start: it names the agents found, picks an
advisor and makes every found agent an executor. Change it only for what the user asked for. Every
change is validated before it is written; an invalid one is refused and nothing changes.

There are two equivalent ways. Use the command line in the session that installed Hufu, and the
MCP tools in any session started afterwards.

| Goal | Command line | MCP tool |
|---|---|---|
| See profiles, pools and roles | `hufu agents`; `hufu agents --probe` | `hufu_agents` |
| Check installation and logins | `hufu doctor` | `hufu_doctor` |
| Read configuration | `hufu config get [key]` | `hufu_config_get` |
| Set a key, for example a role | `hufu config set <key> <value>` | `hufu_config_set` |
| Add or change a profile | `hufu profile add\|set <name> ...` | `hufu_profile_set` |
| Remove a profile | `hufu profile remove <name>` | `hufu_profile_remove` |
| Add or change a pool | `hufu pool add <name> --member a,b` | `hufu_pool_set` |
| Remove a pool | `hufu pool remove <name>` | `hufu_pool_remove` |
| Write a first configuration | `hufu init` | `hufu_init` |
| Register with more hosts | `hufu setup` | `hufu_setup` |

Add `--project` (or `"project": true`) to write to `.hufu/config.yaml` of the current project
instead of the user-wide file.

### Roles: who advises, who executes

```bash
hufu config set roles.advisor claude
hufu config set roles.executors "[codex, agy]"
```

```json
{"tool": "hufu_config_set", "arguments": {"key": "roles.executors", "value": ["codex", "agy"]}}
```

There is one advisor, and it is a profile. Executors are any number of profiles and pools.

### A second account of the same agent

A second account is a second profile with its own login directory. The user logs that account in.

```bash
hufu profile add codex-work --adapter codex-cli --account work --home ~/.codex-work
hufu pool add codex-any --member codex,codex-work --strategy failover
```

```json
{"tool": "hufu_profile_set", "arguments": {"name": "codex-work",
  "profile": {"adapter": "codex-cli", "account": "work", "home": "~/.codex-work"}}}
{"tool": "hufu_pool_set", "arguments": {"name": "codex-any",
  "members": ["codex", "codex-work"], "strategy": "failover"}}
```

Then ask the user to run `CODEX_HOME=~/.codex-work codex login` once.

### A model behind an API key

```bash
hufu profile add or-reasoner --adapter pi --provider openrouter --model vendor/model-slug \
    --env OPENROUTER_API_KEY
```

`--env` takes the variable's **name**. The user exports the value in their shell profile; Hufu
passes it through and never stores it. When two accounts need two keys, map a differently named
shell variable: `--env-from OPENROUTER_API_KEY=OPENROUTER_API_KEY_TEAM`.

### Any ACP agent

```bash
hufu profile add my-agent --adapter acp --command my-agent,acp
```

The adapters are `codex-cli`, `agy-cli`, `cursor-cli`, `claude-cli`, `kimi-cli`, `pi`, `acp` and
`external`. Every setting is described in the
[user guide](user-guide.md#11-configuration-reference), and a complete commented file is
[`doc/examples/config.yaml`](../examples/config.yaml).

## 7. Prove that it works

```bash
hufu doctor --self-test
```

This runs real background jobs against agents built into the program. It needs no account and no
network, and it checks three things: a job runs in the background and its result can be read, a
permission tier is enforced, and a cancelled agent leaves no process behind. All three lines must
say `ok`.

A job against a real agent uses the user's account, so ask first. A small read-only one is enough:

```bash
hufu run -a <profile> -p read-only --wait "Reply with the single word: ok"
```

Exit code `0` means the job was accepted, `10` that the agent is not installed or not logged in,
`3` that the job failed and `5` that it timed out.

Hufu 0.1 was tested against built-in fake agents, not yet against real accounts. If the first real
job fails, collect `hufu logs <job> --raw --stderr` and `hufu --json result <job>`, remove anything
private, and suggest that the user reports it at <https://github.com/kverona-ai/hufu/issues>.

## 8. Report to the user

End with a short report that covers:

- the installed version and where the program is (`hufu --version`, the path the installer printed);
- which agents were found and which of them are logged in;
- the default advisor and executors;
- which hosts were registered, and which of their files were changed (a backup is next to each as
  `<file>.hufu-bak`);
- whether the daemon is running;
- what is left for the user: logins, variables to export, the `PATH` line if one was printed, and
  **restarting their agent** so that the `hufu_*` tools appear;
- how to undo it: see [section 10](#10-upgrade-and-uninstall).

## 9. Environment variables

Read by the install scripts:

| Variable | Meaning |
|---|---|
| `HUFU_UP` | `1` runs `hufu up` after installing |
| `HUFU_VERSION` | A release tag such as `v0.1.0`. Default: the latest release |
| `HUFU_INSTALL_DIR` | Where to put the program. Default `~/.local/bin`, on Windows `%LOCALAPPDATA%\hufu\bin` |
| `HUFU_FROM_SOURCE` | `1` builds with cargo instead of downloading |
| `HUFU_DOWNLOAD_BASE` | The URL of a mirror that holds the release archives and their checksums |

Read by `hufu` itself:

| Variable | Meaning |
|---|---|
| `HUFU_HOME` | Where configuration and job data live. Default `~/.hufu` |
| `HUFU_EXE` | The `hufu` program that `hufu setup` writes into other agents' settings, when it should not be the running one |
| `HUFU_KILL_GRACE_MS` | How long a cancelled agent gets to stop by itself. Default 10000 |
| `HUFU_STALE_AFTER_MS` | After how long without a heartbeat a job with a dead runner counts as lost. Default 30000 |

API keys for agents are not Hufu settings. A profile lists the names of the variables its agent
needs under `env`, and Hufu passes those through from the shell that runs it.

## 10. Upgrade and uninstall

| Task | Steps |
|---|---|
| Upgrade | Run the install command again, then `hufu doctor`. Running jobs keep the program they started with |
| Uninstall | `hufu setup --remove`, `hufu daemon stop`, delete the `hufu` program, then delete `~/.hufu` |

Deleting files is the user's decision. Before removing `~/.hufu`, say that it holds the
configuration and all job records.

## 11. When something goes wrong

| Symptom | Likely cause | What to do |
|---|---|---|
| `curl` cannot reach `raw.githubusercontent.com` | A restricted network or a sandboxed shell | Ask the user to approve network access or to run the command; or build with `cargo install --git` |
| `building from source needs Rust` | No release yet, and no `cargo` | Ask the user whether to install Rust from <https://rustup.rs>, then run the installer again |
| The cargo build fails on the Rust version | Rust older than 1.89 | `rustup update stable`, then run the installer again |
| `checksum mismatch` | A damaged or altered download | Stop and tell the user. Do not install the file |
| `hufu: command not found` | The install directory is not on `PATH` | Use the full path; pass the printed `export PATH=...` line to the user |
| `hufu setup` did nothing for an agent | Its program was not found on `PATH` | `hufu setup --agent <name>`, or `--all` |
| `claude mcp add` failed | Claude Code refused or timed out | Run `hufu setup --agent claude --print` and run the command it shows |
| A host's file was `skipped` as not plain JSON | The file has comments or is damaged | Add the entry from `hufu setup --agent generic --print` by hand |
| The `hufu_*` tools do not appear | The host was not restarted | Ask the user to restart it or to open a new session |
| `daemon not started` | The shell does not let a background process outlive the command | Nothing is lost: the daemon is optional and jobs run without it |
| Exit 10 from `hufu run` | The agent is not installed, not logged in, or a named variable is not exported | `hufu doctor` says which |

More symptoms are in the [user guide](user-guide.md#15-troubleshooting) and the
[deployment guide](../deployment/deployment.md#9-troubleshooting).
