//! Cursor CLI (`cursor-agent -p --output-format stream-json`).
//!
//! Cursor's installer puts the same program on PATH twice, as `agent` and as `cursor-agent`.
//! Other CLIs install an `agent` too (the Grok CLI does, and the last one installed wins), so Hufu
//! runs `cursor-agent` when it exists, as Cursor's staff recommend, and otherwise checks that
//! `agent` really is Cursor's before it calls it installed.
//!
//! Print mode without `--force` is Cursor's own `workspace-write` boundary, unless the user's
//! `cli-config.json` sets `approvalMode: unrestricted`, which makes print mode run everything as
//! `--force` does. Hufu reads that setting and then refuses `workspace-write`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use hufu_contract::{
    Adapter, AdapterError, CancelToken, Capabilities, Effort, EventSink, LaunchPlan, ModelList, ProbeResult, Profile,
    RawStream, RunOutcome, RunRequest, StructuredOutput, Tier, Usage, json::instruct_json,
};
use serde_json::Value;

use crate::common::{self, Collected};

const PRIMARY: &[&str] = &["cursor-agent"];
const SHARED_NAME: &[&str] = &["agent"];
const INSTALL: &str = "install Cursor CLI: `curl https://cursor.com/install -fsS | bash` (Windows: `irm 'https://cursor.com/install?win32=true' | iex`), then run `cursor-agent login`";
const LOGIN: &str = "run `cursor-agent login`, or pass CURSOR_API_KEY through the profile's `env`";

/// The command to run when the profile names none: `cursor-agent` if it is on PATH, else `agent`.
fn default_cmd() -> &'static [&'static str] {
    if hufu_contract::proc::which(PRIMARY[0]).is_some() { PRIMARY } else { SHARED_NAME }
}

/// Cursor's CLI configuration file for this profile, as the configuration reference places it.
fn config_file(profile: &Profile) -> Option<PathBuf> {
    if let Some(home) = &profile.home {
        return Some(common::expand_tilde(home).join("cli-config.json"));
    }
    if let Some(dir) = std::env::var_os("CURSOR_CONFIG_DIR").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(dir).join("cli-config.json"));
    }
    if cfg!(all(unix, not(target_os = "macos")))
        && let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty())
    {
        return Some(PathBuf::from(xdg).join("cursor").join("cli-config.json"));
    }
    common::home_dir().map(|h| h.join(".cursor").join("cli-config.json"))
}

/// Whether the user's configuration makes print mode approve everything (`approvalMode: unrestricted`).
fn approves_everything(profile: &Profile) -> bool {
    let Some(path) = config_file(profile) else { return false };
    let Ok(text) = std::fs::read_to_string(path) else { return false };
    serde_json::from_str::<Value>(&text)
        .ok()
        .and_then(|v| v.get("approvalMode").and_then(Value::as_str).map(|m| m.eq_ignore_ascii_case("unrestricted")))
        .unwrap_or(false)
}
const FORBIDDEN: &[&str] = &["--force", "-f", "--yolo", "--mode", "--plan", "--sandbox", "--approve-mcps"];

pub struct CursorCli;

impl Adapter for CursorCli {
    fn kind(&self) -> &'static str {
        "cursor-cli"
    }

    fn default_command(&self) -> Vec<String> {
        default_cmd().iter().map(|s| s.to_string()).collect()
    }

    fn home_var(&self) -> Option<&'static str> {
        Some("CURSOR_CONFIG_DIR")
    }

    fn probe(&self, profile: &Profile) -> ProbeResult {
        let (mut probe, program) = common::probe_cli(profile, default_cmd(), INSTALL);
        if let Some(other) = probe.version.as_deref().and_then(common::foreign_agent) {
            return ProbeResult {
                installed: false,
                detail: Some(format!(
                    "`{}` is the {other} CLI ({}), not Cursor's agent",
                    probe.path.as_deref().unwrap_or("agent"),
                    probe.version.as_deref().unwrap_or("")
                )),
                hint: Some(format!("{INSTALL}; it also installs `cursor-agent`, which Hufu prefers")),
                ..ProbeResult::default()
            };
        }
        if let Some(program) = program {
            if common::env_set("CURSOR_API_KEY") {
                probe.authenticated = Some(true);
                probe.detail = Some("CURSOR_API_KEY is set".into());
            } else if let Some(line) = common::first_line(&program, &["status"], std::time::Duration::from_secs(15)) {
                let lower = line.to_ascii_lowercase();
                if lower.contains("not logged in") || lower.contains("not authenticated") {
                    probe.authenticated = Some(false);
                    probe.hint = Some("run `cursor-agent login`".into());
                } else if lower.contains("logged in") || lower.contains("authenticated") {
                    probe.authenticated = Some(true);
                }
                probe.detail = Some(line);
            }
        }
        probe
    }

    fn capabilities(&self, profile: &Profile) -> Capabilities {
        let unrestricted = approves_everything(profile);
        let mut notes = vec![
            "Cursor has no effort flag; pick reasoning strength through the model id (`cursor-agent models` lists them)".into(),
            "workspace-write is Cursor's own boundary: workspace edits are applied, commands outside its allow-list are rejected".into(),
        ];
        if unrestricted {
            notes.push("workspace-write is not offered: Cursor's cli-config.json sets `approvalMode: unrestricted`, which makes print mode approve every action".into());
        }
        Capabilities {
            permission_tiers: if unrestricted { vec![Tier::ReadOnly, Tier::FullAccess] } else { Tier::ALL.to_vec() },
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
            notes,
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
        if req.permission == Tier::WorkspaceWrite && approves_everything(profile) {
            return Err(AdapterError(format!(
                "Cursor cannot hold `workspace-write` here: {} sets `approvalMode: unrestricted`, so print mode approves every action. \
                 Accepted: read-only, full-access",
                config_file(profile).map(|p| p.display().to_string()).unwrap_or_else(|| "cli-config.json".into())
            )));
        }
        let mut plan = common::plan_start(profile, default_cmd());
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

    /// `cursor-agent models` lists the models of the logged-in account.
    fn models(&self, profile: &Profile, env: &BTreeMap<String, String>, cwd: &Path) -> Result<ModelList, AdapterError> {
        if let Ok(program) = common::resolve(profile, default_cmd(), common::env_path(env))
            && let Some(version) = common::first_line(&program, &["--version"], Duration::from_secs(15))
            && let Some(other) = common::foreign_agent(&version)
        {
            return Err(AdapterError(format!(
                "`{}` is the {other} CLI ({version}), not Cursor's agent; {INSTALL}",
                program.path.display()
            )));
        }
        let (out, from) = common::run_listing(profile, default_cmd(), env, cwd, &["models"], Duration::from_secs(60))?;
        let models = common::parse_model_lines(&out);
        if models.is_empty() {
            return Err(AdapterError(format!("{from} listed no models; {LOGIN}")));
        }
        Ok(ModelList {
            source: "agent".into(),
            from,
            models,
            notes: vec!["Cursor has no effort setting: a model id that names a level or `thinking` is how reasoning strength is chosen".into()],
            ..ModelList::default()
        })
    }

    fn run(&self, profile: &Profile, req: &RunRequest, sink: &dyn EventSink, cancel: &CancelToken) -> RunOutcome {
        let plan = match self.plan(profile, req) {
            Ok(p) => p,
            Err(e) => return RunOutcome::failed(hufu_contract::reason::UNSUPPORTED, e.0),
        };
        let mut c = Collected::default();
        let ended = common::run_plan(profile, default_cmd(), req, &plan, sink, cancel, &mut |stream, line| {
            if stream == RawStream::Stdout {
                parse_line(&mut c, sink, line);
            }
        });
        c.outcome(sink, ended, req, true, LOGIN)
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
            // Since CLI 2026.10.28 the result can carry `usage`; read either spelling of its keys.
            if let Some(u) = v.get("usage") {
                let pick = |a: &str, b: &str| common::num(u, a).or_else(|| common::num(u, b));
                c.usage = Usage {
                    input_tokens: pick("inputTokens", "input_tokens"),
                    output_tokens: pick("outputTokens", "output_tokens"),
                    cached_input_tokens: pick("cacheReadTokens", "cache_read_tokens"),
                    reasoning_tokens: pick("reasoningTokens", "reasoning_tokens"),
                    total_tokens: pick("totalTokens", "total_tokens"),
                    cost_usd: None,
                };
            }
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
            r#"{"type":"result","subtype":"success","duration_ms":10,"is_error":false,"result":"Wrote a.txt","session_id":"s-9","usage":{"inputTokens":12,"outputTokens":3,"reasoningTokens":2}}"#,
        ] {
            parse_line(&mut c, &sink, line);
        }
        assert_eq!(c.session.as_deref(), Some("s-9"));
        assert_eq!(c.touched, ["a.txt"]);
        assert_eq!(c.denied.len(), 1);
        let out = c.outcome(&sink, ended(0), &request(Tier::WorkspaceWrite), true, "");
        assert_eq!(out.final_message.as_deref(), Some("Wrote a.txt"));
        let usage = out.usage.unwrap();
        assert_eq!((usage.input_tokens, usage.reasoning_tokens), (Some(12), Some(2)));
    }

    #[test]
    fn unrestricted_approval_mode_rules_out_workspace_write() {
        let dir = std::env::temp_dir().join(format!("hufu-cursor-cfg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut p = profile("cursor-cli");
        p.home = Some(dir.display().to_string());
        assert!(CursorCli.capabilities(&p).supports_tier(Tier::WorkspaceWrite));
        std::fs::write(dir.join("cli-config.json"), r#"{"version":1,"approvalMode":"unrestricted"}"#).unwrap();
        assert!(!CursorCli.capabilities(&p).supports_tier(Tier::WorkspaceWrite));
        assert!(CursorCli.plan(&p, &request(Tier::WorkspaceWrite)).unwrap_err().0.contains("unrestricted"));
        assert!(CursorCli.plan(&p, &request(Tier::ReadOnly)).is_ok());
        std::fs::write(dir.join("cli-config.json"), r#"{"version":1,"approvalMode":"allowlist"}"#).unwrap();
        assert!(CursorCli.plan(&p, &request(Tier::WorkspaceWrite)).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
