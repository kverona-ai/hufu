//! Third-party adapters: any program that speaks Hufu's small adapter protocol on stdio.
//!
//! This is how an adapter is added without changing Hufu. The profile names the program:
//!
//! ```yaml
//! profiles:
//!   my-agent:
//!     adapter: external
//!     command: [hufu-adapter-my-agent]
//! ```
//!
//! The program is called with one of three subcommands and receives a JSON document on stdin:
//!
//! | Call | stdin | stdout |
//! |---|---|---|
//! | `<cmd> probe` | `{"v":1,"profile":{...}}` | one `ProbeResult` object |
//! | `<cmd> capabilities` | `{"v":1,"profile":{...}}` | one `Capabilities` object |
//! | `<cmd> run` | `{"v":1,"profile":{...},"request":{...}}` | JSON Lines: normalized events, then one `{"type":"outcome", ...}` line |
//!
//! The shapes are the serialized forms of the types in `hufu-contract`.

use std::time::Duration;

use hufu_contract::proc;
use hufu_contract::{
    Adapter, AdapterError, CancelToken, Capabilities, Effort, EventBody, EventSink, LaunchPlan, ProbeResult, Profile,
    RawStream, RunOutcome, RunRequest, StructuredOutput, reason,
};
use serde_json::{Value, json};

use crate::common;

pub struct External;

fn profile_json(profile: &Profile) -> Value {
    let mut v = serde_json::to_value(profile).unwrap_or(Value::Null);
    if let Some(o) = v.as_object_mut() {
        o.insert("name".into(), json!(profile.name));
    }
    v
}

fn ask<T: serde::de::DeserializeOwned>(profile: &Profile, verb: &str) -> Result<T, String> {
    let program = common::resolve(profile, &[], None)?;
    let mut cmd = proc::command_for(&program.path);
    cmd.args(&program.prefix).arg(verb);
    let input = json!({"v": 1, "profile": profile_json(profile)}).to_string();
    let out = run_with_input(cmd, &input, Duration::from_secs(20))?;
    serde_json::from_str(out.trim())
        .map_err(|e| format!("`{} {verb}` did not print the expected JSON: {e}", program.path.display()))
}

fn run_with_input(mut cmd: std::process::Command, input: &str, timeout: Duration) -> Result<String, String> {
    use std::io::Write;
    use std::process::Stdio;
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
    proc::new_group(&mut cmd);
    let mut child = cmd.spawn().map_err(|e| e.to_string())?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(input.as_bytes());
    }
    let pid = child.id();
    let started = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() > timeout => {
                proc::kill_tree(pid);
                let _ = child.kill();
                return Err("the adapter program did not answer in time".into());
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(15)),
            Err(e) => return Err(e.to_string()),
        }
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

impl Adapter for External {
    fn kind(&self) -> &'static str {
        "external"
    }

    fn default_command(&self) -> Vec<String> {
        Vec::new()
    }

    fn probe(&self, profile: &Profile) -> ProbeResult {
        if profile.command.is_empty() {
            return ProbeResult {
                detail: Some("an external profile needs `command`, the adapter program".into()),
                ..ProbeResult::default()
            };
        }
        match ask::<ProbeResult>(profile, "probe") {
            Ok(p) => p,
            Err(e) => ProbeResult { installed: false, detail: Some(e), ..ProbeResult::default() },
        }
    }

    fn capabilities(&self, profile: &Profile) -> Capabilities {
        ask::<Capabilities>(profile, "capabilities").unwrap_or_else(|e| Capabilities {
            permission_tiers: Vec::new(),
            efforts: Vec::new(),
            native_effort: false,
            checked_at_runtime: false,
            models: Vec::new(),
            resume: false,
            structured_output: StructuredOutput::Instructed,
            reports_usage: false,
            reports_denials: false,
            accepts_mcp_servers: false,
            provides: Vec::new(),
            notes: vec![format!("the adapter program could not be asked for its capabilities: {e}")],
        })
    }

    fn plan(&self, profile: &Profile, req: &RunRequest) -> Result<LaunchPlan, AdapterError> {
        if profile.command.is_empty() {
            return Err(AdapterError(format!(
                "profile `{}`: adapter `external` needs `command`, the adapter program",
                profile.name
            )));
        }
        // The program declares what it can do; a request beyond that never reaches it.
        let caps = self.capabilities(profile);
        if !caps.supports_tier(req.permission) {
            return Err(AdapterError(format!(
                "profile `{}`: the adapter program does not offer the `{}` tier",
                profile.name, req.permission
            )));
        }
        let effort_ok = match &req.effort {
            Effort::Default => true,
            Effort::Native(_) => caps.native_effort,
            level => caps.efforts.contains(level),
        };
        if !effort_ok {
            return Err(AdapterError(format!(
                "profile `{}`: the adapter program has no mapping for effort `{}`",
                profile.name, req.effort
            )));
        }
        if req.resume_session.is_some() && !caps.resume {
            return Err(AdapterError(format!(
                "profile `{}`: the adapter program cannot resume a session",
                profile.name
            )));
        }
        let mut plan = common::plan_start(profile, &[]);
        plan.args.push("run".into());
        plan.stdin = Some(
            json!({
                "v": 1,
                "profile": profile_json(profile),
                "request": {
                    "job_id": req.job_id, "prompt": req.prompt, "cwd": req.cwd, "permission": req.permission,
                    "effort": req.effort, "model": req.model, "output_schema": req.output_schema,
                    "resume_session": req.resume_session, "mcp_servers": req.mcp_servers, "scratch_dir": req.scratch_dir,
                }
            })
            .to_string(),
        );
        plan.notes.push("the request, including the permission tier, is sent as JSON on stdin".into());
        Ok(plan)
    }

    fn run(&self, profile: &Profile, req: &RunRequest, sink: &dyn EventSink, cancel: &CancelToken) -> RunOutcome {
        let plan = match self.plan(profile, req) {
            Ok(p) => p,
            Err(e) => return RunOutcome::failed(reason::UNSUPPORTED, e.0),
        };
        let mut outcome: Option<RunOutcome> = None;
        let ended = common::run_plan(profile, &[], req, &plan, sink, cancel, &mut |stream, line| {
            if stream != RawStream::Stdout {
                return;
            }
            let Ok(v) = serde_json::from_str::<Value>(line) else {
                return;
            };
            if v.get("type").and_then(Value::as_str) == Some("outcome") {
                outcome = serde_json::from_value(v).ok();
            } else if let Ok(event) = serde_json::from_value::<EventBody>(v) {
                sink.emit(event);
            }
        });
        if let Some(e) = ended.spawn_error {
            return RunOutcome::failed(reason::AGENT_UNAVAILABLE, format!("could not start the adapter program: {e}"));
        }
        if ended.cancelled {
            let mut out = RunOutcome::failed(reason::CANCELLED, "cancelled");
            out.status = hufu_contract::RunStatus::Cancelled;
            return out;
        }
        match outcome {
            Some(mut out) => {
                // Success without an answer is not success, whatever the program says.
                if out.status == hufu_contract::RunStatus::Succeeded
                    && out.final_message.as_deref().unwrap_or("").trim().is_empty()
                {
                    out = RunOutcome::failed(
                        reason::EMPTY_OUTPUT,
                        "the adapter reported success without a final message",
                    );
                }
                out.exit_code = ended.exit_code;
                out
            }
            None => RunOutcome::failed(
                reason::PROTOCOL_ERROR,
                format!("the adapter program ended without an outcome line. {}", proc::tail(&ended.stderr_tail, 800)),
            ),
        }
    }
}
