//! Tally checks: the result is compared with the task's acceptance criteria before the work
//! counts as accepted. An exit code of 0 from the agent is never enough on its own.

use std::path::Path;
use std::time::{Duration, Instant};

use hufu_contract::time::parse_duration;
use hufu_contract::{DeniedAction, proc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::spec::{Check, SchemaRef};
use crate::{glob, schema};

/// The outcome of one check.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CheckOutcome {
    pub check: String,
    pub description: String,
    pub passed: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
    #[serde(default)]
    pub duration_ms: u64,
}

/// What the checks look at.
pub struct Evidence<'a> {
    pub workspace: &'a Path,
    pub changed_files: &'a [String],
    pub structured_output: Option<&'a Value>,
    pub denied_actions: &'a [DeniedAction],
}

/// Runs every check in order. A failed check does not stop the later ones, so that the caller
/// sees everything that is wrong at once.
pub fn run(checks: &[Check], evidence: &Evidence) -> Vec<CheckOutcome> {
    checks.iter().map(|c| run_one(c, evidence)).collect()
}

pub fn all_passed(outcomes: &[CheckOutcome]) -> bool {
    outcomes.iter().all(|o| o.passed)
}

fn run_one(check: &Check, ev: &Evidence) -> CheckOutcome {
    let started = Instant::now();
    let (kind, description, passed, detail) = if let Some(line) = &check.command {
        let limit = check.timeout.as_deref().and_then(|t| parse_duration(t).ok()).unwrap_or(Duration::from_secs(600));
        let mut cmd = proc::shell_command(line);
        cmd.current_dir(ev.workspace);
        match proc::run_capture(cmd, limit) {
            Ok(out) if out.timed_out => {
                ("command", line.clone(), false, format!("timed out after {}s", limit.as_secs()))
            }
            Ok(out) if out.ok() => ("command", line.clone(), true, String::new()),
            Ok(out) => {
                let text = if out.stderr.trim().is_empty() { &out.stdout } else { &out.stderr };
                let code = out.code.map(|c| c.to_string()).unwrap_or_else(|| "signal".into());
                ("command", line.clone(), false, format!("exit {code}: {}", proc::tail(text, 600)))
            }
            Err(e) => ("command", line.clone(), false, format!("could not run: {e}")),
        }
    } else if let Some(rules) = &check.paths {
        let mut bad = Vec::new();
        for file in ev.changed_files {
            if rules.deny.iter().any(|p| glob::matches(p, file)) {
                bad.push(format!("{file} (denied)"));
            } else if !rules.allow.is_empty() && !rules.allow.iter().any(|p| glob::matches(p, file)) {
                bad.push(format!("{file} (not allowed)"));
            }
        }
        let description = format!("allow [{}] deny [{}]", rules.allow.join(", "), rules.deny.join(", "));
        ("paths", description, bad.is_empty(), bad.join(", "))
    } else if let Some(schema_ref) = &check.schema {
        let schema = match schema_ref {
            SchemaRef::Inline(v) => Ok(v.clone()),
            other => other.load(ev.workspace).map_err(|e| e.message),
        };
        match (schema, ev.structured_output) {
            (Err(e), _) => ("schema", "structured output".into(), false, e),
            (Ok(_), None) => {
                ("schema", "structured output".into(), false, "the job produced no structured output".into())
            }
            (Ok(schema), Some(value)) => {
                let errors = schema::validate(&schema, value);
                ("schema", "structured output".into(), errors.is_empty(), errors.join("; "))
            }
        }
    } else if check.no_denied_actions == Some(true) {
        let list: Vec<String> = ev.denied_actions.iter().map(|d| format!("{} {}", d.kind, d.tool)).collect();
        ("no_denied_actions", "no action was skipped for lack of permission".into(), list.is_empty(), list.join(", "))
    } else if check.review.is_some() {
        // Advisor review needs the mission's advisor; the mission driver runs it after the job.
        ("review", "advisor review".into(), true, "decided by the mission's advisor".into())
    } else {
        ("unknown", String::new(), true, String::new())
    };
    CheckOutcome {
        check: kind.to_string(),
        description,
        passed,
        detail,
        duration_ms: started.elapsed().as_millis() as u64,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::PathRules;
    use serde_json::json;

    #[test]
    fn checks_pass_and_fail_for_the_right_reasons() {
        let tmp = std::env::temp_dir();
        let changed = vec!["src/auth/refresh.ts".to_string(), "package.json".to_string()];
        let output = json!({"ok": true});
        let denied = vec![DeniedAction { tool: "rm".into(), kind: "execute".into(), detail: String::new() }];
        let ev = Evidence {
            workspace: &tmp,
            changed_files: &changed,
            structured_output: Some(&output),
            denied_actions: &denied,
        };
        let checks = vec![
            Check { command: Some("exit 0".into()), ..Check::default() },
            Check { command: Some("echo broken && exit 7".into()), ..Check::default() },
            Check { paths: Some(PathRules { allow: vec!["src/auth/**".into()], deny: vec![] }), ..Check::default() },
            Check { paths: Some(PathRules { allow: vec![], deny: vec!["**/*.lock".into()] }), ..Check::default() },
            Check {
                schema: Some(SchemaRef::Inline(json!({"type": "object", "required": ["ok"]}))),
                ..Check::default()
            },
            Check { schema: Some(SchemaRef::Inline(json!({"type": "array"}))), ..Check::default() },
            Check { no_denied_actions: Some(true), ..Check::default() },
        ];
        let out = run(&checks, &ev);
        let passed: Vec<bool> = out.iter().map(|o| o.passed).collect();
        assert_eq!(passed, [true, false, false, true, true, false, false]);
        assert!(out[1].detail.contains("exit 7") && out[1].detail.contains("broken"));
        assert!(out[2].detail.contains("package.json (not allowed)"));
        assert!(!all_passed(&out));
    }

    #[test]
    fn a_slow_command_times_out() {
        let tmp = std::env::temp_dir();
        let ev = Evidence { workspace: &tmp, changed_files: &[], structured_output: None, denied_actions: &[] };
        let sleep = if cfg!(windows) { "ping -n 6 127.0.0.1 > NUL" } else { "sleep 5" };
        let out = run(&[Check { command: Some(sleep.into()), timeout: Some("300ms".into()), ..Check::default() }], &ev);
        assert!(!out[0].passed && out[0].detail.contains("timed out"));
    }
}
