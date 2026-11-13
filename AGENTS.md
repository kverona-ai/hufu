# Hufu: instructions for agents

Hufu is a local dispatch layer for coding agents: one self-contained program, `hufu`, that lets any
agent hand work to any other agent as background jobs. This file tells an agent that has been
pointed at this repository where to go.

## You were asked to install or configure Hufu

Follow **[doc/guide/agent-install.md](doc/guide/agent-install.md)** from top to bottom. In short:

1. Check that at least one agent program (`codex`, `agy`, `agent`, `kimi`, `claude` or `pi`) is
   installed and logged in. Hufu installs no agents and logs in for nobody; that part is the user's.
2. Install and set up with one command:

   ```bash
   # macOS and Linux
   curl -fsSL https://raw.githubusercontent.com/kverona-ai/hufu/main/src/install/install.sh | sh -s -- --up
   ```

   ```powershell
   # Windows (PowerShell)
   $env:HUFU_UP = "1"; irm https://raw.githubusercontent.com/kverona-ai/hufu/main/src/install/install.ps1 | iex
   ```

   Until the first release is tagged the installer builds from source, which needs Rust 1.89 or
   newer and takes a minute or more.
3. Run `hufu --json doctor` and act on its `hints`. `hufu doctor --self-test` proves that jobs run,
   without any account.
4. Tell the user to restart their agent: the `hufu_*` MCP tools appear in new sessions only. For a
   host Hufu does not know, `hufu setup --agent generic --print` prints the entry to add.
5. Configure with the `hufu` command line now (`hufu profile add`, `hufu pool add`,
   `hufu config set`), or with the MCP tools after the restart (`hufu_profile_set`,
   `hufu_pool_set`, `hufu_config_set`).

Never log an agent in for the user, never write an API key into a file or onto a command line (the
configuration holds the names of environment variables, never their values), and do not use `sudo`.

## You are using Hufu to dispatch work

The tools and the rules for them are in the
[user guide](doc/guide/user-guide.md#9-using-hufu-from-another-agent-mcp) and in the skill at [src/integrations/claude-code/SKILL.md](src/integrations/claude-code/SKILL.md).

## You are changing this repository

- Read [CONTRIBUTING.md](CONTRIBUTING.md) and [src/README.md](src/README.md) first.
- Documents go under [`doc/`](doc/README.md) and are listed in its index; code goes under `src/`.
- From `src/`: `cargo fmt --all --check`, `cargo clippy --all-targets -- -D warnings` and
  `cargo test` must pass. Tests need no real agent, no login and no network.
- A change that users can see updates the user guide and both `README.md` and `README.zh-CN.md` in
  the same pull request.
- Never run `hufu up` or `hufu setup` on a machine you are developing on without `--print`,
  `--project` or `--no-setup`, unless the user wants Hufu registered with the agents on it. Point
  `HUFU_HOME` at a scratch directory when you try things out.
