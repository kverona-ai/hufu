//! One-command setup: detect installed agents, write a first configuration, and register Hufu's
//! MCP server with the agents that should be able to call it.

use std::path::{Path, PathBuf};
use std::time::Duration;

use hufu_contract::{AdapterRegistry, proc};
use serde::Serialize;
use serde_json::{Value, json};

use crate::config::{self, Home};
use crate::error::{HufuError, IoContext, Result};
use crate::store;

/// Agents Hufu can register its MCP server with.
pub const HOSTS: &[&str] = &["claude", "codex", "cursor", "kimi", "agy"];

#[derive(Clone, Debug, Serialize)]
pub struct SetupAction {
    pub host: String,
    /// `registered`, `unchanged`, `removed`, `absent`, `skipped`, `failed` or `manual`.
    pub action: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub detail: String,
}

#[derive(Clone, Debug)]
pub struct SetupOptions {
    pub remove: bool,
    /// Print what would be written instead of writing it.
    pub print: bool,
    /// Register for one project (where the host supports it) instead of user-wide.
    pub project: Option<PathBuf>,
    /// The `hufu` executable hosts should start.
    pub exe: PathBuf,
}

fn env_dir(var: &str, default: &[&str]) -> Option<PathBuf> {
    if let Some(v) = std::env::var_os(var).filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(v));
    }
    let mut dir = proc::home_dir()?;
    for part in default {
        dir.push(part);
    }
    Some(dir)
}

fn host_dir(host: &str) -> Option<PathBuf> {
    match host {
        "claude" => env_dir("CLAUDE_CONFIG_DIR", &[".claude"]),
        "codex" => env_dir("CODEX_HOME", &[".codex"]),
        "cursor" => env_dir("CURSOR_CONFIG_DIR", &[".cursor"]),
        "kimi" => env_dir("KIMI_CODE_HOME", &[".kimi-code"]),
        "agy" => env_dir("HUFU_AGY_CONFIG_DIR", &[".gemini", "config"]),
        _ => None,
    }
}

fn host_binary(host: &str) -> &'static str {
    match host {
        "claude" => "claude",
        "codex" => "codex",
        "cursor" => "agent",
        "kimi" => "kimi",
        _ => "agy",
    }
}

/// Hosts that look installed: their program is on PATH or their configuration directory exists.
pub fn detect_hosts() -> Vec<String> {
    HOSTS
        .iter()
        .filter(|h| proc::which(host_binary(h)).is_some() || host_dir(h).map(|d| d.is_dir()).unwrap_or(false))
        .map(|h| h.to_string())
        .collect()
}

fn backup(path: &Path) {
    let mut name = path.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(".hufu-bak");
    let target = path.with_file_name(name);
    if path.exists() && !target.exists() {
        let _ = std::fs::copy(path, target);
    }
}

fn server_entry(exe: &Path) -> Value {
    json!({"command": exe.display().to_string(), "args": ["mcp"]})
}

/// The snippet for a host Hufu does not know: the common `mcpServers` shape.
pub fn generic_snippet(exe: &Path) -> String {
    serde_json::to_string_pretty(&json!({"mcpServers": {"hufu": server_entry(exe)}})).unwrap_or_default()
}

/// Adds or removes `mcpServers.hufu` in a JSON file, keeping everything else.
fn json_file(host: &str, path: &Path, opts: &SetupOptions) -> SetupAction {
    let shown = Some(path.display().to_string());
    let mut doc: Value = match std::fs::read_to_string(path) {
        Ok(text) if text.trim().is_empty() => json!({}),
        Ok(text) => match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(e) => {
                return SetupAction {
                    host: host.into(),
                    action: "skipped",
                    path: shown,
                    detail: format!(
                        "the file is not plain JSON ({e}); add this by hand: {}",
                        json!({"hufu": server_entry(&opts.exe)})
                    ),
                };
            }
        },
        Err(_) => json!({}),
    };
    if !doc.is_object() {
        return SetupAction {
            host: host.into(),
            action: "skipped",
            path: shown,
            detail: "the file does not hold a JSON object".into(),
        };
    }
    let entry = server_entry(&opts.exe);
    let current = doc.pointer("/mcpServers/hufu").cloned();
    if opts.remove {
        if current.is_none() {
            return SetupAction {
                host: host.into(),
                action: "absent",
                path: shown,
                detail: "hufu was not registered".into(),
            };
        }
        if let Some(servers) = doc.get_mut("mcpServers").and_then(Value::as_object_mut) {
            servers.remove("hufu");
        }
    } else {
        if current.as_ref() == Some(&entry) {
            return SetupAction {
                host: host.into(),
                action: "unchanged",
                path: shown,
                detail: "already registered".into(),
            };
        }
        let obj = doc.as_object_mut().expect("checked above");
        let servers = obj.entry("mcpServers").or_insert(json!({}));
        if !servers.is_object() {
            return SetupAction {
                host: host.into(),
                action: "skipped",
                path: shown,
                detail: "`mcpServers` is not an object".into(),
            };
        }
        servers.as_object_mut().expect("checked above").insert("hufu".into(), entry);
    }
    let text = serde_json::to_string_pretty(&doc).unwrap_or_default() + "\n";
    if opts.print {
        return SetupAction { host: host.into(), action: "manual", path: shown, detail: text };
    }
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    backup(path);
    match store::write_atomic(path, text.as_bytes()) {
        Ok(()) => SetupAction {
            host: host.into(),
            action: if opts.remove { "removed" } else { "registered" },
            path: shown,
            detail: "restart the agent to pick it up".into(),
        },
        Err(e) => SetupAction { host: host.into(), action: "failed", path: shown, detail: e.message },
    }
}

const BEGIN: &str = "# >>> hufu (managed by `hufu setup`; edit outside this block)";
const END: &str = "# <<< hufu";

fn toml_string(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Adds or removes a managed block in Codex's `config.toml`.
fn codex_toml(path: &Path, opts: &SetupOptions) -> SetupAction {
    let host = "codex".to_string();
    let shown = Some(path.display().to_string());
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let block = format!(
        "{BEGIN}\n[mcp_servers.hufu]\ncommand = {}\nargs = [\"mcp\"]\n{END}\n",
        toml_string(&opts.exe.display().to_string())
    );
    let managed = text.find(BEGIN).and_then(|a| text[a..].find(END).map(|b| (a, a + b + END.len())));
    let outside = match managed {
        Some((a, b)) => format!("{}{}", &text[..a], text[b..].trim_start_matches('\n')),
        None => text.clone(),
    };
    if outside.lines().any(|l| l.trim().starts_with("[mcp_servers.hufu]")) {
        return SetupAction {
            host,
            action: "skipped",
            path: shown,
            detail: "`[mcp_servers.hufu]` is already defined by hand; leaving it alone".into(),
        };
    }
    let next = if opts.remove {
        if managed.is_none() {
            return SetupAction { host, action: "absent", path: shown, detail: "hufu was not registered".into() };
        }
        outside
    } else {
        if managed.map(|(a, b)| text[a..b].trim() == block.trim()).unwrap_or(false) {
            return SetupAction { host, action: "unchanged", path: shown, detail: "already registered".into() };
        }
        let mut base = outside.trim_end().to_string();
        if !base.is_empty() {
            base.push_str("\n\n");
        }
        base + &block
    };
    if opts.print {
        return SetupAction { host, action: "manual", path: shown, detail: block };
    }
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    backup(path);
    match store::write_atomic(path, next.as_bytes()) {
        Ok(()) => SetupAction {
            host,
            action: if opts.remove { "removed" } else { "registered" },
            path: shown,
            detail: "new Codex sessions will see the `hufu` tools".into(),
        },
        Err(e) => SetupAction { host, action: "failed", path: shown, detail: e.message },
    }
}

/// The skill that teaches Claude Code when and how to delegate through Hufu.
pub const CLAUDE_SKILL: &str = include_str!("../../../integrations/claude-code/SKILL.md");

fn claude(opts: &SetupOptions) -> Vec<SetupAction> {
    let host = "claude".to_string();
    let mut out = Vec::new();
    // Project scope is a plain file Claude Code reads from the repository root.
    if let Some(project) = &opts.project {
        out.push(json_file("claude", &project.join(".mcp.json"), opts));
    } else if opts.print {
        out.push(SetupAction {
            host: host.clone(),
            action: "manual",
            path: None,
            detail: format!("claude mcp add --scope user hufu -- {} mcp", opts.exe.display()),
        });
    } else {
        // User scope lives in a file Claude Code owns; its own command is the supported way in.
        match proc::which("claude") {
            None => out.push(SetupAction {
                host: host.clone(),
                action: "manual",
                path: None,
                detail: format!(
                    "`claude` is not on PATH; once it is, run: claude mcp add --scope user hufu -- {} mcp",
                    opts.exe.display()
                ),
            }),
            Some(program) => {
                let run = |args: &[&str]| {
                    let mut cmd = proc::command_for(&program);
                    cmd.args(args);
                    proc::run_capture(cmd, Duration::from_secs(30))
                };
                let exe = opts.exe.display().to_string();
                // Re-registering replaces an entry that points at an older location of the binary.
                let _ = run(&["mcp", "remove", "--scope", "user", "hufu"]);
                if opts.remove {
                    out.push(SetupAction {
                        host: host.clone(),
                        action: "removed",
                        path: None,
                        detail: "ran `claude mcp remove --scope user hufu`".into(),
                    });
                } else {
                    match run(&["mcp", "add", "--scope", "user", "hufu", "--", &exe, "mcp"]) {
                        Ok(done) if done.ok() => out.push(SetupAction {
                            host: host.clone(),
                            action: "registered",
                            path: None,
                            detail: "ran `claude mcp add --scope user hufu`; new sessions will see the `hufu` tools"
                                .into(),
                        }),
                        Ok(done) => out.push(SetupAction {
                            host: host.clone(),
                            action: "failed",
                            path: None,
                            detail: format!(
                                "`claude mcp add` failed: {}",
                                proc::tail(
                                    if done.stderr.trim().is_empty() { &done.stdout } else { &done.stderr },
                                    300
                                )
                            ),
                        }),
                        Err(e) => out.push(SetupAction {
                            host: host.clone(),
                            action: "failed",
                            path: None,
                            detail: format!("`claude mcp add` could not be run: {e}"),
                        }),
                    }
                }
            }
        }
    }
    // The skill.
    let dir = match &opts.project {
        Some(project) => Some(project.join(".claude")),
        None => host_dir("claude"),
    };
    if let Some(dir) = dir {
        let path = dir.join("skills").join("hufu").join("SKILL.md");
        let shown = Some(path.display().to_string());
        if opts.print {
            out.push(SetupAction {
                host,
                action: "manual",
                path: shown,
                detail: "the Hufu skill would be written here".into(),
            });
        } else if opts.remove {
            let existed = path.exists();
            let _ = std::fs::remove_file(&path);
            let _ = std::fs::remove_dir(path.parent().unwrap_or(&dir));
            out.push(SetupAction {
                host,
                action: if existed { "removed" } else { "absent" },
                path: shown,
                detail: "the Hufu skill".into(),
            });
        } else if std::fs::read_to_string(&path).map(|t| t == CLAUDE_SKILL).unwrap_or(false) {
            out.push(SetupAction {
                host,
                action: "unchanged",
                path: shown,
                detail: "the Hufu skill is up to date".into(),
            });
        } else {
            let written = path.parent().map(std::fs::create_dir_all).transpose().is_ok()
                && store::write_atomic(&path, CLAUDE_SKILL.as_bytes()).is_ok();
            out.push(SetupAction {
                host,
                action: if written { "registered" } else { "failed" },
                path: shown,
                detail: "the Hufu skill, which tells Claude Code when to delegate".into(),
            });
        }
    }
    out
}

/// Registers (or removes) Hufu's MCP server with the named hosts.
pub fn setup(hosts: &[String], opts: &SetupOptions) -> Vec<SetupAction> {
    let mut out = Vec::new();
    for host in hosts {
        match host.as_str() {
            "claude" => out.extend(claude(opts)),
            "codex" => match host_dir("codex") {
                Some(dir) => out.push(codex_toml(&dir.join("config.toml"), opts)),
                None => out.push(SetupAction {
                    host: host.clone(),
                    action: "failed",
                    path: None,
                    detail: "cannot find the home directory".into(),
                }),
            },
            "cursor" | "kimi" | "agy" => {
                let file = if host == "agy" { "mcp_config.json" } else { "mcp.json" };
                let path = match (&opts.project, host.as_str()) {
                    (Some(project), "cursor") => Some(project.join(".cursor").join(file)),
                    _ => host_dir(host).map(|d| d.join(file)),
                };
                match path {
                    Some(path) => out.push(json_file(host, &path, opts)),
                    None => out.push(SetupAction {
                        host: host.clone(),
                        action: "failed",
                        path: None,
                        detail: "cannot find the home directory".into(),
                    }),
                }
            }
            "generic" => out.push(SetupAction {
                host: host.clone(),
                action: "manual",
                path: None,
                detail: generic_snippet(&opts.exe),
            }),
            other => out.push(SetupAction {
                host: other.to_string(),
                action: "failed",
                path: None,
                detail: format!("unknown host; accepted: {}, generic", HOSTS.join(", ")),
            }),
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------
// First configuration
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize)]
pub struct InitReport {
    pub path: String,
    /// `created`, `kept` (a configuration already exists), `replaced`, or `skipped` (no agent was
    /// found, so no file was written).
    pub action: &'static str,
    /// Profiles whose agent was found on this machine.
    pub detected: Vec<String>,
    pub advisor: Option<String>,
    pub executors: Vec<String>,
    /// What the user should do next, when the result needs their attention.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub hints: Vec<String>,
}

/// The preset profiles `hufu init` looks for, in the order it prefers them.
pub const PRESET_ORDER: &[&str] = &["codex", "claude", "agy", "cursor", "kimi", "pi"];

/// The agents Hufu can dispatch to, each with the program it puts on `PATH`.
pub const SUPPORTED_AGENTS: &str =
    "Codex (`codex`), Claude Code (`claude`), Antigravity (`agy`), Cursor (`agent`), Kimi Code (`kimi`) or pi (`pi`)";

/// What to run once an agent has been installed. `rewrite` is set when a configuration file
/// exists and names no agent: `hufu init` keeps an existing file, so only `--force` refreshes it.
pub fn after_install(rewrite: bool) -> &'static str {
    if rewrite {
        "run `hufu init --force` to rewrite the configuration from the agents found, then `hufu up`"
    } else {
        "run `hufu up` again"
    }
}

/// The hint for a machine with no agent Hufu could dispatch to.
pub fn no_agent_hint(rewrite: bool) -> String {
    format!(
        "no supported agent is installed, so Hufu has nothing to dispatch work to. Install and log in to at least one of {SUPPORTED_AGENTS}, then {}",
        after_install(rewrite)
    )
}

/// The hint for a configuration in which one profile is the advisor and the only executor.
/// `others` are installed agents the configuration does not use yet.
pub fn single_agent_hint(name: &str, others: &[String]) -> String {
    let add = match others.first() {
        Some(other) => format!(
            "Also installed: {}. Add one as an executor with `hufu config set roles.executors \"[{name}, {other}]\"`",
            others.join(", ")
        ),
        None => format!(
            "Install and log in to a second agent, or add a second account of this one with `hufu profile add`, then name it as an executor with `hufu config set roles.executors \"[{name}, <other>]\"`"
        ),
    };
    format!(
        "only one agent is in use: `{name}` is both the advisor and the only executor, so it plans, does and reviews the work by itself. Hufu works this way, but it is built to hand work to other agents. {add}"
    )
}

/// Preset profiles whose agent is installed, in a stable order.
pub fn detect_agents(adapters: &dyn AdapterRegistry) -> Vec<String> {
    let presets = config::presets();
    PRESET_ORDER
        .iter()
        .filter(|name| {
            presets.get(**name).and_then(|p| adapters.get(&p.adapter).map(|a| a.probe(p).installed)).unwrap_or(false)
        })
        .map(|s| s.to_string())
        .collect()
}

/// Writes a first `config.yaml` that names the agents found on this machine.
///
/// With no agent found there is nothing to name, so no file is written (`skipped`): the next
/// `hufu init` or `hufu up`, after an agent is installed, then writes a complete one. `force`
/// writes the file regardless.
pub fn init(home: &Home, adapters: &dyn AdapterRegistry, force: bool) -> Result<InitReport> {
    let path = home.config_file();
    let detected = detect_agents(adapters);
    let advisor = detected.first().cloned();
    let executors = detected.clone();
    if path.exists() && !force {
        return Ok(InitReport {
            path: path.display().to_string(),
            action: "kept",
            detected,
            advisor: None,
            executors: Vec::new(),
            hints: Vec::new(),
        });
    }
    if detected.is_empty() && !force {
        return Ok(InitReport {
            path: path.display().to_string(),
            action: "skipped",
            detected,
            advisor: None,
            executors: Vec::new(),
            hints: vec![no_agent_hint(false)],
        });
    }
    let existed = path.exists();
    home.ensure()?;
    let presets = config::presets();
    let mut text = String::from(
        "# Hufu configuration, written by `hufu init`.\n\
         # Edit by hand, with `hufu config set` / `hufu profile add`, or through the MCP tools.\n\
         # This file holds names of environment variables, never their values.\n\n\
         version: 1\n\n\
         defaults:\n  permission: read-only      # read-only | workspace-write (full-access is asked for per task)\n  effort: default\n  timeout: 30m\n  inactivity_timeout: 10m\n\n\
         limits:\n  max_concurrent_jobs: 4\n\n",
    );
    text.push_str("# Who plans and who does the work when a command does not say.\nroles:\n");
    match &advisor {
        Some(a) => text.push_str(&format!("  advisor: {a}\n")),
        None => text.push_str("  # advisor: <profile>\n"),
    }
    text.push_str(&format!("  executors: [{}]\n\n", executors.join(", ")));
    text.push_str("# One entry per agent configuration. Several entries may drive the same agent with\n# different accounts: give each its own `home` (the agent's login directory) and `account` label.\nprofiles:\n");
    if detected.is_empty() {
        text.push_str("  {}\n  # No supported agent was found on PATH. Install one, then run `hufu init --force`.\n");
    }
    for name in &detected {
        if let Some(p) = presets.get(name) {
            text.push_str(&format!("  {name}:\n    adapter: {}\n", p.adapter));
            if let Some(d) = &p.description {
                text.push_str(&format!("    description: {d}\n"));
            }
        }
    }
    text.push_str(
        "\n# Examples (uncomment and adjust):\n\
         #   codex-work:\n#     adapter: codex-cli\n#     account: work\n#     home: ~/.codex-work\n\
         #   or-reasoner:\n#     adapter: pi\n#     provider: openrouter\n#     model: <vendor/model>\n#     env: [OPENROUTER_API_KEY]\n\n\
         # Profiles that can stand in for each other, for example two accounts of one agent.\n\
         # pools:\n#   codex-any:\n#     members: [codex, codex-work]\n#     strategy: failover        # failover | round-robin | least-busy\n\n\
         # Which agent operates desktop applications when a task is granted computer use.\n\
         # capabilities:\n#   computer_use:\n#     route: dedicated          # dedicated | executor\n#     agent: <a profile that declares `provides: {computer_use: {via: native|lcu}}`>\n",
    );
    let raw = config::parse_yaml(&text).map_err(HufuError::internal)?;
    config::check_raw(&raw, adapters)
        .map_err(|p| HufuError::internal(format!("the generated configuration is invalid: {}", p.join("; "))))?;
    if existed {
        backup(&path);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).at(parent)?;
    }
    store::write_atomic(&path, text.as_bytes())?;
    let hints = match detected.as_slice() {
        [] => vec![no_agent_hint(true)],
        [only] => vec![single_agent_hint(only, &[])],
        _ => Vec::new(),
    };
    Ok(InitReport {
        path: path.display().to_string(),
        action: if existed { "replaced" } else { "created" },
        detected,
        advisor,
        executors,
        hints,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(remove: bool) -> SetupOptions {
        SetupOptions { remove, print: false, project: None, exe: PathBuf::from("/opt/hufu/bin/hufu") }
    }

    #[test]
    fn json_hosts_keep_other_servers_and_are_idempotent() {
        let dir = std::env::temp_dir().join(format!("hufu-setup-{}", hufu_contract::time::new_id("t")));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("mcp.json");
        std::fs::write(&path, r#"{"mcpServers": {"other": {"command": "x"}}, "keep": 1}"#).unwrap();
        assert_eq!(json_file("cursor", &path, &opts(false)).action, "registered");
        assert_eq!(json_file("cursor", &path, &opts(false)).action, "unchanged");
        let doc: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(doc["mcpServers"]["hufu"]["args"], json!(["mcp"]));
        assert_eq!(doc["mcpServers"]["other"]["command"], "x");
        assert_eq!(doc["keep"], 1);
        assert!(dir.join("mcp.json.hufu-bak").exists(), "the original is backed up once");
        assert_eq!(json_file("cursor", &path, &opts(true)).action, "removed");
        assert_eq!(json_file("cursor", &path, &opts(true)).action, "absent");
        std::fs::write(&path, "{ // comments\n}").unwrap();
        assert_eq!(
            json_file("cursor", &path, &opts(false)).action,
            "skipped",
            "a file Hufu cannot parse is never overwritten"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn codex_block_is_managed_and_hand_written_entries_are_respected() {
        let dir = std::env::temp_dir().join(format!("hufu-setup-{}", hufu_contract::time::new_id("t")));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, "model = \"x\"\n").unwrap();
        assert_eq!(codex_toml(&path, &opts(false)).action, "registered");
        assert_eq!(codex_toml(&path, &opts(false)).action, "unchanged");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.starts_with("model = \"x\"\n")
                && text.contains("[mcp_servers.hufu]\ncommand = \"/opt/hufu/bin/hufu\"\nargs = [\"mcp\"]")
        );
        assert_eq!(codex_toml(&path, &opts(true)).action, "removed");
        assert_eq!(std::fs::read_to_string(&path).unwrap().trim(), "model = \"x\"");
        std::fs::write(&path, "[mcp_servers.hufu]\ncommand = \"mine\"\n").unwrap();
        assert_eq!(codex_toml(&path, &opts(false)).action, "skipped");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn temp_home() -> Home {
        Home::at(std::env::temp_dir().join(format!("hufu-init-{}", hufu_contract::time::new_id("t"))))
    }

    #[test]
    fn init_writes_a_valid_configuration_once() {
        let home = temp_home();
        let reg = &crate::config::tests_support::Found(&["codex-cli", "claude-cli"]);
        let first = init(&home, reg, false).unwrap();
        assert_eq!(first.action, "created");
        assert_eq!(first.detected, vec!["codex", "claude"]);
        assert_eq!(first.advisor.as_deref(), Some("codex"));
        assert_eq!(first.executors, vec!["codex", "claude"]);
        assert!(first.hints.is_empty(), "two agents need no advice: {:?}", first.hints);
        assert_eq!(init(&home, reg, false).unwrap().action, "kept");
        assert_eq!(init(&home, reg, true).unwrap().action, "replaced");
        let loaded = config::load(&home, &home.root).unwrap();
        assert!(loaded.config.problems(reg).is_empty());
        assert_eq!(loaded.config.roles.executors, vec!["codex", "claude"]);
        let _ = std::fs::remove_dir_all(&home.root);
    }

    #[test]
    fn init_without_any_agent_writes_nothing_and_says_what_to_do() {
        let home = temp_home();
        let none = crate::config::tests_support::kinds();
        let first = init(&home, none, false).unwrap();
        assert_eq!(first.action, "skipped");
        assert!(!home.config_file().exists(), "a file naming no agent would later be kept as it is");
        let hint = first.hints.join("\n");
        assert!(hint.contains("`codex`") && hint.contains("`pi`") && hint.contains("`hufu up` again"), "{hint}");
        assert!(!hint.contains("--force"), "nothing was written, so a plain rerun is enough: {hint}");

        // Once an agent is installed the same command completes, with no flag.
        let one = &crate::config::tests_support::Found(&["claude-cli"]);
        assert_eq!(init(&home, one, false).unwrap().action, "created");
        let _ = std::fs::remove_dir_all(&home.root);

        // `--force` is the explicit way to get the starter file anyway; it names the right command.
        let home = temp_home();
        let forced = init(&home, none, true).unwrap();
        assert_eq!(forced.action, "created");
        assert!(forced.hints.join("\n").contains("`hufu init --force`"), "{:?}", forced.hints);
        let loaded = config::load(&home, &home.root).unwrap();
        assert!(loaded.config.problems(none).is_empty());
        let _ = std::fs::remove_dir_all(&home.root);
    }

    #[test]
    fn init_with_one_agent_says_it_holds_both_roles() {
        let home = temp_home();
        let one = &crate::config::tests_support::Found(&["pi"]);
        let report = init(&home, one, false).unwrap();
        assert_eq!(report.action, "created");
        assert_eq!(report.advisor.as_deref(), Some("pi"));
        assert_eq!(report.executors, vec!["pi"]);
        let hint = report.hints.join("\n");
        assert!(
            hint.contains("only one agent") && hint.contains("`pi` is both the advisor and the only executor"),
            "{hint}"
        );
        assert!(hint.contains("roles.executors"), "the hint names the command that adds a second one: {hint}");
        let _ = std::fs::remove_dir_all(&home.root);
    }
}
