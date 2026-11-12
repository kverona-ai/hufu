//! Shared scaffolding: a throwaway Hufu home and workspace, and the binary under test.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

pub const HUFU: &str = env!("CARGO_BIN_EXE_hufu");

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// One isolated Hufu installation. Everything lives under `root` and is removed on drop.
pub struct Env {
    pub root: PathBuf,
    pub home: PathBuf,
    pub work: PathBuf,
    pub vars: Vec<(String, String)>,
}

pub struct Ran {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl Ran {
    pub fn json(&self) -> Value {
        serde_json::from_str(self.stdout.trim()).unwrap_or_else(|e| panic!("not JSON ({e}):\nstdout: {}\nstderr: {}", self.stdout, self.stderr))
    }

    #[track_caller]
    pub fn expect(self, code: i32) -> Ran {
        assert_eq!(self.code, code, "unexpected exit code\nstdout: {}\nstderr: {}", self.stdout, self.stderr);
        self
    }
}

fn ran(out: Output) -> Ran {
    Ran { code: out.status.code().unwrap_or(-1), stdout: String::from_utf8_lossy(&out.stdout).into_owned(), stderr: String::from_utf8_lossy(&out.stderr).into_owned() }
}

/// The argv that starts a fake agent of the given flavor.
pub fn fake(flavor: &str) -> Value {
    json!([HUFU, "__fake-agent", flavor])
}

impl Env {
    pub fn new(name: &str) -> Env {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!("hufu-e2e-{}-{n}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let home = root.join("home");
        let work = root.join("work");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&work).unwrap();
        // Symlinked temp directories (macOS) would make path comparisons fail.
        let root = root.canonicalize().unwrap_or(root);
        let root = PathBuf::from(root.to_string_lossy().trim_start_matches(r"\\?\").to_string());
        let env = Env { home: root.join("home"), work: root.join("work"), root, vars: Vec::new() };
        env.config(json!({}));
        env
    }

    /// Writes the configuration: one profile per fake flavor, plus whatever `extra` adds on top.
    pub fn config(&self, extra: Value) {
        let mut doc = json!({
            "version": 1,
            "profiles": {
                "codex": {"adapter": "codex-cli", "command": fake("codex")},
                "agy": {"adapter": "agy-cli", "command": fake("agy")},
                "cursor": {"adapter": "cursor-cli", "command": fake("cursor")},
                "claude": {"adapter": "claude-cli", "command": fake("claude")},
                "kimi": {"adapter": "kimi-cli", "command": fake("kimi"), "allow_full_access": true},
                "pi": {"adapter": "pi", "command": fake("pi"), "allow_full_access": true},
                "acp": {"adapter": "acp", "command": fake("acp"), "permission_requests": "verified"},
                "ext": {"adapter": "external", "command": fake("external")},
            },
            "roles": {"advisor": "claude", "executors": ["codex"]},
        });
        merge(&mut doc, extra);
        std::fs::write(self.home.join("config.yaml"), serde_json::to_string_pretty(&doc).unwrap()).unwrap();
    }

    pub fn set(&mut self, key: &str, value: &str) {
        self.vars.push((key.to_string(), value.to_string()));
    }

    pub fn command(&self, cwd: &Path) -> Command {
        let mut cmd = Command::new(HUFU);
        cmd.current_dir(cwd)
            .env("HUFU_HOME", &self.home)
            .env("HUFU_KILL_GRACE_MS", "300")
            .env("HUFU_STALE_AFTER_MS", "1500")
            .env_remove("HUFU_FAKE_SCENARIO")
            .env_remove("HUFU_EXE");
        for (k, v) in &self.vars {
            cmd.env(k, v);
        }
        cmd
    }

    pub fn hufu_in(&self, cwd: &Path, args: &[&str]) -> Ran {
        let out = self.command(cwd).args(args).stdin(Stdio::null()).output().expect("hufu starts");
        ran(out)
    }

    pub fn hufu(&self, args: &[&str]) -> Ran {
        self.hufu_in(&self.work, args)
    }

    /// Runs with `--json` and returns the exit code and the document.
    pub fn json(&self, args: &[&str]) -> (i32, Value) {
        let mut all = vec!["--json"];
        all.extend_from_slice(args);
        let r = self.hufu(&all);
        (r.code, r.json())
    }

    pub fn with_stdin(&self, args: &[&str], input: &str) -> Ran {
        let mut child = self.command(&self.work).args(args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().expect("hufu starts");
        child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
        ran(child.wait_with_output().unwrap())
    }

    /// Starts a job without waiting and returns its id.
    pub fn start(&self, args: &[&str]) -> String {
        let mut all = vec!["--json", "run"];
        all.extend_from_slice(args);
        let r = self.hufu(&all).expect(0);
        r.json()["job"].as_str().expect("a job id").to_string()
    }

    /// Runs a job to its end and returns the exit code and the result document.
    pub fn run(&self, args: &[&str]) -> (i32, Value) {
        let mut all = vec!["run", "--wait"];
        all.extend_from_slice(args);
        self.json(&all)
    }

    pub fn state(&self, job: &str) -> Value {
        self.json(&["status", job]).1
    }

    /// Polls until the job's state document satisfies `done`.
    pub fn until(&self, job: &str, what: &str, done: impl Fn(&Value) -> bool) -> Value {
        let started = Instant::now();
        loop {
            let state = self.state(job);
            if done(&state) {
                return state;
            }
            assert!(started.elapsed() < Duration::from_secs(60), "{job} never reached: {what}; last state: {state}");
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    pub fn job_file(&self, job: &str, name: &str) -> PathBuf {
        self.home.join("jobs").join(job).join(name)
    }

    /// Makes the workspace a git repository with one commit.
    pub fn git_init(&self) {
        std::fs::write(self.work.join("README.md"), "hello\n").unwrap();
        for args in [
            vec!["init", "-q", "-b", "main"],
            vec!["add", "-A"],
            vec!["-c", "user.name=t", "-c", "user.email=t@example.invalid", "-c", "commit.gpgsign=false", "commit", "-q", "-m", "init"],
        ] {
            let out = Command::new("git").args(&args).current_dir(&self.work).output().expect("git is installed");
            assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        }
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        // Leave the files behind when a test failed: they explain what happened.
        if !std::thread::panicking() {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
}

/// Deep-merges `extra` into `base`. `null` removes a key.
pub fn merge(base: &mut Value, extra: Value) {
    match (base, extra) {
        (Value::Object(b), Value::Object(e)) => {
            for (k, v) in e {
                if v.is_null() {
                    b.remove(&k);
                } else if b.get(&k).map(Value::is_object).unwrap_or(false) && v.is_object() {
                    merge(b.get_mut(&k).unwrap(), v);
                } else {
                    b.insert(k, v);
                }
            }
        }
        (b, e) => *b = e,
    }
}

/// Ends a process by pid, the hard way.
pub fn kill(pid: u64) {
    #[cfg(unix)]
    let _ = Command::new("kill").args(["-9", &pid.to_string()]).output();
    #[cfg(windows)]
    let _ = Command::new("taskkill").args(["/F", "/T", "/PID", &pid.to_string()]).output();
}

pub fn strings(v: &Value) -> Vec<String> {
    v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default()
}
