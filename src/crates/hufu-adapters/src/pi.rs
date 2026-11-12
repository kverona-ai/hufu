//! pi (`pi --mode json`): Hufu's route to open-API models such as OpenRouter, DeepSeek, Z.ai and
//! Kimi's coding plan.
//!
//! pi has no permission system. `full-access` is its natural tier; `read-only` is offered by
//! restricting pi to its `read` tool with `--tools`.

use hufu_contract::{
    Adapter, AdapterError, CancelToken, Capabilities, Effort, EventBody, EventSink, LaunchPlan, ProbeResult, Profile,
    RawStream, RunOutcome, RunRequest, StructuredOutput, Tier, Usage, json::instruct_json,
};
use serde_json::Value;

use crate::common::{self, Collected};

const DEFAULT: &[&str] = &["pi"];
const INSTALL: &str = "install pi: `curl -fsSL https://pi.dev/install.sh | sh` (Windows: `powershell -c \"irm https://pi.dev/install.ps1 | iex\"`), or `npm install -g --ignore-scripts @earendil-works/pi-coding-agent` with Node.js 22.19 or newer";
const FORBIDDEN: &[&str] =
    &["--tools", "-t", "--exclude-tools", "-xt", "--no-tools", "-nt", "--no-builtin-tools", "-nbt", "--approve", "-a"];

pub struct Pi;

/// The API-key variable pi reads for a provider, as listed in pi's provider documentation.
pub fn provider_key_var(provider: &str) -> Option<&'static str> {
    let p = provider.to_ascii_lowercase();
    let table: &[(&str, &str)] = &[
        ("openrouter", "OPENROUTER_API_KEY"),
        ("deepseek", "DEEPSEEK_API_KEY"),
        ("zai-coding-cn", "ZAI_CODING_CN_API_KEY"),
        ("zai", "ZAI_API_KEY"),
        ("kimi", "KIMI_API_KEY"),
        ("moonshot", "MOONSHOT_API_KEY"),
        ("anthropic", "ANTHROPIC_API_KEY"),
        ("openai", "OPENAI_API_KEY"),
        ("google", "GEMINI_API_KEY"),
        ("gemini", "GEMINI_API_KEY"),
        ("mistral", "MISTRAL_API_KEY"),
        ("groq", "GROQ_API_KEY"),
        ("cerebras", "CEREBRAS_API_KEY"),
        ("xai", "XAI_API_KEY"),
        ("fireworks", "FIREWORKS_API_KEY"),
        ("together", "TOGETHER_API_KEY"),
        ("minimax", "MINIMAX_API_KEY"),
        ("qwen", "QWEN_TOKEN_PLAN_API_KEY"),
        ("huggingface", "HF_TOKEN"),
        ("nvidia", "NVIDIA_API_KEY"),
    ];
    table.iter().find(|(name, _)| p.starts_with(name)).map(|(_, var)| *var)
}

#[derive(Default)]
struct State {
    c: Collected,
    message_usage: Option<Value>,
}

impl Adapter for Pi {
    fn kind(&self) -> &'static str {
        "pi"
    }

    fn default_command(&self) -> Vec<String> {
        DEFAULT.iter().map(|s| s.to_string()).collect()
    }

    fn home_var(&self) -> Option<&'static str> {
        Some("PI_CODING_AGENT_DIR")
    }

    fn probe(&self, profile: &Profile) -> ProbeResult {
        let (mut probe, _) = common::probe_cli(profile, DEFAULT, INSTALL);
        if !probe.installed {
            return probe;
        }
        let Some(provider) = &profile.provider else {
            probe.detail = Some("no provider set; pi will use its own default model".into());
            return probe;
        };
        let Some(var) = provider_key_var(provider) else {
            probe.detail = Some(format!(
                "provider `{provider}`: key variable not known to Hufu; make sure the profile passes it through `env`"
            ));
            return probe;
        };
        let source = profile.env_from.get(var).cloned();
        let passed = source.is_some() || profile.env.iter().any(|e| e.eq_ignore_ascii_case(var));
        let present = common::env_set(source.as_deref().unwrap_or(var));
        match (passed, present) {
            (true, true) => {
                probe.authenticated = Some(true);
                probe.detail = Some(format!("{var} is set and passed through"));
            }
            (false, true) => {
                probe.detail = Some(format!("{var} is set but the profile does not pass it through"));
                probe.hint = Some(format!("add `env: [{var}]` to the profile"));
            }
            (_, false) => {
                probe.detail = Some(format!(
                    "{} is not set in this shell; pi may still have a stored key",
                    source.as_deref().unwrap_or(var)
                ));
                probe.hint = Some(format!("export {} or run `pi` and use /login", source.as_deref().unwrap_or(var)));
            }
        }
        probe
    }

    fn capabilities(&self, _profile: &Profile) -> Capabilities {
        Capabilities {
            permission_tiers: vec![Tier::ReadOnly, Tier::FullAccess],
            efforts: Effort::SCALE.to_vec(),
            native_effort: true,
            checked_at_runtime: false,
            models: Vec::new(),
            resume: true,
            structured_output: StructuredOutput::Instructed,
            reports_usage: true,
            reports_denials: false,
            accepts_mcp_servers: false,
            provides: Vec::new(),
            notes: vec![
                "pi has no permission system: full-access is unrestricted, so run it with worktree isolation or in a container".into(),
                "read-only limits pi to its `read` tool (`--tools read --no-mcp`)".into(),
                "workspace-write is not available: pi cannot confine edits to the workspace".into(),
            ],
        }
    }

    fn plan(&self, profile: &Profile, req: &RunRequest) -> Result<LaunchPlan, AdapterError> {
        common::check_extra_args(profile, FORBIDDEN)?;
        if !req.mcp_servers.is_empty() {
            return Err(AdapterError("pi takes MCP servers from its own configuration only; use an ACP profile to attach one for a single run".into()));
        }
        let mut plan = common::plan_start(profile, DEFAULT);
        let a = &mut plan.args;
        a.extend(["--mode".into(), "json".into()]);
        match req.permission {
            Tier::ReadOnly => a.extend(["--tools".into(), "read".into(), "--no-mcp".into()]),
            Tier::FullAccess => {}
            Tier::WorkspaceWrite => {
                return Err(AdapterError(
                    "pi cannot run at `workspace-write`: it has no way to confine edits to the workspace. Accepted: read-only, full-access".into(),
                ));
            }
        }
        if let Some(model) = &req.model {
            if let Some(provider) = &profile.provider {
                a.extend(["--provider".into(), provider.clone()]);
            }
            a.extend(["--model".into(), model.clone()]);
        } else if profile.provider.is_some() {
            return Err(AdapterError(format!(
                "profile `{}`: pi needs `model` whenever `provider` is set",
                profile.name
            )));
        }
        match &req.effort {
            Effort::Default => {}
            Effort::Native(v) => a.extend(["--thinking".into(), v.clone()]),
            level => a.extend(["--thinking".into(), level.to_string()]),
        }
        if let Some(session) = &req.resume_session {
            a.extend(["--session".into(), session.clone()]);
        }
        a.extend(profile.args.iter().cloned());
        a.push(match &req.output_schema {
            Some(schema) => instruct_json(&req.prompt, schema),
            None => req.prompt.clone(),
        });
        Ok(plan)
    }

    fn run(&self, profile: &Profile, req: &RunRequest, sink: &dyn EventSink, cancel: &CancelToken) -> RunOutcome {
        let plan = match self.plan(profile, req) {
            Ok(p) => p,
            Err(e) => return RunOutcome::failed(hufu_contract::reason::UNSUPPORTED, e.0),
        };
        let mut s = State::default();
        let ended = common::run_plan(profile, DEFAULT, req, &plan, sink, cancel, &mut |stream, line| {
            if stream == RawStream::Stdout {
                parse_line(&mut s, sink, line);
            }
        });
        let hint = match profile.provider.as_deref().and_then(provider_key_var) {
            Some(var) => format!("set {var} and list it in the profile's `env`"),
            None => "set the provider's API key and list it in the profile's `env`".to_string(),
        };
        s.c.outcome(sink, ended, req, true, &hint)
    }
}

fn add_usage(c: &mut Collected, u: &Value) {
    let add = Usage {
        input_tokens: common::num(u, "input"),
        output_tokens: common::num(u, "output"),
        cached_input_tokens: common::num(u, "cacheRead"),
        reasoning_tokens: None,
        total_tokens: common::num(u, "totalTokens"),
        cost_usd: u.pointer("/cost/total").and_then(Value::as_f64),
    };
    if !add.is_empty() {
        c.usage.add(&add);
    }
}

fn parse_line(s: &mut State, sink: &dyn EventSink, line: &str) {
    let Ok(v) = serde_json::from_str::<Value>(line) else {
        return;
    };
    let text = |key: &str| v.get(key).and_then(Value::as_str).unwrap_or("");
    match text("type") {
        "session" => {
            if let Some(id) = v.get("id").and_then(Value::as_str) {
                s.c.session = Some(id.to_string());
            }
        }
        "message_update" => {
            if let Some(u) = v.get("usage") {
                s.message_usage = Some(u.clone());
            }
            if let Some(ev) = v.get("assistantMessageEvent") {
                if ev.get("type").and_then(Value::as_str) == Some("error") {
                    let msg = ev
                        .get("error")
                        .map(common::title_of)
                        .filter(|m| !m.is_empty())
                        .unwrap_or_else(|| "the model call failed".into());
                    s.c.fail(sink, &msg);
                }
            }
        }
        "message_end" => {
            let Some(msg) = v.get("message") else { return };
            if msg.get("role").and_then(Value::as_str) != Some("assistant") {
                return;
            }
            let usage = msg.get("usage").cloned().or_else(|| s.message_usage.take());
            s.message_usage = None;
            if let Some(u) = usage {
                add_usage(&mut s.c, &u);
            }
            if let Some(content) = msg.get("content") {
                let thinking: String = content
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|b| b.get("type").and_then(Value::as_str) == Some("thinking"))
                    .filter_map(|b| b.get("thinking").and_then(Value::as_str))
                    .collect();
                if !thinking.trim().is_empty() {
                    s.c.flush(sink);
                    sink.emit(EventBody::AgentThought { text: thinking });
                }
                let text: String = content
                    .as_array()
                    .map(|blocks| {
                        blocks
                            .iter()
                            .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
                            .filter_map(|b| b.get("text").and_then(Value::as_str))
                            .collect()
                    })
                    .unwrap_or_else(|| common::text_of(content));
                s.c.message(sink, &text);
            }
            if msg.get("stopReason").and_then(Value::as_str) == Some("error") {
                let detail = msg.get("errorMessage").and_then(Value::as_str).unwrap_or("the model call failed");
                s.c.fail(sink, detail);
            }
        }
        "tool_execution_start" => {
            let name = text("toolName");
            let args = v.get("args").cloned().unwrap_or(Value::Null);
            let target = args
                .get("command")
                .or_else(|| args.get("path"))
                .or_else(|| args.get("file_path"))
                .map(common::title_of);
            let title = match target {
                Some(t) if !t.is_empty() => format!("{name}: {t}"),
                _ => name.to_string(),
            };
            s.c.tool(sink, text("toolCallId"), common::tool_kind(name), &title, "in_progress");
        }
        "tool_execution_end" => {
            let name = text("toolName");
            let failed = v.get("isError").and_then(Value::as_bool).unwrap_or(false);
            s.c.tool(
                sink,
                text("toolCallId"),
                common::tool_kind(name),
                name,
                if failed { "failed" } else { "completed" },
            );
        }
        "auto_retry_end" => {
            if v.get("success").and_then(Value::as_bool) == Some(false) {
                let msg = v.get("finalError").and_then(Value::as_str).unwrap_or("retries exhausted");
                s.c.fail(sink, msg);
                s.c.retryable = true;
            }
        }
        "agent_settled" if v.get("aborted").and_then(Value::as_bool) == Some(true) && s.c.error.is_none() => {
            s.c.fail(sink, "pi aborted the run");
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::testing::*;

    #[test]
    fn tiers_provider_and_thinking() {
        let mut p = profile("pi");
        p.provider = Some("openrouter".into());
        let mut req = request(Tier::FullAccess);
        assert!(Pi.plan(&p, &req).unwrap_err().0.contains("model"));
        req.model = Some("vendor/model-slug".into());
        req.effort = Effort::High;
        let plan = Pi.plan(&p, &req).unwrap();
        assert!(plan.args.windows(2).any(|w| w == ["--provider", "openrouter"]));
        assert!(plan.args.windows(2).any(|w| w == ["--thinking", "high"]));
        assert!(!plan.args.contains(&"--tools".to_string()));
        req.permission = Tier::ReadOnly;
        let ro = Pi.plan(&p, &req).unwrap();
        assert!(ro.args.windows(2).any(|w| w == ["--tools", "read"]));
        req.permission = Tier::WorkspaceWrite;
        assert!(Pi.plan(&p, &req).is_err());
        assert_eq!(provider_key_var("deepseek"), Some("DEEPSEEK_API_KEY"));
        assert_eq!(provider_key_var("zai-coding-cn"), Some("ZAI_CODING_CN_API_KEY"));
    }

    #[test]
    fn parses_json_mode_events() {
        let sink = Memory::default();
        let mut s = State::default();
        for line in [
            r#"{"type":"session","version":3,"id":"7b1e","timestamp":"2026-11-12T14:00:00.000Z","cwd":"/w"}"#,
            r#"{"type":"agent_start"}"#,
            r#"{"type":"message_end","message":{"role":"user","content":"Review this repository","timestamp":1}}"#,
            r#"{"type":"tool_execution_start","toolCallId":"call_1","toolName":"bash","args":{"command":"ls -la"}}"#,
            r#"{"type":"tool_execution_end","toolCallId":"call_1","toolName":"bash","result":{"content":[{"type":"text","text":"x"}]},"isError":false,"durationMs":4}"#,
            r#"{"type":"message_update","assistantMessageEvent":{"type":"text_delta","contentIndex":0,"delta":"Loo"},"usage":{"input":100,"output":20,"cacheRead":0,"cacheWrite":0,"totalTokens":120,"cost":{"total":0.01}}}"#,
            r#"{"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"Looks fine."}],"stopReason":"stop"}}"#,
            r#"{"type":"agent_settled","aborted":false}"#,
        ] {
            parse_line(&mut s, &sink, line);
        }
        assert_eq!(s.c.session.as_deref(), Some("7b1e"));
        assert_eq!(s.c.usage.total_tokens, Some(120));
        assert_eq!(s.c.usage.cost_usd, Some(0.01));
        let out = s.c.outcome(&sink, ended(0), &request(Tier::FullAccess), true, "");
        assert_eq!(out.final_message.as_deref(), Some("Looks fine."));
    }
}
