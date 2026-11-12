//! Claude Code (`claude -p --output-format stream-json`), so that Claude Code can be an executor
//! under another advisor.

use hufu_contract::{
    Adapter, AdapterError, CancelToken, Capabilities, Effort, EventBody, EventSink, LaunchPlan, ProbeResult, Profile,
    RawStream, RunOutcome, RunRequest, StructuredOutput, Tier, Usage,
};
use serde_json::{Value, json};

use crate::common::{self, Collected};

const DEFAULT: &[&str] = &["claude"];
const INSTALL: &str = "install Claude Code: `curl -fsSL https://claude.ai/install.sh | bash` or `npm install -g @anthropic-ai/claude-code`, then run `claude` once to sign in";
const FORBIDDEN: &[&str] = &[
    "--permission-mode",
    "--dangerously-skip-permissions",
    "--allow-dangerously-skip-permissions",
    "--allowedTools",
    "--allowed-tools",
    "--add-dir",
    "--permission-prompt-tool",
    "--settings",
];

pub struct ClaudeCli;

impl Adapter for ClaudeCli {
    fn kind(&self) -> &'static str {
        "claude-cli"
    }

    fn default_command(&self) -> Vec<String> {
        DEFAULT.iter().map(|s| s.to_string()).collect()
    }

    fn home_var(&self) -> Option<&'static str> {
        Some("CLAUDE_CONFIG_DIR")
    }

    fn probe(&self, profile: &Profile) -> ProbeResult {
        let (mut probe, program) = common::probe_cli(profile, DEFAULT, INSTALL);
        if let Some(program) = program {
            let mut cmd = hufu_contract::proc::command_for(&program.path);
            cmd.args(&program.prefix).args(["auth", "status"]);
            if let Some(home) = &profile.home {
                cmd.env("CLAUDE_CONFIG_DIR", common::expand_tilde(home));
            }
            if let Ok(out) = hufu_contract::proc::run_capture(cmd, std::time::Duration::from_secs(15)) {
                match out.code {
                    Some(0) => probe.authenticated = Some(true),
                    Some(1) => {
                        probe.authenticated = Some(false);
                        probe.hint = Some("run `claude` and sign in with /login, or pass ANTHROPIC_API_KEY through the profile's `env`".into());
                    }
                    _ => {}
                }
            }
        }
        probe
    }

    fn capabilities(&self, _profile: &Profile) -> Capabilities {
        Capabilities {
            permission_tiers: Tier::ALL.to_vec(),
            efforts: vec![Effort::Low, Effort::Medium, Effort::High, Effort::Max],
            native_effort: true,
            checked_at_runtime: false,
            models: Vec::new(),
            resume: true,
            structured_output: StructuredOutput::Native,
            reports_usage: true,
            reports_denials: true,
            accepts_mcp_servers: true,
            provides: Vec::new(),
            notes: vec![
                "read-only is plan mode; workspace-write is acceptEdits (file edits allowed, other tools need Claude Code's own allow rules)".into(),
            ],
        }
    }

    fn plan(&self, profile: &Profile, req: &RunRequest) -> Result<LaunchPlan, AdapterError> {
        common::check_extra_args(profile, FORBIDDEN)?;
        let mut plan = common::plan_start(profile, DEFAULT);
        let a = &mut plan.args;
        a.extend(["-p".into(), "--output-format".into(), "stream-json".into(), "--verbose".into()]);
        match req.permission {
            Tier::ReadOnly => a.extend(["--permission-mode".into(), "plan".into()]),
            Tier::WorkspaceWrite => a.extend(["--permission-mode".into(), "acceptEdits".into()]),
            Tier::FullAccess => a.push("--dangerously-skip-permissions".into()),
        }
        if let Some(model) = &req.model {
            a.extend(["--model".into(), model.clone()]);
        }
        match &req.effort {
            Effort::Default => {}
            Effort::Low | Effort::Medium | Effort::High | Effort::Max => {
                a.extend(["--effort".into(), req.effort.to_string()])
            }
            Effort::Native(v) => a.extend(["--effort".into(), v.clone()]),
            Effort::Off => {
                return Err(AdapterError(
                    "Claude Code has no `off` effort level; accepted: default, low, medium, high, max, native:<value>"
                        .into(),
                ));
            }
        }
        if let Some(session) = &req.resume_session {
            a.extend(["--resume".into(), session.clone()]);
        }
        if let Some(schema) = &req.output_schema {
            a.extend(["--json-schema".into(), schema.to_string()]);
        }
        if !req.mcp_servers.is_empty() {
            let mut servers = serde_json::Map::new();
            for s in &req.mcp_servers {
                servers.insert(s.name.clone(), json!({"command": s.command, "args": s.args, "env": s.env}));
            }
            a.extend(["--mcp-config".into(), json!({"mcpServers": servers}).to_string()]);
        }
        a.extend(profile.args.iter().cloned());
        plan.stdin = Some(req.prompt.clone());
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
        c.outcome(
            sink,
            ended,
            req,
            true,
            "run `claude` and sign in with /login, or pass ANTHROPIC_API_KEY through the profile's `env`",
        )
    }
}

pub(crate) fn parse_line(c: &mut Collected, sink: &dyn EventSink, line: &str) {
    let Ok(v) = serde_json::from_str::<Value>(line) else {
        return;
    };
    if let Some(id) = v.get("session_id").and_then(Value::as_str).filter(|x| !x.is_empty()) {
        c.session = Some(id.to_string());
    }
    // Messages produced inside a subagent are progress, not the job's own answer.
    let nested = v.get("parent_tool_use_id").map(|p| !p.is_null()).unwrap_or(false);
    match v.get("type").and_then(Value::as_str).unwrap_or("") {
        "assistant" => {
            for block in v.pointer("/message/content").and_then(Value::as_array).into_iter().flatten() {
                match block.get("type").and_then(Value::as_str).unwrap_or("") {
                    "text" if !nested => c.message(sink, block.get("text").and_then(Value::as_str).unwrap_or("")),
                    "thinking" => {
                        c.flush(sink);
                        sink.emit(EventBody::AgentThought {
                            text: block.get("thinking").and_then(Value::as_str).unwrap_or("").to_string(),
                        });
                    }
                    "tool_use" => {
                        let name = block.get("name").and_then(Value::as_str).unwrap_or("tool");
                        let input = block.get("input").cloned().unwrap_or(Value::Null);
                        let target = input
                            .get("file_path")
                            .or_else(|| input.get("command"))
                            .or_else(|| input.get("pattern"))
                            .or_else(|| input.get("url"))
                            .map(common::title_of);
                        let title = match target {
                            Some(t) if !t.is_empty() => format!("{name}: {t}"),
                            _ => name.to_string(),
                        };
                        let kind = common::tool_kind(name);
                        if kind == "edit" {
                            if let Some(p) = input.get("file_path").and_then(Value::as_str) {
                                c.touch(p);
                            }
                        }
                        c.tool(
                            sink,
                            block.get("id").and_then(Value::as_str).unwrap_or("tool"),
                            kind,
                            &title,
                            "in_progress",
                        );
                    }
                    _ => {}
                }
            }
        }
        "user" => {
            for block in v.pointer("/message/content").and_then(Value::as_array).into_iter().flatten() {
                if block.get("type").and_then(Value::as_str) == Some("tool_result") {
                    let failed = block.get("is_error").and_then(Value::as_bool).unwrap_or(false);
                    let id = block.get("tool_use_id").and_then(Value::as_str).unwrap_or("tool");
                    c.tool(sink, id, "other", "", if failed { "failed" } else { "completed" });
                }
            }
        }
        "system" => {
            if v.get("subtype").and_then(Value::as_str) == Some("api_retry") {
                let err = v.get("error").and_then(Value::as_str).unwrap_or("");
                if err == "authentication_failed" {
                    c.auth_required = true;
                }
            }
        }
        "result" => {
            if let Some(u) = v.get("usage") {
                c.usage = Usage {
                    input_tokens: common::num(u, "input_tokens"),
                    output_tokens: common::num(u, "output_tokens"),
                    cached_input_tokens: common::num(u, "cache_read_input_tokens"),
                    reasoning_tokens: None,
                    total_tokens: None,
                    cost_usd: None,
                };
            }
            if let Some(cost) = v.get("total_cost_usd").and_then(Value::as_f64) {
                c.usage.cost_usd = Some(cost);
            }
            if let Some(out) = v.get("structured_output").filter(|x| !x.is_null()) {
                c.structured = Some(out.clone());
            }
            for denial in v.get("permission_denials").and_then(Value::as_array).into_iter().flatten() {
                let name = denial.get("tool_name").and_then(Value::as_str).unwrap_or("tool");
                let input = denial.get("tool_input").map(common::title_of).unwrap_or_default();
                c.deny(sink, name, common::tool_kind(name), &input);
            }
            let text = v.get("result").and_then(Value::as_str).unwrap_or("");
            let subtype = v.get("subtype").and_then(Value::as_str).unwrap_or("success");
            if v.get("is_error").and_then(Value::as_bool).unwrap_or(false) || subtype != "success" {
                let msg =
                    if text.is_empty() { format!("Claude Code ended with `{subtype}`") } else { text.to_string() };
                c.fail(sink, &msg);
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
    use hufu_contract::McpServerSpec;

    #[test]
    fn tiers_effort_and_mcp_config() {
        let p = profile("claude-cli");
        let ro = ClaudeCli.plan(&p, &request(Tier::ReadOnly)).unwrap();
        assert!(ro.args.windows(2).any(|w| w == ["--permission-mode", "plan"]));
        let ww = ClaudeCli.plan(&p, &request(Tier::WorkspaceWrite)).unwrap();
        assert!(ww.args.windows(2).any(|w| w == ["--permission-mode", "acceptEdits"]));
        let mut req = request(Tier::FullAccess);
        req.effort = Effort::Max;
        req.mcp_servers = vec![McpServerSpec {
            name: "hufu".into(),
            command: "hufu".into(),
            args: vec!["mcp".into()],
            env: Default::default(),
        }];
        let full = ClaudeCli.plan(&p, &req).unwrap();
        assert!(full.args.contains(&"--dangerously-skip-permissions".to_string()));
        assert!(full.args.windows(2).any(|w| w == ["--effort", "max"]));
        let cfg = &full.args[full.args.iter().position(|a| a == "--mcp-config").unwrap() + 1];
        assert_eq!(serde_json::from_str::<Value>(cfg).unwrap()["mcpServers"]["hufu"]["command"], "hufu");
        assert_eq!(full.stdin.as_deref(), Some("do the thing"));
    }

    #[test]
    fn parses_stream_json_and_permission_denials() {
        let sink = Memory::default();
        let mut c = Collected::default();
        for line in [
            r#"{"type":"system","subtype":"init","session_id":"sess-1","model":"m","tools":["Read"]}"#,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Let me look."},{"type":"tool_use","id":"tu_1","name":"Edit","input":{"file_path":"src/a.rs"}}]},"parent_tool_use_id":null,"session_id":"sess-1"}"#,
            r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"tu_1","is_error":false}]},"session_id":"sess-1"}"#,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Edited."}]},"session_id":"sess-1"}"#,
            r#"{"type":"result","subtype":"success","is_error":false,"result":"Edited.","session_id":"sess-1","total_cost_usd":0.02,"usage":{"input_tokens":5,"output_tokens":7,"cache_read_input_tokens":3},"permission_denials":[{"tool_name":"Bash","tool_use_id":"tu_2","tool_input":{"command":"rm x"}}]}"#,
        ] {
            parse_line(&mut c, &sink, line);
        }
        assert_eq!(c.touched, ["src/a.rs"]);
        assert_eq!(c.denied.len(), 1);
        let out = c.outcome(&sink, ended(0), &request(Tier::WorkspaceWrite), true, "");
        assert_eq!(out.final_message.as_deref(), Some("Edited."));
        assert_eq!(out.usage.unwrap().cost_usd, Some(0.02));
        assert_eq!(out.session.unwrap().id, "sess-1");
    }

    #[test]
    fn not_logged_in_is_auth_required() {
        let sink = Memory::default();
        let mut c = Collected::default();
        parse_line(
            &mut c,
            &sink,
            r#"{"type":"result","subtype":"success","is_error":true,"result":"Not logged in · Please run /login","session_id":"s"}"#,
        );
        let out = c.outcome(&sink, ended(1), &request(Tier::ReadOnly), true, "sign in");
        assert_eq!(out.reason.as_deref(), Some(hufu_contract::reason::AUTH_REQUIRED));
    }
}
