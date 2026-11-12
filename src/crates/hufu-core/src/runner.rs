//! The runner: one process per job. It waits for its turn, starts the agent through an adapter,
//! watches time limits and cancellation, and writes down what happened.

use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use hufu_contract::time::{now_rfc3339, parse_duration, parse_rfc3339};
use hufu_contract::{
    AdapterRegistry, CancelReason, CancelToken, Event, EventBody, EventSink, FORMAT_VERSION, Profile, RawStream,
    RunOutcome, RunStatus, Tier, Usage, proc, reason,
};

use crate::config::{self, Config, Home, Isolation, Route};
use crate::error::{HufuError, Result};
use crate::spec::JobSpec;
use crate::store::{self, AgentInfo, Changes, HEARTBEAT_EVERY, JobResult, JobState, State, Timing, Verdict};
use crate::tally::{self, CheckOutcome, Evidence};
use crate::{schema, slots, submit, workspace};

/// The label that marks a job as a computer-use step carried out for another job.
pub const OPERATOR_LABEL: &str = "hufu.operator_for";

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

struct Shared<'a> {
    home: &'a Home,
    id: &'a str,
    state: Mutex<JobState>,
    dirty: AtomicBool,
    token: Mutex<CancelToken>,
    user_cancelled: AtomicBool,
    attempt_started: Mutex<Option<Instant>>,
    last_activity: Mutex<Instant>,
    done: AtomicBool,
    timeout: Duration,
    inactivity: Duration,
}

impl Shared<'_> {
    fn save(&self) {
        let mut state = lock(&self.state);
        state.heartbeat_at = Some(now_rfc3339());
        let _ = store::save_state(self.home, &state);
    }

    fn update(&self, change: impl FnOnce(&mut JobState)) {
        {
            let mut state = lock(&self.state);
            change(&mut state);
        }
        self.save();
    }

    fn token(&self) -> CancelToken {
        lock(&self.token).clone()
    }

    fn cancelled(&self) -> bool {
        self.user_cancelled.load(Ordering::SeqCst)
    }

    /// Sleeps unless the job is cancelled first. Returns false when it was.
    fn sleep(&self, total: Duration) -> bool {
        let until = Instant::now() + total;
        while Instant::now() < until {
            if self.cancelled() {
                return false;
            }
            std::thread::sleep(Duration::from_millis(100).min(until.saturating_duration_since(Instant::now())));
        }
        !self.cancelled()
    }

    fn has_running_children(&self) -> bool {
        store::children_of(self.home, self.id)
            .iter()
            .any(|c| store::load_state(self.home, c).map(|s| !s.state.is_terminal()).unwrap_or(false))
    }
}

fn watchdog(shared: &Shared) {
    let mut last_save = Instant::now();
    while !shared.done.load(Ordering::SeqCst) {
        std::thread::sleep(Duration::from_millis(150));
        if !shared.cancelled() && store::cancel_requested(shared.home, shared.id) {
            shared.user_cancelled.store(true, Ordering::SeqCst);
            shared.token().cancel(CancelReason::User);
            for child in store::children_of(shared.home, shared.id) {
                let _ = store::request_cancel(shared.home, &child);
            }
        }
        let started = *lock(&shared.attempt_started);
        if let Some(started) = started {
            if started.elapsed() > shared.timeout {
                shared.token().cancel(CancelReason::Timeout);
            } else if lock(&shared.last_activity).elapsed() > shared.inactivity {
                if shared.has_running_children() {
                    *lock(&shared.last_activity) = Instant::now();
                } else {
                    shared.token().cancel(CancelReason::Inactivity);
                }
            }
        }
        let dirty = shared.dirty.load(Ordering::SeqCst);
        if last_save.elapsed() >= HEARTBEAT_EVERY || (dirty && last_save.elapsed() >= Duration::from_secs(1)) {
            shared.dirty.store(false, Ordering::SeqCst);
            shared.save();
            last_save = Instant::now();
        }
    }
}

struct JobSink<'a, 'b> {
    shared: &'a Shared<'b>,
    events: Mutex<(Option<File>, u64)>,
    stdout: Mutex<Option<File>>,
    stderr: Mutex<Option<File>>,
}

fn append(path: &Path) -> Option<File> {
    std::fs::OpenOptions::new().create(true).append(true).open(path).ok()
}

impl<'a, 'b> JobSink<'a, 'b> {
    fn open(shared: &'a Shared<'b>) -> JobSink<'a, 'b> {
        let dir = shared.home.job(shared.id);
        let _ = std::fs::create_dir_all(dir.join("raw"));
        let seq = store::read_events(shared.home, shared.id, 0).last().map(|e| e.seq).unwrap_or(0);
        JobSink {
            shared,
            events: Mutex::new((append(&dir.join("events.ndjson")), seq)),
            stdout: Mutex::new(append(&dir.join("raw").join("stdout.log"))),
            stderr: Mutex::new(append(&dir.join("raw").join("stderr.log"))),
        }
    }
}

impl EventSink for JobSink<'_, '_> {
    fn emit(&self, body: EventBody) {
        *lock(&self.shared.last_activity) = Instant::now();
        let summary = match &body {
            EventBody::JobState { .. } | EventBody::Usage { .. } => None,
            other => Some(other.summary()),
        };
        {
            let mut guard = lock(&self.events);
            guard.1 += 1;
            let event = Event { v: FORMAT_VERSION, seq: guard.1, ts: now_rfc3339(), body };
            if let (Some(file), Ok(line)) = (guard.0.as_mut(), serde_json::to_string(&event)) {
                let _ = writeln!(file, "{line}");
            }
        }
        if let Some(text) = summary {
            let mut state = lock(&self.shared.state);
            state.last_activity = Some(text);
            state.last_activity_at = Some(now_rfc3339());
            self.shared.dirty.store(true, Ordering::SeqCst);
        }
    }

    fn raw(&self, stream: RawStream, line: &str) {
        *lock(&self.shared.last_activity) = Instant::now();
        let mut guard = match stream {
            RawStream::Stdout => lock(&self.stdout),
            RawStream::Stderr => lock(&self.stderr),
        };
        if let Some(file) = guard.as_mut() {
            let _ = writeln!(file, "{line}");
        }
    }

    fn child_started(&self, pid: u32) {
        lock(&self.shared.state).agent_pid = Some(pid);
        self.shared.dirty.store(true, Ordering::SeqCst);
    }
}

/// What a job ended with, before it is written down.
struct End {
    state: State,
    reason: Option<String>,
    detail: Option<String>,
    outcome: Option<RunOutcome>,
    profile: Profile,
    primary: bool,
    version: Option<String>,
    usage: Usage,
    attempts: u32,
    changes: Changes,
    tally: Vec<CheckOutcome>,
    verdict: Verdict,
    warnings: Vec<String>,
}

impl End {
    fn new(spec: &JobSpec) -> End {
        End {
            state: State::Failed,
            reason: None,
            detail: None,
            outcome: None,
            profile: spec.profile.clone(),
            primary: true,
            version: None,
            usage: Usage::default(),
            attempts: 0,
            changes: Changes::default(),
            tally: Vec::new(),
            verdict: Verdict::Unchecked,
            warnings: spec.warnings.clone(),
        }
    }

    fn fail(mut self, why: &str, detail: impl Into<String>) -> End {
        self.state = State::Failed;
        self.reason = Some(why.to_string());
        self.detail = Some(detail.into());
        self
    }

    fn cancelled(mut self) -> End {
        self.state = State::Cancelled;
        self.reason = Some(reason::CANCELLED.to_string());
        self.detail = None;
        self
    }
}

fn wants<'a>(config: &Config, spec: &'a JobSpec, profile: &'a Profile) -> slots::Want<'a> {
    let operator = spec.labels.contains_key(OPERATOR_LABEL);
    let own_display = spec.computer_use.as_ref().map(|p| p.route == Route::Executor).unwrap_or(false);
    slots::Want {
        job: &spec.id,
        profile: &profile.name,
        // A computer-use step runs while its parent holds a slot; counting it would deadlock a full queue.
        counted: !operator,
        display: operator || own_display,
        max_jobs: config.limits.max_concurrent_jobs.max(1),
        max_for_profile: config.limits.per_profile.get(&profile.name).copied(),
        max_display: config.capabilities.computer_use.max_concurrent,
    }
}

fn acquire(shared: &Shared, config: &Config, spec: &JobSpec, profile: &Profile) -> Option<slots::Slot> {
    let want = wants(config, spec, profile);
    let mut told = false;
    loop {
        if shared.cancelled() {
            return None;
        }
        if let Some(slot) = slots::try_acquire(shared.home, &want) {
            return Some(slot);
        }
        if !told {
            told = true;
            shared.update(|s| {
                s.last_activity = Some("waiting for a free slot".into());
                s.last_activity_at = Some(now_rfc3339());
            });
        }
        std::thread::sleep(Duration::from_millis(300));
    }
}

/// Failures that mean "this account or installation cannot serve right now", so another member of
/// a pool should be tried.
fn fails_over(why: &str) -> bool {
    matches!(why, reason::AUTH_REQUIRED | reason::AGENT_UNAVAILABLE | reason::RATE_LIMITED)
}

fn drive(shared: &Shared, sink: &JobSink, spec: &JobSpec, config: &Config, adapters: &dyn AdapterRegistry) -> End {
    let home = shared.home;
    let mut end = End::new(spec);

    // 1. Dependencies.
    for dep in &spec.depends_on {
        loop {
            if shared.cancelled() {
                return end.cancelled();
            }
            match store::load_state(home, dep) {
                Ok(d) if d.state.is_terminal() => {
                    if d.state != State::Succeeded || d.verdict == Verdict::Rejected {
                        let how = if d.state == State::Succeeded {
                            "was rejected by its tally check".to_string()
                        } else {
                            format!("ended as {}", d.state.as_str())
                        };
                        return end.fail(reason::DEPENDENCY_FAILED, format!("dependency `{dep}` {how}"));
                    }
                    break;
                }
                Ok(_) => std::thread::sleep(Duration::from_millis(400)),
                Err(_) => {
                    return end.fail(reason::DEPENDENCY_FAILED, format!("dependency `{dep}` no longer exists"));
                }
            }
        }
    }

    // 2. A slot.
    let mut members: Vec<&Profile> = vec![&spec.profile];
    members.extend(spec.alternates.iter());
    let mut member = 0usize;
    let Some(mut slot) = acquire(shared, config, spec, members[member]) else {
        return end.cancelled();
    };
    shared.update(|s| {
        s.state = State::Starting;
        s.started_at = Some(now_rfc3339());
    });
    sink.emit(EventBody::JobState { state: State::Starting.as_str().into(), reason: None });

    // 3. The workspace.
    let mut worktree = None;
    let mut before = None;
    let run_dir: PathBuf = if spec.isolation == Isolation::Worktree {
        match workspace::create_worktree(home, &spec.cwd, &spec.id) {
            Ok(wt) => {
                let dir = wt.run_dir.clone();
                shared.update(|s| {
                    s.workspace = Some(store::WorkspaceInfo {
                        path: dir.display().to_string(),
                        repo: Some(wt.repo.display().to_string()),
                        branch: Some(wt.branch.clone()),
                        base: Some(wt.base.clone()),
                        worktree: true,
                    });
                });
                worktree = Some(wt);
                dir
            }
            Err(e) => return end.fail(reason::WORKSPACE_ERROR, e.message),
        }
    } else {
        before = workspace::snapshot(&spec.cwd);
        shared.update(|s| {
            s.workspace = Some(store::WorkspaceInfo {
                path: spec.cwd.display().to_string(),
                repo: workspace::repo_root(&spec.cwd).map(|r| r.display().to_string()),
                branch: None,
                base: None,
                worktree: false,
            });
        });
        spec.cwd.clone()
    };

    // 4. Attempts.
    let backoff = parse_duration(&spec.retry.backoff).unwrap_or(Duration::from_secs(15));
    let mut tries_on_member = 0u32;
    let mut outcome = loop {
        let profile = members[member];
        end.attempts += 1;
        tries_on_member += 1;
        end.profile = profile.clone();
        end.primary = member == 0;
        let Some(adapter) = adapters.get(&profile.adapter) else {
            break RunOutcome::failed(reason::UNSUPPORTED, format!("unknown adapter `{}`", profile.adapter));
        };
        let probe = adapter.probe(profile);
        end.version = probe.version.clone();
        let token = CancelToken::new();
        *lock(&shared.token) = token.clone();
        if shared.cancelled() {
            token.cancel(CancelReason::User);
        }
        shared.update(|s| {
            s.state = State::Running;
            s.attempt = end.attempts;
            s.profile = profile.name.clone();
            s.adapter = profile.adapter.clone();
            s.agent_version = probe.version.clone();
            s.agent_pid = None;
        });
        sink.emit(EventBody::JobState { state: State::Running.as_str().into(), reason: None });

        let missing = submit::missing_env(profile);
        let outcome = if !missing.is_empty() {
            RunOutcome::failed(
                reason::AUTH_REQUIRED,
                format!(
                    "profile `{}` reads {} from the environment, but it is not set",
                    profile.name,
                    missing.join(", ")
                ),
            )
        } else {
            match submit::run_request(home, adapters, spec, profile, member == 0, &run_dir) {
                Err(e) => RunOutcome::failed(reason::INTERNAL, e.message),
                Ok(req) => {
                    *lock(&shared.last_activity) = Instant::now();
                    *lock(&shared.attempt_started) = Some(Instant::now());
                    let out = adapter.run(profile, &req, sink, &token);
                    *lock(&shared.attempt_started) = None;
                    out
                }
            }
        };
        if let Some(u) = &outcome.usage {
            end.usage.add(u);
        }
        if token.is_cancelled() || outcome.status != RunStatus::Failed {
            break outcome;
        }
        let why = outcome.reason.clone().unwrap_or_default();
        let detail = outcome.detail.clone().unwrap_or_default();
        if fails_over(&why) && member + 1 < members.len() {
            member += 1;
            tries_on_member = 0;
            let next = members[member];
            sink.emit(EventBody::warning(format!(
                "profile `{}` cannot serve ({why}: {}); switching to `{}`",
                profile.name,
                proc::tail(&detail, 200),
                next.name
            )));
            end.warnings
                .push(format!("profile `{}` could not serve ({why}); the job ran on `{}`", profile.name, next.name));
            drop(slot);
            match acquire(shared, config, spec, next) {
                Some(s) => slot = s,
                None => return end.cancelled(),
            }
            continue;
        }
        if tries_on_member < spec.retry.max_attempts && spec.retry.on.iter().any(|r| r == &why) {
            sink.emit(EventBody::warning(format!(
                "attempt {} failed ({why}); retrying in {}s",
                end.attempts,
                backoff.as_secs()
            )));
            if !shared.sleep(backoff) {
                break outcome;
            }
            continue;
        }
        break outcome;
    };
    let token = shared.token();

    // 5. What changed.
    let mut files: Vec<String> = match (&worktree, &before) {
        (Some(wt), _) => workspace::changed_since(&wt.path, &wt.base),
        (None, Some(b)) => {
            workspace::snapshot(&spec.cwd).map(|after| workspace::changed(b, &after)).unwrap_or_default()
        }
        // Without a snapshot, what the agent reported is all there is. Agents name paths either way.
        (None, None) => outcome.touched_paths.iter().map(|p| workspace::relative_to(&run_dir, p)).collect(),
    };
    files.sort();
    files.dedup();
    end.changes = Changes { files: files.clone(), branch: None, base: None, worktree: None };
    if let Some(wt) = &worktree {
        if files.is_empty() {
            // Nothing to keep: remove the copy and its branch.
            workspace::remove_worktree(&wt.repo, &wt.path);
            let _ = workspace::git(&wt.repo, &["branch", "-D", &wt.branch]);
        } else {
            workspace::commit_all(&wt.path, &format!("hufu: {} ({})", spec.title, spec.id));
            end.changes.branch = Some(wt.branch.clone());
            end.changes.base = Some(wt.base.clone());
            end.changes.worktree = Some(wt.path.display().to_string());
        }
    }

    // 6. The verdict on the run itself.
    end.state = match (token.reason(), outcome.status) {
        (Some(CancelReason::User), _) => {
            end.reason = Some(reason::CANCELLED.into());
            State::Cancelled
        }
        (Some(CancelReason::Timeout), _) => {
            end.reason = Some(reason::TIMEOUT.into());
            end.detail = Some(format!("the job ran longer than its time limit of {}s", shared.timeout.as_secs()));
            State::TimedOut
        }
        (Some(CancelReason::Inactivity), _) => {
            end.reason = Some(reason::INACTIVITY.into());
            end.detail = Some(format!("the agent produced no output for {}s", shared.inactivity.as_secs()));
            State::TimedOut
        }
        (None, RunStatus::Succeeded) => State::Succeeded,
        (None, RunStatus::Cancelled) => {
            end.reason = Some(reason::CANCELLED.into());
            end.detail = outcome.detail.clone();
            State::Cancelled
        }
        (None, RunStatus::Failed) => {
            end.reason = outcome.reason.clone().or_else(|| Some(reason::AGENT_ERROR.into()));
            end.detail = outcome.detail.clone();
            State::Failed
        }
    };
    if !outcome.denied_actions.is_empty() && end.state == State::Succeeded {
        let list: Vec<String> = outcome.denied_actions.iter().map(|d| format!("{} {}", d.kind, d.tool)).collect();
        if spec.strict_permissions {
            end.state = State::Failed;
            end.reason = Some(reason::PERMISSION_DENIED.into());
            end.detail = Some(format!("the agent skipped actions for lack of permission: {}", list.join(", ")));
        } else {
            end.warnings.push(format!(
                "the agent skipped {} action(s) for lack of permission; see `denied_actions`",
                list.len()
            ));
        }
    }
    let other_writers = if spec.permission == Tier::ReadOnly && !files.is_empty() && worktree.is_none() {
        overlapping_writers(shared, spec)
    } else {
        Vec::new()
    };
    if spec.permission == Tier::ReadOnly && !files.is_empty() && !other_writers.is_empty() {
        // Another job was writing here at the same time, so the changes cannot be pinned on this one.
        end.warnings.push(format!("{} file(s) changed during this read-only job, while {} was writing to the same workspace; the read-only check could not be applied", files.len(), other_writers.join(", ")));
        end.changes.files.clear();
        files.clear();
    } else if spec.permission == Tier::ReadOnly && !files.is_empty() {
        let shown: Vec<&str> = files.iter().take(10).map(String::as_str).collect();
        end.state = State::Failed;
        end.reason = Some(reason::TIER_VIOLATED.into());
        end.detail = Some(format!(
            "the job ran at `read-only`, but {} file(s) in the workspace changed during it: {}{}",
            files.len(),
            shown.join(", "),
            if files.len() > shown.len() { ", …" } else { "" }
        ));
    }
    if end.state == State::Succeeded {
        if let Some(wanted) = &spec.output_schema {
            match outcome.structured_output.as_mut() {
                None => {
                    end.state = State::Failed;
                    end.reason = Some(reason::INVALID_OUTPUT.into());
                    end.detail =
                        Some("the task asked for structured output, but the final answer contains no JSON".into());
                }
                Some(value) => {
                    schema::fill_nullable(wanted, value);
                    let errors = schema::validate(wanted, value);
                    if !errors.is_empty() {
                        end.state = State::Failed;
                        end.reason = Some(reason::INVALID_OUTPUT.into());
                        end.detail =
                            Some(format!("the structured output does not match the schema: {}", errors.join("; ")));
                    }
                }
            }
        }
    }

    // 7. The tally check.
    if end.state == State::Succeeded && !spec.acceptance.is_empty() {
        shared.update(|s| {
            s.last_activity = Some("running acceptance checks".into());
            s.last_activity_at = Some(now_rfc3339());
        });
        let evidence = Evidence {
            workspace: &run_dir,
            changed_files: &files,
            structured_output: outcome.structured_output.as_ref(),
            denied_actions: &outcome.denied_actions,
        };
        end.tally = tally::run(&spec.acceptance, &evidence);
        end.verdict = if tally::all_passed(&end.tally) { Verdict::Accepted } else { Verdict::Rejected };
        let _ = store::write_json(&home.job(&spec.id).join("tally.json"), &end.tally);
    }
    end.outcome = Some(outcome);
    drop(slot);
    end
}

/// Jobs that could write to this job's workspace while it ran.
fn overlapping_writers(shared: &Shared, spec: &JobSpec) -> Vec<String> {
    let started = lock(&shared.state).started_at.as_deref().and_then(parse_rfc3339).unwrap_or(0);
    let here = workspace::repo_root(&spec.cwd).unwrap_or_else(|| spec.cwd.clone());
    store::list_jobs(shared.home)
        .into_iter()
        .filter(|j| j.id != spec.id && j.permission >= Tier::WorkspaceWrite)
        .filter(|j| {
            j.finished_at.as_deref().and_then(parse_rfc3339).map(|end| end >= started).unwrap_or(j.started_at.is_some())
        })
        .filter(|j| {
            let path = j.workspace.as_ref().map(|w| PathBuf::from(&w.path)).unwrap_or_else(|| PathBuf::from(&j.cwd));
            path.starts_with(&here) || here.starts_with(&path)
        })
        .map(|j| j.id)
        .collect()
}

fn write_end(shared: &Shared, sink: &JobSink, spec: &JobSpec, end: End) -> JobState {
    let home = shared.home;
    // Computer use is scoped to the job: no step outlives it.
    for child in store::children_of(home, &spec.id) {
        if store::load_state(home, &child).map(|s| !s.state.is_terminal()).unwrap_or(false) {
            let _ = store::request_cancel(home, &child);
        }
    }
    let finished = now_rfc3339();
    let (effort, model) = if end.primary {
        (spec.effort.clone(), spec.model.clone())
    } else {
        (end.profile.effort.clone().unwrap_or_default(), end.profile.model.clone())
    };
    let outcome = end.outcome;
    let state = {
        let mut state = lock(&shared.state);
        state.state = end.state;
        state.verdict = end.verdict;
        state.reason = end.reason.clone();
        state.detail = end.detail.clone();
        state.finished_at = Some(finished.clone());
        state.agent_pid = None;
        state.clone()
    };
    let started_ms = state.started_at.as_deref().and_then(parse_rfc3339);
    let finished_ms = parse_rfc3339(&finished).unwrap_or(0);
    let result = JobResult {
        v: FORMAT_VERSION,
        job: spec.id.clone(),
        state: end.state,
        verdict: end.verdict,
        reason: end.reason.clone(),
        detail: end.detail.clone(),
        title: spec.title.clone(),
        agent: AgentInfo {
            profile: end.profile.name.clone(),
            adapter: end.profile.adapter.clone(),
            account: end.profile.account.clone(),
            version: end.version.clone(),
            model,
            effort,
            permission: Some(spec.permission),
        },
        final_message: outcome.as_ref().and_then(|o| o.final_message.clone()),
        structured_output: outcome.as_ref().and_then(|o| o.structured_output.clone()),
        changes: end.changes,
        denied_actions: outcome.as_ref().map(|o| o.denied_actions.clone()).unwrap_or_default(),
        warnings: end.warnings,
        usage: (!end.usage.is_empty()).then_some(end.usage),
        session: outcome.as_ref().and_then(|o| o.session.clone()),
        timing: Timing {
            queued_at: state.created_at.clone(),
            started_at: state.started_at.clone(),
            finished_at: finished,
            duration_s: started_ms.map(|s| finished_ms.saturating_sub(s) / 1000).unwrap_or(0),
        },
        tally: end.tally,
        attempts: end.attempts,
        children: store::children_of(home, &spec.id),
        mission: spec.mission.clone(),
    };
    // The result is written before the state turns terminal, so a waiter never sees one without the other.
    let _ = store::write_result(home, &result);
    sink.emit(EventBody::JobState { state: end.state.as_str().into(), reason: end.reason.clone() });
    shared.save();
    state
}

/// Runs one job to its end. This is the body of `hufu __runner <job>`.
pub fn run_job(home: &Home, adapters: &dyn AdapterRegistry, id: &str) -> Result<JobState> {
    let spec = store::load_spec(home, id)?;
    let mut state = store::load_state(home, id)?;
    if state.state.is_terminal() {
        return Ok(state);
    }
    let me = std::process::id();
    if let Some(pid) = state.runner_pid {
        if pid != me && proc::pid_alive(pid) {
            return Err(HufuError::usage(format!("job `{id}` already has a runner (pid {pid})")));
        }
    }
    state.runner_pid = Some(me);
    state.queue = None;
    state.heartbeat_at = Some(now_rfc3339());
    store::save_state(home, &state)?;
    let config = config::load(home, &spec.cwd).map(|l| l.config).unwrap_or_default();

    let shared = Shared {
        home,
        id,
        state: Mutex::new(state),
        dirty: AtomicBool::new(false),
        token: Mutex::new(CancelToken::new()),
        user_cancelled: AtomicBool::new(store::cancel_requested(home, id)),
        attempt_started: Mutex::new(None),
        last_activity: Mutex::new(Instant::now()),
        done: AtomicBool::new(false),
        timeout: Duration::from_secs(spec.timeout_s),
        inactivity: Duration::from_secs(spec.inactivity_timeout_s),
    };
    let sink = JobSink::open(&shared);
    for warning in &spec.warnings {
        sink.emit(EventBody::warning(warning.clone()));
    }
    let end = std::thread::scope(|scope| {
        scope.spawn(|| watchdog(&shared));
        let end = drive(&shared, &sink, &spec, &config, adapters);
        let state = write_end(&shared, &sink, &spec, end);
        shared.done.store(true, Ordering::SeqCst);
        state
    });
    Ok(end)
}

/// A result for a job that ended without its runner writing one (lost, or cancelled before start).
fn synthesize(spec: &JobSpec, state: &JobState) -> JobResult {
    let finished = state.finished_at.clone().unwrap_or_else(now_rfc3339);
    JobResult {
        v: FORMAT_VERSION,
        job: state.id.clone(),
        state: state.state,
        verdict: state.verdict,
        reason: state.reason.clone(),
        detail: state.detail.clone(),
        title: state.title.clone(),
        agent: AgentInfo {
            profile: state.profile.clone(),
            adapter: state.adapter.clone(),
            account: spec.profile.account.clone(),
            version: state.agent_version.clone(),
            model: spec.model.clone(),
            effort: spec.effort.clone(),
            permission: Some(spec.permission),
        },
        final_message: None,
        structured_output: None,
        changes: Changes::default(),
        denied_actions: Vec::new(),
        warnings: spec.warnings.clone(),
        usage: None,
        session: None,
        timing: Timing {
            queued_at: state.created_at.clone(),
            started_at: state.started_at.clone(),
            finished_at: finished,
            duration_s: state.elapsed_s(),
        },
        tally: Vec::new(),
        attempts: state.attempt,
        children: Vec::new(),
        mission: state.mission.clone(),
    }
}

/// The result envelope of a finished job. `None` while the job is still going.
pub fn result_of(home: &Home, id: &str) -> Result<Option<JobResult>> {
    let state = store::load_state(home, id)?;
    if !state.state.is_terminal() {
        return Ok(None);
    }
    if let Some(result) = store::load_result(home, id)? {
        // A job that was lost after its result was written keeps the result but reports the state.
        return Ok(Some(result));
    }
    let spec = store::load_spec(home, id)?;
    Ok(Some(synthesize(&spec, &state)))
}

/// Asks a job to stop. A job that has no runner yet is ended on the spot.
pub fn cancel(home: &Home, id: &str) -> Result<JobState> {
    let mut state = store::load_state(home, id)?;
    if state.state.is_terminal() {
        return Ok(state);
    }
    store::request_cancel(home, id)?;
    for child in store::children_of(home, id) {
        let _ = store::request_cancel(home, &child);
    }
    if state.runner_pid.is_none() {
        state.state = State::Cancelled;
        state.reason = Some(reason::CANCELLED.into());
        state.finished_at = Some(now_rfc3339());
        store::save_state(home, &state)?;
    }
    Ok(state)
}

/// Stops a job whose runner does not react: kills the agent's process tree and the runner.
pub fn force_cancel(home: &Home, id: &str) -> Result<JobState> {
    let mut state = store::load_state(home, id)?;
    if state.state.is_terminal() {
        return Ok(state);
    }
    let _ = store::request_cancel(home, id);
    if let Some(pid) = state.agent_pid {
        proc::kill_tree(pid);
    }
    if let Some(pid) = state.runner_pid.filter(|p| *p != std::process::id()) {
        proc::kill_tree(pid);
    }
    state.state = State::Cancelled;
    state.reason = Some(reason::CANCELLED.into());
    state.detail = Some("stopped by force; the agent's processes were killed".into());
    state.finished_at = Some(now_rfc3339());
    state.agent_pid = None;
    store::save_state(home, &state)?;
    if let Ok(entries) = std::fs::read_dir(home.slots()) {
        for entry in entries.flatten() {
            if entry.file_name().to_string_lossy().starts_with(id) {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
    Ok(state)
}

/// Blocks until a job reaches a terminal state or the time runs out. Returns the last state seen.
pub fn wait(home: &Home, id: &str, limit: Option<Duration>) -> Result<JobState> {
    let started = Instant::now();
    let mut pause = Duration::from_millis(50);
    submit::kick(home, id);
    loop {
        let state = store::load_state(home, id)?;
        if state.state.is_terminal() {
            return Ok(state);
        }
        if let Some(limit) = limit {
            if started.elapsed() >= limit {
                return Ok(state);
            }
            pause = pause.min(limit.saturating_sub(started.elapsed()).max(Duration::from_millis(10)));
        }
        std::thread::sleep(pause);
        pause = (pause * 2).min(Duration::from_millis(500));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{check_raw, parse_yaml};
    use crate::spec::TaskSpec;
    use crate::submit::{Context, Options};
    use hufu_contract::{
        Adapter, AdapterError, Capabilities, DeniedAction, LaunchPlan, ProbeResult, RunRequest, SessionRef,
        StructuredOutput,
    };
    use serde_json::json;

    /// An in-process agent whose behaviour is chosen by the profile's `model`.
    struct Scripted;
    impl Adapter for Scripted {
        fn kind(&self) -> &'static str {
            "scripted"
        }
        fn default_command(&self) -> Vec<String> {
            Vec::new()
        }
        fn probe(&self, _: &Profile) -> ProbeResult {
            ProbeResult { installed: true, version: Some("scripted 1.0".into()), ..ProbeResult::default() }
        }
        fn capabilities(&self, _: &Profile) -> Capabilities {
            Capabilities {
                permission_tiers: Tier::ALL.to_vec(),
                efforts: vec![hufu_contract::Effort::Low, hufu_contract::Effort::High],
                native_effort: false,
                checked_at_runtime: false,
                models: Vec::new(),
                resume: true,
                structured_output: StructuredOutput::Instructed,
                reports_usage: true,
                reports_denials: true,
                accepts_mcp_servers: true,
                provides: Vec::new(),
                notes: Vec::new(),
            }
        }
        fn plan(&self, _: &Profile, _: &RunRequest) -> std::result::Result<LaunchPlan, AdapterError> {
            Ok(LaunchPlan::default())
        }
        fn run(&self, profile: &Profile, req: &RunRequest, sink: &dyn EventSink, cancel: &CancelToken) -> RunOutcome {
            sink.emit(EventBody::tool("t1", "read", "README.md", "completed"));
            let mut out = RunOutcome { status: RunStatus::Succeeded, reason: None, ..RunOutcome::failed("", "") };
            out.detail = None;
            out.usage = Some(Usage { input_tokens: Some(10), output_tokens: Some(5), ..Usage::default() });
            out.session = Some(SessionRef { id: "s-1".into(), resumable: true });
            match profile.model.as_deref().unwrap_or("ok") {
                "auth" => return RunOutcome::failed(reason::AUTH_REQUIRED, "not logged in"),
                "hang" => {
                    while !cancel.is_cancelled() {
                        std::thread::sleep(Duration::from_millis(20));
                    }
                    out.status = RunStatus::Cancelled;
                }
                "write" => {
                    std::fs::write(req.cwd.join("made-by-agent.txt"), "hello\n").unwrap();
                    out.touched_paths = vec!["made-by-agent.txt".into()];
                    out.final_message = Some("wrote a file".into());
                }
                "deny" => {
                    out.final_message = Some("could not run the tests".into());
                    out.denied_actions =
                        vec![DeniedAction { tool: "npm test".into(), kind: "execute".into(), detail: String::new() }];
                }
                "json" => {
                    out.final_message = Some("{\"ok\": true}".into());
                    out.structured_output = Some(json!({"ok": true}));
                }
                _ => out.final_message = Some(format!("done: {}", req.prompt)),
            }
            out
        }
    }
    struct Reg;
    impl AdapterRegistry for Reg {
        fn get(&self, kind: &str) -> Option<&dyn Adapter> {
            (kind == "scripted").then_some(&Scripted as &dyn Adapter)
        }
        fn kinds(&self) -> Vec<&'static str> {
            vec!["scripted"]
        }
    }

    struct Bench {
        home: Home,
        dir: PathBuf,
        config: Config,
    }

    fn bench() -> Bench {
        let dir = std::env::temp_dir().join(format!("hufu-runner-{}", hufu_contract::time::new_id("t")));
        let work = dir.join("work");
        std::fs::create_dir_all(&work).unwrap();
        let home = Home::at(dir.join("home"));
        home.ensure().unwrap();
        let yaml = "profiles:\n  ok: {adapter: scripted}\n  auth: {adapter: scripted, model: auth, account: a}\n  hang: {adapter: scripted, model: hang}\n  write: {adapter: scripted, model: write}\n  deny: {adapter: scripted, model: deny}\n  json: {adapter: scripted, model: json}\npools:\n  team: {members: [auth, ok]}\n";
        std::fs::write(home.config_file(), yaml).unwrap();
        let mut config: Config = serde_json::from_value(parse_yaml(yaml).unwrap()).unwrap();
        for (name, p) in config.profiles.iter_mut() {
            p.name = name.clone();
        }
        assert!(check_raw(&parse_yaml(yaml).unwrap(), &Reg).is_err(), "presets use adapters this registry lacks");
        Bench { home, dir: work, config }
    }

    fn run(b: &Bench, yaml: &str) -> (JobState, JobResult) {
        let task = TaskSpec::from_text(yaml).unwrap();
        let ctx = Context { home: &b.home, config: &b.config, adapters: &Reg };
        let opts = Options { base_dir: b.dir.clone(), hold: true, ..Options::default() };
        let queued = submit::submit(&ctx, &task, &opts).unwrap();
        let state = run_job(&b.home, &Reg, &queued.id).unwrap();
        let result = result_of(&b.home, &queued.id).unwrap().unwrap();
        (state, result)
    }

    #[test]
    fn a_job_succeeds_and_leaves_a_complete_record() {
        let b = bench();
        let (state, result) = run(&b, "prompt: say hi\nagent: ok\n");
        assert_eq!(state.state, State::Succeeded);
        assert_eq!(result.final_message.as_deref(), Some("done: say hi"));
        assert_eq!(result.agent.version.as_deref(), Some("scripted 1.0"));
        assert_eq!(result.usage.unwrap().tokens(), 15);
        assert_eq!(result.verdict, Verdict::Unchecked);
        let events = store::read_events(&b.home, &state.id, 0);
        let kinds: Vec<String> = events.iter().map(|e| e.body.summary()).collect();
        assert!(
            kinds.first().unwrap().contains("starting") && kinds.last().unwrap().contains("succeeded"),
            "{kinds:?}"
        );
        assert!(events.windows(2).all(|w| w[0].seq + 1 == w[1].seq));
        assert!(slots::live(&b.home).is_empty(), "the slot is released");
    }

    #[test]
    fn a_pool_fails_over_to_the_next_account() {
        let b = bench();
        let (state, result) = run(&b, "prompt: x\nagent: team\n");
        assert_eq!(state.state, State::Succeeded);
        assert_eq!(state.agent, "team");
        assert_eq!(result.agent.profile, "ok");
        assert_eq!(result.attempts, 2);
        assert!(result.warnings.iter().any(|w| w.contains("`auth` could not serve")));
        let (alone, _) = run(&b, "prompt: x\nagent: auth\n");
        assert_eq!((alone.state, alone.reason.as_deref()), (State::Failed, Some("auth_required")));
    }

    #[test]
    fn time_limits_and_cancellation_end_a_hanging_agent() {
        let b = bench();
        let (state, _) = run(&b, "prompt: x\nagent: hang\ntimeout: 1s\n");
        assert_eq!((state.state, state.reason.as_deref()), (State::TimedOut, Some("timeout")));

        let task = TaskSpec::from_text("prompt: x\nagent: hang\n").unwrap();
        let ctx = Context { home: &b.home, config: &b.config, adapters: &Reg };
        let queued =
            submit::submit(&ctx, &task, &Options { base_dir: b.dir.clone(), hold: true, ..Options::default() })
                .unwrap();
        let (home, id) = (b.home.clone(), queued.id.clone());
        let canceller = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(400));
            cancel(&home, &id).unwrap();
        });
        let state = run_job(&b.home, &Reg, &queued.id).unwrap();
        canceller.join().unwrap();
        assert_eq!(state.state, State::Cancelled);

        // A job nobody started is cancelled on the spot.
        let idle = submit::submit(&ctx, &task, &Options { base_dir: b.dir.clone(), hold: true, ..Options::default() })
            .unwrap();
        assert_eq!(cancel(&b.home, &idle.id).unwrap().state, State::Cancelled);
        assert_eq!(result_of(&b.home, &idle.id).unwrap().unwrap().state, State::Cancelled);
    }

    #[test]
    fn tally_checks_decide_the_verdict() {
        let b = bench();
        let (state, result) = run(
            &b,
            "prompt: x\nagent: write\npermission: workspace-write\nacceptance:\n  - command: exit 0\n  - command: exit 3\n",
        );
        assert_eq!(state.state, State::Succeeded);
        assert_eq!(state.verdict, Verdict::Rejected);
        assert_eq!(result.tally.iter().map(|t| t.passed).collect::<Vec<_>>(), [true, false]);
        assert_eq!(result.changes.files, ["made-by-agent.txt"], "outside git, the agent's own report is used");
        let (ok, _) = run(
            &b,
            "prompt: x\nagent: json\noutput:\n  schema: {type: object, required: [ok]}\nacceptance:\n  - schema: {type: object}\n  - no_denied_actions: true\n",
        );
        assert_eq!((ok.state, ok.verdict), (State::Succeeded, Verdict::Accepted));
        let (bad, _) = run(&b, "prompt: x\nagent: json\noutput:\n  schema: {type: object, required: [missing]}\n");
        assert_eq!((bad.state, bad.reason.as_deref()), (State::Failed, Some("invalid_output")));
    }

    #[test]
    fn permission_boundaries_are_checked_after_the_run() {
        let b = bench();
        let (lenient, result) = run(&b, "prompt: x\nagent: deny\n");
        assert_eq!(lenient.state, State::Succeeded);
        assert_eq!(result.denied_actions.len(), 1);
        let (strict, _) = run(&b, "prompt: x\nagent: deny\nstrict_permissions: true\n");
        assert_eq!((strict.state, strict.reason.as_deref()), (State::Failed, Some("permission_denied")));
        let (violated, _) = run(&b, "prompt: x\nagent: write\n");
        assert_eq!(
            (violated.state, violated.reason.as_deref()),
            (State::Failed, Some("tier_violated")),
            "a read-only job that changed files fails"
        );
    }

    #[test]
    fn dependencies_and_worktrees() {
        let b = bench();
        let (failed, _) = run(&b, "prompt: x\nagent: auth\n");
        let (blocked, _) = run(&b, &format!("prompt: x\nagent: ok\ndepends_on: [{}]\n", failed.id));
        assert_eq!((blocked.state, blocked.reason.as_deref()), (State::Failed, Some("dependency_failed")));

        let git = |args: &[&str]| workspace::git(&b.dir, args).map(|c| c.ok()).unwrap_or(false);
        if !(git(&["init", "-q"])
            && std::fs::write(b.dir.join("a.txt"), "a\n").is_ok()
            && git(&["add", "."])
            && git(&["-c", "user.name=t", "-c", "user.email=t@example.invalid", "commit", "-q", "-m", "init"]))
        {
            return;
        }
        let (state, result) = run(&b, "prompt: x\nagent: write\npermission: workspace-write\nisolation: worktree\n");
        assert_eq!(state.state, State::Succeeded);
        assert_eq!(result.changes.files, ["made-by-agent.txt"]);
        assert_eq!(result.changes.branch.as_deref(), Some(format!("hufu/{}", state.id).as_str()));
        assert!(!b.dir.join("made-by-agent.txt").exists(), "the main tree is untouched");
        let (clean, result) = run(&b, "prompt: x\nagent: ok\nisolation: worktree\n");
        assert_eq!(clean.state, State::Succeeded);
        assert!(
            result.changes.branch.is_none() && !b.home.worktrees().join(&clean.id).exists(),
            "an unchanged copy is removed"
        );
    }
}
