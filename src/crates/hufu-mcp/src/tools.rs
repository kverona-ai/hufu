//! The tools of Hufu's MCP server.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use hufu_contract::Tier;
use hufu_core::config::{self, Grant, Loaded, Route};
use hufu_core::error::{HufuError, Result};
use hufu_core::mission::{self, MissionSpec};
use hufu_core::spec::{ComputerUseOverride, TaskSpec};
use hufu_core::store::{self, JobState};
use hufu_core::submit::{self, Context, Options};
use hufu_core::{capability, doctor, ops, runner, setup};
use serde_json::{Value, json};

use crate::Server;

struct Tool {
    name: &'static str,
    title: &'static str,
    description: &'static str,
    schema: fn() -> Value,
}

fn obj(properties: Value, required: &[&str]) -> Value {
    json!({"type": "object", "properties": properties, "required": required, "additionalProperties": false})
}

const TIERS: &[&str] = &["read-only", "workspace-write", "full-access"];

fn submit_schema() -> Value {
    obj(
        json!({
            "prompt": {"type": "string", "description": "What the agent should do. Self-contained: the agent sees only this and the workspace."},
            "agent": {"type": "string", "description": "A profile or pool name from hufu_agents."},
            "title": {"type": "string"},
            "cwd": {"type": "string", "description": "The workspace. Default: the directory the MCP server runs in."},
            "permission": {"type": "string", "enum": TIERS, "description": "Default read-only. full-access must be asked for explicitly."},
            "effort": {"type": "string", "description": "default, off, low, medium, high, max, or native:<value>."},
            "model": {"type": "string"},
            "timeout": {"type": "string", "description": "For example 30m or 2h."},
            "inactivity_timeout": {"type": "string"},
            "isolation": {"type": "string", "enum": ["none", "worktree"], "description": "worktree gives the job its own copy of the repository on a branch hufu/<job>."},
            "acceptance": {"type": "array", "description": "Tally checks. Each item is one of {command}, {paths: {allow, deny}}, {schema}, {no_denied_actions: true}.", "items": {"type": "object"}},
            "output_schema": {"type": "object", "description": "A JSON Schema the final answer must match."},
            "strict_permissions": {"type": "boolean", "description": "Fail the job if the agent skipped any action for lack of permission."},
            "depends_on": {"type": "array", "items": {"type": "string"}, "description": "Job ids that must succeed first."},
            "labels": {"type": "object", "additionalProperties": {"type": "string"}},
            "computer_use_apps": {"type": "array", "items": {"type": "string"}, "description": "Grants desktop control of exactly these applications. Only when the user asked for it."},
            "computer_use_agent": {"type": "string", "description": "The profile that operates the desktop, if not the configured one."},
            "computer_use_route": {"type": "string", "enum": ["dedicated", "executor"]},
            "dry_run": {"type": "boolean", "description": "Validate and show the launch plan without starting anything."}
        }),
        &["prompt", "agent"],
    )
}


const TOOLS: &[Tool] = &[
    Tool {
        name: "hufu_agents",
        title: "List agents",
        description: "Lists the configured profiles and pools with their capabilities (permission tiers, effort levels, resume, computer use), their accounts, and which ones are the default advisor, executors and computer-use agent. Set probe to also check installation and login.",
        schema: || obj(json!({"probe": {"type": "boolean"}}), &[]),
    },
    Tool {
        name: "hufu_submit",
        title: "Submit a job",
        description: "Hands one task to another agent as a background job and returns its id at once. Validation happens before anything starts: an unsupported tier, effort or capability is an error, never a silent downgrade.",
        schema: submit_schema,
    },
    Tool {
        name: "hufu_status",
        title: "Job status",
        description: "With `job`: the state of one job, including its last activity. Without: the jobs that are queued or running, plus the most recent finished ones.",
        schema: || obj(json!({"job": {"type": "string"}, "limit": {"type": "integer", "minimum": 1, "maximum": 200}, "mission": {"type": "string"}}), &[]),
    },
    Tool {
        name: "hufu_wait",
        title: "Wait for jobs",
        description: "Blocks until the job (or all of `jobs`) reached a terminal state, or until the timeout. `wait_timed_out: true` means still running, not failed. Keep timeouts short and call again.",
        schema: || obj(json!({"job": {"type": "string"}, "jobs": {"type": "array", "items": {"type": "string"}}, "timeout_s": {"type": "integer", "minimum": 1, "maximum": 600, "description": "Default 50."}}), &[]),
    },
    Tool {
        name: "hufu_result",
        title: "Job result",
        description: "The result of a finished job: state, verdict, final message, structured output, changed files, denied actions, tally outcomes, usage. The final message is clipped unless `full` is set.",
        schema: || obj(json!({"job": {"type": "string"}, "full": {"type": "boolean"}}), &["job"]),
    },
    Tool {
        name: "hufu_logs",
        title: "Job events",
        description: "The normalized events of a job (messages, tool calls, permission decisions), oldest first. Pass `after` to read only what is new.",
        schema: || obj(json!({"job": {"type": "string"}, "after": {"type": "integer", "minimum": 0}, "limit": {"type": "integer", "minimum": 1, "maximum": 500}}), &["job"]),
    },
    Tool {
        name: "hufu_cancel",
        title: "Cancel a job",
        description: "Asks a job to stop. The agent's whole process tree is ended. `force` kills it without waiting for the grace period.",
        schema: || obj(json!({"job": {"type": "string"}, "force": {"type": "boolean"}}), &["job"]),
    },
    Tool {
        name: "hufu_resume",
        title: "Follow up on a job",
        description: "Sends a follow-up prompt to the session of a finished job, as a new job. Only for agents that can resume a session.",
        schema: || obj(json!({"job": {"type": "string"}, "prompt": {"type": "string"}}), &["job", "prompt"]),
    },
    Tool {
        name: "hufu_mission_start",
        title: "Start a mission",
        description: "Starts a managed loop: the advisor profile plans, executors do the work, the advisor reviews, within budgets. Advisor and executors default to `roles` in the configuration.",
        schema: || {
            obj(
                json!({
                    "goal": {"type": "string"},
                    "advisor": {"type": "string"},
                    "executors": {"type": "array", "items": {"type": "string"}},
                    "cwd": {"type": "string"},
                    "isolation": {"type": "string", "enum": ["none", "worktree"]},
                    "review": {"type": "string", "enum": ["always", "on_failure", "never"]},
                    "max_tier": {"type": "string", "enum": TIERS}
                }),
                &["goal"],
            )
        },
    },
    Tool {
        name: "hufu_mission_status",
        title: "Mission status",
        description: "With `mission`: its state, tasks and, once it ended, its report. Without: all missions.",
        schema: || obj(json!({"mission": {"type": "string"}}), &[]),
    },
    Tool {
        name: "hufu_mission_abort",
        title: "Abort a mission",
        description: "Stops a mission and cancels its running jobs. Finished work is kept.",
        schema: || obj(json!({"mission": {"type": "string"}}), &["mission"]),
    },
    Tool {
        name: "hufu_doctor",
        title: "Check the installation",
        description: "Reports which agents are installed and logged in, configuration problems, the default roles, the computer-use setup and what to do next. `self_test` also runs real jobs against built-in fake agents.",
        schema: || obj(json!({"self_test": {"type": "boolean"}}), &[]),
    },
    Tool {
        name: "hufu_config_get",
        title: "Read configuration",
        description: "The merged configuration, or one key of it such as `roles.advisor` or `limits.max_concurrent_jobs`.",
        schema: || obj(json!({"key": {"type": "string"}}), &[]),
    },
    Tool {
        name: "hufu_config_set",
        title: "Change configuration",
        description: "Sets one key, for example roles.advisor, roles.executors, defaults.timeout, capabilities.computer_use.agent. A null value removes the key. The change is validated; an invalid one is refused and nothing is written.",
        schema: || obj(json!({"key": {"type": "string"}, "value": {}, "project": {"type": "boolean", "description": "Write to .hufu/config.yaml of the current project instead of the user-wide file."}}), &["key", "value"]),
    },
    Tool {
        name: "hufu_profile_set",
        title: "Add or change a profile",
        description: "Creates or updates an agent profile. Fields: adapter (codex-cli, agy-cli, cursor-cli, claude-cli, kimi-cli, pi, acp, external), model, provider, effort, permission, account (a label), home (the agent's own login directory, which is how two accounts of one agent are kept apart), env (names of variables to pass through), env_from ({AGENT_VAR: SHELL_VAR}), vars, command, description, provides ({computer_use: {via: native|lcu}}). Never put a secret value here.",
        schema: || obj(json!({"name": {"type": "string"}, "profile": {"type": "object"}, "merge": {"type": "boolean", "description": "Keep fields that are not named. Default true."}, "project": {"type": "boolean"}}), &["name", "profile"]),
    },
    Tool {
        name: "hufu_profile_remove",
        title: "Remove a profile",
        description: "Removes a profile from the configuration file. Refused while a pool or a role still names it.",
        schema: || obj(json!({"name": {"type": "string"}, "project": {"type": "boolean"}}), &["name"]),
    },
    Tool {
        name: "hufu_pool_set",
        title: "Add or change a pool",
        description: "A pool is a set of profiles that can stand in for each other, typically several accounts of one agent. A task may name a pool as its agent; Hufu picks a member (failover, round-robin or least-busy) and moves on when one is not logged in or is rate limited.",
        schema: || {
            obj(
                json!({"name": {"type": "string"}, "members": {"type": "array", "items": {"type": "string"}, "minItems": 1}, "strategy": {"type": "string", "enum": ["failover", "round-robin", "least-busy"]}, "description": {"type": "string"}, "project": {"type": "boolean"}}),
                &["name", "members"],
            )
        },
    },
    Tool {
        name: "hufu_pool_remove",
        title: "Remove a pool",
        description: "Removes a pool from the configuration file.",
        schema: || obj(json!({"name": {"type": "string"}, "project": {"type": "boolean"}}), &["name"]),
    },
    Tool {
        name: "hufu_init",
        title: "Write a first configuration",
        description: "Detects the agents installed on this machine and writes a first config.yaml naming them, with a default advisor and executors. An existing file is kept unless `force` is set.",
        schema: || obj(json!({"force": {"type": "boolean"}}), &[]),
    },
    Tool {
        name: "hufu_setup",
        title: "Register Hufu with agents",
        description: "Registers this MCP server with other agents (claude, codex, cursor, kimi, agy) so that they can dispatch through Hufu too. Without `agents`, every agent found on this machine. `print` shows what would be written.",
        schema: || obj(json!({"agents": {"type": "array", "items": {"type": "string"}}, "remove": {"type": "boolean"}, "print": {"type": "boolean"}}), &[]),
    },
];

const CHILD_TOOLS: &[Tool] = &[Tool {
    name: "computer_use",
    title: "Operate desktop applications",
    description: "Hands one desktop step to the agent configured for computer use and returns what it did and saw. Give a self-contained instruction that names the application. Only the applications granted to this task may be used.",
    schema: || obj(json!({"instruction": {"type": "string"}}), &["instruction"]),
}];

fn table(child: bool) -> &'static [Tool] {
    if child { CHILD_TOOLS } else { TOOLS }
}

pub fn names(child: bool) -> Vec<&'static str> {
    table(child).iter().map(|t| t.name).collect()
}

pub fn list(child: bool) -> Vec<Value> {
    table(child).iter().map(|t| json!({"name": t.name, "title": t.title, "description": t.description, "inputSchema": (t.schema)()})).collect()
}

// ---------------------------------------------------------------------------------------------

fn text<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str).filter(|s| !s.trim().is_empty())
}

fn need<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    text(args, key).ok_or_else(|| HufuError::usage(format!("missing argument `{key}`")))
}

fn flag(args: &Value, key: &str) -> bool {
    args.get(key).and_then(Value::as_bool).unwrap_or(false)
}

/// A compact view of a job for an agent's context.
pub fn job_view(state: &JobState) -> Value {
    let mut v = json!({
        "job": state.id, "state": state.state.as_str(), "verdict": state.verdict.as_str(), "title": state.title,
        "agent": state.agent, "profile": state.profile, "permission": state.permission, "elapsed_s": state.elapsed_s(),
    });
    for (key, value) in [("reason", &state.reason), ("detail", &state.detail), ("last_activity", &state.last_activity), ("queue", &state.queue), ("mission", &state.mission)] {
        if let Some(value) = value {
            v[key] = json!(value);
        }
    }
    v
}

impl Server<'_> {
    fn load(&self, cwd: Option<&str>) -> Result<(Loaded, PathBuf)> {
        let dir = match cwd {
            Some(c) => {
                let p = hufu_contract::proc::expand_tilde(c);
                if p.is_absolute() { p } else { self.cwd.join(p) }
            }
            None => self.cwd.clone(),
        };
        Ok((config::load(&self.home, &dir)?, dir))
    }

    fn target(&self, args: &Value) -> PathBuf {
        ops::config_target(&self.home, flag(args, "project").then_some(self.cwd.as_path()))
    }
}

pub fn call(server: &Server, name: &str, args: &Value) -> Result<Value> {
    let home = &server.home;
    let adapters = server.adapters;
    match name {
        "computer_use" => computer_use(server, args),
        "hufu_agents" => {
            let (loaded, _) = server.load(None)?;
            let agents = ops::agents(&loaded, adapters, flag(args, "probe"));
            Ok(json!({"agents": agents, "roles": loaded.config.roles, "computer_use": loaded.config.capabilities.computer_use}))
        }
        "hufu_submit" => {
            let (loaded, _) = server.load(text(args, "cwd"))?;
            let mut body = args.clone();
            let map = body.as_object_mut().ok_or_else(|| HufuError::usage("arguments must be an object"))?;
            let dry_run = map.remove("dry_run").and_then(|v| v.as_bool()).unwrap_or(false);
            if let Some(schema) = map.remove("output_schema") {
                map.insert("output".into(), json!({"schema": schema}));
            }
            let apps = map.remove("computer_use_apps");
            let cu_agent = map.remove("computer_use_agent").and_then(|v| v.as_str().map(str::to_string));
            let cu_route = map.remove("computer_use_route").and_then(|v| v.as_str().map(str::to_string));
            let mut task = TaskSpec::from_value(body)?;
            if let Some(apps) = apps.and_then(|a| a.as_array().cloned()) {
                task.grants.push(Grant { capability: capability::COMPUTER_USE.into(), apps: apps.iter().filter_map(|a| a.as_str().map(str::to_string)).collect() });
            }
            if cu_agent.is_some() || cu_route.is_some() {
                let route = match cu_route.as_deref() {
                    Some("dedicated") => Some(Route::Dedicated),
                    Some("executor") => Some(Route::Executor),
                    Some(other) => return Err(HufuError::usage(format!("unknown computer_use_route `{other}`; accepted: dedicated, executor"))),
                    None => None,
                };
                task.computer_use = Some(ComputerUseOverride { route, agent: cu_agent });
            }
            let ctx = Context { home, config: &loaded.config, adapters };
            let opts = Options { base_dir: server.cwd.clone(), ..Options::default() };
            if dry_run {
                let spec = submit::prepare(&ctx, &task, &opts)?;
                let plan = submit::launch_plan(&ctx, &spec)?;
                return Ok(json!({"dry_run": true, "profile": spec.profile.name, "permission": spec.permission, "effort": spec.effort, "warnings": spec.warnings, "launch": plan}));
            }
            let spec = submit::prepare(&ctx, &task, &opts)?;
            let state = submit::submit_spec(home, &spec, false)?;
            let mut view = job_view(&state);
            view["warnings"] = json!(spec.warnings);
            view["next"] = json!("call hufu_wait or hufu_status with this job id, then hufu_result");
            Ok(view)
        }
        "hufu_status" => {
            if let Some(job) = text(args, "job") {
                let id = store::resolve_job(home, job)?;
                submit::kick(home, &id);
                return Ok(job_view(&store::load_state(home, &id)?));
            }
            let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(20) as usize;
            let mission = text(args, "mission");
            let jobs: Vec<JobState> = store::list_jobs(home).into_iter().filter(|j| mission.map(|m| j.mission.as_deref().map(|x| x.starts_with(m)).unwrap_or(false)).unwrap_or(true)).collect();
            let active: Vec<Value> = jobs.iter().filter(|j| !j.state.is_terminal()).map(job_view).collect();
            let recent: Vec<Value> = jobs.iter().rev().filter(|j| j.state.is_terminal()).take(limit).map(job_view).collect();
            Ok(json!({"active": active, "recent": recent}))
        }
        "hufu_wait" => {
            let mut ids: Vec<String> = Vec::new();
            if let Some(job) = text(args, "job") {
                ids.push(store::resolve_job(home, job)?);
            }
            for job in args.get("jobs").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str) {
                ids.push(store::resolve_job(home, job)?);
            }
            if ids.is_empty() {
                return Err(HufuError::usage("give `job` or `jobs`"));
            }
            let limit = Duration::from_secs(args.get("timeout_s").and_then(Value::as_u64).unwrap_or(50).clamp(1, 600));
            let started = Instant::now();
            let mut views = Vec::new();
            let mut pending = false;
            for id in &ids {
                let state = runner::wait(home, id, Some(limit.saturating_sub(started.elapsed())))?;
                pending |= !state.state.is_terminal();
                views.push(job_view(&state));
            }
            Ok(json!({"wait_timed_out": pending, "jobs": views}))
        }
        "hufu_result" => {
            let id = store::resolve_job(home, need(args, "job")?)?;
            let Some(mut result) = runner::result_of(home, &id)? else {
                let state = store::load_state(home, &id)?;
                return Ok(json!({"job": id, "state": state.state.as_str(), "finished": false, "last_activity": state.last_activity, "note": "the job is still going; call hufu_wait"}));
            };
            let (loaded, _) = server.load(None)?;
            let max = loaded.config.defaults.result_chars;
            let mut clipped = false;
            if !flag(args, "full") {
                if let Some(message) = &result.final_message {
                    if message.chars().count() > max {
                        result.final_message = Some(message.chars().take(max).collect());
                        clipped = true;
                    }
                }
            }
            let mut value = serde_json::to_value(&result).map_err(|e| HufuError::internal(e.to_string()))?;
            if clipped {
                value["final_message_clipped"] = json!(true);
                value["note"] = json!("the final message was clipped; pass full: true for all of it");
            }
            Ok(value)
        }
        "hufu_logs" => {
            let id = store::resolve_job(home, need(args, "job")?)?;
            let after = args.get("after").and_then(Value::as_u64).unwrap_or(0);
            let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(50) as usize;
            let events = store::read_events(home, &id, after);
            let total = events.len();
            let shown: Vec<Value> = events.iter().take(limit).map(|e| json!({"seq": e.seq, "ts": e.ts, "summary": e.body.summary()})).collect();
            Ok(json!({"job": id, "events": shown, "more": total > limit, "last_seq": events.iter().take(limit).last().map(|e| e.seq).unwrap_or(after)}))
        }
        "hufu_cancel" => {
            let id = store::resolve_job(home, need(args, "job")?)?;
            let state = if flag(args, "force") { runner::force_cancel(home, &id)? } else { runner::cancel(home, &id)? };
            Ok(job_view(&state))
        }
        "hufu_resume" => {
            let id = store::resolve_job(home, need(args, "job")?)?;
            let spec = store::load_spec(home, &id)?;
            let loaded = config::load(home, &spec.cwd)?;
            let ctx = Context { home, config: &loaded.config, adapters };
            let state = submit::resume(&ctx, &id, need(args, "prompt")?, false)?;
            Ok(job_view(&state))
        }
        "hufu_mission_start" => {
            let (loaded, dir) = server.load(text(args, "cwd"))?;
            let mut body = args.clone();
            if let Some(map) = body.as_object_mut() {
                map.remove("cwd");
            }
            let mut spec: MissionSpec = serde_json::from_value(body).map_err(|e| HufuError::usage(format!("invalid mission: {e}")))?;
            spec.cwd = Some(dir.display().to_string());
            let ctx = Context { home, config: &loaded.config, adapters };
            let created = mission::create(&ctx, &spec, &server.cwd)?;
            mission::launch(home, &created.id)?;
            Ok(json!({"mission": created.id, "state": created.state.as_str(), "advisor": created.advisor, "executors": created.executors, "next": "call hufu_mission_status with this id"}))
        }
        "hufu_mission_status" => match text(args, "mission") {
            None => Ok(json!({"missions": mission::list(home).iter().map(|m| json!({"mission": m.id, "title": m.title, "state": m.state.as_str(), "tasks": m.tasks.len(), "created_at": m.created_at})).collect::<Vec<_>>()})),
            Some(partial) => {
                let id = mission::resolve(home, partial)?;
                let m = mission::load(home, &id)?;
                let mut v = json!({
                    "mission": m.id, "title": m.title, "state": m.state.as_str(), "reason": m.reason, "detail": m.detail, "advisor": m.advisor,
                    "summary": m.summary, "tasks": m.tasks, "tokens": m.usage.tokens(), "branch": m.branch, "workdir": m.workdir,
                });
                if m.state.is_terminal() {
                    v["report"] = json!(mission::report(home, &m));
                }
                Ok(v)
            }
        },
        "hufu_mission_abort" => {
            let id = mission::resolve(home, need(args, "mission")?)?;
            let m = mission::abort(home, &id)?;
            Ok(json!({"mission": m.id, "state": m.state.as_str(), "note": "running jobs are being cancelled"}))
        }
        "hufu_doctor" => {
            let (loaded, _) = server.load(None)?;
            let mut v = serde_json::to_value(doctor::doctor(home, &loaded, adapters, server.version)).map_err(|e| HufuError::internal(e.to_string()))?;
            if flag(args, "self_test") {
                v["self_test"] = json!(doctor::self_test(adapters));
            }
            Ok(v)
        }
        "hufu_config_get" => {
            let (loaded, _) = server.load(None)?;
            let value = ops::config_get(&loaded.config, text(args, "key"))?;
            Ok(json!({"key": text(args, "key"), "value": value, "files": [loaded.global_path.display().to_string(), loaded.project_path.map(|p| p.display().to_string())]}))
        }
        "hufu_config_set" => {
            let key = need(args, "key")?;
            let path = server.target(args);
            match args.get("value") {
                None | Some(Value::Null) => ops::config_unset(&path, adapters, key)?,
                Some(value) => ops::config_set(&path, adapters, key, value.clone())?,
            }
            Ok(json!({"written": path.display().to_string(), "key": key}))
        }
        "hufu_profile_set" => {
            let name = need(args, "name")?;
            let body = args.get("profile").cloned().filter(Value::is_object).ok_or_else(|| HufuError::usage("`profile` must be an object"))?;
            let path = server.target(args);
            let merge = args.get("merge").and_then(Value::as_bool).unwrap_or(true);
            ops::profile_set(&path, adapters, name, body, merge)?;
            Ok(json!({"written": path.display().to_string(), "profile": name, "next": "hufu_doctor shows whether the agent is installed and logged in"}))
        }
        "hufu_profile_remove" => {
            let path = server.target(args);
            ops::profile_remove(&path, adapters, need(args, "name")?)?;
            Ok(json!({"written": path.display().to_string()}))
        }
        "hufu_pool_set" => {
            let members: Vec<String> = args.get("members").and_then(Value::as_array).into_iter().flatten().filter_map(|m| m.as_str().map(str::to_string)).collect();
            let path = server.target(args);
            ops::pool_set(&path, adapters, need(args, "name")?, &members, text(args, "strategy"), text(args, "description"))?;
            Ok(json!({"written": path.display().to_string(), "pool": need(args, "name")?}))
        }
        "hufu_pool_remove" => {
            let path = server.target(args);
            ops::pool_remove(&path, adapters, need(args, "name")?)?;
            Ok(json!({"written": path.display().to_string()}))
        }
        "hufu_init" => Ok(serde_json::to_value(setup::init(home, adapters, flag(args, "force"))?).map_err(|e| HufuError::internal(e.to_string()))?),
        "hufu_setup" => {
            let mut hosts: Vec<String> = args.get("agents").and_then(Value::as_array).into_iter().flatten().filter_map(|m| m.as_str().map(str::to_string)).collect();
            if hosts.is_empty() {
                hosts = setup::detect_hosts();
            }
            let opts = setup::SetupOptions { remove: flag(args, "remove"), print: flag(args, "print"), project: None, exe: submit::hufu_exe()? };
            Ok(json!({"actions": setup::setup(&hosts, &opts)}))
        }
        other => Err(HufuError::usage(format!("unknown tool `{other}`"))),
    }
}

/// The hand-over: one desktop step, carried out by the configured agent as a child job.
fn computer_use(server: &Server, args: &Value) -> Result<Value> {
    let home = &server.home;
    let parent = server.child_of.as_deref().ok_or_else(|| HufuError::usage("this tool is only available inside a job"))?;
    let instruction = need(args, "instruction")?;
    let spec = store::load_spec(home, parent)?;
    let plan = spec.computer_use.clone().filter(|p| p.route == Route::Dedicated).ok_or_else(|| HufuError::usage("this job has no computer-use grant that is served by another agent"))?;
    if store::load_state(home, parent)?.state.is_terminal() {
        return Err(HufuError::usage("the job this tool belongs to has ended"));
    }
    let loaded = config::load(home, &spec.cwd)?;
    let operator = loaded.config.profile(&plan.agent).cloned().ok_or_else(|| HufuError::usage(format!("computer-use agent `{}` is no longer a profile", plan.agent)))?;
    let caps = server.adapters.get(&operator.adapter).map(|a| a.capabilities(&operator));
    let tier = operator.permission.or_else(|| caps.and_then(|c| c.permission_tiers.iter().copied().min())).unwrap_or(Tier::ReadOnly);
    let mut labels = std::collections::BTreeMap::new();
    labels.insert(runner::OPERATOR_LABEL.to_string(), parent.to_string());
    let task = TaskSpec {
        prompt: capability::operator_prompt(&plan.apps, instruction),
        agent: plan.agent.clone(),
        title: Some(format!("Computer use for {parent}")),
        cwd: Some(spec.cwd.display().to_string()),
        permission: Some(tier),
        labels,
        grants: vec![Grant { capability: capability::COMPUTER_USE.into(), apps: plan.apps.clone() }],
        computer_use: Some(ComputerUseOverride { route: Some(Route::Executor), agent: None }),
        ..TaskSpec::default()
    };
    let ctx = Context { home, config: &loaded.config, adapters: server.adapters };
    let opts = Options { base_dir: spec.cwd.clone(), parent: Some(parent.to_string()), mission: spec.mission.clone(), ..Options::default() };
    let child = submit::submit(&ctx, &task, &opts)?;
    // Wait for the step, but not beyond the life of the job it belongs to.
    loop {
        let state = runner::wait(home, &child.id, Some(Duration::from_secs(2)))?;
        if state.state.is_terminal() {
            break;
        }
        if store::load_state(home, parent).map(|p| p.state.is_terminal()).unwrap_or(true) || store::cancel_requested(home, parent) {
            let _ = runner::cancel(home, &child.id);
            return Err(HufuError::usage("the job was stopped while the desktop step was running"));
        }
    }
    let result = runner::result_of(home, &child.id)?;
    let state = store::load_state(home, &child.id)?;
    match result {
        Some(r) if r.state == store::State::Succeeded => Ok(Value::String(r.final_message.unwrap_or_default())),
        _ => Err(HufuError::usage(format!(
            "the desktop step did not succeed (job {}, {}): {}",
            child.id,
            state.state.as_str(),
            state.detail.or(state.reason).unwrap_or_default()
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tool_has_a_valid_object_schema() {
        for child in [false, true] {
            for tool in list(child) {
                let schema = &tool["inputSchema"];
                assert_eq!(schema["type"], "object", "{}", tool["name"]);
                for required in schema["required"].as_array().unwrap() {
                    assert!(schema["properties"].get(required.as_str().unwrap()).is_some(), "{}: required `{required}` is not a property", tool["name"]);
                }
                assert!(tool["description"].as_str().unwrap().len() > 20);
            }
        }
        assert_eq!(names(true), ["computer_use"]);
        assert!(names(false).contains(&"hufu_submit") && !names(false).contains(&"computer_use"));
    }
}
