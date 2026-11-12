//! Normalized events: the same shape for every adapter.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::types::Usage;

/// The payload of one event. Serialized with a `type` field.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum EventBody {
    /// The job entered a new state.
    #[serde(rename = "job.state")]
    JobState {
        state: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    /// Text from the agent. `final` marks the final answer.
    #[serde(rename = "agent.message")]
    AgentMessage {
        text: String,
        #[serde(rename = "final", default)]
        is_final: bool,
    },
    /// Reasoning text, when the agent exposes it.
    #[serde(rename = "agent.thought")]
    AgentThought { text: String },
    /// A tool call started or changed status. `kind` uses ACP's vocabulary.
    #[serde(rename = "tool.call")]
    ToolCall {
        id: String,
        kind: String,
        title: String,
        status: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<Value>,
    },
    /// The agent asked for permission; includes Hufu's decision.
    #[serde(rename = "permission.request")]
    PermissionRequest {
        tool: String,
        kind: String,
        decision: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        why: Option<String>,
    },
    /// An action was skipped for lack of permission.
    #[serde(rename = "permission.denied")]
    PermissionDenied {
        tool: String,
        kind: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        detail: String,
    },
    /// The agent published or updated a plan.
    #[serde(rename = "plan")]
    Plan { entries: Value },
    /// Token or cost figures reported by the agent.
    #[serde(rename = "usage")]
    Usage {
        #[serde(flatten)]
        usage: Usage,
    },
    #[serde(rename = "warning")]
    Warning { message: String },
    #[serde(rename = "error")]
    Error { message: String },
}

impl EventBody {
    pub fn message(text: impl Into<String>, is_final: bool) -> EventBody {
        EventBody::AgentMessage { text: text.into(), is_final }
    }

    pub fn warning(message: impl Into<String>) -> EventBody {
        EventBody::Warning { message: message.into() }
    }

    pub fn error(message: impl Into<String>) -> EventBody {
        EventBody::Error { message: message.into() }
    }

    pub fn tool(id: impl Into<String>, kind: &str, title: impl Into<String>, status: &str) -> EventBody {
        EventBody::ToolCall {
            id: id.into(),
            kind: kind.to_string(),
            title: title.into(),
            status: status.to_string(),
            detail: None,
        }
    }

    /// A one-line rendering for `hufu logs` and for the "last activity" column.
    pub fn summary(&self) -> String {
        fn clip(s: &str, n: usize) -> String {
            let one_line = s.split_whitespace().collect::<Vec<_>>().join(" ");
            if one_line.chars().count() > n {
                format!("{}…", one_line.chars().take(n).collect::<String>())
            } else {
                one_line
            }
        }
        match self {
            EventBody::JobState { state, reason } => match reason {
                Some(r) => format!("state {state} ({r})"),
                None => format!("state {state}"),
            },
            EventBody::AgentMessage { text, is_final } => {
                format!("{} {}", if *is_final { "answer" } else { "says" }, clip(text, 160))
            }
            EventBody::AgentThought { text } => format!("thinks {}", clip(text, 120)),
            EventBody::ToolCall { kind, title, status, .. } => {
                format!("{kind} {} [{status}]", clip(title, 120))
            }
            EventBody::PermissionRequest { tool, kind, decision, .. } => {
                format!("permission {kind} {} -> {decision}", clip(tool, 100))
            }
            EventBody::PermissionDenied { tool, kind, detail } => {
                format!("denied {kind} {} {}", clip(tool, 80), clip(detail, 80))
            }
            EventBody::Plan { entries } => {
                format!("plan ({} entries)", entries.as_array().map(|a| a.len()).unwrap_or(0))
            }
            EventBody::Usage { usage } => format!("usage {} tokens", usage.tokens()),
            EventBody::Warning { message } => format!("warning {}", clip(message, 200)),
            EventBody::Error { message } => format!("error {}", clip(message, 200)),
        }
    }
}

/// One line of `events.ndjson`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub v: u32,
    pub seq: u64,
    pub ts: String,
    #[serde(flatten)]
    pub body: EventBody,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_serialize_with_a_type_field() {
        let e = Event { v: 1, seq: 42, ts: "2026-11-12T17:03:11.120Z".into(), body: EventBody::tool("c7", "edit", "src/a.rs", "completed") };
        let s = serde_json::to_string(&e).unwrap();
        assert!(s.contains("\"type\":\"tool.call\"") && s.contains("\"seq\":42"));
        let back: Event = serde_json::from_str(&s).unwrap();
        assert_eq!(back, e);
        let m = serde_json::to_value(EventBody::message("done", true)).unwrap();
        assert_eq!(m["final"], true);
    }
}
