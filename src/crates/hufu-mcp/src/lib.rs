//! Hufu's MCP server: the same operations as the command line, as tools any MCP host can call.
//!
//! Transport is stdio, one JSON-RPC message per line. The server speaks the handshake revisions
//! of MCP (`initialize`, then `notifications/initialized`) and also answers hosts that use the
//! handshake-free revision, where each request carries its protocol version in `_meta`.
//!
//! Started as `hufu mcp --child-of <job>` it serves a single tool, `computer_use`, to the executor
//! of that job: the hand-over to the agent configured for desktop control.

mod tools;

use std::io::{BufRead, Write};
use std::path::PathBuf;

use hufu_contract::AdapterRegistry;
use hufu_core::config::Home;
use serde_json::{Value, json};

/// Revisions with the `initialize` handshake, newest first.
pub const HANDSHAKE_VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];
/// The revision without a handshake.
pub const STATELESS_VERSION: &str = "2026-07-28";
const VERSION_META: &str = "io.modelcontextprotocol/protocolVersion";

pub struct Server<'a> {
    pub home: Home,
    pub adapters: &'a dyn AdapterRegistry,
    pub version: &'a str,
    /// Set when serving the hand-over tool to the executor of this job.
    pub child_of: Option<String>,
    /// The directory relative paths in tool calls are resolved against.
    pub cwd: PathBuf,
}

const INSTRUCTIONS: &str = "Hufu dispatches work to other coding agents as background jobs. Call hufu_agents to see the available profiles and pools, \
hufu_submit to start a job (it returns at once), hufu_wait or hufu_status to follow it, and hufu_result to read the outcome. Read both `state` (did the run finish) \
and `verdict` (did it pass its acceptance checks). Ask for the lowest permission tier that works. hufu_doctor, hufu_profile_set, hufu_pool_set and hufu_config_set \
inspect and change Hufu's configuration; every change is validated before it is written.";

fn error(id: &Value, code: i64, message: impl Into<String>) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message.into()}})
}

impl Server<'_> {
    fn info(&self) -> Value {
        json!({"name": "hufu", "title": "Hufu", "version": self.version})
    }

    fn capabilities(&self) -> Value {
        json!({"tools": {"listChanged": false}})
    }

    /// Handles one message. Notifications produce no answer.
    pub fn handle(&self, msg: &Value) -> Option<Value> {
        let id = msg.get("id").cloned().unwrap_or(Value::Null);
        let Some(method) = msg.get("method").and_then(Value::as_str) else {
            // A response to something we never asked, or garbage with an id.
            return (!id.is_null() && msg.get("result").is_none() && msg.get("error").is_none())
                .then(|| error(&id, -32600, "invalid request: no method"));
        };
        if id.is_null() {
            return None;
        }
        let params = msg.get("params").cloned().unwrap_or(json!({}));
        // Hosts on the handshake-free revision name their version on every request.
        let stated = params
            .pointer(&format!("/_meta/{}", VERSION_META.replace('/', "~1")))
            .and_then(Value::as_str)
            .map(str::to_string);
        if let Some(v) = &stated
            && v != STATELESS_VERSION
            && !HANDSHAKE_VERSIONS.contains(&v.as_str())
        {
            let mut supported = vec![STATELESS_VERSION];
            supported.extend(HANDSHAKE_VERSIONS);
            return Some(
                json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32022, "message": format!("unsupported protocol version `{v}`"), "data": {"supported": supported}}}),
            );
        }
        let stateless = stated.as_deref() == Some(STATELESS_VERSION);
        let done = |mut result: Value| {
            if stateless && let Some(obj) = result.as_object_mut() {
                obj.insert("resultType".into(), json!("complete"));
            }
            Some(json!({"jsonrpc": "2.0", "id": id, "result": result}))
        };
        match method {
            "initialize" => {
                let asked = params.get("protocolVersion").and_then(Value::as_str).unwrap_or("");
                let version = if HANDSHAKE_VERSIONS.contains(&asked) { asked } else { HANDSHAKE_VERSIONS[0] };
                done(
                    json!({"protocolVersion": version, "capabilities": self.capabilities(), "serverInfo": self.info(), "instructions": INSTRUCTIONS}),
                )
            }
            "server/discover" => {
                let mut supported = vec![STATELESS_VERSION];
                supported.extend(HANDSHAKE_VERSIONS);
                done(
                    json!({"supportedVersions": supported, "capabilities": self.capabilities(), "serverInfo": self.info(), "instructions": INSTRUCTIONS}),
                )
            }
            "ping" => done(json!({})),
            "tools/list" => done(json!({"tools": tools::list(self.child_of.is_some())})),
            "tools/call" => {
                let name = params.get("name").and_then(Value::as_str).unwrap_or("");
                let args = params.get("arguments").cloned().unwrap_or(json!({}));
                if !tools::names(self.child_of.is_some()).contains(&name) {
                    return Some(error(&id, -32602, format!("unknown tool: {name}")));
                }
                // A bug in one tool must not take the server, and the host's session, down with it.
                let called = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| tools::call(self, name, &args)))
                    .unwrap_or_else(|_| {
                        Err(hufu_core::error::HufuError::internal(format!("`{name}` hit a bug in Hufu")))
                    });
                match called {
                    Ok(value) => {
                        let text = match &value {
                            Value::String(s) => s.clone(),
                            other => serde_json::to_string_pretty(other).unwrap_or_default(),
                        };
                        let mut result = json!({"content": [{"type": "text", "text": text}], "isError": false});
                        if value.is_object() {
                            result["structuredContent"] = value;
                        }
                        done(result)
                    }
                    Err(e) => done(
                        json!({"content": [{"type": "text", "text": format!("{}: {}", e.kind_str(), e.message)}], "isError": true}),
                    ),
                }
            }
            "resources/list" => done(json!({"resources": []})),
            "prompts/list" => done(json!({"prompts": []})),
            other => Some(error(&id, -32601, format!("method not found: {other}"))),
        }
    }

    /// Serves until stdin closes.
    pub fn serve(&self) -> std::io::Result<()> {
        let stdin = std::io::stdin();
        let stdout = std::io::stdout();
        for line in stdin.lock().lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let answer = match serde_json::from_str::<Value>(&line) {
                Err(e) => Some(error(&Value::Null, -32700, format!("parse error: {e}"))),
                Ok(Value::Array(batch)) => {
                    let answers: Vec<Value> = batch.iter().filter_map(|m| self.handle(m)).collect();
                    (!answers.is_empty()).then_some(Value::Array(answers))
                }
                Ok(msg) => self.handle(&msg),
            };
            if let Some(answer) = answer {
                let mut out = stdout.lock();
                writeln!(out, "{answer}")?;
                out.flush()?;
            }
        }
        Ok(())
    }
}
