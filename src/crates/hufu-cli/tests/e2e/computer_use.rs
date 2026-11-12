//! Computer use: who may serve it, how it is scoped, and the hand-over to a dedicated agent.

use serde_json::{Value, json};

use crate::common::{Env, HUFU, fake, strings};
use crate::mcp::Host;

/// `desk` is configured only to operate the desktop; `codex-eyes` is an executor that can do it
/// itself; `codex` and the others cannot.
fn setup(env: &Env, extra: Value) {
    let lcu = json!({"computer_use": {"via": "lcu", "check": [HUFU, "__fake-agent", "lcu", "apps"]}});
    let mut doc = json!({
        "profiles": {
            "desk": {"adapter": "claude-cli", "command": fake("claude"), "description": "Operates the desktop", "provides": lcu},
            "codex-eyes": {"adapter": "codex-cli", "command": fake("codex"), "provides": {"computer_use": {"via": "native"}}},
        },
        "capabilities": {"computer_use": {"route": "dedicated", "agent": "desk"}},
    });
    crate::common::merge(&mut doc, extra);
    env.config(doc);
}

fn refused(env: &Env, args: &[&str], needle: &str) {
    let mut all = vec!["run", "--dry-run"];
    all.extend_from_slice(args);
    let r = env.hufu(&all);
    assert_eq!(r.code, 2, "{args:?}: {} {}", r.stdout, r.stderr);
    assert!(r.stderr.contains(needle), "{args:?}: expected `{needle}` in: {}", r.stderr);
}

#[test]
fn a_grant_is_routed_to_the_configured_agent_and_scoped_to_named_apps() {
    let env = Env::new("cu-route");
    setup(&env, json!({}));

    // Dedicated: the executor gets Hufu's hand-over tool, the operator is the configured profile.
    let (code, doc) =
        env.json(&["run", "--dry-run", "-a", "codex", "--computer-use", "Figma,Notes", "Export the frame"]);
    assert_eq!(code, 0, "{doc}");
    assert_eq!(doc["computer_use"]["route"], "dedicated");
    assert_eq!(doc["computer_use"]["agent"], "desk");
    assert_eq!(strings(&doc["computer_use"]["apps"]), vec!["Figma", "Notes"]);
    let args = strings(&doc["launch"]["args"]).join(" ");
    assert!(
        args.contains("mcp_servers.hufu") && args.contains("--child-of"),
        "the hand-over server is not attached: {args}"
    );
    assert!(strings(&doc["warnings"]).iter().any(|w| w.contains("outside the workspace")), "{doc}");
    assert!(doc["launch"]["stdin"].as_str().unwrap().contains("computer_use"), "the executor is told how to hand over");

    // The executor's own agent, when it supports it and the task says so.
    let (_, doc) = env.json(&[
        "run",
        "--dry-run",
        "-a",
        "codex-eyes",
        "--computer-use",
        "Figma",
        "--computer-use-route",
        "executor",
        "x",
    ]);
    assert_eq!(doc["computer_use"]["route"], "executor");
    assert_eq!(doc["computer_use"]["agent"], "codex-eyes");
    assert!(!strings(&doc["launch"]["args"]).join(" ").contains("--child-of"));
    // The configured operator needs no hand-over to itself.
    let (_, doc) = env.json(&["run", "--dry-run", "-a", "desk", "--computer-use", "Figma", "x"]);
    assert_eq!(doc["computer_use"]["route"], "executor");
    // Another operator for one task.
    let (_, doc) = env.json(&[
        "run",
        "--dry-run",
        "-a",
        "codex",
        "--computer-use",
        "Figma",
        "--computer-use-agent",
        "codex-eyes",
        "x",
    ]);
    assert_eq!(doc["computer_use"]["agent"], "codex-eyes");

    // Only an agent that supports computer use can serve it.
    refused(
        &env,
        &["-a", "codex", "--computer-use", "Figma", "--computer-use-route", "executor", "x"],
        "does not support computer use",
    );
    refused(
        &env,
        &["-a", "codex", "--computer-use", "Figma", "--computer-use-agent", "agy", "x"],
        "provides.computer_use",
    );
    refused(&env, &["-a", "codex", "--computer-use", "Figma", "--computer-use-agent", "nobody", "x"], "nobody");
    // An executor that cannot be given tools cannot delegate.
    refused(&env, &["-a", "agy", "--computer-use", "Figma", "x"], "hand-over");
    // The scope is a list of applications: no wildcard, no terminal, no agent host.
    refused(&env, &["-a", "codex", "--computer-use", "*", "x"], "wildcards");
    refused(&env, &["-a", "codex", "--computer-use", "Terminal", "x"], "terminals");
    refused(&env, &["-a", "codex", "--computer-use", "Figma,iTerm", "x"], "iTerm");
    // An application the runtime has not been allowed to touch needs a person first.
    refused(&env, &["-a", "codex", "--computer-use", "Slack", "x"], "lcu apps allow \"Slack\"");
    // Routing without a grant is a mistake, not a no-op.
    refused(&env, &["-a", "codex", "--computer-use-agent", "desk", "x"], "grant");
    // A browser is allowed but called out.
    let mut loose = Env::new("cu-browser");
    loose.set("HUFU_FAKE_LCU_APPS", "Safari");
    setup(&loose, json!({}));
    let (code, doc) = loose.json(&["run", "--dry-run", "-a", "codex", "--computer-use", "Safari", "x"]);
    assert_eq!(code, 0, "{doc}");
    assert!(strings(&doc["warnings"]).iter().any(|w| w.contains("credentials")), "{doc}");
}

#[test]
fn configuration_decides_the_default_route() {
    let env = Env::new("cu-config");
    // Nothing configured: the request says what to set.
    env.config(json!({}));
    refused(&env, &["-a", "codex", "--computer-use", "Figma", "x"], "capabilities.computer_use.agent");

    // A profile that does not support computer use cannot be named as the operator.
    let r = env.hufu(&["config", "set", "capabilities.computer_use.agent", "codex"]);
    assert_eq!(r.code, 2, "{}", r.stderr);
    assert!(r.stderr.contains("provides"), "{}", r.stderr);

    // Declaring the capability on an executor makes it eligible; the operator can be any such profile.
    env.hufu(&["profile", "set", "codex", "--computer-use", "native"]).expect(0);
    env.hufu(&["config", "set", "capabilities.computer_use.agent", "codex"]).expect(0);
    let (_, doc) = env.json(&["run", "--dry-run", "-a", "claude", "--computer-use", "Figma", "x"]);
    assert_eq!(
        (doc["computer_use"]["route"].as_str(), doc["computer_use"]["agent"].as_str()),
        (Some("dedicated"), Some("codex"))
    );
    let (_, agents) = env.json(&["agents"]);
    let codex = agents["agents"].as_array().unwrap().iter().find(|a| a["name"] == "codex").unwrap().clone();
    assert!(strings(&codex["roles"]).contains(&"computer_use".to_string()), "{codex}");

    // Route `executor` as the default: every job uses its own agent, or is refused.
    env.hufu(&["config", "set", "capabilities.computer_use.route", "executor"]).expect(0);
    let (_, doc) = env.json(&["run", "--dry-run", "-a", "codex", "--computer-use", "Figma", "x"]);
    assert_eq!(doc["computer_use"]["route"], "executor");
    refused(&env, &["-a", "claude", "--computer-use", "Figma", "x"], "does not support computer use");

    // Falling back to the executor when the dedicated agent cannot serve.
    setup(&env, json!({"capabilities": {"computer_use": {"when_unavailable": "executor"}}}));
    let (code, doc) = env.json(&["run", "--dry-run", "-a", "codex-eyes", "--computer-use", "Slack", "x"]);
    assert_eq!(code, 0, "{doc}");
    assert_eq!(doc["computer_use"]["route"], "executor");
    assert!(strings(&doc["warnings"]).iter().any(|w| w.contains("falls back")), "{doc}");
}

#[test]
fn the_executor_hands_a_desktop_step_to_the_operator() {
    let env = Env::new("cu-handover");
    setup(&env, json!({}));
    let job = env.start(&["-a", "codex", "--computer-use", "Figma", "Export it [[fake:hang]]"]);
    env.until(&job, "running", |s| s["state"] == "running");
    let spec: Value = serde_json::from_str(&std::fs::read_to_string(env.job_file(&job, "spec.json")).unwrap()).unwrap();
    assert_eq!(spec["computer_use"]["agent"], "desk");

    // What the executor's agent sees: one tool, and nothing else of Hufu.
    let mut host = Host::start(&env, &["--child-of", &job]);
    host.handshake();
    let tools = host.request("tools/list", json!({}))["result"]["tools"].as_array().unwrap().clone();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0]["name"], "computer_use");
    assert_eq!(
        host.request("tools/call", json!({"name": "hufu_submit", "arguments": {"agent": "codex", "prompt": "x"}}))["error"]
            ["code"],
        -32602
    );

    let seen = host.call("computer_use", json!({"instruction": "Open the Cover frame and export it as PNG"}));
    let seen = seen.as_str().expect("the operator's report comes back as text");
    assert!(seen.contains("fake claude ok"), "{seen}");

    // The step is a job of its own, on the operator's profile, tied to the executor's job.
    let (_, all) = env.json(&["status", "--all"]);
    let child =
        all["jobs"].as_array().unwrap().iter().find(|j| j["parent"] == job.as_str()).expect("a child job").clone();
    assert_eq!(child["profile"], "desk");
    assert_eq!(child["state"], "succeeded");
    assert_eq!(child["labels"]["hufu.operator_for"], job.as_str());
    let (_, result) = env.json(&["result", child["job"].as_str().unwrap()]);
    let logs = env.hufu(&["logs", child["job"].as_str().unwrap(), "--raw"]).stdout;
    assert!(result["agent"]["profile"] == "desk" && !logs.is_empty());
    let child_spec: Value = serde_json::from_str(
        &std::fs::read_to_string(env.job_file(child["job"].as_str().unwrap(), "spec.json")).unwrap(),
    )
    .unwrap();
    assert!(
        child_spec["prompt"].as_str().unwrap().contains("Open the Cover frame")
            && child_spec["prompt"].as_str().unwrap().contains("Figma")
    );
    assert_eq!(child_spec["computer_use"]["route"], "executor");

    assert!(host.call_err("computer_use", json!({})).contains("instruction"));
    drop(host);
    env.hufu(&["cancel", &job]).expect(0);
    env.hufu(&["wait", &job, "--timeout", "30s"]).expect(4);
    let (_, parent) = env.json(&["result", &job]);
    assert_eq!(strings(&parent["children"]), vec![child["job"].as_str().unwrap().to_string()]);

    // Once the job is over, its hand-over tool is closed.
    let mut late = Host::start(&env, &["--child-of", &job]);
    late.handshake();
    assert!(late.call_err("computer_use", json!({"instruction": "anything"})).contains("ended"));

    // A job without a grant has no hand-over at all.
    let plain = env.start(&["-a", "codex", "[[fake:hang]]"]);
    env.until(&plain, "running", |s| s["state"] == "running");
    let mut none = Host::start(&env, &["--child-of", &plain]);
    none.handshake();
    assert!(none.call_err("computer_use", json!({"instruction": "anything"})).contains("grant"));
    drop(none);
    env.hufu(&["cancel", &plain, "--force"]).expect(0);
    env.hufu(&["wait", &plain, "--timeout", "30s"]).expect(4);
}

#[test]
fn a_mission_may_only_grant_what_the_user_allowed() {
    let env = Env::new("cu-mission");
    setup(&env, json!({"roles": {"advisor": "claude", "executors": ["codex"]}}));
    let plan = json!({"tasks": [{"id": "t1", "title": "Export", "prompt": "p", "agent": "codex", "grants": [{"capability": "computer_use", "apps": ["Figma"]}]}]});
    let goal = format!("Export the designs [[plan: {plan}]]");

    let (code, m) = env.json(&["mission", "start", "--wait", &goal]);
    assert_eq!(code, 3, "{m}");
    assert_eq!(m["reason"], "plan_invalid");

    setup(
        &env,
        json!({"roles": {"advisor": "claude", "executors": ["codex"]}, "missions": {"allowed_grants": [{"capability": "computer_use", "apps": ["Figma"]}]}}),
    );
    let (code, m) = env.json(&["mission", "start", "--wait", &goal]);
    assert_eq!(code, 0, "{m}");
    let job = m["tasks"][0]["jobs"][0].as_str().unwrap();
    let spec: Value = serde_json::from_str(&std::fs::read_to_string(env.job_file(job, "spec.json")).unwrap()).unwrap();
    assert_eq!(spec["computer_use"]["route"], "dedicated");
    assert_eq!(strings(&spec["computer_use"]["apps"]), vec!["Figma"]);

    // An app outside the allowance is refused even though the runtime would permit it.
    let plan = json!({"tasks": [{"id": "t1", "title": "Notes", "prompt": "p", "agent": "codex", "grants": [{"capability": "computer_use", "apps": ["Notes"]}]}]});
    let (code, m) = env.json(&["mission", "start", "--wait", &format!("Write notes [[plan: {plan}]]")]);
    assert_eq!(code, 3, "{m}");
    assert!(m["detail"].as_str().unwrap().contains("Notes"), "{m}");
}
