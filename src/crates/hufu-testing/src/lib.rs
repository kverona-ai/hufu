//! Test doubles for Hufu: fake agents that speak each supported agent's output format, and the
//! adapter conformance suite.
//!
//! The fakes are reachable as `hufu __fake-agent <flavor> ...`, so that tests on every platform can
//! point a profile's `command` at the binary under test, and `hufu doctor --self-test` can exercise
//! the whole pipeline without any real agent installed.
//!
//! What a fake does is chosen by a directive in the prompt, `[[fake:<scenario>]]` or
//! `[[fake:<scenario>=<argument>]]`, or by the `HUFU_FAKE_SCENARIO` variable. Scenarios:
//! `ok` (default), `write[=file]`, `deny`, `json[=text]`, `reply=text`, `env=VAR`, `args`,
//! `slow[=ms]`, `hang`, `silent`, `crash`, `empty`, `malformed`, `auth`, `rate-limit`,
//! `permission` and `refuse` (ACP only).

pub mod conformance;

use std::io::{BufRead, Read, Write};
use std::path::PathBuf;
use std::time::Duration;

use serde_json::{Value, json};

pub const FLAVORS: &[&str] = &["codex", "agy", "cursor", "kimi", "pi", "claude", "acp", "external", "lcu"];

/// The marker lines Hufu's mission driver puts into advisor prompts. The fakes key on them.
pub const PLAN_MARKER: &str = "hufu:mission-plan";
pub const REVIEW_MARKER: &str = "hufu:mission-review";

fn out(line: &str) {
    let stdout = std::io::stdout();
    let mut lock = stdout.lock();
    let _ = writeln!(lock, "{line}");
    let _ = lock.flush();
}

fn emit(v: Value) {
    out(&v.to_string());
}

fn sleep_forever() -> ! {
    loop {
        std::thread::sleep(Duration::from_secs(3600));
    }
}

fn read_stdin() -> String {
    let mut s = String::new();
    let _ = std::io::stdin().read_to_string(&mut s);
    s
}

fn flag(args: &[String], names: &[&str]) -> Option<String> {
    args.iter().position(|a| names.contains(&a.as_str())).and_then(|i| args.get(i + 1)).cloned()
}

fn directive(prompt: &str, key: &str) -> Option<String> {
    let open = format!("[[{key}");
    let start = prompt.find(&open)? + open.len();
    let rest = &prompt[start..];
    let end = rest.find("]]")?;
    let body = rest[..end].trim();
    Some(body.strip_prefix([':', '=']).unwrap_or(body).trim().to_string())
}

/// What a fake was asked to do.
#[derive(Clone, Debug)]
pub struct Script {
    pub flavor: String,
    pub scenario: String,
    pub arg: Option<String>,
    pub prompt: String,
    pub cwd: PathBuf,
    pub resumed: Option<String>,
    pub argv: Vec<String>,
}

impl Script {
    fn new(flavor: &str, prompt: String, cwd: Option<String>, resumed: Option<String>, argv: &[String]) -> Script {
        let chosen = directive(&prompt, "fake").or_else(|| std::env::var("HUFU_FAKE_SCENARIO").ok().filter(|s| !s.is_empty())).unwrap_or_else(|| "ok".into());
        let (scenario, arg) = match chosen.split_once('=') {
            Some((s, a)) => (s.trim().to_string(), Some(a.trim().to_string())),
            None => (chosen.trim().to_string(), None),
        };
        Script {
            flavor: flavor.to_string(),
            scenario,
            arg,
            prompt,
            cwd: cwd.map(PathBuf::from).unwrap_or_else(|| std::env::current_dir().unwrap_or_default()),
            resumed,
            argv: argv.to_vec(),
        }
    }

    /// The final answer for scenarios that produce one.
    fn answer(&self) -> String {
        if self.prompt.contains(PLAN_MARKER) {
            if let Some(plan) = directive(&self.prompt, "plan") {
                return plan;
            }
            let agent = self.prompt.lines().find_map(|l| l.trim().strip_prefix("- `").and_then(|r| r.split('`').next())).unwrap_or("codex").to_string();
            return json!({"v": 1, "summary": "one step", "tasks": [{"id": "t1", "title": "Do the work", "prompt": "Carry out the goal.", "agent": agent}]}).to_string();
        }
        if self.prompt.contains(REVIEW_MARKER) {
            let wanted = directive(&self.prompt, "review").unwrap_or_else(|| "accept".into());
            let round: u32 = self.prompt.lines().find_map(|l| l.trim().strip_prefix("Round: ").and_then(|r| r.trim().parse().ok())).unwrap_or(1);
            let verdict = match wanted.as_str() {
                "revise-then-accept" => {
                    if round <= 1 {
                        "revise"
                    } else {
                        "accept"
                    }
                }
                other => other,
            };
            return json!({"v": 1, "verdict": verdict, "reason": format!("fake review, round {round}"), "follow_up": "Please address the review."}).to_string();
        }
        let clip: String = self.prompt.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(80).collect();
        let base = match self.scenario.as_str() {
            "json" => self.arg.clone().unwrap_or_else(|| json!({"ok": true, "summary": "fake"}).to_string()),
            "reply" => self.arg.clone().unwrap_or_default(),
            "env" => {
                let var = self.arg.clone().unwrap_or_default();
                format!("{var}={}", std::env::var(&var).unwrap_or_else(|_| "<unset>".into()))
            }
            "args" => serde_json::to_string(&self.argv).unwrap_or_default(),
            _ => format!("fake {} ok: {clip}", self.flavor),
        };
        match (&self.resumed, self.scenario.as_str()) {
            (Some(id), "ok") => format!("{base} (resumed {id})"),
            _ => base,
        }
    }
}

/// How one flavor prints what happens.
trait Voice {
    fn start(&mut self, session: &str);
    fn tool(&mut self, id: &str, kind: &str, title: &str, denied: bool);
    fn say(&mut self, text: &str);
    /// Ends a successful turn.
    fn finish(&mut self, text: Option<&str>);
    /// Prints this agent's "log in first" message and returns the exit code.
    fn auth_error(&mut self) -> i32;
}

fn perform(script: &Script, voice: &mut dyn Voice) -> i32 {
    let session = script.resumed.clone().unwrap_or_else(|| "fake-session-1".to_string());
    match script.scenario.as_str() {
        "auth" => return voice.auth_error(),
        "rate-limit" => {
            eprintln!("429 Too Many Requests: rate limit exceeded, retry later");
            return 1;
        }
        "silent" => sleep_forever(),
        "malformed" => {
            out("this is not json");
            out("{\"unterminated\": ");
            return 0;
        }
        _ => {}
    }
    voice.start(&session);
    match script.scenario.as_str() {
        "hang" => sleep_forever(),
        "crash" => {
            eprintln!("fatal: simulated crash");
            return 2;
        }
        "empty" => {
            voice.finish(None);
            return 0;
        }
        "slow" => std::thread::sleep(Duration::from_millis(script.arg.as_deref().and_then(|a| a.parse().ok()).unwrap_or(1500))),
        _ => {}
    }
    voice.tool("t1", "read", "README.md", false);
    if script.scenario == "write" {
        let name = script.arg.clone().unwrap_or_else(|| "fake-output.txt".into());
        let path = script.cwd.join(&name);
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&path, format!("written by fake {}\n", script.flavor));
        voice.tool("t2", "edit", &name, false);
    }
    if script.scenario == "deny" {
        voice.tool("t3", "execute", "npm test", true);
    }
    let text = script.answer();
    voice.say(&text);
    voice.finish(Some(&text));
    0
}

// ---------------------------------------------------------------------------------------------
// Flavors
// ---------------------------------------------------------------------------------------------

struct Codex {
    last_message: Option<String>,
}
impl Voice for Codex {
    fn start(&mut self, session: &str) {
        emit(json!({"type": "thread.started", "thread_id": session}));
        emit(json!({"type": "turn.started"}));
    }
    fn tool(&mut self, id: &str, kind: &str, title: &str, denied: bool) {
        if kind == "edit" {
            emit(json!({"type": "item.completed", "item": {"id": id, "type": "file_change", "changes": [{"path": title, "kind": "add"}], "status": "completed"}}));
        } else {
            let status = if denied { "declined" } else { "completed" };
            emit(json!({"type": "item.completed", "item": {"id": id, "type": "command_execution", "command": title, "aggregated_output": "", "exit_code": if denied { Value::Null } else { json!(0) }, "status": status}}));
        }
    }
    fn say(&mut self, text: &str) {
        emit(json!({"type": "item.completed", "item": {"id": "msg", "type": "agent_message", "text": text}}));
    }
    fn finish(&mut self, text: Option<&str>) {
        if let (Some(path), Some(text)) = (&self.last_message, text) {
            let _ = std::fs::write(path, text);
        }
        emit(json!({"type": "turn.completed", "usage": {"input_tokens": 120, "cached_input_tokens": 20, "output_tokens": 30, "reasoning_output_tokens": 5}}));
    }
    fn auth_error(&mut self) -> i32 {
        emit(json!({"type": "error", "message": "Not logged in. Run `codex login` first."}));
        eprintln!("Not logged in. Run `codex login` first.");
        1
    }
}

struct Agy {
    session: String,
    step: u32,
}
impl Voice for Agy {
    fn start(&mut self, session: &str) {
        self.session = session.to_string();
        emit(json!({"event": "init", "conversation_id": session, "init": {"cwd": ".", "tools": ["run_command"], "permission_mode": "request-review"}}));
    }
    fn tool(&mut self, _id: &str, kind: &str, title: &str, denied: bool) {
        self.step += 1;
        let name = match kind {
            "edit" => "write_file",
            "execute" => "run_command",
            _ => "read_file",
        };
        let mut info = json!({"name": name, "parameters": {"CommandLine": title, "TargetFile": title}});
        if denied {
            info["error"] = json!({"type": "permission_denied", "message": "command was denied"});
        }
        emit(json!({"event": "step_update", "step_update": {"step_index": self.step, "state": "DONE", "step_type": "tool", "tool_name": name, "tool_info": info}}));
    }
    fn say(&mut self, text: &str) {
        self.step += 1;
        emit(json!({"event": "step_update", "step_update": {"step_index": self.step, "state": "ACTIVE", "step_type": "agent_response", "text_delta": text}}));
        emit(json!({"event": "step_update", "step_update": {"step_index": self.step, "state": "DONE", "step_type": "agent_response"}}));
    }
    fn finish(&mut self, text: Option<&str>) {
        emit(json!({"event": "result", "result": {"conversation_id": self.session, "status": "SUCCESS", "response": text.unwrap_or(""), "num_turns": 1,
            "usage": {"input_tokens": 100, "output_tokens": 20, "thinking_tokens": 5, "cache_read_tokens": 0, "total_tokens": 125}}}));
    }
    fn auth_error(&mut self) -> i32 {
        emit(json!({"event": "result", "result": {"conversation_id": "", "status": "ERROR", "response": "", "error": "authentication required: please sign in"}}));
        1
    }
}

struct Cursor {
    session: String,
}
impl Voice for Cursor {
    fn start(&mut self, session: &str) {
        self.session = session.to_string();
        emit(json!({"type": "system", "subtype": "init", "apiKeySource": "login", "cwd": ".", "session_id": session, "model": "fake", "permissionMode": "default"}));
    }
    fn tool(&mut self, id: &str, kind: &str, title: &str, denied: bool) {
        let call = match (kind, denied) {
            ("edit", _) => json!({"writeToolCall": {"args": {"path": title, "fileText": "x"}, "result": {"success": {"path": title, "linesCreated": 1, "fileSize": 1}}}}),
            (_, true) => json!({"function": {"name": "shell", "arguments": json!({"command": title}).to_string(), "result": {"rejected": {"reason": "not in allowlist"}}}}),
            _ => json!({"readToolCall": {"args": {"path": title}, "result": {"success": {"content": "x"}}}}),
        };
        emit(json!({"type": "tool_call", "subtype": "completed", "call_id": id, "tool_call": call, "session_id": self.session}));
    }
    fn say(&mut self, text: &str) {
        emit(json!({"type": "assistant", "message": {"role": "assistant", "content": [{"type": "text", "text": text}]}, "session_id": self.session}));
    }
    fn finish(&mut self, text: Option<&str>) {
        emit(json!({"type": "result", "subtype": "success", "duration_ms": 10, "is_error": false, "result": text.unwrap_or(""), "session_id": self.session}));
    }
    fn auth_error(&mut self) -> i32 {
        eprintln!("Error: Not logged in. Please run `agent login`.");
        1
    }
}

struct Kimi;
impl Voice for Kimi {
    fn start(&mut self, _session: &str) {}
    fn tool(&mut self, id: &str, _kind: &str, title: &str, denied: bool) {
        emit(json!({"role": "assistant", "content": "", "tool_calls": [{"type": "function", "id": id, "function": {"name": "Shell", "arguments": json!({"command": title}).to_string()}}]}));
        let result = if denied { "Error: permission denied by user" } else { "ok" };
        emit(json!({"role": "tool", "tool_call_id": id, "content": result}));
    }
    fn say(&mut self, text: &str) {
        emit(json!({"role": "assistant", "content": text}));
    }
    fn finish(&mut self, _text: Option<&str>) {}
    fn auth_error(&mut self) -> i32 {
        eprintln!("Error: not logged in. Please run `kimi login`.");
        1
    }
}

struct Pi;
impl Voice for Pi {
    fn start(&mut self, session: &str) {
        emit(json!({"type": "session", "version": 3, "id": session, "timestamp": "2026-01-01T00:00:00.000Z", "cwd": "."}));
        emit(json!({"type": "agent_start"}));
    }
    fn tool(&mut self, id: &str, kind: &str, title: &str, denied: bool) {
        let name = match kind {
            "edit" => "write",
            "execute" => "bash",
            _ => "read",
        };
        emit(json!({"type": "tool_execution_start", "toolCallId": id, "toolName": name, "args": {"command": title, "path": title}}));
        let text = if denied { "permission denied: blocked by policy" } else { "ok" };
        emit(json!({"type": "tool_execution_end", "toolCallId": id, "toolName": name, "result": {"content": [{"type": "text", "text": text}]}, "isError": denied, "durationMs": 1}));
    }
    fn say(&mut self, text: &str) {
        emit(json!({"type": "message_update", "assistantMessageEvent": {"type": "text_delta", "contentIndex": 0, "delta": text},
            "usage": {"input": 100, "output": 20, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 120, "cost": {"total": 0.01}}}));
        emit(json!({"type": "message_end", "message": {"role": "assistant", "content": [{"type": "text", "text": text}], "stopReason": "stop"}}));
    }
    fn finish(&mut self, _text: Option<&str>) {
        emit(json!({"type": "agent_settled", "aborted": false}));
    }
    fn auth_error(&mut self) -> i32 {
        eprintln!("Error: No API key found for provider. Authentication required: use /login or set the provider's API key.");
        1
    }
}

struct Claude {
    session: String,
    denied: Vec<Value>,
}
impl Voice for Claude {
    fn start(&mut self, session: &str) {
        self.session = session.to_string();
        emit(json!({"type": "system", "subtype": "init", "session_id": session, "model": "fake", "tools": ["Read", "Edit", "Bash"]}));
    }
    fn tool(&mut self, id: &str, kind: &str, title: &str, denied: bool) {
        let (name, input) = match kind {
            "edit" => ("Write", json!({"file_path": title})),
            "execute" => ("Bash", json!({"command": title})),
            _ => ("Read", json!({"file_path": title})),
        };
        emit(json!({"type": "assistant", "message": {"role": "assistant", "content": [{"type": "tool_use", "id": id, "name": name, "input": input}]}, "session_id": self.session}));
        emit(json!({"type": "user", "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": id, "is_error": denied}]}, "session_id": self.session}));
        if denied {
            self.denied.push(json!({"tool_name": name, "tool_use_id": id, "tool_input": input}));
        }
    }
    fn say(&mut self, text: &str) {
        emit(json!({"type": "assistant", "message": {"role": "assistant", "content": [{"type": "text", "text": text}]}, "session_id": self.session}));
    }
    fn finish(&mut self, text: Option<&str>) {
        emit(json!({"type": "result", "subtype": "success", "is_error": false, "result": text.unwrap_or(""), "session_id": self.session, "total_cost_usd": 0.02,
            "usage": {"input_tokens": 50, "output_tokens": 70, "cache_read_input_tokens": 3}, "permission_denials": self.denied}));
    }
    fn auth_error(&mut self) -> i32 {
        emit(json!({"type": "result", "subtype": "success", "is_error": true, "result": "Not logged in · Please run /login", "session_id": "s"}));
        1
    }
}

/// The external adapter protocol: the fake is the adapter program itself.
struct External {
    session: String,
    denied: Vec<Value>,
    touched: Vec<String>,
}
impl Voice for External {
    fn start(&mut self, session: &str) {
        self.session = session.to_string();
    }
    fn tool(&mut self, id: &str, kind: &str, title: &str, denied: bool) {
        emit(json!({"type": "tool.call", "id": id, "kind": kind, "title": title, "status": if denied { "failed" } else { "completed" }}));
        if denied {
            emit(json!({"type": "permission.denied", "tool": title, "kind": kind}));
            self.denied.push(json!({"tool": title, "kind": kind}));
        } else if kind == "edit" {
            self.touched.push(title.to_string());
        }
    }
    fn say(&mut self, text: &str) {
        emit(json!({"type": "agent.message", "text": text, "final": true}));
    }
    fn finish(&mut self, text: Option<&str>) {
        let structured = text.and_then(|t| serde_json::from_str::<Value>(t).ok());
        emit(json!({"type": "outcome", "status": "succeeded", "final_message": text, "structured_output": structured, "denied_actions": self.denied,
            "session": {"id": self.session, "resumable": true}, "usage": {"input_tokens": 10, "output_tokens": 5}, "touched_paths": self.touched}));
    }
    fn auth_error(&mut self) -> i32 {
        emit(json!({"type": "outcome", "status": "failed", "reason": "auth_required", "detail": "not logged in (run the agent's login command)"}));
        0
    }
}

fn external(args: &[String]) -> i32 {
    let input: Value = serde_json::from_str(&read_stdin()).unwrap_or(Value::Null);
    match args.first().map(String::as_str) {
        Some("probe") => {
            emit(json!({"installed": true, "version": "fake-external 0.0.1", "authenticated": true}));
            0
        }
        Some("capabilities") => {
            emit(json!({"permission_tiers": ["read-only", "workspace-write", "full-access"], "efforts": ["low", "high"], "native_effort": false, "checked_at_runtime": false,
                "models": [], "resume": true, "structured_output": "instructed", "reports_usage": true, "reports_denials": true, "accepts_mcp_servers": false, "provides": [], "notes": []}));
            0
        }
        Some("run") => {
            let req = &input["request"];
            let script = Script::new(
                "external",
                req["prompt"].as_str().unwrap_or("").to_string(),
                req["cwd"].as_str().map(str::to_string),
                req["resume_session"].as_str().map(str::to_string),
                args,
            );
            perform(&script, &mut External { session: String::new(), denied: Vec::new(), touched: Vec::new() })
        }
        _ => 2,
    }
}

// ---------------------------------------------------------------------------------------------
// ACP
// ---------------------------------------------------------------------------------------------

fn acp(argv: &[String]) -> i32 {
    let stdin = std::io::stdin();
    let mut lines = stdin.lock().lines();
    let mut cwd: Option<String> = None;
    let mut resumed: Option<String> = None;
    let mut next_id = 1000u64;
    let reply = |id: &Value, result: Value| emit(json!({"jsonrpc": "2.0", "id": id, "result": result}));
    let update = |session: &str, u: Value| emit(json!({"jsonrpc": "2.0", "method": "session/update", "params": {"sessionId": session, "update": u}}));
    while let Some(Ok(line)) = lines.next() {
        let Ok(msg) = serde_json::from_str::<Value>(&line) else { continue };
        let id = msg.get("id").cloned().unwrap_or(Value::Null);
        let params = msg.get("params").cloned().unwrap_or(Value::Null);
        match msg.get("method").and_then(Value::as_str).unwrap_or("") {
            "initialize" => reply(
                &id,
                json!({"protocolVersion": 1, "agentCapabilities": {"loadSession": true, "sessionCapabilities": {"resume": {}}},
                    "agentInfo": {"name": "fake-acp", "version": "0.0.1"}, "authMethods": []}),
            ),
            "authenticate" => reply(&id, json!({})),
            "session/new" | "session/load" | "session/resume" => {
                cwd = params["cwd"].as_str().map(str::to_string);
                if std::env::var("HUFU_FAKE_SCENARIO").as_deref() == Ok("auth") {
                    emit(json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32000, "message": "Authentication required"}}));
                    continue;
                }
                let session = params["sessionId"].as_str().map(str::to_string);
                resumed = session.clone();
                reply(
                    &id,
                    json!({"sessionId": session.unwrap_or_else(|| "fake-acp-session".into()),
                        "configOptions": [
                            {"id": "model", "category": "model", "currentValue": "fake-small", "options": [{"value": "fake-small", "name": "Small"}, {"value": "fake-large", "name": "Large"}]},
                            {"id": "effort", "category": "thought_level", "currentValue": "medium", "options": [{"value": "low", "name": "Low"}, {"value": "medium", "name": "Medium"}, {"value": "high", "name": "High"}]}
                        ],
                        "modes": {"currentModeId": "agent", "availableModes": [{"id": "agent", "name": "Agent"}, {"id": "plan", "name": "Plan"}]}}),
                );
            }
            "session/set_config_option" | "session/set_mode" => reply(&id, json!({})),
            "session/prompt" => {
                let session = params["sessionId"].as_str().unwrap_or("fake-acp-session").to_string();
                let prompt: String = params["prompt"].as_array().into_iter().flatten().filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join("\n");
                let script = Script::new("acp", prompt, cwd.clone(), resumed.clone(), argv);
                match script.scenario.as_str() {
                    "hang" | "silent" => {
                        // Wait for session/cancel, as a well-behaved agent does.
                        for line in lines.by_ref() {
                            let Ok(line) = line else { break };
                            if line.contains("session/cancel") {
                                reply(&id, json!({"stopReason": "cancelled"}));
                                break;
                            }
                        }
                        continue;
                    }
                    "crash" => return 2,
                    "malformed" => {
                        out("this is not json");
                        out("{\"unterminated\": ");
                        return 0;
                    }
                    "empty" => {
                        reply(&id, json!({"stopReason": "end_turn"}));
                        continue;
                    }
                    "rate-limit" => {
                        emit(json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32603, "message": "429 Too Many Requests: rate limit exceeded"}}));
                        continue;
                    }
                    "refuse" => {
                        reply(&id, json!({"stopReason": "refusal"}));
                        continue;
                    }
                    "slow" => std::thread::sleep(Duration::from_millis(script.arg.as_deref().and_then(|a| a.parse().ok()).unwrap_or(1500))),
                    _ => {}
                }
                update(&session, json!({"sessionUpdate": "agent_thought_chunk", "content": {"type": "text", "text": "thinking"}}));
                update(&session, json!({"sessionUpdate": "tool_call", "toolCallId": "t1", "title": "README.md", "kind": "read", "status": "completed"}));
                // Ask before acting, the way a real agent does: once for an edit, once for a command.
                let mut ask = |call: Value, lines: &mut std::io::Lines<std::io::StdinLock<'_>>| -> bool {
                    next_id += 1;
                    emit(json!({"jsonrpc": "2.0", "id": next_id, "method": "session/request_permission", "params": {"sessionId": session, "toolCall": call,
                        "options": [{"optionId": "yes", "name": "Allow", "kind": "allow_once"}, {"optionId": "no", "name": "Reject", "kind": "reject_once"}]}}));
                    for line in lines.by_ref() {
                        let Ok(line) = line else { return false };
                        let Ok(answer) = serde_json::from_str::<Value>(&line) else { continue };
                        if answer["id"].as_u64() == Some(next_id) {
                            return answer["result"]["outcome"]["optionId"].as_str() == Some("yes");
                        }
                    }
                    false
                };
                if matches!(script.scenario.as_str(), "write" | "permission") {
                    let name = script.arg.clone().unwrap_or_else(|| "fake-output.txt".into());
                    let path = script.cwd.join(&name);
                    let call = json!({"toolCallId": "t2", "title": format!("Write {name}"), "kind": "edit", "status": "pending", "locations": [{"path": path}]});
                    update(&session, json!({"sessionUpdate": "tool_call", "toolCallId": "t2", "title": format!("Write {name}"), "kind": "edit", "status": "pending", "locations": [{"path": path}]}));
                    if ask(call, &mut lines) {
                        let _ = std::fs::write(&path, "written by fake acp\n");
                        update(&session, json!({"sessionUpdate": "tool_call_update", "toolCallId": "t2", "status": "completed", "locations": [{"path": path}]}));
                    } else {
                        update(&session, json!({"sessionUpdate": "tool_call_update", "toolCallId": "t2", "status": "failed"}));
                    }
                }
                if matches!(script.scenario.as_str(), "deny" | "permission") {
                    let call = json!({"toolCallId": "t3", "title": "npm test", "kind": "execute", "status": "pending", "rawInput": {"command": "npm test"}});
                    update(&session, json!({"sessionUpdate": "tool_call", "toolCallId": "t3", "title": "npm test", "kind": "execute", "status": "pending", "rawInput": {"command": "npm test"}}));
                    let allowed = ask(call, &mut lines);
                    update(&session, json!({"sessionUpdate": "tool_call_update", "toolCallId": "t3", "status": if allowed { "completed" } else { "failed" }}));
                }
                let text = script.answer();
                let (a, b) = text.split_at(text.char_indices().nth(text.chars().count() / 2).map(|(i, _)| i).unwrap_or(0));
                update(&session, json!({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": a}}));
                update(&session, json!({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": b}}));
                update(&session, json!({"sessionUpdate": "usage_update", "used": 321, "size": 100000, "cost": {"amount": 0.03, "currency": "USD"}}));
                reply(&id, json!({"stopReason": "end_turn"}));
            }
            "session/cancel" => {}
            "" => {}
            other => {
                if !id.is_null() {
                    emit(json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": format!("method not found: {other}")}}));
                }
            }
        }
    }
    0
}

// ---------------------------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------------------------

/// Runs a fake agent. `args` are the arguments after `__fake-agent`. Returns the exit code.
pub fn fake_agent_main(args: &[String]) -> i32 {
    let Some(flavor) = args.first().map(String::as_str) else {
        eprintln!("usage: hufu __fake-agent <{}> [agent arguments]", FLAVORS.join("|"));
        return 2;
    };
    let rest = &args[1..];
    if rest.iter().any(|a| a == "--version") {
        out(&format!("fake-{flavor} 0.0.1"));
        return 0;
    }
    match flavor {
        "lcu" => {
            // `lcu apps`: the applications the runtime already allows.
            if rest.first().map(String::as_str) == Some("apps") && rest.len() == 1 {
                for app in std::env::var("HUFU_FAKE_LCU_APPS").unwrap_or_else(|_| "Figma,Notes".into()).split(',') {
                    out(app.trim());
                }
                return 0;
            }
            2
        }
        "external" => external(rest),
        "acp" => acp(rest),
        "codex" => {
            if rest.first().map(String::as_str) != Some("exec") {
                return 0; // `login status` and friends
            }
            let resumed = rest.iter().position(|a| a == "resume").and_then(|i| rest.get(i + 1)).cloned();
            let script = Script::new("codex", read_stdin(), flag(rest, &["--cd", "-C"]), resumed, rest);
            perform(&script, &mut Codex { last_message: flag(rest, &["-o", "--output-last-message"]) })
        }
        "agy" => {
            let Some(prompt) = flag(rest, &["-p"]) else { return 0 };
            let script = Script::new("agy", prompt, None, flag(rest, &["--conversation"]), rest);
            perform(&script, &mut Agy { session: String::new(), step: 0 })
        }
        "cursor" => {
            if rest.first().map(String::as_str) == Some("status") {
                out("Logged in as fake@example.invalid");
                return 0;
            }
            let prompt = rest.last().cloned().unwrap_or_default();
            let script = Script::new("cursor", prompt, flag(rest, &["--workspace"]), flag(rest, &["--resume"]), rest);
            perform(&script, &mut Cursor { session: String::new() })
        }
        "kimi" => {
            let Some(prompt) = flag(rest, &["-p"]) else { return 0 };
            let script = Script::new("kimi", prompt, None, None, rest);
            perform(&script, &mut Kimi)
        }
        "pi" => {
            if rest.first().map(String::as_str) == Some("auth") {
                return 0;
            }
            let prompt = rest.last().cloned().unwrap_or_default();
            let script = Script::new("pi", prompt, None, flag(rest, &["--session"]), rest);
            perform(&script, &mut Pi)
        }
        "claude" => {
            if rest.first().map(String::as_str) == Some("auth") {
                return if std::env::var("HUFU_FAKE_SCENARIO").as_deref() == Ok("auth") { 1 } else { 0 };
            }
            if rest.first().map(String::as_str) == Some("mcp") {
                return 0;
            }
            let script = Script::new("claude", read_stdin(), None, flag(rest, &["--resume"]), rest);
            perform(&script, &mut Claude { session: String::new(), denied: Vec::new() })
        }
        other => {
            eprintln!("unknown fake agent `{other}`; accepted: {}", FLAVORS.join(", "));
            2
        }
    }
}

/// The adapter kind that drives a fake flavor.
pub fn adapter_of(flavor: &str) -> &'static str {
    match flavor {
        "codex" => "codex-cli",
        "agy" => "agy-cli",
        "cursor" => "cursor-cli",
        "kimi" => "kimi-cli",
        "pi" => "pi",
        "claude" => "claude-cli",
        "acp" => "acp",
        _ => "external",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directives_are_read_from_the_prompt() {
        assert_eq!(directive("do it [[fake:write=a.txt]] now", "fake").as_deref(), Some("write=a.txt"));
        assert_eq!(directive("x [[review: revise]]", "review").as_deref(), Some("revise"));
        assert_eq!(directive("nothing here", "fake"), None);
        let s = Script::new("codex", "hello [[fake:reply=custom text]]".into(), None, None, &[]);
        assert_eq!((s.scenario.as_str(), s.answer().as_str()), ("reply", "custom text"));
        let plan = Script::new("codex", format!("<!-- {PLAN_MARKER} -->\n- `worker` does things\n"), None, None, &[]).answer();
        assert!(plan.contains("\"agent\":\"worker\""), "{plan}");
        let review = Script::new("codex", format!("<!-- {REVIEW_MARKER} -->\nRound: 2\n[[review: revise-then-accept]]"), None, None, &[]).answer();
        assert!(review.contains("\"verdict\":\"accept\""), "{review}");
    }
}
