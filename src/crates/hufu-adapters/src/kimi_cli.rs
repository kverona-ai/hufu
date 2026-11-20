//! Kimi Code CLI (`kimi -p --output-format stream-json`).
//!
//! Print mode runs every tool call without asking, so `full-access` is its only tier. For lower
//! tiers use the ACP surface (`kimi acp`) through the generic ACP adapter.
//!
//! Checked against the Kimi Code documentation on 2026-11-20 (CLI 2.7):
//! - the stream ends with a `session.resume_hint` line that carries the session id, and
//!   `kimi -S <id> -p ...` continues that session;
//! - exit code 4 means the session is held by another process;
//! - `KIMI_MODEL_THINKING_EFFORT` only applies to a model defined in the environment
//!   (`KIMI_MODEL_NAME`); for the logged-in `kimi-code` models the effort lives in `config.toml`
//!   and there is no per-run flag, so Hufu offers effort only to profiles that define the model.

use std::collections::BTreeMap;
use std::path::Path;

use hufu_contract::{
    Adapter, AdapterError, CancelToken, Capabilities, Effort, EventSink, LaunchPlan, ModelInfo, ModelList, ProbeResult,
    Profile, RawStream, RunOutcome, RunRequest, StructuredOutput, Tier, json::instruct_json,
};
use serde_json::Value;

use crate::common::{self, Collected};

const DEFAULT: &[&str] = &["kimi"];
const INSTALL: &str = "install Kimi Code CLI: `curl -fsSL https://code.kimi.com/kimi-code/install.sh | bash` (Windows: `irm https://code.kimi.com/kimi-code/install.ps1 | iex`), then run `kimi login`";
const FORBIDDEN: &[&str] = &["--yolo", "-y", "--auto", "--plan", "--yes", "--auto-approve", "--add-dir"];
/// The variable that makes Kimi Code build its model from `KIMI_MODEL_*` variables.
const MODEL_FROM_ENV: &str = "KIMI_MODEL_NAME";

/// Whether the profile defines its model through the environment, which is the only case in
/// which `KIMI_MODEL_THINKING_EFFORT` is read.
fn defines_model(profile: &Profile) -> bool {
    profile.env.iter().any(|v| v.eq_ignore_ascii_case(MODEL_FROM_ENV))
        || profile.env_from.keys().any(|v| v.eq_ignore_ascii_case(MODEL_FROM_ENV))
        || profile.vars.keys().any(|v| v.eq_ignore_ascii_case(MODEL_FROM_ENV))
}

pub struct KimiCli;

/// The model aliases of a Kimi Code `config.toml`: `[models."<alias>"]` tables with their
/// `support_efforts` and `default_effort`, and the top-level `default_model` marked.
fn parse_models(text: &str, from: String) -> ModelList {
    let default = common::toml_top_string(text, "default_model");
    let mut models: Vec<ModelInfo> = Vec::new();
    let mut current: Option<usize> = None;
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            current = None;
            let Some(key) = line.strip_prefix("[models.").and_then(|k| k.strip_suffix(']')) else { continue };
            let alias = common::toml_unquote(key).unwrap_or_else(|| key.trim().to_string());
            if alias.is_empty() || (!key.trim_start().starts_with(['"', '\'']) && alias.contains('.')) {
                continue;
            }
            current = Some(models.len());
            models.push(ModelInfo {
                default: default.as_deref() == Some(alias.as_str()),
                id: alias,
                ..ModelInfo::default()
            });
        } else if let Some(i) = current {
            if let Some(model) = common::toml_assignment(line, "model").and_then(common::toml_unquote) {
                models[i].description = Some(format!("model `{model}`"));
            } else if let Some(name) = common::toml_assignment(line, "display_name").and_then(common::toml_unquote) {
                models[i].name = Some(name);
            } else if let Some(efforts) = common::toml_assignment(line, "support_efforts") {
                models[i].efforts = common::toml_string_array(efforts);
            } else if let Some(effort) = common::toml_assignment(line, "default_effort").and_then(common::toml_unquote)
            {
                models[i].default_effort = Some(effort);
            }
        }
    }
    ModelList { source: "config".into(), from, models, ..ModelList::default() }
}

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

    fn capabilities(&self, profile: &Profile) -> Capabilities {
        let env_model = defines_model(profile);
        let mut notes = vec![
            "print mode approves every tool call except what `[[permission.rules]]` deny; run it with worktree isolation or in a container, or use an ACP profile (`command: [kimi, acp]`) for lower tiers".into(),
            "models: k3.1-pro, k3, kimi-for-coding and their variants, by the alias defined in config.toml (for example kimi-code/k3)".into(),
        ];
        if env_model {
            notes.push("effort is passed as KIMI_MODEL_THINKING_EFFORT, which Kimi Code reads for the model defined by KIMI_MODEL_NAME".into());
        } else {
            notes.push("no per-run effort: Kimi Code reads the effort from `[thinking] effort` in config.toml; use the `kimi-acp` profile, which sets it per session, or define the model with KIMI_MODEL_NAME".into());
        }
        Capabilities {
            permission_tiers: vec![Tier::FullAccess],
            efforts: if env_model { Effort::LOW_TO_MAX.to_vec() } else { Vec::new() },
            native_effort: env_model,
            checked_at_runtime: false,
            models: Vec::new(),
            resume: true,
            structured_output: StructuredOutput::Instructed,
            reports_usage: false,
            reports_denials: false,
            accepts_mcp_servers: false,
            provides: Vec::new(),
            notes,
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
        let mut plan = common::plan_start(profile, DEFAULT);
        let prompt = match &req.output_schema {
            Some(schema) => instruct_json(&req.prompt, schema),
            None => req.prompt.clone(),
        };
        plan.args.extend(["-p".into(), prompt, "--output-format".into(), "stream-json".into()]);
        if let Some(model) = &req.model {
            plan.args.extend(["-m".into(), model.clone()]);
        }
        if let Some(session) = &req.resume_session {
            plan.args.extend(["-S".into(), session.clone()]);
        }
        let level = match &req.effort {
            Effort::Default => None,
            _ if !defines_model(profile) => {
                return Err(AdapterError(format!(
                    "Kimi Code print mode has no per-run effort for its logged-in models, so `{}` cannot be honoured. \
                     Set `[thinking] effort` in Kimi's config.toml, use the `kimi-acp` profile, or define the model with {MODEL_FROM_ENV}",
                    req.effort
                )));
            }
            Effort::Off => {
                return Err(AdapterError(
                    "Kimi Code has no `off` effort level; accepted: default, low, medium, high, xhigh, max, native:<value>"
                        .into(),
                ));
            }
            Effort::Native(v) => Some(v.clone()),
            level => Some(level.to_string()),
        };
        if let Some(level) = level {
            plan.env.insert("KIMI_MODEL_THINKING_EFFORT".into(), level);
        }
        plan.args.extend(profile.args.iter().cloned());
        Ok(plan)
    }

    /// The models are the aliases in Kimi Code's `config.toml` (`[models.<alias>]` and
    /// `default_model`), which `kimi login` writes; `--model` takes an alias.
    fn models(
        &self,
        profile: &Profile,
        env: &BTreeMap<String, String>,
        _cwd: &Path,
    ) -> Result<ModelList, AdapterError> {
        let home = env
            .get("KIMI_CODE_HOME")
            .map(std::path::PathBuf::from)
            .or_else(|| common::agent_home(profile, "KIMI_CODE_HOME", ".kimi-code"))
            .ok_or_else(|| AdapterError("cannot find Kimi Code's home directory".into()))?;
        let path = home.join("config.toml");
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let mut list = parse_models(&text, path.display().to_string());
        if let Some(name) = env.get(MODEL_FROM_ENV).or_else(|| profile.vars.get(MODEL_FROM_ENV)) {
            list.models.iter_mut().for_each(|m| m.default = false);
            list.models.insert(
                0,
                ModelInfo {
                    id: name.clone(),
                    description: Some(format!("defined by {MODEL_FROM_ENV}; jobs of this profile use it")),
                    efforts: Effort::LOW_TO_MAX.iter().map(|e| e.to_string()).collect(),
                    default: true,
                    ..ModelInfo::default()
                },
            );
        }
        if list.models.is_empty() {
            return Err(AdapterError(format!(
                "{} defines no models; run `kimi login`, which writes them, or define one with {MODEL_FROM_ENV}",
                path.display()
            )));
        }
        if !defines_model(profile) {
            list.notes.push("effort for these models comes from `[thinking] effort` in config.toml; the kimi-acp profile sets it per session".into());
        }
        Ok(list)
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
        if ended.exit_code == Some(4) && c.error.is_none() {
            c.fail(
                sink,
                "the Kimi session is held by another process (exit code 4); wait for it to end, or start a new session",
            );
        }
        c.outcome(sink, ended, req, true, "run `kimi login`")
    }
}

pub(crate) fn parse_line(c: &mut Collected, sink: &dyn EventSink, line: &str) {
    let Ok(v) = serde_json::from_str::<Value>(line) else {
        return;
    };
    if v.get("type").and_then(Value::as_str) == Some("session.resume_hint") {
        if let Some(id) = v.get("session_id").and_then(Value::as_str).filter(|x| !x.is_empty()) {
            c.session = Some(id.to_string());
        }
        return;
    }
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
    fn model_aliases_come_from_config_toml() {
        let text = "default_model = \"kimi-code/k3\"\n\n[models.\"kimi-code/k3\"]\nprovider = \"kimi\"\nmodel = \"k3\"\n\n[models.\"kimi-code/k3.1-pro\"]\nmodel = \"k3.1-pro\"\nsupport_efforts = [\"low\", \"high\", \"max\"]\ndefault_effort = \"high\"\n\n[thinking]\neffort = \"high\"\n";
        let list = parse_models(text, "config.toml".into());
        let ids: Vec<&str> = list.models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["kimi-code/k3", "kimi-code/k3.1-pro"]);
        assert!(list.models[0].default && !list.models[1].default);
        assert_eq!(list.models[1].description.as_deref(), Some("model `k3.1-pro`"));
        assert_eq!(list.models[1].efforts, ["low", "high", "max"]);
        assert_eq!(list.models[1].default_effort.as_deref(), Some("high"));
        assert!(parse_models("", "x".into()).models.is_empty());
    }

    #[test]
    fn only_full_access_is_accepted() {
        let p = profile("kimi-cli");
        assert_eq!(KimiCli.capabilities(&p).permission_tiers, [Tier::FullAccess]);
        assert!(KimiCli.plan(&p, &request(Tier::ReadOnly)).unwrap_err().0.contains("full-access"));
        let plan = KimiCli.plan(&p, &request(Tier::FullAccess)).unwrap();
        assert!(plan.args.starts_with(&["-p".to_string(), "do the thing".to_string()]));
        assert!(plan.env.is_empty());
    }

    #[test]
    fn effort_is_offered_only_for_a_model_defined_in_the_environment() {
        let p = profile("kimi-cli");
        assert!(KimiCli.capabilities(&p).efforts.is_empty());
        let mut req = request(Tier::FullAccess);
        req.effort = Effort::High;
        assert!(KimiCli.plan(&p, &req).unwrap_err().0.contains("config.toml"));

        let mut env_model = profile("kimi-cli");
        env_model.env = vec!["KIMI_MODEL_NAME".into(), "KIMI_MODEL_API_KEY".into()];
        assert_eq!(KimiCli.capabilities(&env_model).efforts, Effort::LOW_TO_MAX);
        req.effort = Effort::XHigh;
        let plan = KimiCli.plan(&env_model, &req).unwrap();
        assert_eq!(plan.env.get("KIMI_MODEL_THINKING_EFFORT").map(String::as_str), Some("xhigh"));
        req.effort = Effort::Off;
        assert!(KimiCli.plan(&env_model, &req).is_err());
    }

    #[test]
    fn a_session_hint_makes_the_job_resumable() {
        let p = profile("kimi-cli");
        let mut req = request(Tier::FullAccess);
        req.resume_session = Some("s-42".into());
        let plan = KimiCli.plan(&p, &req).unwrap();
        assert!(plan.args.windows(2).any(|w| w == ["-S", "s-42"]), "{:?}", plan.args);
        let sink = Memory::default();
        let mut c = Collected::default();
        parse_line(&mut c, &sink, r#"{"role":"assistant","content":"Done."}"#);
        parse_line(&mut c, &sink, r#"{"type":"session.resume_hint","session_id":"s-43"}"#);
        let out = c.outcome(&sink, ended(0), &req, true, "");
        let session = out.session.unwrap();
        assert_eq!((session.id.as_str(), session.resumable), ("s-43", true));
        assert_eq!(out.final_message.as_deref(), Some("Done."));
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
