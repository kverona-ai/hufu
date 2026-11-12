//! The adapter conformance suite: the checks C1 to C12 of `doc/technical/adapter-contract.md`,
//! run against an adapter that is pointed at the matching fake agent.

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use hufu_contract::{Adapter, CancelReason, CancelToken, Effort, EventBody, EventSink, Profile, RawStream, RunOutcome, RunRequest, RunStatus, Tier, proc, reason};

/// A sink that keeps everything in memory.
#[derive(Default)]
pub struct Memory {
    pub events: Mutex<Vec<EventBody>>,
    pub stdout: Mutex<Vec<String>>,
    pub stderr: Mutex<Vec<String>>,
    pub pid: Mutex<Option<u32>>,
}

impl EventSink for Memory {
    fn emit(&self, event: EventBody) {
        self.events.lock().unwrap_or_else(|e| e.into_inner()).push(event);
    }
    fn raw(&self, stream: RawStream, line: &str) {
        let target = match stream {
            RawStream::Stdout => &self.stdout,
            RawStream::Stderr => &self.stderr,
        };
        target.lock().unwrap_or_else(|e| e.into_inner()).push(line.to_string());
    }
    fn child_started(&self, pid: u32) {
        *self.pid.lock().unwrap_or_else(|e| e.into_inner()) = Some(pid);
    }
}

/// An adapter under test, with the fake agent it should drive.
pub struct Subject<'a> {
    pub adapter: &'a dyn Adapter,
    /// The argv that starts the fake, for example `[hufu, __fake-agent, codex]`.
    pub fake: Vec<String>,
    pub workspace: PathBuf,
    pub scratch: PathBuf,
    /// Extra profile settings a flavor needs (ACP: `permission_requests: verified`).
    pub tune: fn(&mut Profile),
}

impl Subject<'_> {
    pub fn profile(&self) -> Profile {
        let mut p = Profile { name: "subject".into(), adapter: self.adapter.kind().to_string(), command: self.fake.clone(), ..Profile::default() };
        (self.tune)(&mut p);
        p
    }

    pub fn request(&self, scenario: &str, tier: Tier) -> RunRequest {
        let mut env = proc::agent_env(&[]);
        env.insert("HUFU_FAKE_SCENARIO".into(), scenario.to_string());
        RunRequest {
            job_id: "j_conformance".into(),
            prompt: format!("conformance check [[fake:{scenario}]]"),
            cwd: self.workspace.clone(),
            permission: tier,
            effort: Effort::Default,
            model: None,
            output_schema: None,
            resume_session: None,
            env,
            mcp_servers: Vec::new(),
            scratch_dir: self.scratch.clone(),
        }
    }

    fn run(&self, req: &RunRequest) -> (RunOutcome, Memory) {
        let sink = Memory::default();
        let out = self.adapter.run(&self.profile(), req, &sink, &CancelToken::new());
        (out, sink)
    }
}

fn expect(ok: bool, message: impl FnOnce() -> String) -> Result<(), String> {
    if ok { Ok(()) } else { Err(message()) }
}

/// Runs every check. Each entry is the check's id and what went wrong, if anything.
pub fn run_all(s: &Subject) -> Vec<(&'static str, Result<(), String>)> {
    let profile = s.profile();
    let caps = s.adapter.capabilities(&profile);
    let tier = caps.permission_tiers.iter().copied().max().unwrap_or(Tier::FullAccess);
    let mut out: Vec<(&'static str, Result<(), String>)> = Vec::new();

    // C1: every supported tier has its own explicit invocation.
    out.push(("C1", {
        let plans: Vec<_> = caps.permission_tiers.iter().map(|t| (*t, s.adapter.plan(&profile, &s.request("ok", *t)))).collect();
        let all_ok = plans.iter().all(|(_, p)| p.is_ok());
        let args: Vec<Vec<String>> = plans.iter().filter_map(|(_, p)| p.as_ref().ok().map(|p| p.args.clone())).collect();
        let distinct = caps.checked_at_runtime || args.iter().enumerate().all(|(i, a)| args.iter().skip(i + 1).all(|b| a != b));
        expect(!caps.permission_tiers.is_empty() && all_ok && distinct, || format!("plans for {:?} are missing or identical: {plans:?}", caps.permission_tiers))
    }));

    // C2: an unsupported tier is never launched.
    out.push(("C2", {
        let mut result = Ok(());
        for t in Tier::ALL.into_iter().filter(|t| !caps.supports_tier(*t)) {
            let (o, sink) = s.run(&s.request("write=c2-must-not-exist.txt", t));
            let launched = sink.pid.lock().map(|p| p.is_some()).unwrap_or(false);
            if o.status == RunStatus::Succeeded || launched || s.workspace.join("c2-must-not-exist.txt").exists() {
                result = Err(format!("tier {t} is not supported but the agent was started"));
            }
        }
        result
    }));

    // C3: an effort level without a mapping is never launched.
    out.push(("C3", {
        let mut result = Ok(());
        if !caps.checked_at_runtime {
            for level in Effort::SCALE.into_iter().filter(|e| !caps.efforts.contains(e)) {
                let mut req = s.request("ok", tier);
                req.effort = level.clone();
                if s.adapter.plan(&profile, &req).is_ok() {
                    result = Err(format!("effort {level} has no mapping but a launch plan was produced"));
                }
            }
        }
        result
    }));

    // C4: exit 0 without a final message is a failure.
    out.push(("C4", {
        let (o, _) = s.run(&s.request("empty", tier));
        expect(o.status == RunStatus::Failed && o.reason.as_deref() == Some(reason::EMPTY_OUTPUT), || format!("expected failed/empty_output, got {:?}/{:?}", o.status, o.reason))
    }));

    // C5: a skipped action is reported.
    out.push(("C5", {
        if caps.reports_denials {
            let (o, sink) = s.run(&s.request("deny", tier.min(Tier::WorkspaceWrite).max(caps.permission_tiers.iter().copied().min().unwrap_or(tier))));
            let event = sink.events.lock().map(|e| e.iter().any(|e| matches!(e, EventBody::PermissionDenied { .. }))).unwrap_or(false);
            expect(!o.denied_actions.is_empty() && event, || format!("expected a denied action and a permission.denied event, got {:?}", o.denied_actions))
        } else {
            Ok(())
        }
    }));

    // C6 and C7: cancelling stops a hanging agent within the grace period and leaves no process.
    let hang = {
        let sink = Memory::default();
        let token = CancelToken::new();
        let started = Instant::now();
        let (o, pid) = std::thread::scope(|scope| {
            scope.spawn(|| {
                std::thread::sleep(Duration::from_millis(500));
                token.cancel(CancelReason::User);
            });
            let o = s.adapter.run(&profile, &s.request("hang", tier), &sink, &token);
            (o, *sink.pid.lock().unwrap_or_else(|e| e.into_inner()))
        });
        (o, pid, started.elapsed())
    };
    out.push(("C6", expect(hang.0.status == RunStatus::Cancelled && hang.2 < proc::kill_grace() + Duration::from_secs(8), || format!("expected cancelled, got {:?} after {:?}", hang.0.status, hang.2))));
    out.push(("C7", {
        std::thread::sleep(Duration::from_millis(200));
        expect(hang.1.map(|p| !proc::pid_alive(p)).unwrap_or(false), || format!("the agent process {:?} survived the cancellation", hang.1))
    }));

    // C8: a missing login is reported as such, with a hint, and does not hang.
    out.push(("C8", {
        let (o, _) = s.run(&s.request("auth", tier));
        expect(o.status == RunStatus::Failed && o.reason.as_deref() == Some(reason::AUTH_REQUIRED) && o.detail.as_deref().map(|d| !d.is_empty()).unwrap_or(false), || {
            format!("expected failed/auth_required with a hint, got {:?}/{:?}: {:?}", o.status, o.reason, o.detail)
        })
    }));

    // C9: malformed output does not crash the adapter.
    out.push(("C9", {
        let (o, _) = s.run(&s.request("malformed", tier));
        expect(o.status == RunStatus::Failed, || format!("expected a failure for malformed output, got {:?}", o.status))
    }));

    // C10: raw output is preserved.
    out.push(("C10", {
        let (o, sink) = s.run(&s.request("reply=raw-marker-7f3a", tier));
        let raw = sink.stdout.lock().map(|l| l.join("\n")).unwrap_or_default();
        expect(o.status == RunStatus::Succeeded && raw.contains("raw-marker-7f3a"), || format!("the raw log does not contain the agent's output: {:?} {raw}", o.reason))
    }));

    // C11: a follow-up reaches the agent's own resume mechanism, or the capability is absent.
    out.push(("C11", {
        let mut req = s.request("ok", tier);
        req.resume_session = Some("sess-42".into());
        if caps.resume {
            let (o, _) = s.run(&req);
            expect(o.final_message.as_deref().map(|m| m.contains("resumed sess-42")).unwrap_or(false), || format!("the session id did not reach the agent: {:?}", o.final_message))
        } else {
            expect(s.adapter.plan(&profile, &req).is_err(), || "resume is not supported, but a launch plan with a session was produced".into())
        }
    }));

    // C12: nothing outside the request's environment reaches the agent.
    out.push(("C12", {
        let req = s.request("ok", tier);
        let stray = std::env::vars().map(|(k, _)| k).find(|k| !req.env.keys().any(|e| e.eq_ignore_ascii_case(k)) && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'));
        match stray {
            None => Ok(()),
            Some(var) => {
                let mut req = s.request(&format!("env={var}"), tier);
                req.env.remove("HUFU_FAKE_SCENARIO");
                let (o, _) = s.run(&req);
                expect(o.final_message.as_deref() == Some(format!("{var}=<unset>").as_str()), || format!("`{var}` leaked into the agent: {:?} ({:?})", o.final_message, o.detail))
            }
        }
    }));
    out
}
