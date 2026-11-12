//! Cursor CLI (`agent -p --output-format stream-json`).

use hufu_contract::{
    Adapter, AdapterError, CancelToken, Capabilities, Effort, EventSink, LaunchPlan, ProbeResult, Profile, RawStream,
    RunOutcome, RunRequest, StructuredOutput, Tier, json::instruct_json,
};
use serde_json::Value;

use crate::common::{self, Collected};

const DEFAULT: &[&str] = &["agent"];
const INSTALL: &str = "install Cursor CLI: `curl https://cursor.com/install -fsS | bash` (Windows: `irm 'https://cursor.com/install?win32=true' | iex`), then run `agent login`";
const FORBIDDEN: &[&str] = &["--force", "-f", "--yolo", "--mode", "--plan", "--sandbox", "--approve-mcps"];

pub struct CursorCli;

impl Adapter for CursorCli {
    fn kind(&self) -> &'static str {
        "cursor-cli"
    }

    fn default_command(&self) -> Vec<String> {
        DEFAULT.iter().map(|s| s.to_string()).collect()
    }

    fn home_var(&self) -> Option<&'static str> {
        Some("CURSOR_CONFIG_DIR")
    }

    fn probe(&self, profile: &Profile) -> ProbeResult {
        let (mut probe, program) = common::probe_cli(profile, DEFAULT, INSTALL);
        if let Some(program) = program {
            if common::env_set("CURSOR_API_KEY") {
                probe.authenticated = Some(true);
                probe.detail = Some("CURSOR_API_KEY is set".into());
            } else if let Some(line) = common::first_line(&program, &["status"], std::time::Duration::from_secs(15)) {
                let lower = line.to_ascii_lowercase();
                if lower.contains("not logged in") || lower.contains("not authenticated") {
                    probe.authenticated = Some(false);
                    probe.hint = Some("run `agent login`".into());
                } else if lower.contains("logged in") || lower.contains("authenticated") {
                    probe.authenticated = Some(true);
                }
                probe.detail = Some(line);
            }
        }
        probe
    }

    fn capabilities(&self, _profile: &Profile) -> Capabilities {
        Capabilities {
            permission_tiers: Tier::ALL.to_vec(),
            efforts: Vec::new(),
            native_effort: false,
            checked_at_runtime: false,
            models: Vec::new(),
            resume: true,
            structured_output: StructuredOutput::Instructed,
            reports_usage: false,
            reports_denials: true,
            accepts_mcp_servers: false,
            provides: Vec::new(),
            notes: vec![
                "Cursor has no effort flag; pick reasoning strength through the model id".into(),
                "workspace-write is Cursor's own boundary: workspace edits are applied, commands outside its allow-list are rejected".into(),
            ],
        }
    }

    fn plan(&self, profile: &Profile, req: &RunRequest) -> Result<LaunchPlan, AdapterError> {
        common::check_extra_args(profile, FORBIDDEN)?;
        if req.effort != Effort::Default {
            return Err(AdapterError(format!(
                "Cursor has no effort control, so `{}` cannot be honoured; use effort `default` and select a model instead",
                req.effort
            )));
        }
        if !req.mcp_servers.is_empty() {
            return Err(AdapterError("Cursor CLI reads MCP servers from .cursor/mcp.json only; use the ACP profile to attach one for a single run".into()));
        }
        let mut plan = common::plan_start(profile, DEFAULT);
        let a = &mut plan.args;
        a.extend(["-p".into(), "--output-format".into(), "stream-json".into()]);
        a.extend(["--workspace".into(), req.cwd.display().to_string(), "--trust".into()]);
        match req.permission {
            Tier::ReadOnly => a.extend(["--mode".into(), "plan".into()]),
            Tier::WorkspaceWrite => {}
            Tier::FullAccess => a.push("--force".into()),
        }
        if let Some(model) = &req.model {
            a.extend(["--model".into(), model.clone()]);
        }
        if let Some(session) = &req.resume_session {
            a.extend(["--resume".into(), session.clone()]);
        }
        a.extend(profile.args.iter().cloned());
        a.push(match &req.output_schema {
            Some(schema) => instruct_json(&req.prompt, schema),
            None => req.prompt.clone(),
        });
        if req.permission == Tier::WorkspaceWrite {
            plan.notes.push(
                "print mode without --force: workspace edits are applied, other actions are rejected by Cursor".into(),
            );
        }
        Ok(plan)
    }

    fn run(&self, profile: &Profile, req: &RunRequest, sink: &dyn EventSink, cancel: &CancelToken) -> RunOutcome {
        let plan = match self.plan(profile, req) {
            Ok(p) => p,
            Err(e) => return RunOutcome::failed(hufu_contract::reason::UNSUPPORTED, e.0),
        };
        let mut c = Collected::default();
        let ended = common::run_plan(profile, DEFAULT, req, &plan, sink, cancel, &mut |stream, line| {
            if stream == RawStream::Stdout {
                parse_line(&mut c, sink, line);
            }
        });
        c.outcome(sink, ended, req, true, "run `agent login`, or pass CURSOR_API_KEY through the profile's `env`")
    }
}

pub(crate) fn parse_line(c: &mut Collected, sink: &dyn EventSink, line: &str) {
    let Ok(v) = serde_json::from_str::<Value>(line) else {
        return;
    };
    if let Some(id) = v.get("session_id").and_then(Value::as_str).filter(|x| !x.is_empty()) {
        c.session = Some(id.to_string());
    }
    match v.get("type").and_then(Value::as_str).unwrap_or("") {
        "assistant" => {
            // With partial output on, only the final flush (no timestamp_ms) is a complete message.
            if v.get("timestamp_ms").is_none()
                && let Some(content) = v.pointer("/message/content")
            {
                c.message(sink, &common::text_of(content));
            }
        }
        "tool_call" => {
            let id = v.get("call_id").and_then(Value::as_str).unwrap_or("call");
            let completed = v.get("subtype").and_then(Value::as_str) == Some("completed");
            let Some((name, body)) = v.get("tool_call").and_then(Value::as_object).and_then(|o| o.iter().next()) else {
                return;
            };
            let (tool, args) = if name == "function" {
                (
                    body.get("name").and_then(Value::as_str).unwrap_or("function").to_string(),
                    body.get("arguments").cloned().unwrap_or(Value::Null),
                )
            } else {
                (name.trim_end_matches("ToolCall").to_string(), body.get("args").cloned().unwrap_or(Value::Null))
            };
            let kind = common::tool_kind(&tool);
            let target = args.get("path").or_else(|| args.get("command")).map(common::title_of);
            let title = match &target {
                Some(t) if !t.is_empty() => format!("{tool}: {t}"),
                _ => tool.clone(),
            };
            if !completed {
                c.tool(sink, id, kind, &title, "in_progress");
                return;
            }
            let result = body.get("result").cloned().unwrap_or(Value::Null);
            let ok = result.get("success").is_some();
            c.tool(sink, id, kind, &title, if ok { "completed" } else { "failed" });
            if ok {
                if kind == "edit"
                    && let Some(p) = args.get("path").and_then(Value::as_str)
                {
                    c.touch(p);
                }
            } else {
                let text = result.to_string();
                let key = result.as_object().and_then(|o| o.keys().next().cloned()).unwrap_or_default();
                if key.to_ascii_lowercase().contains("reject") || common::looks_like_denial(&text) {
                    c.deny(sink, &title, kind, &hufu_contract::proc::tail(&text, 300));
                }
            }
        }
        "result" => {
            let text = v.get("result").and_then(Value::as_str).unwrap_or("");
            if v.get("is_error").and_then(Value::as_bool).unwrap_or(false) {
                c.fail(sink, if text.is_empty() { "Cursor reported an error" } else { text });
            } else {
                c.set_final(sink, text);
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
    fn tiers_and_unsupported_effort() {
        let p = profile("cursor-cli");
        let ro = CursorCli.plan(&p, &request(Tier::ReadOnly)).unwrap();
        assert!(ro.args.windows(2).any(|w| w == ["--mode", "plan"]));
        let ww = CursorCli.plan(&p, &request(Tier::WorkspaceWrite)).unwrap();
        assert!(!ww.args.contains(&"--force".to_string()) && !ww.args.contains(&"--mode".to_string()));
        let full = CursorCli.plan(&p, &request(Tier::FullAccess)).unwrap();
        assert!(full.args.contains(&"--force".to_string()));
        assert_eq!(full.args.last().map(String::as_str), Some("do the thing"));
        let mut req = request(Tier::ReadOnly);
        req.effort = Effort::High;
        assert!(CursorCli.plan(&p, &req).is_err());
    }

    #[test]
    fn parses_documented_events_and_rejections() {
        let sink = Memory::default();
        let mut c = Collected::default();
        for line in [
            r#"{"type":"system","subtype":"init","apiKeySource":"login","cwd":"/w","session_id":"s-9","model":"m","permissionMode":"default"}"#,
            r#"{"type":"tool_call","subtype":"started","call_id":"t1","tool_call":{"writeToolCall":{"args":{"path":"a.txt","fileText":"x"}}},"session_id":"s-9"}"#,
            r#"{"type":"tool_call","subtype":"completed","call_id":"t1","tool_call":{"writeToolCall":{"args":{"path":"a.txt","fileText":"x"},"result":{"success":{"path":"a.txt","linesCreated":1,"fileSize":1}}}},"session_id":"s-9"}"#,
            r#"{"type":"tool_call","subtype":"completed","call_id":"t2","tool_call":{"function":{"name":"shell","arguments":"{\"command\":\"rm -rf /\"}","result":{"rejected":{"reason":"not in allowlist"}}}},"session_id":"s-9"}"#,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Wrote a.txt"}]},"session_id":"s-9"}"#,
            r#"{"type":"result","subtype":"success","duration_ms":10,"is_error":false,"result":"Wrote a.txt","session_id":"s-9"}"#,
        ] {
            parse_line(&mut c, &sink, line);
        }
        assert_eq!(c.session.as_deref(), Some("s-9"));
        assert_eq!(c.touched, ["a.txt"]);
        assert_eq!(c.denied.len(), 1);
        let out = c.outcome(&sink, ended(0), &request(Tier::WorkspaceWrite), true, "");
        assert_eq!(out.final_message.as_deref(), Some("Wrote a.txt"));
    }
}
