//! Kimi Code CLI (`kimi -p --output-format stream-json`).
//!
//! Print mode runs every tool call without asking, so `full-access` is its only tier. For lower
//! tiers use the ACP surface (`kimi acp`) through the generic ACP adapter.

use hufu_contract::{
    Adapter, AdapterError, CancelToken, Capabilities, Effort, EventSink, LaunchPlan, Profile, ProbeResult, RawStream,
    RunOutcome, RunRequest, StructuredOutput, Tier, json::instruct_json,
};
use serde_json::Value;

use crate::common::{self, Collected};

const DEFAULT: &[&str] = &["kimi"];
const INSTALL: &str = "install Kimi Code CLI: `curl -fsSL https://code.kimi.com/kimi-code/install.sh | bash` (Windows: `irm https://code.kimi.com/kimi-code/install.ps1 | iex`), then run `kimi login`";
const FORBIDDEN: &[&str] = &["--yolo", "-y", "--auto", "--plan", "--yes", "--auto-approve", "--add-dir"];

pub struct KimiCli;

impl Adapter for KimiCli {
    fn kind(&self) -> &'static str {
        "kimi-cli"
    }

    fn default_command(&self) -> Vec<String> {
        DEFAULT.iter().map(|s| s.to_string()).collect()
    }

    fn home_var(&self) -> Option<&'static str> {
        Some("KIMI_CODE_HOME")
    }

    fn probe(&self, profile: &Profile) -> ProbeResult {
        let (mut probe, _) = common::probe_cli(profile, DEFAULT, INSTALL);
        if probe.installed {
            probe.detail = Some("login state cannot be read without running the agent".into());
            probe.hint = Some("run `kimi login` if jobs fail with auth_required".into());
        }
        probe
    }

    fn capabilities(&self, _profile: &Profile) -> Capabilities {
        Capabilities {
            permission_tiers: vec![Tier::FullAccess],
            efforts: vec![Effort::Low, Effort::Medium, Effort::High, Effort::Max],
            native_effort: true,
            checked_at_runtime: false,
            models: Vec::new(),
            resume: false,
            structured_output: StructuredOutput::Instructed,
            reports_usage: false,
            reports_denials: false,
            accepts_mcp_servers: false,
            provides: Vec::new(),
            notes: vec![
                "print mode approves every tool call; run it with worktree isolation or in a container, or use an ACP profile (`command: [kimi, acp]`) for lower tiers".into(),
                "effort is passed as KIMI_MODEL_THINKING_EFFORT".into(),
            ],
        }
    }

    fn plan(&self, profile: &Profile, req: &RunRequest) -> Result<LaunchPlan, AdapterError> {
        common::check_extra_args(profile, FORBIDDEN)?;
        if req.permission != Tier::FullAccess {
            return Err(AdapterError(format!(
                "Kimi print mode cannot run at `{}`: it approves every tool call. Accepted: full-access",
                req.permission
            )));
        }
        if !req.mcp_servers.is_empty() {
            return Err(AdapterError("Kimi print mode reads MCP servers from ~/.kimi-code/mcp.json only; use an ACP profile to attach one for a single run".into()));
        }
        if req.resume_session.is_some() {
            return Err(AdapterError("Kimi print mode does not report a session id, so a follow-up cannot be sent; use an ACP profile".into()));
        }
        let mut plan = common::plan_start(profile, DEFAULT);
        let prompt = match &req.output_schema {
            Some(schema) => instruct_json(&req.prompt, schema),
            None => req.prompt.clone(),
        };
        plan.args.extend(["-p".into(), prompt, "--output-format".into(), "stream-json".into()]);
        if let Some(model) = &req.model {
            plan.args.extend(["-m".into(), model.clone()]);
        }
        let level = match &req.effort {
            Effort::Default => None,
            Effort::Low | Effort::Medium | Effort::High | Effort::Max => Some(req.effort.to_string()),
            Effort::Native(v) => Some(v.clone()),
            Effort::Off => {
                return Err(AdapterError("Kimi Code has no `off` effort level; accepted: default, low, medium, high, max, native:<value>".into()));
            }
        };
        if let Some(level) = level {
            plan.env.insert("KIMI_MODEL_THINKING_EFFORT".into(), level);
        }
        plan.args.extend(profile.args.iter().cloned());
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
        // The retired kimi-cli used exit code 75 for transient failures; keep honouring it.
        c.retryable = ended.exit_code == Some(75);
        c.outcome(sink, ended, req, false, "run `kimi login`")
    }
}

pub(crate) fn parse_line(c: &mut Collected, sink: &dyn EventSink, line: &str) {
    let Ok(v) = serde_json::from_str::<Value>(line) else { return };
    match v.get("role").and_then(Value::as_str).unwrap_or("") {
        "assistant" => {
            if let Some(content) = v.get("content") {
                c.message(sink, &common::text_of(content));
            }
            for call in v.get("tool_calls").and_then(Value::as_array).into_iter().flatten() {
                let id = call.get("id").and_then(Value::as_str).unwrap_or("call");
                let name = call.pointer("/function/name").and_then(Value::as_str).unwrap_or("tool");
                let args = call.pointer("/function/arguments").map(common::title_of).unwrap_or_default();
                c.tool(sink, id, common::tool_kind(name), &format!("{name} {args}"), "in_progress");
            }
        }
        "tool" => {
            let id = v.get("tool_call_id").and_then(Value::as_str).unwrap_or("call");
            c.tool(sink, id, "other", "", "completed");
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::testing::*;

    #[test]
    fn only_full_access_is_accepted() {
        let p = profile("kimi-cli");
        assert_eq!(KimiCli.capabilities(&p).permission_tiers, [Tier::FullAccess]);
        assert!(KimiCli.plan(&p, &request(Tier::ReadOnly)).unwrap_err().0.contains("full-access"));
        let mut req = request(Tier::FullAccess);
        req.effort = Effort::High;
        let plan = KimiCli.plan(&p, &req).unwrap();
        assert_eq!(plan.env.get("KIMI_MODEL_THINKING_EFFORT").map(String::as_str), Some("high"));
        assert!(plan.args.starts_with(&["-p".to_string(), "do the thing".to_string()]));
    }

    #[test]
    fn parses_role_lines() {
        let sink = Memory::default();
        let mut c = Collected::default();
        for line in [
            r#"{"role":"assistant","content":"","tool_calls":[{"type":"function","id":"tc_1","function":{"name":"Shell","arguments":"{\"command\":\"ls\"}"}}]}"#,
            r#"{"role":"tool","tool_call_id":"tc_1","content":"file1.py\nfile2.py"}"#,
            r#"{"role":"assistant","content":"Two files."}"#,
        ] {
            parse_line(&mut c, &sink, line);
        }
        let out = c.outcome(&sink, ended(0), &request(Tier::FullAccess), false, "");
        assert_eq!(out.final_message.as_deref(), Some("Two files."));
        assert_eq!(sink.types(), ["tool.call", "tool.call", "agent.message"]);
    }
}
