//! Helpers shared by the headless CLI adapters.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use hufu_contract::proc::{self, StreamResult};
pub use hufu_contract::proc::{expand_tilde, home_dir};
use hufu_contract::{
    AdapterError, DeniedAction, EventBody, EventSink, LaunchPlan, ProbeResult, Profile, RunOutcome, RunRequest,
    RunStatus, SessionRef, Usage, json::extract_json, reason,
};
use serde_json::Value;

/// A resolved agent executable plus any arguments that come before the adapter's own.
#[derive(Clone, Debug)]
pub struct Program {
    pub path: PathBuf,
    pub prefix: Vec<String>,
}

/// The command a profile runs: its own `command`, or the adapter's default.
pub fn command_of(profile: &Profile, default: &[&str]) -> Vec<String> {
    if profile.command.is_empty() { default.iter().map(|s| s.to_string()).collect() } else { profile.command.clone() }
}

/// Finds the agent executable. `path_var` is the `PATH` the agent itself will see.
pub fn resolve(profile: &Profile, default: &[&str], path_var: Option<&str>) -> Result<Program, String> {
    let command = command_of(profile, default);
    let name = command.first().cloned().unwrap_or_default();
    let found = match path_var {
        Some(p) => proc::which_in(&name, Some(std::ffi::OsStr::new(p))),
        None => proc::which(&name),
    };
    match found {
        Some(path) => Ok(Program { path, prefix: command[1..].to_vec() }),
        None => Err(format!("`{name}` was not found on PATH")),
    }
}

fn path_of(req: &RunRequest) -> Option<&str> {
    req.env.iter().find(|(k, _)| k.eq_ignore_ascii_case("PATH")).map(|(_, v)| v.as_str())
}

/// Builds the process for a launch plan: resolved program, clean environment, workspace as cwd.
pub fn command_from_plan(
    profile: &Profile,
    default: &[&str],
    req: &RunRequest,
    plan: &LaunchPlan,
) -> Result<Command, String> {
    let program = resolve(profile, default, path_of(req))?;
    let mut cmd = proc::command_for(&program.path);
    // `plan.args` already starts with the profile's prefix arguments.
    cmd.args(&plan.args);
    cmd.env_clear();
    cmd.envs(&req.env);
    cmd.envs(&plan.env);
    cmd.current_dir(&req.cwd);
    Ok(cmd)
}

/// Starts a launch plan's argument list with the profile's prefix arguments.
pub fn plan_start(profile: &Profile, default: &[&str]) -> LaunchPlan {
    let command = command_of(profile, default);
    LaunchPlan {
        program: command.first().cloned().unwrap_or_default(),
        args: command.iter().skip(1).cloned().collect(),
        ..LaunchPlan::default()
    }
}

/// Rejects extra arguments that would change the permission tier behind Hufu's back.
pub fn check_extra_args(profile: &Profile, forbidden: &[&str]) -> Result<(), AdapterError> {
    for arg in &profile.args {
        let flag = arg.split('=').next().unwrap_or(arg);
        if forbidden.contains(&flag) {
            return Err(AdapterError(format!(
                "profile `{}`: `args` may not contain `{flag}` because it changes the permission tier; \
                 set `permission` on the task or profile instead",
                profile.name
            )));
        }
    }
    Ok(())
}

/// Runs `<agent> <args>` briefly and returns the first non-empty line it printed.
pub fn first_line(program: &Program, args: &[&str], timeout: Duration) -> Option<String> {
    let mut cmd = proc::command_for(&program.path);
    cmd.args(&program.prefix).args(args);
    let out = proc::run_capture(cmd, timeout).ok()?;
    let text = if out.stdout.trim().is_empty() { out.stderr } else { out.stdout };
    text.lines().map(str::trim).find(|l| !l.is_empty()).map(str::to_string)
}

/// The usual probe: is the binary on PATH, and what does `--version` say.
pub fn probe_cli(profile: &Profile, default: &[&str], install_hint: &str) -> (ProbeResult, Option<Program>) {
    match resolve(profile, default, None) {
        Err(detail) => (
            ProbeResult {
                installed: false,
                detail: Some(detail),
                hint: Some(install_hint.to_string()),
                ..ProbeResult::default()
            },
            None,
        ),
        Ok(program) => {
            let version = first_line(&program, &["--version"], Duration::from_secs(10));
            (
                ProbeResult {
                    installed: true,
                    path: Some(program.path.display().to_string()),
                    version,
                    ..ProbeResult::default()
                },
                Some(program),
            )
        }
    }
}

/// Whether an environment variable is set to a non-empty value in Hufu's own environment.
pub fn env_set(name: &str) -> bool {
    std::env::var(name).map(|v| !v.trim().is_empty()).unwrap_or(false)
}

/// Whether `dir` is inside a git work tree.
pub fn in_git_repo(dir: &Path) -> bool {
    dir.ancestors().any(|d| d.join(".git").exists())
}

/// Text that means "log in first", across the agents Hufu drives.
pub fn looks_like_auth_error(text: &str) -> bool {
    let t = text.to_ascii_lowercase();
    [
        "authentication required",
        "not logged in",
        "not authenticated",
        "please run /login",
        "please log in",
        "login required",
        "unauthorized",
        "invalid api key",
        "no api key",
        "missing api key",
        "api key not found",
        "authrequired",
        "auth_required",
    ]
    .iter()
    .any(|needle| t.contains(needle))
}

/// Text that means "this account is out of quota or being throttled": worth trying another one.
pub fn looks_like_rate_limit(text: &str) -> bool {
    let t = text.to_ascii_lowercase();
    [
        "rate limit",
        "rate_limit",
        "ratelimit",
        "too many requests",
        "quota",
        "usage limit",
        "limit reached",
        "overloaded",
        "resource_exhausted",
        "resource exhausted",
        "insufficient credit",
        "insufficient balance",
        "status 429",
        "http 429",
        "429 too many",
        "529",
    ]
    .iter()
    .any(|needle| t.contains(needle))
}

/// The directory that holds this profile's agent login: the profile's `home`, the agent's own
/// variable, or a default under the user's home directory.
pub fn agent_home(profile: &Profile, var: &str, default_dir: &str) -> Option<PathBuf> {
    if let Some(home) = &profile.home {
        return Some(expand_tilde(home));
    }
    if let Some(v) = std::env::var_os(var).filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(v));
    }
    home_dir().map(|h| h.join(default_dir))
}

/// Text that means "blocked by a sandbox or permission rule".
pub fn looks_like_denial(text: &str) -> bool {
    let t = text.to_ascii_lowercase();
    [
        "permission denied",
        "operation not permitted",
        "read-only file system",
        "not permitted",
        "was denied",
        "denied by",
        "requires approval",
        "needs approval",
        "not allowed",
        "sandbox",
        "soft-denied",
        "rejected",
    ]
    .iter()
    .any(|needle| t.contains(needle))
}

/// Maps a tool name from any agent onto ACP's tool kinds.
pub fn tool_kind(name: &str) -> &'static str {
    let n = name.to_ascii_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| n.contains(w));
    if has(&["delete", "remove"]) {
        "delete"
    } else if has(&["move", "rename"]) {
        "move"
    } else if has(&["write", "edit", "patch", "create", "replace", "notebook"]) {
        "edit"
    } else if has(&["bash", "shell", "command", "exec", "terminal", "powershell", "run"]) {
        "execute"
    } else if has(&["grep", "glob", "search", "find", "ls", "list"]) && !has(&["web"]) {
        "search"
    } else if has(&["fetch", "web", "url", "browse", "http"]) {
        "fetch"
    } else if has(&["read", "view", "cat", "open"]) {
        "read"
    } else if has(&["think", "plan", "todo", "task"]) {
        "think"
    } else {
        "other"
    }
}

/// Clips a value for use as an event title.
pub fn title_of(value: &Value) -> String {
    let text = match value {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    };
    let one_line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() > 200 {
        format!("{}…", one_line.chars().take(200).collect::<String>())
    } else {
        one_line
    }
}

/// What a CLI adapter gathers while it reads an agent's output.
#[derive(Default)]
pub struct Collected {
    pending: Option<String>,
    pub final_message: Option<String>,
    pub session: Option<String>,
    pub usage: Usage,
    pub denied: Vec<DeniedAction>,
    /// An error the agent itself reported.
    pub error: Option<String>,
    pub structured: Option<Value>,
    pub touched: Vec<String>,
    pub retryable: bool,
    pub auth_required: bool,
}

impl Collected {
    /// Records a complete agent message. The newest one is held back until it is known whether
    /// it is the final answer.
    pub fn message(&mut self, sink: &dyn EventSink, text: &str) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        self.flush(sink);
        self.pending = Some(text.to_string());
        self.final_message = Some(text.to_string());
    }

    /// Emits the held-back message as an ordinary one. Call before any later event.
    pub fn flush(&mut self, sink: &dyn EventSink) {
        if let Some(text) = self.pending.take() {
            sink.emit(EventBody::message(text, false));
        }
    }

    /// Replaces the final answer, for agents that state it in a result record.
    pub fn set_final(&mut self, sink: &dyn EventSink, text: &str) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        if self.pending.as_deref() != Some(text) {
            self.flush(sink);
        }
        self.pending = Some(text.to_string());
        self.final_message = Some(text.to_string());
    }

    pub fn tool(&mut self, sink: &dyn EventSink, id: &str, kind: &str, title: &str, status: &str) {
        self.flush(sink);
        sink.emit(EventBody::tool(id, kind, title, status));
    }

    pub fn deny(&mut self, sink: &dyn EventSink, tool: &str, kind: &str, detail: &str) {
        self.flush(sink);
        let action = DeniedAction { tool: tool.to_string(), kind: kind.to_string(), detail: detail.to_string() };
        sink.emit(EventBody::PermissionDenied {
            tool: action.tool.clone(),
            kind: action.kind.clone(),
            detail: action.detail.clone(),
        });
        if !self.denied.contains(&action) {
            self.denied.push(action);
        }
    }

    pub fn fail(&mut self, sink: &dyn EventSink, message: &str) {
        self.flush(sink);
        sink.emit(EventBody::error(message));
        if looks_like_auth_error(message) {
            self.auth_required = true;
        }
        self.error = Some(message.to_string());
    }

    pub fn touch(&mut self, path: &str) {
        if !path.is_empty() && !self.touched.iter().any(|p| p == path) {
            self.touched.push(path.to_string());
        }
    }

    /// Turns what was gathered, and how the process ended, into the adapter's outcome.
    pub fn outcome(
        mut self,
        sink: &dyn EventSink,
        ended: StreamResult,
        req: &RunRequest,
        resumable: bool,
        login_hint: &str,
    ) -> RunOutcome {
        if let Some(text) = self.pending.take() {
            sink.emit(EventBody::message(text, true));
        }
        if !self.usage.is_empty() {
            sink.emit(EventBody::Usage { usage: self.usage.clone() });
        }
        let session = self.session.take().map(|id| SessionRef { id, resumable });
        let usage = if self.usage.is_empty() { None } else { Some(self.usage.clone()) };
        let mut out = RunOutcome {
            status: RunStatus::Succeeded,
            reason: None,
            detail: None,
            final_message: self.final_message.take(),
            structured_output: None,
            denied_actions: std::mem::take(&mut self.denied),
            session,
            usage,
            exit_code: ended.exit_code,
            retryable: self.retryable,
            touched_paths: std::mem::take(&mut self.touched),
        };
        let fail = |out: &mut RunOutcome, why: &str, detail: String| {
            out.status = RunStatus::Failed;
            out.reason = Some(why.to_string());
            out.detail = Some(detail);
        };
        if let Some(e) = ended.spawn_error {
            fail(&mut out, reason::AGENT_UNAVAILABLE, format!("could not start the agent: {e}"));
            return out;
        }
        if ended.cancelled {
            out.status = RunStatus::Cancelled;
            out.reason = Some(reason::CANCELLED.to_string());
            return out;
        }
        let exit_failed = ended.exit_code != Some(0);
        if self.error.is_some() || exit_failed {
            let detail = match (&self.error, ended.stderr_tail.is_empty()) {
                (Some(e), _) => e.clone(),
                (None, false) => proc::tail(&ended.stderr_tail, 1500),
                (None, true) => match ended.exit_code {
                    Some(code) => format!("the agent exited with code {code}"),
                    None => "the agent was terminated by a signal".to_string(),
                },
            };
            let auth =
                self.auth_required || looks_like_auth_error(&detail) || looks_like_auth_error(&ended.stderr_tail);
            if auth {
                fail(&mut out, reason::AUTH_REQUIRED, format!("{detail} ({login_hint})"));
            } else if looks_like_rate_limit(&detail) || looks_like_rate_limit(&ended.stderr_tail) {
                out.retryable = true;
                fail(&mut out, reason::RATE_LIMITED, detail);
            } else {
                fail(&mut out, reason::AGENT_ERROR, detail);
            }
            return out;
        }
        if out.final_message.as_deref().map(str::trim).unwrap_or("").is_empty() {
            fail(
                &mut out,
                reason::EMPTY_OUTPUT,
                "the agent exited with code 0 but produced no final message".to_string(),
            );
            return out;
        }
        if req.output_schema.is_some() {
            let structured = self.structured.take().or_else(|| out.final_message.as_deref().and_then(extract_json));
            match structured {
                Some(v) => out.structured_output = Some(v),
                None => fail(
                    &mut out,
                    reason::INVALID_OUTPUT,
                    "the final answer is not JSON, but the task asked for structured output".to_string(),
                ),
            }
        }
        out
    }
}

/// Runs a planned one-shot CLI invocation and feeds each output line to `on_line`.
pub fn run_plan(
    profile: &Profile,
    default: &[&str],
    req: &RunRequest,
    plan: &LaunchPlan,
    sink: &dyn EventSink,
    cancel: &hufu_contract::CancelToken,
    on_line: &mut dyn FnMut(hufu_contract::RawStream, &str),
) -> StreamResult {
    match command_from_plan(profile, default, req, plan) {
        Ok(cmd) => proc::run_streamed(cmd, plan.stdin.as_deref(), sink, cancel, on_line),
        Err(e) => StreamResult { spawn_error: Some(e), ..StreamResult::default() },
    }
}

/// Reads a `u64` out of a JSON object, accepting numbers stored as floats.
pub fn num(v: &Value, key: &str) -> Option<u64> {
    let x = v.get(key)?;
    x.as_u64().or_else(|| x.as_f64().filter(|f| *f >= 0.0).map(|f| f as u64))
}

/// Joins the text blocks of a content value that is either a string or an array of blocks.
pub fn text_of(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(items) => items
            .iter()
            .filter_map(|b| match b {
                Value::String(s) => Some(s.as_str()),
                _ if b.get("type").and_then(Value::as_str).map(|t| t == "text").unwrap_or(true) => {
                    b.get("text").and_then(Value::as_str)
                }
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(""),
        Value::Object(_) => content.get("text").and_then(Value::as_str).unwrap_or("").to_string(),
        _ => String::new(),
    }
}

#[cfg(test)]
pub mod testing {
    //! A sink that remembers everything, and request/profile builders for adapter tests.
    use super::*;
    use hufu_contract::{Effort, RawStream, Tier};
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    #[derive(Default)]
    pub struct Memory {
        pub events: Mutex<Vec<EventBody>>,
    }

    impl EventSink for Memory {
        fn emit(&self, event: EventBody) {
            self.events.lock().unwrap().push(event);
        }
        fn raw(&self, _: RawStream, _: &str) {}
        fn child_started(&self, _: u32) {}
    }

    impl Memory {
        pub fn types(&self) -> Vec<String> {
            self.events
                .lock()
                .unwrap()
                .iter()
                .map(|e| serde_json::to_value(e).unwrap()["type"].as_str().unwrap().to_string())
                .collect()
        }
    }

    pub fn profile(adapter: &str) -> Profile {
        Profile { name: "p".into(), adapter: adapter.into(), ..Profile::default() }
    }

    pub fn request(tier: Tier) -> RunRequest {
        RunRequest {
            job_id: "j_test".into(),
            prompt: "do the thing".into(),
            cwd: std::env::temp_dir(),
            permission: tier,
            effort: Effort::Default,
            model: None,
            output_schema: None,
            resume_session: None,
            env: BTreeMap::new(),
            mcp_servers: Vec::new(),
            scratch_dir: std::env::temp_dir(),
        }
    }

    pub fn ended(code: i32) -> StreamResult {
        StreamResult { exit_code: Some(code), ..StreamResult::default() }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use super::*;
    use hufu_contract::Tier;

    #[test]
    fn exit_zero_without_a_message_is_not_success() {
        let sink = Memory::default();
        let out = Collected::default().outcome(&sink, ended(0), &request(Tier::ReadOnly), false, "log in");
        assert_eq!(out.status, RunStatus::Failed);
        assert_eq!(out.reason.as_deref(), Some(reason::EMPTY_OUTPUT));
    }

    #[test]
    fn the_last_message_is_marked_final() {
        let sink = Memory::default();
        let mut c = Collected::default();
        c.message(&sink, "working");
        c.tool(&sink, "1", "read", "a.rs", "completed");
        c.message(&sink, "done");
        let out = c.outcome(&sink, ended(0), &request(Tier::ReadOnly), true, "log in");
        assert_eq!(out.status, RunStatus::Succeeded);
        assert_eq!(out.final_message.as_deref(), Some("done"));
        let events = sink.events.lock().unwrap();
        let finals: Vec<bool> = events
            .iter()
            .filter_map(|e| match e {
                EventBody::AgentMessage { is_final, .. } => Some(*is_final),
                _ => None,
            })
            .collect();
        assert_eq!(finals, [false, true]);
    }

    #[test]
    fn auth_failures_are_classified() {
        let sink = Memory::default();
        let ended = StreamResult {
            exit_code: Some(1),
            stderr_tail: "Error: authentication required".into(),
            ..StreamResult::default()
        };
        let out = Collected::default().outcome(&sink, ended, &request(Tier::ReadOnly), false, "run `x login`");
        assert_eq!(out.reason.as_deref(), Some(reason::AUTH_REQUIRED));
        assert!(out.detail.unwrap().contains("x login"));
    }

    #[test]
    fn structured_output_is_extracted_or_reported() {
        let sink = Memory::default();
        let mut req = request(Tier::ReadOnly);
        req.output_schema = Some(serde_json::json!({"type":"object"}));
        let mut c = Collected::default();
        c.message(&sink, "```json\n{\"ok\":true}\n```");
        let out = c.outcome(&sink, ended(0), &req, false, "");
        assert_eq!(out.structured_output.unwrap()["ok"], true);
        let mut c = Collected::default();
        c.message(&sink, "sorry, no json");
        let out = c.outcome(&sink, ended(0), &req, false, "");
        assert_eq!(out.reason.as_deref(), Some(reason::INVALID_OUTPUT));
    }

    #[test]
    fn tier_changing_args_are_rejected() {
        let mut p = profile("codex-cli");
        p.args = vec!["--sandbox=danger-full-access".into()];
        assert!(check_extra_args(&p, &["--sandbox"]).is_err());
        p.args = vec!["--search".into()];
        assert!(check_extra_args(&p, &["--sandbox"]).is_ok());
    }

    #[test]
    fn tool_names_map_to_acp_kinds() {
        assert_eq!(tool_kind("Bash"), "execute");
        assert_eq!(tool_kind("writeToolCall"), "edit");
        assert_eq!(tool_kind("Grep"), "search");
        assert_eq!(tool_kind("WebFetch"), "fetch");
        assert_eq!(tool_kind("read_file"), "read");
        assert_eq!(tool_kind("mystery"), "other");
    }
}
