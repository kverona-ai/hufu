//! Submission: a task spec becomes a validated job spec, a job directory and a running runner.
//! Everything that can be rejected is rejected here, before any agent is started.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use hufu_contract::time::{new_id, now_rfc3339, parse_duration};
use hufu_contract::{AdapterRegistry, Effort, EffortFallback, FORMAT_VERSION, LaunchPlan, McpServerSpec, Profile, RunRequest, Tier, effort_list, proc, tier_list};
use serde::{Deserialize, Serialize};

use crate::capability;
use crate::config::{AgentRef, Config, Home, Isolation, Route, Strategy};
use crate::error::{HufuError, IoContext, Result};
use crate::spec::{JobSpec, SchemaRef, TaskSpec, title_from_prompt};
use crate::store::{self, JobState, State};
use crate::{slots, workspace};

/// What a submission needs besides the task.
pub struct Context<'a> {
    pub home: &'a Home,
    pub config: &'a Config,
    pub adapters: &'a dyn AdapterRegistry,
}

/// Where a submission comes from and what it is part of.
#[derive(Clone, Debug, Default)]
pub struct Options {
    /// Relative paths in the task (`cwd`, schema files) are resolved against this directory.
    pub base_dir: PathBuf,
    pub parent: Option<String>,
    pub mission: Option<String>,
    /// Missions cap the tier their tasks may ask for.
    pub max_tier: Option<Tier>,
    pub resume_session: Option<String>,
    /// Create the job but do not start a runner (the caller runs it itself).
    pub hold: bool,
}

/// `run/daemon.json`: written by a running daemon.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DaemonInfo {
    pub pid: u32,
    pub started_at: String,
    pub version: String,
}

pub fn daemon_file(home: &Home) -> PathBuf {
    home.run().join("daemon.json")
}

/// The running daemon, if there is one.
pub fn daemon_info(home: &Home) -> Option<DaemonInfo> {
    let info: DaemonInfo = store::read_json(&daemon_file(home)).ok()?;
    proc::pid_alive(info.pid).then_some(info)
}

struct Fit {
    permission: Tier,
    effort: Effort,
    model: Option<String>,
    warnings: Vec<String>,
}

/// Checks one profile against one task and works out the settings it would run with.
fn fit(ctx: &Context, profile: &Profile, task: &TaskSpec, opts: &Options) -> Result<Fit> {
    let name = &profile.name;
    let adapter = ctx
        .adapters
        .get(&profile.adapter)
        .ok_or_else(|| HufuError::usage(format!("profile `{name}` uses unknown adapter `{}`; accepted: {}", profile.adapter, ctx.adapters.kinds().join(", "))))?;
    let caps = adapter.capabilities(profile);
    let mut warnings = Vec::new();

    // Permission tier: the task, then the profile, then the global default.
    let permission = task.permission.or(profile.permission).unwrap_or(ctx.config.defaults.permission);
    if permission == Tier::FullAccess && task.permission.is_none() && !(profile.permission == Some(Tier::FullAccess) && profile.allow_full_access) {
        return Err(HufuError::usage(
            "`full-access` must be asked for explicitly: pass `--permission full-access`, or set it on a profile together with `allow_full_access: true`",
        ));
    }
    if let Some(max) = opts.max_tier {
        if permission > max {
            return Err(HufuError::usage(format!("permission `{permission}` exceeds the mission's limit `{max}` (`missions.max_tier`)")));
        }
    }
    if !caps.supports_tier(permission) {
        return Err(HufuError::usage(format!(
            "profile `{name}` (adapter `{}`) cannot enforce permission `{permission}`; it supports: {}. Hufu does not silently widen or narrow a tier",
            profile.adapter,
            tier_list(&caps.permission_tiers)
        )));
    }

    // Effort.
    let (asked, explicit) = match (&task.effort, &profile.effort) {
        (Some(e), _) => (e.clone(), true),
        (None, Some(e)) => (e.clone(), true),
        (None, None) => (ctx.config.defaults.effort.clone(), false),
    };
    let effort = match &asked {
        Effort::Default => Effort::Default,
        Effort::Native(v) => {
            if caps.native_effort || caps.checked_at_runtime {
                asked.clone()
            } else {
                return Err(HufuError::usage(format!("profile `{name}` does not accept a native effort value (`native:{v}`); accepted: {}", effort_list(&caps.efforts))));
            }
        }
        level if caps.efforts.contains(level) || caps.checked_at_runtime => level.clone(),
        level => {
            if !explicit {
                warnings.push(format!("the default effort `{level}` has no mapping for profile `{name}`; the agent's own default is used"));
                Effort::Default
            } else if profile.effort_fallback == EffortFallback::Nearest {
                match level.nearest(&caps.efforts) {
                    Some(near) => {
                        warnings.push(format!("effort `{level}` has no mapping for profile `{name}`; using the nearest level `{near}`"));
                        near
                    }
                    None => {
                        warnings.push(format!("profile `{name}` has no effort control; effort `{level}` is ignored"));
                        Effort::Default
                    }
                }
            } else {
                return Err(HufuError::usage(format!(
                    "effort `{level}` has no mapping for profile `{name}`; accepted: {}. Set `effort_fallback: nearest` on the profile to use the closest level instead",
                    effort_list(&caps.efforts)
                )));
            }
        }
    };

    // Model.
    let model = task.model.clone().or_else(|| profile.model.clone());
    if let Some(m) = &model {
        if !caps.models.is_empty() && !caps.models.contains(m) && !caps.checked_at_runtime {
            warnings.push(format!("model `{m}` is not in the list profile `{name}` reports; the agent decides whether it exists"));
        }
    }

    if opts.resume_session.is_some() && !caps.resume {
        return Err(HufuError::usage(format!("profile `{name}` (adapter `{}`) cannot resume a session", profile.adapter)));
    }
    Ok(Fit { permission, effort, model, warnings })
}

fn rotate(home: &Home, pool: &str, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    let path = home.run().join(format!("rr-{pool}"));
    let _ = std::fs::create_dir_all(home.run());
    let n: usize = std::fs::read_to_string(&path).ok().and_then(|t| t.trim().parse().ok()).unwrap_or(0);
    let _ = store::write_atomic(&path, (n.wrapping_add(1)).to_string().as_bytes());
    n % len
}

/// Orders the members of a pool for one job.
fn order_members(home: &Home, name: &str, strategy: Strategy, mut members: Vec<Profile>) -> Vec<Profile> {
    match strategy {
        Strategy::Failover => {}
        Strategy::RoundRobin => {
            let start = rotate(home, name, members.len());
            members.rotate_left(start);
        }
        Strategy::LeastBusy => {
            let live = slots::live(home);
            members.sort_by_key(|m| live.iter().filter(|s| s.profile == m.name).count());
        }
    }
    members
}

fn resolve_cwd(task: &TaskSpec, base: &Path) -> Result<PathBuf> {
    let raw = task.cwd.as_deref().unwrap_or(".");
    let expanded = proc::expand_tilde(raw);
    let joined = if expanded.is_absolute() { expanded } else { base.join(expanded) };
    let real = std::fs::canonicalize(&joined).map_err(|e| HufuError::usage(format!("cwd {}: {e}", joined.display())))?;
    if !real.is_dir() {
        return Err(HufuError::usage(format!("cwd {} is not a directory", real.display())));
    }
    Ok(strip_verbatim(real))
}

/// `canonicalize` on Windows returns `\\?\C:\...`, which many tools reject.
fn strip_verbatim(path: PathBuf) -> PathBuf {
    if cfg!(windows) {
        let text = path.to_string_lossy();
        if let Some(rest) = text.strip_prefix(r"\\?\") {
            if !rest.starts_with("UNC\\") {
                return PathBuf::from(rest);
            }
        }
    }
    path
}

fn seconds(field: &str, text: &str) -> Result<u64> {
    let d = parse_duration(text).map_err(|e| HufuError::usage(format!("{field}: {e}")))?;
    if d.as_secs() == 0 {
        return Err(HufuError::usage(format!("{field}: must be at least one second")));
    }
    Ok(d.as_secs())
}

/// Validates a task and resolves it into the job spec that would run. Nothing is written.
pub fn prepare(ctx: &Context, task: &TaskSpec, opts: &Options) -> Result<JobSpec> {
    if let Some(v) = task.version {
        if v != FORMAT_VERSION {
            return Err(HufuError::usage(format!("task spec version {v} is not supported; this build reads version {FORMAT_VERSION}")));
        }
    }
    if task.prompt.trim().is_empty() {
        return Err(HufuError::usage("the task has no prompt; pass it as an argument, with `--prompt-file`, on stdin with `-`, or as `prompt:` in a task file"));
    }
    if task.agent.trim().is_empty() {
        return Err(HufuError::usage("the task names no agent; pass `--agent <profile-or-pool>` (see `hufu agents`)"));
    }
    let cwd = resolve_cwd(task, &opts.base_dir)?;

    // Profile or pool.
    let mut warnings = Vec::new();
    let candidates = match ctx.config.agent(&task.agent)? {
        AgentRef::Profile(p) => vec![p],
        AgentRef::Pool { name, strategy, members } => {
            if members.is_empty() {
                return Err(HufuError::usage(format!("pool `{name}` has no members")));
            }
            order_members(ctx.home, &name, strategy, members)
        }
    };
    let is_pool = candidates.len() > 1 || ctx.config.pools.contains_key(&task.agent);
    let mut usable: Vec<(Profile, Fit, capability::Resolved)> = Vec::new();
    let mut first_error: Option<HufuError> = None;
    for profile in candidates {
        let checked = fit(ctx, &profile, task, opts).and_then(|f| {
            let cu = capability::resolve(ctx.config, ctx.adapters, &profile, &task.grants, task.computer_use.as_ref())?;
            Ok((f, cu))
        });
        match checked {
            Ok((f, cu)) => {
                // Every member must run the task under the same limits; one that would not is left out.
                if let Some((_, first, first_cu)) = usable.first() {
                    if first.permission != f.permission {
                        warnings.push(format!("pool member `{}` would run this task at a different permission tier and is left out", profile.name));
                        continue;
                    }
                    if first_cu.plan.is_some() {
                        // Computer use is planned for one operator; a stand-in would change who operates the desktop.
                        continue;
                    }
                }
                usable.push((profile, f, cu));
            }
            Err(e) if is_pool => {
                warnings.push(format!("pool member `{}` cannot take this task: {}", profile.name, e.message));
                first_error.get_or_insert(e);
            }
            Err(e) => return Err(e),
        }
    }
    if usable.is_empty() {
        let why = first_error.map(|e| e.message).unwrap_or_default();
        return Err(HufuError::usage(format!("no member of pool `{}` can take this task. {why}", task.agent)));
    }
    let (profile, chosen, cu) = usable.remove(0);
    warnings.extend(chosen.warnings);
    warnings.extend(cu.warnings);
    let alternates: Vec<Profile> = usable
        .into_iter()
        .map(|(mut p, f, _)| {
            // An alternate carries its own resolved settings.
            p.effort = Some(f.effort);
            p.model = f.model;
            p
        })
        .collect();

    // Output schema and acceptance checks. Schema files are read now, so the job is self-contained.
    let output_schema = match &task.output {
        Some(out) => {
            let schema = out.schema.load(&opts.base_dir)?;
            if !schema.is_object() {
                return Err(HufuError::usage("output.schema must be a JSON Schema object"));
            }
            Some(schema)
        }
        None => None,
    };
    let mut acceptance = task.acceptance.clone();
    for (i, check) in acceptance.iter_mut().enumerate() {
        let at = format!("acceptance[{i}]");
        let kind = check.kind().map_err(|e| HufuError::usage(format!("{at}: {e}")))?;
        match kind {
            "command" => {
                if check.command.as_deref().unwrap_or("").trim().is_empty() {
                    return Err(HufuError::usage(format!("{at}.command: is empty")));
                }
                if let Some(t) = &check.timeout {
                    seconds(&format!("{at}.timeout"), t)?;
                }
            }
            "schema" => {
                let schema = check.schema.as_ref().map(|s| s.load(&opts.base_dir)).transpose()?.unwrap_or_default();
                check.schema = Some(SchemaRef::Inline(schema));
            }
            "review" => {
                if opts.mission.is_none() {
                    return Err(HufuError::usage(format!("{at}.review: a review check needs an advisor, so it is only valid inside a mission")));
                }
                if check.review.as_deref() != Some("advisor") {
                    return Err(HufuError::usage(format!("{at}.review: the only accepted value is `advisor`")));
                }
            }
            _ => {}
        }
        if kind != "command" && check.timeout.is_some() {
            return Err(HufuError::usage(format!("{at}.timeout: only applies to a `command` check")));
        }
    }

    let timeout_s = seconds("timeout", task.timeout.as_deref().unwrap_or(&ctx.config.defaults.timeout))?;
    let inactivity_timeout_s = seconds("inactivity_timeout", task.inactivity_timeout.as_deref().unwrap_or(&ctx.config.defaults.inactivity_timeout))?;
    let retry = task.retry.clone().unwrap_or_else(|| ctx.config.defaults.retry.clone());
    if retry.max_attempts == 0 {
        return Err(HufuError::usage("retry.max_attempts: must be at least 1"));
    }
    parse_duration(&retry.backoff).map_err(|e| HufuError::usage(format!("retry.backoff: {e}")))?;

    // Workspace.
    let isolation = task.isolation.unwrap_or(ctx.config.defaults.isolation);
    if isolation == Isolation::Worktree {
        let root = workspace::repo_root(&cwd)
            .ok_or_else(|| HufuError::usage(format!("{} is not inside a git repository, which `isolation: worktree` needs", cwd.display())))?;
        if workspace::head(&root).is_none() {
            return Err(HufuError::usage("the repository has no commit yet; `isolation: worktree` needs one to branch from"));
        }
    } else if chosen.permission >= Tier::WorkspaceWrite {
        let here = cwd.display().to_string();
        let sharing: Vec<String> = store::list_jobs(ctx.home)
            .into_iter()
            .filter(|j| !j.state.is_terminal() && j.permission >= Tier::WorkspaceWrite && j.cwd == here && j.workspace.as_ref().map(|w| !w.worktree).unwrap_or(true))
            .map(|j| j.id)
            .collect();
        if !sharing.is_empty() {
            warnings.push(format!(
                "{} already writes to this workspace; two writers in one directory can overwrite each other. Use `--isolation worktree` to give this job its own copy",
                sharing.join(", ")
            ));
        }
    }

    let mut depends_on = Vec::new();
    for dep in &task.depends_on {
        depends_on.push(store::resolve_job(ctx.home, dep).map_err(|e| HufuError::usage(format!("depends_on: {}", e.message)))?);
    }

    Ok(JobSpec {
        v: FORMAT_VERSION,
        id: new_id("j"),
        title: task.title.clone().filter(|t| !t.trim().is_empty()).unwrap_or_else(|| title_from_prompt(&task.prompt)),
        prompt: task.prompt.clone(),
        agent: task.agent.clone(),
        profile,
        alternates,
        cwd,
        permission: chosen.permission,
        effort: chosen.effort,
        model: chosen.model,
        timeout_s,
        inactivity_timeout_s,
        isolation,
        output_schema,
        acceptance,
        strict_permissions: task.strict_permissions.unwrap_or(false),
        depends_on,
        labels: task.labels.clone(),
        grants: task.grants.clone(),
        retry,
        resume_session: opts.resume_session.clone(),
        parent: opts.parent.clone(),
        mission: opts.mission.clone(),
        computer_use: cu.plan,
        warnings,
    })
}

/// The executable that runs jobs: this binary, unless `HUFU_EXE` says otherwise.
pub fn hufu_exe() -> Result<PathBuf> {
    if let Some(p) = std::env::var_os("HUFU_EXE").filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(p));
    }
    std::env::current_exe().map_err(|e| HufuError::internal(format!("cannot find the hufu executable: {e}")))
}

/// Starts a detached helper process of this binary (`__runner`, `__mission`, `daemon run`).
pub fn spawn_detached(home: &Home, args: &[&str], log: &Path) -> Result<u32> {
    let exe = hufu_exe()?;
    if let Some(parent) = log.parent() {
        std::fs::create_dir_all(parent).at(parent)?;
    }
    let out = std::fs::OpenOptions::new().create(true).append(true).open(log).at(log)?;
    let err = out.try_clone().at(log)?;
    let mut cmd = Command::new(&exe);
    cmd.args(args).env("HUFU_HOME", &home.root).stdin(Stdio::null()).stdout(out).stderr(err);
    if home.root.is_dir() {
        cmd.current_dir(&home.root);
    }
    proc::detach(&mut cmd);
    let mut child = cmd.spawn().map_err(|e| HufuError::internal(format!("cannot start {}: {e}", exe.display())))?;
    let pid = child.id();
    // Reap the child if this process outlives it; a short-lived caller simply exits first.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(pid)
}

/// Starts the runner of a queued job.
pub fn launch(home: &Home, id: &str) -> Result<u32> {
    spawn_detached(home, &["__runner", id], &home.job(id).join("runner.log"))
}

/// Starts a job that was left for a daemon which is no longer running.
pub fn kick(home: &Home, id: &str) {
    let Ok(mut state) = store::load_state(home, id) else { return };
    if state.state == State::Queued && state.runner_pid.is_none() && state.queue.as_deref() == Some("daemon") && daemon_info(home).is_none() {
        state.queue = None;
        if store::save_state(home, &state).is_ok() {
            let _ = launch(home, id);
        }
    }
}

/// Validates, stores and starts a job.
pub fn submit(ctx: &Context, task: &TaskSpec, opts: &Options) -> Result<JobState> {
    let spec = prepare(ctx, task, opts)?;
    submit_spec(ctx.home, &spec, opts.hold)
}

/// Stores a prepared job and starts it, unless a daemon will or the caller holds it.
pub fn submit_spec(home: &Home, spec: &JobSpec, hold: bool) -> Result<JobState> {
    let mut state = store::create_job(home, spec)?;
    if let Some(parent) = &spec.parent {
        let _ = store::add_child(home, parent, &spec.id);
    }
    if hold {
        return Ok(state);
    }
    if daemon_info(home).is_some() {
        state.queue = Some("daemon".into());
        store::save_state(home, &state)?;
        return Ok(state);
    }
    if let Err(e) = launch(home, &spec.id) {
        state.state = State::Failed;
        state.reason = Some(hufu_contract::reason::INTERNAL.into());
        state.detail = Some(e.message.clone());
        state.finished_at = Some(now_rfc3339());
        store::save_state(home, &state)?;
        return Err(e);
    }
    Ok(state)
}

/// Starts a follow-up in the session of a finished job. The follow-up keeps the workspace, tier,
/// acceptance checks and output schema of that job; `edit` may change what differs this time.
pub fn resume(ctx: &Context, previous: &str, prompt: &str, hold: bool, edit: impl FnOnce(&mut JobSpec)) -> Result<JobState> {
    let old = store::load_spec(ctx.home, previous)?;
    let state = store::load_state(ctx.home, previous)?;
    if !state.state.is_terminal() {
        return Err(HufuError::usage(format!("job `{previous}` is still {}; wait for it or cancel it before resuming its session", state.state.as_str())));
    }
    let result = store::load_result(ctx.home, previous)?;
    let session = result
        .as_ref()
        .and_then(|r| r.session.clone())
        .filter(|s| s.resumable)
        .ok_or_else(|| HufuError::usage(format!("job `{previous}` left no resumable session; start a new job with the context it needs instead")))?;
    if prompt.trim().is_empty() {
        return Err(HufuError::usage("the follow-up has no prompt"));
    }
    // The follow-up runs with the profile the session belongs to, as it is configured today.
    let profile_name = state.profile.clone();
    let profile = ctx.config.profile(&profile_name).cloned().unwrap_or_else(|| old.profile.clone());
    let caps = ctx.adapters.get(&profile.adapter).map(|a| a.capabilities(&profile));
    if !caps.map(|c| c.resume).unwrap_or(false) {
        return Err(HufuError::usage(format!("profile `{profile_name}` cannot resume a session")));
    }
    let mut spec = old.clone();
    spec.id = new_id("j");
    spec.title = title_from_prompt(prompt);
    spec.prompt = prompt.to_string();
    spec.agent = profile_name;
    spec.profile = profile;
    spec.alternates = Vec::new();
    spec.resume_session = Some(session.id);
    spec.parent = Some(previous.to_string());
    spec.depends_on = Vec::new();
    spec.warnings = Vec::new();
    // A capability grant does not carry over to a follow-up.
    spec.grants = Vec::new();
    spec.computer_use = None;
    if spec.isolation == Isolation::Worktree {
        // Continue in the worktree the first job left behind, if it is still there.
        if let Some(path) = result.as_ref().and_then(|r| r.changes.worktree.clone()).filter(|p| Path::new(p).is_dir()) {
            spec.cwd = PathBuf::from(path);
            spec.isolation = Isolation::None;
        }
    }
    edit(&mut spec);
    let mut state = store::create_job(ctx.home, &spec)?;
    if !hold {
        if daemon_info(ctx.home).is_some() {
            state.queue = Some("daemon".into());
            store::save_state(ctx.home, &state)?;
        } else {
            launch(ctx.home, &spec.id)?;
        }
    }
    Ok(state)
}

/// The environment an agent process gets for one profile: the small base set, the variables the
/// profile names, renamed variables, literal variables, and the agent's home directory.
pub fn env_for(adapters: &dyn AdapterRegistry, profile: &Profile) -> std::collections::BTreeMap<String, String> {
    let mut env = proc::agent_env(&profile.env);
    for (target, source) in &profile.env_from {
        if let Ok(value) = std::env::var(source) {
            env.insert(target.clone(), value);
        }
    }
    for (key, value) in &profile.vars {
        env.insert(key.clone(), value.clone());
    }
    if let (Some(home), Some(var)) = (&profile.home, adapters.get(&profile.adapter).and_then(|a| a.home_var())) {
        env.insert(var.to_string(), proc::expand_tilde(home).display().to_string());
    }
    env
}

/// Variables a profile names that are not set, so that a missing key is reported before launch.
pub fn missing_env(profile: &Profile) -> Vec<String> {
    let mut out: Vec<String> = profile.env_from.values().filter(|v| std::env::var_os(v).is_none()).cloned().collect();
    out.sort();
    out.dedup();
    out
}

/// The MCP server an executor gets when computer use is handed to a dedicated agent.
pub fn handover_server(home: &Home, job: &str) -> Result<McpServerSpec> {
    let mut env = std::collections::BTreeMap::new();
    env.insert("HUFU_HOME".to_string(), home.root.display().to_string());
    Ok(McpServerSpec { name: "hufu".into(), command: hufu_exe()?.display().to_string(), args: vec!["mcp".into(), "--child-of".into(), job.to_string()], env })
}

/// The request an adapter receives for one attempt with one profile.
pub fn run_request(home: &Home, adapters: &dyn AdapterRegistry, spec: &JobSpec, profile: &Profile, primary: bool, cwd: &Path) -> Result<RunRequest> {
    let (effort, model) = if primary { (spec.effort.clone(), spec.model.clone()) } else { (profile.effort.clone().unwrap_or_default(), profile.model.clone()) };
    let mut prompt = spec.prompt.clone();
    let mut mcp_servers = Vec::new();
    if let Some(plan) = &spec.computer_use {
        prompt = format!("{}{}", capability::preamble(plan), prompt);
        if plan.route == Route::Dedicated {
            mcp_servers.push(handover_server(home, &spec.id)?);
        }
    }
    Ok(RunRequest {
        job_id: spec.id.clone(),
        prompt,
        cwd: cwd.to_path_buf(),
        permission: spec.permission,
        effort,
        model,
        output_schema: spec.output_schema.clone(),
        resume_session: spec.resume_session.clone(),
        env: env_for(adapters, profile),
        mcp_servers,
        scratch_dir: home.job(&spec.id),
    })
}

/// The exact invocation a job would use, for `--dry-run`.
pub fn launch_plan(ctx: &Context, spec: &JobSpec) -> Result<LaunchPlan> {
    let adapter = ctx.adapters.get(&spec.profile.adapter).ok_or_else(|| HufuError::usage(format!("unknown adapter `{}`", spec.profile.adapter)))?;
    let req = run_request(ctx.home, ctx.adapters, spec, &spec.profile, true, &spec.cwd)?;
    adapter.plan(&spec.profile, &req).map_err(|e| HufuError::usage(e.0))
}
