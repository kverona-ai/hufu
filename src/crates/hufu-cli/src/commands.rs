//! What each command does. Everything of substance lives in `hufu-core`; this file turns flags
//! into calls and results into text or JSON.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use hufu_adapters::Registry;
use hufu_contract::time::parse_duration;
use hufu_contract::{Effort, Tier};
use hufu_core::config::{self, Grant, Home, Isolation, Loaded, RetryPolicy, ReviewPolicy, Route};
use hufu_core::error::{HufuError, Result, exit};
use hufu_core::mission::{self, MissionSpec, MissionState};
use hufu_core::ops::{self, PruneOptions};
use hufu_core::spec::{Check, ComputerUseOverride, OutputSpec, PathRules, SchemaRef, TaskSpec};
use hufu_core::store::{self, JobState, State};
use hufu_core::submit::{self, Context, Options};
use hufu_core::{capability, doctor, runner, setup, update};
use serde::Serialize;
use serde_json::{Value, json};

use crate::cli::*;
use crate::render;

const VERSION: &str = env!("CARGO_PKG_VERSION");

struct App {
    home: Home,
    adapters: Registry,
    cwd: PathBuf,
    json: bool,
}

impl App {
    fn load(&self) -> Result<Loaded> {
        config::load(&self.home, &self.cwd)
    }

    /// Prints a JSON document with the format version, or runs the plain-text printer.
    fn out<T: Serialize>(&self, value: &T, human: impl FnOnce()) {
        if self.json {
            let mut v = serde_json::to_value(value).unwrap_or(Value::Null);
            if let Some(obj) = v.as_object_mut() {
                obj.entry("v").or_insert(json!(1));
            }
            println!("{v}");
        } else {
            human();
        }
    }

    fn target(&self, project: bool) -> ops::Target {
        ops::config_target(&self.home, project.then_some(self.cwd.as_path()))
    }
}

fn read_prompt(words: &[String], file: Option<&Path>) -> Result<Option<String>> {
    if let Some(path) = file {
        return std::fs::read_to_string(path)
            .map(Some)
            .map_err(|e| HufuError::usage(format!("prompt file {}: {e}", path.display())));
    }
    if words.len() == 1 && words[0] == "-" {
        let mut text = String::new();
        std::io::stdin()
            .read_to_string(&mut text)
            .map_err(|e| HufuError::usage(format!("cannot read the prompt from stdin: {e}")))?;
        return Ok(Some(text));
    }
    Ok((!words.is_empty()).then(|| words.join(" ")))
}

fn duration(flag: &str, text: &str) -> Result<Duration> {
    parse_duration(text).map_err(|e| HufuError::usage(format!("{flag}: {e}")))
}

fn pairs(flag: &str, items: &[String]) -> Result<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    for item in items {
        let (k, v) =
            item.split_once('=').ok_or_else(|| HufuError::usage(format!("{flag} takes KEY=VALUE, got `{item}`")))?;
        out.insert(k.trim().to_string(), v.to_string());
    }
    Ok(out)
}

pub fn run(args: Cli) -> Result<i32> {
    let app = App {
        home: Home::resolve()?,
        adapters: Registry::builtin(),
        cwd: std::env::current_dir()
            .map_err(|e| HufuError::internal(format!("cannot read the current directory: {e}")))?,
        json: args.json,
    };
    match args.command {
        Command::Run(a) => cmd_run(&app, a),
        Command::Status(a) => cmd_status(&app, a),
        Command::Wait(a) => cmd_wait(&app, a),
        Command::Result(a) => cmd_result(&app, &a.job, a.full),
        Command::Logs(a) => cmd_logs(&app, a),
        Command::Cancel(a) => cmd_cancel(&app, a),
        Command::Resume(a) => cmd_resume(&app, a),
        Command::Agents(a) => cmd_agents(&app, a),
        Command::Doctor(a) => cmd_doctor(&app, a),
        Command::Prune(a) => cmd_prune(&app, a),
        Command::Mission(c) => cmd_mission(&app, c),
        Command::Mcp(a) => {
            let server = hufu_mcp::Server {
                home: app.home.clone(),
                adapters: &app.adapters,
                version: VERSION,
                child_of: a.child_of,
                cwd: app.cwd.clone(),
            };
            server.serve().map_err(|e| HufuError::internal(e.to_string()))?;
            Ok(exit::OK)
        }
        Command::Daemon(c) => cmd_daemon(&app, c),
        Command::Init(a) => cmd_init(&app, a.force),
        Command::Up(a) => cmd_up(&app, a),
        Command::Setup(a) => cmd_setup(&app, a),
        Command::Update(a) => cmd_update(&app, a),
        Command::Config(c) => cmd_config(&app, c),
        Command::Profile(c) => cmd_profile(&app, c),
        Command::Pool(c) => cmd_pool(&app, c),
        Command::Runner { job } => {
            let state = runner::run_job(&app.home, &app.adapters, &job)?;
            Ok(render::exit_code(state.state, state.verdict, state.reason.as_deref()))
        }
        Command::Sweep { exe, known } => {
            update::sweep_until_gone(&exe, &known);
            Ok(exit::OK)
        }
        Command::MissionDriver { mission } => {
            let m = mission::load(&app.home, &mission)?;
            let loaded = config::load(&app.home, Path::new(&m.cwd))?;
            let start = |home: &Home, id: &str| submit::launch(home, id).map(|_| ());
            let done = mission::run(&app.home, &loaded.config, &app.adapters, &mission, &start)?;
            Ok(if done.state == MissionState::Succeeded || done.state == MissionState::Paused {
                exit::OK
            } else {
                exit::JOB_FAILED
            })
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Jobs
// ---------------------------------------------------------------------------------------------

fn task_from(app: &App, loaded: &Loaded, a: &RunArgs) -> Result<(TaskSpec, PathBuf)> {
    let (mut task, base) = match &a.file {
        Some(path) => {
            let dir = path
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| PathBuf::from("."));
            (TaskSpec::from_file(path)?, if dir.is_absolute() { dir } else { app.cwd.join(dir) })
        }
        None => (TaskSpec::default(), app.cwd.clone()),
    };
    if let Some(prompt) = read_prompt(&a.prompt, a.prompt_file.as_deref())? {
        task.prompt = prompt;
    }
    if let Some(agent) = &a.agent {
        task.agent = agent.clone();
    }
    if task.agent.is_empty() {
        task.agent = loaded.config.roles.executors.first().cloned().ok_or_else(|| {
            HufuError::usage("no agent given: pass `--agent <profile-or-pool>` (see `hufu agents`), or set a default with `hufu config set roles.executors \"[<profile>]\"`")
        })?;
    }
    if let Some(p) = &a.permission {
        task.permission = Some(Tier::parse(p).map_err(HufuError::usage)?);
    }
    if let Some(e) = &a.effort {
        task.effort = Some(Effort::parse(e).map_err(HufuError::usage)?);
    }
    if let Some(i) = &a.isolation {
        task.isolation = Some(Isolation::parse(i).map_err(HufuError::usage)?);
    }
    if a.cwd.is_some() {
        // A directory given on the command line is relative to where the command runs.
        task.cwd = a.cwd.as_ref().map(|c| {
            let p = hufu_contract::proc::expand_tilde(c);
            if p.is_absolute() { p } else { app.cwd.join(p) }.display().to_string()
        });
    }
    for (target, value) in [
        (&mut task.model, &a.model),
        (&mut task.timeout, &a.timeout),
        (&mut task.inactivity_timeout, &a.inactivity_timeout),
        (&mut task.title, &a.title),
    ] {
        if value.is_some() {
            *target = value.clone();
        }
    }
    if let Some(schema) = &a.schema {
        let path = hufu_contract::proc::expand_tilde(schema);
        task.output = Some(OutputSpec {
            schema: SchemaRef::Path(if path.is_absolute() { path } else { app.cwd.join(path) }.display().to_string()),
        });
    }
    for command in &a.accept {
        task.acceptance.push(Check { command: Some(command.clone()), ..Check::default() });
    }
    if !a.allow_path.is_empty() || !a.deny_path.is_empty() {
        task.acceptance.push(Check {
            paths: Some(PathRules { allow: a.allow_path.clone(), deny: a.deny_path.clone() }),
            ..Check::default()
        });
    }
    if a.strict {
        task.strict_permissions = Some(true);
    }
    task.depends_on.extend(a.depends_on.iter().cloned());
    task.labels.extend(pairs("--label", &a.label)?);
    if let Some(apps) = &a.computer_use {
        task.grants.push(Grant {
            capability: capability::COMPUTER_USE.into(),
            apps: apps.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(),
        });
    }
    if a.computer_use_agent.is_some() || a.computer_use_route.is_some() {
        let route = match a.computer_use_route.as_deref() {
            Some("dedicated") => Some(Route::Dedicated),
            Some("executor") => Some(Route::Executor),
            Some(other) => {
                return Err(HufuError::usage(format!(
                    "unknown --computer-use-route `{other}`; accepted: dedicated, executor"
                )));
            }
            None => None,
        };
        task.computer_use = Some(ComputerUseOverride { route, agent: a.computer_use_agent.clone() });
    }
    if let Some(n) = a.retry {
        let mut policy = task.retry.clone().unwrap_or_else(|| loaded.config.defaults.retry.clone());
        policy.max_attempts = n;
        task.retry = Some(RetryPolicy { ..policy });
    }
    Ok((task, base))
}

fn cmd_run(app: &App, a: RunArgs) -> Result<i32> {
    let loaded = app.load()?;
    let (task, base) = task_from(app, &loaded, &a)?;
    let ctx = Context { home: &app.home, config: &loaded.config, adapters: &app.adapters };
    let opts = Options { base_dir: base, ..Options::default() };
    let spec = submit::prepare(&ctx, &task, &opts)?;
    if a.dry_run {
        let plan = submit::launch_plan(&ctx, &spec)?;
        let doc = json!({
            "dry_run": true, "agent": spec.agent, "profile": spec.profile.name, "adapter": spec.profile.adapter, "permission": spec.permission, "effort": spec.effort,
            "model": spec.model, "cwd": spec.cwd, "isolation": spec.isolation, "timeout_s": spec.timeout_s, "alternates": spec.alternates.iter().map(|p| p.name.clone()).collect::<Vec<_>>(),
            "computer_use": spec.computer_use, "warnings": spec.warnings, "launch": plan,
        });
        app.out(&doc, || {
            println!(
                "would run profile `{}` (adapter {}) at {} in {}",
                spec.profile.name,
                spec.profile.adapter,
                spec.permission,
                spec.cwd.display()
            );
            if !spec.alternates.is_empty() {
                println!(
                    "fails over to: {}",
                    spec.alternates.iter().map(|p| p.name.as_str()).collect::<Vec<_>>().join(", ")
                );
            }
            if let Some(cu) = &spec.computer_use {
                println!(
                    "computer use: {} via profile `{}` ({}), apps: {}",
                    format!("{:?}", cu.route).to_lowercase(),
                    cu.agent,
                    cu.via,
                    cu.apps.join(", ")
                );
            }
            let shown: Vec<String> = plan
                .args
                .iter()
                .map(|x| {
                    if x.chars().count() > 120 {
                        format!("{}…", x.chars().take(117).collect::<String>())
                    } else {
                        x.clone()
                    }
                })
                .map(|x| if x.contains(char::is_whitespace) { format!("{x:?}") } else { x })
                .collect();
            println!("\n{} {}", plan.program, shown.join(" "));
            if plan.stdin.is_some() {
                println!("(the prompt is sent on stdin)");
            }
            for (k, v) in &plan.env {
                println!("env {k}={v}");
            }
            for note in plan.notes.iter().chain(spec.warnings.iter()) {
                println!("note: {note}");
            }
        });
        return Ok(exit::OK);
    }
    let state = submit::submit_spec(&app.home, &spec, false)?;
    if !a.wait {
        let mut view = hufu_mcp_view(&state);
        view["warnings"] = json!(spec.warnings);
        app.out(&view, || {
            for warning in &spec.warnings {
                eprintln!("warning: {warning}");
            }
            println!("{}", state.id);
            eprintln!(
                "started on `{}` at {}. Follow with: hufu wait {id}   hufu logs {id} -f   hufu result {id}",
                spec.profile.name,
                spec.permission,
                id = state.id
            );
        });
        return Ok(exit::OK);
    }
    if !app.json {
        eprintln!("{} started on `{}` at {}", state.id, spec.profile.name, spec.permission);
    }
    let limit = a.wait_timeout.as_deref().map(|t| duration("--wait-timeout", t)).transpose()?;
    let end = if a.follow && !app.json {
        follow(app, &state.id, 0, false, limit)?
    } else {
        runner::wait(&app.home, &state.id, limit)?
    };
    if !end.state.is_terminal() {
        app.out(&hufu_mcp_view(&end), || {
            eprintln!("still {} after the wait limit; the job keeps running: hufu wait {}", end.state.as_str(), end.id)
        });
        return Ok(exit::WAIT_TIMED_OUT);
    }
    cmd_result(app, &state.id, false)
}

fn hufu_mcp_view(state: &JobState) -> Value {
    let mut v = json!({
        "job": state.id, "state": state.state.as_str(), "verdict": state.verdict.as_str(), "title": state.title, "agent": state.agent, "profile": state.profile,
        "adapter": state.adapter, "permission": state.permission, "cwd": state.cwd, "created_at": state.created_at, "elapsed_s": state.elapsed_s(), "attempt": state.attempt,
    });
    let optional: [(&str, Option<Value>); 8] = [
        ("reason", state.reason.clone().map(Value::from)),
        ("detail", state.detail.clone().map(Value::from)),
        ("last_activity", state.last_activity.clone().map(Value::from)),
        ("queue", state.queue.clone().map(Value::from)),
        ("mission", state.mission.clone().map(Value::from)),
        ("parent", state.parent.clone().map(Value::from)),
        ("started_at", state.started_at.clone().map(Value::from)),
        ("finished_at", state.finished_at.clone().map(Value::from)),
    ];
    for (key, value) in optional {
        if let Some(value) = value {
            v[key] = value;
        }
    }
    if let Some(w) = &state.workspace {
        v["workspace"] = json!(w);
    }
    if !state.labels.is_empty() {
        v["labels"] = json!(state.labels);
    }
    v
}

fn cmd_status(app: &App, a: StatusArgs) -> Result<i32> {
    if let Some(partial) = &a.job {
        let id = store::resolve_job(&app.home, partial)?;
        submit::kick(&app.home, &id);
        let state = store::load_state(&app.home, &id)?;
        app.out(&hufu_mcp_view(&state), || render::job(&state));
        return Ok(exit::OK);
    }
    let wanted = a.state.as_deref();
    let mission = a.mission.as_deref().map(|m| mission::resolve(&app.home, m)).transpose()?;
    let all: Vec<JobState> = store::list_jobs(&app.home)
        .into_iter()
        .filter(|j| wanted.map(|s| j.state.as_str() == s).unwrap_or(true))
        .filter(|j| mission.as_ref().map(|m| j.mission.as_ref() == Some(m)).unwrap_or(true))
        .collect();
    let shown: Vec<JobState> = if a.all || wanted.is_some() || mission.is_some() {
        all
    } else {
        let active: Vec<JobState> = all.iter().filter(|j| !j.state.is_terminal()).cloned().collect();
        let mut recent: Vec<JobState> =
            all.iter().rev().filter(|j| j.state.is_terminal()).take(a.limit).cloned().collect();
        recent.reverse();
        recent.into_iter().chain(active).collect()
    };
    app.out(&json!({"jobs": shown.iter().map(hufu_mcp_view).collect::<Vec<_>>()}), || render::jobs(&shown));
    Ok(exit::OK)
}

fn cmd_wait(app: &App, a: WaitArgs) -> Result<i32> {
    let ids: Vec<String> = a.jobs.iter().map(|j| store::resolve_job(&app.home, j)).collect::<Result<_>>()?;
    let limit = a.timeout.as_deref().map(|t| duration("--timeout", t)).transpose()?;
    let started = Instant::now();
    for id in &ids {
        submit::kick(&app.home, id);
    }
    let states: Vec<JobState> = loop {
        let states: Vec<JobState> = ids.iter().map(|id| store::load_state(&app.home, id)).collect::<Result<_>>()?;
        let finished = states.iter().filter(|s| s.state.is_terminal()).count();
        if finished == states.len() || (a.any && finished > 0) || limit.map(|l| started.elapsed() >= l).unwrap_or(false)
        {
            break states;
        }
        std::thread::sleep(Duration::from_millis(150));
    };
    let pending = states.iter().any(|s| !s.state.is_terminal());
    let timed_out = pending && !(a.any && states.iter().any(|s| s.state.is_terminal()));
    app.out(
        &json!({"wait_timed_out": timed_out, "jobs": states.iter().map(hufu_mcp_view).collect::<Vec<_>>()}),
        || render::jobs(&states),
    );
    if timed_out {
        return Ok(exit::WAIT_TIMED_OUT);
    }
    // The worst outcome decides the exit code, so `hufu wait a b && next` is safe.
    Ok(states
        .iter()
        .filter(|s| s.state.is_terminal())
        .map(|s| render::exit_code(s.state, s.verdict, s.reason.as_deref()))
        .max()
        .unwrap_or(exit::OK))
}

fn cmd_result(app: &App, partial: &str, full: bool) -> Result<i32> {
    let id = store::resolve_job(&app.home, partial)?;
    let Some(result) = runner::result_of(&app.home, &id)? else {
        let state = store::load_state(&app.home, &id)?;
        app.out(&json!({"job": id, "state": state.state.as_str(), "finished": false}), || {
            eprintln!("{id} is still {}; wait for it with: hufu wait {id}", state.state.as_str())
        });
        return Ok(exit::WAIT_TIMED_OUT);
    };
    let max = app.load().map(|l| l.config.defaults.result_chars).unwrap_or(4000);
    app.out(&result, || render::result(&result, (!full).then_some(max)));
    Ok(render::exit_code(result.state, result.verdict, result.reason.as_deref()))
}

/// Prints events as they arrive until the job ends or the limit passes.
fn follow(app: &App, id: &str, mut after: u64, as_json: bool, limit: Option<Duration>) -> Result<JobState> {
    let started = Instant::now();
    submit::kick(&app.home, id);
    loop {
        let state = store::load_state(&app.home, id)?;
        for event in store::read_events(&app.home, id, after) {
            after = event.seq;
            if as_json {
                println!("{}", serde_json::to_string(&event).unwrap_or_default());
            } else {
                let time = event.ts.get(11..19).unwrap_or("");
                eprintln!("{time}  {}", event.body.summary());
            }
        }
        if state.state.is_terminal() || limit.map(|l| started.elapsed() >= l).unwrap_or(false) {
            return Ok(state);
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn cmd_logs(app: &App, a: LogsArgs) -> Result<i32> {
    let id = store::resolve_job(&app.home, &a.job)?;
    if a.raw {
        let path = app.home.job(&id).join("raw").join(if a.stderr { "stderr.log" } else { "stdout.log" });
        let mut offset = 0usize;
        loop {
            let bytes = std::fs::read(&path).unwrap_or_default();
            if bytes.len() > offset {
                use std::io::Write;
                let _ = std::io::stdout().write_all(&bytes[offset..]);
                let _ = std::io::stdout().flush();
                offset = bytes.len();
            }
            if !a.follow || store::load_state(&app.home, &id)?.state.is_terminal() {
                break;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        return Ok(exit::OK);
    }
    if a.follow {
        // Events go to stdout here: following is the whole point of the command.
        let mut after = a.since;
        loop {
            let state = store::load_state(&app.home, &id)?;
            for event in store::read_events(&app.home, &id, after) {
                after = event.seq;
                if app.json {
                    println!("{}", serde_json::to_string(&event).unwrap_or_default());
                } else {
                    println!("{:>4}  {}  {}", event.seq, event.ts.get(11..19).unwrap_or(""), event.body.summary());
                }
            }
            if state.state.is_terminal() {
                break;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        return Ok(exit::OK);
    }
    for event in store::read_events(&app.home, &id, a.since) {
        if app.json {
            println!("{}", serde_json::to_string(&event).unwrap_or_default());
        } else {
            println!("{:>4}  {}  {}", event.seq, event.ts.get(11..19).unwrap_or(""), event.body.summary());
        }
    }
    Ok(exit::OK)
}

fn cmd_cancel(app: &App, a: CancelArgs) -> Result<i32> {
    let mut states = Vec::new();
    for partial in &a.jobs {
        let id = store::resolve_job(&app.home, partial)?;
        states.push(if a.force { runner::force_cancel(&app.home, &id)? } else { runner::cancel(&app.home, &id)? });
    }
    app.out(&json!({"jobs": states.iter().map(hufu_mcp_view).collect::<Vec<_>>()}), || {
        for s in &states {
            match s.state {
                State::Cancelled => println!("{} cancelled", s.id),
                state if state.is_terminal() => {
                    println!("{} had already ended ({})", s.id, state.as_str())
                }
                _ => println!("{} is stopping; `hufu wait {}` returns when it has", s.id, s.id),
            }
        }
    });
    Ok(exit::OK)
}

fn cmd_resume(app: &App, a: ResumeArgs) -> Result<i32> {
    let id = store::resolve_job(&app.home, &a.job)?;
    let prompt = read_prompt(&a.prompt, None)?
        .ok_or_else(|| HufuError::usage("give the follow-up prompt as an argument, or `-` to read it from stdin"))?;
    let spec = store::load_spec(&app.home, &id)?;
    let loaded = config::load(&app.home, &spec.cwd)?;
    let ctx = Context { home: &app.home, config: &loaded.config, adapters: &app.adapters };
    let state = submit::resume(&ctx, &id, &prompt, false, |_| {})?;
    if a.wait {
        runner::wait(&app.home, &state.id, None)?;
        return cmd_result(app, &state.id, false);
    }
    app.out(&hufu_mcp_view(&state), || {
        println!("{}", state.id);
        eprintln!("follow-up to {id} started. Follow with: hufu wait {}", state.id);
    });
    Ok(exit::OK)
}

fn cmd_agents(app: &App, a: AgentsArgs) -> Result<i32> {
    let loaded = app.load()?;
    let list = ops::agents(&loaded, &app.adapters, a.probe);
    app.out(&json!({"agents": list, "roles": loaded.config.roles, "computer_use": loaded.config.capabilities.computer_use}), || {
        render::agents(&list, a.probe);
        let roles = &loaded.config.roles;
        println!("\nadvisor: {}   executors: {}   computer use: {}", roles.advisor.as_deref().unwrap_or("not set"), if roles.executors.is_empty() { "not set".to_string() } else { roles.executors.join(", ") }, loaded.config.capabilities.computer_use.agent.as_deref().unwrap_or("not set"));
        if !a.probe {
            println!("`hufu agents --probe` also checks installation and login; `hufu profile add` adds an agent or another account.");
        }
    });
    Ok(exit::OK)
}

fn cmd_doctor(app: &App, a: DoctorArgs) -> Result<i32> {
    let loaded = app.load()?;
    let report = doctor::doctor(&app.home, &loaded, &app.adapters, VERSION);
    let tests = a.self_test.then(|| doctor::self_test(&app.adapters));
    let mut doc = serde_json::to_value(&report).map_err(|e| HufuError::internal(e.to_string()))?;
    if let Some(tests) = &tests {
        doc["self_test"] = json!(tests);
    }
    app.out(&doc, || {
        render::doctor(&report);
        if let Some(tests) = &tests {
            render::self_test(tests);
        }
    });
    let failed = !report.problems.is_empty() || tests.map(|t| t.iter().any(|x| !x.ok)).unwrap_or(false);
    Ok(if failed { exit::INTERNAL } else { exit::OK })
}

fn cmd_prune(app: &App, a: PruneArgs) -> Result<i32> {
    let loaded = app.load()?;
    let opts = PruneOptions {
        older_than: a.older_than.as_deref().map(|t| duration("--older-than", t)).transpose()?,
        keep: a.keep,
        worktrees: a.worktrees,
        dry_run: a.dry_run,
    };
    let report = ops::prune(&app.home, &loaded.config, &opts)?;
    app.out(&report, || {
        let verb = if report.dry_run { "would remove" } else { "removed" };
        println!("{verb} {} job(s), {} mission(s), {} worktree(s); {} job(s) remain", report.removed_jobs.len(), report.removed_missions.len(), report.removed_worktrees.len(), report.remaining_jobs);
        if !report.kept_for_worktree.is_empty() {
            println!("{} finished job(s) still have a worktree and were kept; pass --worktrees to remove them (branches stay)", report.kept_for_worktree.len());
        }
    });
    Ok(exit::OK)
}

// ---------------------------------------------------------------------------------------------
// Missions
// ---------------------------------------------------------------------------------------------

fn mission_view(home: &Home, m: &mission::Mission) -> Value {
    let mut v = serde_json::to_value(m).unwrap_or(Value::Null);
    if m.state.is_terminal() {
        v["report_path"] = json!(home.mission(&m.id).join("report.md"));
    }
    v
}

fn wait_mission(app: &App, id: &str, limit: Option<Duration>) -> Result<mission::Mission> {
    let started = Instant::now();
    loop {
        let m = mission::load(&app.home, id)?;
        if m.state.is_terminal()
            || m.state == MissionState::Paused
            || limit.map(|l| started.elapsed() >= l).unwrap_or(false)
        {
            return Ok(m);
        }
        std::thread::sleep(Duration::from_millis(300));
    }
}

fn mission_exit(m: &mission::Mission) -> i32 {
    match m.state {
        MissionState::Succeeded => exit::OK,
        MissionState::Failed => exit::JOB_FAILED,
        MissionState::Aborted => exit::JOB_CANCELLED,
        _ => exit::WAIT_TIMED_OUT,
    }
}

fn cmd_mission(app: &App, c: MissionCommand) -> Result<i32> {
    let home = &app.home;
    match c {
        MissionCommand::Start(a) => {
            let loaded = app.load()?;
            let mut spec = match &a.file {
                Some(path) => MissionSpec::from_file(path)?,
                None => MissionSpec::default(),
            };
            if let Some(goal) = read_prompt(&a.goal, None)? {
                spec.goal = goal;
            }
            if a.advisor.is_some() {
                spec.advisor = a.advisor.clone();
            }
            if !a.executors.is_empty() {
                spec.executors = a.executors.clone();
            }
            if a.cwd.is_some() {
                spec.cwd = a.cwd.clone();
            }
            if let Some(i) = &a.isolation {
                spec.isolation = Some(Isolation::parse(i).map_err(HufuError::usage)?);
            }
            if let Some(r) = &a.review {
                spec.review = Some(match r.as_str() {
                    "always" => ReviewPolicy::Always,
                    "on_failure" | "on-failure" => ReviewPolicy::OnFailure,
                    "never" => ReviewPolicy::Never,
                    other => {
                        return Err(HufuError::usage(format!(
                            "unknown --review `{other}`; accepted: always, on_failure, never"
                        )));
                    }
                });
            }
            if let Some(t) = &a.max_tier {
                spec.max_tier = Some(Tier::parse(t).map_err(HufuError::usage)?);
            }
            if let Some(n) = a.max_tasks {
                let mut budgets = spec.budgets.clone().unwrap_or_else(|| loaded.config.missions.budgets.clone());
                budgets.max_tasks = n;
                spec.budgets = Some(budgets);
            }
            if !a.allow_command.is_empty() {
                spec.acceptance_commands = Some(a.allow_command.clone());
            }
            let base = a
                .file
                .as_ref()
                .and_then(|f| f.parent())
                .filter(|p| !p.as_os_str().is_empty() && a.cwd.is_none())
                .map(|p| app.cwd.join(p))
                .unwrap_or_else(|| app.cwd.clone());
            let ctx = Context { home, config: &loaded.config, adapters: &app.adapters };
            let created = mission::create(&ctx, &spec, &base)?;
            mission::launch(home, &created.id)?;
            if a.wait {
                if !app.json {
                    eprintln!(
                        "{} started: advisor `{}`, executors {}",
                        created.id,
                        created.advisor,
                        created.executors.join(", ")
                    );
                }
                let done = wait_mission(app, &created.id, None)?;
                app.out(&mission_view(home, &done), || print!("{}", mission::report(home, &done)));
                return Ok(mission_exit(&done));
            }
            app.out(&mission_view(home, &created), || {
                println!("{}", created.id);
                eprintln!("mission started: advisor `{}`, executors {}. Follow with: hufu mission status {id}   hufu mission wait {id}", created.advisor, created.executors.join(", "), id = created.id);
            });
            Ok(exit::OK)
        }
        MissionCommand::Status { mission: None } | MissionCommand::List => {
            let all = mission::list(home);
            app.out(&json!({"missions": all.iter().map(|m| json!({"mission": m.id, "title": m.title, "state": m.state.as_str(), "advisor": m.advisor, "tasks": m.tasks.len(), "created_at": m.created_at})).collect::<Vec<_>>()}), || {
                if all.is_empty() {
                    println!("no missions");
                } else {
                    let rows: Vec<Vec<String>> = all.iter().map(|m| vec![m.id.clone(), m.state.as_str().into(), m.advisor.clone(), format!("{}/{}", m.tasks.iter().filter(|t| t.state == mission::TaskState::Accepted).count(), m.tasks.len()), render::clip(&m.title, 70)]).collect();
                    render::table(&["MISSION", "STATE", "ADVISOR", "DONE", "TITLE"], &rows);
                }
            });
            Ok(exit::OK)
        }
        MissionCommand::Status { mission: Some(partial) } => {
            let m = mission::load(home, &mission::resolve(home, &partial)?)?;
            app.out(&mission_view(home, &m), || render::mission(&m));
            Ok(exit::OK)
        }
        MissionCommand::Report { mission: partial } => {
            let m = mission::load(home, &mission::resolve(home, &partial)?)?;
            let text = mission::report(home, &m);
            app.out(&json!({"mission": m.id, "state": m.state.as_str(), "report": text}), || print!("{text}"));
            Ok(exit::OK)
        }
        MissionCommand::Wait { mission: partial, timeout } => {
            let id = mission::resolve(home, &partial)?;
            let m = wait_mission(app, &id, timeout.as_deref().map(|t| duration("--timeout", t)).transpose()?)?;
            app.out(&mission_view(home, &m), || render::mission(&m));
            Ok(mission_exit(&m))
        }
        MissionCommand::Pause { mission: partial } => {
            let id = mission::resolve(home, &partial)?;
            mission::pause(home, &id)?;
            app.out(&json!({"mission": id, "pausing": true}), || {
                println!("{id} will pause once its running jobs are done; continue with: hufu mission resume {id}")
            });
            Ok(exit::OK)
        }
        MissionCommand::Resume { mission: partial } => {
            let id = mission::resolve(home, &partial)?;
            mission::resume(home, &id)?;
            app.out(&json!({"mission": id, "resumed": true}), || println!("{id} resumed"));
            Ok(exit::OK)
        }
        MissionCommand::Abort { mission: partial } => {
            let id = mission::resolve(home, &partial)?;
            let m = mission::abort(home, &id)?;
            app.out(&json!({"mission": id, "state": m.state.as_str()}), || {
                println!("{id}: abort requested; running jobs are being cancelled, finished work is kept")
            });
            Ok(exit::OK)
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Updating Hufu itself
// ---------------------------------------------------------------------------------------------

/// What to do about a release that was found.
#[derive(Clone, Copy, PartialEq)]
enum Choice {
    Install,
    UpToDate,
    /// The installed program is newer than the newest release (a build from source).
    Ahead,
}

fn choose(current: &str, release: &update::Release, asked_for: bool, force: bool) -> Choice {
    use std::cmp::Ordering;
    use update::Version;
    let order = match (Version::parse(&release.version), Version::parse(current)) {
        (Some(new), Some(old)) => new.cmp(&old),
        _ if release.version == current => Ordering::Equal,
        _ => Ordering::Greater,
    };
    match order {
        _ if force => Choice::Install,
        Ordering::Equal => Choice::UpToDate,
        Ordering::Greater => Choice::Install,
        // Going back to an older release is fine when it was asked for by name.
        Ordering::Less if asked_for => Choice::Install,
        Ordering::Less => Choice::Ahead,
    }
}

fn cmd_update(app: &App, a: UpdateArgs) -> Result<i32> {
    let exe = update::installed_exe()?;
    let source = update::Source::from_env();
    let scratch = update::Scratch::new()?;
    let note = |text: &str| {
        if !app.json {
            eprintln!("{text}");
        }
    };
    // A wrong `--version` is a usage error, whatever else was asked for.
    let wanted_tag = a.version.as_deref().map(update::tag_for).transpose()?;
    let release =
        if a.from_source { None } else { update::find_release(&source, a.version.as_deref(), scratch.path())? };

    if a.check {
        let doc = match &release {
            Some(r) => {
                let v = choose(VERSION, r, a.version.is_some(), false);
                json!({
                    "action": "check", "current": VERSION, "latest": r.version, "tag": r.tag, "release_found": true,
                    "update_available": v == Choice::Install, "binary": r.asset.is_some(), "source": source.describe(), "exe": exe,
                })
            }
            None => json!({
                "action": "check", "current": VERSION, "latest": null, "tag": wanted_tag, "release_found": false,
                "update_available": false, "binary": false, "source": source.describe(), "exe": exe,
            }),
        };
        app.out(&doc, || {
            println!("installed: hufu {VERSION} ({})", exe.display());
            match &release {
                Some(r) => {
                    let what = if a.version.is_some() { "release" } else { "latest release" };
                    println!("{what}: {} ({})", r.version, r.tag);
                    match choose(VERSION, r, a.version.is_some(), false) {
                        Choice::Install if r.asset.is_some() => match &a.version {
                            Some(v) => println!("that is not the installed version: `hufu update --version {v}` installs it"),
                            None => println!("an update is available: run `hufu update`"),
                        },
                        Choice::Install => println!(
                            "an update is available, but without a binary for this platform: `hufu update` would build it from source"
                        ),
                        Choice::UpToDate => println!("up to date"),
                        Choice::Ahead => println!("the installed program is newer than the latest release; nothing to do"),
                    }
                }
                None => match &wanted_tag {
                    Some(tag) => println!("there is no release {tag} at {}", source.describe()),
                    None => println!(
                        "no release is published at {}; `hufu update` would rebuild from the main branch with cargo",
                        source.describe()
                    ),
                },
            }
        });
        return Ok(exit::OK);
    }

    // Decide where the new program comes from, or that there is nothing to do.
    let mut build_tag: Option<Option<String>> = None;
    match &release {
        _ if a.from_source => build_tag = Some(wanted_tag.clone()),
        Some(r) => {
            let v = choose(VERSION, r, a.version.is_some(), a.force);
            if v != Choice::Install {
                let reason = if v == Choice::Ahead { "newer_than_latest" } else { "up_to_date" };
                let doc = json!({"action": "update", "updated": false, "reason": reason, "current": VERSION, "latest": r.version, "tag": r.tag, "exe": exe});
                app.out(&doc, || match v {
                    Choice::Ahead => println!(
                        "hufu {VERSION} is newer than the latest release ({}); nothing to do. `hufu update --version {}` installs that release",
                        r.version, r.tag
                    ),
                    _ => println!("hufu {VERSION} is up to date. `hufu update --force` installs it again"),
                });
                return Ok(exit::OK);
            }
            if r.asset.is_none() {
                note(&format!(
                    "Release {} has no binary for this platform ({} {}); building it from source.",
                    r.tag,
                    std::env::consts::OS,
                    std::env::consts::ARCH
                ));
                build_tag = Some(Some(r.tag.clone()));
            }
        }
        None => match &wanted_tag {
            Some(tag) => {
                return Err(HufuError::usage(format!(
                    "there is no release {tag} at {}. To build that tag from source, add --from-source",
                    source.describe()
                )));
            }
            None => {
                note(&format!(
                    "No release is published at {}; building the main branch from source.",
                    source.describe()
                ));
                build_tag = Some(None);
            }
        },
    }

    update::check_writable(&exe)?;
    let (new, method) = match (&build_tag, &release) {
        (Some(tag), _) => {
            note("Building Hufu with cargo (this takes a minute)...");
            (update::build_from_source(tag.as_deref(), scratch.path(), app.json)?, "source")
        }
        (None, Some(r)) => {
            note(&format!("Downloading Hufu {} ...", r.version));
            (update::download(r, scratch.path())?, "release")
        }
        (None, None) => return Err(HufuError::internal("no release and no build to install")),
    };
    // The new program must start here, and must be what the release says it is.
    let version = update::probe_version(&new)?;
    if let (None, Some(r)) = (&build_tag, &release)
        && version != r.version
    {
        return Err(HufuError::internal(format!(
            "release {} holds a program that reports version {version}. Nothing was changed",
            r.tag
        )));
    }
    let daemon_before = hufu_daemon::status(&app.home);
    update::install(&new, &exe)?;
    drop(scratch);

    // From here on the new program is in place: everything else is done by running it.
    let helper = |args: &[&str]| {
        let mut cmd = std::process::Command::new(&exe);
        cmd.args(args).env("HUFU_HOME", &app.home.root).current_dir(&app.cwd);
        cmd
    };
    // Windows only: the old program was moved aside and cannot be removed while this process,
    // which runs from it, is alive. A short-lived helper removes it afterwards.
    let aside = update::sweep_old(&exe);
    if !aside.is_empty() {
        let mut cmd = helper(&["__sweep"]);
        cmd.arg(&exe)
            .args(&aside)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        hufu_contract::proc::detach(&mut cmd);
        let _ = cmd.spawn();
    }
    let daemon = match &daemon_before {
        None => json!({"was_running": false, "running": false}),
        Some(_) => {
            // If the old daemon cannot be stopped, the report below names the one that runs.
            let _ = hufu_daemon::stop(&app.home);
            let started = hufu_contract::proc::run_capture(helper(&["daemon", "start"]), Duration::from_secs(60));
            match (hufu_daemon::status(&app.home), started) {
                (Some(info), _) => {
                    json!({"was_running": true, "running": true, "pid": info.pid, "version": info.version})
                }
                (None, Ok(out)) => json!({"was_running": true, "running": false, "error": out.stderr.trim()}),
                (None, Err(e)) => json!({"was_running": true, "running": false, "error": e.to_string()}),
            }
        }
    };
    let daemon_failed = daemon["was_running"] == true && daemon["running"] == false;

    if !app.json {
        println!("updated hufu {VERSION} -> {version} ({})", exe.display());
        match (daemon["was_running"] == true, daemon_failed) {
            (false, _) => {}
            (true, false) => println!("daemon restarted (pid {})", daemon["pid"]),
            (true, true) => println!(
                "the daemon was stopped but did not start again: {}. Start it with `hufu daemon start`",
                daemon["error"].as_str().unwrap_or("unknown reason")
            ),
        }
        println!(
            "running jobs finish with the version they started with; agents that have Hufu connected use the new one after their next restart"
        );
        println!();
    }
    // The checks are the new program's own.
    let (doctor_code, doctor_doc) = if app.json {
        let out = hufu_contract::proc::run_capture(helper(&["--json", "doctor"]), Duration::from_secs(120))
            .map_err(|e| HufuError::internal(format!("updated, but `hufu doctor` did not start: {e}")))?;
        (out.code.unwrap_or(exit::INTERNAL), serde_json::from_str::<Value>(out.stdout.trim()).unwrap_or(Value::Null))
    } else {
        let status = helper(&["doctor"])
            .status()
            .map_err(|e| HufuError::internal(format!("updated, but `hufu doctor` did not start: {e}")))?;
        (status.code().unwrap_or(exit::INTERNAL), Value::Null)
    };
    let doc = json!({
        "action": "update", "updated": true, "from": VERSION, "to": version, "method": method, "exe": exe,
        "daemon": daemon, "doctor_ok": doctor_code == exit::OK, "doctor": doctor_doc,
    });
    app.out(&doc, || {
        if doctor_code != exit::OK {
            eprintln!("hufu: the update is installed, but `hufu doctor` reports problems (see above)");
        }
    });
    Ok(if doctor_code != exit::OK {
        doctor_code
    } else if daemon_failed {
        exit::INTERNAL
    } else {
        exit::OK
    })
}

// ---------------------------------------------------------------------------------------------
// Setup and configuration
// ---------------------------------------------------------------------------------------------

fn cmd_daemon(app: &App, c: DaemonCommand) -> Result<i32> {
    match c {
        DaemonCommand::Run => {
            hufu_daemon::run(&app.home, VERSION)?;
            Ok(exit::OK)
        }
        DaemonCommand::Start => {
            let info = hufu_daemon::start(&app.home)?;
            app.out(&json!({"running": true, "daemon": info}), || println!("daemon running (pid {})", info.pid));
            Ok(exit::OK)
        }
        DaemonCommand::Stop => {
            let was = hufu_daemon::stop(&app.home)?;
            app.out(&json!({"running": false, "stopped": was}), || {
                println!(
                    "{}",
                    if was { "daemon stopped; running jobs are not affected" } else { "no daemon was running" }
                )
            });
            Ok(exit::OK)
        }
        DaemonCommand::Status => {
            let info = hufu_daemon::status(&app.home);
            app.out(&json!({"running": info.is_some(), "daemon": info}), || match &info {
                Some(i) => println!("daemon running (pid {}, since {})", i.pid, i.started_at),
                None => println!("daemon not running. It is optional: without it each job waits for its own turn. Start it with `hufu daemon start`"),
            });
            Ok(exit::OK)
        }
    }
}

fn print_init(report: &setup::InitReport, with_hints: bool) {
    match report.action {
        "kept" => println!("configuration kept: {} (use `hufu init --force` to write a fresh one)", report.path),
        "skipped" => println!("configuration not written: no supported agent was found on PATH ({})", report.path),
        action => {
            println!("configuration {action}: {}", report.path);
            if report.detected.is_empty() {
                println!("no supported agent was found on PATH, so the file names none");
            } else {
                println!("agents found: {}", report.detected.join(", "));
                println!(
                    "default advisor: {}   default executors: {}",
                    report.advisor.as_deref().unwrap_or("-"),
                    report.executors.join(", ")
                );
            }
        }
    }
    if with_hints {
        for hint in &report.hints {
            println!("note: {hint}");
        }
    }
}

fn cmd_init(app: &App, force: bool) -> Result<i32> {
    let report = setup::init(&app.home, &app.adapters, force)?;
    app.out(&report, || print_init(&report, true));
    // Nothing was written because there is no agent to name: the caller has a step to do first.
    Ok(if report.action == "skipped" { exit::AGENT_UNAVAILABLE } else { exit::OK })
}

fn print_actions(actions: &[setup::SetupAction]) {
    for a in actions {
        let place = a.path.as_ref().map(|p| format!(" ({p})")).unwrap_or_default();
        if a.action == "manual" {
            println!("{}: add this yourself{place}:\n{}", a.host, a.detail);
        } else {
            println!("{}: {}{place} — {}", a.host, a.action, a.detail);
        }
    }
}

fn cmd_setup(app: &App, a: SetupArgs) -> Result<i32> {
    let hosts: Vec<String> = if !a.agent.is_empty() {
        a.agent.clone()
    } else if a.all {
        setup::HOSTS.iter().map(|s| s.to_string()).collect()
    } else {
        setup::detect_hosts()
    };
    let opts = setup::SetupOptions {
        remove: a.remove,
        print: a.print,
        project: a.project.then(|| app.cwd.clone()),
        exe: submit::hufu_exe()?,
    };
    let actions = setup::setup(&hosts, &opts);
    app.out(&json!({"actions": actions}), || {
        if hosts.is_empty() {
            print_no_host(&setup::generic_snippet(&opts.exe));
        }
        print_actions(&actions);
    });
    Ok(if actions.iter().any(|x| x.action == "failed") { exit::INTERNAL } else { exit::OK })
}

fn print_no_host(snippet: &str) {
    println!("no agent that could host Hufu was found. For any MCP-capable agent, add this server:\n{snippet}");
}

/// The profile or pool to suggest for a first job: a default executor if it can run, otherwise
/// any agent that is installed and not known to be logged out.
fn first_usable(report: &doctor::DoctorReport) -> Option<String> {
    let usable = |p: &doctor::ProfileCheck| p.installed && p.authenticated != Some(false) && p.missing_env.is_empty();
    let by_name = |name: &str| report.profiles.iter().find(|p| p.name == name);
    report
        .executors
        .iter()
        // A name that is not a profile is a pool; its members are checked when a job is sent.
        .find(|name| by_name(name).map(usable).unwrap_or(true))
        .cloned()
        .or_else(|| report.advisor.clone().filter(|name| by_name(name).map(usable).unwrap_or(false)))
        .or_else(|| report.profiles.iter().find(|p| usable(p)).map(|p| p.name.clone()))
}

fn cmd_up(app: &App, a: UpArgs) -> Result<i32> {
    let init = setup::init(&app.home, &app.adapters, false)?;
    let loaded = app.load()?;
    let mut report = doctor::doctor(&app.home, &loaded, &app.adapters, VERSION);

    // With no agent there is nothing to dispatch to. Stop here, before other agents' settings are
    // edited or a daemon is started, and say what is missing instead of reporting success.
    if !report.profiles.iter().any(|p| p.installed) {
        let rewrite = init.action == "kept" && report.advisor.is_none() && report.executors.is_empty();
        let next = setup::after_install(rewrite);
        let doc = json!({
            "ready": false,
            "reason": "no_agent",
            "next": format!("install and log in to at least one of {}, then {next}", setup::SUPPORTED_AGENTS),
            "init": init,
            "setup": [],
            "daemon": {"running": false, "skipped": true},
            "doctor": report,
        });
        app.out(&doc, || {
            print_init(&init, false);
            println!("\nHufu is installed, but it has no agent to dispatch work to, so setup stopped here:");
            println!("nothing was registered with other agents and the daemon was not started.");
            println!("\nInstall at least one of these and log in to it:");
            for name in setup::PRESET_ORDER {
                if let Some(hint) = report.profiles.iter().find(|p| p.name == *name).and_then(|p| p.hint.as_deref()) {
                    println!("  {name:<7} {hint}");
                }
            }
            println!("\nThen {next}.");
            eprintln!("hufu: not ready: no supported agent is installed");
        });
        return Ok(exit::AGENT_UNAVAILABLE);
    }

    let hosts = if a.no_setup { None } else { Some(setup::detect_hosts()) };
    let actions = match &hosts {
        None => Vec::new(),
        Some(hosts) => {
            let opts = setup::SetupOptions { remove: false, print: false, project: None, exe: submit::hufu_exe()? };
            // With no host to register with, hand back the entry to add by hand.
            let generic = ["generic".to_string()];
            setup::setup(if hosts.is_empty() { &generic } else { hosts }, &opts)
        }
    };
    let no_host = hosts.as_ref().is_some_and(|h| h.is_empty());
    let daemon = if a.no_daemon { None } else { Some(hufu_daemon::start(&app.home)) };
    report.daemon = submit::daemon_info(&app.home);
    let daemon_json = match &daemon {
        Some(Ok(info)) => json!({"running": true, "pid": info.pid}),
        Some(Err(e)) => json!({"running": false, "error": e.message}),
        None => json!({"running": false, "skipped": true}),
    };
    let valid = report.problems.is_empty();
    let try_with = first_usable(&report);
    let ready = valid && try_with.is_some();
    let doc = json!({"ready": ready, "init": init, "setup": actions, "daemon": daemon_json, "doctor": report});
    app.out(&doc, || {
        print_init(&init, false);
        match actions.first() {
            Some(manual) if no_host => print_no_host(&manual.detail),
            _ => print_actions(&actions),
        }
        match &daemon {
            Some(Ok(info)) => println!("daemon running (pid {})", info.pid),
            Some(Err(e)) => println!("daemon not started: {}", e.message),
            None => {}
        }
        println!();
        render::doctor(&report);
        let example = |agent: &str| format!("hufu run -a {agent} \"Summarise this repository\" --wait");
        match (&try_with, valid) {
            (_, false) => println!("\nnot ready: fix the configuration problems above, then run `hufu up` again."),
            (Some(agent), true) => println!("\nready. Try: {}", example(agent)),
            (None, true) => println!(
                "\nset up, but not ready: no installed agent is logged in. Follow the next steps above, then try: {}",
                example(report.profiles.iter().find(|p| p.installed).map(|p| p.name.as_str()).unwrap_or("<profile>"))
            ),
        }
    });
    Ok(if valid { exit::OK } else { exit::INTERNAL })
}

fn cmd_config(app: &App, c: ConfigCommand) -> Result<i32> {
    match c {
        ConfigCommand::Get { key } => {
            let loaded = app.load()?;
            let value = ops::config_get(&loaded.config, key.as_deref())?;
            if app.json {
                println!("{}", json!({"v": 1, "key": key, "value": value}));
            } else {
                match &value {
                    Value::String(s) => println!("{s}"),
                    Value::Object(_) | Value::Array(_) => print!("{}", config::to_yaml(&value)?),
                    other => println!("{other}"),
                }
            }
        }
        ConfigCommand::Set { key, value, project } => {
            let path = app.target(project);
            ops::config_set(&path, &app.adapters, &key, config::parse_scalar(&value))?;
            app.out(&json!({"written": path.path, "key": key}), || println!("set {key} in {}", path.display()));
        }
        ConfigCommand::Unset { key, project } => {
            let path = app.target(project);
            ops::config_unset(&path, &app.adapters, &key)?;
            app.out(&json!({"written": path.path, "key": key}), || println!("removed {key} from {}", path.display()));
        }
        ConfigCommand::Path => {
            let loaded = app.load()?;
            app.out(&json!({"global": loaded.global_path, "global_exists": loaded.global_path.exists(), "project": loaded.project_path}), || {
                println!("{}{}", loaded.global_path.display(), if loaded.global_path.exists() { "" } else { "  (not created yet; `hufu init` writes it)" });
                if let Some(p) = &loaded.project_path {
                    println!("{}  (project, overrides the user-wide file)", p.display());
                }
            });
        }
        ConfigCommand::Validate => {
            let loaded = app.load()?;
            let problems = loaded.config.problems(&app.adapters);
            app.out(&json!({"valid": problems.is_empty(), "problems": problems}), || {
                if problems.is_empty() {
                    println!(
                        "the configuration is valid ({} profiles, {} pools)",
                        loaded.config.profiles.len(),
                        loaded.config.pools.len()
                    );
                } else {
                    for p in &problems {
                        println!("{p}");
                    }
                }
            });
            return Ok(if problems.is_empty() { exit::OK } else { exit::USAGE });
        }
    }
    Ok(exit::OK)
}

fn profile_body(a: &ProfileArgs) -> Result<Value> {
    let mut body = serde_json::Map::new();
    let mut put = |key: &str, value: Option<Value>| {
        if let Some(value) = value {
            body.insert(key.to_string(), value);
        }
    };
    put("adapter", a.adapter.clone().map(Value::from));
    put("description", a.description.clone().map(Value::from));
    put("model", a.model.clone().map(Value::from));
    put("provider", a.provider.clone().map(Value::from));
    put("account", a.account.clone().map(Value::from));
    put("home", a.home.clone().map(Value::from));
    if let Some(e) = &a.effort {
        Effort::parse(e).map_err(HufuError::usage)?;
        put("effort", Some(json!(e)));
    }
    if let Some(f) = &a.effort_fallback {
        if !["error", "nearest"].contains(&f.as_str()) {
            return Err(HufuError::usage(format!("unknown effort fallback `{f}`; accepted: error, nearest")));
        }
        put("effort_fallback", Some(json!(f)));
    }
    if let Some(p) = &a.permission {
        put("permission", Some(json!(Tier::parse(p).map_err(HufuError::usage)?)));
    }
    put("allow_full_access", a.allow_full_access.then_some(Value::Bool(true)));
    put("env", (!a.env.is_empty()).then(|| json!(a.env)));
    put("env_from", (!a.env_from.is_empty()).then(|| pairs("--env-from", &a.env_from).map(|m| json!(m))).transpose()?);
    put("vars", (!a.var.is_empty()).then(|| pairs("--var", &a.var).map(|m| json!(m))).transpose()?);
    put("command", (!a.command.is_empty()).then(|| json!(a.command)));
    put("provides", a.computer_use.as_ref().map(|via| json!({"computer_use": {"via": via}})));
    put("permission_requests", a.verified.then(|| json!("verified")));
    Ok(Value::Object(body))
}

fn cmd_profile(app: &App, c: ProfileCommand) -> Result<i32> {
    match c {
        ProfileCommand::Add(a) => {
            if a.adapter.is_none() {
                return Err(HufuError::usage(format!(
                    "`hufu profile add` needs `--adapter`; accepted: {}",
                    hufu_contract::AdapterRegistry::kinds(&app.adapters).join(", ")
                )));
            }
            let path = app.target(a.project);
            ops::profile_set(&path, &app.adapters, &a.name, profile_body(&a)?, false)?;
            app.out(&json!({"written": path.path, "profile": a.name}), || {
                println!("profile `{}` written to {}. Check it with: hufu doctor", a.name, path.display())
            });
        }
        ProfileCommand::Set(a) => {
            let path = app.target(a.project);
            // A preset has no entry in the file yet; changing one starts from the preset.
            let loaded = app.load()?;
            let mut body = profile_body(&a)?;
            let in_file =
                config::get_path(&config::read_raw(&path.path)?, "profiles").and_then(|p| p.get(&a.name)).is_some();
            if !in_file {
                let base = loaded.config.profile(&a.name).ok_or_else(|| {
                    HufuError::usage(format!("no profile `{}`; create it with `hufu profile add`", a.name))
                })?;
                let mut merged = serde_json::to_value(base).map_err(|e| HufuError::internal(e.to_string()))?;
                if let (Some(m), Some(b)) = (merged.as_object_mut(), body.as_object()) {
                    m.remove("name");
                    for (k, v) in b {
                        m.insert(k.clone(), v.clone());
                    }
                }
                body = merged;
            }
            ops::profile_set(&path, &app.adapters, &a.name, body, true)?;
            app.out(&json!({"written": path.path, "profile": a.name}), || {
                println!("profile `{}` updated in {}", a.name, path.display())
            });
        }
        ProfileCommand::Remove { name, project } => {
            let path = app.target(project);
            ops::profile_remove(&path, &app.adapters, &name)?;
            app.out(&json!({"written": path.path, "removed": name}), || {
                println!("profile `{name}` removed from {}", path.display())
            });
        }
        ProfileCommand::List => return cmd_agents(app, AgentsArgs { probe: false }),
        ProfileCommand::Show { name } => {
            let loaded = app.load()?;
            let view = ops::agents(&loaded, &app.adapters, true)
                .into_iter()
                .find(|v| v.name == name)
                .ok_or_else(|| HufuError::usage(format!("no profile or pool `{name}`; see `hufu agents`")))?;
            let profile = loaded.config.profile(&name).cloned();
            let doc = json!({"agent": view, "profile": profile});
            app.out(&doc, || print!("{}", config::to_yaml(&doc).unwrap_or_default()));
        }
    }
    Ok(exit::OK)
}

fn cmd_pool(app: &App, c: PoolCommand) -> Result<i32> {
    match c {
        PoolCommand::Add { name, member, strategy, description, project } => {
            let path = app.target(project);
            ops::pool_set(&path, &app.adapters, &name, &member, strategy.as_deref(), description.as_deref())?;
            app.out(&json!({"written": path.path, "pool": name}), || {
                println!(
                    "pool `{name}` ({}) written to {}. Use it as an agent: hufu run -a {name} \"...\"",
                    member.join(", "),
                    path.display()
                )
            });
        }
        PoolCommand::Remove { name, project } => {
            let path = app.target(project);
            ops::pool_remove(&path, &app.adapters, &name)?;
            app.out(&json!({"written": path.path, "removed": name}), || {
                println!("pool `{name}` removed from {}", path.display())
            });
        }
        PoolCommand::List => {
            let loaded = app.load()?;
            app.out(&json!({"pools": loaded.config.pools}), || {
                if loaded.config.pools.is_empty() {
                    println!("no pools. Create one with: hufu pool add <name> --member <profile> --member <profile>");
                }
                for (name, pool) in &loaded.config.pools {
                    println!("{name}: {} ({:?})", pool.members.join(", "), pool.strategy);
                }
            });
        }
    }
    Ok(exit::OK)
}
