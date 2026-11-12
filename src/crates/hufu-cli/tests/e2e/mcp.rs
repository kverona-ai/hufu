//! The MCP server over stdio, driven the way a host drives it.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Stdio};

use serde_json::{Value, json};

use crate::common::{Env, HUFU, fake, strings};

/// A minimal MCP host.
pub struct Host {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    next: u64,
}

impl Host {
    pub fn start(env: &Env, args: &[&str]) -> Host {
        let mut child = env
            .command(&env.work)
            .arg("mcp")
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("hufu mcp starts");
        let stdin = child.stdin.take();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Host { child, stdin, stdout, next: 0 }
    }

    pub fn send(&mut self, message: &Value) {
        let stdin = self.stdin.as_mut().unwrap();
        writeln!(stdin, "{message}").unwrap();
        stdin.flush().unwrap();
    }

    pub fn read(&mut self) -> Value {
        let mut line = String::new();
        self.stdout.read_line(&mut line).unwrap();
        serde_json::from_str(&line)
            .unwrap_or_else(|e| panic!("the server wrote something that is not JSON ({e}): {line:?}"))
    }

    pub fn request(&mut self, method: &str, params: Value) -> Value {
        self.next += 1;
        let id = self.next;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        let answer = self.read();
        assert_eq!(answer["id"], id, "{answer}");
        assert_eq!(answer["jsonrpc"], "2.0");
        answer
    }

    pub fn handshake(&mut self) -> Value {
        let answer = self.request("initialize", json!({"protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "test-host", "version": "1"}}));
        self.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
        answer["result"].clone()
    }

    /// Calls a tool and returns its structured content, failing the test on a tool error.
    #[track_caller]
    pub fn call(&mut self, name: &str, arguments: Value) -> Value {
        let answer = self.request("tools/call", json!({"name": name, "arguments": arguments}));
        let result = &answer["result"];
        assert_eq!(result["isError"], false, "{name} failed: {answer}");
        assert_eq!(result["content"][0]["type"], "text");
        match result.get("structuredContent") {
            Some(v) => v.clone(),
            None => result["content"][0]["text"].clone(),
        }
    }

    /// Calls a tool that is expected to fail and returns the error text.
    #[track_caller]
    pub fn call_err(&mut self, name: &str, arguments: Value) -> String {
        let answer = self.request("tools/call", json!({"name": name, "arguments": arguments}));
        assert_eq!(answer["result"]["isError"], true, "{name} should have failed: {answer}");
        answer["result"]["content"][0]["text"].as_str().unwrap().to_string()
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        drop(self.stdin.take());
        let _ = self.child.wait();
    }
}

#[test]
fn the_handshake_and_the_tool_list_follow_the_protocol() {
    let env = Env::new("mcp-handshake");
    let mut host = Host::start(&env, &[]);
    let init = host.handshake();
    assert_eq!(init["protocolVersion"], "2025-11-25");
    assert_eq!(init["serverInfo"]["name"], "hufu");
    assert!(init["capabilities"]["tools"].is_object());
    assert!(init["instructions"].as_str().unwrap().contains("hufu_submit"));
    assert_eq!(host.request("ping", json!({}))["result"], json!({}));

    let tools = host.request("tools/list", json!({}))["result"]["tools"].as_array().unwrap().clone();
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    for wanted in [
        "hufu_agents",
        "hufu_submit",
        "hufu_status",
        "hufu_wait",
        "hufu_result",
        "hufu_logs",
        "hufu_cancel",
        "hufu_resume",
        "hufu_mission_start",
        "hufu_mission_status",
        "hufu_mission_abort",
        "hufu_mission_pause",
        "hufu_mission_resume",
        "hufu_doctor",
        "hufu_config_get",
        "hufu_config_set",
        "hufu_profile_set",
        "hufu_profile_remove",
        "hufu_pool_set",
        "hufu_pool_remove",
        "hufu_init",
        "hufu_setup",
    ] {
        assert!(names.contains(&wanted), "tool `{wanted}` is missing: {names:?}");
    }
    assert!(!names.contains(&"computer_use"), "the hand-over tool belongs to jobs only");
    for tool in &tools {
        assert_eq!(tool["inputSchema"]["type"], "object", "{tool}");
        assert!(tool["description"].as_str().unwrap().len() > 20);
    }

    // An older host gets its own revision back; an unknown one gets the newest the server knows.
    let old = host.request(
        "initialize",
        json!({"protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": {"name": "old", "version": "1"}}),
    );
    assert_eq!(old["result"]["protocolVersion"], "2024-11-05");
    let odd = host.request(
        "initialize",
        json!({"protocolVersion": "1999-01-01", "capabilities": {}, "clientInfo": {"name": "odd", "version": "1"}}),
    );
    assert_eq!(odd["result"]["protocolVersion"], "2025-11-25");

    // Errors use the protocol's codes, and garbage does not end the session.
    assert_eq!(host.request("no/such/method", json!({}))["error"]["code"], -32601);
    assert_eq!(host.request("tools/call", json!({"name": "hufu_nope", "arguments": {}}))["error"]["code"], -32602);
    host.send(&json!("not a request"));
    let stdin = host.stdin.as_mut().unwrap();
    writeln!(stdin, "{{ this is not json").unwrap();
    stdin.flush().unwrap();
    assert_eq!(host.read()["error"]["code"], -32700);
    assert_eq!(host.request("ping", json!({}))["result"], json!({}));

    // A batch gets a batch back; notifications get nothing.
    host.send(&json!([{"jsonrpc": "2.0", "id": 901, "method": "ping"}, {"jsonrpc": "2.0", "method": "notifications/cancelled"}, {"jsonrpc": "2.0", "id": 902, "method": "ping"}]));
    let batch = host.read();
    assert_eq!(batch.as_array().map(Vec::len), Some(2), "{batch}");
}

#[test]
fn a_host_without_a_handshake_is_served_too() {
    let env = Env::new("mcp-stateless");
    let mut host = Host::start(&env, &[]);
    let meta = |version: &str| json!({"_meta": {"io.modelcontextprotocol/protocolVersion": version}});
    let discover = host.request("server/discover", meta("2026-07-28"));
    assert!(strings(&discover["result"]["supportedVersions"]).contains(&"2026-07-28".to_string()), "{discover}");
    assert_eq!(discover["result"]["resultType"], "complete");
    let list = host.request("tools/list", meta("2026-07-28"));
    assert_eq!(list["result"]["resultType"], "complete");
    assert!(!list["result"]["tools"].as_array().unwrap().is_empty());
    let mut call = meta("2026-07-28");
    call["name"] = json!("hufu_agents");
    call["arguments"] = json!({});
    let agents = host.request("tools/call", call);
    assert_eq!(agents["result"]["isError"], false, "{agents}");
    assert_eq!(agents["result"]["resultType"], "complete");

    let refused = host.request("tools/list", meta("2031-01-01"));
    assert_eq!(refused["error"]["code"], -32022, "{refused}");
    assert!(strings(&refused["error"]["data"]["supported"]).contains(&"2025-11-25".to_string()));
}

#[test]
fn an_agent_can_dispatch_a_job_and_read_its_result() {
    let env = Env::new("mcp-jobs");
    let mut host = Host::start(&env, &[]);
    host.handshake();

    let agents = host.call("hufu_agents", json!({}));
    let names: Vec<&str> = agents["agents"].as_array().unwrap().iter().filter_map(|a| a["name"].as_str()).collect();
    assert!(names.contains(&"codex") && names.contains(&"acp"), "{names:?}");
    assert_eq!(agents["roles"]["advisor"], "claude");

    let dry = host.call("hufu_submit", json!({"agent": "codex", "prompt": "x", "dry_run": true}));
    assert_eq!(dry["dry_run"], true);
    assert!(host.call("hufu_status", json!({}))["recent"].as_array().unwrap().is_empty());

    let job = host.call("hufu_submit", json!({"agent": "codex", "prompt": "write it [[fake:write=from-mcp.txt]]", "permission": "workspace-write", "labels": {"via": "mcp"},
        "acceptance": [{"paths": {"allow": ["*.txt"]}}]}));
    let id = job["job"].as_str().unwrap().to_string();
    let waited = host.call("hufu_wait", json!({"job": id, "timeout_s": 30}));
    assert_eq!(waited["wait_timed_out"], false);
    assert_eq!(waited["jobs"][0]["state"], "succeeded");
    let result = host.call("hufu_result", json!({"job": id}));
    assert_eq!(result["verdict"], "accepted");
    assert_eq!(strings(&result["changes"]["files"]), vec!["from-mcp.txt"]);
    let logs = host.call("hufu_logs", json!({"job": id, "limit": 3}));
    assert_eq!(logs["events"].as_array().unwrap().len(), 3);
    assert_eq!(logs["more"], true);
    let rest = host.call("hufu_logs", json!({"job": id, "after": logs["last_seq"]}));
    assert!(rest["events"][0]["seq"].as_u64() > logs["last_seq"].as_u64());

    let follow = host.call("hufu_resume", json!({"job": id, "prompt": "one more thing"}));
    let follow_id = follow["job"].as_str().unwrap().to_string();
    host.call("hufu_wait", json!({"jobs": [follow_id]}));
    assert!(
        host.call("hufu_result", json!({"job": follow_id}))["final_message"]
            .as_str()
            .unwrap()
            .contains("resumed fake-session-1")
    );

    // A structured answer, a still-running job, and a cancellation.
    let structured = host.call("hufu_submit", json!({"agent": "claude", "prompt": "[[fake:json={\"n\":3}]]", "output_schema": {"type": "object", "required": ["n"], "properties": {"n": {"type": "integer"}}}}));
    host.call("hufu_wait", json!({"job": structured["job"]}));
    assert_eq!(host.call("hufu_result", json!({"job": structured["job"]}))["structured_output"], json!({"n": 3}));

    let hanging = host.call("hufu_submit", json!({"agent": "codex", "prompt": "[[fake:hang]]"}));
    let hid = hanging["job"].as_str().unwrap().to_string();
    assert_eq!(host.call("hufu_wait", json!({"job": hid, "timeout_s": 1}))["wait_timed_out"], true);
    assert_eq!(host.call("hufu_result", json!({"job": hid}))["finished"], false);
    assert_eq!(host.call("hufu_status", json!({}))["active"][0]["job"], hid);
    host.call("hufu_cancel", json!({"job": hid}));
    assert_eq!(host.call("hufu_wait", json!({"job": hid, "timeout_s": 30}))["jobs"][0]["state"], "cancelled");

    // Refusals come back as tool errors the model can read, not as protocol errors.
    assert!(host.call_err("hufu_submit", json!({"agent": "kimi", "prompt": "x"})).contains("full-access"));
    assert!(host.call_err("hufu_submit", json!({"agent": "codex"})).contains("prompt"));
    assert!(host.call_err("hufu_result", json!({"job": "j_nope"})).contains("j_nope"));
    assert!(host.call_err("hufu_submit", json!({"agent": "codex", "prompt": "x", "surprise": 1})).contains("surprise"));
}

#[test]
fn an_agent_can_configure_hufu() {
    let env = Env::new("mcp-config");
    let mut host = Host::start(&env, &[]);
    host.handshake();

    // Two accounts of one agent, a pool over them, and the roles.
    let home_a = env.root.join("acct-a").display().to_string();
    host.call("hufu_profile_set", json!({"name": "codex-work", "profile": {"adapter": "codex-cli", "command": fake("codex"), "account": "work", "home": home_a}}));
    host.call("hufu_profile_set", json!({"name": "codex-home", "profile": {"adapter": "codex-cli", "command": fake("codex"), "account": "home", "env_from": {"OPENAI_API_KEY": "HOME_KEY"}}}));
    host.call(
        "hufu_pool_set",
        json!({"name": "codex-any", "members": ["codex-work", "codex-home"], "strategy": "round-robin"}),
    );
    host.call("hufu_config_set", json!({"key": "roles.executors", "value": ["codex-any", "agy"]}));
    host.call("hufu_config_set", json!({"key": "roles.advisor", "value": "claude"}));
    host.call("hufu_config_set", json!({"key": "limits.max_concurrent_jobs", "value": 2}));

    assert_eq!(host.call("hufu_config_get", json!({"key": "roles.executors"}))["value"], json!(["codex-any", "agy"]));
    assert_eq!(host.call("hufu_config_get", json!({"key": "pools.codex-any.strategy"}))["value"], "round-robin");
    assert_eq!(host.call("hufu_config_get", json!({}))["value"]["limits"]["max_concurrent_jobs"], 2);
    let agents = host.call("hufu_agents", json!({}));
    let pool = agents["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["name"] == "codex-any")
        .expect("the pool is listed")
        .clone();
    assert_eq!(pool["kind"], "pool");
    assert_eq!(strings(&pool["members"]), vec!["codex-work", "codex-home"]);

    // A merge keeps what is not named; the new settings take effect for the next job.
    host.call("hufu_profile_set", json!({"name": "codex-work", "profile": {"model": "gpt-big"}}));
    assert_eq!(host.call("hufu_config_get", json!({"key": "profiles.codex-work.account"}))["value"], "work");
    let job = host.call("hufu_submit", json!({"agent": "codex-work", "prompt": "[[fake:env=CODEX_HOME]]"}));
    host.call("hufu_wait", json!({"job": job["job"]}));
    let result = host.call("hufu_result", json!({"job": job["job"]}));
    assert_eq!(result["final_message"], format!("CODEX_HOME={home_a}"));
    assert_eq!(result["agent"]["model"], "gpt-big");

    // Invalid changes are refused and leave the file as it was.
    let before = std::fs::read_to_string(env.home.join("config.yaml")).unwrap();
    assert!(host.call_err("hufu_config_set", json!({"key": "roles.advisor", "value": "nobody"})).contains("nobody"));
    assert!(host.call_err("hufu_config_set", json!({"key": "roles.advisor", "value": "codex-any"})).contains("pool"));
    assert!(
        host.call_err("hufu_config_set", json!({"key": "defaults.permission", "value": "full-access"}))
            .contains("full-access")
    );
    assert!(
        host.call_err("hufu_profile_set", json!({"name": "bad", "profile": {"adapter": "telepathy"}}))
            .contains("telepathy")
    );
    assert!(host.call_err("hufu_pool_set", json!({"name": "p2", "members": ["ghost"]})).contains("ghost"));
    assert!(host.call_err("hufu_profile_remove", json!({"name": "codex-work"})).contains("codex-any"));
    assert!(host.call_err("hufu_profile_set", json!({"name": "leaky", "profile": {"adapter": "codex-cli", "vars": {"OPENAI_API_KEY": "sk-live-abcdefghijklmnop"}}})).to_lowercase().contains("secret"));
    assert_eq!(std::fs::read_to_string(env.home.join("config.yaml")).unwrap(), before);

    host.call("hufu_config_set", json!({"key": "roles.executors", "value": ["agy"]}));
    host.call("hufu_pool_remove", json!({"name": "codex-any"}));
    host.call("hufu_profile_remove", json!({"name": "codex-work"}));
    host.call("hufu_config_set", json!({"key": "limits.max_concurrent_jobs", "value": null}));
    assert_eq!(host.call("hufu_config_get", json!({"key": "limits.max_concurrent_jobs"}))["value"], 4);

    // Doctor, with the self-test, and registration with another agent.
    let doctor = host.call("hufu_doctor", json!({"self_test": true}));
    assert!(
        doctor["profiles"].as_array().unwrap().iter().any(|p| p["name"] == "codex" && p["installed"] == true),
        "{doctor}"
    );
    assert!(doctor["self_test"].as_array().unwrap().iter().all(|t| t["ok"] == true), "{}", doctor["self_test"]);
    let printed = host.call("hufu_setup", json!({"agents": ["generic", "cursor"], "print": true}));
    assert!(printed["actions"][0]["detail"].as_str().unwrap().contains("mcpServers"));
    assert_eq!(printed["actions"][1]["action"], "manual");
}

#[test]
fn an_agent_can_run_a_mission() {
    let env = Env::new("mcp-mission");
    env.config(json!({"profiles": {"slow": {"adapter": "codex-cli", "command": fake("codex"), "vars": {"HUFU_FAKE_SCENARIO": "slow=1200"}}}}));
    let mut host = Host::start(&env, &[]);
    host.handshake();
    let plan = json!({"tasks": [
        {"id": "a", "title": "A", "prompt": "p", "agent": "slow"},
        {"id": "b", "title": "B", "prompt": "p", "agent": "codex", "depends_on": ["a"]},
    ]});
    let started = host.call(
        "hufu_mission_start",
        json!({"goal": format!("Do both [[plan: {plan}]]"), "executors": ["slow", "codex"], "review": "never"}),
    );
    let id = started["mission"].as_str().unwrap().to_string();
    assert_eq!(started["advisor"], "claude");
    let poll = |host: &mut Host, done: &dyn Fn(&Value) -> bool| -> Value {
        let began = std::time::Instant::now();
        loop {
            let status = host.call("hufu_mission_status", json!({"mission": id}));
            if done(&status) {
                return status;
            }
            assert!(began.elapsed().as_secs() < 60, "{status}");
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    };
    poll(&mut host, &|s| s["tasks"].get(0).map(|t| !t["jobs"].as_array().unwrap().is_empty()).unwrap_or(false));
    host.call("hufu_mission_pause", json!({"mission": id}));
    let paused = poll(&mut host, &|s| s["state"] == "paused");
    assert!(paused["tasks"][1]["jobs"].as_array().unwrap().is_empty());
    host.call("hufu_mission_resume", json!({"mission": id}));
    let done = poll(&mut host, &|s| s["state"] == "succeeded" || s["state"] == "failed");
    assert_eq!(done["state"], "succeeded", "{done}");
    assert!(done["report"].as_str().unwrap().contains("Do both"));
    assert_eq!(host.call("hufu_mission_status", json!({}))["missions"][0]["mission"], id);
    assert_eq!(
        host.call("hufu_mission_abort", json!({"mission": id}))["state"],
        "succeeded",
        "aborting an ended mission changes nothing"
    );
    assert!(host.call_err("hufu_mission_start", json!({"goal": "x", "advisor": "nobody"})).contains("nobody"));
}

#[test]
fn a_fresh_home_is_served_and_can_be_initialized() {
    // `hufu mcp` must answer on a fresh home with no configuration at all.
    let env = Env::new("mcp-empty");
    std::fs::remove_file(env.home.join("config.yaml")).unwrap();
    let mut host = Host::start(&env, &[]);
    host.handshake();
    let agents = host.call("hufu_agents", json!({}));
    assert!(
        agents["agents"].as_array().unwrap().iter().any(|a| a["name"] == "codex" && a["builtin"] == true),
        "the presets are there without a file: {agents}"
    );
    let init = host.call("hufu_init", json!({}));
    assert_eq!(init["action"], "created");
    assert!(env.home.join("config.yaml").exists());
    assert_eq!(host.call("hufu_init", json!({}))["action"], "kept");
    let _ = HUFU;
}
