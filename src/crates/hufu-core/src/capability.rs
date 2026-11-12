//! Capabilities beyond code tools. The first one is computer use: which agent operates the
//! desktop for a task, and whether the scope the task asks for is acceptable.

use std::time::Duration;

use hufu_contract::{AdapterRegistry, Profile, Provided, proc};

use crate::config::{Config, Grant, Route, WhenUnavailable};
use crate::error::{HufuError, Result};
use crate::spec::{ComputerUseOverride, ComputerUsePlan};

pub const COMPUTER_USE: &str = "computer_use";

/// Applications a grant may never name: handing an agent a terminal or its own host removes
/// every other boundary Hufu enforces.
const REFUSED: &[&str] = &[
    "terminal",
    "iterm",
    "iterm2",
    "warp",
    "alacritty",
    "kitty",
    "wezterm",
    "ghostty",
    "hyper",
    "tabby",
    "windows terminal",
    "powershell",
    "command prompt",
    "cmd",
    "cmd.exe",
    "konsole",
    "gnome terminal",
    "gnome-terminal",
    "xterm",
    "tilix",
    "terminator",
    "chatgpt",
    "codex",
    "claude",
    "cursor",
    "antigravity",
    "kimi",
    "kimi code",
];

/// Applications that are allowed but deserve a warning.
const SENSITIVE: &[&str] = &[
    "safari",
    "chrome",
    "google chrome",
    "firefox",
    "edge",
    "microsoft edge",
    "arc",
    "brave",
    "brave browser",
    "opera",
    "1password",
    "bitwarden",
    "keychain access",
    "lastpass",
    "keepassxc",
    "dashlane",
    "passwords",
];

fn norm(app: &str) -> String {
    app.trim().trim_end_matches(".app").trim_end_matches(".exe").to_lowercase()
}

/// The result of routing: the plan, if the task uses computer use, and things worth saying.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Resolved {
    pub plan: Option<ComputerUsePlan>,
    pub warnings: Vec<String>,
}

/// Whether a grant list in a mission plan stays inside what the configuration allows.
pub fn within_allowed(grants: &[Grant], allowed: &[Grant]) -> std::result::Result<(), String> {
    for grant in grants {
        let Some(limit) = allowed.iter().find(|a| a.capability == grant.capability) else {
            return Err(format!("capability `{}` is not in `missions.allowed_grants`", grant.capability));
        };
        for app in &grant.apps {
            if !limit.apps.iter().any(|a| norm(a) == norm(app)) {
                return Err(format!("app `{app}` is not in `missions.allowed_grants` for `{}`", grant.capability));
            }
        }
    }
    Ok(())
}

/// Checks the scope of a computer-use grant. Returns warnings.
fn check_scope(grant: &Grant) -> Result<Vec<String>> {
    if grant.apps.is_empty() {
        return Err(HufuError::usage(
            "a `computer_use` grant must list the applications it covers, for example `apps: [\"Figma\"]`; there is no whole-desktop grant",
        ));
    }
    let mut warnings = Vec::new();
    for app in &grant.apps {
        let n = norm(app);
        if n.is_empty() || n.contains('*') || n.contains('?') {
            return Err(HufuError::usage(format!(
                "`{app}` is not an application name; a `computer_use` grant names each application, without wildcards"
            )));
        }
        if REFUSED.contains(&n.as_str()) {
            return Err(HufuError::usage(format!(
                "a `computer_use` grant cannot cover `{app}`: terminals and agent host applications would let the agent step outside every other limit"
            )));
        }
        if SENSITIVE.contains(&n.as_str()) {
            warnings.push(format!("computer use covers `{app}`, which can reach accounts and saved credentials; grant it only for tasks that need it"));
        }
    }
    Ok(warnings)
}

fn provided(profile: &Profile) -> Option<&Provided> {
    profile.provides.get(COMPUTER_USE)
}

/// Asks the runtime which applications it already allows. `None` when that cannot be found out.
fn allowed_apps(provided: &Provided) -> std::result::Result<Option<Vec<String>>, String> {
    let argv: Vec<String> = if !provided.check.is_empty() {
        provided.check.clone()
    } else if provided.via == "lcu" {
        vec!["lcu".into(), "apps".into()]
    } else {
        return Ok(None);
    };
    let Some(program) =
        proc::which(&argv[0]).or_else(|| std::path::Path::new(&argv[0]).is_file().then(|| argv[0].clone().into()))
    else {
        return Err(format!("`{}` is not installed or not on PATH", argv[0]));
    };
    let mut cmd = proc::command_for(&program);
    cmd.args(&argv[1..]);
    match proc::run_capture(cmd, Duration::from_secs(20)) {
        Ok(out) if out.ok() => Ok(Some(
            out.stdout
                .lines()
                .map(|l| l.trim().trim_start_matches(['-', '*', '•']).trim())
                .filter(|l| !l.is_empty())
                .map(norm)
                .collect(),
        )),
        Ok(out) => Err(format!(
            "`{}` failed: {}",
            argv.join(" "),
            proc::tail(if out.stderr.trim().is_empty() { &out.stdout } else { &out.stderr }, 300)
        )),
        Err(e) => Err(format!("`{}` could not be run: {e}", argv.join(" "))),
    }
}

/// Checks that the runtime behind a profile can serve the apps of a grant right now.
fn preflight(profile: &Profile, apps: &[String]) -> std::result::Result<Vec<String>, String> {
    let Some(p) = provided(profile) else {
        return Err(format!("profile `{}` does not declare `provides.computer_use`", profile.name));
    };
    let mut warnings = Vec::new();
    if p.via == "lcu" && cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        return Err("the LCU runtime does not support Intel Macs".into());
    }
    match allowed_apps(p)? {
        Some(allowed) => {
            let missing: Vec<&String> = apps.iter().filter(|a| !allowed.iter().any(|x| x == &norm(a) || x.contains(&norm(a)))).collect();
            if !missing.is_empty() {
                let how = if p.via == "lcu" {
                    missing.iter().map(|a| format!("`lcu apps allow \"{a}\"`")).collect::<Vec<_>>().join(", ")
                } else {
                    "allow them in the runtime's own settings".to_string()
                };
                return Err(format!(
                    "the computer-use runtime of profile `{}` has not allowed: {}. A person must allow them first ({how}); Hufu never does this itself, and an unattended job cannot answer the prompt",
                    profile.name,
                    missing.iter().map(|a| a.as_str()).collect::<Vec<_>>().join(", ")
                ));
            }
        }
        None => warnings.push(format!(
            "computer use through profile `{}` ({}) cannot be checked ahead of time; if the agent has to ask for an application, the job will report it as a denied action",
            profile.name, p.via
        )),
    }
    Ok(warnings)
}

/// Decides how computer use is served for a task, or rejects the task.
pub fn resolve(
    config: &Config,
    adapters: &dyn AdapterRegistry,
    executor: &Profile,
    grants: &[Grant],
    over: Option<&ComputerUseOverride>,
) -> Result<Resolved> {
    for grant in grants {
        if grant.capability != COMPUTER_USE {
            return Err(HufuError::usage(format!(
                "unknown capability `{}` in `grants`; accepted: computer_use",
                grant.capability
            )));
        }
    }
    let mut apps: Vec<String> = Vec::new();
    let mut warnings = Vec::new();
    for grant in grants {
        warnings.extend(check_scope(grant)?);
        for app in &grant.apps {
            if !apps.iter().any(|a| norm(a) == norm(app)) {
                apps.push(app.trim().to_string());
            }
        }
    }
    if grants.is_empty() {
        if over.is_some() {
            return Err(HufuError::usage(
                "`computer_use` routing was given, but the task has no `computer_use` grant; add `grants: [{capability: computer_use, apps: [...]}]`",
            ));
        }
        return Ok(Resolved::default());
    }

    let cu = &config.capabilities.computer_use;
    let route = over.and_then(|o| o.route).unwrap_or(cu.route);
    let dedicated_name = over.and_then(|o| o.agent.clone()).or_else(|| cu.agent.clone());

    let executor_plan = |warnings: &mut Vec<String>| -> std::result::Result<ComputerUsePlan, String> {
        let Some(p) = provided(executor) else {
            return Err(format!(
                "profile `{}` does not support computer use. Either declare `provides: {{computer_use: {{via: native|lcu}}}}` on it, or name a profile that does in `capabilities.computer_use.agent` and use `route: dedicated`",
                executor.name
            ));
        };
        warnings.extend(preflight(executor, &apps)?);
        Ok(ComputerUsePlan {
            route: Route::Executor,
            agent: executor.name.clone(),
            apps: apps.clone(),
            via: p.via.clone(),
        })
    };

    let plan = match route {
        Route::Executor => executor_plan(&mut warnings).map_err(HufuError::usage)?,
        Route::Dedicated => {
            let dedicated = (|| -> std::result::Result<ComputerUsePlan, String> {
                let Some(name) = &dedicated_name else {
                    return Err("no computer-use agent is configured; set `capabilities.computer_use.agent` to a profile that declares `provides.computer_use` (`hufu config set capabilities.computer_use.agent <profile>`)".into());
                };
                let Some(profile) = config.profile(name) else {
                    return Err(format!("computer-use agent `{name}` is not a profile"));
                };
                let Some(p) = provided(profile) else {
                    return Err(format!(
                        "profile `{name}` does not declare `provides.computer_use`, so it cannot serve computer use"
                    ));
                };
                if profile.name == executor.name {
                    // The executor is the configured operator: no hand-over is needed.
                    warnings.extend(preflight(executor, &apps)?);
                    return Ok(ComputerUsePlan {
                        route: Route::Executor,
                        agent: executor.name.clone(),
                        apps: apps.clone(),
                        via: p.via.clone(),
                    });
                }
                let accepts = adapters
                    .get(&executor.adapter)
                    .map(|a| a.capabilities(executor).accepts_mcp_servers)
                    .unwrap_or(false);
                if !accepts {
                    return Err(format!(
                        "adapter `{}` cannot attach Hufu's hand-over tool to the executor's session, so profile `{}` cannot delegate computer use",
                        executor.adapter, executor.name
                    ));
                }
                warnings.extend(preflight(profile, &apps)?);
                Ok(ComputerUsePlan {
                    route: Route::Dedicated,
                    agent: name.clone(),
                    apps: apps.clone(),
                    via: p.via.clone(),
                })
            })();
            match dedicated {
                Ok(plan) => plan,
                Err(why) if cu.when_unavailable == WhenUnavailable::Executor && provided(executor).is_some() => {
                    let plan = executor_plan(&mut warnings).map_err(|e| {
                        HufuError::usage(format!("{why}; the fallback to the executor also failed: {e}"))
                    })?;
                    warnings.push(format!("computer use falls back to the executor's own agent: {why}"));
                    plan
                }
                Err(why) => return Err(HufuError::usage(why)),
            }
        }
    };
    warnings.push(format!(
        "computer use acts outside the workspace: permission tiers and worktree isolation do not contain it. Scope: {}",
        plan.apps.join(", ")
    ));
    Ok(Resolved { plan: Some(plan), warnings })
}

/// The text added to an executor's prompt so that it knows how computer use is served.
pub fn preamble(plan: &ComputerUsePlan) -> String {
    let apps = plan.apps.join(", ");
    match plan.route {
        Route::Dedicated => format!(
            "[Hufu] This task may operate desktop applications, limited to: {apps}.\n\
             You do not operate the desktop yourself. Call the MCP tool `computer_use` from the `hufu` server with a\n\
             self-contained `instruction` for each desktop step; another agent carries it out and returns what it saw.\n\
             Do not touch any application outside that list.\n\n"
        ),
        Route::Executor => format!(
            "[Hufu] This task may operate desktop applications with your own computer-use tools, limited to: {apps}.\n\
             Do not touch any application outside that list, and never operate a terminal or an agent's own window.\n\n"
        ),
    }
}

/// The prompt given to a dedicated operator for one hand-over step.
pub fn operator_prompt(apps: &[String], instruction: &str) -> String {
    format!(
        "[Hufu] You operate the desktop on behalf of another agent. Applications you may use: {}.\n\
         Do only what the instruction says, touch no other application, and finish by reporting what you did\n\
         and what you observed, in plain text.\n\nInstruction:\n{instruction}\n",
        apps.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{check_raw, parse_yaml};
    use hufu_contract::{
        Adapter, AdapterError, CancelToken, Capabilities, EventSink, LaunchPlan, ProbeResult, RunOutcome, RunRequest,
        StructuredOutput, Tier,
    };

    struct Fake(&'static str, bool);
    impl Adapter for Fake {
        fn kind(&self) -> &'static str {
            self.0
        }
        fn default_command(&self) -> Vec<String> {
            Vec::new()
        }
        fn probe(&self, _: &Profile) -> ProbeResult {
            ProbeResult::default()
        }
        fn capabilities(&self, _: &Profile) -> Capabilities {
            Capabilities {
                permission_tiers: Tier::ALL.to_vec(),
                efforts: Vec::new(),
                native_effort: false,
                checked_at_runtime: false,
                models: Vec::new(),
                resume: false,
                structured_output: StructuredOutput::Instructed,
                reports_usage: false,
                reports_denials: false,
                accepts_mcp_servers: self.1,
                provides: Vec::new(),
                notes: Vec::new(),
            }
        }
        fn plan(&self, _: &Profile, _: &RunRequest) -> std::result::Result<LaunchPlan, AdapterError> {
            unimplemented!()
        }
        fn run(&self, _: &Profile, _: &RunRequest, _: &dyn EventSink, _: &CancelToken) -> RunOutcome {
            unimplemented!()
        }
    }
    struct Reg;
    static FAKES: [Fake; 8] = [
        Fake("codex-cli", true),
        Fake("kimi-cli", false),
        Fake("acp", true),
        Fake("agy-cli", false),
        Fake("cursor-cli", false),
        Fake("claude-cli", true),
        Fake("pi", false),
        Fake("external", false),
    ];
    impl AdapterRegistry for Reg {
        fn get(&self, kind: &str) -> Option<&dyn Adapter> {
            FAKES.iter().find(|f| f.0 == kind).map(|f| f as &dyn Adapter)
        }
        fn kinds(&self) -> Vec<&'static str> {
            FAKES.iter().map(|f| f.0).collect()
        }
    }

    fn config(extra: &str) -> Config {
        let text = format!(
            "profiles:\n  plain: {{adapter: codex-cli}}\n  mute: {{adapter: kimi-cli}}\n  hands: {{adapter: codex-cli, provides: {{computer_use: {{via: native}}}}}}\n  operator: {{adapter: acp, command: [x, acp], provides: {{computer_use: {{via: native}}}}}}\n{extra}"
        );
        check_raw(&parse_yaml(&text).unwrap(), &Reg).unwrap()
    }

    fn grant(apps: &[&str]) -> Vec<Grant> {
        vec![Grant { capability: COMPUTER_USE.into(), apps: apps.iter().map(|s| s.to_string()).collect() }]
    }

    #[test]
    fn no_grant_means_no_plan() {
        let c = config("");
        let r = resolve(&c, &Reg, &c.profiles["plain"], &[], None).unwrap();
        assert!(r.plan.is_none() && r.warnings.is_empty());
    }

    #[test]
    fn scope_rules_refuse_terminals_and_wildcards() {
        let c = config("capabilities: {computer_use: {route: executor}}\n");
        let hands = &c.profiles["hands"];
        for bad in [&["Terminal"][..], &["Codex.app"], &["*"], &[]] {
            assert!(resolve(&c, &Reg, hands, &grant(bad), None).is_err(), "{bad:?} must be refused");
        }
        let ok = resolve(&c, &Reg, hands, &grant(&["Figma", "Safari"]), None).unwrap();
        assert!(ok.warnings.iter().any(|w| w.contains("Safari")));
        let unknown = vec![Grant { capability: "network".into(), apps: vec![] }];
        assert!(resolve(&c, &Reg, hands, &unknown, None).unwrap_err().message.contains("unknown capability"));
    }

    #[test]
    fn executor_route_needs_an_executor_that_supports_it() {
        let c = config("capabilities: {computer_use: {route: executor}}\n");
        let plan = resolve(&c, &Reg, &c.profiles["hands"], &grant(&["Figma"]), None).unwrap().plan.unwrap();
        assert_eq!((plan.route, plan.agent.as_str(), plan.via.as_str()), (Route::Executor, "hands", "native"));
        let err = resolve(&c, &Reg, &c.profiles["plain"], &grant(&["Figma"]), None).unwrap_err();
        assert!(err.message.contains("does not support computer use"), "{}", err.message);
    }

    #[test]
    fn dedicated_route_hands_over_to_the_configured_profile() {
        let c = config("capabilities: {computer_use: {route: dedicated, agent: operator}}\n");
        let plan = resolve(&c, &Reg, &c.profiles["plain"], &grant(&["Figma"]), None).unwrap().plan.unwrap();
        assert_eq!((plan.route, plan.agent.as_str()), (Route::Dedicated, "operator"));
        // An executor can be named as the operator; when it runs the task itself no hand-over happens.
        let c2 = config("capabilities: {computer_use: {route: dedicated, agent: hands}}\n");
        let own = resolve(&c2, &Reg, &c2.profiles["hands"], &grant(&["Figma"]), None).unwrap().plan.unwrap();
        assert_eq!((own.route, own.agent.as_str()), (Route::Executor, "hands"));
        let other = resolve(&c2, &Reg, &c2.profiles["plain"], &grant(&["Figma"]), None).unwrap().plan.unwrap();
        assert_eq!((other.route, other.agent.as_str()), (Route::Dedicated, "hands"));
        // An executor whose adapter cannot attach the hand-over tool is rejected.
        let err = resolve(&c, &Reg, &c.profiles["mute"], &grant(&["Figma"]), None).unwrap_err();
        assert!(err.message.contains("cannot attach"), "{}", err.message);
        // Nothing configured at all.
        let none = config("");
        assert!(
            resolve(&none, &Reg, &none.profiles["plain"], &grant(&["Figma"]), None)
                .unwrap_err()
                .message
                .contains("no computer-use agent is configured")
        );
    }

    #[test]
    fn task_override_and_fallback() {
        let c = config("capabilities: {computer_use: {route: dedicated, when_unavailable: executor}}\n");
        let fell_back = resolve(&c, &Reg, &c.profiles["hands"], &grant(&["Figma"]), None).unwrap();
        assert_eq!(fell_back.plan.unwrap().route, Route::Executor);
        assert!(fell_back.warnings.iter().any(|w| w.contains("falls back")));
        let over = ComputerUseOverride { route: Some(Route::Dedicated), agent: Some("operator".into()) };
        let plan = resolve(&c, &Reg, &c.profiles["plain"], &grant(&["Figma"]), Some(&over)).unwrap().plan.unwrap();
        assert_eq!(plan.agent, "operator");
        assert!(resolve(&c, &Reg, &c.profiles["plain"], &[], Some(&over)).is_err());
    }

    #[test]
    fn preflight_reports_apps_the_runtime_has_not_allowed() {
        let missing = Provided { via: "lcu".into(), check: vec!["hufu-no-such-runtime-binary".into()] };
        assert!(allowed_apps(&missing).unwrap_err().contains("not installed"));
        assert_eq!(allowed_apps(&Provided { via: "native".into(), check: vec![] }).unwrap(), None);
        assert!(within_allowed(&grant(&["Figma"]), &grant(&["figma", "Notes"])).is_ok());
        assert!(within_allowed(&grant(&["Mail"]), &grant(&["Figma"])).is_err());
        assert!(within_allowed(&grant(&["Figma"]), &[]).is_err());
    }
}
