//! Operations shared by the command line and the MCP server: listing agents, editing
//! configuration, pruning old records.

use std::path::{Path, PathBuf};
use std::time::Duration;

use hufu_contract::time::{now_ms, parse_duration, parse_rfc3339};
use hufu_contract::{AdapterRegistry, Capabilities, ProbeResult};
use serde::Serialize;
use serde_json::{Value, json};

use crate::config::{self, Config, Home, Loaded, Strategy};
use crate::error::{HufuError, Result};
use crate::mission;
use crate::store;
use crate::workspace;

/// One row of `hufu agents`.
#[derive(Clone, Debug, Serialize)]
pub struct AgentView {
    pub name: String,
    /// `profile` or `pool`.
    pub kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub adapter: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Comes from Hufu's built-in presets, not from a configuration file.
    pub builtin: bool,
    /// `advisor`, `executor`, `computer_use`.
    pub roles: Vec<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<Capabilities>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub probe: Option<ProbeResult>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub members: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strategy: Option<Strategy>,
    /// Pools this profile is a member of.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub pools: Vec<String>,
}

/// Every profile and pool, with what each can do. `probe` also checks installation and login.
pub fn agents(loaded: &Loaded, adapters: &dyn AdapterRegistry, probe: bool) -> Vec<AgentView> {
    let config = &loaded.config;
    let role_of = |name: &str| {
        let mut roles = Vec::new();
        if config.roles.advisor.as_deref() == Some(name) {
            roles.push("advisor");
        }
        if config.roles.executors.iter().any(|e| e == name) {
            roles.push("executor");
        }
        if config.capabilities.computer_use.agent.as_deref() == Some(name) {
            roles.push("computer_use");
        }
        roles
    };
    let mut out = Vec::new();
    for (name, profile) in &config.profiles {
        let adapter = adapters.get(&profile.adapter);
        out.push(AgentView {
            name: name.clone(),
            kind: "profile",
            adapter: Some(profile.adapter.clone()),
            account: profile.account.clone(),
            description: profile.description.clone(),
            model: profile.model.clone(),
            builtin: loaded.builtin.contains(name),
            roles: role_of(name),
            capabilities: adapter.map(|a| a.capabilities(profile)),
            probe: if probe { adapter.map(|a| a.probe(profile)) } else { None },
            members: Vec::new(),
            strategy: None,
            pools: config.pools.iter().filter(|(_, p)| p.members.contains(name)).map(|(n, _)| n.clone()).collect(),
        });
    }
    for (name, pool) in &config.pools {
        out.push(AgentView {
            name: name.clone(),
            kind: "pool",
            adapter: None,
            account: None,
            description: pool.description.clone(),
            model: None,
            builtin: false,
            roles: role_of(name),
            capabilities: None,
            probe: None,
            members: pool.members.clone(),
            strategy: Some(pool.strategy),
            pools: Vec::new(),
        });
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Editing configuration
// ---------------------------------------------------------------------------------------------

/// The file an edit goes to: the user-wide one, or `.hufu/config.yaml` of a project.
pub fn config_target(home: &Home, project: Option<&Path>) -> PathBuf {
    match project {
        Some(dir) => dir.join(".hufu").join("config.yaml"),
        None => home.config_file(),
    }
}

/// Applies an edit to a configuration file. The file is only written if the result is valid.
pub fn edit(path: &Path, adapters: &dyn AdapterRegistry, change: impl FnOnce(&mut Value) -> Result<()>) -> Result<()> {
    let mut raw = config::read_raw(path)?;
    change(&mut raw)?;
    config::write_raw(path, &raw, adapters)
}

fn check_name(kind: &str, name: &str) -> Result<()> {
    let ok = !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if ok { Ok(()) } else { Err(HufuError::usage(format!("{kind} name `{name}` may only use letters, digits, `-`, `_` and `.`"))) }
}

/// Creates or replaces a profile. `body` is the profile as it would appear in YAML.
pub fn profile_set(path: &Path, adapters: &dyn AdapterRegistry, name: &str, body: Value, merge: bool) -> Result<()> {
    check_name("profile", name)?;
    edit(path, adapters, |raw| {
        let mut body = body;
        if merge {
            if let (Some(old), Some(new)) = (config::get_path(raw, &format!("profiles.{name}")).and_then(Value::as_object).cloned(), body.as_object_mut()) {
                for (k, v) in old {
                    new.entry(k).or_insert(v);
                }
            }
        }
        if let Some(obj) = body.as_object_mut() {
            obj.retain(|_, v| !v.is_null());
        }
        // `set_path` splits on dots, so a dotted profile name is inserted by hand.
        if !raw.is_object() {
            *raw = json!({});
        }
        let profiles = raw.as_object_mut().expect("an object").entry("profiles").or_insert(json!({}));
        if !profiles.is_object() {
            *profiles = json!({});
        }
        profiles.as_object_mut().expect("an object").insert(name.to_string(), body);
        Ok(())
    })
}

fn remove_named(raw: &mut Value, section: &str, name: &str) -> bool {
    raw.get_mut(section).and_then(Value::as_object_mut).map(|m| m.remove(name).is_some()).unwrap_or(false)
}

pub fn profile_remove(path: &Path, adapters: &dyn AdapterRegistry, name: &str) -> Result<()> {
    edit(path, adapters, |raw| {
        if remove_named(raw, "profiles", name) { Ok(()) } else { Err(HufuError::usage(format!("{} has no profile `{name}`", path.display()))) }
    })
}

pub fn pool_set(path: &Path, adapters: &dyn AdapterRegistry, name: &str, members: &[String], strategy: Option<&str>, description: Option<&str>) -> Result<()> {
    check_name("pool", name)?;
    edit(path, adapters, |raw| {
        let mut body = json!({"members": members});
        if let Some(s) = strategy {
            body["strategy"] = json!(s);
        }
        if let Some(d) = description {
            body["description"] = json!(d);
        }
        if !raw.is_object() {
            *raw = json!({});
        }
        let pools = raw.as_object_mut().expect("an object").entry("pools").or_insert(json!({}));
        if !pools.is_object() {
            *pools = json!({});
        }
        pools.as_object_mut().expect("an object").insert(name.to_string(), body);
        Ok(())
    })
}

pub fn pool_remove(path: &Path, adapters: &dyn AdapterRegistry, name: &str) -> Result<()> {
    edit(path, adapters, |raw| {
        if remove_named(raw, "pools", name) { Ok(()) } else { Err(HufuError::usage(format!("{} has no pool `{name}`", path.display()))) }
    })
}

pub fn config_set(path: &Path, adapters: &dyn AdapterRegistry, key: &str, value: Value) -> Result<()> {
    edit(path, adapters, |raw| config::set_path(raw, key, value))
}

pub fn config_unset(path: &Path, adapters: &dyn AdapterRegistry, key: &str) -> Result<()> {
    edit(path, adapters, |raw| {
        if config::remove_path(raw, key) { Ok(()) } else { Err(HufuError::usage(format!("{} does not set `{key}`", path.display()))) }
    })
}

/// The merged configuration as a JSON value, or one key of it.
pub fn config_get(config: &Config, key: Option<&str>) -> Result<Value> {
    let all = serde_json::to_value(config).map_err(|e| HufuError::internal(e.to_string()))?;
    match key {
        None => Ok(all),
        Some(k) => config::get_path(&all, k).cloned().ok_or_else(|| HufuError::usage(format!("no configuration key `{k}`"))),
    }
}

// ---------------------------------------------------------------------------------------------
// Pruning
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Debug, Default)]
pub struct PruneOptions {
    /// Remove finished jobs older than this. Default: `retention.max_age`.
    pub older_than: Option<Duration>,
    /// Keep at most this many finished jobs. Default: `retention.max_jobs`.
    pub keep: Option<usize>,
    /// Also remove worktrees that jobs and missions left behind. Their branches are kept.
    pub worktrees: bool,
    pub dry_run: bool,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct PruneReport {
    pub removed_jobs: Vec<String>,
    pub removed_missions: Vec<String>,
    pub removed_worktrees: Vec<String>,
    /// Finished jobs that are due but still have a worktree; pass `--worktrees` to remove them.
    pub kept_for_worktree: Vec<String>,
    pub remaining_jobs: usize,
    pub dry_run: bool,
}

/// Removes old finished jobs and missions. Running work is never touched.
pub fn prune(home: &Home, config: &Config, opts: &PruneOptions) -> Result<PruneReport> {
    let max_age = opts.older_than.unwrap_or_else(|| parse_duration(&config.retention.max_age).unwrap_or(Duration::from_secs(30 * 86400)));
    let keep = opts.keep.unwrap_or(config.retention.max_jobs);
    let now = now_ms();
    let mut report = PruneReport { dry_run: opts.dry_run, ..PruneReport::default() };

    let missions = mission::list(home);
    let live_missions: Vec<&str> = missions.iter().filter(|m| !m.state.is_terminal()).map(|m| m.id.as_str()).collect();
    let jobs = store::list_jobs(home);
    let finished: Vec<&store::JobState> = jobs.iter().filter(|j| j.state.is_terminal() && !j.mission.as_deref().map(|m| live_missions.contains(&m)).unwrap_or(false)).collect();
    let excess = finished.len().saturating_sub(keep);
    for (i, job) in finished.iter().enumerate() {
        let ended = job.finished_at.as_deref().and_then(parse_rfc3339).unwrap_or(now);
        let old = now.saturating_sub(ended) > max_age.as_millis() as u64;
        if !(old || i < excess) {
            continue;
        }
        let worktree = home.worktrees().join(&job.id);
        if worktree.exists() {
            if !opts.worktrees {
                report.kept_for_worktree.push(job.id.clone());
                continue;
            }
            report.removed_worktrees.push(worktree.display().to_string());
            if !opts.dry_run {
                let repo = job.workspace.as_ref().and_then(|w| w.repo.clone()).map(PathBuf::from);
                match repo {
                    Some(repo) => {
                        workspace::remove_worktree(&repo, &worktree);
                    }
                    None => {
                        let _ = std::fs::remove_dir_all(&worktree);
                    }
                }
            }
        }
        report.removed_jobs.push(job.id.clone());
        if !opts.dry_run {
            store::remove_job(home, &job.id)?;
        }
    }
    for m in missions.iter().filter(|m| m.state.is_terminal()) {
        let ended = m.finished_at.as_deref().and_then(parse_rfc3339).unwrap_or(now);
        if now.saturating_sub(ended) <= max_age.as_millis() as u64 {
            continue;
        }
        let worktree = home.worktrees().join(&m.id);
        if worktree.exists() {
            if !opts.worktrees {
                continue;
            }
            report.removed_worktrees.push(worktree.display().to_string());
            if !opts.dry_run {
                workspace::remove_worktree(Path::new(&m.cwd), &worktree);
            }
        }
        report.removed_missions.push(m.id.clone());
        if !opts.dry_run {
            let _ = std::fs::remove_dir_all(home.mission(&m.id));
        }
    }
    report.remaining_jobs = jobs.len() - if opts.dry_run { 0 } else { report.removed_jobs.len() };
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::tests_support::kinds;

    #[test]
    fn profiles_and_pools_are_edited_and_validated() {
        let dir = std::env::temp_dir().join(format!("hufu-ops-{}", hufu_contract::time::new_id("t")));
        let path = dir.join("config.yaml");
        let reg = kinds();
        profile_set(&path, reg, "codex-work", json!({"adapter": "codex-cli", "account": "work", "home": "~/.codex-work"}), false).unwrap();
        profile_set(&path, reg, "codex-home", json!({"adapter": "codex-cli", "account": "home", "home": "~/.codex-home"}), false).unwrap();
        profile_set(&path, reg, "codex-work", json!({"effort": "high", "account": null}), true).unwrap();
        pool_set(&path, reg, "codex-any", &["codex-work".into(), "codex-home".into()], Some("round-robin"), None).unwrap();
        config_set(&path, reg, "roles.advisor", json!("codex-work")).unwrap();
        config_set(&path, reg, "roles.executors", json!(["codex-any"])).unwrap();
        let raw = config::read_raw(&path).unwrap();
        assert_eq!(config::get_path(&raw, "profiles.codex-work.effort"), Some(&json!("high")));
        assert_eq!(config::get_path(&raw, "profiles.codex-work.adapter"), Some(&json!("codex-cli")), "a merge keeps what was there");
        assert_eq!(config::get_path(&raw, "profiles.codex-work.account"), None, "null removes a field");
        // Invalid edits are refused and leave the file as it was.
        assert!(pool_set(&path, reg, "bad", &["ghost".into()], None, None).is_err());
        assert!(profile_remove(&path, reg, "codex-home").unwrap_err().message.contains("pools.codex-any"), "a profile a pool still uses cannot be removed");
        assert!(config_set(&path, reg, "limits.max_concurrent_jobs", json!(0)).is_err());
        assert!(profile_set(&path, reg, "bad name", json!({"adapter": "codex-cli"}), false).is_err());
        pool_remove(&path, reg, "codex-any").unwrap_err();
        config_unset(&path, reg, "roles.executors").unwrap();
        pool_remove(&path, reg, "codex-any").unwrap();
        profile_remove(&path, reg, "codex-home").unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
