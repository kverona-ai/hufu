//! Codex CLI (`codex exec --json`).
//!
//! Flags and event shapes follow the Codex documentation and `codex-rs/exec` sources, last checked
//! on 2026-11-20 against Codex CLI 0.171. See `doc/research/agent-landscape.md`.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use hufu_contract::{
    Adapter, AdapterError, CancelToken, Capabilities, Effort, EventBody, EventSink, LaunchPlan, ModelInfo, ModelList,
    ProbeResult, Profile, RawStream, RunOutcome, RunRequest, StructuredOutput, Tier,
};
use serde_json::Value;

use crate::common::{self, Collected};

const DEFAULT: &[&str] = &["codex"];
const INSTALL: &str = "install Codex CLI: `npm install -g @openai/codex` or `curl -fsSL https://chatgpt.com/codex/install.sh | sh`, then run `codex login`";
const FORBIDDEN: &[&str] = &[
    "--sandbox",
    "-s",
    "--dangerously-bypass-approvals-and-sandbox",
    "--yolo",
    "--full-auto",
    "--approve-for-me",
    "--not-so-yolo",
    "--ask-for-approval",
    "-a",
    "--add-dir",
];

pub struct CodexCli;

fn effort_value(effort: &Effort) -> Result<Option<String>, AdapterError> {
    Ok(match effort {
        Effort::Default => None,
        Effort::Low => Some("low".into()),
        Effort::Medium => Some("medium".into()),
        Effort::High => Some("high".into()),
        Effort::XHigh => Some("xhigh".into()),
        Effort::Max => Some("max".into()),
        Effort::Native(v) => Some(v.clone()),
        Effort::Off => {
            return Err(AdapterError(
                "Codex has no `off` effort level; accepted: default, low, medium, high, xhigh, max, native:<value>"
                    .into(),
            ));
        }
    })
}

fn toml_string(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

impl Adapter for CodexCli {
    fn kind(&self) -> &'static str {
        "codex-cli"
    }

    fn default_command(&self) -> Vec<String> {
        DEFAULT.iter().map(|s| s.to_string()).collect()
    }

    fn home_var(&self) -> Option<&'static str> {
        Some("CODEX_HOME")
    }

    fn probe(&self, profile: &Profile) -> ProbeResult {
        let (mut probe, _) = common::probe_cli(profile, DEFAULT, INSTALL);
        if probe.installed {
            let home = common::agent_home(profile, "CODEX_HOME", ".codex");
            if common::env_set("CODEX_API_KEY") && profile.home.is_none() {
                probe.authenticated = Some(true);
                probe.detail = Some("CODEX_API_KEY is set".into());
            } else if home.map(|h| h.join("auth.json").exists()).unwrap_or(false) {
                probe.authenticated = Some(true);
                probe.detail = Some("saved login found".into());
            } else {
                probe.detail =
                    Some("no saved login file found; if Codex keeps its login in the OS keyring this is fine".into());
                probe.hint = Some("run `codex login` if jobs fail with auth_required".into());
            }
        }
        probe
    }

    fn capabilities(&self, _profile: &Profile) -> Capabilities {
        Capabilities {
            permission_tiers: Tier::ALL.to_vec(),
            efforts: Effort::LOW_TO_MAX.to_vec(),
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
                "which effort levels a model accepts depends on the model (GPT-6 Luna stops at max); Codex's `ultra` hands the work to subagents and is reachable as native:ultra".into(),
                "models are chosen by id, for example gpt-6.2-sol (Codex's default) or gpt-6.2-astra; the `/model` picker lists them".into(),
                "since Codex 0.169, `codex exec` ignores project configuration and project MCP servers in a directory Codex does not trust; trust it once with `codex exec --trust` or add `--trust` to the profile's args".into(),
            ],
        }
    }

    fn plan(&self, profile: &Profile, req: &RunRequest) -> Result<LaunchPlan, AdapterError> {
        common::check_extra_args(profile, FORBIDDEN)?;
        let mut plan = common::plan_start(profile, DEFAULT);
        let a = &mut plan.args;
        a.push("exec".into());
        // Flags that are not global must come before the `resume` subcommand.
        a.extend(["--cd".into(), req.cwd.display().to_string()]);
        match req.permission {
            Tier::ReadOnly => a.extend(["--sandbox".into(), "read-only".into()]),
            Tier::WorkspaceWrite => a.extend(["--sandbox".into(), "workspace-write".into()]),
            Tier::FullAccess => a.push("--dangerously-bypass-approvals-and-sandbox".into()),
        }
        if let Some(level) = effort_value(&req.effort)? {
            a.extend(["-c".into(), format!("model_reasoning_effort={}", toml_string(&level))]);
        }
        for server in &req.mcp_servers {
            let args = server.args.iter().map(|x| toml_string(x)).collect::<Vec<_>>().join(", ");
            a.extend(["-c".into(), format!("mcp_servers.{}.command={}", server.name, toml_string(&server.command))]);
            a.extend(["-c".into(), format!("mcp_servers.{}.args=[{args}]", server.name)]);
            for (k, v) in &server.env {
                a.extend(["-c".into(), format!("mcp_servers.{}.env.{k}={}", server.name, toml_string(v))]);
            }
        }
        a.extend(["--json".into(), "--color".into(), "never".into()]);
        if !common::in_git_repo(&req.cwd) {
            a.push("--skip-git-repo-check".into());
            plan.notes.push("the workspace is not a git repository; passing --skip-git-repo-check".into());
        }
        if let Some(model) = &req.model {
            a.extend(["-m".into(), model.clone()]);
        }
        if req.output_schema.is_some() {
            a.extend(["--output-schema".into(), req.scratch_dir.join("output-schema.json").display().to_string()]);
        }
        a.extend(["-o".into(), req.scratch_dir.join("last-message.txt").display().to_string()]);
        a.extend(profile.args.iter().cloned());
        if let Some(session) = &req.resume_session {
            a.extend(["resume".into(), session.clone()]);
        }
        a.push("-".into());
        plan.stdin = Some(req.prompt.clone());
        Ok(plan)
    }

    /// `codex debug models` prints the model catalog Codex refreshed for this login, as JSON.
    fn models(&self, profile: &Profile, env: &BTreeMap<String, String>, cwd: &Path) -> Result<ModelList, AdapterError> {
        let (out, from) =
            common::run_listing(profile, DEFAULT, env, cwd, &["debug", "models"], Duration::from_secs(90))?;
        let mut list = parse_models(&out, from)?;
        let home = env
            .get("CODEX_HOME")
            .map(std::path::PathBuf::from)
            .or_else(|| common::agent_home(profile, "CODEX_HOME", ".codex"));
        let configured = home
            .and_then(|h| std::fs::read_to_string(h.join("config.toml")).ok())
            .and_then(|text| common::toml_top_string(&text, "model"));
        match configured {
            Some(model) => {
                for m in &mut list.models {
                    m.default = m.id == model;
                }
                list.notes.push(format!("config.toml sets `model = \"{model}\"`, which jobs without a model use"));
            }
            None => list.notes.push(
                "listed in Codex's own order; without `model` in config.toml Codex picks its built-in default".into(),
            ),
        }
        Ok(list)
    }

    fn run(&self, profile: &Profile, req: &RunRequest, sink: &dyn EventSink, cancel: &CancelToken) -> RunOutcome {
        let plan = match self.plan(profile, req) {
            Ok(p) => p,
            Err(e) => return RunOutcome::failed(hufu_contract::reason::UNSUPPORTED, e.0),
        };
        for note in &plan.notes {
            sink.emit(EventBody::warning(note.clone()));
        }
        if let Some(schema) = &req.output_schema {
            let path = req.scratch_dir.join("output-schema.json");
            if let Err(e) = std::fs::write(&path, serde_json::to_vec_pretty(schema).unwrap_or_default()) {
                return RunOutcome::failed(
                    hufu_contract::reason::INTERNAL,
                    format!("cannot write {}: {e}", path.display()),
                );
            }
        }
        // A file left by an earlier attempt must not be taken for this attempt's answer.
        let _ = std::fs::remove_file(req.scratch_dir.join("last-message.txt"));
        let mut c = Collected::default();
        let ended = common::run_plan(profile, DEFAULT, req, &plan, sink, cancel, &mut |stream, line| {
            if stream == RawStream::Stdout {
                parse_line(&mut c, sink, line);
            }
        });
        if let Ok(last) = std::fs::read_to_string(req.scratch_dir.join("last-message.txt")) {
            c.set_final(sink, &last);
        }
        c.outcome(sink, ended, req, true, "run `codex login`, or pass CODEX_API_KEY through the profile's `env`")
    }
}

/// Translates one line of `codex exec --json` output.
/// The catalog `codex debug models` prints: `{"models": [{"slug", "display_name", "description",
/// "default_reasoning_level", "supported_reasoning_levels": [{"effort"}], "visibility", "priority"}]}`.
/// Models with `visibility: hide` are left out, as Codex's own picker leaves them out.
pub(crate) fn parse_models(text: &str, from: String) -> Result<ModelList, AdapterError> {
    let doc: Value = serde_json::from_str(text.trim())
        .or_else(|_| hufu_contract::json::extract_json(text).ok_or(()))
        .map_err(|_| AdapterError(format!("{from} did not print the JSON model catalog")))?;
    let mut entries: Vec<&Value> = doc
        .get("models")
        .and_then(Value::as_array)
        .ok_or_else(|| AdapterError(format!("{from} printed no `models` list")))?
        .iter()
        .filter(|m| m.get("visibility").and_then(Value::as_str) != Some("hide"))
        .collect();
    entries.sort_by_key(|m| m.get("priority").and_then(Value::as_i64).unwrap_or(i64::MAX));
    let text_of = |m: &Value, key: &str| m.get(key).and_then(Value::as_str).map(str::to_string);
    let models = entries
        .into_iter()
        .filter_map(|m| {
            let id = text_of(m, "slug")?;
            let efforts = m
                .get("supported_reasoning_levels")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|l| l.get("effort").or(Some(l)).and_then(Value::as_str).map(str::to_string))
                .collect();
            Some(ModelInfo {
                name: text_of(m, "display_name").filter(|n| *n != id),
                efforts,
                default_effort: text_of(m, "default_reasoning_level"),
                default: false,
                description: text_of(m, "description"),
                id,
            })
        })
        .collect();
    Ok(ModelList { source: "agent".into(), from, models, ..ModelList::default() })
}

pub(crate) fn parse_line(c: &mut Collected, sink: &dyn EventSink, line: &str) {
    let Ok(v) = serde_json::from_str::<Value>(line) else {
        return;
    };
    let kind = v.get("type").and_then(Value::as_str).unwrap_or("");
    match kind {
        "thread.started" => {
            if let Some(id) = v.get("thread_id").and_then(Value::as_str) {
                c.session = Some(id.to_string());
            }
        }
        "turn.completed" => {
            if let Some(u) = v.get("usage") {
                let add = hufu_contract::Usage {
                    input_tokens: common::num(u, "input_tokens"),
                    output_tokens: common::num(u, "output_tokens"),
                    cached_input_tokens: common::num(u, "cached_input_tokens"),
                    reasoning_tokens: common::num(u, "reasoning_output_tokens"),
                    total_tokens: None,
                    cost_usd: None,
                };
                c.usage.add(&add);
            }
        }
        "turn.failed" => {
            let msg = v.pointer("/error/message").and_then(Value::as_str).unwrap_or("the turn failed");
            c.fail(sink, msg);
        }
        "error" => {
            let msg = v.get("message").and_then(Value::as_str).unwrap_or("unknown error");
            c.fail(sink, msg);
        }
        "item.started" | "item.updated" | "item.completed" => {
            if let Some(item) = v.get("item") {
                parse_item(c, sink, item, kind == "item.completed");
            }
        }
        _ => {}
    }
}

fn parse_item(c: &mut Collected, sink: &dyn EventSink, item: &Value, completed: bool) {
    let id = item.get("id").and_then(Value::as_str).unwrap_or("item");
    let text = |key: &str| item.get(key).and_then(Value::as_str).unwrap_or("");
    let status = match text("status") {
        "" if completed => "completed",
        "" => "in_progress",
        "declined" => "failed",
        other => other,
    };
    match text("type") {
        "agent_message" if completed => c.message(sink, text("text")),
        "reasoning" if completed => {
            c.flush(sink);
            sink.emit(EventBody::AgentThought { text: text("text").to_string() });
        }
        "command_execution" => {
            let command = text("command");
            c.tool(sink, id, "execute", command, status);
            let output = text("aggregated_output");
            let declined = text("status") == "declined";
            let blocked = text("status") == "failed" && common::looks_like_denial(output);
            if declined || blocked {
                let detail = if declined {
                    "declined by the approval policy".to_string()
                } else {
                    hufu_contract::proc::tail(output, 300)
                };
                c.deny(sink, command, "execute", &detail);
            }
        }
        "file_change" => {
            let mut paths = Vec::new();
            for change in item.get("changes").and_then(Value::as_array).into_iter().flatten() {
                if let Some(p) = change.get("path").and_then(Value::as_str) {
                    paths.push(p.to_string());
                    if status == "completed" {
                        c.touch(p);
                    }
                }
            }
            c.tool(sink, id, "edit", &paths.join(", "), status);
            if status == "failed" {
                c.deny(sink, &paths.join(", "), "edit", "the file change was not applied");
            }
        }
        "mcp_tool_call" => {
            let title = format!("{}.{}", text("server"), text("tool"));
            c.tool(sink, id, "other", &title, status);
        }
        "web_search" => c.tool(sink, id, "fetch", text("query"), status),
        "todo_list" => {
            c.flush(sink);
            let entries: Vec<Value> = item
                .get("items")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(|t| {
                    let done = t.get("completed").and_then(Value::as_bool).unwrap_or(false);
                    serde_json::json!({"content": t.get("text").cloned().unwrap_or(Value::Null), "status": if done { "completed" } else { "pending" }})
                })
                .collect();
            sink.emit(EventBody::Plan { entries: Value::Array(entries) });
        }
        "error" => {
            c.flush(sink);
            sink.emit(EventBody::warning(text("message")));
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::testing::*;

    #[test]
    fn every_tier_is_spelled_out() {
        let p = profile("codex-cli");
        let ro = CodexCli.plan(&p, &request(Tier::ReadOnly)).unwrap();
        assert!(ro.args.windows(2).any(|w| w == ["--sandbox", "read-only"]));
        let ww = CodexCli.plan(&p, &request(Tier::WorkspaceWrite)).unwrap();
        assert!(ww.args.windows(2).any(|w| w == ["--sandbox", "workspace-write"]));
        let full = CodexCli.plan(&p, &request(Tier::FullAccess)).unwrap();
        assert!(full.args.contains(&"--dangerously-bypass-approvals-and-sandbox".to_string()));
        assert!(!full.args.contains(&"--sandbox".to_string()));
        assert_eq!(ro.stdin.as_deref(), Some("do the thing"));
        assert_eq!(ro.args.last().map(String::as_str), Some("-"));
    }

    #[test]
    fn effort_model_and_resume_are_mapped() {
        let p = profile("codex-cli");
        let mut req = request(Tier::ReadOnly);
        req.effort = Effort::High;
        req.model = Some("gpt-6.2-sol".into());
        req.resume_session = Some("0199-abc".into());
        let plan = CodexCli.plan(&p, &req).unwrap();
        assert!(plan.args.contains(&"model_reasoning_effort=\"high\"".to_string()));
        assert!(plan.args.windows(2).any(|w| w == ["-m", "gpt-6.2-sol"]));
        let resume = plan.args.iter().position(|a| a == "resume").unwrap();
        let sandbox = plan.args.iter().position(|a| a == "--sandbox").unwrap();
        assert!(sandbox < resume, "non-global flags must precede `resume`");
        req.effort = Effort::XHigh;
        assert!(CodexCli.plan(&p, &req).unwrap().args.contains(&"model_reasoning_effort=\"xhigh\"".to_string()));
        req.effort = Effort::Off;
        assert!(CodexCli.plan(&p, &req).is_err());
    }

    #[test]
    fn parses_the_documented_event_stream() {
        let sink = Memory::default();
        let mut c = Collected::default();
        for line in [
            r#"{"type":"thread.started","thread_id":"0199a213-81c0-7800-8aa1-bbab2a035a53"}"#,
            r#"{"type":"turn.started"}"#,
            r#"{"type":"item.started","item":{"id":"item_1","type":"command_execution","command":"bash -lc ls","status":"in_progress"}}"#,
            r#"{"type":"item.completed","item":{"id":"item_1","type":"command_execution","command":"bash -lc ls","aggregated_output":"a\n","exit_code":0,"status":"completed"}}"#,
            r#"{"type":"item.completed","item":{"id":"item_2","type":"command_execution","command":"rm -rf x","status":"declined"}}"#,
            r#"{"type":"item.completed","item":{"id":"item_3","type":"file_change","changes":[{"path":"src/a.rs","kind":"update"}],"status":"completed"}}"#,
            r#"{"type":"item.completed","item":{"id":"item_4","type":"agent_message","text":"All done."}}"#,
            r#"{"type":"turn.completed","usage":{"input_tokens":24763,"cached_input_tokens":24448,"output_tokens":122,"reasoning_output_tokens":0}}"#,
        ] {
            parse_line(&mut c, &sink, line);
        }
        assert_eq!(c.session.as_deref(), Some("0199a213-81c0-7800-8aa1-bbab2a035a53"));
        assert_eq!(c.denied.len(), 1);
        assert_eq!(c.touched, ["src/a.rs"]);
        assert_eq!(c.usage.input_tokens, Some(24763));
        let out = c.outcome(&sink, ended(0), &request(Tier::WorkspaceWrite), true, "");
        assert_eq!(out.final_message.as_deref(), Some("All done."));
        assert!(sink.types().contains(&"permission.denied".to_string()));
    }

    #[test]
    fn reads_the_model_catalog() {
        let catalog = r#"{"models": [
            {"slug": "gpt-6.2-astra", "display_name": "GPT-6.2-Astra", "description": "Deep work.", "default_reasoning_level": "medium",
             "supported_reasoning_levels": [{"effort": "low", "description": "x"}, {"effort": "xhigh", "description": "y"}], "visibility": "list", "priority": 2},
            {"slug": "gpt-6.2-sol", "display_name": "GPT-6.2-Sol", "default_reasoning_level": "low",
             "supported_reasoning_levels": [{"effort": "low"}, {"effort": "ultra"}], "visibility": "list", "priority": 1},
            {"slug": "codex-internal", "display_name": "x", "visibility": "hide", "priority": 0}
        ]}"#;
        let list = super::parse_models(catalog, "`codex debug models`".into()).unwrap();
        let ids: Vec<&str> = list.models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["gpt-6.2-sol", "gpt-6.2-astra"]);
        assert_eq!(list.models[0].efforts, ["low", "ultra"]);
        assert_eq!(list.models[1].default_effort.as_deref(), Some("medium"));
        assert_eq!(list.models[1].name.as_deref(), Some("GPT-6.2-Astra"));
        assert!(super::parse_models("not json", "x".into()).is_err());
    }

    #[test]
    fn a_failed_turn_fails_the_run_even_with_exit_zero() {
        let sink = Memory::default();
        let mut c = Collected::default();
        parse_line(&mut c, &sink, r#"{"type":"turn.failed","error":{"message":"stream disconnected"}}"#);
        let out = c.outcome(&sink, ended(0), &request(Tier::ReadOnly), true, "");
        assert_eq!(out.reason.as_deref(), Some(hufu_contract::reason::AGENT_ERROR));
    }
}
