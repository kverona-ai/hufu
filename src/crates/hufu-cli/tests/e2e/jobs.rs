//! Jobs from submission to result: tiers, isolation, the tally check, stopping, pools, accounts.

use serde_json::{Value, json};

use crate::common::{Env, HUFU, fake, kill_job, strings};

fn files(result: &Value) -> Vec<String> {
    strings(&result["changes"]["files"])
}

#[test]
fn every_agent_flavor_runs_a_job_to_a_result() {
    let env = Env::new("flavors");
    for (profile, flavor, tier) in [
        ("codex", "codex", "read-only"),
        ("agy", "agy", "read-only"),
        ("cursor", "cursor", "read-only"),
        ("claude", "claude", "read-only"),
        ("kimi", "kimi", "full-access"),
        ("pi", "pi", "read-only"),
        ("acp", "acp", "read-only"),
        ("ext", "external", "read-only"),
    ] {
        let (code, r) = env.run(&["-a", profile, "-p", tier, "Summarize the repository"]);
        assert_eq!(code, 0, "{profile}: {r}");
        assert_eq!(r["v"], 1);
        assert_eq!(r["state"], "succeeded", "{profile}: {r}");
        assert_eq!(r["agent"]["profile"], profile);
        assert_eq!(r["agent"]["permission"], tier);
        let message = r["final_message"].as_str().unwrap_or_default();
        assert!(message.contains(&format!("fake {flavor} ok")), "{profile}: {message}");
        assert!(
            message.contains("Summarize the repository"),
            "{profile}: the prompt did not reach the agent: {message}"
        );
    }
    let (_, all) = env.json(&["status", "--all"]);
    assert_eq!(all["jobs"].as_array().unwrap().len(), 8);
}

#[test]
fn the_default_executor_is_used_and_the_prompt_can_come_from_stdin_or_a_file() {
    let env = Env::new("defaults");
    let r = env.with_stdin(&["--json", "run", "--wait", "-"], "prompt from stdin");
    let doc = r.json();
    assert_eq!(doc["agent"]["profile"], "codex", "{doc}");
    assert!(doc["final_message"].as_str().unwrap().contains("prompt from stdin"));

    std::fs::write(env.work.join("prompt.txt"), "prompt from a file").unwrap();
    let (code, doc) = env.run(&["--prompt-file", "prompt.txt", "-a", "agy"]);
    assert_eq!(code, 0);
    assert!(doc["final_message"].as_str().unwrap().contains("prompt from a file"));

    std::fs::write(
        env.work.join("task.yaml"),
        "version: 1\ntitle: From a task file\nagent: cursor\nprompt: do the thing\nlabels:\n  ticket: ABC-1\n",
    )
    .unwrap();
    let (code, doc) = env.run(&["-f", "task.yaml"]);
    assert_eq!(code, 0, "{doc}");
    assert_eq!(doc["title"], "From a task file");
    assert_eq!(doc["agent"]["profile"], "cursor");
    // A flag overrides the file.
    let (_, doc) = env.run(&["-f", "task.yaml", "-a", "claude"]);
    assert_eq!(doc["agent"]["profile"], "claude");
}

#[test]
fn a_read_only_job_that_changes_files_is_failed() {
    let env = Env::new("readonly");
    let (code, r) = env.run(&["-a", "codex", "please [[fake:write=sneaky.txt]]"]);
    assert_eq!(code, 3, "{r}");
    assert_eq!(r["state"], "failed");
    assert_eq!(r["reason"], "tier_violated");
    assert!(files(&r).contains(&"sneaky.txt".to_string()), "{r}");
}

#[test]
fn a_workspace_write_job_reports_what_it_changed() {
    let env = Env::new("write");
    env.git_init();
    let (code, r) = env.run(&["-a", "codex", "-p", "workspace-write", "[[fake:write=src/new.txt]]"]);
    assert_eq!(code, 0, "{r}");
    assert_eq!(files(&r), vec!["src/new.txt".to_string()]);
    assert!(env.work.join("src/new.txt").exists());
}

#[test]
fn worktree_isolation_keeps_the_checkout_untouched() {
    let env = Env::new("worktree");
    env.git_init();
    let (code, r) =
        env.run(&["-a", "codex", "-p", "workspace-write", "--isolation", "worktree", "[[fake:write=isolated.txt]]"]);
    assert_eq!(code, 0, "{r}");
    let job = r["job"].as_str().unwrap();
    assert_eq!(r["changes"]["branch"], format!("hufu/{job}"));
    let worktree = r["changes"]["worktree"].as_str().expect("a worktree path");
    assert!(std::path::Path::new(worktree).join("isolated.txt").exists());
    assert!(!env.work.join("isolated.txt").exists(), "the user's checkout was changed");
    assert_eq!(files(&r), vec!["isolated.txt".to_string()]);

    // Outside a repository the request is refused instead of silently running unisolated.
    let plain = Env::new("worktree-plain");
    let r = plain.hufu(&["run", "-a", "codex", "-p", "workspace-write", "--isolation", "worktree", "x"]);
    assert_eq!(r.code, 2, "{}", r.stderr);
    assert!(r.stderr.contains("git"), "{}", r.stderr);
}

#[test]
fn the_tally_check_decides_the_verdict() {
    let env = Env::new("tally");
    let pass = format!("\"{HUFU}\" --version");
    let fail = format!("\"{HUFU}\" status j_does_not_exist");

    let (code, r) = env.run(&["-a", "codex", "--accept", &pass, "work"]);
    assert_eq!(code, 0, "{r}");
    assert_eq!(r["verdict"], "accepted");
    assert_eq!(r["tally"][0]["passed"], true);

    let (code, r) = env.run(&["-a", "codex", "--accept", &pass, "--accept", &fail, "work"]);
    assert_eq!(code, 8, "{r}");
    assert_eq!(r["state"], "succeeded");
    assert_eq!(r["verdict"], "rejected");
    assert_eq!(r["tally"][1]["passed"], false);

    let (code, r) =
        env.run(&["-a", "codex", "-p", "workspace-write", "--allow-path", "src/**", "[[fake:write=docs/readme.txt]]"]);
    assert_eq!(code, 8, "{r}");
    assert_eq!(r["verdict"], "rejected");
    let (code, r) =
        env.run(&["-a", "codex", "-p", "workspace-write", "--allow-path", "src/**", "[[fake:write=src/ok.txt]]"]);
    assert_eq!(code, 0, "{r}");
    assert_eq!(r["verdict"], "accepted");
    let (code, r) =
        env.run(&["-a", "codex", "-p", "workspace-write", "--deny-path", "**/*.lock", "[[fake:write=deps/app.lock]]"]);
    assert_eq!(code, 8, "{r}");

    // A job without checks is not judged at all.
    let (_, r) = env.run(&["-a", "codex", "work"]);
    assert_eq!(r["verdict"], "unchecked");
}

#[test]
fn skipped_actions_are_reported_and_strict_mode_fails_on_them() {
    let env = Env::new("deny");
    let (code, r) = env.run(&["-a", "codex", "[[fake:deny]]"]);
    assert_eq!(code, 0, "{r}");
    assert!(!r["denied_actions"].as_array().unwrap().is_empty(), "{r}");

    let (code, r) = env.run(&["-a", "codex", "--strict", "[[fake:deny]]"]);
    assert_eq!(code, 3, "{r}");
    assert_eq!(r["reason"], "permission_denied");
}

#[test]
fn acp_permission_requests_follow_the_tier() {
    let env = Env::new("acp-perm");
    env.config(json!({"profiles": {"acp-npm": {"adapter": "acp", "command": fake("acp"), "permission_requests": "verified", "allow_execute": ["npm test"]}}}));
    let denied = |r: &Value| -> Vec<String> {
        r["denied_actions"].as_array().unwrap().iter().filter_map(|d| d["kind"].as_str().map(String::from)).collect()
    };
    // At read-only both the write and the command are refused by Hufu, and the refusals are on record.
    let (code, r) = env.run(&["-a", "acp", "[[fake:permission]]"]);
    assert_eq!(code, 0, "{r}");
    assert_eq!(denied(&r), vec!["edit", "execute"], "{r}");
    assert!(files(&r).is_empty(), "{r}");
    assert!(!env.work.join("fake-output.txt").exists());
    // At workspace-write the edit is allowed; a command still needs to be on the profile's list.
    let (code, r) = env.run(&["-a", "acp", "-p", "workspace-write", "[[fake:permission]]"]);
    assert_eq!(code, 0, "{r}");
    assert_eq!(denied(&r), vec!["execute"], "{r}");
    assert_eq!(files(&r), vec!["fake-output.txt".to_string()], "{r}");
    let (_, r) = env.run(&["-a", "acp-npm", "-p", "workspace-write", "[[fake:permission=second.txt]]"]);
    assert!(denied(&r).is_empty(), "{r}");
    // A refusal by the model is a failure with its own reason.
    let (code, r) = env.run(&["-a", "acp", "[[fake:refuse]]"]);
    assert_eq!(code, 3, "{r}");
    assert_eq!(r["reason"], "refusal");
}

#[test]
fn an_unverified_acp_agent_is_limited_to_full_access() {
    let env = Env::new("acp-unverified");
    env.config(json!({"profiles": {"raw": {"adapter": "acp", "command": fake("acp")}}}));
    let r = env.hufu(&["run", "-a", "raw", "x"]);
    assert_eq!(r.code, 2, "{}", r.stderr);
    assert!(r.stderr.contains("full-access"), "{}", r.stderr);
}

#[test]
fn failures_carry_a_reason_and_the_matching_exit_code() {
    let env = Env::new("failures");
    for (scenario, reason, exit) in [
        ("crash", "agent_error", 3),
        ("empty", "empty_output", 3),
        ("auth", "auth_required", 10),
        ("rate-limit", "rate_limited", 3),
    ] {
        let (code, r) = env.run(&["-a", "codex", &format!("[[fake:{scenario}]]")]);
        assert_eq!(r["state"], "failed", "{scenario}: {r}");
        assert_eq!(r["reason"], reason, "{scenario}: {r}");
        assert_eq!(code, exit, "{scenario}");
        assert!(
            r["detail"].as_str().map(|d| !d.is_empty()).unwrap_or(false),
            "{scenario}: a failure needs a detail: {r}"
        );
    }
    // An agent that is not installed.
    env.config(json!({"profiles": {"ghost": {"adapter": "codex-cli", "command": ["hufu-no-such-binary-xyz"]}}}));
    let (code, r) = env.run(&["-a", "ghost", "x"]);
    assert_eq!(code, 10, "{r}");
    assert_eq!(r["reason"], "agent_unavailable");
}

#[test]
fn retryable_failures_are_retried_with_backoff() {
    let env = Env::new("retry");
    env.config(json!({"defaults": {"retry": {"max_attempts": 1, "on": ["rate_limited"], "backoff": "100ms"}}}));
    let (code, r) = env.run(&["-a", "codex", "--retry", "3", "[[fake:rate-limit]]"]);
    assert_eq!(code, 3, "{r}");
    assert_eq!(r["attempts"], 3, "{r}");
    let (_, r) = env.run(&["-a", "codex", "--retry", "3", "[[fake:crash]]"]);
    assert_eq!(r["attempts"], 1, "an agent error is not retried by default: {r}");
}

#[test]
fn cancelling_stops_the_job_and_its_agent() {
    let env = Env::new("cancel");
    let job = env.start(&["-a", "codex", "[[fake:hang]]"]);
    env.until(&job, "running", |s| s["state"] == "running" && s["last_activity"].is_string());
    let state: Value =
        serde_json::from_str(&std::fs::read_to_string(env.job_file(&job, "state.json")).unwrap()).unwrap();
    let agent_pid = state["agent_pid"].as_u64().expect("the agent's pid is recorded") as u32;
    assert!(hufu_contract::proc::pid_alive(agent_pid));

    env.hufu(&["cancel", &job]).expect(0);
    let r = env.hufu(&["wait", &job, "--timeout", "30s"]);
    assert_eq!(r.code, 4, "{} {}", r.stdout, r.stderr);
    let (code, result) = env.json(&["result", &job]);
    assert_eq!(code, 4);
    assert_eq!(result["state"], "cancelled");
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert!(!hufu_contract::proc::pid_alive(agent_pid), "the agent survived the cancellation");

    // Cancelling a finished job changes nothing.
    env.hufu(&["cancel", &job]).expect(0);
    assert_eq!(env.state(&job)["state"], "cancelled");
}

#[test]
fn time_limits_end_the_job() {
    let env = Env::new("timeout");
    let (code, r) = env.run(&["-a", "codex", "-t", "1s", "[[fake:hang]]"]);
    assert_eq!(code, 5, "{r}");
    assert_eq!(r["state"], "timed_out");
    assert_eq!(r["reason"], "timeout");

    let (code, r) = env.run(&["-a", "codex", "--inactivity-timeout", "1s", "[[fake:silent]]"]);
    assert_eq!(code, 5, "{r}");
    assert_eq!(r["reason"], "inactivity_timeout");
}

#[test]
fn waiting_can_give_up_without_touching_the_job() {
    let env = Env::new("wait");
    let job = env.start(&["-a", "codex", "[[fake:slow=2500]]"]);
    let r = env.hufu(&["--json", "wait", &job, "--timeout", "300ms"]);
    assert_eq!(r.code, 6, "{}", r.stdout);
    assert_eq!(r.json()["wait_timed_out"], true);
    let (code, unfinished) = env.json(&["result", &job]);
    assert_eq!(code, 6);
    assert_eq!(unfinished["finished"], false);
    env.hufu(&["wait", &job]).expect(0);
    let (code, r) = env.json(&["result", &job]);
    assert_eq!((code, r["state"].as_str()), (0, Some("succeeded")));

    // `--any` returns with the first finished job; the worst outcome decides the exit code.
    let slow = env.start(&["-a", "codex", "[[fake:slow=2500]]"]);
    let quick = env.start(&["-a", "codex", "[[fake:crash]]"]);
    let r = env.hufu(&["--json", "wait", &slow, &quick, "--any"]);
    assert_eq!(r.code, 3, "{}", r.stdout);
    env.hufu(&["wait", &slow, &quick]).expect(3);
}

#[test]
fn a_job_whose_runner_died_is_reported_as_lost() {
    let env = Env::new("lost");
    let job = env.start(&["-a", "codex", "[[fake:hang]]"]);
    env.until(&job, "running", |s| s["state"] == "running" && s["last_activity"].is_string());
    let state: Value =
        serde_json::from_str(&std::fs::read_to_string(env.job_file(&job, "state.json")).unwrap()).unwrap();
    let agent = state["agent_pid"].as_u64().unwrap();
    kill_job(state["runner_pid"].as_u64().expect("the runner's pid is recorded"), agent);
    let r = env.hufu(&["wait", &job, "--timeout", "30s"]);
    assert_eq!(r.code, 7, "{} {}", r.stdout, r.stderr);
    let (code, result) = env.json(&["result", &job]);
    assert_eq!(code, 7);
    assert_eq!(result["state"], "lost");
    assert_eq!(result["reason"], "runner_died");
}

#[test]
fn a_pool_fails_over_to_the_next_account() {
    let env = Env::new("pool");
    env.config(json!({
        "profiles": {
            "work": {"adapter": "codex-cli", "command": fake("codex"), "account": "work", "vars": {"HUFU_FAKE_SCENARIO": "auth"}},
            "personal": {"adapter": "codex-cli", "command": fake("codex"), "account": "personal"},
            "limited": {"adapter": "codex-cli", "command": fake("codex"), "account": "limited", "vars": {"HUFU_FAKE_SCENARIO": "rate-limit"}},
        },
        "pools": {
            "team": {"members": ["work", "limited", "personal"]},
            "broken": {"members": ["work", "limited"]},
        },
    }));
    let (code, r) = env.run(&["-a", "team", "hello"]);
    assert_eq!(code, 0, "{r}");
    assert_eq!(r["agent"]["profile"], "personal");
    assert_eq!(r["agent"]["account"], "personal");
    assert!(strings(&r["warnings"]).iter().any(|w| w.contains("work")), "the failover is not on record: {r}");

    // A single account fails outright, with the exit code for "log in first".
    let (code, r) = env.run(&["-a", "work", "hello"]);
    assert_eq!(code, 10, "{r}");
    assert_eq!(r["reason"], "auth_required");

    // When every member fails, the last failure is reported.
    let (code, r) = env.run(&["-a", "broken", "hello"]);
    assert_ne!(code, 0, "{r}");
    assert_eq!(r["state"], "failed");
    assert_eq!(r["reason"], "rate_limited");

    let (_, status) = env.json(&["status", r["job"].as_str().unwrap()]);
    assert_eq!(status["agent"], "broken");
}

#[test]
fn round_robin_pools_spread_jobs_over_accounts() {
    let env = Env::new("rr");
    env.config(json!({
        "profiles": {
            "a1": {"adapter": "codex-cli", "command": fake("codex")},
            "a2": {"adapter": "codex-cli", "command": fake("codex")},
        },
        "pools": {"both": {"members": ["a1", "a2"], "strategy": "round-robin"}},
    }));
    let used: Vec<String> =
        (0..4).map(|_| env.run(&["-a", "both", "x"]).1["agent"]["profile"].as_str().unwrap().to_string()).collect();
    assert_eq!(used.iter().filter(|p| *p == "a1").count(), 2, "{used:?}");
    assert_eq!(used.iter().filter(|p| *p == "a2").count(), 2, "{used:?}");
}

#[test]
fn model_and_effort_come_from_the_task_then_the_profile_then_the_default() {
    let env = Env::new("effort");
    env.config(json!({
        "profiles": {"deep": {"adapter": "codex-cli", "command": fake("codex"), "model": "gpt-big", "effort": "max"}},
        "defaults": {"effort": "medium"},
    }));
    let ran = |args: &[&str]| {
        let (code, r) = env.run(args);
        assert_eq!(code, 0, "{args:?}: {r}");
        r
    };
    // The profile's own settings.
    let r = ran(&["-a", "deep", "x"]);
    assert_eq!((r["agent"]["model"].as_str(), r["agent"]["effort"].as_str()), (Some("gpt-big"), Some("max")));
    // The task overrides the profile, for this job only.
    let r = ran(&["-a", "deep", "-m", "gpt-small", "-e", "low", "[[fake:args]]"]);
    assert_eq!((r["agent"]["model"].as_str(), r["agent"]["effort"].as_str()), (Some("gpt-small"), Some("low")));
    let sent = r["final_message"].as_str().unwrap();
    assert!(sent.contains("gpt-small") && sent.contains(r#"model_reasoning_effort=\"low\""#), "{sent}");
    let shown = env.hufu(&["result", r["job"].as_str().unwrap()]).expect(0).stdout;
    assert!(shown.contains("model gpt-small · effort low"), "{shown}");
    // Neither says anything: the global default effort, and the agent's own model.
    let r = ran(&["-a", "codex", "x"]);
    assert_eq!(r["agent"]["effort"], "medium");
    assert!(r["agent"]["model"].is_null(), "{r}");
    // A default the agent has no level for is not an error: the agent's own default is used.
    let r = ran(&["-a", "cursor", "x"]);
    assert_eq!(r["agent"]["effort"], "default");
    assert!(strings(&r["warnings"]).iter().any(|w| w.contains("default effort `medium`")), "{r}");

    // A level asked for by name is refused, unless the profile accepts the nearest one.
    let refused = env.hufu(&["run", "-a", "agy", "-e", "max", "x"]).expect(2);
    assert!(refused.stderr.contains("effort_fallback: nearest"), "{}", refused.stderr);
    assert!(env.hufu(&["profile", "set", "agy", "--effort-fallback", "closest"]).expect(2).stderr.contains("nearest"));
    env.hufu(&["profile", "set", "agy", "--effort-fallback", "nearest"]).expect(0);
    let r = ran(&["-a", "agy", "-e", "max", "x"]);
    assert_eq!(r["agent"]["effort"], "high");
    assert!(strings(&r["warnings"]).iter().any(|w| w.contains("nearest level `high`")), "{r}");
}

#[test]
fn a_pool_leaves_out_members_that_cannot_take_the_task() {
    let env = Env::new("pool-fit");
    env.config(json!({
        "profiles": {
            "a1": {"adapter": "codex-cli", "command": fake("codex"), "model": "own-model"},
            "a2": {"adapter": "codex-cli", "command": fake("codex")},
            "out": {"adapter": "codex-cli", "command": fake("codex"), "vars": {"HUFU_FAKE_SCENARIO": "auth"}},
        },
        "pools": {
            "mixed": {"members": ["cursor", "a1"]},
            "spare": {"members": ["out", "a1"]},
            "idle": {"members": ["a1", "a2"], "strategy": "least-busy"},
        },
    }));
    // Cursor has no effort control, so a task that asks for a level goes to the next member.
    let (code, r) = env.run(&["-a", "mixed", "-e", "high", "x"]);
    assert_eq!(code, 0, "{r}");
    assert_eq!((r["agent"]["profile"].as_str(), r["agent"]["effort"].as_str()), (Some("a1"), Some("high")));
    assert_eq!(r["agent"]["model"], "own-model");
    assert!(strings(&r["warnings"]).iter().any(|w| w.contains("pool member `cursor` cannot take this task")), "{r}");
    // Without a level, the first member takes it.
    assert_eq!(env.run(&["-a", "mixed", "x"]).1["agent"]["profile"], "cursor");
    // No member can: refused before anything starts.
    assert!(env.hufu(&["run", "-a", "mixed", "-e", "off", "x"]).expect(2).stderr.contains("no member of pool `mixed`"));

    // A model named on the task goes with the job to whichever member runs it.
    let (code, r) = env.run(&["-a", "spare", "-m", "task-model", "x"]);
    assert_eq!(code, 0, "{r}");
    assert_eq!((r["agent"]["profile"].as_str(), r["agent"]["model"].as_str()), (Some("a1"), Some("task-model")));

    // least-busy: the member with fewer running jobs comes first.
    let busy = env.start(&["-a", "idle", "[[fake:hang]]"]);
    env.until(&busy, "running", |s| s["state"] == "running");
    assert_eq!(env.state(&busy)["profile"], "a1");
    assert_eq!(env.run(&["-a", "idle", "x"]).1["agent"]["profile"], "a2");
    env.hufu(&["cancel", &busy]).expect(0);
    assert_eq!(env.hufu(&["wait", &busy, "--timeout", "30s"]).code, 4);
}

#[test]
fn each_account_gets_its_own_login_directory_and_credentials() {
    let mut env = Env::new("accounts");
    env.set("TEAM_KEY_B", "secret-b");
    env.set("STRAY_SECRET", "must-not-leak");
    let home_a = env.root.join("codex-a");
    env.config(json!({
        "profiles": {
            "codex-a": {"adapter": "codex-cli", "command": fake("codex"), "account": "a", "home": home_a},
            "codex-b": {"adapter": "codex-cli", "command": fake("codex"), "account": "b", "env_from": {"OPENAI_API_KEY": "TEAM_KEY_B"}},
            "codex-c": {"adapter": "codex-cli", "command": fake("codex"), "env_from": {"OPENAI_API_KEY": "TEAM_KEY_MISSING"}},
            "claude-a": {"adapter": "claude-cli", "command": fake("claude"), "home": home_a},
        },
    }));
    let said = |args: &[&str]| env.run(args).1["final_message"].as_str().unwrap_or_default().to_string();
    assert_eq!(said(&["-a", "codex-a", "[[fake:env=CODEX_HOME]]"]), format!("CODEX_HOME={}", home_a.display()));
    assert_eq!(
        said(&["-a", "claude-a", "[[fake:env=CLAUDE_CONFIG_DIR]]"]),
        format!("CLAUDE_CONFIG_DIR={}", home_a.display())
    );
    assert_eq!(said(&["-a", "codex-b", "[[fake:env=OPENAI_API_KEY]]"]), "OPENAI_API_KEY=secret-b");
    assert_eq!(said(&["-a", "codex-b", "[[fake:env=CODEX_HOME]]"]), "CODEX_HOME=<unset>");
    // Nothing the profile did not name reaches the agent.
    assert_eq!(said(&["-a", "codex-b", "[[fake:env=STRAY_SECRET]]"]), "STRAY_SECRET=<unset>");
    assert_eq!(said(&["-a", "codex-a", "[[fake:env=TEAM_KEY_B]]"]), "TEAM_KEY_B=<unset>");

    // A credential variable that is not set fails the job the same way a missing login does.
    let (code, r) = env.run(&["-a", "codex-c", "x"]);
    assert_eq!(code, 10, "{r}");
    assert_eq!(r["reason"], "auth_required");
    assert!(r["detail"].as_str().unwrap().contains("TEAM_KEY_MISSING"), "{r}");
    // The secret's value is not stored with the job.
    let job = env.start(&["-a", "codex-b", "x"]);
    env.hufu(&["wait", &job]).expect(0);
    let spec = std::fs::read_to_string(env.job_file(&job, "spec.json")).unwrap();
    assert!(!spec.contains("secret-b"), "a secret was written to spec.json");
}

#[test]
fn a_finished_session_can_be_resumed() {
    let env = Env::new("resume");
    let (_, first) = env.run(&["-a", "codex", "-p", "workspace-write", "first"]);
    let job = first["job"].as_str().unwrap();
    assert_eq!(first["session"]["resumable"], true, "{first}");
    let r = env.hufu(&["--json", "resume", job, "--wait", "and now the follow-up"]).expect(0);
    let second = r.json();
    assert_ne!(second["job"], first["job"]);
    let message = second["final_message"].as_str().unwrap();
    assert!(message.contains("resumed fake-session-1") && message.contains("follow-up"), "{message}");
    assert_eq!(second["agent"]["permission"], "workspace-write", "the follow-up keeps the tier");

    // An adapter without sessions says so instead of starting a fresh conversation.
    let (_, kimi) = env.run(&["-a", "kimi", "-p", "full-access", "x"]);
    let r = env.hufu(&["resume", kimi["job"].as_str().unwrap(), "more"]);
    assert_eq!(r.code, 2, "{}", r.stderr);
}

#[test]
fn structured_output_is_validated_against_the_schema() {
    let env = Env::new("schema");
    std::fs::write(
        env.work.join("schema.json"),
        r#"{"type":"object","properties":{"ok":{"type":"boolean"},"summary":{"type":"string"}},"required":["ok","summary"],"additionalProperties":false}"#,
    )
    .unwrap();
    for agent in ["codex", "claude", "agy", "pi"] {
        let (code, r) =
            env.run(&["-a", agent, "--schema", "schema.json", r#"[[fake:json={"ok":true,"summary":"done"}]]"#]);
        assert_eq!(code, 0, "{agent}: {r}");
        assert_eq!(r["structured_output"], json!({"ok": true, "summary": "done"}), "{agent}");
    }
    let (code, r) = env.run(&["-a", "codex", "--schema", "schema.json", r#"[[fake:json={"ok":"yes"}]]"#]);
    assert_eq!(code, 3, "{r}");
    assert_eq!(r["reason"], "invalid_output");
    let (code, r) = env.run(&["-a", "agy", "--schema", "schema.json", "[[fake:reply=no json here]]"]);
    assert_eq!(code, 3, "{r}");
    assert_eq!(r["reason"], "invalid_output");
}

#[test]
fn a_dry_run_shows_the_invocation_and_creates_nothing() {
    let env = Env::new("dry");
    let (code, doc) = env.json(&[
        "run",
        "--dry-run",
        "-a",
        "codex",
        "-p",
        "workspace-write",
        "-e",
        "high",
        "-m",
        "gpt-x",
        "plan this",
    ]);
    assert_eq!(code, 0, "{doc}");
    assert_eq!(doc["dry_run"], true);
    assert_eq!(doc["permission"], "workspace-write");
    let args = strings(&doc["launch"]["args"]);
    assert!(args.iter().any(|a| a == "workspace-write"), "{args:?}");
    assert!(args.iter().any(|a| a.contains("gpt-x")), "{args:?}");
    assert!(args.iter().any(|a| a.contains("high")), "{args:?}");
    let (_, all) = env.json(&["status", "--all"]);
    assert!(all["jobs"].as_array().unwrap().is_empty());
    assert!(
        env.hufu(&["run", "--dry-run", "-a", "claude", "x"]).expect(0).stdout.contains("would run profile `claude`")
    );
}

#[test]
fn requests_an_agent_cannot_honour_are_refused_before_anything_starts() {
    let env = Env::new("usage");
    let refused = |args: &[&str], needle: &str| {
        let r = env.hufu(args);
        assert_eq!(r.code, 2, "{args:?}: {} {}", r.stdout, r.stderr);
        assert!(
            r.stderr.to_lowercase().contains(&needle.to_lowercase()),
            "{args:?}: expected `{needle}` in: {}",
            r.stderr
        );
    };
    refused(&["run", "-a", "nobody", "x"], "nobody");
    refused(&["run", "-a", "codex"], "prompt");
    refused(&["run", "-a", "codex", "-p", "root", "x"], "read-only");
    refused(&["run", "-a", "kimi", "-p", "read-only", "x"], "full-access");
    refused(&["run", "-a", "pi", "-p", "workspace-write", "x"], "workspace-write");
    refused(&["run", "-a", "kimi", "-p", "full-access", "-e", "off", "x"], "effort");
    refused(&["run", "-a", "codex", "-t", "soon", "x"], "soon");
    refused(&["run", "-a", "codex", "--bogus-flag", "x"], "bogus");
    refused(&["result", "j_nope"], "j_nope");
    let (_, all) = env.json(&["status", "--all"]);
    assert!(all["jobs"].as_array().unwrap().is_empty(), "a refused request left a job behind");

    // In JSON mode the error is a document on stdout.
    let r = env.hufu(&["--json", "run", "-a", "nobody", "x"]);
    assert_eq!(r.code, 2);
    assert_eq!(r.json()["error"]["kind"], "usage");
    env.hufu(&["--help"]).expect(0);
    env.hufu(&["--version"]).expect(0);
}

#[test]
fn a_job_waits_for_the_jobs_it_depends_on() {
    let env = Env::new("deps");
    let first = env.start(&["-a", "codex", "[[fake:slow=1200]]"]);
    let second = env.start(&["-a", "codex", "--depends-on", &first, "after the first"]);
    env.hufu(&["wait", &first, &second]).expect(0);
    let (a, b) = (env.state(&first), env.state(&second));
    assert!(b["started_at"].as_str().unwrap() >= a["finished_at"].as_str().unwrap(), "{a} {b}");

    let bad = env.start(&["-a", "codex", "[[fake:crash]]"]);
    let after = env.start(&["-a", "codex", "--depends-on", &bad, "never runs"]);
    env.hufu(&["wait", &after]).expect(3);
    assert_eq!(env.state(&after)["reason"], "dependency_failed");
}

#[test]
fn logs_show_normalized_events_and_the_agents_own_output() {
    let env = Env::new("logs");
    let (_, r) = env.run(&["-a", "codex", "-p", "workspace-write", "[[fake:write=log.txt]]"]);
    let job = r["job"].as_str().unwrap();
    let human = env.hufu(&["logs", job]).expect(0).stdout;
    assert!(human.contains("log.txt"), "{human}");
    let ndjson = env.hufu(&["--json", "logs", job]).expect(0).stdout;
    let events: Vec<Value> =
        ndjson.lines().map(|l| serde_json::from_str(l).expect("one JSON document per line")).collect();
    let types: Vec<&str> = events.iter().filter_map(|e| e["type"].as_str()).collect();
    assert_eq!(types.first(), Some(&"job.state"), "{types:?}");
    assert!(types.contains(&"tool.call") && types.contains(&"agent.message"), "{types:?}");
    assert!(types.last().map(|t| t.starts_with("job.")).unwrap_or(false), "{types:?}");
    let seqs: Vec<u64> = events.iter().filter_map(|e| e["seq"].as_u64()).collect();
    assert!(seqs.windows(2).all(|w| w[0] < w[1]), "sequence numbers must increase: {seqs:?}");
    let later = env.hufu(&["--json", "logs", job, "--since", &seqs[seqs.len() - 2].to_string()]).expect(0).stdout;
    assert_eq!(later.lines().count(), 1);
    let raw = env.hufu(&["logs", job, "--raw"]).expect(0).stdout;
    assert!(raw.contains("thread.started") || raw.contains("session"), "the agent's own output was not kept: {raw}");
    // Jobs can be named by a unique prefix of their id.
    env.hufu(&["status", &job[..job.len() - 2]]).expect(0);
}

#[test]
fn old_jobs_are_pruned_and_active_ones_are_kept() {
    let env = Env::new("prune");
    let done: Vec<String> =
        (0..3).map(|i| env.run(&["-a", "codex", &format!("job {i}")]).1["job"].as_str().unwrap().to_string()).collect();
    let active = env.start(&["-a", "codex", "[[fake:hang]]"]);
    let (_, dry) = env.json(&["prune", "--keep", "1", "--dry-run"]);
    assert_eq!(dry["removed_jobs"].as_array().map(Vec::len), Some(2), "{dry}");
    assert!(env.job_file(&done[0], "state.json").exists(), "a dry run removed something");
    env.hufu(&["prune", "--keep", "1"]).expect(0);
    assert!(!env.job_file(&done[0], "state.json").exists());
    assert!(!env.job_file(&done[1], "state.json").exists());
    assert!(env.job_file(&done[2], "state.json").exists());
    assert!(env.job_file(&active, "state.json").exists(), "an active job was pruned");
    env.hufu(&["cancel", &active, "--force"]).expect(0);
    env.hufu(&["wait", &active]).expect(4);
}
