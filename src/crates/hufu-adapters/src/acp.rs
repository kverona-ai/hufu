//! Generic Agent Client Protocol (ACP v1) adapter: drives any agent that speaks ACP over stdio.
//!
//! The client is deliberately small and tolerant. It speaks newline-delimited JSON-RPC 2.0,
//! advertises no file-system or terminal capabilities (so the agent uses its own tools), and
//! answers every permission request from the tier policy in `doc/technical/adapter-contract.md`.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

use hufu_contract::proc::{self, Step, Streamed};
use hufu_contract::{
    Adapter, AdapterError, CancelToken, Capabilities, Effort, EffortFallback, EventBody, EventSink, LaunchPlan,
    PermissionRequests, ProbeResult, Profile, RawStream, RunOutcome, RunRequest, StructuredOutput, Tier, Usage,
    json::instruct_json, reason,
};
use serde_json::{Value, json};

use crate::common::{self, Collected};

pub struct Acp;

const HANDSHAKE: Duration = Duration::from_secs(120);

impl Adapter for Acp {
    fn kind(&self) -> &'static str {
        "acp"
    }

    fn default_command(&self) -> Vec<String> {
        Vec::new()
    }

    fn probe(&self, profile: &Profile) -> ProbeResult {
        if profile.command.is_empty() {
            return ProbeResult {
                installed: false,
                detail: Some("an ACP profile needs `command`, the argv that starts the agent in ACP mode".into()),
                hint: Some("for example `command: [kimi, acp]` or `command: [gemini, --acp]`".into()),
                ..ProbeResult::default()
            };
        }
        let name = profile.command[0].clone();
        match proc::which(&name) {
            None => ProbeResult {
                installed: false,
                detail: Some(format!("`{name}` was not found on PATH")),
                hint: Some(format!("install the agent that provides `{name}`")),
                ..ProbeResult::default()
            },
            Some(path) => {
                let program = common::Program { path: path.clone(), prefix: Vec::new() };
                ProbeResult {
                    installed: true,
                    path: Some(path.display().to_string()),
                    version: common::first_line(&program, &["--version"], Duration::from_secs(10)),
                    authenticated: None,
                    detail: Some("login state is reported by the agent when a session opens".into()),
                    hint: None,
                }
            }
        }
    }

    fn capabilities(&self, profile: &Profile) -> Capabilities {
        let verified = profile.permission_requests == PermissionRequests::Verified;
        let mut notes = vec![
            "model and effort are matched against the options the agent advertises when the session opens".to_string(),
        ];
        if !verified {
            notes.push(
                "only full-access is offered: set `permission_requests: verified` once you have checked that this agent asks before it edits or runs commands".into(),
            );
        }
        Capabilities {
            permission_tiers: if verified { Tier::ALL.to_vec() } else { vec![Tier::FullAccess] },
            efforts: Effort::SCALE.to_vec(),
            native_effort: true,
            checked_at_runtime: true,
            models: Vec::new(),
            resume: true,
            structured_output: StructuredOutput::Instructed,
            reports_usage: true,
            reports_denials: true,
            accepts_mcp_servers: true,
            provides: Vec::new(),
            notes,
        }
    }

    fn plan(&self, profile: &Profile, req: &RunRequest) -> Result<LaunchPlan, AdapterError> {
        if profile.command.is_empty() {
            return Err(AdapterError(format!(
                "profile `{}`: adapter `acp` needs `command`, for example `command: [kimi, acp]`",
                profile.name
            )));
        }
        if req.permission != Tier::FullAccess && profile.permission_requests != PermissionRequests::Verified {
            return Err(AdapterError(format!(
                "profile `{}` cannot run at `{}`: its permission requests are unverified, so only full-access is offered. \
                 Set `permission_requests: verified` after checking that the agent asks before acting",
                profile.name, req.permission
            )));
        }
        let mut plan = common::plan_start(profile, &[]);
        plan.args.extend(profile.args.iter().cloned());
        plan.notes.push("speaks ACP v1 over stdio; the prompt is sent with session/prompt".into());
        Ok(plan)
    }

    fn run(&self, profile: &Profile, req: &RunRequest, sink: &dyn EventSink, cancel: &CancelToken) -> RunOutcome {
        let plan = match self.plan(profile, req) {
            Ok(p) => p,
            Err(e) => return RunOutcome::failed(reason::UNSUPPORTED, e.0),
        };
        let cmd = match common::command_from_plan(profile, &[], req, &plan) {
            Ok(c) => c,
            Err(e) => {
                return RunOutcome::failed(reason::AGENT_UNAVAILABLE, format!("could not start the agent: {e}"));
            }
        };
        let proc = match Streamed::spawn(cmd, None, true, sink) {
            Ok(p) => p,
            Err(e) => {
                return RunOutcome::failed(reason::AGENT_UNAVAILABLE, format!("could not start the agent: {e}"));
            }
        };
        let mut client = Client {
            proc,
            sink,
            cancel,
            profile,
            req,
            next_id: 1,
            c: Collected::default(),
            text: String::new(),
            thought: String::new(),
            tools: HashMap::new(),
            rejected: Vec::new(),
            replaying: false,
            session_id: None,
            cancel_sent_at: None,
        };
        let result = client.session();
        client.finish(result)
    }
}

/// Why a JSON-RPC exchange did not produce a result.
#[derive(Debug)]
enum Rpc {
    Error { code: i64, message: String },
    Closed,
    Timeout,
    Cancelled,
}

/// How the prompt turn ended, before it is turned into an outcome.
enum End {
    Stop(String),
    Fail(&'static str, String),
}

struct Client<'a> {
    proc: Streamed,
    sink: &'a dyn EventSink,
    cancel: &'a CancelToken,
    profile: &'a Profile,
    req: &'a RunRequest,
    next_id: u64,
    c: Collected,
    text: String,
    thought: String,
    /// Tool call id -> (kind, title), so later updates and permission requests can be described.
    tools: HashMap<String, (String, String)>,
    rejected: Vec<String>,
    replaying: bool,
    session_id: Option<String>,
    cancel_sent_at: Option<Instant>,
}

impl Client<'_> {
    fn send(&mut self, msg: &Value) -> bool {
        self.proc.send_line(&msg.to_string()).is_ok()
    }

    fn call(&mut self, method: &str, params: Value, timeout: Option<Duration>) -> Result<Value, Rpc> {
        let id = self.next_id;
        self.next_id += 1;
        if !self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})) {
            return Err(Rpc::Closed);
        }
        let deadline = timeout.map(|t| Instant::now() + t);
        let in_turn = method == "session/prompt";
        loop {
            if self.cancel.is_cancelled() {
                if !in_turn {
                    return Err(Rpc::Cancelled);
                }
                match self.cancel_sent_at {
                    None => {
                        self.cancel_sent_at = Some(Instant::now());
                        if let Some(sid) = self.session_id.clone() {
                            self.send(
                                &json!({"jsonrpc": "2.0", "method": "session/cancel", "params": {"sessionId": sid}}),
                            );
                        }
                    }
                    Some(at) if at.elapsed() >= proc::kill_grace() => return Err(Rpc::Cancelled),
                    Some(_) => {}
                }
            }
            if deadline.map(|d| Instant::now() >= d).unwrap_or(false) {
                return Err(Rpc::Timeout);
            }
            match self.proc.next(self.sink, Duration::from_millis(100)) {
                Step::Line(RawStream::Stdout, line) => {
                    if let Some(result) = self.incoming(&line, id) {
                        return result;
                    }
                }
                Step::Line(RawStream::Stderr, _) | Step::Idle => {}
                Step::Done => return Err(Rpc::Closed),
            }
        }
    }

    /// Handles one line from the agent. Returns the response to request `waiting_for`, if this is it.
    fn incoming(&mut self, line: &str, waiting_for: u64) -> Option<Result<Value, Rpc>> {
        let msg: Value = serde_json::from_str(line).ok()?;
        let method = msg.get("method").and_then(Value::as_str);
        match (method, msg.get("id")) {
            (Some(method), Some(id)) if !id.is_null() => {
                let id = id.clone();
                let params = msg.get("params").cloned().unwrap_or(Value::Null);
                let reply = self.agent_request(method, &params);
                let body = match reply {
                    Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
                    Err((code, message)) => {
                        json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
                    }
                };
                self.send(&body);
                None
            }
            (Some(method), _) => {
                if method == "session/update" && !self.replaying {
                    if let Some(update) = msg.pointer("/params/update") {
                        self.update(update);
                    }
                }
                None
            }
            (None, Some(id)) if id.as_u64() == Some(waiting_for) => {
                if let Some(err) = msg.get("error") {
                    Some(Err(Rpc::Error {
                        code: err.get("code").and_then(Value::as_i64).unwrap_or(-32603),
                        message: err.get("message").and_then(Value::as_str).unwrap_or("unknown error").to_string(),
                    }))
                } else {
                    Some(Ok(msg.get("result").cloned().unwrap_or(Value::Null)))
                }
            }
            _ => None,
        }
    }

    fn agent_request(&mut self, method: &str, params: &Value) -> Result<Value, (i64, String)> {
        match method {
            "session/request_permission" => Ok(self.permission(params)),
            other => Err((-32601, format!("method not found: {other} (this client advertises no such capability)"))),
        }
    }

    // --- permission policy -------------------------------------------------------------------

    fn permission(&mut self, params: &Value) -> Value {
        let call = params.get("toolCall").cloned().unwrap_or(Value::Null);
        let id = call.get("toolCallId").and_then(Value::as_str).unwrap_or("").to_string();
        let known = self.tools.get(&id).cloned();
        let kind = call
            .get("kind")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| known.as_ref().map(|k| k.0.clone()))
            .unwrap_or_else(|| "other".to_string());
        let title = call
            .get("title")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| known.map(|k| k.1))
            .unwrap_or_else(|| kind.clone());
        self.tools.insert(id.clone(), (kind.clone(), title.clone()));
        let options: Vec<(String, String)> = params
            .get("options")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|o| {
                Some((
                    o.get("optionId")?.as_str()?.to_string(),
                    o.get("kind").and_then(Value::as_str).unwrap_or("").to_string(),
                ))
            })
            .collect();
        let pick = |wanted: &str| options.iter().find(|(_, k)| k == wanted).map(|(id, _)| id.clone());

        self.flush_text();
        if self.cancel.is_cancelled() {
            return json!({"outcome": {"outcome": "cancelled"}});
        }
        let (mut allow, mut why) = decide(self.req.permission, self.profile, &self.req.cwd, &kind, &call);
        let mut chosen = None;
        if allow {
            chosen = pick("allow_once");
            if chosen.is_none() && self.req.permission == Tier::FullAccess {
                chosen = pick("allow_always");
            }
            if chosen.is_none() {
                allow = false;
                why = "the agent offered no once-only allow option".to_string();
            }
        }
        if !allow {
            chosen = pick("reject_once").or_else(|| pick("reject_always"));
        }
        self.sink.emit(EventBody::PermissionRequest {
            tool: title.clone(),
            kind: kind.clone(),
            decision: if allow { "allow" } else { "reject" }.to_string(),
            why: Some(why.clone()),
        });
        if !allow {
            self.rejected.push(id);
            self.c.deny(self.sink, &title, &kind, &why);
        }
        match chosen {
            Some(option) => json!({"outcome": {"outcome": "selected", "optionId": option}}),
            None => json!({"outcome": {"outcome": "cancelled"}}),
        }
    }

    // --- session/update translation ----------------------------------------------------------

    fn flush_text(&mut self) {
        if !self.thought.trim().is_empty() {
            self.c.flush(self.sink);
            self.sink.emit(EventBody::AgentThought { text: std::mem::take(&mut self.thought).trim().to_string() });
        }
        self.thought.clear();
        if !self.text.trim().is_empty() {
            let text = std::mem::take(&mut self.text);
            self.c.message(self.sink, &text);
        }
        self.text.clear();
    }

    fn update(&mut self, u: &Value) {
        let text = |key: &str| u.get(key).and_then(Value::as_str);
        match text("sessionUpdate").unwrap_or("") {
            "agent_message_chunk" => {
                if !self.thought.is_empty() {
                    self.flush_text();
                }
                if let Some(c) = u.get("content") {
                    self.text.push_str(&common::text_of(c));
                }
            }
            "agent_thought_chunk" => {
                if !self.text.is_empty() {
                    self.flush_text();
                }
                if let Some(c) = u.get("content") {
                    self.thought.push_str(&common::text_of(c));
                }
            }
            "tool_call" | "tool_call_update" => {
                self.flush_text();
                let id = text("toolCallId").unwrap_or("tool").to_string();
                let known = self.tools.get(&id).cloned();
                let kind = text("kind")
                    .map(str::to_string)
                    .or_else(|| known.as_ref().map(|k| k.0.clone()))
                    .unwrap_or_else(|| "other".into());
                let title =
                    text("title").map(str::to_string).or_else(|| known.map(|k| k.1)).unwrap_or_else(|| kind.clone());
                let status = text("status").unwrap_or(if text("sessionUpdate") == Some("tool_call") {
                    "pending"
                } else {
                    "in_progress"
                });
                self.tools.insert(id.clone(), (kind.clone(), title.clone()));
                self.c.tool(self.sink, &id, &kind, &title, status);
                if status == "completed" && matches!(kind.as_str(), "edit" | "delete" | "move") {
                    for path in paths_of(u) {
                        self.c.touch(&path);
                    }
                }
            }
            "plan" => {
                self.flush_text();
                self.sink
                    .emit(EventBody::Plan { entries: u.get("entries").cloned().unwrap_or(Value::Array(Vec::new())) });
            }
            "usage_update" => {
                self.c.usage = Usage {
                    total_tokens: common::num(u, "used"),
                    cost_usd: u.pointer("/cost/amount").and_then(Value::as_f64),
                    ..Usage::default()
                };
            }
            _ => {}
        }
    }

    // --- the session -------------------------------------------------------------------------

    fn session(&mut self) -> End {
        let fail = |why: &'static str, detail: String| End::Fail(why, detail);
        let init = self.call(
            "initialize",
            json!({
                "protocolVersion": 1,
                "clientCapabilities": {"fs": {"readTextFile": false, "writeTextFile": false}, "terminal": false},
                "clientInfo": {"name": "hufu", "title": "Hufu", "version": env!("CARGO_PKG_VERSION")}
            }),
            Some(HANDSHAKE),
        );
        let init = match init {
            Ok(v) => v,
            Err(e) => return self.rpc_failure("initialize", e),
        };
        let version = init.get("protocolVersion").and_then(Value::as_u64).unwrap_or(1);
        if version != 1 {
            return fail(
                reason::PROTOCOL_ERROR,
                format!("the agent speaks ACP protocol version {version}; Hufu supports version 1"),
            );
        }
        let caps = init.get("agentCapabilities").cloned().unwrap_or(Value::Null);
        let methods: Vec<(String, bool)> = init
            .get("authMethods")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|m| {
                Some((m.get("id")?.as_str()?.to_string(), m.get("type").and_then(Value::as_str) == Some("terminal")))
            })
            .collect();

        // A profile that names an auth method wants it called up front (Cursor's documented flow).
        if let Some(method) = self.profile.auth_method.clone() {
            if let Err(e) = self.call("authenticate", json!({"methodId": method}), Some(HANDSHAKE)) {
                return self.rpc_failure("authenticate", e);
            }
        }

        let servers: Vec<Value> = self
            .req
            .mcp_servers
            .iter()
            .map(|s| {
                let command =
                    proc::which(&s.command).map(|p| p.display().to_string()).unwrap_or_else(|| s.command.clone());
                let env: Vec<Value> = s.env.iter().map(|(k, v)| json!({"name": k, "value": v})).collect();
                json!({"name": s.name, "command": command, "args": s.args, "env": env})
            })
            .collect();
        let cwd = self.req.cwd.display().to_string();
        let open = |me: &mut Self| -> Result<(String, Value), Rpc> {
            match &me.req.resume_session {
                None => {
                    let r = me.call("session/new", json!({"cwd": cwd, "mcpServers": servers}), Some(HANDSHAKE))?;
                    let id = r.get("sessionId").and_then(Value::as_str).unwrap_or("").to_string();
                    Ok((id, r))
                }
                Some(id) => {
                    let params = json!({"sessionId": id, "cwd": cwd, "mcpServers": servers});
                    let can_resume = caps
                        .pointer("/sessionCapabilities/resume")
                        .map(|v| !v.is_null() && v != &Value::Bool(false))
                        .unwrap_or(false);
                    let can_load = caps.get("loadSession").and_then(Value::as_bool).unwrap_or(false);
                    if can_resume {
                        let r = me.call("session/resume", params, Some(HANDSHAKE))?;
                        Ok((id.clone(), r))
                    } else if can_load {
                        me.replaying = true;
                        let r = me.call("session/load", params, Some(HANDSHAKE));
                        me.replaying = false;
                        Ok((id.clone(), r?))
                    } else {
                        Err(Rpc::Error {
                            code: -32601,
                            message: "the agent can neither resume nor load sessions".into(),
                        })
                    }
                }
            }
        };
        let opened = match open(self) {
            Err(Rpc::Error { code: -32000, .. }) if self.profile.auth_method.is_none() => {
                // Not logged in. Try the agent's stored login once through a non-interactive
                // method; Hufu never performs an interactive login.
                match methods.iter().find(|(_, terminal)| !terminal) {
                    Some((method, _)) => {
                        match self.call("authenticate", json!({"methodId": method}), Some(HANDSHAKE)) {
                            Ok(_) => open(self),
                            Err(e) => return self.rpc_failure("authenticate", e),
                        }
                    }
                    None => Err(Rpc::Error { code: -32000, message: "authentication required".into() }),
                }
            }
            other => other,
        };
        let (session_id, opened) = match opened {
            Ok(v) => v,
            Err(e) => return self.rpc_failure("session/new", e),
        };
        if session_id.is_empty() {
            return fail(reason::PROTOCOL_ERROR, "the agent returned no session id".into());
        }
        self.session_id = Some(session_id.clone());
        self.c.session = Some(session_id.clone());

        let options = opened.get("configOptions").cloned().unwrap_or(Value::Null);
        if let Some(model) = self.req.model.clone() {
            if let Err(e) = self.set_option(
                &session_id,
                &options,
                "model",
                std::slice::from_ref(&model),
                &format!("model `{model}`"),
            ) {
                return e;
            }
        }
        if self.req.effort != Effort::Default {
            if let Err(e) = self.set_effort(&session_id, &options) {
                return e;
            }
        }
        if self.req.permission == Tier::ReadOnly {
            self.select_read_only_mode(&session_id, &opened);
        }

        let prompt = match &self.req.output_schema {
            Some(schema) => instruct_json(&self.req.prompt, schema),
            None => self.req.prompt.clone(),
        };
        match self.call(
            "session/prompt",
            json!({"sessionId": session_id, "prompt": [{"type": "text", "text": prompt}]}),
            None,
        ) {
            Ok(r) => End::Stop(r.get("stopReason").and_then(Value::as_str).unwrap_or("end_turn").to_string()),
            Err(e) => self.rpc_failure("session/prompt", e),
        }
    }

    fn rpc_failure(&mut self, method: &str, e: Rpc) -> End {
        match e {
            Rpc::Cancelled => End::Stop("cancelled".into()),
            Rpc::Timeout => End::Fail(reason::PROTOCOL_ERROR, format!("the agent did not answer `{method}` in time")),
            Rpc::Closed => End::Fail(reason::PROTOCOL_ERROR, format!("the agent exited before answering `{method}`")),
            Rpc::Error { code: -32000, message } => {
                End::Fail(reason::AUTH_REQUIRED, format!("{message} (log in with the agent's own login command first)"))
            }
            Rpc::Error { code, message } if common::looks_like_auth_error(&message) => End::Fail(
                reason::AUTH_REQUIRED,
                format!("{message} (code {code}; log in with the agent's own login command first)"),
            ),
            Rpc::Error { code, message } if common::looks_like_rate_limit(&message) => {
                self.c.retryable = true;
                End::Fail(reason::RATE_LIMITED, format!("`{method}` failed: {message} (code {code})"))
            }
            Rpc::Error { code, message } => {
                End::Fail(reason::AGENT_ERROR, format!("`{method}` failed: {message} (code {code})"))
            }
        }
    }

    /// Sets the config option of `category` to the first of `wanted` the agent offers.
    fn set_option(
        &mut self,
        session: &str,
        options: &Value,
        category: &str,
        wanted: &[String],
        what: &str,
    ) -> Result<String, End> {
        let option = options.as_array().into_iter().flatten().find(|o| {
            o.get("category").and_then(Value::as_str) == Some(category)
                || o.get("id").and_then(Value::as_str) == Some(category)
        });
        let Some(option) = option else {
            return Err(End::Fail(
                reason::UNSUPPORTED,
                format!(
                    "the agent advertises no `{category}` option, so {what} cannot be honoured; leave it unset for this profile"
                ),
            ));
        };
        let values = option_values(option);
        let hit = wanted.iter().find_map(|w| {
            values.iter().find(|(value, name)| value.eq_ignore_ascii_case(w) || name.eq_ignore_ascii_case(w))
        });
        let Some((value, _)) = hit else {
            let offered = values.iter().map(|(v, _)| v.as_str()).collect::<Vec<_>>().join(", ");
            return Err(End::Fail(
                reason::UNSUPPORTED,
                format!("the agent does not offer {what}; it accepts: {offered}"),
            ));
        };
        let config_id = option.get("id").and_then(Value::as_str).unwrap_or(category).to_string();
        match self.call(
            "session/set_config_option",
            json!({"sessionId": session, "configId": config_id, "value": value}),
            Some(HANDSHAKE),
        ) {
            Ok(_) => Ok(value.clone()),
            Err(e) => Err(self.rpc_failure("session/set_config_option", e)),
        }
    }

    fn set_effort(&mut self, session: &str, options: &Value) -> Result<(), End> {
        let aliases = |e: &Effort| -> Vec<String> {
            match e {
                Effort::Off => vec!["off".into(), "none".into(), "disabled".into()],
                Effort::Max => vec!["max".into(), "maximum".into(), "xhigh".into()],
                Effort::Native(v) => vec![v.clone()],
                other => vec![other.to_string()],
            }
        };
        let wanted = self.req.effort.clone();
        let what = format!("effort `{wanted}`");
        match self.set_option(session, options, "thought_level", &aliases(&wanted), &what) {
            Ok(_) => Ok(()),
            Err(End::Fail(why, detail))
                if why == reason::UNSUPPORTED && self.profile.effort_fallback == EffortFallback::Nearest =>
            {
                let mut order: Vec<Effort> = Effort::SCALE.to_vec();
                let rank = wanted.rank().unwrap_or(2);
                order.sort_by_key(|e| ((e.rank().unwrap_or(0) - rank).abs(), e.rank()));
                for candidate in order.into_iter().filter(|e| *e != wanted) {
                    if self.set_option(session, options, "thought_level", &aliases(&candidate), &what).is_ok() {
                        self.sink.emit(EventBody::warning(format!(
                            "effort `{wanted}` is not offered by the agent; using nearest level `{candidate}`"
                        )));
                        return Ok(());
                    }
                }
                Err(End::Fail(why, detail))
            }
            Err(e) => Err(e),
        }
    }

    /// For read-only jobs, also select a plan-like mode when the agent has one: a second control
    /// on top of the permission answers.
    fn select_read_only_mode(&mut self, session: &str, opened: &Value) {
        let modes: Vec<String> = opened
            .pointer("/modes/availableModes")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|m| m.get("id").and_then(Value::as_str).map(str::to_string))
            .collect();
        let pick = ["plan", "read-only", "readonly", "ask"]
            .iter()
            .find_map(|want| modes.iter().find(|m| m.to_ascii_lowercase().contains(want)).cloned());
        if let Some(mode) = pick {
            if opened.pointer("/modes/currentModeId").and_then(Value::as_str) == Some(mode.as_str()) {
                return;
            }
            if self.call("session/set_mode", json!({"sessionId": session, "modeId": mode}), Some(HANDSHAKE)).is_err() {
                self.sink.emit(EventBody::warning(format!(
                    "could not switch the agent to its `{mode}` mode; relying on permission answers"
                )));
            }
        }
    }

    fn finish(mut self, end: End) -> RunOutcome {
        self.flush_text();
        self.proc.close_stdin();
        // Give the agent a moment to exit by itself once its stdin is closed.
        let until = Instant::now() + Duration::from_secs(3);
        while Instant::now() < until {
            if let Step::Done = self.proc.next(self.sink, Duration::from_millis(50)) {
                break;
            }
        }
        let mut ended = self.proc.finish(self.cancel);
        // The process is always stopped by Hufu, so its exit code says nothing about the turn.
        ended.exit_code = Some(0);
        ended.stderr_tail = proc::tail(&ended.stderr_tail, 1500);
        let stderr = ended.stderr_tail.clone();
        let login_hint = "log in with the agent's own login command first";
        match end {
            End::Stop(stop) => match stop.as_str() {
                "end_turn" => {
                    ended.cancelled = false;
                    self.c.outcome(self.sink, ended, self.req, true, login_hint)
                }
                "cancelled" => {
                    ended.cancelled = true;
                    self.c.outcome(self.sink, ended, self.req, true, login_hint)
                }
                other => {
                    ended.cancelled = false;
                    let why = match other {
                        "refusal" => reason::REFUSAL,
                        "max_tokens" => reason::MAX_TOKENS,
                        "max_turn_requests" => reason::MAX_TURNS,
                        _ => reason::AGENT_ERROR,
                    };
                    let mut out = self.c.outcome(self.sink, ended, self.req, true, login_hint);
                    out.status = hufu_contract::RunStatus::Failed;
                    out.reason = Some(why.to_string());
                    out.detail = Some(format!("the agent stopped with `{other}`"));
                    out
                }
            },
            End::Fail(why, detail) => {
                ended.cancelled = false;
                let retryable = self.c.retryable;
                let mut out = self.c.outcome(self.sink, ended, self.req, true, login_hint);
                out.status = hufu_contract::RunStatus::Failed;
                out.reason = Some(why.to_string());
                out.detail = Some(if stderr.is_empty() || why == reason::UNSUPPORTED {
                    detail
                } else {
                    format!("{detail}\n{stderr}")
                });
                out.retryable = retryable;
                out.structured_output = None;
                out
            }
        }
    }
}

fn option_values(option: &Value) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for entry in option.get("options").and_then(Value::as_array).into_iter().flatten() {
        if let Some(group) = entry.get("options").and_then(Value::as_array) {
            for inner in group {
                push_value(&mut out, inner);
            }
        } else {
            push_value(&mut out, entry);
        }
    }
    out
}

fn push_value(out: &mut Vec<(String, String)>, entry: &Value) {
    if let Some(value) = entry.get("value").and_then(Value::as_str) {
        let name = entry.get("name").and_then(Value::as_str).unwrap_or(value);
        out.push((value.to_string(), name.to_string()));
    }
}

/// Every path a tool call says it touches: its locations and its diff contents.
fn paths_of(call: &Value) -> Vec<String> {
    let mut out = Vec::new();
    for loc in call.get("locations").and_then(Value::as_array).into_iter().flatten() {
        if let Some(p) = loc.get("path").and_then(Value::as_str) {
            out.push(p.to_string());
        }
    }
    for item in call.get("content").and_then(Value::as_array).into_iter().flatten() {
        if item.get("type").and_then(Value::as_str) == Some("diff") {
            if let Some(p) = item.get("path").and_then(Value::as_str) {
                out.push(p.to_string());
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Resolves `.` and `..` without touching the file system, so that a path which does not exist
/// yet can still be compared with the workspace.
fn normalize(path: &Path, base: &Path) -> PathBuf {
    let joined = if path.is_absolute() { path.to_path_buf() } else { base.join(path) };
    let mut out = PathBuf::new();
    for part in joined.components() {
        match part {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn inside(path: &str, workspace: &Path) -> bool {
    let root = normalize(workspace, workspace);
    let target = normalize(Path::new(path), workspace);
    let root_real = std::fs::canonicalize(&root).unwrap_or(root.clone());
    target.starts_with(&root) || target.starts_with(&root_real)
}

fn command_text(call: &Value) -> Option<String> {
    let raw = call.get("rawInput")?;
    let cmd = raw.get("command").or_else(|| raw.get("cmd")).or_else(|| raw.get("commandLine"))?;
    match cmd {
        Value::String(s) => Some(s.clone()),
        Value::Array(parts) => Some(parts.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" ")),
        _ => None,
    }
}

/// The tier policy: should this tool call be allowed, and why.
pub(crate) fn decide(tier: Tier, profile: &Profile, workspace: &Path, kind: &str, call: &Value) -> (bool, String) {
    if tier == Tier::FullAccess {
        return (true, "full-access allows every request".into());
    }
    if matches!(kind, "read" | "search" | "think") {
        return (true, format!("`{kind}` is allowed at {tier}"));
    }
    if tier == Tier::ReadOnly {
        return (false, format!("`{kind}` is not allowed at read-only"));
    }
    match kind {
        "edit" | "delete" | "move" => {
            let paths = paths_of(call);
            if paths.is_empty() {
                return (false, format!("`{kind}` gave no path, so it cannot be confined to the workspace"));
            }
            match paths.iter().find(|p| !inside(p, workspace)) {
                Some(outside) => (false, format!("`{outside}` is outside the workspace")),
                None => (true, "all paths are inside the workspace".into()),
            }
        }
        "execute" => {
            let Some(command) = command_text(call) else {
                return (
                    false,
                    "the command text was not given, so it cannot be matched against `allow_execute`".into(),
                );
            };
            let trimmed = command.trim();
            // A prefix only vouches for one command, so anything that chains or redirects is refused.
            let chained = ["&&", "||", ";", "|", "`", "$(", ">", "<", "\n"].iter().any(|op| trimmed.contains(op));
            let hit = profile.allow_execute.iter().filter(|p| p.as_str() != "fetch").find(|p| {
                trimmed == p.as_str()
                    || trimmed.strip_prefix(p.as_str()).map(|rest| rest.starts_with(' ')).unwrap_or(false)
            });
            match (hit, chained) {
                (Some(prefix), false) => (true, format!("matches `allow_execute` entry `{prefix}`")),
                (Some(_), true) => {
                    (false, "the command chains or redirects, which `allow_execute` prefixes do not cover".into())
                }
                (None, _) => (false, "the command is not in the profile's `allow_execute` list".into()),
            }
        }
        "fetch" => {
            if profile.allow_execute.iter().any(|p| p == "fetch") {
                (true, "`fetch` is listed in `allow_execute`".into())
            } else {
                (false, "network fetches need the entry `fetch` in the profile's `allow_execute` list".into())
            }
        }
        other => (false, format!("`{other}` is not allowed at workspace-write")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::testing::*;

    #[test]
    fn unverified_profiles_only_offer_full_access() {
        let mut p = profile("acp");
        p.command = vec!["kimi".into(), "acp".into()];
        assert_eq!(Acp.capabilities(&p).permission_tiers, [Tier::FullAccess]);
        assert!(Acp.plan(&p, &request(Tier::ReadOnly)).unwrap_err().0.contains("unverified"));
        p.permission_requests = PermissionRequests::Verified;
        assert_eq!(Acp.capabilities(&p).permission_tiers.len(), 3);
        let plan = Acp.plan(&p, &request(Tier::ReadOnly)).unwrap();
        assert_eq!((plan.program.as_str(), plan.args.as_slice()), ("kimi", &["acp".to_string()][..]));
        p.command.clear();
        assert!(Acp.plan(&p, &request(Tier::FullAccess)).is_err());
    }

    #[test]
    fn tier_policy_matches_the_contract() {
        let ws = std::env::temp_dir().join("hufu-acp-ws");
        let mut p = profile("acp");
        let none = Value::Null;
        assert!(decide(Tier::FullAccess, &p, &ws, "execute", &none).0);
        assert!(decide(Tier::ReadOnly, &p, &ws, "read", &none).0);
        assert!(decide(Tier::ReadOnly, &p, &ws, "search", &none).0);
        assert!(!decide(Tier::ReadOnly, &p, &ws, "edit", &none).0);
        assert!(!decide(Tier::ReadOnly, &p, &ws, "fetch", &none).0);

        let inside_edit = json!({"locations": [{"path": ws.join("src/a.rs")}]});
        let outside_edit = json!({"locations": [{"path": ws.join("../elsewhere/a.rs")}]});
        assert!(decide(Tier::WorkspaceWrite, &p, &ws, "edit", &inside_edit).0);
        assert!(!decide(Tier::WorkspaceWrite, &p, &ws, "edit", &outside_edit).0);
        assert!(!decide(Tier::WorkspaceWrite, &p, &ws, "edit", &none).0, "an edit with no path cannot be confined");

        let test_cmd = json!({"rawInput": {"command": "npm test -- auth"}});
        assert!(!decide(Tier::WorkspaceWrite, &p, &ws, "execute", &test_cmd).0);
        p.allow_execute = vec!["npm test".into()];
        assert!(decide(Tier::WorkspaceWrite, &p, &ws, "execute", &test_cmd).0);
        let sneaky = json!({"rawInput": {"command": "npm test && rm -rf ~"}});
        assert!(!decide(Tier::WorkspaceWrite, &p, &ws, "execute", &sneaky).0);
        let lookalike = json!({"rawInput": {"command": "npm testify"}});
        assert!(!decide(Tier::WorkspaceWrite, &p, &ws, "execute", &lookalike).0);
        assert!(!decide(Tier::WorkspaceWrite, &p, &ws, "fetch", &none).0);
        p.allow_execute.push("fetch".into());
        assert!(decide(Tier::WorkspaceWrite, &p, &ws, "fetch", &none).0);
    }

    #[test]
    fn config_option_values_include_groups() {
        let opt = json!({"id": "model", "options": [{"value": "a", "name": "A"}, {"group": "g", "name": "G", "options": [{"value": "b", "name": "B"}]}]});
        assert_eq!(option_values(&opt), [("a".to_string(), "A".to_string()), ("b".to_string(), "B".to_string())]);
    }
}
