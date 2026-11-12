//! Configuration: a user-wide file, an optional project file that overrides it, and built-in
//! presets so that common agents work before anything is configured.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use hufu_contract::time::parse_duration;
use hufu_contract::{AdapterRegistry, Effort, PermissionRequests, Profile, Tier, proc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::error::{HufuError, IoContext, Result};

/// The directory that holds configuration and every job record. `$HUFU_HOME`, or `~/.hufu`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Home {
    pub root: PathBuf,
}

impl Home {
    pub fn resolve() -> Result<Home> {
        if let Some(v) = std::env::var_os("HUFU_HOME").filter(|v| !v.is_empty()) {
            return Ok(Home { root: PathBuf::from(v) });
        }
        match proc::home_dir() {
            Some(home) => Ok(Home { root: home.join(".hufu") }),
            None => Err(HufuError::internal("cannot find the home directory; set HUFU_HOME")),
        }
    }

    pub fn at(root: impl Into<PathBuf>) -> Home {
        Home { root: root.into() }
    }

    pub fn config_file(&self) -> PathBuf {
        self.root.join("config.yaml")
    }
    pub fn jobs(&self) -> PathBuf {
        self.root.join("jobs")
    }
    pub fn job(&self, id: &str) -> PathBuf {
        self.jobs().join(id)
    }
    pub fn missions(&self) -> PathBuf {
        self.root.join("missions")
    }
    pub fn mission(&self, id: &str) -> PathBuf {
        self.missions().join(id)
    }
    pub fn run(&self) -> PathBuf {
        self.root.join("run")
    }
    pub fn slots(&self) -> PathBuf {
        self.run().join("slots")
    }
    pub fn worktrees(&self) -> PathBuf {
        self.root.join("worktrees")
    }

    pub fn ensure(&self) -> Result<()> {
        for dir in [self.jobs(), self.missions(), self.slots(), self.worktrees()] {
            std::fs::create_dir_all(&dir).at(&dir)?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Isolation {
    #[default]
    None,
    Worktree,
}

impl Isolation {
    pub fn parse(s: &str) -> std::result::Result<Isolation, String> {
        match s {
            "none" => Ok(Isolation::None),
            "worktree" => Ok(Isolation::Worktree),
            other => Err(format!("unknown isolation `{other}`; accepted: none, worktree")),
        }
    }
}

/// Which failures are retried, how often, and how long to wait in between.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetryPolicy {
    /// Total attempts, including the first. `1` means no retry.
    #[serde(default = "one")]
    pub max_attempts: u32,
    /// Failure reasons that are retried, for example `rate_limited`, `agent_error`, `timeout`.
    #[serde(default = "default_retry_on")]
    pub on: Vec<String>,
    #[serde(default = "default_backoff")]
    pub backoff: String,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        RetryPolicy { max_attempts: 1, on: default_retry_on(), backoff: default_backoff() }
    }
}

fn one() -> u32 {
    1
}
fn default_retry_on() -> Vec<String> {
    vec!["rate_limited".into()]
}
fn default_backoff() -> String {
    "15s".into()
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Defaults {
    pub permission: Tier,
    pub effort: Effort,
    pub timeout: String,
    pub inactivity_timeout: String,
    pub isolation: Isolation,
    /// How many characters of the final message `hufu result` shows before `--full`.
    pub result_chars: usize,
    pub retry: RetryPolicy,
}

impl Default for Defaults {
    fn default() -> Self {
        Defaults {
            permission: Tier::ReadOnly,
            effort: Effort::Default,
            timeout: "30m".into(),
            inactivity_timeout: "10m".into(),
            isolation: Isolation::None,
            result_chars: 4000,
            retry: RetryPolicy::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Limits {
    pub max_concurrent_jobs: usize,
    pub per_profile: BTreeMap<String, usize>,
}

impl Default for Limits {
    fn default() -> Self {
        Limits { max_concurrent_jobs: 4, per_profile: BTreeMap::new() }
    }
}

/// Who plans and who does the work, when a command does not say.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Roles {
    /// The profile that advises missions by default.
    pub advisor: Option<String>,
    /// The default roster: profiles or pools a mission may dispatch to.
    pub executors: Vec<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Strategy {
    /// Use the first member; move to the next only when one fails.
    #[default]
    Failover,
    /// Take members in turn.
    RoundRobin,
    /// Use the member with the fewest running jobs.
    LeastBusy,
}

/// Several profiles that can stand in for each other, typically several accounts of one agent.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pool {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub members: Vec<String>,
    #[serde(default)]
    pub strategy: Strategy,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewPolicy {
    Always,
    #[default]
    OnFailure,
    Never,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Budgets {
    pub max_tasks: u32,
    pub max_rounds_per_task: u32,
    pub max_wall_clock: String,
    pub max_tokens: Option<u64>,
    pub max_cost: Option<f64>,
}

impl Default for Budgets {
    fn default() -> Self {
        Budgets { max_tasks: 12, max_rounds_per_task: 3, max_wall_clock: "3h".into(), max_tokens: None, max_cost: None }
    }
}

/// A capability a task may use beyond its workspace, with its scope.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Grant {
    pub capability: String,
    #[serde(default)]
    pub apps: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct MissionDefaults {
    pub review: ReviewPolicy,
    /// No task in a mission may request more than this.
    pub max_tier: Tier,
    /// Command prefixes an advisor-written acceptance check may run.
    pub acceptance_commands: Vec<String>,
    /// Grants an advisor may request for a task, with the apps it may name.
    pub allowed_grants: Vec<Grant>,
    pub budgets: Budgets,
    /// How many tasks of one mission may run at the same time.
    pub max_parallel: u32,
}

impl Default for MissionDefaults {
    fn default() -> Self {
        MissionDefaults {
            review: ReviewPolicy::OnFailure,
            max_tier: Tier::WorkspaceWrite,
            acceptance_commands: Vec::new(),
            allowed_grants: Vec::new(),
            budgets: Budgets::default(),
            max_parallel: 1,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Route {
    /// A separately named profile operates the desktop on the executor's behalf.
    #[default]
    Dedicated,
    /// The executor's own agent operates the desktop.
    Executor,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WhenUnavailable {
    #[default]
    Error,
    /// Fall back to the executor's own agent when the dedicated one cannot serve.
    Executor,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ComputerUse {
    pub route: Route,
    /// The profile that serves computer use on the dedicated route. It may be one of the
    /// executors or a profile configured only for this. It must declare `provides.computer_use`.
    pub agent: Option<String>,
    pub when_unavailable: WhenUnavailable,
    /// One operator per display.
    pub max_concurrent: usize,
}

impl Default for ComputerUse {
    fn default() -> Self {
        ComputerUse { route: Route::Dedicated, agent: None, when_unavailable: WhenUnavailable::Error, max_concurrent: 1 }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct CapabilityConfig {
    pub computer_use: ComputerUse,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Retention {
    /// Finished jobs older than this are removed by `hufu prune` and by the daemon.
    pub max_age: String,
    /// Keep at most this many finished jobs.
    pub max_jobs: usize,
}

impl Default for Retention {
    fn default() -> Self {
        Retention { max_age: "30d".into(), max_jobs: 2000 }
    }
}

/// The merged configuration.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default = "one")]
    pub version: u32,
    #[serde(default)]
    pub defaults: Defaults,
    #[serde(default)]
    pub limits: Limits,
    #[serde(default)]
    pub roles: Roles,
    #[serde(default)]
    pub profiles: BTreeMap<String, Profile>,
    #[serde(default)]
    pub pools: BTreeMap<String, Pool>,
    #[serde(default)]
    pub missions: MissionDefaults,
    #[serde(default)]
    pub capabilities: CapabilityConfig,
    #[serde(default)]
    pub retention: Retention,
}

impl Default for Config {
    fn default() -> Self {
        from_value(json!({})).expect("the empty configuration is valid")
    }
}

impl Config {
    /// Refuses to work on a configuration that has problems, naming all of them at once.
    pub fn ensure_valid(&self, adapters: &dyn AdapterRegistry) -> Result<()> {
        let problems = self.problems(adapters);
        if problems.is_empty() {
            Ok(())
        } else {
            Err(HufuError::usage(format!("the configuration has {} problem(s); fix them first (`hufu config validate` lists them):\n  {}", problems.len(), problems.join("\n  "))))
        }
    }
}

/// What a name in `agent:` refers to.
#[derive(Clone, Debug, PartialEq)]
pub enum AgentRef {
    Profile(Profile),
    Pool { name: String, strategy: Strategy, members: Vec<Profile> },
}

impl Config {
    pub fn profile(&self, name: &str) -> Option<&Profile> {
        self.profiles.get(name)
    }

    /// Resolves a profile or pool name. The error lists what would have been accepted.
    pub fn agent(&self, name: &str) -> Result<AgentRef> {
        if let Some(p) = self.profiles.get(name) {
            return Ok(AgentRef::Profile(p.clone()));
        }
        if let Some(pool) = self.pools.get(name) {
            let members = pool.members.iter().filter_map(|m| self.profiles.get(m).cloned()).collect();
            return Ok(AgentRef::Pool { name: name.to_string(), strategy: pool.strategy, members });
        }
        let mut known: Vec<&str> = self.profiles.keys().map(String::as_str).collect();
        known.extend(self.pools.keys().map(String::as_str));
        Err(HufuError::usage(format!(
            "unknown agent `{name}`; configured profiles and pools: {}. Add one with `hufu profile add` or see `hufu agents`",
            known.join(", ")
        )))
    }

    pub fn timeout(&self) -> Duration {
        parse_duration(&self.defaults.timeout).unwrap_or(Duration::from_secs(1800))
    }

    pub fn inactivity_timeout(&self) -> Duration {
        parse_duration(&self.defaults.inactivity_timeout).unwrap_or(Duration::from_secs(600))
    }

    /// Everything that is wrong with this configuration, each line naming the field.
    pub fn problems(&self, adapters: &dyn AdapterRegistry) -> Vec<String> {
        let mut out = Vec::new();
        if self.version != 1 {
            out.push(format!("version: {} is not supported; this build reads version 1", self.version));
        }
        for (field, value) in [
            ("defaults.timeout", &self.defaults.timeout),
            ("defaults.inactivity_timeout", &self.defaults.inactivity_timeout),
            ("defaults.retry.backoff", &self.defaults.retry.backoff),
            ("missions.budgets.max_wall_clock", &self.missions.budgets.max_wall_clock),
            ("retention.max_age", &self.retention.max_age),
        ] {
            if let Err(e) = parse_duration(value) {
                out.push(format!("{field}: {e}"));
            }
        }
        if self.defaults.permission == Tier::FullAccess {
            out.push("defaults.permission: `full-access` cannot be the global default; ask for it per task, or on a profile with `allow_full_access: true`".into());
        }
        if self.defaults.retry.max_attempts == 0 {
            out.push("defaults.retry.max_attempts: must be at least 1".into());
        }
        if self.limits.max_concurrent_jobs == 0 {
            out.push("limits.max_concurrent_jobs: must be at least 1".into());
        }
        for (name, profile) in &self.profiles {
            let at = format!("profiles.{name}");
            let Some(adapter) = adapters.get(&profile.adapter) else {
                out.push(format!("{at}.adapter: unknown adapter `{}`; accepted: {}", profile.adapter, adapters.kinds().join(", ")));
                continue;
            };
            if matches!(profile.adapter.as_str(), "acp" | "external") && profile.command.is_empty() {
                out.push(format!("{at}.command: adapter `{}` needs the argv that starts the agent, for example `[kimi, acp]`", profile.adapter));
            }
            if profile.permission == Some(Tier::FullAccess) && !profile.allow_full_access {
                out.push(format!("{at}.permission: `full-access` as a profile default also needs `allow_full_access: true`"));
            }
            if profile.home.is_some() && adapter.home_var().is_none() {
                out.push(format!(
                    "{at}.home: adapter `{}` has no known configuration-directory variable; use `vars` to set the agent's own variable instead",
                    profile.adapter
                ));
            }
            if profile.permission_requests == PermissionRequests::Verified && profile.adapter != "acp" {
                out.push(format!("{at}.permission_requests: only applies to adapter `acp`"));
            }
            if self.pools.contains_key(name) {
                out.push(format!("{at}: a pool has the same name; profile and pool names share one namespace"));
            }
            for (key, value) in &profile.vars {
                if looks_secret(key, value) {
                    out.push(format!(
                        "{at}.vars.{key}: this looks like a secret, and `vars` is stored in plain text. Keep the value in your shell and name the variable under `env`, or map it with `env_from: {{{key}: YOUR_SHELL_VARIABLE}}`"
                    ));
                }
            }
        }
        for name in self.limits.per_profile.keys() {
            if !self.profiles.contains_key(name) {
                out.push(format!("limits.per_profile.{name}: no such profile"));
            }
        }
        for (name, pool) in &self.pools {
            if pool.members.is_empty() {
                out.push(format!("pools.{name}.members: a pool needs at least one profile"));
            }
            for member in &pool.members {
                if !self.profiles.contains_key(member) {
                    out.push(format!("pools.{name}.members: `{member}` is not a profile (pools cannot contain pools)"));
                }
            }
        }
        if let Some(advisor) = &self.roles.advisor {
            if self.pools.contains_key(advisor) {
                out.push(format!("roles.advisor: `{advisor}` is a pool; the advisor is one profile, so that a mission keeps one planner from start to end"));
            } else if !self.profiles.contains_key(advisor) {
                out.push(format!("roles.advisor: `{advisor}` is not a profile"));
            }
        }
        for executor in &self.roles.executors {
            if !self.profiles.contains_key(executor) && !self.pools.contains_key(executor) {
                out.push(format!("roles.executors: `{executor}` is neither a profile nor a pool"));
            }
        }
        let cu = &self.capabilities.computer_use;
        match &cu.agent {
            Some(agent) => match self.profiles.get(agent) {
                None => out.push(format!("capabilities.computer_use.agent: `{agent}` is not a profile")),
                Some(p) if !p.provides.contains_key("computer_use") => out.push(format!(
                    "capabilities.computer_use.agent: profile `{agent}` does not declare `provides: {{computer_use: {{via: ...}}}}`; only an agent that supports computer use can serve it"
                )),
                Some(_) => {}
            },
            None => {}
        }
        if cu.max_concurrent == 0 {
            out.push("capabilities.computer_use.max_concurrent: must be at least 1".into());
        }
        out
    }
}

/// Whether a literal variable is more likely a credential than a setting.
fn looks_secret(key: &str, value: &str) -> bool {
    let key = key.to_ascii_uppercase();
    let named = ["_KEY", "_TOKEN", "_SECRET", "_PASSWORD", "_CREDENTIALS"].iter().any(|s| key.ends_with(s));
    let shaped = ["sk-", "ghp_", "gho_", "xoxb-", "xoxp-", "AKIA"].iter().any(|p| value.starts_with(p)) && value.len() >= 16;
    !value.is_empty() && (named || shaped)
}

/// Profiles that exist without any configuration. A configured profile of the same name wins.
pub fn presets() -> BTreeMap<String, Profile> {
    let mut out = BTreeMap::new();
    let mut add = |name: &str, adapter: &str, description: &str, edit: &dyn Fn(&mut Profile)| {
        let mut p = Profile { name: name.to_string(), adapter: adapter.to_string(), description: Some(description.to_string()), ..Profile::default() };
        edit(&mut p);
        out.insert(name.to_string(), p);
    };
    add("codex", "codex-cli", "Codex CLI with its default model", &|_| {});
    add("agy", "agy-cli", "Antigravity CLI with its default model", &|_| {});
    add("cursor", "cursor-cli", "Cursor agent, headless CLI", &|_| {});
    add("claude", "claude-cli", "Claude Code, headless CLI", &|_| {});
    add("kimi", "kimi-cli", "Kimi Code print mode (full-access only)", &|_| {});
    add("pi", "pi", "pi with its default model (read-only or full-access)", &|_| {});
    add("kimi-acp", "acp", "Kimi Code over ACP, so that Hufu answers permission requests", &|p| {
        p.command = vec!["kimi".into(), "acp".into()];
    });
    add("cursor-acp", "acp", "Cursor agent over ACP", &|p| {
        p.command = vec!["agent".into(), "acp".into()];
        p.auth_method = Some("cursor_login".into());
        p.env = vec!["CURSOR_API_KEY".into()];
    });
    out
}

/// Reads one YAML file into a JSON value. A missing or empty file is an empty mapping.
pub fn read_raw(path: &Path) -> Result<Value> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(json!({})),
        Err(e) => return Err(HufuError::internal(format!("{}: {e}", path.display()))),
    };
    parse_yaml(&text).map_err(|e| HufuError::usage(format!("{}: {e}", path.display())))
}

/// Parses YAML (or JSON, which is a subset) into a JSON value.
pub fn parse_yaml(text: &str) -> std::result::Result<Value, String> {
    if text.trim().is_empty() || text.lines().all(|l| l.trim().is_empty() || l.trim_start().starts_with('#')) {
        return Ok(json!({}));
    }
    let v: Value = serde_saphyr::from_str_with_options(text, yaml_options()).map_err(|e| e.to_string())?;
    Ok(if v.is_null() { json!({}) } else { v })
}

/// How Hufu reads YAML: only `true` and `false` are booleans, and numbers written in a way no
/// one writes a number stay text. A profile called `no` or a model called `0613` is a name.
fn yaml_options() -> serde_saphyr::Options {
    serde_saphyr::options! {
        strict_booleans: true,
        leading_zero_numbers_as_strings: true,
        underscored_numbers_as_strings: true,
    }
}

/// Serializes a value as YAML.
pub fn to_yaml(value: &Value) -> Result<String> {
    serde_saphyr::to_string(value).map_err(|e| HufuError::internal(format!("cannot write YAML: {e}")))
}

fn merge(base: &mut Value, over: Value, replace_children: bool) {
    match (base, over) {
        (Value::Object(b), Value::Object(o)) => {
            for (key, value) in o {
                let whole = replace_children;
                let named = key == "profiles" || key == "pools";
                match b.get_mut(&key) {
                    Some(existing) if !whole => merge(existing, value, named),
                    _ => {
                        b.insert(key, value);
                    }
                }
            }
        }
        (b, o) => *b = o,
    }
}

fn from_value(value: Value) -> std::result::Result<Config, String> {
    let mut config: Config = serde_json::from_value(value).map_err(|e| e.to_string())?;
    for (name, profile) in config.profiles.iter_mut() {
        profile.name = name.clone();
    }
    Ok(config)
}

/// The result of loading configuration for a directory.
#[derive(Clone, Debug)]
pub struct Loaded {
    pub config: Config,
    pub global_path: PathBuf,
    pub project_path: Option<PathBuf>,
    /// Names of profiles that come from the built-in presets rather than a file.
    pub builtin: BTreeSet<String>,
}

fn find_project_config(cwd: &Path, global: &Path) -> Option<PathBuf> {
    for dir in cwd.ancestors() {
        let candidate = dir.join(".hufu").join("config.yaml");
        if candidate.is_file() && candidate != global {
            return Some(candidate);
        }
    }
    None
}

/// Loads the user-wide file, overlays the project file found from `cwd` upwards, and adds presets.
pub fn load(home: &Home, cwd: &Path) -> Result<Loaded> {
    let global_path = home.config_file();
    let mut merged = read_raw(&global_path)?;
    let project_path = find_project_config(cwd, &global_path);
    if let Some(project) = &project_path {
        merge(&mut merged, read_raw(project)?, false);
    }
    let mut config = from_value(merged).map_err(|e| {
        let files = match &project_path {
            Some(p) => format!("{} or {}", global_path.display(), p.display()),
            None => global_path.display().to_string(),
        };
        HufuError::usage(format!("invalid configuration in {files}: {e}"))
    })?;
    let mut builtin = BTreeSet::new();
    for (name, preset) in presets() {
        if !config.profiles.contains_key(&name) && !config.pools.contains_key(&name) {
            builtin.insert(name.clone());
            config.profiles.insert(name, preset);
        }
    }
    Ok(Loaded { config, global_path, project_path, builtin })
}

/// Checks a raw configuration document the same way `load` would, including cross-references.
pub fn check_raw(raw: &Value, adapters: &dyn AdapterRegistry) -> std::result::Result<Config, Vec<String>> {
    let mut config = from_value(raw.clone()).map_err(|e| vec![e])?;
    for (name, preset) in presets() {
        if !config.profiles.contains_key(&name) && !config.pools.contains_key(&name) {
            config.profiles.insert(name, preset);
        }
    }
    let problems = config.problems(adapters);
    if problems.is_empty() { Ok(config) } else { Err(problems) }
}

const HEADER: &str = "# Hufu configuration. Reference: doc/technical/technical-design.md, section 3.\n\
# Edit by hand, with `hufu config set` / `hufu profile add`, or through the MCP tools.\n\
# This file holds names of environment variables, never their values.\n";

/// Validates and writes a raw configuration document. Nothing is written if it is invalid.
pub fn write_raw(path: &Path, raw: &Value, base: Option<&Value>, adapters: &dyn AdapterRegistry) -> Result<()> {
    // A project file is judged together with the user-wide file it overlays.
    let effective = match base {
        Some(base) => {
            let mut merged = base.clone();
            merge(&mut merged, raw.clone(), false);
            merged
        }
        None => raw.clone(),
    };
    if let Err(problems) = check_raw(&effective, adapters) {
        return Err(HufuError::usage(format!("the change would make the configuration invalid:\n  {}", problems.join("\n  "))));
    }
    let mut doc = raw.clone();
    if let Some(map) = doc.as_object_mut() {
        map.entry("version").or_insert(json!(1));
    }
    let text = format!("{HEADER}\n{}", to_yaml(&ordered(&doc))?);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).at(parent)?;
    }
    crate::store::write_atomic(path, text.as_bytes())
}

/// Puts top-level sections in the order a reader expects.
fn ordered(doc: &Value) -> Value {
    let Some(map) = doc.as_object() else { return doc.clone() };
    let mut out = Map::new();
    for key in ["version", "defaults", "limits", "roles", "profiles", "pools", "missions", "capabilities", "retention"] {
        if let Some(v) = map.get(key) {
            out.insert(key.to_string(), v.clone());
        }
    }
    for (k, v) in map {
        out.entry(k.clone()).or_insert(v.clone());
    }
    Value::Object(out)
}

/// Sets `a.b.c` in a raw document, creating mappings on the way.
pub fn set_path(raw: &mut Value, dotted: &str, value: Value) -> Result<()> {
    let parts: Vec<&str> = dotted.split('.').filter(|p| !p.is_empty()).collect();
    if parts.is_empty() {
        return Err(HufuError::usage("empty configuration key"));
    }
    let mut cursor = raw;
    for part in &parts[..parts.len() - 1] {
        if !cursor.is_object() {
            *cursor = json!({});
        }
        cursor = cursor.as_object_mut().expect("just made an object").entry(part.to_string()).or_insert(json!({}));
    }
    if !cursor.is_object() {
        *cursor = json!({});
    }
    cursor.as_object_mut().expect("just made an object").insert(parts[parts.len() - 1].to_string(), value);
    Ok(())
}

/// Removes `a.b.c` from a raw document. Returns whether it was there.
pub fn remove_path(raw: &mut Value, dotted: &str) -> bool {
    let parts: Vec<&str> = dotted.split('.').filter(|p| !p.is_empty()).collect();
    let Some((last, parents)) = parts.split_last() else { return false };
    let mut cursor = raw;
    for part in parents {
        match cursor.get_mut(*part) {
            Some(next) => cursor = next,
            None => return false,
        }
    }
    cursor.as_object_mut().map(|m| m.remove(*last).is_some()).unwrap_or(false)
}

/// Reads `a.b.c` from a value.
pub fn get_path<'a>(value: &'a Value, dotted: &str) -> Option<&'a Value> {
    dotted.split('.').filter(|p| !p.is_empty()).try_fold(value, |v, part| v.get(part))
}

/// Parses a command-line value as YAML, so `4`, `true`, `[a, b]` and `{k: v}` keep their types.
pub fn parse_scalar(text: &str) -> Value {
    match serde_saphyr::from_str_with_options::<Value>(text, yaml_options()) {
        Ok(v) if !v.is_null() || text.trim() == "null" || text.trim() == "~" => v,
        _ => Value::String(text.to_string()),
    }
}

#[cfg(test)]
pub(crate) mod tests_support {
    use hufu_contract::{Adapter, AdapterRegistry, Profile};

    pub struct Kinds;
    struct Fake(&'static str, Option<&'static str>);
    impl Adapter for Fake {
        fn kind(&self) -> &'static str {
            self.0
        }
        fn default_command(&self) -> Vec<String> {
            Vec::new()
        }
        fn home_var(&self) -> Option<&'static str> {
            self.1
        }
        fn probe(&self, _: &Profile) -> hufu_contract::ProbeResult {
            Default::default()
        }
        fn capabilities(&self, _: &Profile) -> hufu_contract::Capabilities {
            unimplemented!()
        }
        fn plan(&self, _: &Profile, _: &hufu_contract::RunRequest) -> std::result::Result<hufu_contract::LaunchPlan, hufu_contract::AdapterError> {
            unimplemented!()
        }
        fn run(&self, _: &Profile, _: &hufu_contract::RunRequest, _: &dyn hufu_contract::EventSink, _: &hufu_contract::CancelToken) -> hufu_contract::RunOutcome {
            unimplemented!()
        }
    }
    static FAKES: [Fake; 8] = [
        Fake("codex-cli", Some("CODEX_HOME")),
        Fake("agy-cli", None),
        Fake("cursor-cli", Some("CURSOR_CONFIG_DIR")),
        Fake("kimi-cli", Some("KIMI_CODE_HOME")),
        Fake("pi", Some("PI_CODING_AGENT_DIR")),
        Fake("claude-cli", Some("CLAUDE_CONFIG_DIR")),
        Fake("acp", None),
        Fake("external", None),
    ];
    impl AdapterRegistry for Kinds {
        fn get(&self, kind: &str) -> Option<&dyn Adapter> {
            FAKES.iter().find(|f| f.0 == kind).map(|f| f as &dyn Adapter)
        }
        fn kinds(&self) -> Vec<&'static str> {
            FAKES.iter().map(|f| f.0).collect()
        }
    }

    /// A registry that knows every built-in adapter kind but cannot run anything.
    pub fn kinds() -> &'static dyn AdapterRegistry {
        &Kinds
    }
}

#[cfg(test)]
mod tests {
    use super::tests_support::Kinds;
    use super::*;

    const SAMPLE: &str = r#"
version: 1
defaults:
  permission: read-only
  timeout: 45m
limits:
  max_concurrent_jobs: 4
  per_profile:
    codex-work: 2
roles:
  advisor: or-reasoner
  executors: [codex, kimi-acp]
profiles:
  codex-work:
    adapter: codex-cli
    account: work
    home: ~/.codex-work
    effort: high
  codex-personal:
    adapter: codex-cli
    account: personal
    home: ~/.codex-personal
  or-reasoner:
    adapter: pi
    provider: openrouter
    model: vendor/model-slug
    env: [OPENROUTER_API_KEY]
  operator:
    adapter: acp
    command: [some-agent, acp]
    provides:
      computer_use: { via: lcu }
pools:
  codex:
    members: [codex-work, codex-personal]
    strategy: least-busy
capabilities:
  computer_use:
    route: dedicated
    agent: operator
"#;

    #[test]
    fn parses_validates_and_resolves_pools() {
        let config = check_raw(&parse_yaml(SAMPLE).unwrap(), &Kinds).unwrap();
        assert_eq!(config.defaults.timeout, "45m");
        assert_eq!(config.profiles["codex-work"].effort, Some(Effort::High));
        assert_eq!(config.profiles["codex-work"].name, "codex-work");
        match config.agent("codex").unwrap() {
            AgentRef::Pool { members, strategy, .. } => {
                assert_eq!(members.len(), 2);
                assert_eq!(strategy, Strategy::LeastBusy);
            }
            other => panic!("expected a pool, got {other:?}"),
        }
        assert!(matches!(config.agent("agy").unwrap(), AgentRef::Profile(_)), "presets fill in unconfigured agents");
        let err = config.agent("nope").unwrap_err();
        assert!(err.message.contains("codex-work"));
    }

    #[test]
    fn problems_name_the_field() {
        let raw = parse_yaml(
            "profiles:\n  a: {adapter: nope}\n  b: {adapter: agy-cli, home: /x, permission: full-access}\n  c: {adapter: acp}\npools:\n  p: {members: [zzz]}\nroles: {advisor: ghost}\ncapabilities: {computer_use: {agent: b}}\n",
        )
        .unwrap();
        let problems = check_raw(&raw, &Kinds).unwrap_err().join("\n");
        for needle in [
            "profiles.a.adapter: unknown adapter",
            "profiles.b.permission",
            "profiles.b.home",
            "profiles.c.command",
            "pools.p.members: `zzz`",
            "roles.advisor",
            "capabilities.computer_use.agent: profile `b` does not declare",
        ] {
            assert!(problems.contains(needle), "missing `{needle}` in:\n{problems}");
        }
        let unknown = check_raw(&parse_yaml("defaults: {tiemout: 3m}").unwrap(), &Kinds).unwrap_err().join("\n");
        assert!(unknown.contains("tiemout"), "{unknown}");
    }

    #[test]
    fn project_file_overrides_and_replaces_whole_profiles() {
        let dir = std::env::temp_dir().join(format!("hufu-config-{}", hufu_contract::time::new_id("t")));
        let home = Home::at(dir.join("home"));
        let project = dir.join("proj");
        std::fs::create_dir_all(project.join(".hufu")).unwrap();
        std::fs::create_dir_all(&home.root).unwrap();
        std::fs::write(home.config_file(), "defaults: {timeout: 10m, effort: low}\nprofiles:\n  x: {adapter: codex-cli, model: m1, effort: high}\n").unwrap();
        std::fs::write(project.join(".hufu/config.yaml"), "defaults: {timeout: 20m}\nprofiles:\n  x: {adapter: codex-cli, model: m2}\n").unwrap();
        let loaded = load(&home, &project.join("deeper/dir")).unwrap();
        assert_eq!(loaded.config.defaults.timeout, "20m");
        assert_eq!(loaded.config.defaults.effort, Effort::Low, "unset fields keep the global value");
        assert_eq!(loaded.config.profiles["x"].model.as_deref(), Some("m2"));
        assert_eq!(loaded.config.profiles["x"].effort, None, "a project profile replaces the global one entirely");
        assert!(loaded.builtin.contains("codex"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn raw_editing_round_trips_through_yaml() {
        let dir = std::env::temp_dir().join(format!("hufu-config-{}", hufu_contract::time::new_id("t")));
        let path = dir.join("config.yaml");
        let mut raw = json!({});
        set_path(&mut raw, "defaults.timeout", parse_scalar("45m")).unwrap();
        set_path(&mut raw, "limits.max_concurrent_jobs", parse_scalar("8")).unwrap();
        set_path(&mut raw, "profiles.fast", json!({"adapter": "agy-cli", "effort": "low"})).unwrap();
        write_raw(&path, &raw, None, &Kinds).unwrap();
        let back = read_raw(&path).unwrap();
        assert_eq!(get_path(&back, "limits.max_concurrent_jobs"), Some(&json!(8)));
        assert_eq!(get_path(&back, "profiles.fast.effort"), Some(&json!("low")));
        assert!(std::fs::read_to_string(&path).unwrap().starts_with("# Hufu configuration"));
        let mut bad = back.clone();
        set_path(&mut bad, "profiles.fast.adapter", json!("nope")).unwrap();
        assert!(write_raw(&path, &bad, None, &Kinds).unwrap_err().message.contains("unknown adapter"));
        assert!(remove_path(&mut raw, "profiles.fast"));
        assert!(!remove_path(&mut raw, "profiles.fast"));
        assert_eq!(parse_scalar("[a, b]"), json!(["a", "b"]));
        assert_eq!(parse_scalar("true"), json!(true));
        assert_eq!(parse_scalar("plain text"), json!("plain text"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod yaml_gotchas {
    use super::*;

    #[test]
    fn retry_on_is_a_string_key() {
        let v = parse_yaml("defaults:\n  retry:\n    max_attempts: 3\n    on: [rate_limited, agent_error]\n    backoff: 5s\n").unwrap();
        let config: Config = serde_json::from_value(v).unwrap();
        assert_eq!(config.defaults.retry.on, ["rate_limited", "agent_error"]);
        // Words YAML 1.1 read as booleans are names here.
        assert_eq!(parse_yaml("a: [yes, no, on, off, y, true]").unwrap()["a"], json!(["yes", "no", "on", "off", "y", true]));
        assert_eq!(parse_yaml("model: 0613
count: 12").unwrap(), json!({"model": "0613", "count": 12}));
        assert_eq!(parse_scalar("no"), json!("no"));
        assert_eq!(parse_scalar("[a, b]"), json!(["a", "b"]));
        assert_eq!(parse_scalar("4"), json!(4));
    }
}
