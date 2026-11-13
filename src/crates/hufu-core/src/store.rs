//! The job store: one directory per job, written atomically, readable by any later process.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use hufu_contract::time::{now_ms, now_rfc3339, parse_rfc3339};
use hufu_contract::{DeniedAction, Effort, Event, SessionRef, Tier, Usage, proc};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::Home;
use crate::error::{HufuError, IoContext, Result};
use crate::spec::JobSpec;
use crate::tally::CheckOutcome;

/// A heartbeat older than this, with no live runner, means the job is lost.
pub const STALE_AFTER: Duration = Duration::from_secs(30);
pub const HEARTBEAT_EVERY: Duration = Duration::from_secs(5);

/// `STALE_AFTER`, unless `HUFU_STALE_AFTER_MS` shortens it (tests do, so they need not wait).
pub fn stale_after() -> Duration {
    std::env::var("HUFU_STALE_AFTER_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .map(Duration::from_millis)
        .unwrap_or(STALE_AFTER)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Queued,
    Starting,
    Running,
    Succeeded,
    Failed,
    Cancelled,
    TimedOut,
    Lost,
}

impl State {
    pub fn is_terminal(self) -> bool {
        matches!(self, State::Succeeded | State::Failed | State::Cancelled | State::TimedOut | State::Lost)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            State::Queued => "queued",
            State::Starting => "starting",
            State::Running => "running",
            State::Succeeded => "succeeded",
            State::Failed => "failed",
            State::Cancelled => "cancelled",
            State::TimedOut => "timed_out",
            State::Lost => "lost",
        }
    }
}

/// Whether the work was accepted. Separate from the state: a job can succeed and be rejected.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    #[default]
    Unchecked,
    Accepted,
    Rejected,
}

impl Verdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Verdict::Unchecked => "unchecked",
            Verdict::Accepted => "accepted",
            Verdict::Rejected => "rejected",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceInfo {
    /// Where the agent actually ran.
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    #[serde(default)]
    pub worktree: bool,
}

/// `state.json`: the current state of a job, rewritten atomically as it changes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct JobState {
    pub v: u32,
    pub id: String,
    pub state: State,
    #[serde(default)]
    pub verdict: Verdict,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub title: String,
    /// The name the task asked for: a profile or a pool.
    pub agent: String,
    /// The profile actually in use.
    pub profile: String,
    pub adapter: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_version: Option<String>,
    pub permission: Tier,
    pub cwd: String,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heartbeat_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runner_pid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_pid: Option<u32>,
    #[serde(default)]
    pub attempt: u32,
    /// `daemon` while the job waits for the daemon to start its runner.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_activity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_activity_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<WorkspaceInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mission: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub labels: BTreeMap<String, String>,
}

impl JobState {
    /// Seconds from start (or creation) to finish (or now).
    pub fn elapsed_s(&self) -> u64 {
        let start = self.started_at.as_deref().or(Some(self.created_at.as_str())).and_then(parse_rfc3339).unwrap_or(0);
        let end = self.finished_at.as_deref().and_then(parse_rfc3339).unwrap_or_else(now_ms);
        end.saturating_sub(start) / 1000
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AgentInfo {
    pub profile: String,
    pub adapter: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub effort: Effort,
    pub permission: Option<Tier>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Changes {
    pub files: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Timing {
    pub queued_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    pub finished_at: String,
    pub duration_s: u64,
}

/// `result.json`: the result envelope, written once at the terminal state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct JobResult {
    pub v: u32,
    pub job: String,
    pub state: State,
    pub verdict: Verdict,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub title: String,
    pub agent: AgentInfo,
    #[serde(default)]
    pub final_message: Option<String>,
    #[serde(default)]
    pub structured_output: Option<Value>,
    #[serde(default)]
    pub changes: Changes,
    #[serde(default)]
    pub denied_actions: Vec<DeniedAction>,
    #[serde(default)]
    pub warnings: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<SessionRef>,
    pub timing: Timing,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tally: Vec<CheckOutcome>,
    #[serde(default)]
    pub attempts: u32,
    /// Child jobs, for example computer-use steps handed to a dedicated agent.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mission: Option<String>,
}

// ---------------------------------------------------------------------------------------------
// File primitives
// ---------------------------------------------------------------------------------------------

pub(crate) fn retry<T>(mut op: impl FnMut() -> std::io::Result<T>) -> std::io::Result<T> {
    // On Windows a rename or read can briefly collide with another process holding the file.
    let mut last = None;
    for attempt in 0..8 {
        match op() {
            Ok(v) => return Ok(v),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(e),
            Err(e) => {
                last = Some(e);
                std::thread::sleep(Duration::from_millis(5 * (attempt + 1)));
            }
        }
    }
    Err(last.unwrap_or_else(|| std::io::Error::other("retry failed")))
}

/// Writes a file by writing a temporary sibling and renaming it over the target.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    // The counter keeps two threads of one process apart within the same millisecond.
    static SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = dir.join(format!(".{name}.{}.{}.{seq}.tmp", std::process::id(), now_ms()));
    {
        let mut f = std::fs::File::create(&tmp).at(&tmp)?;
        f.write_all(bytes).at(&tmp)?;
        f.sync_all().ok();
    }
    retry(|| std::fs::rename(&tmp, path)).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        HufuError::internal(format!("{}: {e}", path.display()))
    })
}

pub fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|e| HufuError::internal(e.to_string()))?;
    write_atomic(path, &bytes)
}

pub fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let bytes = retry(|| std::fs::read(path)).at(path)?;
    serde_json::from_slice(&bytes).map_err(|e| HufuError::internal(format!("{}: {e}", path.display())))
}

// ---------------------------------------------------------------------------------------------
// Jobs
// ---------------------------------------------------------------------------------------------

/// Creates the job directory with its spec and a `queued` state.
pub fn create_job(home: &Home, spec: &JobSpec) -> Result<JobState> {
    home.ensure()?;
    let dir = home.job(&spec.id);
    std::fs::create_dir_all(dir.join("raw")).at(&dir)?;
    write_json(&dir.join("spec.json"), spec)?;
    let state = JobState {
        v: hufu_contract::FORMAT_VERSION,
        id: spec.id.clone(),
        state: State::Queued,
        verdict: Verdict::Unchecked,
        reason: None,
        detail: None,
        title: spec.title.clone(),
        agent: spec.agent.clone(),
        profile: spec.profile.name.clone(),
        adapter: spec.profile.adapter.clone(),
        agent_version: None,
        permission: spec.permission,
        cwd: spec.cwd.display().to_string(),
        created_at: now_rfc3339(),
        started_at: None,
        finished_at: None,
        heartbeat_at: None,
        runner_pid: None,
        agent_pid: None,
        attempt: 0,
        queue: None,
        last_activity: None,
        last_activity_at: None,
        workspace: None,
        mission: spec.mission.clone(),
        parent: spec.parent.clone(),
        labels: spec.labels.clone(),
    };
    save_state(home, &state)?;
    Ok(state)
}

pub fn save_state(home: &Home, state: &JobState) -> Result<()> {
    write_json(&home.job(&state.id).join("state.json"), state)
}

pub fn load_spec(home: &Home, id: &str) -> Result<JobSpec> {
    read_json(&home.job(id).join("spec.json"))
}

/// Loads a job's state. A job whose runner died is rewritten as `lost` on the way.
pub fn load_state(home: &Home, id: &str) -> Result<JobState> {
    let path = home.job(id).join("state.json");
    if !path.exists() {
        return Err(HufuError::usage(format!("no such job `{id}`; list jobs with `hufu status --all`")));
    }
    let mut state: JobState = read_json(&path)?;
    if is_lost(&state) {
        state.state = State::Lost;
        state.reason = Some(hufu_contract::reason::RUNNER_DIED.into());
        state.detail = Some("the runner process is gone and its heartbeat stopped".into());
        state.finished_at = Some(now_rfc3339());
        state.agent_pid = None;
        save_state(home, &state)?;
        release_slots_of(home, id);
    }
    Ok(state)
}

fn is_lost(state: &JobState) -> bool {
    if state.state.is_terminal() {
        return false;
    }
    let Some(pid) = state.runner_pid else {
        // No runner yet: either the daemon will start one, or `hufu run` is about to.
        return false;
    };
    let beat = state.heartbeat_at.as_deref().and_then(parse_rfc3339).unwrap_or(0);
    let stale = now_ms().saturating_sub(beat) > stale_after().as_millis() as u64;
    stale && !proc::pid_alive(pid)
}

fn release_slots_of(home: &Home, id: &str) {
    if let Ok(entries) = std::fs::read_dir(home.slots()) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with(id) {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
}

/// All job ids, oldest first. Ids sort by creation time.
pub fn job_ids(home: &Home) -> Vec<String> {
    let mut ids: Vec<String> = std::fs::read_dir(home.jobs())
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().join("state.json").exists())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    ids.sort();
    ids
}

/// All jobs, oldest first. Unreadable records are skipped rather than failing the listing.
pub fn list_jobs(home: &Home) -> Vec<JobState> {
    let mut jobs: Vec<JobState> = job_ids(home).iter().filter_map(|id| load_state(home, id).ok()).collect();
    // Ids only sort to the second; the creation time breaks ties.
    jobs.sort_by(|a, b| (&a.created_at, &a.id).cmp(&(&b.created_at, &b.id)));
    jobs
}

/// Accepts a full job id or an unambiguous prefix.
pub fn resolve_job(home: &Home, partial: &str) -> Result<String> {
    if home.job(partial).join("state.json").exists() {
        return Ok(partial.to_string());
    }
    let with_prefix = if partial.starts_with("j_") { partial.to_string() } else { format!("j_{partial}") };
    let hits: Vec<String> =
        job_ids(home).into_iter().filter(|id| id.starts_with(&with_prefix) || id.starts_with(partial)).collect();
    match hits.len() {
        1 => Ok(hits.into_iter().next().unwrap_or_default()),
        0 => Err(HufuError::usage(format!("no such job `{partial}`; list jobs with `hufu status --all`"))),
        n => Err(HufuError::usage(format!("`{partial}` matches {n} jobs; give more of the id"))),
    }
}

/// Reads the events with a sequence number greater than `after`.
pub fn read_events(home: &Home, id: &str, after: u64) -> Vec<Event> {
    let Ok(text) = std::fs::read_to_string(home.job(id).join("events.ndjson")) else {
        return Vec::new();
    };
    text.lines().filter_map(|l| serde_json::from_str::<Event>(l).ok()).filter(|e| e.seq > after).collect()
}

pub fn write_result(home: &Home, result: &JobResult) -> Result<()> {
    write_json(&home.job(&result.job).join("result.json"), result)
}

pub fn load_result(home: &Home, id: &str) -> Result<Option<JobResult>> {
    let path = home.job(id).join("result.json");
    if !path.exists() {
        return Ok(None);
    }
    read_json(&path).map(Some)
}

fn cancel_marker(home: &Home, id: &str) -> PathBuf {
    home.job(id).join("cancel.request")
}

/// Leaves the marker a runner watches for.
pub fn request_cancel(home: &Home, id: &str) -> Result<()> {
    let path = cancel_marker(home, id);
    std::fs::write(&path, now_rfc3339()).at(&path)
}

pub fn cancel_requested(home: &Home, id: &str) -> bool {
    cancel_marker(home, id).exists()
}

/// Jobs that name `id` as their parent.
pub fn children_of(home: &Home, id: &str) -> Vec<String> {
    let path = home.job(id).join("children.ndjson");
    std::fs::read_to_string(path)
        .map(|t| t.lines().map(str::to_string).filter(|l| !l.is_empty()).collect())
        .unwrap_or_default()
}

/// Records a child job under its parent, so that cancelling the parent reaches it.
pub fn add_child(home: &Home, parent: &str, child: &str) -> Result<()> {
    let path = home.job(parent).join("children.ndjson");
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&path).at(&path)?;
    writeln!(f, "{child}").at(&path)
}

pub fn remove_job(home: &Home, id: &str) -> Result<()> {
    let dir = home.job(id);
    retry(|| std::fs::remove_dir_all(&dir)).at(&dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_writes_replace_and_leave_no_temp_files() {
        let dir = std::env::temp_dir().join(format!("hufu-store-{}", hufu_contract::time::new_id("t")));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("state.json");
        write_json(&path, &serde_json::json!({"a": 1})).unwrap();
        write_json(&path, &serde_json::json!({"a": 2})).unwrap();
        let back: Value = read_json(&path).unwrap();
        assert_eq!(back["a"], 2);
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn states_know_when_they_are_terminal() {
        assert!(!State::Queued.is_terminal() && !State::Running.is_terminal());
        assert!(State::Lost.is_terminal() && State::TimedOut.is_terminal());
        assert_eq!(serde_json::to_string(&State::TimedOut).unwrap(), "\"timed_out\"");
    }
}
