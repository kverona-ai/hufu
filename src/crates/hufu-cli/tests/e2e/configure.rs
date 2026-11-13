//! Setting Hufu up: first configuration, profiles, pools, roles, registration with other agents.

use serde_json::{Value, json};

use crate::common::{Env, HUFU, fake, strings};

fn refused(env: &Env, args: &[&str], needle: &str) {
    let r = env.hufu(args);
    assert_eq!(r.code, 2, "{args:?}: {} {}", r.stdout, r.stderr);
    assert!(r.stderr.contains(needle), "{args:?}: expected `{needle}` in: {}", r.stderr);
}

/// A home with no configuration file, as on a fresh machine.
fn fresh(name: &str) -> Env {
    let env = Env::new(name);
    std::fs::remove_file(env.home.join("config.yaml")).unwrap();
    env
}

#[test]
fn a_fresh_machine_works_with_presets_and_init_writes_a_first_file() {
    let mut env = fresh("init");
    env.only_agents(&["codex", "agy"]);
    // Without any file there are presets for every supported agent.
    let (_, agents) = env.json(&["agents"]);
    let names: Vec<String> =
        agents["agents"].as_array().unwrap().iter().map(|a| a["name"].as_str().unwrap().to_string()).collect();
    for preset in ["codex", "agy", "cursor", "claude", "kimi", "pi", "kimi-acp", "cursor-acp"] {
        assert!(names.contains(&preset.to_string()), "preset `{preset}` is missing: {names:?}");
    }
    let (_, paths) = env.json(&["config", "path"]);
    assert_eq!(paths["global_exists"], false);
    env.hufu(&["config", "validate"]).expect(0);

    let (code, init) = env.json(&["init"]);
    assert_eq!(code, 0, "{init}");
    assert_eq!(init["action"], "created");
    assert_eq!(strings(&init["detected"]), vec!["codex", "agy"]);
    assert_eq!(init["advisor"], "codex");
    assert_eq!(strings(&init["executors"]), vec!["codex", "agy"]);
    assert!(init.get("hints").is_none(), "two agents need no advice: {init}");
    let text = std::fs::read_to_string(env.home.join("config.yaml")).unwrap();
    assert!(text.contains("version: 1") && text.contains("read-only"), "{text}");
    env.hufu(&["config", "validate"]).expect(0);
    assert_eq!(env.json(&["init"]).1["action"], "kept");
    env.hufu(&["config", "set", "defaults.timeout", "45m"]).expect(0);
    assert_eq!(env.json(&["init", "--force"]).1["action"], "replaced");
    assert_eq!(env.hufu(&["config", "get", "defaults.timeout"]).stdout.trim(), "30m", "a forced init starts over");
    let backups = std::fs::read_dir(&env.home)
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().contains("config.yaml."))
        .count();
    assert!(backups >= 1, "a replaced configuration must leave a backup");
}

#[test]
fn up_prepares_everything_in_one_command() {
    let mut env = fresh("up");
    env.at_least_agents(&["codex", "agy"]);
    let (code, up) = env.json(&["up", "--no-setup"]);
    let _ = env.hufu(&["daemon", "stop"]);
    assert_eq!(code, 0, "{up}");
    assert_eq!(up["ready"], true);
    assert_eq!(up["init"]["action"], "created");
    assert_eq!(up["daemon"]["running"], true);
    assert!(up["doctor"]["daemon"]["pid"].is_number(), "the report shows the daemon that was just started: {up}");
    assert!(up["doctor"]["profiles"].as_array().unwrap().len() >= 6);
    // Running it again changes nothing and still succeeds.
    let (code, again) = env.json(&["up", "--no-setup", "--no-daemon"]);
    assert_eq!(code, 0);
    assert_eq!(again["init"]["action"], "kept");
    assert_eq!(again["daemon"]["skipped"], true);
    let text = env.hufu(&["up", "--no-setup", "--no-daemon"]).expect(0).stdout;
    assert!(text.contains("ready. Try: hufu run -a codex "), "{text}");
    assert!(!text.contains("only one agent"), "{text}");
}

#[test]
fn up_without_any_agent_stops_and_says_what_to_install() {
    let mut env = fresh("up-none");
    env.only_agents(&[]);
    let (code, up) = env.json(&["up"]);
    assert_eq!(code, 10, "no agent is exit code 10: {up}");
    assert_eq!(up["ready"], false);
    assert_eq!(up["reason"], "no_agent");
    assert_eq!(up["init"]["action"], "skipped");
    assert!(up["next"].as_str().unwrap().contains("`hufu up` again"), "{up}");
    assert!(up["setup"].as_array().unwrap().is_empty(), "nothing is registered anywhere: {up}");
    assert_eq!(up["daemon"]["skipped"], true);
    assert_eq!(env.json(&["daemon", "status"]).1["running"], false, "no daemon is started for nothing");
    assert!(!env.home.join("config.yaml").exists(), "a file naming no agent would be kept by the next run");

    let r = env.hufu(&["up"]).expect(10);
    for needle in
        ["no agent to dispatch work to", "Install at least one of these", "codex login", "Then run `hufu up` again."]
    {
        assert!(r.stdout.contains(needle), "expected `{needle}` in: {}", r.stdout);
    }
    assert!(!r.stdout.contains("ready. Try"), "nothing is ready: {}", r.stdout);
    assert!(r.stderr.contains("not ready: no supported agent is installed"), "{}", r.stderr);

    // `hufu init` alone behaves the same way.
    let (code, init) = env.json(&["init"]);
    assert_eq!((code, init["action"].as_str()), (10, Some("skipped")), "{init}");
    assert!(strings(&init["hints"])[0].contains("Kimi Code (`kimi`)"), "{init}");

    // Once an agent is installed, the same command finishes the setup. No flag is needed.
    env.only_agents(&["pi"]);
    let (code, up) = env.json(&["up", "--no-daemon"]);
    assert_eq!(code, 0, "{up}");
    assert_eq!((up["ready"].as_bool(), up["init"]["action"].as_str()), (Some(true), Some("created")), "{up}");
}

#[test]
fn an_existing_file_that_names_no_agent_is_fixed_with_init_force() {
    let mut env = fresh("up-empty-file");
    env.only_agents(&[]);
    // `--force` writes the starter file even with nothing to name.
    let (code, init) = env.json(&["init", "--force"]);
    assert_eq!((code, init["action"].as_str()), (0, Some("created")), "{init}");
    let (_, doc) = env.json(&["doctor"]);
    assert!(
        strings(&doc["hints"]).iter().any(|h| h.contains("no supported agent") && h.contains("`hufu init --force`")),
        "plain `hufu init` would keep the file: {doc}"
    );
    let r = env.hufu(&["up"]).expect(10);
    assert!(r.stdout.contains("configuration kept") && r.stdout.contains("run `hufu init --force`"), "{}", r.stdout);

    // With an agent installed later, the file still names none, and doctor says how to refresh it.
    env.only_agents(&["codex"]);
    let (_, doc) = env.json(&["doctor"]);
    assert!(
        strings(&doc["hints"])
            .iter()
            .any(|h| h.contains("names no advisor and no executors") && h.contains("`hufu init --force`")),
        "{doc}"
    );
    let (code, init) = env.json(&["init", "--force"]);
    assert_eq!((code, init["action"].as_str()), (0, Some("replaced")), "{init}");
    assert_eq!(env.json(&["doctor"]).1["advisor"], "codex");
}

#[test]
fn up_with_one_agent_says_it_holds_both_roles() {
    let mut env = fresh("up-one");
    // pi can do the work but cannot host Hufu, so there is also nothing to register with.
    env.only_agents(&["pi"]);
    let (code, up) = env.json(&["up", "--no-daemon"]);
    assert_eq!(code, 0, "{up}");
    assert_eq!(up["ready"], true);
    assert_eq!(up["init"]["advisor"], "pi");
    assert_eq!(strings(&up["init"]["executors"]), vec!["pi"]);
    let single = |doc: &Value| strings(&doc["hints"]).into_iter().find(|h| h.contains("only one agent"));
    let hint = single(&up["doctor"]).unwrap_or_else(|| panic!("no single-agent hint in {up}"));
    assert!(
        hint.contains("`pi` is both the advisor and the only executor") && hint.contains("roles.executors"),
        "{hint}"
    );
    assert_eq!(up["setup"][0]["host"], "generic");
    assert_eq!(up["setup"][0]["action"], "manual");
    assert!(up["setup"][0]["detail"].as_str().unwrap().contains("mcpServers"), "{up}");

    let text = env.hufu(&["up", "--no-daemon"]).expect(0).stdout;
    for needle in [
        "no agent that could host Hufu was found",
        "\"mcpServers\"",
        "only one agent is in use",
        "ready. Try: hufu run -a pi ",
    ] {
        assert!(text.contains(needle), "expected `{needle}` in: {text}");
    }
    assert!(env.hufu(&["init", "--force"]).expect(0).stdout.contains("note: only one agent is in use"));

    // A second agent installed later is named in the hint, and using it ends the hint.
    env.only_agents(&["codex"]);
    let hint = single(&env.json(&["doctor"]).1).expect("still one agent in use");
    assert!(hint.contains("Also installed: codex") && hint.contains("\"[pi, codex]\""), "{hint}");
    env.hufu(&["config", "set", "roles.executors", "[pi, codex]"]).expect(0);
    assert_eq!(single(&env.json(&["doctor"]).1), None);
}

#[test]
fn configuration_can_be_read_and_changed_from_the_command_line() {
    let env = Env::new("config");
    assert_eq!(env.hufu(&["config", "get", "defaults.permission"]).expect(0).stdout.trim(), "read-only");
    env.hufu(&["config", "set", "defaults.timeout", "45m"]).expect(0);
    env.hufu(&["config", "set", "limits.max_concurrent_jobs", "2"]).expect(0);
    env.hufu(&["config", "set", "roles.executors", "[codex, agy]"]).expect(0);
    env.hufu(&["config", "set", "missions.review", "always"]).expect(0);
    env.hufu(&["config", "set", "defaults.retry", "{max_attempts: 2, on: [rate_limited, timeout]}"]).expect(0);
    let (_, all) = env.json(&["config", "get"]);
    let c = &all["value"];
    assert_eq!(c["defaults"]["timeout"], "45m");
    assert_eq!(c["limits"]["max_concurrent_jobs"], 2);
    assert_eq!(c["roles"]["executors"], json!(["codex", "agy"]));
    assert_eq!(c["missions"]["review"], "always");
    assert_eq!(c["defaults"]["retry"]["on"], json!(["rate_limited", "timeout"]));
    assert!(env.hufu(&["config", "get", "roles"]).stdout.contains("advisor: claude"));

    // What would break the configuration is refused, with the file untouched.
    let before = std::fs::read_to_string(env.home.join("config.yaml")).unwrap();
    refused(&env, &["config", "set", "defaults.timeout", "soon"], "soon");
    refused(&env, &["config", "set", "defaults.permission", "full-access"], "full-access");
    refused(&env, &["config", "set", "limits.max_concurrent_jobs", "0"], "at least 1");
    refused(&env, &["config", "set", "roles.advisor", "ghost"], "ghost");
    refused(&env, &["config", "set", "no.such.key", "1"], "no");
    refused(&env, &["config", "get", "no.such.key"], "no.such.key");
    assert_eq!(std::fs::read_to_string(env.home.join("config.yaml")).unwrap(), before);

    env.hufu(&["config", "unset", "defaults.timeout"]).expect(0);
    assert_eq!(env.hufu(&["config", "get", "defaults.timeout"]).stdout.trim(), "30m");

    // A project file overrides the user-wide one, for that project only.
    env.hufu(&["config", "set", "--project", "defaults.timeout", "5m"]).expect(0);
    assert!(env.work.join(".hufu").join("config.yaml").exists());
    assert_eq!(env.hufu(&["config", "get", "defaults.timeout"]).stdout.trim(), "5m");
    assert_eq!(env.hufu_in(&env.root, &["config", "get", "defaults.timeout"]).stdout.trim(), "30m");
    let nested = env.work.join("deep").join("er");
    std::fs::create_dir_all(&nested).unwrap();
    assert_eq!(
        env.hufu_in(&nested, &["config", "get", "defaults.timeout"]).stdout.trim(),
        "5m",
        "the project file is found from a sub-directory"
    );
    let (_, paths) = env.json(&["config", "path"]);
    assert!(paths["project"].as_str().unwrap().ends_with("config.yaml"));

    // A broken file is reported with every problem, and nothing runs on it.
    std::fs::write(
        env.home.join("config.yaml"),
        "version: 1\nprofiles:\n  x: {adapter: nope}\nroles: {advisor: y}\nlimits: {max_concurrent_jobs: 0}\n",
    )
    .unwrap();
    let r = env.hufu(&["run", "-a", "codex", "x"]);
    assert_eq!(r.code, 2, "{}", r.stderr);
    for needle in ["nope", "roles.advisor", "max_concurrent_jobs"] {
        assert!(r.stderr.contains(needle), "every problem is listed at once; missing `{needle}`: {}", r.stderr);
    }
    std::fs::write(env.home.join("config.yaml"), "version: 1\nsurprise: true\n").unwrap();
    refused(&env, &["agents"], "surprise");
    std::fs::write(env.home.join("config.yaml"), "version: [1\n").unwrap();
    refused(&env, &["agents"], "config.yaml");
}

#[test]
fn several_accounts_of_one_agent_become_profiles_and_a_pool() {
    let env = Env::new("profiles");
    let fake_codex = format!("{HUFU},__fake-agent,codex");
    let home_a = env.root.join("codex-work").display().to_string();
    env.hufu(&[
        "profile",
        "add",
        "codex-work",
        "--adapter",
        "codex-cli",
        "--command",
        &fake_codex,
        "--account",
        "work",
        "--home",
        &home_a,
        "--model",
        "gpt-big",
        "--effort",
        "high",
    ])
    .expect(0);
    env.hufu(&[
        "profile",
        "add",
        "codex-me",
        "--adapter",
        "codex-cli",
        "--command",
        &fake_codex,
        "--account",
        "me",
        "--env-from",
        "OPENAI_API_KEY=MY_OTHER_KEY",
        "--var",
        "HUFU_FAKE_SCENARIO=ok",
    ])
    .expect(0);
    env.hufu(&[
        "profile",
        "add",
        "api",
        "--adapter",
        "pi",
        "--provider",
        "openrouter",
        "--model",
        "some/model",
        "--env",
        "OPENROUTER_API_KEY",
        "--description",
        "An API key instead of a subscription",
    ])
    .expect(0);
    env.hufu(&[
        "pool",
        "add",
        "codex-any",
        "--member",
        "codex-work,codex-me",
        "--strategy",
        "least-busy",
        "--description",
        "Either Codex account",
    ])
    .expect(0);
    env.hufu(&["config", "set", "roles.executors", "[codex-any, api, agy]"]).expect(0);

    // A project file may build on what the user-wide file defines.
    env.hufu(&["config", "set", "--project", "roles.advisor", "codex-work"]).expect(0);
    env.hufu(&["pool", "add", "--project", "project-pool", "--member", "codex-me,api"]).expect(0);
    assert_eq!(env.hufu(&["config", "get", "roles.advisor"]).stdout.trim(), "codex-work");
    assert_eq!(env.hufu_in(&env.root, &["config", "get", "roles.advisor"]).stdout.trim(), "claude");
    refused(&env, &["config", "set", "--project", "roles.advisor", "ghost"], "ghost");
    std::fs::remove_dir_all(env.work.join(".hufu")).unwrap();

    let (_, shown) = env.json(&["profile", "show", "codex-work"]);
    assert_eq!(shown["profile"]["account"], "work");
    assert_eq!(shown["profile"]["home"], home_a);
    assert_eq!(shown["profile"]["effort"], "high");
    assert_eq!(strings(&shown["agent"]["pools"]), vec!["codex-any"]);
    assert_eq!(shown["agent"]["probe"]["installed"], true);
    let (_, api) = env.json(&["profile", "show", "api"]);
    assert_eq!(api["profile"]["provider"], "openrouter");
    assert_eq!(strings(&api["profile"]["env"]), vec!["OPENROUTER_API_KEY"]);
    let (_, pools) = env.json(&["pool", "list"]);
    assert_eq!(pools["pools"]["codex-any"]["strategy"], "least-busy");

    let listing = env.hufu(&["agents"]).expect(0).stdout;
    assert!(listing.contains("codex-any") && listing.contains("pool of codex-work, codex-me"), "{listing}");
    assert!(listing.contains("executors: codex-any, api, agy"), "{listing}");
    let probed = env.hufu(&["agents", "--probe"]).expect(0).stdout;
    assert!(probed.contains("STATUS"), "{probed}");

    // The profile's settings reach the agent.
    let (code, r) = env.run(&["-a", "codex-work", "[[fake:args]]"]);
    assert_eq!(code, 0, "{r}");
    assert_eq!(r["agent"]["model"], "gpt-big");
    assert_eq!(r["agent"]["effort"], "high");
    let argv = r["final_message"].as_str().unwrap();
    assert!(argv.contains("gpt-big") && argv.contains("high"), "{argv}");
    let (_, r) = env.run(&["-a", "codex-any", "[[fake:env=CODEX_HOME]]"]);
    assert_eq!(r["final_message"], format!("CODEX_HOME={home_a}"));

    // `set` changes some fields and keeps the rest; a preset is copied into the file on first change.
    env.hufu(&["profile", "set", "codex-work", "--model", "gpt-small"]).expect(0);
    let (_, shown) = env.json(&["profile", "show", "codex-work"]);
    assert_eq!(
        (shown["profile"]["model"].as_str(), shown["profile"]["account"].as_str()),
        (Some("gpt-small"), Some("work"))
    );
    env.hufu(&["profile", "set", "kimi-acp", "--verified"]).expect(0);
    assert_eq!(env.json(&["profile", "show", "kimi-acp"]).1["profile"]["permission_requests"], "verified");
    assert_eq!(env.json(&["profile", "show", "kimi-acp"]).1["profile"]["command"], json!(["kimi", "acp"]));

    refused(&env, &["profile", "add", "x"], "--adapter");
    refused(&env, &["profile", "add", "x", "--adapter", "smoke-signals"], "smoke-signals");
    refused(&env, &["profile", "add", "x", "--adapter", "acp"], "command");
    refused(
        &env,
        &["profile", "add", "x", "--adapter", "codex-cli", "--permission", "full-access"],
        "allow_full_access",
    );
    refused(
        &env,
        &["profile", "add", "x", "--adapter", "codex-cli", "--var", "OPENAI_API_KEY=sk-not-a-real-key-000000"],
        "secret",
    );
    refused(&env, &["profile", "add", "x", "--adapter", "codex-cli", "--env-from", "broken"], "env-from");
    refused(
        &env,
        &["profile", "add", "x", "--adapter", "external", "--command", "some-adapter", "--home", "/somewhere"],
        "home",
    );
    refused(&env, &["profile", "set", "nobody", "--model", "m"], "nobody");
    refused(&env, &["profile", "add", "codex-any", "--adapter", "codex-cli"], "pool");
    refused(&env, &["pool", "add", "p", "--member", "ghost"], "ghost");
    refused(&env, &["pool", "add", "p", "--member", "codex-any"], "codex-any");
    refused(&env, &["pool", "add", "p", "--member", "codex-me", "--strategy", "random"], "random");
    refused(&env, &["profile", "remove", "codex-me"], "codex-any");
    refused(&env, &["pool", "remove", "codex-any"], "roles.executors");
    refused(&env, &["profile", "show", "ghost"], "ghost");

    env.hufu(&["config", "set", "roles.executors", "[agy]"]).expect(0);
    env.hufu(&["pool", "remove", "codex-any"]).expect(0);
    env.hufu(&["profile", "remove", "codex-me"]).expect(0);
    env.hufu(&["config", "validate"]).expect(0);
    refused(&env, &["run", "-a", "codex-me", "x"], "codex-me");
}

#[test]
fn hufu_registers_itself_with_other_agents() {
    let mut env = Env::new("setup");
    let root = env.root.clone();
    let dir = |name: &str| root.join(name);
    for (var, name) in [
        ("CURSOR_CONFIG_DIR", "cursor"),
        ("CODEX_HOME", "codex"),
        ("KIMI_CODE_HOME", "kimi"),
        ("HUFU_AGY_CONFIG_DIR", "agy"),
    ] {
        let path = dir(name).display().to_string();
        env.set(var, &path);
    }
    // Existing settings of the user must survive.
    std::fs::create_dir_all(dir("cursor")).unwrap();
    std::fs::write(
        dir("cursor").join("mcp.json"),
        r#"{"mcpServers": {"other": {"command": "other-server"}}, "keep": true}"#,
    )
    .unwrap();
    std::fs::create_dir_all(dir("codex")).unwrap();
    std::fs::write(dir("codex").join("config.toml"), "model = \"gpt-x\"\n\n[mcp_servers.other]\ncommand = \"other\"\n")
        .unwrap();

    let hosts = ["--agent", "cursor", "--agent", "codex", "--agent", "kimi", "--agent", "agy"];
    let with = |extra: &[&'static str]| -> Vec<&'static str> {
        ["setup"].iter().chain(hosts.iter()).chain(extra.iter()).copied().collect()
    };

    // `--print` shows what would happen and writes nothing.
    let printed = env.hufu(&with(&["--print"])).expect(0).stdout;
    assert!(printed.contains("mcp_servers.hufu") && printed.contains("mcpServers"), "{printed}");
    assert!(
        !dir("kimi").exists() && !std::fs::read_to_string(dir("cursor").join("mcp.json")).unwrap().contains("hufu")
    );

    let (code, done) = env.json(&with(&[]));
    assert_eq!(code, 0, "{done}");
    let actions: Vec<(String, String)> = done["actions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| (a["host"].as_str().unwrap().to_string(), a["action"].as_str().unwrap().to_string()))
        .collect();
    assert_eq!(actions.len(), 4);
    assert!(actions.iter().all(|(_, action)| action == "registered"), "{actions:?}");

    let cursor: Value =
        serde_json::from_str(&std::fs::read_to_string(dir("cursor").join("mcp.json")).unwrap()).unwrap();
    assert_eq!(cursor["mcpServers"]["hufu"]["args"], json!(["mcp"]));
    assert!(std::path::Path::new(cursor["mcpServers"]["hufu"]["command"].as_str().unwrap()).is_absolute());
    assert_eq!(cursor["mcpServers"]["other"]["command"], "other-server");
    assert_eq!(cursor["keep"], true);
    assert!(dir("cursor").join("mcp.json.hufu-bak").exists(), "the original file is kept as a backup");
    let codex = std::fs::read_to_string(dir("codex").join("config.toml")).unwrap();
    assert!(
        codex.contains("model = \"gpt-x\"")
            && codex.contains("[mcp_servers.other]")
            && codex.contains("[mcp_servers.hufu]"),
        "{codex}"
    );
    assert!(dir("kimi").join("mcp.json").exists() && dir("agy").join("mcp_config.json").exists());

    // The registered command really is an MCP server.
    let command = cursor["mcpServers"]["hufu"]["command"].as_str().unwrap();
    let mut server = std::process::Command::new(command)
        .arg("mcp")
        .env("HUFU_HOME", &env.home)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    {
        use std::io::Write;
        let stdin = server.stdin.as_mut().unwrap();
        writeln!(stdin, "{}", json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "t", "version": "1"}}})).unwrap();
    }
    let out = server.wait_with_output().unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).contains("\"serverInfo\""));

    // Idempotent, and removable without touching anything else.
    let (_, again) = env.json(&with(&[]));
    assert!(again["actions"].as_array().unwrap().iter().all(|a| a["action"] == "unchanged"), "{again}");
    let (_, removed) = env.json(&with(&["--remove"]));
    assert!(removed["actions"].as_array().unwrap().iter().all(|a| a["action"] == "removed"), "{removed}");
    let cursor: Value =
        serde_json::from_str(&std::fs::read_to_string(dir("cursor").join("mcp.json")).unwrap()).unwrap();
    assert!(cursor["mcpServers"].get("hufu").is_none() && cursor["mcpServers"]["other"].is_object());
    let codex = std::fs::read_to_string(dir("codex").join("config.toml")).unwrap();
    assert!(!codex.contains("hufu") && codex.contains("[mcp_servers.other]"), "{codex}");
    assert!(env.json(&with(&["--remove"])).1["actions"].as_array().unwrap().iter().all(|a| a["action"] == "absent"));

    // A file Hufu cannot read safely is left alone, with the snippet to add by hand.
    std::fs::write(dir("cursor").join("mcp.json"), "{ // comments are not JSON\n}").unwrap();
    let (code, skipped) = env.json(&["setup", "--agent", "cursor"]);
    assert_eq!(code, 0);
    assert_eq!(skipped["actions"][0]["action"], "skipped");
    assert!(std::fs::read_to_string(dir("cursor").join("mcp.json")).unwrap().contains("comments are not JSON"));
    // A hand-written Codex entry is not overwritten either.
    std::fs::write(dir("codex").join("config.toml"), "[mcp_servers.hufu]\ncommand = \"mine\"\n").unwrap();
    assert_eq!(env.json(&["setup", "--agent", "codex"]).1["actions"][0]["action"], "skipped");

    // Any other MCP host gets the snippet; an unknown name is an error.
    assert!(env.hufu(&["setup", "--agent", "generic"]).expect(0).stdout.contains("\"mcpServers\""));
    assert_eq!(env.hufu(&["setup", "--agent", "emacs"]).code, 1);

    // Project scope: files in the repository, including the skill for Claude Code. Nothing is run.
    let (code, project) = env.json(&["setup", "--agent", "claude", "--agent", "cursor", "--project"]);
    assert_eq!(code, 0, "{project}");
    let mcp: Value = serde_json::from_str(&std::fs::read_to_string(env.work.join(".mcp.json")).unwrap()).unwrap();
    assert_eq!(mcp["mcpServers"]["hufu"]["args"], json!(["mcp"]));
    let skill = std::fs::read_to_string(env.work.join(".claude").join("skills").join("hufu").join("SKILL.md")).unwrap();
    assert!(
        skill.starts_with("---") && skill.contains("hufu_submit"),
        "the skill needs front matter and must name the tools"
    );
    assert!(env.work.join(".cursor").join("mcp.json").exists());
    env.hufu(&["setup", "--agent", "claude", "--agent", "cursor", "--project", "--remove"]).expect(0);
    assert!(!env.work.join(".claude").join("skills").join("hufu").join("SKILL.md").exists());
    // For Claude Code's user scope, `--print` shows the one command instead of running it.
    assert!(
        env.hufu(&["setup", "--agent", "claude", "--print"])
            .expect(0)
            .stdout
            .contains("claude mcp add --scope user hufu")
    );
}

#[test]
fn doctor_reports_the_state_of_the_installation() {
    let env = Env::new("doctor");
    env.config(json!({"profiles": {
        "logged-out": {"adapter": "codex-cli", "command": [HUFU, "__fake-agent", "codex"], "env_from": {"OPENAI_API_KEY": "HUFU_TEST_KEY_THAT_IS_NOT_SET"}},
        "missing": {"adapter": "agy-cli", "command": ["hufu-no-such-agent-binary"]},
    }}));
    let (code, doc) = env.json(&["doctor"]);
    assert_eq!(code, 0, "{doc}");
    let profile = |name: &str| {
        doc["profiles"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["name"] == name)
            .unwrap_or_else(|| panic!("no {name} in {doc}"))
            .clone()
    };
    assert_eq!(profile("codex")["installed"], true);
    assert_eq!(profile("missing")["installed"], false);
    assert_eq!(strings(&profile("logged-out")["missing_env"]), vec!["HUFU_TEST_KEY_THAT_IS_NOT_SET"]);
    assert_eq!(doc["advisor"], "claude");
    assert!(strings(&doc["hints"]).iter().any(|h| h.contains("HUFU_TEST_KEY_THAT_IS_NOT_SET")), "{doc}");
    let text = env.hufu(&["doctor"]).expect(0).stdout;
    assert!(text.contains("PROFILE") && text.contains("not installed") && text.contains("next steps"), "{text}");

    // The self-test runs real jobs against the built-in fake agents, in a home of its own.
    let (code, doc) = env.json(&["doctor", "--self-test"]);
    assert_eq!(code, 0, "{doc}");
    let tests = doc["self_test"].as_array().unwrap();
    assert!(tests.len() >= 3 && tests.iter().all(|t| t["ok"] == true), "{tests:?}");
    assert!(
        env.json(&["status", "--all"]).1["jobs"].as_array().unwrap().is_empty(),
        "the self-test must not leave jobs in the user's home"
    );
}

#[test]
fn the_documented_examples_are_valid() {
    let examples = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../doc/examples");
    let env = Env::new("examples");
    env.git_init();
    for name in ["task.yaml", "mission.yaml"] {
        std::fs::copy(examples.join(name), env.work.join(name)).unwrap();
    }

    // The sample configuration, exactly as documented.
    std::fs::copy(examples.join("config.yaml"), env.home.join("config.yaml")).unwrap();
    let (code, doc) = env.json(&["config", "validate"]);
    assert_eq!((code, doc["valid"].as_bool()), (0, Some(true)), "{doc}");
    let (_, agents) = env.json(&["agents"]);
    assert_eq!(agents["roles"]["advisor"], "claude", "{agents}");
    assert_eq!(strings(&agents["roles"]["executors"]), vec!["codex-any", "agy-fast", "kimi"]);
    assert_eq!(agents["computer_use"]["agent"], "desk", "{agents}");

    // The sample task names a pool from that configuration.
    let (code, plan) = env.json(&["run", "-f", "task.yaml", "--dry-run"]);
    assert_eq!(code, 0, "{plan}");
    assert!(["codex-high", "codex-work"].contains(&plan["profile"].as_str().unwrap()), "{plan}");
    assert_eq!(plan["permission"], "workspace-write");

    // The sample mission, with the same names driven by fake agents so that it can really start.
    env.config(json!({
        "profiles": {
            "codex-high": {"adapter": "codex-cli", "command": fake("codex")},
            "codex-work": {"adapter": "codex-cli", "command": fake("codex")},
            "agy-fast": {"adapter": "agy-cli", "command": fake("agy")},
            "kimi": {"adapter": "acp", "command": fake("acp"), "permission_requests": "verified"},
        },
        "pools": {"codex-any": {"members": ["codex-high", "codex-work"]}},
    }));
    let (code, started) = env.json(&["mission", "start", "-f", "mission.yaml"]);
    assert_eq!(code, 0, "{started}");
    let id = started["id"].as_str().expect("a mission id").to_string();
    env.hufu(&["mission", "abort", &id]).expect(0);
    env.hufu(&["mission", "wait", &id, "--timeout", "60s"]);
    let (_, ended) = env.json(&["mission", "status", &id]);
    assert_eq!(strings(&ended["executors"]), vec!["codex-any", "kimi", "agy-fast"], "{ended}");
}
