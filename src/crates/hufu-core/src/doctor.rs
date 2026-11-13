//! `hufu doctor`: what is installed, what is logged in, what is misconfigured, and a self-test
//! that exercises the whole pipeline with built-in fake agents.

use std::time::{Duration, Instant};

use hufu_contract::{AdapterRegistry, Tier, proc};
use serde::Serialize;

use crate::capability::COMPUTER_USE;
use crate::config::{self, Home, Loaded};
use crate::spec::TaskSpec;
use crate::store::{self, State};
use crate::submit::{self, Context, DaemonInfo, Options};
use crate::{runner, setup};

#[derive(Clone, Debug, Serialize)]
pub struct ProfileCheck {
    pub name: String,
    pub adapter: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    pub builtin: bool,
    pub roles: Vec<&'static str>,
    pub installed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// `None` when the login state cannot be read without running the agent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authenticated: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    /// Variables the profile reads from the environment that are not set.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub missing_env: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ComputerUseCheck {
    pub route: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// Profiles that declare `provides.computer_use`.
    pub capable: Vec<String>,
    pub ready: bool,
    pub detail: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct DoctorReport {
    pub version: String,
    pub exe: String,
    pub home: String,
    pub config_files: Vec<String>,
    /// Configuration errors. Empty means the configuration is valid.
    pub problems: Vec<String>,
    pub advisor: Option<String>,
    pub executors: Vec<String>,
    pub profiles: Vec<ProfileCheck>,
    pub computer_use: ComputerUseCheck,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub daemon: Option<DaemonInfo>,
    pub running_jobs: usize,
    pub queued_jobs: usize,
    /// Agents that could call Hufu over MCP and look installed.
    pub hosts: Vec<String>,
    /// What to do next, most important first.
    pub hints: Vec<String>,
}

/// Collects the report. Probes run side by side, so a slow agent does not hold up the rest.
pub fn doctor(home: &Home, loaded: &Loaded, adapters: &dyn AdapterRegistry, version: &str) -> DoctorReport {
    let config = &loaded.config;
    let views = crate::ops::agents(loaded, adapters, false);
    let mut profiles: Vec<ProfileCheck> = std::thread::scope(|scope| {
        let handles: Vec<_> = config
            .profiles
            .iter()
            .map(|(name, profile)| {
                let roles = views.iter().find(|v| &v.name == name).map(|v| v.roles.clone()).unwrap_or_default();
                let builtin = loaded.builtin.contains(name);
                scope.spawn(move || {
                    let probe = adapters.get(&profile.adapter).map(|a| a.probe(profile)).unwrap_or_default();
                    ProfileCheck {
                        name: name.clone(),
                        adapter: profile.adapter.clone(),
                        account: profile.account.clone(),
                        builtin,
                        roles,
                        installed: probe.installed,
                        version: probe.version,
                        authenticated: probe.authenticated,
                        detail: probe.detail,
                        hint: probe.hint,
                        missing_env: submit::missing_env(profile),
                    }
                })
            })
            .collect();
        handles.into_iter().filter_map(|h| h.join().ok()).collect()
    });
    profiles.sort_by(|a, b| a.name.cmp(&b.name));

    let cu = &config.capabilities.computer_use;
    let capable: Vec<String> =
        config.profiles.iter().filter(|(_, p)| p.provides.contains_key(COMPUTER_USE)).map(|(n, _)| n.clone()).collect();
    let (ready, detail) = match (&cu.agent, capable.is_empty()) {
        (Some(agent), _) => {
            let installed = profiles.iter().find(|p| &p.name == agent).map(|p| p.installed).unwrap_or(false);
            let via = config.profile(agent).and_then(|p| p.provides.get(COMPUTER_USE)).map(|p| p.via.clone()).unwrap_or_default();
            let runtime = via != "lcu" || proc::which("lcu").is_some();
            match (installed, runtime) {
                (true, true) => (true, format!("profile `{agent}` serves computer use through `{via}`")),
                (false, _) => (false, format!("profile `{agent}` is configured for computer use, but its agent is not installed")),
                (_, false) => (false, format!("profile `{agent}` uses the LCU runtime, but `lcu` is not on PATH")),
            }
        }
        (None, true) => (false, "not configured. Declare `provides: {computer_use: {via: native|lcu}}` on a profile that can operate the desktop, then set `capabilities.computer_use.agent`".into()),
        (None, false) => (true, format!("no dedicated agent is set; tasks can use the executor route with: {}", capable.join(", "))),
    };

    let jobs = store::list_jobs(home);
    let mut hints = Vec::new();
    if !loaded.global_path.exists() && loaded.project_path.is_none() {
        hints.push(
            "no configuration file yet: run `hufu init` to write one from the agents found on this machine".into(),
        );
    }
    let any_installed = profiles.iter().any(|p| p.installed);
    let no_roles = config.roles.advisor.is_none() && config.roles.executors.is_empty();
    if !any_installed {
        // `hufu init` keeps an existing file, so a file that names no agent needs `--force`.
        hints.push(setup::no_agent_hint(loaded.global_path.exists() && no_roles));
    }
    if any_installed && no_roles && loaded.global_path.exists() {
        hints.push("the configuration names no advisor and no executors although an agent is installed: `hufu init --force` rewrites it from the agents found, or set `roles.advisor` and `roles.executors` with `hufu config set`".into());
    } else if config.roles.advisor.is_none() {
        hints.push("no default advisor: `hufu config set roles.advisor <profile>` lets `hufu mission start` run without `--advisor`".into());
    }
    for p in profiles.iter().filter(|p| !p.builtin || !p.roles.is_empty()) {
        if !p.installed {
            hints.push(format!(
                "profile `{}`: the agent is not installed. {}",
                p.name,
                p.hint.clone().unwrap_or_default()
            ));
        } else if p.authenticated == Some(false) {
            hints.push(format!("profile `{}`: not logged in. {}", p.name, p.hint.clone().unwrap_or_default()));
        }
        if !p.missing_env.is_empty() {
            hints.push(format!("profile `{}`: set {} in the shell that runs hufu", p.name, p.missing_env.join(", ")));
        }
    }
    // Last, because nothing is broken: one profile that is the advisor and the only executor.
    if let (Some(advisor), [only]) = (&config.roles.advisor, config.roles.executors.as_slice())
        && advisor == only
        && config.profile(advisor).is_some()
    {
        let others: Vec<String> = profiles
            .iter()
            .filter(|p| p.installed && &p.name != advisor)
            .filter(|p| !p.builtin || setup::PRESET_ORDER.contains(&p.name.as_str()))
            .map(|p| p.name.clone())
            .collect();
        hints.push(setup::single_agent_hint(advisor, &others));
    }
    DoctorReport {
        version: version.to_string(),
        exe: submit::hufu_exe().map(|p| p.display().to_string()).unwrap_or_default(),
        home: home.root.display().to_string(),
        config_files: std::iter::once(&loaded.global_path)
            .chain(loaded.project_path.iter())
            .filter(|p| p.exists())
            .map(|p| p.display().to_string())
            .collect(),
        problems: config.problems(adapters),
        advisor: config.roles.advisor.clone(),
        executors: config.roles.executors.clone(),
        profiles,
        computer_use: ComputerUseCheck {
            route: format!("{:?}", cu.route).to_lowercase(),
            agent: cu.agent.clone(),
            capable,
            ready,
            detail,
        },
        daemon: submit::daemon_info(home),
        running_jobs: jobs.iter().filter(|j| matches!(j.state, State::Running | State::Starting)).count(),
        queued_jobs: jobs.iter().filter(|j| j.state == State::Queued).count(),
        hosts: setup::detect_hosts(),
        hints,
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct SelfTest {
    pub name: &'static str,
    pub ok: bool,
    pub detail: String,
}

/// Runs real jobs against the built-in fake agents in a throwaway home. Needs no agent and no
/// network, so it tells apart "Hufu is broken here" from "an agent is misconfigured".
pub fn self_test(adapters: &dyn AdapterRegistry) -> Vec<SelfTest> {
    let mut out = Vec::new();
    let root = std::env::temp_dir().join(format!("hufu-selftest-{}", hufu_contract::time::new_id("s")));
    let home = Home::at(root.join("home"));
    let work = root.join("work");
    let setup = (|| -> crate::error::Result<Loaded> {
        std::fs::create_dir_all(&work)?;
        home.ensure()?;
        let exe = submit::hufu_exe()?.display().to_string();
        let yaml = serde_json::json!({
            "profiles": {
                "selftest-cli": {"adapter": "codex-cli", "command": [exe, "__fake-agent", "codex"]},
                "selftest-acp": {"adapter": "acp", "command": [exe, "__fake-agent", "acp"], "permission_requests": "verified"}
            }
        });
        std::fs::write(home.config_file(), yaml.to_string())?;
        config::load(&home, &work)
    })();
    let loaded = match setup {
        Ok(l) => l,
        Err(e) => {
            out.push(SelfTest { name: "prepare a throwaway home", ok: false, detail: e.message });
            return out;
        }
    };
    let ctx = Context { home: &home, config: &loaded.config, adapters };
    let opts = Options { base_dir: work.clone(), ..Options::default() };
    let submit_task = |agent: &str, prompt: &str, tier: Tier| {
        let task = TaskSpec {
            prompt: prompt.into(),
            agent: agent.into(),
            permission: Some(tier),
            timeout: Some("60s".into()),
            ..TaskSpec::default()
        };
        submit::submit(&ctx, &task, &opts)
    };
    let limit = Some(Duration::from_secs(45));

    // 1. A job runs in its own process and leaves a result.
    let check = submit_task("selftest-cli", "self-test", Tier::ReadOnly).and_then(|job| {
        let state = runner::wait(&home, &job.id, limit)?;
        let result = runner::result_of(&home, &job.id)?;
        let message = result.and_then(|r| r.final_message).unwrap_or_default();
        Ok((
            state.state == State::Succeeded && message.contains("fake codex ok"),
            format!("state {}, final message `{message}`", state.state.as_str()),
        ))
    });
    out.push(match check {
        Ok((ok, detail)) => SelfTest { name: "run a job in the background and read its result", ok, detail },
        Err(e) => SelfTest { name: "run a job in the background and read its result", ok: false, detail: e.message },
    });

    // 2. Permission requests are answered by policy: the edit inside the workspace is allowed, the command is not.
    let check = submit_task("selftest-acp", "self-test [[fake:permission]]", Tier::WorkspaceWrite).and_then(|job| {
        let state = runner::wait(&home, &job.id, limit)?;
        let result = runner::result_of(&home, &job.id)?;
        let denied = result.map(|r| r.denied_actions.len()).unwrap_or(0);
        let written = work.join("fake-output.txt").exists();
        Ok((
            state.state == State::Succeeded && written && denied == 1,
            format!("state {}, file written: {written}, denied actions: {denied}", state.state.as_str()),
        ))
    });
    out.push(match check {
        Ok((ok, detail)) => SelfTest { name: "enforce workspace-write over ACP", ok, detail },
        Err(e) => SelfTest { name: "enforce workspace-write over ACP", ok: false, detail: e.message },
    });

    // 3. Cancelling stops a hanging agent.
    let check = submit_task("selftest-cli", "self-test [[fake:hang]]", Tier::ReadOnly).and_then(|job| {
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(20)
            && store::load_state(&home, &job.id).map(|s| s.agent_pid.is_none()).unwrap_or(true)
        {
            std::thread::sleep(Duration::from_millis(50));
        }
        let pid = store::load_state(&home, &job.id)?.agent_pid;
        runner::cancel(&home, &job.id)?;
        let state = runner::wait(&home, &job.id, limit)?;
        let gone = pid.map(|p| !proc::pid_alive(p)).unwrap_or(false);
        Ok((
            state.state == State::Cancelled && gone,
            format!("state {}, agent process gone: {gone}", state.state.as_str()),
        ))
    });
    out.push(match check {
        Ok((ok, detail)) => SelfTest { name: "cancel a hanging agent and leave no process behind", ok, detail },
        Err(e) => SelfTest { name: "cancel a hanging agent and leave no process behind", ok: false, detail: e.message },
    });

    if out.iter().all(|t| t.ok) {
        let _ = std::fs::remove_dir_all(&root);
    } else {
        out.push(SelfTest {
            name: "keep the evidence",
            ok: true,
            detail: format!("the throwaway home was left at {} for inspection", root.display()),
        });
    }
    out
}
