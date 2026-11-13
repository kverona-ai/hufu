//! Plain-text rendering for people. Machines use `--json`.

use hufu_contract::time::fmt_duration;
use hufu_contract::{Effort, effort_list, tier_list};
use hufu_core::doctor::{DoctorReport, SelfTest};
use hufu_core::error::exit;
use hufu_core::mission::Mission;
use hufu_core::ops::AgentView;
use hufu_core::store::{JobResult, JobState, State, Verdict};

/// Prints rows as aligned columns.
pub fn table(headers: &[&str], rows: &[Vec<String>]) {
    let mut widths: Vec<usize> = headers.iter().map(|h| h.chars().count()).collect();
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            if i < widths.len() {
                widths[i] = widths[i].max(cell.chars().count());
            }
        }
    }
    let line = |cells: Vec<String>| {
        let mut out = String::new();
        for (i, cell) in cells.iter().enumerate() {
            if i + 1 == cells.len() {
                out.push_str(cell);
            } else {
                out.push_str(&format!("{cell:<width$}  ", width = widths[i]));
            }
        }
        println!("{}", out.trim_end());
    };
    line(headers.iter().map(|h| h.to_string()).collect());
    for row in rows {
        line(row.clone());
    }
}

pub fn clip(text: &str, max: usize) -> String {
    let one: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() > max {
        format!("{}…", one.chars().take(max.saturating_sub(1)).collect::<String>())
    } else {
        one
    }
}

/// The process exit code that reports how a job ended.
pub fn exit_code(state: State, verdict: Verdict, reason: Option<&str>) -> i32 {
    match state {
        State::Succeeded if verdict == Verdict::Rejected => exit::JOB_REJECTED,
        State::Succeeded => exit::OK,
        State::Failed if matches!(reason, Some("auth_required" | "agent_unavailable")) => exit::AGENT_UNAVAILABLE,
        State::Failed => exit::JOB_FAILED,
        State::Cancelled => exit::JOB_CANCELLED,
        State::TimedOut => exit::JOB_TIMED_OUT,
        State::Lost => exit::JOB_LOST,
        State::Queued | State::Starting | State::Running => exit::WAIT_TIMED_OUT,
    }
}

pub fn job_row(j: &JobState) -> Vec<String> {
    let state = match (&j.queue, j.state) {
        (Some(q), State::Queued) => format!("queued ({q})"),
        _ => j.state.as_str().to_string(),
    };
    let what = match (&j.last_activity, j.state.is_terminal(), &j.reason) {
        (_, true, Some(reason)) => format!("{} [{reason}]", j.title),
        (Some(activity), false, _) => format!("{} · {}", clip(&j.title, 40), activity),
        _ => j.title.clone(),
    };
    let agent = if j.agent == j.profile { j.agent.clone() } else { format!("{}→{}", j.agent, j.profile) };
    vec![
        j.id.clone(),
        state,
        if j.verdict == Verdict::Unchecked { "-".into() } else { j.verdict.as_str().into() },
        agent,
        fmt_duration(j.elapsed_s()),
        clip(&what, 90),
    ]
}

pub fn jobs(list: &[JobState]) {
    if list.is_empty() {
        println!("no jobs");
        return;
    }
    let rows: Vec<Vec<String>> = list.iter().map(job_row).collect();
    table(&["JOB", "STATE", "VERDICT", "AGENT", "TIME", "TITLE"], &rows);
}

pub fn job(j: &JobState) {
    println!("{}  {}  {}", j.id, j.state.as_str(), j.title);
    println!(
        "  agent       {} (profile {}, adapter {}{})",
        j.agent,
        j.profile,
        j.adapter,
        j.agent_version.as_ref().map(|v| format!(", {v}")).unwrap_or_default()
    );
    println!("  permission  {}", j.permission);
    println!("  workspace   {}", j.workspace.as_ref().map(|w| w.path.clone()).unwrap_or_else(|| j.cwd.clone()));
    if let Some(branch) = j.workspace.as_ref().and_then(|w| w.branch.as_ref()) {
        println!("  branch      {branch}");
    }
    println!("  elapsed     {}", fmt_duration(j.elapsed_s()));
    if j.verdict != Verdict::Unchecked {
        println!("  verdict     {}", j.verdict.as_str());
    }
    if let Some(reason) = &j.reason {
        println!("  reason      {reason}");
    }
    if let Some(detail) = &j.detail {
        println!("  detail      {detail}");
    }
    if let Some(activity) = &j.last_activity {
        println!("  last        {activity}");
    }
    if let Some(mission) = &j.mission {
        println!("  mission     {mission}");
    }
}

pub fn result(r: &JobResult, max_chars: Option<usize>) {
    let verdict =
        if r.verdict == Verdict::Unchecked { String::new() } else { format!("  verdict: {}", r.verdict.as_str()) };
    println!("{}  {}{}  {}", r.job, r.state.as_str(), verdict, r.title);
    let mut facts = vec![format!(
        "agent {} ({}{})",
        r.agent.profile,
        r.agent.adapter,
        r.agent.version.as_ref().map(|v| format!(", {v}")).unwrap_or_default()
    )];
    if let Some(account) = &r.agent.account {
        facts.push(format!("account {account}"));
    }
    if let Some(model) = &r.agent.model {
        facts.push(format!("model {model}"));
    }
    if r.agent.effort != Effort::Default {
        facts.push(format!("effort {}", r.agent.effort));
    }
    if let Some(tier) = r.agent.permission {
        facts.push(format!("permission {tier}"));
    }
    facts.push(fmt_duration(r.timing.duration_s));
    if let Some(u) = &r.usage {
        facts.push(format!("{} tokens", u.tokens()));
        if let Some(cost) = u.cost_usd {
            facts.push(format!("${cost:.4}"));
        }
    }
    if r.attempts > 1 {
        facts.push(format!("{} attempts", r.attempts));
    }
    println!("  {}", facts.join(" · "));
    if let Some(reason) = &r.reason {
        println!("  reason: {reason}{}", r.detail.as_ref().map(|d| format!("\n  {d}")).unwrap_or_default());
    }
    for warning in &r.warnings {
        println!("  warning: {warning}");
    }
    if let Some(message) = &r.final_message {
        println!("\n--- final message ---");
        match max_chars {
            Some(max) if message.chars().count() > max => {
                println!("{}", message.chars().take(max).collect::<String>());
                println!("… ({} more characters; use --full)", message.chars().count() - max);
            }
            _ => println!("{}", message.trim_end()),
        }
    }
    if !r.changes.files.is_empty() {
        println!("\n--- changed files ({}) ---", r.changes.files.len());
        for f in &r.changes.files {
            println!("{f}");
        }
        if let (Some(branch), Some(worktree)) = (&r.changes.branch, &r.changes.worktree) {
            println!("on branch {branch}, in {worktree}");
        }
    }
    if !r.denied_actions.is_empty() {
        println!("\n--- skipped for lack of permission ---");
        for d in &r.denied_actions {
            println!("{} {} {}", d.kind, d.tool, d.detail);
        }
    }
    if !r.tally.is_empty() {
        println!("\n--- tally ---");
        for t in &r.tally {
            println!(
                "{} {} {}{}",
                if t.passed { "pass" } else { "FAIL" },
                t.check,
                t.description,
                if t.detail.is_empty() { String::new() } else { format!("\n     {}", t.detail) }
            );
        }
    }
    if let Some(session) = r.session.as_ref().filter(|s| s.resumable) {
        println!("\nfollow up with: hufu resume {} \"...\"   (session {})", r.job, session.id);
    }
}

pub fn agents(list: &[AgentView], probed: bool) {
    let mut rows = Vec::new();
    for a in list {
        let caps = a.capabilities.as_ref();
        let mut row = vec![
            a.name.clone(),
            match a.kind {
                "pool" => format!("pool of {}", a.members.join(", ")),
                _ => a.adapter.clone().unwrap_or_default(),
            },
            a.account.clone().or(a.model.clone()).unwrap_or_else(|| "-".into()),
            caps.map(|c| tier_list(&c.permission_tiers)).unwrap_or_else(|| "-".into()),
            caps.map(|c| if c.checked_at_runtime { "per session".to_string() } else { effort_list(&c.efforts) })
                .unwrap_or_else(|| "-".into()),
            if a.roles.is_empty() {
                if a.builtin { "(preset)".to_string() } else { "-".into() }
            } else {
                a.roles.join(",")
            },
        ];
        if probed {
            row.push(match &a.probe {
                Some(p) if !p.installed => "not installed".into(),
                Some(p) => match p.authenticated {
                    Some(true) => "ready".into(),
                    Some(false) => "not logged in".into(),
                    None => "installed".into(),
                },
                None => "-".into(),
            });
        }
        rows.push(row);
    }
    let mut headers = vec!["NAME", "ADAPTER", "ACCOUNT/MODEL", "TIERS", "EFFORT", "ROLES"];
    if probed {
        headers.push("STATUS");
    }
    table(&headers, &rows);
}

pub fn doctor(r: &DoctorReport) {
    println!("hufu {}  ({})", r.version, r.exe);
    println!("home     {}", r.home);
    println!(
        "config   {}",
        if r.config_files.is_empty() {
            "none yet (built-in presets only)".to_string()
        } else {
            r.config_files.join(", ")
        }
    );
    println!(
        "daemon   {}",
        r.daemon
            .as_ref()
            .map(|d| format!("running (pid {})", d.pid))
            .unwrap_or_else(|| "not running (optional)".into())
    );
    println!("jobs     {} running, {} queued", r.running_jobs, r.queued_jobs);
    println!("advisor  {}", r.advisor.clone().unwrap_or_else(|| "not set".into()));
    println!("executors  {}", if r.executors.is_empty() { "not set".to_string() } else { r.executors.join(", ") });
    println!("computer use  {}", r.computer_use.detail);
    println!("MCP hosts found  {}", if r.hosts.is_empty() { "none".to_string() } else { r.hosts.join(", ") });
    println!();
    let rows: Vec<Vec<String>> = r
        .profiles
        .iter()
        .map(|p| {
            vec![
                p.name.clone(),
                p.adapter.clone(),
                p.account.clone().unwrap_or_else(|| "-".into()),
                if p.installed {
                    p.version.clone().unwrap_or_else(|| "installed".into())
                } else {
                    "not installed".into()
                },
                match (p.installed, p.authenticated) {
                    (false, _) => "-".into(),
                    (_, Some(true)) => "yes".into(),
                    (_, Some(false)) => "NO".into(),
                    (_, None) => "unknown".into(),
                },
                clip(p.detail.as_deref().unwrap_or(""), 70),
            ]
        })
        .collect();
    table(&["PROFILE", "ADAPTER", "ACCOUNT", "VERSION", "LOGIN", "NOTE"], &rows);
    if !r.problems.is_empty() {
        println!("\nconfiguration problems:");
        for p in &r.problems {
            println!("  - {p}");
        }
    }
    if !r.hints.is_empty() {
        println!("\nnext steps:");
        for h in &r.hints {
            println!("  - {h}");
        }
    }
}

pub fn self_test(tests: &[SelfTest]) {
    println!("\nself-test (built-in fake agents, no network):");
    for t in tests {
        println!("  {} {} — {}", if t.ok { "ok  " } else { "FAIL" }, t.name, t.detail);
    }
}

pub fn mission(m: &Mission) {
    println!("{}  {}  {}", m.id, m.state.as_str(), m.title);
    println!("  advisor    {}", m.advisor);
    println!("  executors  {}", m.executors.join(", "));
    println!("  workspace  {}", m.workdir);
    if let Some(branch) = &m.branch {
        println!("  branch     {branch}");
    }
    if let Some(reason) = &m.reason {
        println!("  reason     {reason}");
    }
    if let Some(detail) = &m.detail {
        println!("  detail     {detail}");
    }
    if let Some(summary) = &m.summary {
        println!("  plan       {summary}");
    }
    if !m.tasks.is_empty() {
        println!();
        let rows: Vec<Vec<String>> = m
            .tasks
            .iter()
            .map(|t| {
                vec![
                    t.id.clone(),
                    t.state.as_str().into(),
                    t.agent.clone(),
                    t.rounds.to_string(),
                    t.jobs.last().cloned().unwrap_or_else(|| "-".into()),
                    clip(&t.title, 60),
                ]
            })
            .collect();
        table(&["TASK", "STATE", "AGENT", "ROUNDS", "LAST JOB", "TITLE"], &rows);
    }
}
