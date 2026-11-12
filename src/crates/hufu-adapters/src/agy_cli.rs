//! Antigravity CLI (`agy -p --output-format stream-json`).
//!
//! `agy` has no working-directory flag and no ACP entry point. Actions that need approval are
//! soft-denied in headless mode: the run carries on and exits 0, so denial detection is mandatory.

use hufu_contract::{
    Adapter, AdapterError, CancelToken, Capabilities, Effort, EventBody, EventSink, LaunchPlan, ProbeResult, Profile,
    RawStream, RunOutcome, RunRequest, StructuredOutput, Tier, Usage,
};
use serde_json::Value;

use crate::common::{self, Collected};

const DEFAULT: &[&str] = &["agy"];
const INSTALL: &str = "install Antigravity CLI: `curl -fsSL https://antigravity.google/cli/install.sh | bash`, then run `agy` once interactively to sign in";
const FORBIDDEN: &[&str] = &["--dangerously-skip-permissions", "--sandbox", "--mode", "--add-dir"];

pub struct AgyCli;

#[derive(Default)]
struct State {
    c: Collected,
    text: String,
    saw_result: bool,
}

impl Adapter for AgyCli {
    fn kind(&self) -> &'static str {
        "agy-cli"
    }

    fn default_command(&self) -> Vec<String> {
        DEFAULT.iter().map(|s| s.to_string()).collect()
    }

    fn probe(&self, profile: &Profile) -> ProbeResult {
        let (mut probe, _) = common::probe_cli(profile, DEFAULT, INSTALL);
        if probe.installed {
            probe.detail = Some(
                "login state cannot be read without running the agent; headless runs use the cached interactive login"
                    .into(),
            );
            probe.hint = Some("run `agy` once interactively if jobs fail with auth_required".into());
        }
        probe
    }

    fn capabilities(&self, _profile: &Profile) -> Capabilities {
        Capabilities {
            permission_tiers: Tier::ALL.to_vec(),
            efforts: vec![Effort::Low, Effort::Medium, Effort::High],
            native_effort: true,
            checked_at_runtime: false,
            models: Vec::new(),
            resume: true,
            structured_output: StructuredOutput::Native,
            reports_usage: true,
            reports_denials: true,
            accepts_mcp_servers: false,
            provides: Vec::new(),
            notes: vec![
                "read-only uses `--mode plan --sandbox`; Hufu also checks afterwards that the workspace did not change".into(),
                "workspace-write is agy's default headless boundary: workspace edits are applied, commands need an allow rule in agy's settings or are soft-denied".into(),
                "model ids come from `agy models`; an unknown id fails the run".into(),
            ],
        }
    }

    fn plan(&self, profile: &Profile, req: &RunRequest) -> Result<LaunchPlan, AdapterError> {
        common::check_extra_args(profile, FORBIDDEN)?;
        let mut plan = common::plan_start(profile, DEFAULT);
        let a = &mut plan.args;
        a.extend(["-p".into(), req.prompt.clone(), "--output-format".into(), "stream-json".into()]);
        match req.permission {
            Tier::ReadOnly => a.extend(["--mode".into(), "plan".into(), "--sandbox".into()]),
            Tier::WorkspaceWrite => a.push("--sandbox".into()),
            Tier::FullAccess => a.push("--dangerously-skip-permissions".into()),
        }
        if let Some(model) = &req.model {
            a.extend(["--model".into(), model.clone()]);
        }
        match &req.effort {
            Effort::Default => {}
            Effort::Low | Effort::Medium | Effort::High => a.extend(["--effort".into(), req.effort.to_string()]),
            Effort::Native(v) => a.extend(["--effort".into(), v.clone()]),
            other => {
                return Err(AdapterError(format!(
                    "Antigravity has no `{other}` effort level; accepted: default, low, medium, high, native:<value>"
                )));
            }
        }
        if req.output_schema.is_some() {
            a.extend(["--json-schema".into(), req.scratch_dir.join("output-schema.json").display().to_string()]);
        }
        if let Some(session) = &req.resume_session {
            a.extend(["--conversation".into(), session.clone()]);
        }
        a.extend(profile.args.iter().cloned());
        if !req.mcp_servers.is_empty() {
            return Err(AdapterError("Antigravity takes MCP servers from its own configuration files only; Hufu cannot attach one for a single run".into()));
        }
        Ok(plan)
    }

    fn run(&self, profile: &Profile, req: &RunRequest, sink: &dyn EventSink, cancel: &CancelToken) -> RunOutcome {
        let plan = match self.plan(profile, req) {
            Ok(p) => p,
            Err(e) => return RunOutcome::failed(hufu_contract::reason::UNSUPPORTED, e.0),
        };
        if let Some(schema) = &req.output_schema {
            let path = req.scratch_dir.join("output-schema.json");
            if let Err(e) = std::fs::write(&path, serde_json::to_vec_pretty(schema).unwrap_or_default()) {
                return RunOutcome::failed(
                    hufu_contract::reason::INTERNAL,
                    format!("cannot write {}: {e}", path.display()),
                );
            }
        }
        let mut s = State::default();
        let skip_denials = req.permission == Tier::FullAccess;
        let ended = common::run_plan(profile, DEFAULT, req, &plan, sink, cancel, &mut |stream, line| match stream {
            RawStream::Stdout => parse_line(&mut s, sink, line),
            RawStream::Stderr => {
                // The soft-denial notice names the tool and how to allow it; its exact wording is
                // not documented, so any line that reads like a denial is recorded.
                if !skip_denials && common::looks_like_denial(line) {
                    s.c.deny(sink, "agy", "other", line.trim());
                }
            }
        });
        if !s.saw_result && ended.exit_code == Some(0) && s.c.final_message.is_none() && s.c.error.is_none() {
            sink.emit(EventBody::warning(
                "agy printed no result. Some versions print nothing without a terminal (antigravity-cli issue #76)",
            ));
        }
        s.c.outcome(sink, ended, req, true, "run `agy` once interactively to sign in")
    }
}

fn parse_line(s: &mut State, sink: &dyn EventSink, line: &str) {
    let Ok(v) = serde_json::from_str::<Value>(line) else {
        return;
    };
    match v.get("event").and_then(Value::as_str).unwrap_or("") {
        "init" => {
            if let Some(id) = v.get("conversation_id").and_then(Value::as_str).filter(|x| !x.is_empty()) {
                s.c.session = Some(id.to_string());
            }
        }
        "step_update" => {
            let Some(step) = v.get("step_update") else {
                return;
            };
            let done = step.get("state").and_then(Value::as_str) == Some("DONE");
            let index = step.get("step_index").map(|i| i.to_string()).unwrap_or_default();
            match step.get("step_type").and_then(Value::as_str).unwrap_or("") {
                "agent_response" => {
                    if let Some(delta) = step.get("text_delta").and_then(Value::as_str) {
                        s.text.push_str(delta);
                    }
                    if done {
                        let text = std::mem::take(&mut s.text);
                        s.c.message(sink, &text);
                    }
                }
                "tool" => {
                    let info = step.get("tool_info").cloned().unwrap_or(Value::Null);
                    let name =
                        step.get("tool_name").or_else(|| info.get("name")).and_then(Value::as_str).unwrap_or("tool");
                    let kind = common::tool_kind(name);
                    let params = info.get("parameters").cloned().unwrap_or(Value::Null);
                    let title = match params
                        .get("CommandLine")
                        .or_else(|| params.get("AbsolutePath"))
                        .or_else(|| params.get("TargetFile"))
                    {
                        Some(x) => format!("{name}: {}", common::title_of(x)),
                        None => name.to_string(),
                    };
                    let error = info.get("error").filter(|e| !e.is_null());
                    let status = match (done, error.is_some()) {
                        (false, _) => "in_progress",
                        (true, true) => "failed",
                        (true, false) => "completed",
                    };
                    s.c.tool(sink, &format!("step{index}"), kind, &title, status);
                    if let Some(err) = error {
                        let text = format!(
                            "{} {}",
                            err.get("type").and_then(Value::as_str).unwrap_or(""),
                            err.get("message").and_then(Value::as_str).unwrap_or("")
                        );
                        if common::looks_like_denial(&text) {
                            s.c.deny(sink, &title, kind, text.trim());
                        }
                    } else if done && kind == "edit" {
                        if let Some(path) =
                            params.get("TargetFile").or_else(|| params.get("AbsolutePath")).and_then(Value::as_str)
                        {
                            s.c.touch(path);
                        }
                    }
                }
                _ => {}
            }
        }
        "result" => {
            let Some(r) = v.get("result") else { return };
            s.saw_result = true;
            if let Some(id) = r.get("conversation_id").and_then(Value::as_str).filter(|x| !x.is_empty()) {
                s.c.session = Some(id.to_string());
            }
            if let Some(u) = r.get("usage") {
                s.c.usage = Usage {
                    input_tokens: common::num(u, "input_tokens"),
                    output_tokens: common::num(u, "output_tokens"),
                    cached_input_tokens: common::num(u, "cache_read_tokens"),
                    reasoning_tokens: common::num(u, "thinking_tokens"),
                    total_tokens: common::num(u, "total_tokens"),
                    cost_usd: None,
                };
            }
            if let Some(out) = r.get("structured_output").filter(|x| !x.is_null()) {
                s.c.structured = Some(out.clone());
            }
            let status = r.get("status").and_then(Value::as_str).unwrap_or("");
            let response = r.get("response").and_then(Value::as_str).unwrap_or("");
            if status == "SUCCESS" {
                s.c.set_final(sink, response);
            } else {
                let err = r.get("error").and_then(Value::as_str).filter(|e| !e.is_empty());
                let msg = match err {
                    Some(e) => format!("agy ended with status {status}: {e}"),
                    None => format!("agy ended with status {status}"),
                };
                s.c.fail(sink, &msg);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::testing::*;

    #[test]
    fn tiers_map_to_explicit_flags() {
        let p = profile("agy-cli");
        let ro = AgyCli.plan(&p, &request(Tier::ReadOnly)).unwrap();
        assert!(ro.args.windows(2).any(|w| w == ["--mode", "plan"]) && ro.args.contains(&"--sandbox".to_string()));
        let ww = AgyCli.plan(&p, &request(Tier::WorkspaceWrite)).unwrap();
        assert!(!ww.args.contains(&"--dangerously-skip-permissions".to_string()));
        let full = AgyCli.plan(&p, &request(Tier::FullAccess)).unwrap();
        assert!(full.args.contains(&"--dangerously-skip-permissions".to_string()));
        assert!(ro.stdin.is_none(), "stdin must stay closed for agy");
        let mut req = request(Tier::ReadOnly);
        req.effort = Effort::Max;
        assert!(AgyCli.plan(&p, &req).unwrap_err().0.contains("accepted"));
    }

    #[test]
    fn parses_stream_and_detects_soft_denials() {
        let sink = Memory::default();
        let mut s = State::default();
        for line in [
            r#"{"event":"init","conversation_id":"c-1","init":{"cwd":"/w","tools":["run_command"],"permission_mode":"request-review"}}"#,
            r#"{"event":"step_update","step_update":{"conversation_id":"c-1","step_index":0,"state":"DONE","step_type":"user_input"}}"#,
            r#"{"event":"step_update","step_update":{"step_index":1,"state":"DONE","step_type":"tool","tool_name":"run_command","tool_info":{"name":"run_command","parameters":{"CommandLine":"npm test"},"error":{"type":"permission_denied","message":"command was denied"}}}}"#,
            r#"{"event":"step_update","step_update":{"step_index":2,"state":"ACTIVE","step_type":"agent_response","text_delta":"Hel"}}"#,
            r#"{"event":"step_update","step_update":{"step_index":2,"state":"ACTIVE","step_type":"agent_response","text_delta":"lo"}}"#,
            r#"{"event":"step_update","step_update":{"step_index":2,"state":"DONE","step_type":"agent_response"}}"#,
            r#"{"event":"result","result":{"conversation_id":"c-1","status":"SUCCESS","response":"Hello\n","num_turns":1,"usage":{"input_tokens":10,"output_tokens":2,"thinking_tokens":1,"cache_read_tokens":0,"total_tokens":13}}}"#,
        ] {
            parse_line(&mut s, &sink, line);
        }
        assert_eq!(s.c.session.as_deref(), Some("c-1"));
        assert_eq!(s.c.denied.len(), 1);
        assert_eq!(s.c.usage.total_tokens, Some(13));
        let out = s.c.outcome(&sink, ended(0), &request(Tier::WorkspaceWrite), true, "");
        assert_eq!(out.final_message.as_deref(), Some("Hello"));
        assert_eq!(out.denied_actions.len(), 1);
    }

    #[test]
    fn an_error_status_fails_the_run() {
        let sink = Memory::default();
        let mut s = State::default();
        parse_line(
            &mut s,
            &sink,
            r#"{"event":"result","result":{"conversation_id":"","status":"ERROR","response":"","error":"invalid model selection"}}"#,
        );
        let out = s.c.outcome(&sink, ended(1), &request(Tier::ReadOnly), true, "");
        assert_eq!(out.reason.as_deref(), Some(hufu_contract::reason::AGENT_ERROR));
        assert!(out.detail.unwrap().contains("invalid model"));
    }
}
