//! The optional Hufu daemon. Without it every job's runner waits for its own turn; with it, queued
//! jobs are started in order, lost jobs are restarted when their retry policy allows, and old
//! records are pruned.
//!
//! The daemon keeps no state of its own: everything it knows is in the job store, so it can be
//! stopped and started at any time.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use hufu_contract::time::{now_rfc3339, parse_rfc3339};
use hufu_contract::{proc, reason};
use hufu_core::config::{self, Config, Home};
use hufu_core::error::{HufuError, Result};
use hufu_core::ops::{self, PruneOptions};
use hufu_core::slots;
use hufu_core::store::{self, State};
use hufu_core::submit::{self, DaemonInfo};

fn stop_marker(home: &Home) -> std::path::PathBuf {
    home.run().join("daemon.stop")
}

/// The running daemon, if any.
pub fn status(home: &Home) -> Option<DaemonInfo> {
    submit::daemon_info(home)
}

/// Starts queued jobs while there is room, oldest first. Returns how many were started.
pub fn start_queued(home: &Home, config: &Config, launched: &mut BTreeMap<String, Instant>) -> usize {
    let jobs = store::list_jobs(home);
    launched.retain(|id, at| {
        jobs.iter().any(|j| &j.id == id && j.state == State::Queued && j.runner_pid.is_none())
            && at.elapsed() < Duration::from_secs(30)
    });
    let mut busy = slots::live(home).iter().filter(|s| s.counted).count() + launched.len();
    let mut started = 0;
    for job in jobs
        .iter()
        .filter(|j| j.state == State::Queued && j.runner_pid.is_none() && j.queue.as_deref() == Some("daemon"))
    {
        if launched.contains_key(&job.id) {
            continue;
        }
        if store::cancel_requested(home, &job.id) {
            let _ = hufu_core::runner::cancel(home, &job.id);
            continue;
        }
        if busy >= config.limits.max_concurrent_jobs.max(1) {
            break;
        }
        if submit::launch(home, &job.id).is_ok() {
            launched.insert(job.id.clone(), Instant::now());
            busy += 1;
            started += 1;
        }
    }
    started
}

/// Puts jobs whose runner died back in the queue, when their retry policy has attempts left.
pub fn requeue_lost(home: &Home, since_ms: u64) -> usize {
    let mut count = 0;
    for mut job in store::list_jobs(home) {
        if job.state != State::Lost || job.reason.as_deref() != Some(reason::RUNNER_DIED) {
            continue;
        }
        if job.finished_at.as_deref().and_then(parse_rfc3339).unwrap_or(0) < since_ms {
            continue;
        }
        let Ok(spec) = store::load_spec(home, &job.id) else {
            continue;
        };
        if job.attempt >= spec.retry.max_attempts || store::cancel_requested(home, &job.id) {
            continue;
        }
        job.state = State::Queued;
        job.reason = None;
        job.detail = None;
        job.finished_at = None;
        job.runner_pid = None;
        job.agent_pid = None;
        job.heartbeat_at = None;
        job.queue = Some("daemon".into());
        if store::save_state(home, &job).is_ok() {
            count += 1;
        }
    }
    count
}

/// Runs the daemon in the foreground until it is asked to stop.
pub fn run(home: &Home, version: &str) -> Result<()> {
    home.ensure()?;
    if let Some(other) = status(home) {
        if other.pid != std::process::id() {
            return Err(HufuError::usage(format!("a daemon is already running (pid {})", other.pid)));
        }
    }
    let _ = std::fs::remove_file(stop_marker(home));
    let mut info = DaemonInfo {
        pid: std::process::id(),
        started_at: now_rfc3339(),
        version: version.to_string(),
        heartbeat_at: Some(now_rfc3339()),
    };
    store::write_json(&submit::daemon_file(home), &info)?;
    let mut beat = Instant::now();
    let started_ms = hufu_contract::time::now_ms();
    let mut launched = BTreeMap::new();
    let mut config = config::load(home, &home.root).map(|l| l.config).unwrap_or_default();
    let mut config_read = Instant::now();
    let mut pruned: Option<Instant> = None;
    loop {
        if stop_marker(home).exists() {
            break;
        }
        // Someone else took over the pid file: step aside.
        if store::read_json::<DaemonInfo>(&submit::daemon_file(home)).map(|d| d.pid != info.pid).unwrap_or(false) {
            return Ok(());
        }
        if beat.elapsed() > Duration::from_secs(3) {
            info.heartbeat_at = Some(now_rfc3339());
            let _ = store::write_json(&submit::daemon_file(home), &info);
            beat = Instant::now();
        }
        if config_read.elapsed() > Duration::from_secs(5) {
            if let Ok(loaded) = config::load(home, &home.root) {
                config = loaded.config;
            }
            config_read = Instant::now();
        }
        requeue_lost(home, started_ms);
        start_queued(home, &config, &mut launched);
        if pruned.map(|t| t.elapsed() > Duration::from_secs(3600)).unwrap_or(true) {
            let _ = ops::prune(home, &config, &PruneOptions::default());
            pruned = Some(Instant::now());
        }
        std::thread::sleep(Duration::from_millis(400));
    }
    let _ = std::fs::remove_file(submit::daemon_file(home));
    let _ = std::fs::remove_file(stop_marker(home));
    Ok(())
}

/// Starts the daemon in the background. Returns once it has written its pid file.
pub fn start(home: &Home) -> Result<DaemonInfo> {
    if let Some(info) = status(home) {
        return Ok(info);
    }
    home.ensure()?;
    let _ = std::fs::remove_file(stop_marker(home));
    submit::spawn_detached(home, &["daemon", "run"], &home.run().join("daemon.log"))?;
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(10) {
        if let Some(info) = status(home) {
            return Ok(info);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Err(HufuError::internal(format!("the daemon did not start; see {}", home.run().join("daemon.log").display())))
}

/// Stops the daemon. Returns whether one was running. Jobs keep running: each has its own runner.
pub fn stop(home: &Home) -> Result<bool> {
    let Some(info) = status(home) else {
        let _ = std::fs::remove_file(submit::daemon_file(home));
        return Ok(false);
    };
    std::fs::write(stop_marker(home), now_rfc3339()).map_err(|e| HufuError::internal(e.to_string()))?;
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(5) && proc::pid_alive(info.pid) {
        std::thread::sleep(Duration::from_millis(50));
    }
    if proc::pid_alive(info.pid) {
        proc::kill_tree(info.pid);
    }
    let _ = std::fs::remove_file(submit::daemon_file(home));
    let _ = std::fs::remove_file(stop_marker(home));
    Ok(true)
}
