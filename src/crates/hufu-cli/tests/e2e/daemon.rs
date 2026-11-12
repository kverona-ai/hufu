//! Queueing with and without the daemon.

use serde_json::{Value, json};

use crate::common::{Env, kill};

/// Stops the daemon when the test ends, however it ends.
struct Daemon<'a>(&'a Env);

impl Drop for Daemon<'_> {
    fn drop(&mut self) {
        let _ = self.0.hufu(&["daemon", "stop"]);
    }
}

fn state_file(env: &Env, job: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(env.job_file(job, "state.json")).unwrap_or_default()).unwrap_or(Value::Null)
}

#[test]
fn the_limit_on_concurrent_jobs_holds_without_a_daemon() {
    let env = Env::new("slots");
    env.config(json!({"limits": {"max_concurrent_jobs": 1}}));
    let first = env.start(&["-a", "codex", "[[fake:slow=1500]]"]);
    env.until(&first, "running", |s| s["state"] == "running");
    let second = env.start(&["-a", "agy", "second"]);
    std::thread::sleep(std::time::Duration::from_millis(400));
    assert_eq!(env.state(&second)["state"], "queued", "the second job must wait for a free slot");
    env.hufu(&["wait", &first, &second, "--timeout", "60s"]).expect(0);
    let (a, b) = (env.state(&first), env.state(&second));
    assert!(b["started_at"].as_str().unwrap() >= a["finished_at"].as_str().unwrap(), "{a} {b}");

    // A per-profile limit: two accounts may run side by side, one profile may not run twice.
    env.config(json!({"limits": {"max_concurrent_jobs": 4, "per_profile": {"codex": 1}}}));
    let one = env.start(&["-a", "codex", "[[fake:slow=1500]]"]);
    env.until(&one, "running", |s| s["state"] == "running");
    let two = env.start(&["-a", "codex", "again"]);
    let other = env.start(&["-a", "agy", "elsewhere"]);
    env.hufu(&["wait", &other, "--timeout", "60s"]).expect(0);
    assert_eq!(env.state(&one)["state"], "running", "another profile is not held back");
    assert_eq!(env.state(&two)["state"], "queued");
    env.hufu(&["wait", &one, &two, "--timeout", "60s"]).expect(0);

    // A queued job can be cancelled before it ever starts.
    env.config(json!({"limits": {"max_concurrent_jobs": 1}}));
    let busy = env.start(&["-a", "codex", "[[fake:hang]]"]);
    env.until(&busy, "running", |s| s["state"] == "running");
    let waiting = env.start(&["-a", "codex", "never starts"]);
    env.hufu(&["cancel", &waiting]).expect(0);
    env.hufu(&["wait", &waiting, "--timeout", "30s"]).expect(4);
    assert!(env.state(&waiting)["started_at"].is_null());
    env.hufu(&["cancel", &busy]).expect(0);
    env.hufu(&["wait", &busy, "--timeout", "30s"]).expect(4);
}

#[test]
fn the_daemon_starts_queued_jobs_in_order() {
    let env = Env::new("daemon");
    env.config(json!({"limits": {"max_concurrent_jobs": 1}}));
    assert_eq!(env.json(&["daemon", "status"]).1["running"], false);
    let _guard = Daemon(&env);
    let (code, started) = env.json(&["daemon", "start"]);
    assert_eq!(code, 0, "{started}");
    let pid = started["daemon"]["pid"].as_u64().expect("a pid");
    assert_eq!(env.json(&["daemon", "start"]).1["daemon"]["pid"], pid, "starting twice keeps the one daemon");
    assert_eq!(env.json(&["daemon", "status"]).1["running"], true);
    let r = env.hufu(&["daemon", "run"]);
    assert_eq!(r.code, 2, "a second daemon must refuse to run: {}", r.stderr);

    let submitted = env.hufu(&["--json", "run", "-a", "codex", "[[fake:slow=1200]]"]).expect(0).json();
    assert_eq!(submitted["queue"], "daemon", "{submitted}");
    let ids: Vec<String> = std::iter::once(submitted["job"].as_str().unwrap().to_string()).chain((0..3).map(|i| env.start(&["-a", "codex", &format!("job {i}")]))).collect();
    let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
    let mut wait = vec!["wait", "--timeout", "60s"];
    wait.extend(&refs);
    env.hufu(&wait).expect(0);
    let starts: Vec<String> = ids.iter().map(|id| env.state(id)["started_at"].as_str().unwrap().to_string()).collect();
    let mut sorted = starts.clone();
    sorted.sort();
    assert_eq!(starts, sorted, "jobs must start in the order they were submitted");

    // Stopping the daemon leaves running jobs alone, and later jobs start themselves again.
    let long = env.start(&["-a", "codex", "[[fake:slow=1500]]"]);
    env.until(&long, "running", |s| s["state"] == "running");
    let (_, stopped) = env.json(&["daemon", "stop"]);
    assert_eq!(stopped["stopped"], true);
    assert_eq!(env.json(&["daemon", "status"]).1["running"], false);
    assert_eq!(env.json(&["daemon", "stop"]).1["stopped"], false);
    env.hufu(&["wait", &long, "--timeout", "60s"]).expect(0);
    let (code, r) = env.run(&["-a", "codex", "after the daemon"]);
    assert_eq!((code, r["state"].as_str()), (0, Some("succeeded")));
}

#[test]
fn a_job_left_for_a_daemon_that_is_gone_still_runs() {
    let env = Env::new("daemon-gone");
    let _guard = Daemon(&env);
    let (_, started) = env.json(&["daemon", "start"]);
    let pid = started["daemon"]["pid"].as_u64().unwrap();
    // The daemon dies without cleaning up, between the submission and the start.
    kill(pid);
    let job = env.start(&["-a", "codex", "orphaned in the queue"]);
    env.hufu(&["wait", &job, "--timeout", "60s"]).expect(0);
    assert_eq!(env.state(&job)["state"], "succeeded");
}

#[test]
fn the_daemon_restarts_a_lost_job_that_may_be_retried() {
    let env = Env::new("daemon-lost");
    let _guard = Daemon(&env);
    env.hufu(&["daemon", "start"]).expect(0);
    let job = env.start(&["-a", "codex", "--retry", "2", "[[fake:hang]]"]);
    env.until(&job, "running", |s| s["state"] == "running" && s["last_activity"].is_string());
    let before = state_file(&env, &job);
    let (runner, agent) = (before["runner_pid"].as_u64().unwrap(), before["agent_pid"].as_u64().unwrap());
    kill(runner);
    kill(agent);
    let started = std::time::Instant::now();
    loop {
        let _ = env.state(&job); // reading the state is what notices the dead runner
        let now = state_file(&env, &job);
        if now["state"] == "running" && now["runner_pid"].as_u64().map(|p| p != runner).unwrap_or(false) && now["agent_pid"].is_u64() {
            break;
        }
        assert!(started.elapsed().as_secs() < 60, "the job was not restarted: {now}");
        std::thread::sleep(std::time::Duration::from_millis(150));
    }
    env.hufu(&["cancel", &job]).expect(0);
    env.hufu(&["wait", &job, "--timeout", "30s"]).expect(4);

    // Without attempts left a lost job stays lost.
    let once = env.start(&["-a", "codex", "[[fake:hang]]"]);
    env.until(&once, "running", |s| s["state"] == "running" && s["last_activity"].is_string());
    let s = state_file(&env, &once);
    kill(s["runner_pid"].as_u64().unwrap());
    kill(s["agent_pid"].as_u64().unwrap());
    env.hufu(&["wait", &once, "--timeout", "30s"]).expect(7);
    std::thread::sleep(std::time::Duration::from_millis(1200));
    assert_eq!(env.state(&once)["state"], "lost");
}
