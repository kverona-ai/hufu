//! Cross-platform process helpers: finding programs, starting them in their own group, streaming
//! their output, and stopping a whole process tree.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crate::types::{CancelToken, EventSink, RawStream};

// ---------------------------------------------------------------------------------------------
// Finding programs
// ---------------------------------------------------------------------------------------------

/// Resolves a program name the way a shell would, including `PATHEXT` on Windows.
pub fn which(name: &str) -> Option<PathBuf> {
    which_in(name, std::env::var_os("PATH").as_deref())
}

/// Like [`which`], with an explicit search path. Used by tests and by adapters given a job's `PATH`.
pub fn which_in(name: &str, path_var: Option<&std::ffi::OsStr>) -> Option<PathBuf> {
    if name.is_empty() {
        return None;
    }
    let direct = Path::new(name);
    if direct.components().count() > 1 || direct.is_absolute() {
        return candidates(direct).into_iter().find(|p| is_executable(p));
    }
    let path_var = path_var?;
    for dir in std::env::split_paths(path_var) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        if let Some(found) = candidates(&dir.join(name)).into_iter().find(|p| is_executable(p)) {
            return Some(found);
        }
    }
    None
}

#[cfg(windows)]
fn candidates(base: &Path) -> Vec<PathBuf> {
    // A file with no extension is never runnable on Windows (npm also installs extension-less
    // shell shims next to the `.cmd` ones), so only names with an executable extension count.
    let exts = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_string());
    let exts: Vec<String> = exts.split(';').filter(|e| !e.is_empty()).map(|e| e.to_ascii_lowercase()).collect();
    let has_known_ext = base
        .extension()
        .map(|e| {
            let e = format!(".{}", e.to_string_lossy().to_ascii_lowercase());
            exts.contains(&e) || e == ".ps1"
        })
        .unwrap_or(false);
    if has_known_ext {
        return vec![base.to_path_buf()];
    }
    let mut out = Vec::new();
    for ext in exts.iter().map(String::as_str).chain([".ps1"]) {
        let mut s = base.as_os_str().to_os_string();
        s.push(ext);
        out.push(PathBuf::from(s));
    }
    out
}

#[cfg(not(windows))]
fn candidates(base: &Path) -> Vec<PathBuf> {
    vec![base.to_path_buf()]
}

#[cfg(unix)]
fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0).unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(p: &Path) -> bool {
    p.is_file()
}

/// Builds a `Command` for a resolved program. PowerShell scripts are run through PowerShell;
/// `.cmd` and `.bat` shims are handled by the standard library.
pub fn command_for(program: &Path) -> Command {
    #[cfg(windows)]
    {
        let is_ps1 = program.extension().map(|e| e.to_string_lossy().eq_ignore_ascii_case("ps1")).unwrap_or(false);
        if is_ps1 {
            let mut c = Command::new("powershell.exe");
            c.args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File"]).arg(program);
            return c;
        }
    }
    Command::new(program)
}

/// A command that runs one line through the platform shell (`sh -c` or `cmd /C`).
pub fn shell_command(line: &str) -> Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let mut c = Command::new("cmd.exe");
        c.arg("/D").arg("/S").arg("/C").raw_arg(format!("\"{line}\""));
        c
    }
    #[cfg(not(windows))]
    {
        let mut c = Command::new("sh");
        c.arg("-c").arg(line);
        c
    }
}

// ---------------------------------------------------------------------------------------------
// Process groups, detaching, liveness and termination
// ---------------------------------------------------------------------------------------------

#[cfg(windows)]
const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Starts the child in its own process group, so that the whole tree can be stopped later.
pub fn new_group(cmd: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
    }
}

/// Starts the child fully detached from the caller: its own session on Unix, its own hidden
/// console on Windows. Used for job runners, which must outlive the command that started them.
pub fn detach(cmd: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: `setsid` is async-signal-safe and takes no arguments. It runs in the child
        // between fork and exec, where only such calls are allowed.
        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        keep_std_handles_private();
        cmd.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
    }
}

/// Windows hands every inheritable handle to a new process, not only the ones named as its
/// standard streams. A detached runner that inherited the caller's stdout pipe would keep that
/// pipe open, and whoever reads `hufu run`'s output would wait for the whole job. The standard
/// library duplicates what a child is meant to get, so the originals need not be inheritable.
#[cfg(windows)]
fn keep_std_handles_private() {
    use windows_sys::Win32::Foundation::{HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE, SetHandleInformation};
    use windows_sys::Win32::System::Console::{GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE};
    for id in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
        // SAFETY: the handle comes from the system and is checked before use; clearing a flag
        // on it does not close or change what it refers to.
        unsafe {
            let handle = GetStdHandle(id);
            if !handle.is_null() && handle != INVALID_HANDLE_VALUE {
                SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0);
            }
        }
    }
}

/// Whether a process with this id exists.
pub fn pid_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    #[cfg(unix)]
    {
        // SAFETY: signal 0 performs the permission and existence check only.
        let r = unsafe { libc::kill(pid as libc::pid_t, 0) };
        if r == 0 {
            return !is_zombie(pid);
        }
        std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{
            GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        };
        const STILL_ACTIVE: u32 = 259;
        // SAFETY: the handle is checked before use and closed exactly once.
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if handle.is_null() {
                return false;
            }
            let mut code: u32 = 0;
            let ok = GetExitCodeProcess(handle, &mut code);
            CloseHandle(handle);
            ok != 0 && code == STILL_ACTIVE
        }
    }
}

/// Asks a process group to stop. On Windows there is no polite signal for a hidden console
/// process, so this is a no-op and [`kill_tree`] does the work.
pub fn terminate_tree(pid: u32) {
    #[cfg(unix)]
    signal_group(pid, libc::SIGTERM);
    #[cfg(windows)]
    let _ = pid;
}

/// A process that has ended but was never collected by its parent still answers signal 0.
/// That happens where nothing reaps orphans, for example in a container without an init.
#[cfg(unix)]
fn is_zombie(pid: u32) -> bool {
    #[cfg(target_os = "linux")]
    {
        // "pid (comm) S ...": the name may hold spaces and brackets, so read after the last one.
        std::fs::read_to_string(format!("/proc/{pid}/stat"))
            .ok()
            .and_then(|stat| stat.rsplit_once(')').map(|(_, rest)| rest.trim_start().starts_with(['Z', 'X'])))
            .unwrap_or(false)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pid;
        false
    }
}

/// Every process below `pid`, from the process table. A child that moved to its own process
/// group is not reached by a signal to its parent's group, so the forced path looks it up.
#[cfg(unix)]
fn descendants(pid: u32) -> Vec<u32> {
    let Ok(out) =
        Command::new("ps").args(["-A", "-o", "pid=,ppid="]).stdin(Stdio::null()).stderr(Stdio::null()).output()
    else {
        return Vec::new();
    };
    let table: Vec<(u32, u32)> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            Some((fields.next()?.parse().ok()?, fields.next()?.parse().ok()?))
        })
        .collect();
    let mut found: Vec<u32> = Vec::new();
    let mut frontier = vec![pid];
    while let Some(parent) = frontier.pop() {
        for (child, of) in &table {
            if *of == parent && *child > 1 && !found.contains(child) {
                found.push(*child);
                frontier.push(*child);
            }
        }
    }
    found
}

/// Forcibly stops a process and everything it started.
pub fn kill_tree(pid: u32) {
    #[cfg(unix)]
    {
        let below = descendants(pid);
        signal_group(pid, libc::SIGKILL);
        for child in below {
            signal_group(child, libc::SIGKILL);
        }
    }
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

#[cfg(unix)]
fn signal_group(pid: u32, sig: libc::c_int) {
    if pid <= 1 {
        return;
    }
    // SAFETY: plain signal delivery. The group is tried first, then the process itself, which
    // covers children that were not started as group leaders.
    unsafe {
        if libc::kill(-(pid as libc::pid_t), sig) != 0 {
            libc::kill(pid as libc::pid_t, sig);
        }
    }
}

/// How long a cancelled agent gets between the polite stop and the forced one.
pub fn kill_grace() -> Duration {
    std::env::var("HUFU_KILL_GRACE_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .map(Duration::from_millis)
        .unwrap_or(Duration::from_secs(10))
}

// ---------------------------------------------------------------------------------------------
// Environment
// ---------------------------------------------------------------------------------------------

const BASE_ENV: &[&str] = &[
    "PATH",
    "HOME",
    "USER",
    "LOGNAME",
    "SHELL",
    "LANG",
    "LANGUAGE",
    "TZ",
    "TERM",
    "TMPDIR",
    "TEMP",
    "TMP",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_CACHE_HOME",
    "XDG_STATE_HOME",
    "XDG_RUNTIME_DIR",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "NO_PROXY",
    "ALL_PROXY",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "NODE_EXTRA_CA_CERTS",
    // Windows
    "SYSTEMROOT",
    "SYSTEMDRIVE",
    "WINDIR",
    "COMSPEC",
    "PATHEXT",
    "USERPROFILE",
    "USERNAME",
    "USERDOMAIN",
    "APPDATA",
    "LOCALAPPDATA",
    "PROGRAMDATA",
    "PROGRAMFILES",
    "PROGRAMFILES(X86)",
    "PROGRAMW6432",
    "COMMONPROGRAMFILES",
    "HOMEDRIVE",
    "HOMEPATH",
    "NUMBER_OF_PROCESSORS",
    "PROCESSOR_ARCHITECTURE",
    "OS",
    // Hufu's own home must reach helper processes the agent starts (the capability MCP server).
    "HUFU_HOME",
];

/// The small base environment every agent process gets, plus the variables named in `extra`.
/// Names are matched case-insensitively, as Windows does; values are never logged.
pub fn agent_env(extra: &[String]) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for (key, value) in std::env::vars_os() {
        let (Some(key), Some(value)) = (key.to_str(), value.to_str()) else {
            continue;
        };
        let upper = key.to_ascii_uppercase();
        let wanted = BASE_ENV.contains(&upper.as_str())
            || upper.starts_with("LC_")
            || extra.iter().any(|e| e.eq_ignore_ascii_case(key));
        if wanted {
            out.insert(key.to_string(), value.to_string());
        }
    }
    out
}

/// The user's home directory.
pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|v| !v.is_empty())
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
}

/// Expands a leading `~` to the user's home directory.
pub fn expand_tilde(path: &str) -> PathBuf {
    if path == "~" {
        return home_dir().unwrap_or_else(|| PathBuf::from(path));
    }
    if let Some(rest) = path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\"))
        && let Some(home) = home_dir()
    {
        return home.join(rest);
    }
    PathBuf::from(path)
}

// ---------------------------------------------------------------------------------------------
// Running and capturing
// ---------------------------------------------------------------------------------------------

/// Output of a short helper command.
#[derive(Clone, Debug, Default)]
pub struct Captured {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
}

impl Captured {
    pub fn ok(&self) -> bool {
        self.code == Some(0) && !self.timed_out
    }
}

/// Runs a command to completion with a time limit and returns what it printed.
pub fn run_capture(mut cmd: Command, timeout: Duration) -> std::io::Result<Captured> {
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    new_group(&mut cmd);
    let mut child = cmd.spawn()?;
    let pid = child.id();
    let out = slurp(child.stdout.take());
    let err = slurp(child.stderr.take());
    let started = Instant::now();
    let mut timed_out = false;
    let status = loop {
        match child.try_wait()? {
            Some(status) => break Some(status),
            None if started.elapsed() >= timeout => {
                timed_out = true;
                kill_tree(pid);
                let _ = child.kill();
                break child.wait().ok();
            }
            None => std::thread::sleep(Duration::from_millis(15)),
        }
    };
    // After a timeout a grandchild may still hold the pipes open; do not wait for it forever.
    let wait = if timed_out { Duration::from_millis(500) } else { Duration::from_secs(5) };
    Ok(Captured {
        code: status.and_then(|s| s.code()),
        stdout: out.recv_timeout(wait).unwrap_or_default(),
        stderr: err.recv_timeout(wait).unwrap_or_default(),
        timed_out,
    })
}

fn slurp<R: Read + Send + 'static>(reader: Option<R>) -> mpsc::Receiver<String> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut r) = reader {
            let _ = r.read_to_end(&mut buf);
        }
        let _ = tx.send(String::from_utf8_lossy(&buf).into_owned());
    });
    rx
}

/// How a streamed agent process ended.
#[derive(Clone, Debug, Default)]
pub struct StreamResult {
    pub exit_code: Option<i32>,
    /// The run was stopped because the cancel token fired.
    pub cancelled: bool,
    /// The process could not be started at all.
    pub spawn_error: Option<String>,
    /// The last few kilobytes of stderr, for error messages.
    pub stderr_tail: String,
}

enum Line {
    Data(RawStream, String),
    Eof,
}

fn pump<R: Read + Send + 'static>(reader: R, which: RawStream, tx: mpsc::Sender<Line>) {
    std::thread::spawn(move || {
        let mut reader = BufReader::new(reader);
        let mut buf = Vec::new();
        loop {
            buf.clear();
            match reader.read_until(b'\n', &mut buf) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    while matches!(buf.last(), Some(b'\n') | Some(b'\r')) {
                        buf.pop();
                    }
                    let line = String::from_utf8_lossy(&buf).into_owned();
                    if tx.send(Line::Data(which, line)).is_err() {
                        return;
                    }
                }
            }
        }
        let _ = tx.send(Line::Eof);
    });
}

/// A running agent process whose output arrives line by line.
pub struct Streamed {
    child: Child,
    rx: mpsc::Receiver<Line>,
    open_streams: u8,
    stderr_tail: String,
    stop_requested_at: Option<Instant>,
    killed: bool,
    exited_at: Option<Instant>,
    exit_code: Option<i32>,
    stdin: Option<std::process::ChildStdin>,
}

/// What [`Streamed::next`] produced.
pub enum Step {
    Line(RawStream, String),
    /// Nothing arrived within the poll interval.
    Idle,
    /// The process has exited and all output was read.
    Done,
}

impl Streamed {
    /// Starts `cmd` in its own process group with piped output. When `keep_stdin` is false the
    /// given text (if any) is written to stdin, which is then closed.
    pub fn spawn(
        mut cmd: Command,
        stdin: Option<&str>,
        keep_stdin: bool,
        sink: &dyn EventSink,
    ) -> Result<Streamed, String> {
        cmd.stdin(if stdin.is_some() || keep_stdin { Stdio::piped() } else { Stdio::null() })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        new_group(&mut cmd);
        let mut child = cmd.spawn().map_err(|e| e.to_string())?;
        sink.child_started(child.id());
        let (tx, rx) = mpsc::channel();
        if let Some(out) = child.stdout.take() {
            pump(out, RawStream::Stdout, tx.clone());
        }
        if let Some(err) = child.stderr.take() {
            pump(err, RawStream::Stderr, tx);
        }
        let mut kept = None;
        if let Some(mut pipe) = child.stdin.take() {
            if keep_stdin {
                if let Some(text) = stdin {
                    let _ = pipe.write_all(text.as_bytes());
                    let _ = pipe.flush();
                }
                kept = Some(pipe);
            } else if let Some(text) = stdin {
                // Written from a thread so that a large prompt cannot deadlock against a child
                // that fills its output pipe before reading its input.
                let text = text.to_string();
                std::thread::spawn(move || {
                    let _ = pipe.write_all(text.as_bytes());
                });
            }
        }
        Ok(Streamed {
            child,
            rx,
            open_streams: 2,
            stderr_tail: String::new(),
            stop_requested_at: None,
            killed: false,
            exited_at: None,
            exit_code: None,
            stdin: kept,
        })
    }

    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// Writes one line to the child's stdin. Only valid when stdin was kept open.
    pub fn send_line(&mut self, line: &str) -> std::io::Result<()> {
        match self.stdin.as_mut() {
            Some(pipe) => {
                pipe.write_all(line.as_bytes())?;
                pipe.write_all(b"\n")?;
                pipe.flush()
            }
            None => Err(std::io::Error::new(std::io::ErrorKind::BrokenPipe, "stdin is closed")),
        }
    }

    /// Closes the child's stdin, which is how line protocols signal the end of the session.
    pub fn close_stdin(&mut self) {
        self.stdin = None;
    }

    /// Asks the process group to stop. It is killed if it is still there after the grace period.
    pub fn request_stop(&mut self) {
        if self.stop_requested_at.is_none() {
            self.stop_requested_at = Some(Instant::now());
            self.stdin = None;
            terminate_tree(self.child.id());
        }
    }

    pub fn stop_requested(&self) -> bool {
        self.stop_requested_at.is_some()
    }

    /// Waits up to `poll` for the next line, enforcing the stop grace period as it goes.
    pub fn next(&mut self, sink: &dyn EventSink, poll: Duration) -> Step {
        if self.exited_at.is_none()
            && let Ok(Some(status)) = self.child.try_wait()
        {
            self.exited_at = Some(Instant::now());
            self.exit_code = status.code();
        }
        if let Some(at) = self.stop_requested_at
            && !self.killed
            && self.exited_at.is_none()
            // Windows has no polite signal to wait out, so there the forced stop follows at once.
            && (cfg!(windows) || at.elapsed() >= kill_grace())
        {
            self.killed = true;
            kill_tree(self.child.id());
            let _ = self.child.kill();
        }
        if self.open_streams == 0 {
            return self.finish_or_idle(poll);
        }
        match self.rx.recv_timeout(poll) {
            Ok(Line::Data(which, line)) => {
                sink.raw(which, &line);
                if which == RawStream::Stderr {
                    self.stderr_tail.push_str(&line);
                    self.stderr_tail.push('\n');
                    if self.stderr_tail.len() > 8192 {
                        let cut = self.stderr_tail.len() - 4096;
                        let cut =
                            (cut..self.stderr_tail.len()).find(|i| self.stderr_tail.is_char_boundary(*i)).unwrap_or(0);
                        self.stderr_tail.drain(..cut);
                    }
                }
                Step::Line(which, line)
            }
            Ok(Line::Eof) => {
                self.open_streams -= 1;
                Step::Idle
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                self.open_streams = 0;
                Step::Idle
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // The agent exited but something it started still holds the pipes: stop waiting.
                if self.exited_at.map(|t| t.elapsed() >= Duration::from_secs(2)).unwrap_or(false) {
                    self.open_streams = 0;
                    kill_tree(self.child.id());
                }
                Step::Idle
            }
        }
    }

    fn finish_or_idle(&mut self, poll: Duration) -> Step {
        if self.exited_at.is_some() {
            return Step::Done;
        }
        std::thread::sleep(poll.min(Duration::from_millis(50)));
        Step::Idle
    }

    /// Consumes the handle once [`Step::Done`] was seen (or the caller gave up) and reports how
    /// the process ended. A process that is still alive is killed.
    pub fn finish(mut self, cancel: &CancelToken) -> StreamResult {
        if self.exited_at.is_none() {
            kill_tree(self.child.id());
            let _ = self.child.kill();
            self.exit_code = self.child.wait().ok().and_then(|s| s.code());
        }
        StreamResult {
            exit_code: self.exit_code,
            cancelled: cancel.is_cancelled(),
            spawn_error: None,
            stderr_tail: self.stderr_tail.trim().to_string(),
        }
    }
}

/// Runs a one-shot agent command: writes `stdin`, streams every line to `handler` and to the raw
/// log, stops the process tree when `cancel` fires.
pub fn run_streamed(
    cmd: Command,
    stdin: Option<&str>,
    sink: &dyn EventSink,
    cancel: &CancelToken,
    handler: &mut dyn FnMut(RawStream, &str),
) -> StreamResult {
    let mut proc = match Streamed::spawn(cmd, stdin, false, sink) {
        Ok(p) => p,
        Err(e) => {
            return StreamResult { spawn_error: Some(e), ..StreamResult::default() };
        }
    };
    loop {
        if cancel.is_cancelled() && !proc.stop_requested() {
            proc.request_stop();
        }
        match proc.next(sink, Duration::from_millis(100)) {
            Step::Line(which, line) => handler(which, &line),
            Step::Idle => {}
            Step::Done => break,
        }
    }
    proc.finish(cancel)
}

/// The tail of a text, cut on a character boundary, for error messages.
pub fn tail(text: &str, max: usize) -> String {
    let text = text.trim();
    if text.len() <= max {
        return text.to_string();
    }
    let mut start = text.len() - max;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    format!("…{}", &text[start..])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::EventBody;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Collect(Mutex<Vec<String>>);
    impl EventSink for Collect {
        fn emit(&self, _: EventBody) {}
        fn raw(&self, _: RawStream, line: &str) {
            self.0.lock().unwrap().push(line.to_string());
        }
        fn child_started(&self, _: u32) {}
    }

    #[test]
    fn captures_shell_output_and_exit_code() {
        let out = run_capture(shell_command("echo hello"), Duration::from_secs(10)).unwrap();
        assert!(out.ok());
        assert_eq!(out.stdout.trim(), "hello");
        let bad = run_capture(shell_command("exit 3"), Duration::from_secs(10)).unwrap();
        assert_eq!(bad.code, Some(3));
    }

    #[test]
    fn streams_lines_and_reports_spawn_failures() {
        let sink = Collect::default();
        let cancel = CancelToken::new();
        let mut seen = Vec::new();
        let r = run_streamed(shell_command("echo one&& echo two"), None, &sink, &cancel, &mut |_, l| {
            seen.push(l.to_string())
        });
        assert_eq!(r.exit_code, Some(0));
        assert_eq!(seen.iter().map(|s| s.trim()).collect::<Vec<_>>(), ["one", "two"]);
        assert_eq!(sink.0.lock().unwrap().len(), 2);
        let missing = run_streamed(Command::new("hufu-no-such-program-xyz"), None, &sink, &cancel, &mut |_, _| {});
        assert!(missing.spawn_error.is_some());
    }

    #[test]
    fn finds_programs_and_knows_live_pids() {
        assert!(pid_alive(std::process::id()));
        assert!(!pid_alive(0));
        assert!(which("hufu-no-such-program-xyz").is_none());
        #[cfg(unix)]
        assert!(which("sh").is_some());
        #[cfg(windows)]
        assert!(which("cmd").is_some());
    }

    #[test]
    fn agent_env_is_a_small_allow_list() {
        let env = agent_env(&[]);
        assert!(env.keys().any(|k| k.eq_ignore_ascii_case("PATH")));
        assert!(!env.contains_key("CARGO_MANIFEST_DIR"));
        let with = agent_env(&["CARGO_MANIFEST_DIR".to_string()]);
        assert!(with.contains_key("CARGO_MANIFEST_DIR"));
    }

    #[test]
    fn tail_respects_char_boundaries() {
        assert_eq!(tail("short", 100), "short");
        assert!(tail("ééééééééé", 5).starts_with('…'));
    }
}
