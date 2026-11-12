//! Missions: an advisor profile plans the work, executor profiles do it, and the advisor reviews
//! the results. Hufu drives the loop, enforces the limits and keeps the ledger.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use hufu_contract::time::{fmt_duration, new_id, now_rfc3339, parse_duration, parse_rfc3339};
use hufu_contract::{AdapterRegistry, Effort, FORMAT_VERSION, Tier, Usage, effort_list, proc, tier_list};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::capability;
use crate::config::{AgentRef, Budgets, Config, Grant, Home, Isolation, ReviewPolicy};
use crate::error::{HufuError, IoContext, Result};
use crate::spec::{Check, JobSpec, OutputSpec, SchemaRef, TaskSpec};
use crate::store::{self, JobResult, State, Verdict};
use crate::submit::{self, Context, Options};
use crate::{runner, workspace};

/// Marker lines in advisor prompts. Test doubles key on them; real agents ignore them.
pub const PLAN_MARKER: &str = "hufu:mission-plan";
pub const REVIEW_MARKER: &str = "hufu:mission-review";

/// What a user writes in a mission file. Every field but the goal has a default in `config.yaml`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MissionSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<u32>,
    #[serde(default)]
    pub goal: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The profile that plans and reviews. Default: `roles.advisor`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub advisor: Option<String>,
    /// Profiles and pools the advisor may dispatch to. Default: `roles.executors`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub executors: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// `worktree` gives the whole mission one copy of the repository on its own branch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub isolation: Option<Isolation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review: Option<ReviewPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tier: Option<Tier>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budgets: Option<Budgets>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acceptance_commands: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_grants: Option<Vec<Grant>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_parallel: Option<u32>,
    /// Checks the user wants on every task that may write, on top of what the advisor asks for.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub acceptance: Vec<Check>,
}

impl MissionSpec {
    pub fn from_file(path: &Path) -> Result<MissionSpec> {
        let text = std::fs::read_to_string(path).map_err(|e| HufuError::usage(format!("mission file {}: {e}", path.display())))?;
        let value = crate::config::parse_yaml(&text).map_err(|e| HufuError::usage(format!("mission file {}: {e}", path.display())))?;
        serde_json::from_value(value).map_err(|e| HufuError::usage(format!("mission file {}: {e}", path.display())))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MissionState {
    Planning,
    Running,
    Paused,
    Succeeded,
    Failed,
    Aborted,
}

impl MissionState {
    pub fn is_terminal(self) -> bool {
        matches!(self, MissionState::Succeeded | MissionState::Failed | MissionState::Aborted)
    }
    pub fn as_str(self) -> &'static str {
        match self {
            MissionState::Planning => "planning",
            MissionState::Running => "running",
            MissionState::Paused => "paused",
            MissionState::Succeeded => "succeeded",
            MissionState::Failed => "failed",
            MissionState::Aborted => "aborted",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    Pending,
    Running,
    Reviewing,
    Accepted,
    Failed,
    Skipped,
}

impl TaskState {
    pub fn as_str(self) -> &'static str {
        match self {
            TaskState::Pending => "pending",
            TaskState::Running => "running",
            TaskState::Reviewing => "reviewing",
            TaskState::Accepted => "accepted",
            TaskState::Failed => "failed",
            TaskState::Skipped => "skipped",
        }
    }
    fn is_open(self) -> bool {
        matches!(self, TaskState::Pending | TaskState::Running | TaskState::Reviewing)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TaskRecord {
    pub id: String,
    pub title: String,
    pub agent: String,
    pub state: TaskState,
    /// Executor jobs, one per round.
    #[serde(default)]
    pub jobs: Vec<String>,
    /// Advisor review jobs.
    #[serde(default)]
    pub reviews: Vec<String>,
    #[serde(default)]
    pub rounds: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// One task of a plan, as the advisor writes it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanTask {
    pub id: String,
    pub title: String,
    pub prompt: String,
    pub agent: String,
    #[serde(default)]
    pub permission: Option<Tier>,
    #[serde(default)]
    pub effort: Option<Effort>,
    #[serde(default, deserialize_with = "null_default")]
    pub depends_on: Vec<String>,
    #[serde(default, deserialize_with = "null_default")]
    pub acceptance: Vec<Check>,
    #[serde(default, deserialize_with = "null_default")]
    pub grants: Vec<Grant>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Plan {
    #[serde(default = "one", deserialize_with = "null_one")]
    pub v: u32,
    #[serde(default, deserialize_with = "null_default")]
    pub summary: String,
    pub tasks: Vec<PlanTask>,
}

fn one() -> u32 {
    1
}

/// Reads `null` as the type's default: an advisor may write `null` for what it leaves open.
fn null_default<'de, D: serde::Deserializer<'de>, T: Default + Deserialize<'de>>(d: D) -> std::result::Result<T, D::Error> {
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

fn null_one<'de, D: serde::Deserializer<'de>>(d: D) -> std::result::Result<u32, D::Error> {
    Ok(Option::<u32>::deserialize(d)?.unwrap_or(1))
}

/// The limits a mission runs under, fixed when it starts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Policy {
    pub review: ReviewPolicy,
    pub max_tier: Tier,
    pub acceptance_commands: Vec<String>,
    pub allowed_grants: Vec<Grant>,
    pub budgets: Budgets,
    pub max_parallel: u32,
    #[serde(default)]
    pub acceptance: Vec<Check>,
}

/// `mission.json`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Mission {
    pub v: u32,
    pub id: String,
    pub title: String,
    pub goal: String,
    pub advisor: String,
    pub executors: Vec<String>,
    /// Where the user started the mission.
    pub cwd: String,
    /// Where the tasks run: `cwd`, or the mission's own worktree.
    pub workdir: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    pub state: MissionState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub policy: Policy,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub driver_pid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heartbeat_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default)]
    pub tasks: Vec<TaskRecord>,
    /// Advisor jobs that produced or repaired the plan.
    #[serde(default)]
    pub planning_jobs: Vec<String>,
    #[serde(default)]
    pub usage: Usage,
    /// Seconds the mission has been active, across pauses.
    #[serde(default)]
    pub active_s: u64,
}

fn file(home: &Home, id: &str, name: &str) -> PathBuf {
    home.mission(id).join(name)
}

pub fn load(home: &Home, id: &str) -> Result<Mission> {
    let path = file(home, id, "mission.json");
    if !path.exists() {
        return Err(HufuError::usage(format!("no such mission `{id}`; list missions with `hufu mission list`")));
    }
    store::read_json(&path)
}

pub fn save(home: &Home, mission: &Mission) -> Result<()> {
    store::write_json(&file(home, &mission.id, "mission.json"), mission)
}

pub fn load_plan(home: &Home, id: &str) -> Option<Plan> {
    store::read_json(&file(home, id, "plan.json")).ok()
}

pub fn list(home: &Home) -> Vec<Mission> {
    let mut ids: Vec<String> = std::fs::read_dir(home.missions()).into_iter().flatten().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    ids.sort();
    ids.iter().filter_map(|id| load(home, id).ok()).collect()
}

/// Accepts a full mission id or an unambiguous prefix.
pub fn resolve(home: &Home, partial: &str) -> Result<String> {
    if file(home, partial, "mission.json").exists() {
        return Ok(partial.to_string());
    }
    let hits: Vec<String> = list(home).into_iter().map(|m| m.id).filter(|id| id.starts_with(partial) || id.starts_with(&format!("m_{partial}"))).collect();
    match hits.len() {
        1 => Ok(hits.into_iter().next().unwrap_or_default()),
        0 => Err(HufuError::usage(format!("no such mission `{partial}`; list missions with `hufu mission list`"))),
        n => Err(HufuError::usage(format!("`{partial}` matches {n} missions; give more of the id"))),
    }
}

fn ledger(home: &Home, id: &str, event: &str, data: Value) {
    let path = file(home, id, "ledger.ndjson");
    let mut line = json!({"ts": now_rfc3339(), "event": event});
    if let (Some(obj), Some(extra)) = (line.as_object_mut(), data.as_object()) {
        for (k, v) in extra {
            obj.insert(k.clone(), v.clone());
        }
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "{line}");
    }
}

/// Reads the ledger of a mission.
pub fn read_ledger(home: &Home, id: &str) -> Vec<Value> {
    std::fs::read_to_string(file(home, id, "ledger.ndjson")).map(|t| t.lines().filter_map(|l| serde_json::from_str(l).ok()).collect()).unwrap_or_default()
}

/// Validates a mission request and writes its record. Nothing runs yet.
pub fn create(ctx: &Context, spec: &MissionSpec, base_dir: &Path) -> Result<Mission> {
    ctx.config.ensure_valid(ctx.adapters)?;
    if let Some(v) = spec.version {
        if v != FORMAT_VERSION {
            return Err(HufuError::usage(format!("mission file version {v} is not supported; this build reads version {FORMAT_VERSION}")));
        }
    }
    if spec.goal.trim().is_empty() {
        return Err(HufuError::usage("the mission has no goal; pass it as an argument or as `goal:` in a mission file"));
    }
    let config = ctx.config;
    let advisor = spec.advisor.clone().or_else(|| config.roles.advisor.clone()).ok_or_else(|| {
        HufuError::usage("no advisor: pass `--advisor <profile>`, or set a default once with `hufu config set roles.advisor <profile>`")
    })?;
    let Some(advisor_profile) = config.profile(&advisor) else {
        return Err(HufuError::usage(format!("advisor `{advisor}` is not a profile; an advisor is one profile, not a pool (see `hufu agents`)")));
    };
    let advisor_caps = ctx.adapters.get(&advisor_profile.adapter).map(|a| a.capabilities(advisor_profile));
    let Some(advisor_caps) = advisor_caps else {
        return Err(HufuError::usage(format!("advisor `{advisor}` uses unknown adapter `{}`", advisor_profile.adapter)));
    };
    if advisor_caps.permission_tiers.is_empty() {
        return Err(HufuError::usage(format!("advisor `{advisor}` cannot run at any permission tier")));
    }
    let executors = if spec.executors.is_empty() { config.roles.executors.clone() } else { spec.executors.clone() };
    if executors.is_empty() {
        return Err(HufuError::usage("no executors: pass `--executors a,b`, or set a default roster with `hufu config set roles.executors \"[a, b]\"`"));
    }
    for name in &executors {
        config.agent(name)?;
    }
    let raw_cwd = proc::expand_tilde(spec.cwd.as_deref().unwrap_or("."));
    let joined = if raw_cwd.is_absolute() { raw_cwd } else { base_dir.join(raw_cwd) };
    let cwd = std::fs::canonicalize(&joined).map_err(|e| HufuError::usage(format!("cwd {}: {e}", joined.display())))?;
    let m = &config.missions;
    let policy = Policy {
        review: spec.review.unwrap_or(m.review),
        max_tier: spec.max_tier.unwrap_or(m.max_tier),
        acceptance_commands: spec.acceptance_commands.clone().unwrap_or_else(|| m.acceptance_commands.clone()),
        allowed_grants: spec.allowed_grants.clone().unwrap_or_else(|| m.allowed_grants.clone()),
        budgets: spec.budgets.clone().unwrap_or_else(|| m.budgets.clone()),
        max_parallel: spec.max_parallel.unwrap_or(m.max_parallel).max(1),
        acceptance: spec.acceptance.clone(),
    };
    parse_duration(&policy.budgets.max_wall_clock).map_err(|e| HufuError::usage(format!("budgets.max_wall_clock: {e}")))?;
    for (i, check) in policy.acceptance.iter().enumerate() {
        check.kind().map_err(|e| HufuError::usage(format!("acceptance[{i}]: {e}")))?;
    }
    let id = new_id("m");
    let mut mission = Mission {
        v: FORMAT_VERSION,
        id: id.clone(),
        title: spec.title.clone().filter(|t| !t.trim().is_empty()).unwrap_or_else(|| crate::spec::title_from_prompt(&spec.goal)),
        goal: spec.goal.trim().to_string(),
        advisor,
        executors,
        cwd: cwd.display().to_string(),
        workdir: cwd.display().to_string(),
        branch: None,
        base: None,
        state: MissionState::Planning,
        reason: None,
        detail: None,
        policy,
        created_at: now_rfc3339(),
        finished_at: None,
        driver_pid: None,
        heartbeat_at: None,
        summary: None,
        tasks: Vec::new(),
        planning_jobs: Vec::new(),
        usage: Usage::default(),
        active_s: 0,
    };
    ctx.home.ensure()?;
    let dir = ctx.home.mission(&id);
    std::fs::create_dir_all(&dir).at(&dir)?;
    if spec.isolation.unwrap_or(Isolation::None) == Isolation::Worktree {
        let wt = workspace::create_worktree(ctx.home, &cwd, &id)?;
        mission.workdir = wt.run_dir.display().to_string();
        mission.branch = Some(wt.branch);
        mission.base = Some(wt.base);
    }
    save(ctx.home, &mission)?;
    ledger(ctx.home, &id, "mission.created", json!({"advisor": mission.advisor, "executors": mission.executors, "goal": mission.goal}));
    Ok(mission)
}

/// Starts the detached driver of a mission.
pub fn launch(home: &Home, id: &str) -> Result<u32> {
    submit::spawn_detached(home, &["__mission", id], &file(home, id, "driver.log"))
}

// ---------------------------------------------------------------------------------------------
// Prompts and schemas
// ---------------------------------------------------------------------------------------------

/// The plan an advisor writes. Every property is required and the optional ones admit `null`,
/// which is what agents with strict structured output need; see `schema::fill_nullable` for the
/// agents that leave properties out.
fn plan_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["v", "summary", "tasks"],
        "properties": {
            "v": {"type": ["integer", "null"]},
            "summary": {"type": ["string", "null"]},
            "tasks": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["id", "title", "prompt", "agent", "permission", "effort", "depends_on", "acceptance", "grants"],
                    "properties": {
                        "id": {"type": "string"},
                        "title": {"type": "string"},
                        "prompt": {"type": "string"},
                        "agent": {"type": "string"},
                        "permission": {"type": ["string", "null"]},
                        "effort": {"type": ["string", "null"]},
                        "depends_on": {"type": ["array", "null"], "items": {"type": "string"}},
                        "acceptance": {"type": ["array", "null"], "items": {"type": "object", "additionalProperties": false, "required": ["command"], "properties": {"command": {"type": "string"}}}},
                        "grants": {"type": ["array", "null"], "items": {"type": "object", "additionalProperties": false, "required": ["capability", "apps"], "properties": {"capability": {"type": "string"}, "apps": {"type": "array", "items": {"type": "string"}}}}}
                    }
                }
            }
        }
    })
}

fn review_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["v", "verdict", "reason", "follow_up"],
        "properties": {
            "v": {"type": ["integer", "null"]},
            "verdict": {"type": "string", "enum": ["accept", "revise", "abort"]},
            "reason": {"type": "string"},
            "follow_up": {"type": ["string", "null"]}
        }
    })
}

/// One line per roster entry: what the advisor needs to know to pick an executor.
pub fn roster(config: &Config, adapters: &dyn AdapterRegistry, executors: &[String]) -> String {
    let mut out = String::new();
    let describe = |name: &str| -> String {
        let Some(p) = config.profile(name) else { return String::new() };
        let Some(adapter) = adapters.get(&p.adapter) else { return String::new() };
        let caps = adapter.capabilities(p);
        let mut parts = vec![format!("adapter {}", p.adapter), format!("tiers: {}", tier_list(&caps.permission_tiers)), format!("effort: {}", effort_list(&caps.efforts))];
        if let Some(m) = &p.model {
            parts.push(format!("model {m}"));
        }
        if let Some(a) = &p.account {
            parts.push(format!("account {a}"));
        }
        if caps.resume {
            parts.push("can take follow-ups".into());
        }
        if p.provides.contains_key(capability::COMPUTER_USE) {
            parts.push("can operate desktop applications".into());
        }
        parts.join("; ")
    };
    for name in executors {
        match config.agent(name) {
            Ok(AgentRef::Profile(p)) => {
                out.push_str(&format!("- `{name}`: {}. {}\n", p.description.clone().unwrap_or_else(|| "no description".into()), describe(name)));
            }
            Ok(AgentRef::Pool { members, .. }) => {
                let pool = config.pools.get(name);
                let first = members.first().map(|m| describe(&m.name)).unwrap_or_default();
                out.push_str(&format!(
                    "- `{name}`: {} A pool of {} interchangeable profiles ({}); Hufu picks one and fails over. {first}\n",
                    pool.and_then(|p| p.description.clone()).map(|d| format!("{d}.")).unwrap_or_default(),
                    members.len(),
                    members.iter().map(|m| m.name.as_str()).collect::<Vec<_>>().join(", ")
                ));
            }
            Err(_) => {}
        }
    }
    out
}

fn plan_prompt(mission: &Mission, config: &Config, adapters: &dyn AdapterRegistry, errors: Option<&[String]>) -> String {
    let p = &mission.policy;
    let commands = if p.acceptance_commands.is_empty() {
        "Acceptance commands are not enabled for this mission: leave every `acceptance` list empty.".to_string()
    } else {
        format!("An acceptance check is a shell command that must exit 0. It must start with one of: {}.", p.acceptance_commands.iter().map(|c| format!("`{c}`")).collect::<Vec<_>>().join(", "))
    };
    let mut text = format!(
        "<!-- {PLAN_MARKER} -->\n\
         You are the advisor of a Hufu mission. You plan the work; other agents carry it out and you review their results later.\n\
         You are running read-only: inspect the workspace as needed, but change nothing.\n\n\
         ## Goal\n{goal}\n\n\
         ## Roster (the only agents you may assign)\n{roster}\n\
         ## Rules\n\
         - At most {max_tasks} tasks. Fewer, larger tasks are better than many small ones.\n\
         - Every `agent` must be a name from the roster.\n\
         - `permission` is one of read-only, workspace-write, full-access, never above `{max_tier}`. Ask for the lowest tier that works, or null for the agent's default.\n\
         - `effort` is one of low, medium, high, max, or null. Use a level the chosen agent supports.\n\
         - `depends_on` lists task ids that must be accepted first. No cycles.\n\
         - Each `prompt` must stand alone: the executor sees nothing but that prompt and the workspace.\n\
         - {commands}\n\n\
         ## Answer\n\
         Answer with one JSON object and nothing else:\n\
         {{\"v\": 1, \"summary\": \"...\", \"tasks\": [{{\"id\": \"t1\", \"title\": \"...\", \"prompt\": \"...\", \"agent\": \"...\", \"permission\": null, \"effort\": null, \"depends_on\": [], \"acceptance\": []}}]}}\n",
        goal = mission.goal,
        roster = roster(config, adapters, &mission.executors),
        max_tasks = p.budgets.max_tasks,
        max_tier = p.max_tier,
    );
    if let Some(errors) = errors {
        text.push_str("\n## Your previous plan was rejected\nFix these problems and answer with the complete corrected plan:\n");
        for e in errors {
            text.push_str(&format!("- {e}\n"));
        }
    }
    text
}

fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max { text.to_string() } else { format!("{}\n… (clipped)", text.chars().take(max).collect::<String>()) }
}

fn review_prompt(mission: &Mission, task: &PlanTask, record: &TaskRecord, result: Option<&JobResult>) -> String {
    let mut text = format!(
        "<!-- {REVIEW_MARKER} -->\n\
         You are the advisor of a Hufu mission, reviewing one task. You are running read-only: inspect the workspace as needed, but change nothing.\n\n\
         ## Goal of the mission\n{}\n\n## Task `{}`: {}\nAssigned to: {}\nRound: {}\n\n### What the executor was asked\n{}\n\n### Result\n",
        mission.goal,
        task.id,
        task.title,
        task.agent,
        record.rounds,
        clip(&task.prompt, 4000)
    );
    match result {
        None => text.push_str("The job left no result.\n"),
        Some(r) => {
            text.push_str(&format!("State: {}", r.state.as_str()));
            if let Some(why) = &r.reason {
                text.push_str(&format!(" ({why})"));
            }
            text.push_str(&format!("\nTally verdict: {}\n", r.verdict.as_str()));
            if let Some(d) = &r.detail {
                text.push_str(&format!("Detail: {}\n", clip(d, 1500)));
            }
            for t in &r.tally {
                text.push_str(&format!("- check `{}` {}: {} {}\n", t.check, t.description, if t.passed { "passed" } else { "FAILED" }, clip(&t.detail, 600)));
            }
            if !r.changes.files.is_empty() {
                let shown: Vec<&str> = r.changes.files.iter().take(60).map(String::as_str).collect();
                text.push_str(&format!("Changed files ({}): {}\n", r.changes.files.len(), shown.join(", ")));
            }
            if !r.denied_actions.is_empty() {
                let list: Vec<String> = r.denied_actions.iter().map(|d| format!("{} {}", d.kind, d.tool)).collect();
                text.push_str(&format!("Actions skipped for lack of permission: {}\n", list.join(", ")));
            }
            text.push_str(&format!("\n### The executor's final message\n{}\n", clip(r.final_message.as_deref().unwrap_or("(none)"), 6000)));
        }
    }
    text.push_str(
        "\n## Answer\nAnswer with one JSON object and nothing else:\n\
         {\"v\": 1, \"verdict\": \"accept\" | \"revise\" | \"abort\", \"reason\": \"...\", \"follow_up\": \"what the executor must change, when the verdict is revise, else null\"}\n\
         `accept` means the task is done. `revise` sends your follow-up to the executor. `abort` stops the whole mission.\n",
    );
    text
}

// ---------------------------------------------------------------------------------------------
// Plan validation
// ---------------------------------------------------------------------------------------------

/// Everything that is wrong with a plan. Empty means it can run.
pub fn check_plan(plan: &Plan, mission: &Mission, ctx: &Context) -> Vec<String> {
    let mut errors = Vec::new();
    let p = &mission.policy;
    if plan.tasks.is_empty() {
        errors.push("the plan has no tasks".into());
    }
    if plan.tasks.len() as u32 > p.budgets.max_tasks {
        errors.push(format!("the plan has {} tasks; the limit is {}", plan.tasks.len(), p.budgets.max_tasks));
    }
    let mut ids = BTreeSet::new();
    for task in &plan.tasks {
        let at = format!("task `{}`", task.id);
        if task.id.trim().is_empty() || !task.id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
            errors.push(format!("{at}: an id uses letters, digits, `-` and `_` only"));
        }
        if !ids.insert(task.id.clone()) {
            errors.push(format!("{at}: the id is used twice"));
        }
        if task.prompt.trim().is_empty() {
            errors.push(format!("{at}: the prompt is empty"));
        }
        if !mission.executors.contains(&task.agent) {
            errors.push(format!("{at}: agent `{}` is not on the roster ({})", task.agent, mission.executors.join(", ")));
            continue;
        }
        for check in &task.acceptance {
            match (check.kind(), &check.command) {
                (Ok("command"), Some(command)) => {
                    let ok = p.acceptance_commands.iter().any(|prefix| command.trim() == prefix.trim() || command.trim().starts_with(&format!("{} ", prefix.trim())));
                    let chained = ["&&", "||", ";", "|", "`", "$(", ">", "<", "\n"].iter().any(|t| command.contains(t));
                    if !ok || chained {
                        errors.push(format!(
                            "{at}: acceptance command `{command}` is not allowed; a command must start with one of [{}] and must not chain or redirect",
                            p.acceptance_commands.join(", ")
                        ));
                    }
                }
                (Ok(_), _) => errors.push(format!("{at}: an advisor may only write `command` acceptance checks")),
                (Err(e), _) => errors.push(format!("{at}: {e}")),
            }
        }
        if let Err(e) = capability::within_allowed(&task.grants, &p.allowed_grants) {
            errors.push(format!("{at}: {e}"));
        }
        // The same validation a task gets on `hufu run`.
        let spec = task_spec(mission, task, None);
        let opts = Options { base_dir: PathBuf::from(&mission.workdir), mission: Some(mission.id.clone()), max_tier: Some(p.max_tier), ..Options::default() };
        if let Err(e) = submit::prepare(ctx, &spec, &opts) {
            errors.push(format!("{at}: {}", e.message));
        }
    }
    for task in &plan.tasks {
        for dep in &task.depends_on {
            if !ids.contains(dep) {
                errors.push(format!("task `{}`: depends on `{dep}`, which is not in the plan", task.id));
            }
        }
    }
    // Cycles: repeatedly remove tasks whose dependencies are all removed.
    let mut left: BTreeMap<&str, Vec<&str>> = plan.tasks.iter().map(|t| (t.id.as_str(), t.depends_on.iter().map(String::as_str).filter(|d| ids.contains(*d)).collect())).collect();
    loop {
        let free: Vec<&str> = left.iter().filter(|(_, deps)| deps.iter().all(|d| !left.contains_key(d))).map(|(id, _)| *id).collect();
        if free.is_empty() {
            break;
        }
        for id in free {
            left.remove(id);
        }
    }
    if !left.is_empty() {
        errors.push(format!("the dependencies form a cycle among: {}", left.keys().copied().collect::<Vec<_>>().join(", ")));
    }
    errors
}

fn task_spec(mission: &Mission, task: &PlanTask, follow_up: Option<(&str, Option<&JobResult>)>) -> TaskSpec {
    let mut prompt = task.prompt.clone();
    if let Some((feedback, previous)) = follow_up {
        prompt.push_str("\n\n## A reviewer looked at the previous attempt and asks for changes\n");
        prompt.push_str(feedback);
        if let Some(message) = previous.and_then(|r| r.final_message.as_deref()) {
            prompt.push_str("\n\n## What the previous attempt reported\n");
            prompt.push_str(&clip(message, 3000));
        }
    }
    let writes = task.permission.map(|t| t >= Tier::WorkspaceWrite).unwrap_or(false);
    let mut acceptance = task.acceptance.clone();
    if writes {
        acceptance.extend(mission.policy.acceptance.iter().cloned());
    }
    let mut labels = BTreeMap::new();
    labels.insert("hufu.mission_task".to_string(), task.id.clone());
    TaskSpec {
        prompt,
        agent: task.agent.clone(),
        title: Some(task.title.clone()),
        cwd: Some(mission.workdir.clone()),
        permission: task.permission,
        effort: task.effort.clone(),
        acceptance,
        labels,
        grants: task.grants.clone(),
        ..TaskSpec::default()
    }
}

// ---------------------------------------------------------------------------------------------
// The driver
// ---------------------------------------------------------------------------------------------

/// Starts the runner of a held job. The binary spawns a process; tests run it on a thread.
pub type Starter<'a> = &'a (dyn Fn(&Home, &str) -> Result<()> + Sync);

struct Driver<'a> {
    ctx: Context<'a>,
    start: Starter<'a>,
    mission: Mission,
    began: Instant,
    base_active: u64,
}

enum Halt {
    Paused,
    Aborted(String),
    Failed(String, String),
}

impl Driver<'_> {
    fn home(&self) -> &Home {
        self.ctx.home
    }

    fn save(&mut self) {
        self.mission.heartbeat_at = Some(now_rfc3339());
        self.mission.active_s = self.base_active + self.began.elapsed().as_secs();
        let _ = save(self.ctx.home, &self.mission);
    }

    fn note(&self, event: &str, data: Value) {
        ledger(self.ctx.home, &self.mission.id, event, data);
    }

    fn marker(&self, name: &str) -> bool {
        file(self.ctx.home, &self.mission.id, name).exists()
    }

    fn add_usage(&mut self, job: &str) {
        if let Ok(Some(result)) = store::load_result(self.ctx.home, job) {
            if let Some(u) = &result.usage {
                self.mission.usage.add(u);
            }
        }
    }

    /// The first exhausted budget, if any.
    fn over_budget(&self) -> Option<String> {
        let b = &self.mission.policy.budgets;
        let limit = parse_duration(&b.max_wall_clock).unwrap_or(Duration::from_secs(3 * 3600));
        let active = self.base_active + self.began.elapsed().as_secs();
        if active > limit.as_secs() {
            return Some(format!("the mission has been active for {}, over its limit of {}", fmt_duration(active), b.max_wall_clock));
        }
        if let Some(max) = b.max_tokens {
            if self.mission.usage.tokens() > max {
                return Some(format!("{} tokens used, over the limit of {max}", self.mission.usage.tokens()));
            }
        }
        if let (Some(max), Some(cost)) = (b.max_cost, self.mission.usage.cost_usd) {
            if cost > max {
                return Some(format!("${cost:.2} spent, over the limit of ${max:.2}"));
            }
        }
        None
    }

    fn advisor_job(&mut self, prompt: String, schema: Value, title: &str, resume_from: Option<&str>) -> Result<String> {
        if let Some(previous) = resume_from {
            let resumable = store::load_result(self.home(), previous).ok().flatten().and_then(|r| r.session).map(|s| s.resumable).unwrap_or(false);
            if resumable {
                let edit = |spec: &mut JobSpec| {
                    spec.title = title.to_string();
                    spec.output_schema = Some(schema.clone());
                };
                if let Ok(state) = submit::resume(&self.ctx, previous, &prompt, true, edit) {
                    (self.start)(self.home(), &state.id)?;
                    return Ok(state.id);
                }
            }
        }
        let advisor = self.ctx.config.profile(&self.mission.advisor).cloned().ok_or_else(|| HufuError::usage(format!("advisor `{}` is no longer a profile", self.mission.advisor)))?;
        let caps = self.ctx.adapters.get(&advisor.adapter).map(|a| a.capabilities(&advisor)).ok_or_else(|| HufuError::usage("the advisor's adapter is unknown"))?;
        // The advisor only reads. An agent that cannot be held to read-only runs at its lowest tier.
        let tier = caps.permission_tiers.iter().copied().min().unwrap_or(Tier::ReadOnly);
        let mut labels = BTreeMap::new();
        labels.insert("hufu.role".to_string(), "advisor".to_string());
        let task = TaskSpec {
            prompt,
            agent: self.mission.advisor.clone(),
            title: Some(title.to_string()),
            cwd: Some(self.mission.workdir.clone()),
            permission: Some(tier),
            output: Some(OutputSpec { schema: SchemaRef::Inline(schema) }),
            labels,
            ..TaskSpec::default()
        };
        let opts = Options { base_dir: PathBuf::from(&self.mission.workdir), mission: Some(self.mission.id.clone()), hold: true, ..Options::default() };
        let state = submit::submit(&self.ctx, &task, &opts)?;
        (self.start)(self.home(), &state.id)?;
        Ok(state.id)
    }

    /// Waits for a job while watching for abort requests and budgets.
    fn wait(&mut self, job: &str) -> std::result::Result<(), Halt> {
        let mut last_save = Instant::now();
        loop {
            match store::load_state(self.home(), job) {
                Ok(s) if s.state.is_terminal() => return Ok(()),
                Ok(_) => {}
                Err(e) => return Err(Halt::Failed("internal_error".into(), e.message)),
            }
            if self.marker("abort.request") {
                let _ = runner::cancel(self.home(), job);
                return Err(Halt::Aborted("aborted on request".into()));
            }
            if let Some(why) = self.over_budget() {
                let _ = runner::cancel(self.home(), job);
                return Err(Halt::Failed("budget_exceeded".into(), why));
            }
            if last_save.elapsed() > Duration::from_secs(5) {
                self.save();
                last_save = Instant::now();
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    fn plan(&mut self) -> std::result::Result<Plan, Halt> {
        if let Some(plan) = load_plan(self.home(), &self.mission.id) {
            return Ok(plan);
        }
        let fail = |why: &str, detail: String| Halt::Failed(why.to_string(), detail);
        let mut errors: Option<Vec<String>> = None;
        for attempt in 0..2 {
            let prompt = plan_prompt(&self.mission, self.ctx.config, self.ctx.adapters, errors.as_deref());
            let previous = self.mission.planning_jobs.last().cloned();
            let job = self.advisor_job(prompt, plan_schema(), &format!("Plan: {}", self.mission.title), previous.as_deref()).map_err(|e| fail("advisor_failed", e.message))?;
            self.mission.planning_jobs.push(job.clone());
            self.save();
            self.note("plan.requested", json!({"job": job, "attempt": attempt + 1}));
            self.wait(&job)?;
            self.add_usage(&job);
            let result = store::load_result(self.home(), &job).ok().flatten();
            let Some(result) = result.filter(|r| r.state == State::Succeeded) else {
                let state = store::load_state(self.home(), &job).ok();
                let why = state.as_ref().and_then(|s| s.detail.clone().or(s.reason.clone())).unwrap_or_else(|| "the advisor job did not succeed".into());
                // A plan that is not valid JSON gets the same second chance as an invalid plan.
                if state.as_ref().and_then(|s| s.reason.as_deref()) == Some(hufu_contract::reason::INVALID_OUTPUT) && attempt == 0 {
                    errors = Some(vec![why]);
                    continue;
                }
                return Err(fail("advisor_failed", format!("the advisor could not produce a plan (job {job}): {why}")));
            };
            let value = result.structured_output.clone().unwrap_or(Value::Null);
            let parsed: std::result::Result<Plan, String> = serde_json::from_value(value).map_err(|e| format!("the plan does not have the expected shape: {e}"));
            let problems = match &parsed {
                Ok(plan) => check_plan(plan, &self.mission, &self.ctx),
                Err(e) => vec![e.clone()],
            };
            if let (Ok(plan), true) = (parsed, problems.is_empty()) {
                let _ = store::write_json(&file(self.home(), &self.mission.id, "plan.json"), &plan);
                self.mission.summary = Some(plan.summary.clone());
                self.mission.tasks = plan.tasks.iter().map(|t| TaskRecord { id: t.id.clone(), title: t.title.clone(), agent: t.agent.clone(), state: TaskState::Pending, jobs: Vec::new(), reviews: Vec::new(), rounds: 0, note: None }).collect();
                self.note("plan.accepted", json!({"tasks": plan.tasks.len(), "summary": plan.summary}));
                return Ok(plan);
            }
            self.note("plan.rejected", json!({"job": job, "errors": problems}));
            errors = Some(problems);
        }
        Err(fail("plan_invalid", format!("the advisor's plan was invalid twice: {}", errors.unwrap_or_default().join("; "))))
    }

    fn start_task(&mut self, index: usize, task: &PlanTask, follow_up: Option<(String, Option<JobResult>)>) -> Result<()> {
        // A follow-up goes to the same session when the agent can take one.
        let previous = self.mission.tasks[index].jobs.last().cloned();
        let mut started: Option<String> = None;
        if let (Some((feedback, _)), Some(prev)) = (&follow_up, &previous) {
            let resumable = store::load_result(self.home(), prev).ok().flatten().and_then(|r| r.session).map(|s| s.resumable).unwrap_or(false);
            if resumable {
                if let Ok(state) = submit::resume(&self.ctx, prev, &format!("A reviewer looked at your work and asks for changes:\n{feedback}"), true, |spec| spec.title = format!("{} (revision)", task.title)) {
                    started = Some(state.id);
                }
            }
        }
        let id = match started {
            Some(id) => id,
            None => {
                let spec = task_spec(&self.mission, task, follow_up.as_ref().map(|(f, r)| (f.as_str(), r.as_ref())));
                let opts = Options { base_dir: PathBuf::from(&self.mission.workdir), mission: Some(self.mission.id.clone()), max_tier: Some(self.mission.policy.max_tier), hold: true, ..Options::default() };
                submit::submit(&self.ctx, &spec, &opts)?.id
            }
        };
        (self.start)(self.home(), &id)?;
        let record = &mut self.mission.tasks[index];
        record.jobs.push(id.clone());
        record.rounds += 1;
        record.state = TaskState::Running;
        let round = record.rounds;
        self.note("task.started", json!({"task": task.id, "job": id, "round": round, "agent": task.agent}));
        self.save();
        Ok(())
    }

    fn settle(&mut self, index: usize, state: TaskState, note: Option<String>) {
        let record = &mut self.mission.tasks[index];
        record.state = state;
        record.note = note.clone();
        let id = record.id.clone();
        self.note(&format!("task.{}", state.as_str()), json!({"task": id, "note": note}));
        self.save();
    }

    /// One pass over every open task. Returns whether anything is still open.
    fn step(&mut self, plan: &Plan) -> std::result::Result<bool, Halt> {
        let policy = self.mission.policy.clone();
        for index in 0..self.mission.tasks.len() {
            let Some(task) = plan.tasks.iter().find(|t| t.id == self.mission.tasks[index].id) else { continue };
            match self.mission.tasks[index].state {
                TaskState::Running => {
                    let Some(job) = self.mission.tasks[index].jobs.last().cloned() else { continue };
                    let Ok(state) = store::load_state(self.home(), &job) else { continue };
                    if !state.state.is_terminal() {
                        continue;
                    }
                    self.add_usage(&job);
                    let good = state.state == State::Succeeded && state.verdict != Verdict::Rejected;
                    let wants_review = task.acceptance.iter().any(|c| c.review.is_some());
                    let review = match policy.review {
                        ReviewPolicy::Always => true,
                        ReviewPolicy::OnFailure => !good || wants_review,
                        ReviewPolicy::Never => false,
                    };
                    self.note("task.finished", json!({"task": task.id, "job": job, "state": state.state.as_str(), "verdict": state.verdict.as_str(), "reason": state.reason}));
                    if !review {
                        let note = (!good).then(|| format!("job {job} ended as {}{}", state.state.as_str(), state.reason.as_ref().map(|r| format!(" ({r})")).unwrap_or_default()));
                        self.settle(index, if good { TaskState::Accepted } else { TaskState::Failed }, note);
                        continue;
                    }
                    let result = store::load_result(self.home(), &job).ok().flatten();
                    let prompt = review_prompt(&self.mission, task, &self.mission.tasks[index], result.as_ref());
                    let previous = self.mission.tasks[index].reviews.last().cloned().or_else(|| self.mission.planning_jobs.last().cloned());
                    match self.advisor_job(prompt, review_schema(), &format!("Review {}: {}", task.id, task.title), previous.as_deref()) {
                        Ok(review_job) => {
                            let record = &mut self.mission.tasks[index];
                            record.reviews.push(review_job.clone());
                            record.state = TaskState::Reviewing;
                            self.note("review.requested", json!({"task": task.id, "job": review_job}));
                            self.save();
                        }
                        Err(e) => self.settle(index, if good { TaskState::Accepted } else { TaskState::Failed }, Some(format!("the review could not be started: {}", e.message))),
                    }
                }
                TaskState::Reviewing => {
                    let Some(review_job) = self.mission.tasks[index].reviews.last().cloned() else { continue };
                    let Ok(review_state) = store::load_state(self.home(), &review_job) else { continue };
                    if !review_state.state.is_terminal() {
                        continue;
                    }
                    self.add_usage(&review_job);
                    let job = self.mission.tasks[index].jobs.last().cloned().unwrap_or_default();
                    let job_state = store::load_state(self.home(), &job).ok();
                    let good = job_state.as_ref().map(|s| s.state == State::Succeeded && s.verdict != Verdict::Rejected).unwrap_or(false);
                    let answer = store::load_result(self.home(), &review_job).ok().flatten().filter(|r| r.state == State::Succeeded).and_then(|r| r.structured_output);
                    let verdict = answer.as_ref().and_then(|a| a.get("verdict")).and_then(Value::as_str).unwrap_or("").to_string();
                    let why = answer.as_ref().and_then(|a| a.get("reason")).and_then(Value::as_str).unwrap_or("").to_string();
                    let follow_up = answer.as_ref().and_then(|a| a.get("follow_up")).and_then(Value::as_str).unwrap_or("").to_string();
                    self.note("review.finished", json!({"task": task.id, "job": review_job, "verdict": verdict, "reason": why}));
                    let rounds = self.mission.tasks[index].rounds;
                    match verdict.as_str() {
                        "abort" => {
                            self.settle(index, TaskState::Failed, Some(format!("the advisor aborted the mission: {why}")));
                            return Err(Halt::Failed("advisor_abort".into(), format!("the advisor stopped the mission at task `{}`: {why}", task.id)));
                        }
                        // The deterministic checks outrank the advisor: it cannot accept work that failed them.
                        "accept" if good => self.settle(index, TaskState::Accepted, (!why.is_empty()).then_some(why)),
                        "accept" | "revise" => {
                            if rounds >= policy.budgets.max_rounds_per_task {
                                self.settle(index, TaskState::Failed, Some(format!("not accepted after {rounds} round(s): {why}")));
                            } else {
                                let feedback = if verdict == "accept" {
                                    format!("The reviewer was satisfied, but the job itself did not pass: {}. Fix that.", job_state.as_ref().and_then(|s| s.detail.clone().or(s.reason.clone())).unwrap_or_else(|| "its acceptance checks failed".into()))
                                } else if follow_up.trim().is_empty() {
                                    why.clone()
                                } else {
                                    follow_up
                                };
                                let previous = store::load_result(self.home(), &job).ok().flatten();
                                if let Err(e) = self.start_task(index, task, Some((feedback, previous))) {
                                    self.settle(index, TaskState::Failed, Some(format!("the next round could not be started: {}", e.message)));
                                }
                            }
                        }
                        _ => {
                            // The review itself failed. The job's own outcome stands.
                            let note = format!("the review did not produce a verdict (job {review_job}); the job's own outcome stands");
                            self.settle(index, if good { TaskState::Accepted } else { TaskState::Failed }, Some(note));
                        }
                    }
                }
                _ => {}
            }
        }

        // Tasks whose dependencies cannot be met any more are skipped.
        loop {
            let states: BTreeMap<String, TaskState> = self.mission.tasks.iter().map(|t| (t.id.clone(), t.state)).collect();
            let doomed: Vec<usize> = self
                .mission
                .tasks
                .iter()
                .enumerate()
                .filter(|(_, r)| r.state == TaskState::Pending)
                .filter(|(_, r)| plan.tasks.iter().find(|t| t.id == r.id).map(|t| t.depends_on.iter().any(|d| matches!(states.get(d), Some(TaskState::Failed | TaskState::Skipped)))).unwrap_or(false))
                .map(|(i, _)| i)
                .collect();
            if doomed.is_empty() {
                break;
            }
            for i in doomed {
                self.settle(i, TaskState::Skipped, Some("a task it depends on did not succeed".into()));
            }
        }

        if self.marker("abort.request") {
            return Err(Halt::Aborted("aborted on request".into()));
        }
        if let Some(why) = self.over_budget() {
            return Err(Halt::Failed("budget_exceeded".into(), why));
        }
        let active = self.mission.tasks.iter().filter(|t| matches!(t.state, TaskState::Running | TaskState::Reviewing)).count();
        if self.marker("pause.request") {
            return if active == 0 { Err(Halt::Paused) } else { Ok(true) };
        }

        // Start what is ready.
        let states: BTreeMap<String, TaskState> = self.mission.tasks.iter().map(|t| (t.id.clone(), t.state)).collect();
        let mut room = (policy.max_parallel as usize).saturating_sub(active);
        for index in 0..self.mission.tasks.len() {
            if room == 0 {
                break;
            }
            if self.mission.tasks[index].state != TaskState::Pending {
                continue;
            }
            let Some(task) = plan.tasks.iter().find(|t| t.id == self.mission.tasks[index].id) else { continue };
            if task.depends_on.iter().all(|d| states.get(d) == Some(&TaskState::Accepted)) {
                if let Err(e) = self.start_task(index, task, None) {
                    self.settle(index, TaskState::Failed, Some(format!("could not be started: {}", e.message)));
                }
                room -= 1;
            }
        }
        Ok(self.mission.tasks.iter().any(|t| t.state.is_open()))
    }

    fn cancel_running(&self) {
        for task in &self.mission.tasks {
            if matches!(task.state, TaskState::Running | TaskState::Reviewing) {
                for job in task.jobs.last().into_iter().chain(task.reviews.last()) {
                    let _ = runner::cancel(self.home(), job);
                }
            }
        }
    }
}

/// Runs a mission until it ends or is paused. This is the body of `hufu __mission <id>`.
pub fn run(home: &Home, config: &Config, adapters: &dyn AdapterRegistry, id: &str, start: Starter) -> Result<Mission> {
    let mut mission = load(home, id)?;
    if mission.state.is_terminal() {
        return Ok(mission);
    }
    let me = std::process::id();
    if let Some(pid) = mission.driver_pid {
        let fresh = mission.heartbeat_at.as_deref().and_then(parse_rfc3339).map(|t| hufu_contract::time::now_ms().saturating_sub(t) < 30_000).unwrap_or(false);
        if pid != me && proc::pid_alive(pid) && fresh {
            return Err(HufuError::usage(format!("mission `{id}` already has a driver (pid {pid})")));
        }
    }
    mission.driver_pid = Some(me);
    if mission.state == MissionState::Paused {
        mission.state = if mission.tasks.is_empty() { MissionState::Planning } else { MissionState::Running };
    }
    let _ = std::fs::remove_file(file(home, id, "pause.request"));
    let base_active = mission.active_s;
    let mut driver = Driver { ctx: Context { home, config, adapters }, start, mission, began: Instant::now(), base_active };
    driver.save();

    let outcome: std::result::Result<(), Halt> = (|| {
        let plan = driver.plan()?;
        driver.mission.state = MissionState::Running;
        driver.save();
        let mut last_save = Instant::now();
        while driver.step(&plan)? {
            if last_save.elapsed() > Duration::from_secs(5) {
                driver.save();
                last_save = Instant::now();
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        Ok(())
    })();

    match outcome {
        Ok(()) => {
            let bad: Vec<&TaskRecord> = driver.mission.tasks.iter().filter(|t| t.state != TaskState::Accepted).collect();
            if bad.is_empty() {
                driver.mission.state = MissionState::Succeeded;
            } else {
                driver.mission.state = MissionState::Failed;
                driver.mission.reason = Some("tasks_failed".into());
                driver.mission.detail = Some(format!("{} of {} task(s) were not accepted: {}", bad.len(), driver.mission.tasks.len(), bad.iter().map(|t| t.id.as_str()).collect::<Vec<_>>().join(", ")));
            }
        }
        Err(Halt::Paused) => {
            driver.mission.state = MissionState::Paused;
            driver.mission.driver_pid = None;
            driver.note("mission.paused", json!({}));
            driver.save();
            return Ok(driver.mission);
        }
        Err(Halt::Aborted(why)) => {
            driver.cancel_running();
            driver.mission.state = MissionState::Aborted;
            driver.mission.reason = Some("aborted".into());
            driver.mission.detail = Some(why);
        }
        Err(Halt::Failed(reason, detail)) => {
            driver.cancel_running();
            driver.mission.state = MissionState::Failed;
            driver.mission.reason = Some(reason);
            driver.mission.detail = Some(detail);
        }
    }
    // Work left in the mission's own worktree is committed to its branch, so nothing is lost.
    if driver.mission.branch.is_some() {
        if let Some(root) = workspace::repo_root(Path::new(&driver.mission.workdir)) {
            workspace::commit_all(&root, &format!("hufu: mission {} ({})", driver.mission.title, driver.mission.id));
        }
    }
    driver.mission.finished_at = Some(now_rfc3339());
    driver.mission.driver_pid = None;
    driver.save();
    driver.note("mission.finished", json!({"state": driver.mission.state.as_str(), "reason": driver.mission.reason}));
    let text = report(home, &driver.mission);
    let _ = store::write_atomic(&file(home, id, "report.md"), text.as_bytes());
    Ok(driver.mission)
}

/// Asks a mission to stop starting work and exit once running jobs are done.
pub fn pause(home: &Home, id: &str) -> Result<()> {
    let mission = load(home, id)?;
    if mission.state.is_terminal() {
        return Err(HufuError::usage(format!("mission `{id}` has already ended ({})", mission.state.as_str())));
    }
    let path = file(home, id, "pause.request");
    std::fs::write(&path, now_rfc3339()).at(&path)
}

/// Continues a paused mission, or one whose driver is gone.
pub fn resume(home: &Home, id: &str) -> Result<()> {
    let mut mission = load(home, id)?;
    if mission.state.is_terminal() {
        return Err(HufuError::usage(format!("mission `{id}` has already ended ({})", mission.state.as_str())));
    }
    if mission.state != MissionState::Paused && mission.driver_pid.map(proc::pid_alive).unwrap_or(false) {
        return Err(HufuError::usage(format!("mission `{id}` is already being driven")));
    }
    let _ = std::fs::remove_file(file(home, id, "pause.request"));
    if mission.state == MissionState::Paused {
        // Leave "paused" before the driver is up, so that a caller that waits right away waits.
        mission.state = if mission.tasks.is_empty() { MissionState::Planning } else { MissionState::Running };
        save(home, &mission)?;
    }
    launch(home, id).map(|_| ())
}

/// Asks a mission to stop now: running jobs are cancelled.
pub fn abort(home: &Home, id: &str) -> Result<Mission> {
    let mut mission = load(home, id)?;
    if mission.state.is_terminal() {
        return Ok(mission);
    }
    let path = file(home, id, "abort.request");
    std::fs::write(&path, now_rfc3339()).at(&path)?;
    let driver_alive = mission.driver_pid.map(proc::pid_alive).unwrap_or(false);
    if !driver_alive {
        // Nobody is driving it: end it here.
        for task in &mission.tasks {
            for job in task.jobs.last().into_iter().chain(task.reviews.last()) {
                let _ = runner::cancel(home, job);
            }
        }
        mission.state = MissionState::Aborted;
        mission.reason = Some("aborted".into());
        mission.finished_at = Some(now_rfc3339());
        mission.driver_pid = None;
        save(home, &mission)?;
        let _ = store::write_atomic(&file(home, id, "report.md"), report(home, &mission).as_bytes());
    }
    Ok(mission)
}

/// The mission report, in Markdown.
pub fn report(home: &Home, mission: &Mission) -> String {
    let mut out = format!("# Mission {}: {}\n\n", mission.id, mission.title);
    out.push_str(&format!("- State: **{}**", mission.state.as_str()));
    if let Some(reason) = &mission.reason {
        out.push_str(&format!(" ({reason})"));
    }
    out.push('\n');
    if let Some(detail) = &mission.detail {
        out.push_str(&format!("- Detail: {detail}\n"));
    }
    out.push_str(&format!("- Advisor: `{}`\n- Executors: {}\n", mission.advisor, mission.executors.iter().map(|e| format!("`{e}`")).collect::<Vec<_>>().join(", ")));
    out.push_str(&format!("- Workspace: `{}`\n", mission.workdir));
    if let (Some(branch), Some(base)) = (&mission.branch, &mission.base) {
        out.push_str(&format!("- Branch: `{branch}` (from `{}`)\n", &base[..base.len().min(12)]));
    }
    out.push_str(&format!("- Active time: {}\n", fmt_duration(mission.active_s)));
    if !mission.usage.is_empty() {
        out.push_str(&format!("- Tokens: {}", mission.usage.tokens()));
        if let Some(cost) = mission.usage.cost_usd {
            out.push_str(&format!(", cost: ${cost:.2}"));
        }
        out.push_str(" (only where agents report usage)\n");
    }
    out.push_str(&format!("\n## Goal\n\n{}\n", mission.goal));
    if let Some(summary) = &mission.summary {
        out.push_str(&format!("\n## Plan\n\n{summary}\n"));
    }
    if !mission.tasks.is_empty() {
        out.push_str("\n## Tasks\n\n| Task | Title | Agent | State | Rounds | Jobs |\n|---|---|---|---|---|---|\n");
        for t in &mission.tasks {
            out.push_str(&format!("| {} | {} | {} | {} | {} | {} |\n", t.id, t.title.replace('|', "/"), t.agent, t.state.as_str(), t.rounds, t.jobs.join(", ")));
        }
        for t in &mission.tasks {
            out.push_str(&format!("\n### {}: {}\n\n", t.id, t.title));
            if let Some(note) = &t.note {
                out.push_str(&format!("{note}\n\n"));
            }
            if let Some(result) = t.jobs.last().and_then(|j| store::load_result(home, j).ok().flatten()) {
                if !result.changes.files.is_empty() {
                    out.push_str(&format!("Changed files: {}\n\n", result.changes.files.iter().map(|f| format!("`{f}`")).collect::<Vec<_>>().join(", ")));
                }
                for check in &result.tally {
                    out.push_str(&format!("- {} `{}`: {}\n", if check.passed { "passed" } else { "FAILED" }, check.description, check.detail));
                }
                if let Some(message) = &result.final_message {
                    out.push_str(&format!("\n{}\n", clip(message, 2000)));
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema;

    #[test]
    fn mission_files_parse_and_reject_unknown_fields() {
        let dir = std::env::temp_dir().join(format!("hufu-mission-{}", new_id("t")));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("mission.yaml");
        std::fs::write(&path, "version: 1\ngoal: Migrate the client\nadvisor: a\nexecutors: [x1, x2]\nisolation: worktree\nreview: always\nbudgets: {max_tasks: 4}\nmax_tier: workspace-write\n").unwrap();
        let spec = MissionSpec::from_file(&path).unwrap();
        assert_eq!(spec.executors, ["x1", "x2"]);
        assert_eq!(spec.budgets.unwrap().max_tasks, 4);
        std::fs::write(&path, "goal: x\nadvisors: [a]\n").unwrap();
        assert!(MissionSpec::from_file(&path).unwrap_err().message.contains("advisors"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn schemas_accept_the_documented_shapes() {
        let plan = json!({"v": 1, "summary": "s", "tasks": [{"id": "t1", "title": "T", "prompt": "p", "agent": "a", "permission": null, "effort": "high", "depends_on": [], "acceptance": [{"command": "npm test"}], "grants": null}]});
        assert!(schema::validate(&plan_schema(), &plan).is_empty());
        let parsed: Plan = serde_json::from_value(plan).unwrap();
        assert_eq!(parsed.tasks[0].effort, Some(Effort::High));
        // An advisor that is only told about the schema may leave out what it has nothing to say about.
        let mut short = json!({"tasks": [{"id": "t1", "title": "T", "prompt": "p", "agent": "a"}]});
        schema::fill_nullable(&plan_schema(), &mut short);
        assert!(schema::validate(&plan_schema(), &short).is_empty(), "{short}");
        let parsed: Plan = serde_json::from_value(short).unwrap();
        assert_eq!((parsed.v, parsed.tasks[0].depends_on.len(), parsed.tasks[0].grants.len()), (1, 0, 0));
        let review = json!({"v": 1, "verdict": "revise", "reason": "r", "follow_up": "do x"});
        assert!(schema::validate(&review_schema(), &review).is_empty());
        assert!(!schema::validate(&review_schema(), &json!({"v": 1, "verdict": "maybe", "reason": "", "follow_up": null})).is_empty());
    }
}
