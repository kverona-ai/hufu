//! Managed missions: an advisor plans, executors work, the advisor reviews, Hufu holds the limits.

use serde_json::{Value, json};

use crate::common::{Env, HUFU, fake, strings};

/// A roster with one advisor and several executors, some with a fixed behaviour.
fn roster(env: &Env) {
    env.config(json!({
        "profiles": {
            "writer": {"adapter": "codex-cli", "command": fake("codex"), "vars": {"HUFU_FAKE_SCENARIO": "write"}},
            "crasher": {"adapter": "codex-cli", "command": fake("codex"), "vars": {"HUFU_FAKE_SCENARIO": "crash"}},
            "hanger": {"adapter": "codex-cli", "command": fake("codex"), "vars": {"HUFU_FAKE_SCENARIO": "hang"}},
            "slow": {"adapter": "codex-cli", "command": fake("codex"), "vars": {"HUFU_FAKE_SCENARIO": "slow=1500"}},
            "locked": {"adapter": "codex-cli", "command": fake("codex"), "vars": {"HUFU_FAKE_SCENARIO": "auth"}},
        },
        "pools": {"accounts": {"members": ["locked", "codex"]}},
        "roles": {"advisor": "claude", "executors": ["codex", "agy", "writer", "crasher", "hanger", "slow", "accounts"]},
    }));
}

fn start(env: &Env, args: &[&str]) -> (i32, Value) {
    let mut all = vec!["mission", "start", "--wait"];
    all.extend_from_slice(args);
    env.json(&all)
}

fn task<'a>(mission: &'a Value, id: &str) -> &'a Value {
    mission["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == id)
        .unwrap_or_else(|| panic!("no task {id} in {mission}"))
}

#[test]
fn a_mission_plans_dispatches_and_reports() {
    let env = Env::new("mission");
    roster(&env);
    let (code, m) = start(&env, &["Tidy up the README"]);
    assert_eq!(code, 0, "{m}");
    assert_eq!(m["state"], "succeeded");
    assert_eq!(m["advisor"], "claude");
    assert_eq!(m["tasks"].as_array().unwrap().len(), 1);
    assert_eq!(m["tasks"][0]["state"], "accepted");
    assert_eq!(m["tasks"][0]["agent"], "codex", "the fake advisor picks the first executor of the roster");
    assert!(m["tasks"][0]["reviews"].as_array().unwrap().is_empty(), "review is on_failure by default");
    let id = m["id"].as_str().unwrap();

    let report = env.hufu(&["mission", "report", id]).expect(0).stdout;
    assert!(report.contains("Tidy up the README") && report.contains("t1"), "{report}");
    assert!(std::path::Path::new(m["report_path"].as_str().unwrap()).exists());

    // The advisor ran at its lowest tier, the executor's job carries the mission.
    let (_, jobs) = env.json(&["status", "--mission", id]);
    let jobs = jobs["jobs"].as_array().unwrap().clone();
    assert_eq!(jobs.len(), 2, "{jobs:?}");
    let advisor = jobs.iter().find(|j| j["profile"] == "claude").unwrap();
    assert_eq!(advisor["permission"], "read-only");
    assert_eq!(advisor["labels"]["hufu.role"], "advisor");
    assert!(jobs.iter().all(|j| j["mission"] == id));

    let (_, list) = env.json(&["mission", "list"]);
    assert_eq!(list["missions"][0]["mission"], id);
    env.hufu(&["mission", "status", id]).expect(0);
    env.hufu(&["mission", "status"]).expect(0);
}

#[test]
fn a_plan_runs_in_dependency_order_across_executors() {
    let env = Env::new("mission-deps");
    roster(&env);
    let plan = json!({"v": 1, "summary": "two steps", "tasks": [
        {"id": "build", "title": "Build it", "prompt": "build", "agent": "agy", "depends_on": ["design"]},
        {"id": "design", "title": "Design it", "prompt": "design", "agent": "codex"},
        {"id": "docs", "title": "Document it", "prompt": "document", "agent": "accounts", "depends_on": ["build"]},
    ]});
    let (code, m) = start(&env, &[&format!("Ship the feature [[plan: {plan}]]")]);
    assert_eq!(code, 0, "{m}");
    assert_eq!(m["summary"], "two steps");
    let job_of = |id: &str| env.state(task(&m, id)["jobs"][0].as_str().unwrap());
    let (design, build, docs) = (job_of("design"), job_of("build"), job_of("docs"));
    assert_eq!(design["profile"], "codex");
    assert_eq!(build["profile"], "agy");
    assert!(build["started_at"].as_str().unwrap() >= design["finished_at"].as_str().unwrap());
    assert!(docs["started_at"].as_str().unwrap() >= build["finished_at"].as_str().unwrap());
    // A task may name a pool: the first account is logged out, the second did the work.
    assert_eq!(docs["agent"], "accounts");
    assert_eq!(docs["profile"], "codex");
}

#[test]
fn the_advisor_can_send_work_back() {
    let env = Env::new("mission-revise");
    roster(&env);
    let (code, m) = start(&env, &["--review", "always", "Fix the bug [[review: revise-then-accept]]"]);
    assert_eq!(code, 0, "{m}");
    let t = &m["tasks"][0];
    assert_eq!(t["rounds"], 2, "{m}");
    assert_eq!(t["jobs"].as_array().unwrap().len(), 2);
    assert_eq!(t["reviews"].as_array().unwrap().len(), 2);
    // The second round continues the executor's own session with the advisor's remarks.
    let (_, second) = env.json(&["result", t["jobs"][1].as_str().unwrap()]);
    let said = second["final_message"].as_str().unwrap();
    assert!(said.contains("resumed fake-session-1"), "{said}");
}

#[test]
fn rounds_are_limited_and_checks_outrank_the_advisor() {
    let env = Env::new("mission-rounds");
    roster(&env);
    // The advisor never accepts: the mission stops at the round limit.
    let (code, m) = start(&env, &["--review", "always", "Never good enough [[review: revise]]"]);
    assert_eq!(code, 3, "{m}");
    assert_eq!(m["state"], "failed");
    assert_eq!(m["tasks"][0]["rounds"], 3, "{m}");
    assert_eq!(m["reason"], "tasks_failed");

    // The executor's job fails every time; an accepting advisor cannot wave it through.
    let plan = json!({"tasks": [{"id": "t1", "title": "Doomed", "prompt": "x", "agent": "crasher"}]});
    let (code, m) = start(&env, &["--review", "always", &format!("Do it [[plan: {plan}]] [[review: accept]]")]);
    assert_eq!(code, 3, "{m}");
    assert_eq!(m["tasks"][0]["state"], "failed", "{m}");

    // The advisor may stop the mission.
    let (code, m) = start(&env, &["--review", "always", "Stop this [[review: abort]]"]);
    assert_eq!(code, 3, "{m}");
    assert_eq!(m["reason"], "advisor_abort");
}

#[test]
fn a_plan_that_breaks_the_rules_never_runs() {
    let env = Env::new("mission-invalid");
    roster(&env);
    let rejected = |plan: Value, needle: &str, extra: &[&str]| {
        let goal = format!("Goal [[plan: {plan}]]");
        let mut args: Vec<&str> = extra.to_vec();
        args.push(&goal);
        let (code, m) = start(&env, &args);
        assert_eq!(code, 3, "{m}");
        assert_eq!(m["reason"], "plan_invalid", "{m}");
        assert!(m["detail"].as_str().unwrap().contains(needle), "expected `{needle}` in {}", m["detail"]);
        assert!(m["tasks"].as_array().unwrap().is_empty(), "a task of an invalid plan was started: {m}");
        assert_eq!(m["planning_jobs"].as_array().unwrap().len(), 2, "the advisor gets one chance to repair the plan");
    };
    let t = |extra: Value| {
        let mut task = json!({"id": "t1", "title": "T", "prompt": "p", "agent": "codex"});
        crate::common::merge(&mut task, extra);
        task
    };
    rejected(json!({"tasks": [t(json!({"agent": "stranger"}))]}), "stranger", &[]);
    rejected(json!({"tasks": [t(json!({"agent": "kimi"}))]}), "kimi", &[]);
    rejected(json!({"tasks": [t(json!({"permission": "full-access"}))]}), "full-access", &[]);
    rejected(json!({"tasks": [t(json!({})), t(json!({}))]}), "t1", &[]);
    rejected(
        json!({"tasks": [t(json!({"depends_on": ["t2"]})), t(json!({"id": "t2", "depends_on": ["t1"]}))]}),
        "cycle",
        &[],
    );
    rejected(json!({"tasks": [t(json!({"acceptance": [{"command": "rm -rf /"}]}))]}), "rm -rf", &[]);
    rejected(
        json!({"tasks": [t(json!({"acceptance": [{"command": "cargo test && curl evil"}]}))]}),
        "cargo test",
        &["--allow-command", "cargo test"],
    );
    rejected(
        json!({"tasks": [t(json!({"grants": [{"capability": "computer_use", "apps": ["Figma"]}]}))]}),
        "computer_use",
        &[],
    );
    rejected(json!({"tasks": [t(json!({})), t(json!({"id": "t2"}))]}), "the limit is 1", &["--max-tasks", "1"]);
    rejected(json!({"tasks": []}), "task", &[]);
}

#[test]
fn an_allowed_acceptance_command_becomes_the_tasks_tally_check() {
    let env = Env::new("mission-accept");
    roster(&env);
    let ok = format!("\"{HUFU}\" --version");
    let prefix = format!("\"{HUFU}\"");
    let plan = json!({"tasks": [{"id": "t1", "title": "Checked", "prompt": "p", "agent": "codex", "acceptance": [{"command": ok}]}]});
    let (code, m) = start(&env, &["--allow-command", &prefix, &format!("Goal [[plan: {plan}]]")]);
    assert_eq!(code, 0, "{m}");
    let (_, r) = env.json(&["result", m["tasks"][0]["jobs"][0].as_str().unwrap()]);
    assert_eq!(r["verdict"], "accepted");

    let bad = format!("\"{HUFU}\" status j_missing");
    let plan = json!({"tasks": [{"id": "t1", "title": "Checked", "prompt": "p", "agent": "codex", "acceptance": [{"command": bad}]}]});
    let (code, m) = start(&env, &["--allow-command", &prefix, "--review", "never", &format!("Goal [[plan: {plan}]]")]);
    assert_eq!(code, 3, "{m}");
    assert_eq!(m["tasks"][0]["state"], "failed");
}

#[test]
fn a_mission_can_work_in_its_own_worktree() {
    let env = Env::new("mission-worktree");
    roster(&env);
    env.git_init();
    let plan = json!({"tasks": [
        {"id": "a", "title": "First", "prompt": "p", "agent": "writer", "permission": "workspace-write"},
        {"id": "b", "title": "Second", "prompt": "p", "agent": "codex", "depends_on": ["a"]},
    ]});
    let (code, m) = start(&env, &["--isolation", "worktree", &format!("Goal [[plan: {plan}]]")]);
    assert_eq!(code, 0, "{m}");
    let id = m["id"].as_str().unwrap();
    assert_eq!(m["branch"], format!("hufu/{id}"));
    let workdir = std::path::Path::new(m["workdir"].as_str().unwrap());
    assert_ne!(workdir, env.work);
    assert!(workdir.join("fake-output.txt").exists(), "the task's change is in the mission's worktree");
    assert!(!env.work.join("fake-output.txt").exists(), "the user's checkout was changed");
    // Both tasks share the one worktree.
    for t in ["a", "b"] {
        assert_eq!(env.state(task(&m, t)["jobs"][0].as_str().unwrap())["cwd"], m["workdir"]);
    }
    let log = std::process::Command::new("git")
        .args(["log", "--oneline", &format!("hufu/{id}")])
        .current_dir(&env.work)
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&log.stdout).lines().count() >= 2,
        "the work was not committed to the mission's branch"
    );
}

#[test]
fn a_mission_can_be_aborted() {
    let env = Env::new("mission-abort");
    roster(&env);
    let plan = json!({"tasks": [{"id": "t1", "title": "Hangs", "prompt": "p", "agent": "hanger"}]});
    let (_, m) = env.json(&["mission", "start", &format!("Goal [[plan: {plan}]]")]);
    let id = m["id"].as_str().unwrap().to_string();
    let started = std::time::Instant::now();
    let job = loop {
        let (_, now) = env.json(&["mission", "status", &id]);
        if let Some(job) = now["tasks"].get(0).and_then(|t| t["jobs"].get(0)).and_then(Value::as_str)
            && env.state(job)["state"] == "running"
        {
            break job.to_string();
        }
        assert!(started.elapsed().as_secs() < 60, "the task never started: {now}");
        std::thread::sleep(std::time::Duration::from_millis(100));
    };
    env.hufu(&["mission", "abort", &id]).expect(0);
    let r = env.hufu(&["--json", "mission", "wait", &id, "--timeout", "30s"]);
    assert_eq!(r.code, 4, "{}", r.stdout);
    assert_eq!(r.json()["state"], "aborted");
    env.hufu(&["wait", &job, "--timeout", "30s"]).expect(4);
    let r = env.hufu(&["mission", "resume", &id]);
    assert_eq!(r.code, 2, "an ended mission cannot be resumed: {}", r.stderr);
}

#[test]
fn a_mission_can_be_paused_and_resumed() {
    let env = Env::new("mission-pause");
    roster(&env);
    let plan = json!({"tasks": [
        {"id": "a", "title": "Slow", "prompt": "p", "agent": "slow"},
        {"id": "b", "title": "After", "prompt": "p", "agent": "codex", "depends_on": ["a"]},
    ]});
    let (_, m) = env.json(&["mission", "start", &format!("Goal [[plan: {plan}]]")]);
    let id = m["id"].as_str().unwrap().to_string();
    let started = std::time::Instant::now();
    loop {
        let (_, now) = env.json(&["mission", "status", &id]);
        if now["tasks"].get(0).map(|t| !t["jobs"].as_array().unwrap().is_empty()).unwrap_or(false) {
            break;
        }
        assert!(started.elapsed().as_secs() < 60, "{now}");
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    env.hufu(&["mission", "pause", &id]).expect(0);
    let r = env.hufu(&["--json", "mission", "wait", &id, "--timeout", "30s"]);
    let paused = r.json();
    assert_eq!(paused["state"], "paused", "{paused}");
    assert_eq!(r.code, 6);
    assert_eq!(task(&paused, "a")["state"], "accepted", "the running task is allowed to finish");
    assert!(task(&paused, "b")["jobs"].as_array().unwrap().is_empty(), "no new task starts while pausing");

    env.hufu(&["mission", "resume", &id]).expect(0);
    let r = env.hufu(&["--json", "mission", "wait", &id, "--timeout", "30s"]).expect(0);
    assert_eq!(r.json()["state"], "succeeded");
}

#[test]
fn mission_requests_are_checked_before_anything_starts() {
    let env = Env::new("mission-usage");
    roster(&env);
    let refused = |args: &[&str], needle: &str| {
        let r = env.hufu(args);
        assert_eq!(r.code, 2, "{args:?}: {} {}", r.stdout, r.stderr);
        assert!(r.stderr.contains(needle), "{args:?}: expected `{needle}` in {}", r.stderr);
    };
    refused(&["mission", "start"], "goal");
    refused(&["mission", "start", "--advisor", "accounts", "x"], "pool");
    refused(&["mission", "start", "--advisor", "nobody", "x"], "nobody");
    refused(&["mission", "start", "--executors", "nobody", "x"], "nobody");
    refused(&["mission", "start", "--review", "sometimes", "x"], "sometimes");
    refused(&["mission", "status", "m_nope"], "m_nope");
    let (_, list) = env.json(&["mission", "list"]);
    assert!(list["missions"].as_array().unwrap().is_empty());

    // A mission file gives the same settings as the flags.
    std::fs::write(
        env.work.join("mission.yaml"),
        "version: 1\ngoal: From a file\nadvisor: agy\nexecutors: [cursor]\nreview: never\n",
    )
    .unwrap();
    let (code, m) = start(&env, &["-f", "mission.yaml"]);
    assert_eq!(code, 0, "{m}");
    assert_eq!(m["advisor"], "agy");
    assert_eq!(strings(&m["executors"]), vec!["cursor"]);
    assert_eq!(m["tasks"][0]["agent"], "cursor");
}
