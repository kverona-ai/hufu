//! Types shared between the core and every adapter.

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::event::EventBody;

/// Version of the task spec, the JSON outputs and the job store layout.
pub const FORMAT_VERSION: u32 = 1;

// ---------------------------------------------------------------------------------------------
// Permission tiers
// ---------------------------------------------------------------------------------------------

/// What an agent is allowed to do. Ordered from least to most privilege.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Tier {
    ReadOnly,
    WorkspaceWrite,
    FullAccess,
}

impl Tier {
    pub const ALL: [Tier; 3] = [Tier::ReadOnly, Tier::WorkspaceWrite, Tier::FullAccess];

    pub fn as_str(self) -> &'static str {
        match self {
            Tier::ReadOnly => "read-only",
            Tier::WorkspaceWrite => "workspace-write",
            Tier::FullAccess => "full-access",
        }
    }

    pub fn parse(s: &str) -> Result<Tier, String> {
        match s.trim() {
            "read-only" | "readonly" | "ro" => Ok(Tier::ReadOnly),
            "workspace-write" | "write" | "ww" => Ok(Tier::WorkspaceWrite),
            "full-access" | "full" => Ok(Tier::FullAccess),
            other => Err(format!(
                "unknown permission tier `{other}`; accepted: read-only, workspace-write, full-access"
            )),
        }
    }
}

impl fmt::Display for Tier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Formats a list of tiers for messages.
pub fn tier_list(tiers: &[Tier]) -> String {
    tiers.iter().map(|t| t.as_str()).collect::<Vec<_>>().join(", ")
}

// ---------------------------------------------------------------------------------------------
// Reasoning effort
// ---------------------------------------------------------------------------------------------

/// Hufu's uniform effort scale. `Default` passes nothing and lets the agent decide.
/// `Native` carries an agent-specific value that is passed through verbatim.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Default)]
pub enum Effort {
    #[default]
    Default,
    Off,
    Low,
    Medium,
    High,
    Max,
    Native(String),
}

impl Effort {
    pub const SCALE: [Effort; 5] = [Effort::Off, Effort::Low, Effort::Medium, Effort::High, Effort::Max];

    pub fn parse(s: &str) -> Result<Effort, String> {
        let s = s.trim();
        Ok(match s {
            "default" | "" => Effort::Default,
            "off" | "none" => Effort::Off,
            "low" => Effort::Low,
            "medium" => Effort::Medium,
            "high" => Effort::High,
            "max" => Effort::Max,
            _ => match s.strip_prefix("native:") {
                Some(v) if !v.is_empty() => Effort::Native(v.to_string()),
                _ => {
                    return Err(format!(
                        "unknown effort `{s}`; accepted: default, off, low, medium, high, max, or native:<value>"
                    ));
                }
            },
        })
    }

    /// Position on the uniform scale, used to find the nearest supported level.
    pub fn rank(&self) -> Option<i32> {
        match self {
            Effort::Off => Some(0),
            Effort::Low => Some(1),
            Effort::Medium => Some(2),
            Effort::High => Some(3),
            Effort::Max => Some(4),
            Effort::Default | Effort::Native(_) => None,
        }
    }

    /// The closest level in `supported`. Ties go to the lower level, which never raises cost.
    pub fn nearest(&self, supported: &[Effort]) -> Option<Effort> {
        let want = self.rank()?;
        supported
            .iter()
            .filter_map(|e| e.rank().map(|r| (r, e)))
            .min_by_key(|(r, _)| ((r - want).abs(), *r))
            .map(|(_, e)| e.clone())
    }
}

impl fmt::Display for Effort {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Effort::Default => f.write_str("default"),
            Effort::Off => f.write_str("off"),
            Effort::Low => f.write_str("low"),
            Effort::Medium => f.write_str("medium"),
            Effort::High => f.write_str("high"),
            Effort::Max => f.write_str("max"),
            Effort::Native(v) => write!(f, "native:{v}"),
        }
    }
}

impl Serialize for Effort {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Effort {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Effort::parse(&s).map_err(serde::de::Error::custom)
    }
}

/// Formats a list of effort levels for messages.
pub fn effort_list(levels: &[Effort]) -> String {
    if levels.is_empty() {
        return "default only".to_string();
    }
    let mut out = vec!["default".to_string()];
    out.extend(levels.iter().map(|e| e.to_string()));
    out.join(", ")
}

// ---------------------------------------------------------------------------------------------
// Profiles
// ---------------------------------------------------------------------------------------------

/// What to do when a requested effort level has no mapping for the agent.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EffortFallback {
    #[default]
    Error,
    Nearest,
}

/// Whether someone has checked that an ACP agent asks before it acts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PermissionRequests {
    #[default]
    Unverified,
    Verified,
}

/// A capability a profile provides beyond code tools, for example computer use.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Provided {
    /// `native`, `lcu`, or the name of another runtime.
    #[serde(default = "default_via")]
    pub via: String,
    /// A command that prints the applications the runtime already allows, one per line. For
    /// `via: lcu` this defaults to `lcu apps`. Hufu only reads its output.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub check: Vec<String>,
}

fn default_via() -> String {
    "native".to_string()
}

/// An agent profile: one agent, configured one way. This is the shape used in `config.yaml`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    /// Filled in from the map key of `profiles`; a `name` written in the body is overwritten.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub adapter: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub command: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<Effort>,
    #[serde(default, skip_serializing_if = "is_default")]
    pub effort_fallback: EffortFallback,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission: Option<Tier>,
    #[serde(default, skip_serializing_if = "is_default")]
    pub allow_full_access: bool,
    /// A label for the account this profile uses, shown in listings. Several profiles may drive
    /// the same agent with different accounts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    /// The agent's own configuration and login directory for this profile. The adapter turns it
    /// into the agent's variable (`CODEX_HOME`, `CLAUDE_CONFIG_DIR`, ...), which is how two
    /// accounts of one agent are kept apart.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home: Option<String>,
    /// Names of environment variables the agent process may inherit. Never values.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub env: Vec<String>,
    /// Variables the agent sees under one name but Hufu reads from another, for example
    /// `OPENROUTER_API_KEY: OPENROUTER_KEY_TEAM_B`. Names only, never values.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env_from: BTreeMap<String, String>,
    /// Literal, non-secret variables set for the agent process.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub vars: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "is_default")]
    pub permission_requests: PermissionRequests,
    /// ACP only: command prefixes an agent may run at `workspace-write`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allow_execute: Vec<String>,
    /// ACP only: the `authenticate` method to call when the agent offers a non-interactive one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_method: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub provides: BTreeMap<String, Provided>,
}

fn is_default<T: Default + PartialEq>(v: &T) -> bool {
    *v == T::default()
}

// ---------------------------------------------------------------------------------------------
// Capabilities and probing
// ---------------------------------------------------------------------------------------------

/// How a profile produces output that matches a JSON Schema.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StructuredOutput {
    /// The agent has its own schema flag.
    Native,
    /// Hufu instructs the agent to answer with one JSON block and validates it.
    Instructed,
}

/// What a profile can do. Hufu validates every task against this before launch.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Capabilities {
    pub permission_tiers: Vec<Tier>,
    /// Levels of the uniform scale that map to this agent. `default` is always accepted.
    pub efforts: Vec<Effort>,
    /// Whether `native:<value>` is passed through.
    pub native_effort: bool,
    /// Effort and model can only be checked once a session is open (ACP).
    pub checked_at_runtime: bool,
    /// Known model ids. Empty means "not known ahead of time".
    pub models: Vec<String>,
    pub resume: bool,
    pub structured_output: StructuredOutput,
    pub reports_usage: bool,
    pub reports_denials: bool,
    pub accepts_mcp_servers: bool,
    /// Capabilities beyond code tools, such as `computer_use`.
    pub provides: Vec<String>,
    /// Caveats a caller should know about.
    pub notes: Vec<String>,
}

impl Capabilities {
    pub fn supports_tier(&self, tier: Tier) -> bool {
        self.permission_tiers.contains(&tier)
    }
}

/// Result of checking whether an agent is installed and usable.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ProbeResult {
    pub installed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// `None` when login state cannot be detected without running the agent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authenticated: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// What the user should do next, for example an install or login command.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

// ---------------------------------------------------------------------------------------------
// Run request and outcome
// ---------------------------------------------------------------------------------------------

/// An MCP server to attach to the agent's session.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct McpServerSpec {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

/// Everything an adapter needs to run one prompt.
#[derive(Clone, Debug)]
pub struct RunRequest {
    pub job_id: String,
    pub prompt: String,
    pub cwd: PathBuf,
    pub permission: Tier,
    pub effort: Effort,
    pub model: Option<String>,
    pub output_schema: Option<Value>,
    pub resume_session: Option<String>,
    /// The complete environment of the agent process. Nothing else is inherited.
    pub env: BTreeMap<String, String>,
    pub mcp_servers: Vec<McpServerSpec>,
    /// A directory the adapter may write helper files into (the job directory).
    pub scratch_dir: PathBuf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Succeeded,
    Failed,
    Cancelled,
}

/// Machine-readable reasons for a job that did not succeed.
pub mod reason {
    pub const AUTH_REQUIRED: &str = "auth_required";
    pub const AGENT_UNAVAILABLE: &str = "agent_unavailable";
    pub const EMPTY_OUTPUT: &str = "empty_output";
    pub const AGENT_ERROR: &str = "agent_error";
    pub const PROTOCOL_ERROR: &str = "protocol_error";
    pub const PERMISSION_DENIED: &str = "permission_denied";
    pub const TIER_VIOLATED: &str = "tier_violated";
    pub const REFUSAL: &str = "refusal";
    pub const MAX_TOKENS: &str = "max_tokens";
    pub const MAX_TURNS: &str = "max_turn_requests";
    pub const UNSUPPORTED: &str = "unsupported";
    pub const INVALID_OUTPUT: &str = "invalid_output";
    pub const TIMEOUT: &str = "timeout";
    pub const INACTIVITY: &str = "inactivity_timeout";
    pub const CANCELLED: &str = "cancelled";
    pub const DEPENDENCY_FAILED: &str = "dependency_failed";
    pub const WORKSPACE_ERROR: &str = "workspace_error";
    pub const RUNNER_DIED: &str = "runner_died";
    pub const RATE_LIMITED: &str = "rate_limited";
    pub const INTERNAL: &str = "internal_error";
}

/// An action the agent skipped, or was refused, for lack of permission.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeniedAction {
    pub tool: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
}

/// Token and cost figures, as far as the agent reports them.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached_input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
}

impl Usage {
    pub fn is_empty(&self) -> bool {
        *self == Usage::default()
    }

    /// Total tokens, computed from the parts when the agent gives no total.
    pub fn tokens(&self) -> u64 {
        self.total_tokens
            .unwrap_or_else(|| self.input_tokens.unwrap_or(0) + self.output_tokens.unwrap_or(0))
    }

    /// Adds another usage record to this one.
    pub fn add(&mut self, other: &Usage) {
        fn sum(a: &mut Option<u64>, b: Option<u64>) {
            if let Some(b) = b {
                *a = Some(a.unwrap_or(0) + b);
            }
        }
        let combined = self.tokens() + other.tokens();
        sum(&mut self.input_tokens, other.input_tokens);
        sum(&mut self.output_tokens, other.output_tokens);
        sum(&mut self.cached_input_tokens, other.cached_input_tokens);
        sum(&mut self.reasoning_tokens, other.reasoning_tokens);
        if combined > 0 {
            self.total_tokens = Some(combined);
        }
        if let Some(c) = other.cost_usd {
            self.cost_usd = Some(self.cost_usd.unwrap_or(0.0) + c);
        }
    }
}

/// A session that a follow-up prompt may be sent to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionRef {
    pub id: String,
    pub resumable: bool,
}

/// What an adapter reports when the agent has stopped.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RunOutcome {
    pub status: RunStatus,
    /// One of the codes in [`reason`], when the run did not succeed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// A human-readable explanation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub final_message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured_output: Option<Value>,
    #[serde(default)]
    pub denied_actions: Vec<DeniedAction>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<SessionRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    /// The failure is transient (rate limit, server error) and may be retried.
    #[serde(default)]
    pub retryable: bool,
    /// Paths the agent reported editing, for workspaces that are not git repositories.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub touched_paths: Vec<String>,
}

impl RunOutcome {
    pub fn failed(reason: &str, detail: impl Into<String>) -> RunOutcome {
        RunOutcome {
            status: RunStatus::Failed,
            reason: Some(reason.to_string()),
            detail: Some(detail.into()),
            final_message: None,
            structured_output: None,
            denied_actions: Vec::new(),
            session: None,
            usage: None,
            exit_code: None,
            retryable: false,
            touched_paths: Vec::new(),
        }
    }
}

/// The exact invocation an adapter would use. Shown by `hufu run --dry-run` and checked in tests.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LaunchPlan {
    pub program: String,
    pub args: Vec<String>,
    /// Text written to the agent's stdin before it is closed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdin: Option<String>,
    /// Extra environment variables the adapter sets itself (names and values are not secrets).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

/// An error raised by an adapter before or while building an invocation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdapterError(pub String);

impl fmt::Display for AdapterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for AdapterError {}

// ---------------------------------------------------------------------------------------------
// Cancellation and the event sink
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CancelReason {
    User,
    Timeout,
    Inactivity,
}

/// A flag shared between the runner and an adapter. Once set it stays set.
#[derive(Clone, Debug, Default)]
pub struct CancelToken(Arc<AtomicU8>);

impl CancelToken {
    pub fn new() -> CancelToken {
        CancelToken::default()
    }

    pub fn cancel(&self, why: CancelReason) {
        let v = match why {
            CancelReason::User => 1,
            CancelReason::Timeout => 2,
            CancelReason::Inactivity => 3,
        };
        let _ = self.0.compare_exchange(0, v, Ordering::SeqCst, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst) != 0
    }

    pub fn reason(&self) -> Option<CancelReason> {
        match self.0.load(Ordering::SeqCst) {
            1 => Some(CancelReason::User),
            2 => Some(CancelReason::Timeout),
            3 => Some(CancelReason::Inactivity),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RawStream {
    Stdout,
    Stderr,
}

/// Where an adapter sends what it observes. The runner implements this on top of the job store.
pub trait EventSink: Send + Sync {
    /// Records one normalized event.
    fn emit(&self, event: EventBody);
    /// Records one line of the agent's untouched output.
    fn raw(&self, stream: RawStream, line: &str);
    /// Tells the runner which process to terminate if the adapter cannot stop the agent itself.
    fn child_started(&self, pid: u32);
}

/// One agent surface. Implementations are stateless; everything arrives through the arguments.
pub trait Adapter: Send + Sync {
    /// The adapter kind used in configuration, for example `codex-cli`.
    fn kind(&self) -> &'static str;

    /// The command used when the profile gives none.
    fn default_command(&self) -> Vec<String>;

    /// The environment variable that points the agent at its configuration and login directory,
    /// if it has one. A profile's `home` is passed through it.
    fn home_var(&self) -> Option<&'static str> {
        None
    }

    /// Checks whether the agent is installed and, where possible, logged in. Must not prompt.
    fn probe(&self, profile: &Profile) -> ProbeResult;

    /// What this profile can do. Static: must not start the agent.
    fn capabilities(&self, profile: &Profile) -> Capabilities;

    /// Builds the invocation without running it.
    fn plan(&self, profile: &Profile, req: &RunRequest) -> Result<LaunchPlan, AdapterError>;

    /// Runs one prompt to completion, emitting events as it goes.
    fn run(
        &self,
        profile: &Profile,
        req: &RunRequest,
        sink: &dyn EventSink,
        cancel: &CancelToken,
    ) -> RunOutcome;
}

/// Looks up adapters by kind. The binary wires the real registry in; tests can supply their own.
pub trait AdapterRegistry: Send + Sync {
    fn get(&self, kind: &str) -> Option<&dyn Adapter>;
    fn kinds(&self) -> Vec<&'static str>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiers_are_ordered_and_parse() {
        assert!(Tier::ReadOnly < Tier::WorkspaceWrite && Tier::WorkspaceWrite < Tier::FullAccess);
        assert_eq!(Tier::parse("workspace-write").unwrap(), Tier::WorkspaceWrite);
        assert!(Tier::parse("root").unwrap_err().contains("accepted"));
        assert_eq!(serde_json::to_string(&Tier::FullAccess).unwrap(), "\"full-access\"");
    }

    #[test]
    fn effort_parses_and_finds_nearest() {
        assert_eq!(Effort::parse("native:xhigh").unwrap(), Effort::Native("xhigh".into()));
        assert!(Effort::parse("extreme").is_err());
        let supported = [Effort::Low, Effort::Medium, Effort::High];
        assert_eq!(Effort::Max.nearest(&supported), Some(Effort::High));
        assert_eq!(Effort::Off.nearest(&supported), Some(Effort::Low));
        assert_eq!(Effort::Medium.nearest(&[Effort::Off, Effort::High]), Some(Effort::High));
        assert_eq!(Effort::Default.nearest(&supported), None);
        let e: Effort = serde_json::from_str("\"high\"").unwrap();
        assert_eq!(e, Effort::High);
    }

    #[test]
    fn cancel_token_keeps_the_first_reason() {
        let t = CancelToken::new();
        assert!(!t.is_cancelled());
        t.cancel(CancelReason::Timeout);
        t.cancel(CancelReason::User);
        assert_eq!(t.reason(), Some(CancelReason::Timeout));
    }

    #[test]
    fn usage_adds_up() {
        let mut a = Usage { input_tokens: Some(10), output_tokens: Some(5), ..Usage::default() };
        a.add(&Usage { input_tokens: Some(1), cost_usd: Some(0.5), ..Usage::default() });
        assert_eq!(a.input_tokens, Some(11));
        assert_eq!(a.tokens(), 16);
        assert_eq!(a.cost_usd, Some(0.5));
    }
}
