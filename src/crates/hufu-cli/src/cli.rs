//! The command-line surface.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(
    name = "hufu",
    version,
    about = "Hufu: dispatch work to any coding agent. Any agent holds the tally, any agent does the work.",
    after_help = "Start here:\n  hufu up                      one-command setup: configure, register with your agents, start the daemon\n  hufu agents                  see what is available\n  hufu run -a codex \"...\"      hand off a task\n  hufu update                  move to the newest release\n\nExit codes: 0 ok, 1 internal, 2 usage, 3 job failed, 4 cancelled, 5 timed out, 6 wait timed out,\n7 lost, 8 rejected by the tally check, 10 agent unavailable or not logged in."
)]
pub struct Cli {
    /// Print machine-readable JSON (every document carries `"v": 1`).
    #[arg(long, global = true)]
    pub json: bool,

    #[command(subcommand)]
    pub command: Command,
}

// Parsed once per process: the size of the largest variant does not matter.
#[allow(clippy::large_enum_variant)]
#[derive(Subcommand, Debug)]
pub enum Command {
    /// Hand a task to an agent as a background job.
    Run(RunArgs),
    /// Show jobs: the active ones and the most recent, or one job.
    Status(StatusArgs),
    /// Block until jobs reach a terminal state.
    Wait(WaitArgs),
    /// Print the result of a finished job.
    Result(ResultArgs),
    /// Print or follow the events of a job.
    Logs(LogsArgs),
    /// Stop jobs. The agent's whole process tree is ended.
    Cancel(CancelArgs),
    /// Send a follow-up prompt to the session of a finished job.
    Resume(ResumeArgs),
    /// List profiles and pools with their capabilities and roles.
    Agents(AgentsArgs),
    /// Check installation, logins and configuration.
    Doctor(DoctorArgs),
    /// Remove old finished jobs and missions.
    Prune(PruneArgs),
    /// Managed missions: an advisor plans, executors work, the advisor reviews.
    #[command(subcommand)]
    Mission(MissionCommand),
    /// Serve Hufu's tools over MCP on stdio.
    Mcp(McpArgs),
    /// The optional daemon: starts queued jobs in order, restarts lost ones, prunes.
    #[command(subcommand)]
    Daemon(DaemonCommand),
    /// Write a first configuration from the agents found on this machine.
    Init(InitArgs),
    /// One-command setup: init, register with your agents, start the daemon, check.
    Up(UpArgs),
    /// Register Hufu's MCP server with other agents so that they can dispatch through it.
    Setup(SetupArgs),
    /// Update Hufu itself to the newest release, restart the daemon and run the checks.
    Update(UpdateArgs),
    /// Read and change configuration.
    #[command(subcommand)]
    Config(ConfigCommand),
    /// Add, change and remove agent profiles (one per agent, model or account).
    #[command(subcommand)]
    Profile(ProfileCommand),
    /// Pools: profiles that can stand in for each other, for example several accounts.
    #[command(subcommand)]
    Pool(PoolCommand),
    #[command(name = "__runner", hide = true)]
    Runner { job: String },
    #[command(name = "__mission", hide = true)]
    MissionDriver { mission: String },
    #[command(name = "__sweep", hide = true)]
    Sweep { exe: PathBuf, known: Vec<String> },
}

#[derive(Args, Debug, Default)]
pub struct RunArgs {
    /// The prompt. `-` reads it from stdin.
    pub prompt: Vec<String>,
    /// A profile or pool. Default: the first of `roles.executors`.
    #[arg(short, long)]
    pub agent: Option<String>,
    /// read-only, workspace-write or full-access.
    #[arg(short, long)]
    pub permission: Option<String>,
    /// default, off, low, medium, high, max, or native:<value>.
    #[arg(short, long)]
    pub effort: Option<String>,
    #[arg(short, long)]
    pub model: Option<String>,
    /// The workspace. Default: the current directory.
    #[arg(short = 'C', long)]
    pub cwd: Option<String>,
    /// Time limit, for example 30m or 2h.
    #[arg(short, long)]
    pub timeout: Option<String>,
    /// Stop the job when the agent has been silent this long.
    #[arg(long)]
    pub inactivity_timeout: Option<String>,
    /// none, or worktree for an own copy of the repository on branch hufu/<job>.
    #[arg(long)]
    pub isolation: Option<String>,
    #[arg(long)]
    pub title: Option<String>,
    /// A task file (YAML or JSON). Flags override it.
    #[arg(short, long)]
    pub file: Option<PathBuf>,
    /// Read the prompt from a file.
    #[arg(long)]
    pub prompt_file: Option<PathBuf>,
    /// A JSON Schema file the final answer must match.
    #[arg(long)]
    pub schema: Option<String>,
    /// An acceptance command that must exit 0. Repeatable.
    #[arg(long = "accept")]
    pub accept: Vec<String>,
    /// The job may only change paths matching this glob. Repeatable.
    #[arg(long = "allow-path")]
    pub allow_path: Vec<String>,
    /// The job must not change paths matching this glob. Repeatable.
    #[arg(long = "deny-path")]
    pub deny_path: Vec<String>,
    /// Fail the job if the agent skipped any action for lack of permission.
    #[arg(long)]
    pub strict: bool,
    /// Start only after this job succeeded. Repeatable.
    #[arg(long = "depends-on")]
    pub depends_on: Vec<String>,
    /// key=value, repeatable.
    #[arg(long = "label")]
    pub label: Vec<String>,
    /// Grant desktop control of these applications (comma separated).
    #[arg(long = "computer-use")]
    pub computer_use: Option<String>,
    /// The profile that operates the desktop, instead of the configured one.
    #[arg(long = "computer-use-agent")]
    pub computer_use_agent: Option<String>,
    /// dedicated (another agent operates) or executor (the job's own agent does).
    #[arg(long = "computer-use-route")]
    pub computer_use_route: Option<String>,
    /// Total attempts for retryable failures.
    #[arg(long)]
    pub retry: Option<u32>,
    /// Wait for the job and print its result.
    #[arg(short, long)]
    pub wait: bool,
    /// With --wait: print events as they happen.
    #[arg(long)]
    pub follow: bool,
    /// With --wait: give up waiting after this long (the job keeps running).
    #[arg(long)]
    pub wait_timeout: Option<String>,
    /// Validate and show the exact invocation without starting anything.
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Args, Debug)]
pub struct StatusArgs {
    pub job: Option<String>,
    /// Every job, not only the active and the most recent ones.
    #[arg(long)]
    pub all: bool,
    /// Only jobs in this state.
    #[arg(long)]
    pub state: Option<String>,
    /// Only jobs of this mission.
    #[arg(long)]
    pub mission: Option<String>,
    #[arg(long, default_value_t = 10)]
    pub limit: usize,
}

#[derive(Args, Debug)]
pub struct WaitArgs {
    #[arg(required = true)]
    pub jobs: Vec<String>,
    /// Give up after this long. The jobs keep running.
    #[arg(short, long)]
    pub timeout: Option<String>,
    /// Return as soon as one of the jobs has finished.
    #[arg(long)]
    pub any: bool,
}

#[derive(Args, Debug)]
pub struct ResultArgs {
    pub job: String,
    /// Do not clip the final message.
    #[arg(long)]
    pub full: bool,
}

#[derive(Args, Debug)]
pub struct LogsArgs {
    pub job: String,
    /// Keep printing until the job ends.
    #[arg(short, long)]
    pub follow: bool,
    /// The agent's untouched output instead of normalized events.
    #[arg(long)]
    pub raw: bool,
    /// With --raw: the agent's stderr.
    #[arg(long)]
    pub stderr: bool,
    /// Only events after this sequence number.
    #[arg(long, default_value_t = 0)]
    pub since: u64,
}

#[derive(Args, Debug)]
pub struct CancelArgs {
    #[arg(required = true)]
    pub jobs: Vec<String>,
    /// Kill the processes at once instead of asking them to stop.
    #[arg(long)]
    pub force: bool,
}

#[derive(Args, Debug)]
pub struct ResumeArgs {
    pub job: String,
    /// The follow-up prompt. `-` reads it from stdin.
    pub prompt: Vec<String>,
    #[arg(short, long)]
    pub wait: bool,
}

#[derive(Args, Debug)]
pub struct AgentsArgs {
    /// Also check whether each agent is installed and logged in.
    #[arg(long)]
    pub probe: bool,
}

#[derive(Args, Debug)]
pub struct DoctorArgs {
    /// Also run real jobs against built-in fake agents.
    #[arg(long = "self-test")]
    pub self_test: bool,
}

#[derive(Args, Debug)]
pub struct PruneArgs {
    /// Remove finished jobs older than this. Default: retention.max_age.
    #[arg(long = "older-than")]
    pub older_than: Option<String>,
    /// Keep at most this many finished jobs. Default: retention.max_jobs.
    #[arg(long)]
    pub keep: Option<usize>,
    /// Also remove worktrees left behind. Their branches are kept.
    #[arg(long)]
    pub worktrees: bool,
    #[arg(long = "dry-run")]
    pub dry_run: bool,
}

#[derive(Subcommand, Debug)]
pub enum MissionCommand {
    /// Start a mission.
    Start(MissionStartArgs),
    /// Show one mission, or all of them.
    Status { mission: Option<String> },
    /// List missions.
    List,
    /// Print the report of a mission.
    Report { mission: String },
    /// Block until a mission ends.
    Wait {
        mission: String,
        #[arg(short, long)]
        timeout: Option<String>,
    },
    /// Finish the running jobs, then stop starting new ones.
    Pause { mission: String },
    /// Continue a paused or interrupted mission.
    Resume { mission: String },
    /// Stop now and cancel the running jobs. Finished work is kept.
    Abort { mission: String },
}

#[derive(Args, Debug)]
pub struct MissionStartArgs {
    /// The goal, in your own words.
    pub goal: Vec<String>,
    /// A mission file (YAML). Flags override it.
    #[arg(short, long)]
    pub file: Option<PathBuf>,
    /// The profile that plans and reviews. Default: roles.advisor.
    #[arg(long)]
    pub advisor: Option<String>,
    /// Profiles and pools the advisor may dispatch to, comma separated. Default: roles.executors.
    #[arg(long, value_delimiter = ',')]
    pub executors: Vec<String>,
    #[arg(short = 'C', long)]
    pub cwd: Option<String>,
    /// worktree runs the whole mission in its own copy of the repository.
    #[arg(long)]
    pub isolation: Option<String>,
    /// always, on_failure or never.
    #[arg(long)]
    pub review: Option<String>,
    /// The highest tier a task may ask for.
    #[arg(long = "max-tier")]
    pub max_tier: Option<String>,
    #[arg(long = "max-tasks")]
    pub max_tasks: Option<u32>,
    /// Command prefixes the advisor may use in acceptance checks. Repeatable.
    #[arg(long = "allow-command")]
    pub allow_command: Vec<String>,
    /// Wait for the mission and print its report.
    #[arg(short, long)]
    pub wait: bool,
}

#[derive(Args, Debug)]
pub struct McpArgs {
    /// Serve only the computer-use hand-over tool, for the executor of this job.
    #[arg(long = "child-of")]
    pub child_of: Option<String>,
}

#[derive(Subcommand, Debug)]
pub enum DaemonCommand {
    /// Start the daemon in the background.
    Start,
    /// Stop the daemon. Running jobs are not affected.
    Stop,
    /// Show whether the daemon is running.
    Status,
    /// Run the daemon in the foreground.
    Run,
}

#[derive(Args, Debug)]
pub struct InitArgs {
    /// Replace an existing configuration (a backup is kept).
    #[arg(long)]
    pub force: bool,
}

#[derive(Args, Debug)]
pub struct UpArgs {
    /// Do not register with other agents.
    #[arg(long = "no-setup")]
    pub no_setup: bool,
    /// Do not start the daemon.
    #[arg(long = "no-daemon")]
    pub no_daemon: bool,
}

#[derive(Args, Debug)]
pub struct SetupArgs {
    /// claude, codex, cursor, kimi, agy or generic. Repeatable. Default: every agent found.
    #[arg(short, long = "agent")]
    pub agent: Vec<String>,
    /// Every known agent, whether it looks installed or not.
    #[arg(long)]
    pub all: bool,
    /// Take the registration out again.
    #[arg(long)]
    pub remove: bool,
    /// Show what would be written instead of writing it.
    #[arg(long)]
    pub print: bool,
    /// Register for the current project only, where the agent supports it.
    #[arg(long)]
    pub project: bool,
}

#[derive(Args, Debug)]
pub struct UpdateArgs {
    /// Only report whether a newer release exists. Nothing is downloaded or changed.
    #[arg(long, conflicts_with_all = ["from_source", "force"])]
    pub check: bool,
    /// Install this release, for example v0.1.0, instead of the newest one. An older one works too.
    #[arg(long = "version", value_name = "VERSION")]
    pub version: Option<String>,
    /// Build with cargo instead of downloading a binary: the main branch, or the tag given with --version.
    #[arg(long = "from-source")]
    pub from_source: bool,
    /// Install again even when that version is already installed.
    #[arg(long)]
    pub force: bool,
}

#[derive(Subcommand, Debug)]
pub enum ConfigCommand {
    /// Print the merged configuration, or one key such as roles.advisor.
    Get { key: Option<String> },
    /// Set a key. The value is read as YAML: 4, true, [a, b], {k: v}.
    Set {
        key: String,
        value: String,
        /// Write to .hufu/config.yaml of the current project.
        #[arg(long)]
        project: bool,
    },
    /// Remove a key from the file.
    Unset {
        key: String,
        #[arg(long)]
        project: bool,
    },
    /// Print the configuration files in use.
    Path,
    /// Check the configuration and report every problem.
    Validate,
}

#[derive(Args, Debug)]
pub struct ProfileArgs {
    pub name: String,
    /// codex-cli, agy-cli, cursor-cli, claude-cli, kimi-cli, pi, acp or external.
    #[arg(long)]
    pub adapter: Option<String>,
    /// The default model of this profile, passed to the agent as it is.
    #[arg(long)]
    pub model: Option<String>,
    /// pi only: the model provider.
    #[arg(long)]
    pub provider: Option<String>,
    /// The default effort of this profile: default, off, low, medium, high, max or native:<value>.
    #[arg(long)]
    pub effort: Option<String>,
    /// What to do with an effort level this agent does not have: error (the default) or nearest.
    #[arg(long = "effort-fallback")]
    pub effort_fallback: Option<String>,
    #[arg(long)]
    pub permission: Option<String>,
    /// Needed together with --permission full-access.
    #[arg(long = "allow-full-access")]
    pub allow_full_access: bool,
    /// A label for the account, shown in listings.
    #[arg(long)]
    pub account: Option<String>,
    /// The agent's own login directory for this profile; keeps two accounts of one agent apart.
    #[arg(long)]
    pub home: Option<String>,
    /// The name of an environment variable to pass through. Repeatable.
    #[arg(long = "env")]
    pub env: Vec<String>,
    /// AGENT_VAR=SHELL_VAR: the agent sees AGENT_VAR with the value of SHELL_VAR. Repeatable.
    #[arg(long = "env-from")]
    pub env_from: Vec<String>,
    /// KEY=VALUE: a literal, non-secret variable. Repeatable.
    #[arg(long = "var")]
    pub var: Vec<String>,
    /// The argv that starts the agent, comma separated, for example `kimi,acp`.
    #[arg(long, value_delimiter = ',')]
    pub command: Vec<String>,
    #[arg(long)]
    pub description: Option<String>,
    /// This profile can operate desktop applications: native, lcu, or another runtime name.
    #[arg(long = "computer-use")]
    pub computer_use: Option<String>,
    /// ACP only: someone checked that this agent asks before it acts.
    #[arg(long = "verified")]
    pub verified: bool,
    /// Write to .hufu/config.yaml of the current project.
    #[arg(long)]
    pub project: bool,
}

#[derive(Subcommand, Debug)]
pub enum ProfileCommand {
    /// Create a profile (replaces one of the same name).
    Add(ProfileArgs),
    /// Change some fields of a profile.
    Set(ProfileArgs),
    /// Remove a profile.
    Remove {
        name: String,
        #[arg(long)]
        project: bool,
    },
    /// List profiles.
    List,
    /// Show one profile with its capabilities.
    Show { name: String },
}

#[derive(Subcommand, Debug)]
pub enum PoolCommand {
    /// Create or replace a pool.
    Add {
        name: String,
        /// A member profile. Repeatable, or comma separated.
        #[arg(short, long = "member", required = true, value_delimiter = ',')]
        member: Vec<String>,
        /// failover, round-robin or least-busy.
        #[arg(long)]
        strategy: Option<String>,
        #[arg(long)]
        description: Option<String>,
        #[arg(long)]
        project: bool,
    },
    /// Remove a pool.
    Remove {
        name: String,
        #[arg(long)]
        project: bool,
    },
    /// List pools.
    List,
}
