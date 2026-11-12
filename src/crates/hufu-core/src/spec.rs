//! The task spec a caller writes, and the fully resolved job spec that is stored with the job.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use hufu_contract::{Effort, Profile, Tier};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::{Grant, Isolation, RetryPolicy, Route};
use crate::error::{HufuError, Result};

/// A JSON Schema given inline or as a path to a file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SchemaRef {
    Path(String),
    Inline(Value),
}

impl SchemaRef {
    /// Loads the schema. Paths are relative to `base`.
    pub fn load(&self, base: &Path) -> Result<Value> {
        match self {
            SchemaRef::Inline(v) => Ok(v.clone()),
            SchemaRef::Path(p) => {
                let path = if Path::new(p).is_absolute() { PathBuf::from(p) } else { base.join(p) };
                let text = std::fs::read_to_string(&path).map_err(|e| HufuError::usage(format!("schema file {}: {e}", path.display())))?;
                crate::config::parse_yaml(&text).map_err(|e| HufuError::usage(format!("schema file {}: {e}", path.display())))
            }
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PathRules {
    #[serde(default)]
    pub allow: Vec<String>,
    #[serde(default)]
    pub deny: Vec<String>,
}

/// One tally check. Exactly one of the kinds is set.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Check {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// Time limit for `command`. Default ten minutes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paths: Option<PathRules>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<SchemaRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub no_denied_actions: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review: Option<String>,
}

impl Check {
    pub fn kind(&self) -> std::result::Result<&'static str, String> {
        let kinds: Vec<&'static str> = [
            self.command.as_ref().map(|_| "command"),
            self.paths.as_ref().map(|_| "paths"),
            self.schema.as_ref().map(|_| "schema"),
            self.no_denied_actions.map(|_| "no_denied_actions"),
            self.review.as_ref().map(|_| "review"),
        ]
        .into_iter()
        .flatten()
        .collect();
        match kinds.as_slice() {
            [one] => Ok(one),
            [] => Err("an acceptance check needs one of: command, paths, schema, no_denied_actions, review".into()),
            many => Err(format!("an acceptance check may have only one kind, found: {}", many.join(", "))),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputSpec {
    pub schema: SchemaRef,
}

/// A per-task override of how computer use is routed.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComputerUseOverride {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<Route>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
}

/// What a caller writes: a YAML or JSON file, flags, or both.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<u32>,
    #[serde(default)]
    pub prompt: String,
    /// A profile or pool name.
    #[serde(default)]
    pub agent: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission: Option<Tier>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<Effort>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inactivity_timeout: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub isolation: Option<Isolation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<OutputSpec>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub acceptance: Vec<Check>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strict_permissions: Option<bool>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub labels: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub grants: Vec<Grant>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry: Option<RetryPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub computer_use: Option<ComputerUseOverride>,
}

impl TaskSpec {
    /// Reads a task spec from a YAML or JSON file.
    pub fn from_file(path: &Path) -> Result<TaskSpec> {
        let text = std::fs::read_to_string(path).map_err(|e| HufuError::usage(format!("task file {}: {e}", path.display())))?;
        TaskSpec::from_text(&text).map_err(|e| HufuError::usage(format!("task file {}: {}", path.display(), e.message)))
    }

    pub fn from_text(text: &str) -> Result<TaskSpec> {
        let value = crate::config::parse_yaml(text).map_err(HufuError::usage)?;
        TaskSpec::from_value(value)
    }

    pub fn from_value(value: Value) -> Result<TaskSpec> {
        serde_json::from_value(value).map_err(|e| HufuError::usage(format!("invalid task spec: {e}")))
    }
}

/// How computer use is served for one job, after routing was resolved.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ComputerUsePlan {
    pub route: Route,
    /// The profile that operates the desktop: the executor itself, or the dedicated profile.
    pub agent: String,
    pub apps: Vec<String>,
    /// The runtime behind it: `native`, `lcu`, or another name.
    pub via: String,
}

/// `spec.json`: the task as it will run, with every default applied. Written once.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct JobSpec {
    pub v: u32,
    pub id: String,
    pub title: String,
    pub prompt: String,
    /// The name the task asked for: a profile or a pool.
    pub agent: String,
    /// The profile chosen to run it.
    pub profile: Profile,
    /// Other members of the pool, tried in order when the chosen one cannot serve.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub alternates: Vec<Profile>,
    pub cwd: PathBuf,
    pub permission: Tier,
    pub effort: Effort,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub timeout_s: u64,
    pub inactivity_timeout_s: u64,
    pub isolation: Isolation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub acceptance: Vec<Check>,
    pub strict_permissions: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub labels: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub grants: Vec<Grant>,
    pub retry: RetryPolicy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_session: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mission: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub computer_use: Option<ComputerUsePlan>,
    /// Problems found at submission that did not prevent it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

/// The first line of a prompt, clipped, for listings.
pub fn title_from_prompt(prompt: &str) -> String {
    let line = prompt.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("untitled task");
    if line.chars().count() > 72 { format!("{}…", line.chars().take(72).collect::<String>()) } else { line.to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_documented_task_file_parses() {
        let spec = TaskSpec::from_text(
            r#"
version: 1
title: Refactor token refresh
prompt: |
  Refactor the token refresh logic under src/auth.
agent: codex-high
cwd: .
permission: workspace-write
effort: high
timeout: 45m
isolation: worktree
output:
  schema: ./schemas/change-summary.json
acceptance:
  - command: npm test -- auth
  - paths:
      allow: ["src/auth/**", "test/auth/**"]
grants:
  - capability: computer_use
    apps: ["Figma"]
"#,
        )
        .unwrap();
        assert_eq!(spec.permission, Some(Tier::WorkspaceWrite));
        assert_eq!(spec.isolation, Some(Isolation::Worktree));
        assert_eq!(spec.acceptance.len(), 2);
        assert_eq!(spec.acceptance[0].kind().unwrap(), "command");
        assert_eq!(spec.acceptance[1].kind().unwrap(), "paths");
        assert!(matches!(spec.output.unwrap().schema, SchemaRef::Path(_)));
        assert_eq!(spec.grants[0].apps, ["Figma"]);
    }

    #[test]
    fn unknown_fields_and_ambiguous_checks_are_errors() {
        let err = TaskSpec::from_text("prompt: x\nagent: a\npermision: read-only\n").unwrap_err();
        assert!(err.message.contains("permision"), "{}", err.message);
        let both = Check { command: Some("x".into()), no_denied_actions: Some(true), ..Check::default() };
        assert!(both.kind().is_err());
        assert!(Check::default().kind().is_err());
        assert_eq!(title_from_prompt("\n  Fix the bug  \nmore"), "Fix the bug");
    }
}
