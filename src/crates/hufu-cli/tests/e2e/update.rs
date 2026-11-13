//! `hufu update`: against a release in a local directory, against a local HTTP server, and with
//! a stand-in for cargo. The program that is replaced is always a copy, never the binary under
//! test.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant, SystemTime};

use hufu_core::update::{MANIFEST, asset_name, release_target, sha256_file};
use serde_json::{Value, json};

use crate::common::{Env, HUFU, Ran};
use crate::mcp::Host;

const VERSION: &str = env!("CARGO_PKG_VERSION");
const EXE: &str = std::env::consts::EXE_SUFFIX;

/// An installed copy of Hufu in a directory of its own, as the install scripts leave it.
struct Installed<'a> {
    env: &'a Env,
    exe: PathBuf,
}

impl<'a> Installed<'a> {
    fn new(env: &'a Env) -> Installed<'a> {
        let bin = env.root.join("install").join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let exe = bin.join(format!("hufu{EXE}"));
        std::fs::copy(HUFU, &exe).unwrap();
        // An old date, so that a replaced file can be told from the original.
        let old = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000_000);
        std::fs::File::options().write(true).open(&exe).unwrap().set_modified(old).unwrap();
        // While this file was open for writing, another test thread may have started a process.
        // Until that child has replaced itself, it holds a copy of the handle, and Linux refuses
        // to run a file that is open for writing ("text file busy"). Wait for that to pass.
        let started = Instant::now();
        while let Err(e) = Command::new(&exe).arg("--version").output() {
            assert!(started.elapsed() < Duration::from_secs(20), "the installed copy does not start: {e}");
            std::thread::sleep(Duration::from_millis(20));
        }
        Installed { env, exe }
    }

    fn hufu(&self, args: &[&str]) -> Ran {
        let out = self
            .env
            .command_of(&self.exe, &self.env.work)
            .args(args)
            .stdin(std::process::Stdio::null())
            .output()
            .expect("the installed hufu starts");
        Ran {
            code: out.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        }
    }

    fn json(&self, args: &[&str]) -> (i32, Value) {
        let mut all = vec!["--json"];
        all.extend_from_slice(args);
        let r = self.hufu(&all);
        (r.code, r.json())
    }

    /// Whether the program file is still the one that was first installed.
    fn untouched(&self) -> bool {
        let modified = std::fs::metadata(&self.exe).unwrap().modified().unwrap();
        modified < SystemTime::UNIX_EPOCH + Duration::from_secs(1_100_000_000)
    }

    fn dir(&self) -> &Path {
        self.exe.parent().unwrap()
    }

    /// The files next to the program, other than the program.
    fn leftovers(&self) -> Vec<String> {
        std::fs::read_dir(self.dir())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n != &format!("hufu{EXE}"))
            .collect()
    }

    /// Waits until nothing but the program is left. On Windows the old program is removed a
    /// moment after the updater that ran from it has ended.
    #[track_caller]
    fn assert_clean(&self) {
        let started = Instant::now();
        while !self.leftovers().is_empty() {
            assert!(started.elapsed() < Duration::from_secs(40), "left behind: {:?}", self.leftovers());
            std::thread::sleep(Duration::from_millis(200));
        }
    }
}

impl Drop for Installed<'_> {
    fn drop(&mut self) {
        let _ = self.hufu(&["daemon", "stop"]);
    }
}

fn tar() -> PathBuf {
    if cfg!(windows) {
        let root = std::env::var_os("SystemRoot").expect("SystemRoot is set");
        return PathBuf::from(root).join("System32").join("tar.exe");
    }
    PathBuf::from("tar")
}

/// Builds the files of a release the way the release workflow does: an archive that holds
/// `hufu-<target>/hufu`, and the manifest with its checksum. `claims` is the version the manifest
/// states; `program` is what goes into the archive.
fn release(dir: &Path, claims: &str, program: &Path) {
    let target = release_target().expect("the tests run on a platform with releases");
    let stage = dir.join("stage").join(format!("hufu-{target}"));
    let _ = std::fs::remove_dir_all(dir.join("stage"));
    std::fs::create_dir_all(&stage).unwrap();
    std::fs::copy(program, stage.join(format!("hufu{EXE}"))).unwrap();
    let asset = asset_name(target);
    let _ = std::fs::remove_file(dir.join(&asset));
    let mut cmd = Command::new(tar());
    cmd.current_dir(dir);
    if cfg!(windows) {
        cmd.args(["-a", "-cf", &asset, "-C", "stage", &format!("hufu-{target}")]);
    } else {
        cmd.args(["-czf", &asset, "-C", "stage", &format!("hufu-{target}")]);
    }
    let out = cmd.output().expect("tar is installed");
    assert!(out.status.success(), "tar: {}", String::from_utf8_lossy(&out.stderr));
    let sum = sha256_file(&dir.join(&asset)).unwrap();
    manifest(dir, claims, json!({asset: {"sha256": sum}}));
}

fn manifest(dir: &Path, version: &str, assets: Value) {
    std::fs::create_dir_all(dir).unwrap();
    let doc = json!({"v": 1, "version": version, "tag": format!("v{version}"), "assets": assets});
    std::fs::write(dir.join(MANIFEST), doc.to_string()).unwrap();
}

fn env_with_base(name: &str, base: &str) -> Env {
    let mut env = Env::new(name);
    env.set("HUFU_DOWNLOAD_BASE", base);
    // The tests must not depend on a proxy that the machine happens to use.
    env.set("NO_PROXY", "127.0.0.1,localhost");
    env.set("no_proxy", "127.0.0.1,localhost");
    env
}

fn release_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("hufu-e2e-{}-release-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn check_reports_and_changes_nothing() {
    let dir = release_dir("check");
    let env = env_with_base("update-check", &dir.to_string_lossy());
    let hufu = Installed::new(&env);

    // Nothing published.
    let (code, doc) = hufu.json(&["update", "--check"]);
    assert_eq!(code, 0, "{doc}");
    assert_eq!(
        (&doc["release_found"], &doc["update_available"], &doc["latest"]),
        (&json!(false), &json!(false), &Value::Null)
    );
    assert_eq!(doc["current"], VERSION);
    assert!(hufu.hufu(&["update", "--check"]).expect(0).stdout.contains("no release is published"));

    // A newer release. Its archive is never read by a check.
    manifest(&dir, "99.1.0", json!({asset_name(release_target().unwrap()): {"sha256": "0".repeat(64)}}));
    let (code, doc) = hufu.json(&["update", "--check"]);
    assert_eq!(code, 0, "{doc}");
    assert_eq!(
        (&doc["update_available"], &doc["latest"], &doc["tag"], &doc["binary"]),
        (&json!(true), &json!("99.1.0"), &json!("v99.1.0"), &json!(true))
    );
    assert!(hufu.hufu(&["update", "--check"]).expect(0).stdout.contains("an update is available"));
    // A release that has no binary for this platform.
    manifest(&dir, "99.1.0", json!({}));
    assert_eq!(hufu.json(&["update", "--check"]).1["binary"], false);

    // The installed version itself.
    manifest(&dir, VERSION, json!({}));
    let (_, doc) = hufu.json(&["update", "--check"]);
    assert_eq!((&doc["update_available"], &doc["latest"]), (&json!(false), &json!(VERSION)));
    let (code, doc) = hufu.json(&["update"]);
    assert_eq!((code, &doc["updated"], &doc["reason"]), (0, &json!(false), &json!("up_to_date")));

    // An older release is not an update, unless it is asked for by name.
    manifest(&dir, "0.0.1", json!({}));
    assert_eq!(hufu.json(&["update", "--check"]).1["update_available"], false);
    let (code, doc) = hufu.json(&["update"]);
    assert_eq!((code, &doc["updated"], &doc["reason"]), (0, &json!(false), &json!("newer_than_latest")));
    assert_eq!(hufu.json(&["update", "--check", "--version", "0.0.1"]).1["update_available"], true);

    // A version that is not there, and one that is not a version.
    let (code, doc) = hufu.json(&["update", "--check", "--version", "v7.7.7"]);
    assert_eq!((code, &doc["release_found"], &doc["tag"]), (0, &json!(false), &json!("v7.7.7")));
    let r = hufu.hufu(&["update", "--version", "v7.7.7"]).expect(2);
    assert!(r.stderr.contains("there is no release v7.7.7"), "{}", r.stderr);
    hufu.hufu(&["update", "--version", "newest"]).expect(2);
    hufu.hufu(&["update", "--check", "--force"]).expect(2);

    assert!(hufu.untouched(), "nothing above may replace the program");
    assert!(hufu.leftovers().is_empty(), "{:?}", hufu.leftovers());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_update_replaces_the_program_restarts_the_daemon_and_runs_the_checks() {
    let dir = release_dir("apply");
    release(&dir, VERSION, Path::new(HUFU));
    let env = env_with_base("update-apply", &dir.to_string_lossy());
    let hufu = Installed::new(&env);

    let (_, started) = hufu.json(&["daemon", "start"]);
    let old_daemon = started["daemon"]["pid"].as_u64().expect("a daemon pid");
    // A job that is running while the program is replaced, and a server that an agent keeps open.
    let running = hufu.json(&["run", "-a", "codex", "[[fake:slow=2500]]"]).1["job"].as_str().unwrap().to_string();
    env.until(&running, "running", |s| s["state"] == "running");
    let mut host = Host::start_with(env.command_of(&hufu.exe, &env.work), &[]);
    host.handshake();

    let (code, doc) = hufu.json(&["update", "--force"]);
    assert_eq!(code, 0, "{doc}");
    assert_eq!(
        (&doc["updated"], &doc["method"], &doc["from"], &doc["to"]),
        (&json!(true), &json!("release"), &json!(VERSION), &json!(VERSION))
    );
    assert!(!hufu.untouched(), "the program file must have been replaced");
    assert_eq!(doc["doctor_ok"], true, "{doc}");
    assert_eq!(doc["doctor"]["version"], VERSION);
    assert_eq!(doc["daemon"]["was_running"], true);
    assert_eq!(doc["daemon"]["running"], true, "{doc}");
    let new_daemon = doc["daemon"]["pid"].as_u64().expect("the new daemon's pid");
    assert_ne!(new_daemon, old_daemon, "the daemon must have been restarted");
    assert_eq!(hufu.json(&["daemon", "status"]).1["daemon"]["pid"], new_daemon);

    // The job that was running finishes, and new work starts.
    hufu.hufu(&["wait", &running, "--timeout", "60s"]).expect(0);
    let (code, result) = hufu.json(&["run", "--wait", "-a", "codex", "after the update"]);
    assert_eq!((code, result["state"].as_str()), (0, Some("succeeded")), "{result}");

    // The server that was started before the update still runs the old program. With no daemon
    // to start jobs for it, it starts them itself, from the program that is installed now.
    hufu.hufu(&["daemon", "stop"]).expect(0);
    let job = host.call("hufu_submit", json!({"agent": "codex", "prompt": "from the old server"}));
    let waited = host.call("hufu_wait", json!({"job": job["job"], "timeout_s": 60}));
    assert_eq!(waited["jobs"][0]["state"], "succeeded", "{waited}");
    drop(host);

    // Without a daemon there is none to restart, and none is started.
    let r = hufu.hufu(&["update", "--force"]).expect(0);
    assert!(r.stdout.contains(&format!("updated hufu {VERSION} -> {VERSION}")), "{}", r.stdout);
    assert!(r.stdout.contains("PROFILE"), "the checks are printed: {}", r.stdout);
    assert!(!r.stdout.contains("daemon restarted"), "{}", r.stdout);
    assert_eq!(hufu.json(&["daemon", "status"]).1["running"], false);

    hufu.assert_clean();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_release_that_is_not_what_it_claims_is_refused() {
    let dir = release_dir("refuse");
    let env = env_with_base("update-refuse", &dir.to_string_lossy());
    let hufu = Installed::new(&env);
    let asset = asset_name(release_target().unwrap());

    // The archive is not the one the manifest describes.
    release(&dir, "99.2.0", Path::new(HUFU));
    manifest(&dir, "99.2.0", json!({&asset: {"sha256": "1".repeat(64)}}));
    let r = hufu.hufu(&["update"]).expect(1);
    assert!(r.stderr.contains("checksum mismatch"), "{}", r.stderr);

    // The checksum is right, but the program inside is another version than the release says.
    release(&dir, "99.2.0", Path::new(HUFU));
    let r = hufu.hufu(&["update"]).expect(1);
    assert!(r.stderr.contains(&format!("reports version {VERSION}")), "{}", r.stderr);

    // The checksum is right, but what is inside does not run.
    let junk = dir.join("junk");
    std::fs::write(&junk, b"this is not a program").unwrap();
    release(&dir, "99.2.0", &junk);
    let r = hufu.hufu(&["update"]).expect(1);
    assert!(r.stderr.contains("Nothing was changed"), "{}", r.stderr);

    // The manifest lists an archive that is not there.
    std::fs::remove_file(dir.join(&asset)).unwrap();
    let r = hufu.hufu(&["update"]).expect(1);
    assert!(r.stderr.contains("is missing"), "{}", r.stderr);

    // The manifest itself is damaged.
    std::fs::write(dir.join(MANIFEST), "{").unwrap();
    hufu.hufu(&["update"]).expect(1);
    hufu.hufu(&["update", "--check"]).expect(1);

    assert!(hufu.untouched(), "a refused update must leave the program alone");
    assert!(hufu.leftovers().is_empty(), "{:?}", hufu.leftovers());
    hufu.hufu(&["--version"]).expect(0);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Serves the files of `dir` under `/files/` on a local port. `/forbidden/` answers 403.
fn serve(dir: PathBuf) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let dir = dir.clone();
            std::thread::spawn(move || {
                let mut request = Vec::new();
                let mut byte = [0u8; 1];
                while !request.ends_with(b"\r\n\r\n") && stream.read(&mut byte).unwrap_or(0) == 1 {
                    request.push(byte[0]);
                }
                let request = String::from_utf8_lossy(&request).into_owned();
                let path = request.split_whitespace().nth(1).unwrap_or("/").to_string();
                let (status, body) = match path.strip_prefix("/files/") {
                    Some(name) if !name.contains("..") => match std::fs::read(dir.join(name)) {
                        Ok(bytes) => ("200 OK", bytes),
                        Err(_) => ("404 Not Found", b"not found".to_vec()),
                    },
                    _ if path.starts_with("/forbidden/") => ("403 Forbidden", b"no".to_vec()),
                    _ => ("404 Not Found", b"not found".to_vec()),
                };
                let head = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(&body);
                let _ = stream.flush();
            });
        }
    });
    port
}

fn update_over_http(name: &str, prepare: impl Fn(&mut Env)) {
    let dir = release_dir(name);
    release(&dir, VERSION, Path::new(HUFU));
    let port = serve(dir.clone());
    let mut env = env_with_base(name, &format!("http://127.0.0.1:{port}/files/"));
    prepare(&mut env);
    let hufu = Installed::new(&env);

    let (code, doc) = hufu.json(&["update", "--check"]);
    assert_eq!((code, &doc["latest"], &doc["update_available"]), (0, &json!(VERSION), &json!(false)), "{doc}");
    let (code, doc) = hufu.json(&["update", "--force"]);
    assert_eq!(code, 0, "{doc}");
    assert_eq!(
        (&doc["updated"], &doc["method"], &doc["doctor_ok"]),
        (&json!(true), &json!("release"), &json!(true)),
        "{doc}"
    );
    assert!(!hufu.untouched());

    // A place that has no release is "nothing published"; a server that refuses is an error.
    let mut elsewhere = env_with_base(&format!("{name}-none"), &format!("http://127.0.0.1:{port}/empty"));
    prepare(&mut elsewhere);
    let none = Installed::new(&elsewhere);
    let (code, doc) = none.json(&["update", "--check"]);
    assert_eq!((code, &doc["release_found"]), (0, &json!(false)), "{doc}");
    let mut refused = env_with_base(&format!("{name}-403"), &format!("http://127.0.0.1:{port}/forbidden"));
    prepare(&mut refused);
    let no = Installed::new(&refused);
    let r = no.hufu(&["update", "--check"]).expect(1);
    assert!(r.stderr.contains("cannot download"), "{}", r.stderr);
    assert!(no.hufu(&["update"]).expect(1).stderr.contains("cannot download"));
    assert!(no.untouched());

    hufu.assert_clean();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_update_downloads_over_http() {
    update_over_http("update-http", |_| {});
}

/// Ubuntu's desktop ships wget and no curl.
#[cfg(unix)]
#[test]
fn an_update_downloads_with_wget_where_there_is_no_curl() {
    let find = |name: &str| {
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).map(|d| d.join(name)).find(|p| p.is_file())
    };
    let Some(wget) = find("wget") else {
        eprintln!("skipped: wget is not installed");
        return;
    };
    update_over_http("update-wget", |env| {
        let tools = env.root.join("tools");
        std::fs::create_dir_all(&tools).unwrap();
        for (name, path) in [("wget", Some(wget.clone())), ("tar", find("tar")), ("gzip", find("gzip"))] {
            std::os::unix::fs::symlink(path.unwrap_or_else(|| panic!("{name} is installed")), tools.join(name))
                .unwrap();
        }
        env.set("PATH", &tools.to_string_lossy());
    });
}

/// Puts a program named `cargo` first on `PATH`. It records its arguments and "builds" by copying
/// the binary under test to where `cargo install --root` would put it.
fn fake_cargo(env: &mut Env) -> PathBuf {
    let dir = env.root.join("fake-cargo");
    std::fs::create_dir_all(&dir).unwrap();
    let log = dir.join("args.txt");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let script = dir.join("cargo");
        std::fs::write(
            &script,
            "#!/bin/sh\necho \"$*\" > \"$FAKE_CARGO_LOG\"\nwhile [ $# -gt 0 ]; do\n  if [ \"$1\" = \"--root\" ]; then root=\"$2\"; fi\n  shift\ndone\nmkdir -p \"$root/bin\" && cp \"$FAKE_CARGO_BINARY\" \"$root/bin/hufu\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    #[cfg(windows)]
    std::fs::write(
        dir.join("cargo.cmd"),
        "@echo off\r\nsetlocal\r\necho %*> \"%FAKE_CARGO_LOG%\"\r\n:next\r\nif \"%~1\"==\"\" goto done\r\nif \"%~1\"==\"--root\" set \"ROOT=%~2\"\r\nshift\r\ngoto next\r\n:done\r\nmkdir \"%ROOT%\\bin\"\r\ncopy /Y \"%FAKE_CARGO_BINARY%\" \"%ROOT%\\bin\\hufu.exe\" >nul\r\n",
    )
    .unwrap();
    let rest = std::env::var_os("PATH").unwrap_or_default();
    let path = std::env::join_paths(std::iter::once(dir).chain(std::env::split_paths(&rest))).unwrap();
    env.set("PATH", &path.to_string_lossy());
    env.set("FAKE_CARGO_LOG", &log.to_string_lossy());
    env.set("FAKE_CARGO_BINARY", HUFU);
    log
}

#[test]
fn with_no_release_the_update_builds_from_source() {
    let dir = release_dir("source");
    let mut env = env_with_base("update-source", &dir.to_string_lossy());
    let log = fake_cargo(&mut env);
    let hufu = Installed::new(&env);

    // Nothing is published: the main branch is built.
    let r = hufu.hufu(&["update"]).expect(0);
    assert!(r.stderr.contains("building the main branch from source"), "{}", r.stderr);
    assert!(r.stdout.contains(&format!("updated hufu {VERSION} -> {VERSION}")), "{}", r.stdout);
    assert!(!hufu.untouched());
    let args = std::fs::read_to_string(&log).unwrap();
    for wanted in
        ["install", "--git", "https://github.com/kverona-ai/hufu", "hufu-cli", "--locked", "--force", "--root"]
    {
        assert!(args.contains(wanted), "cargo was called with: {args}");
    }
    assert!(!args.contains("--tag"), "{args}");

    // A release exists, and the source of a tag is asked for anyway.
    release(&dir, VERSION, Path::new(HUFU));
    let (code, doc) = hufu.json(&["update", "--from-source", "--version", "1.2.3"]);
    assert_eq!((code, &doc["updated"], &doc["method"]), (0, &json!(true), &json!("source")), "{doc}");
    let args = std::fs::read_to_string(&log).unwrap();
    assert!(args.contains("--tag") && args.contains("v1.2.3"), "{args}");

    // A release without a binary for this platform is built from its tag.
    manifest(&dir, "99.3.0", json!({}));
    let r = hufu.hufu(&["update"]).expect(0);
    assert!(r.stderr.contains("has no binary for this platform"), "{}", r.stderr);
    assert!(std::fs::read_to_string(&log).unwrap().contains("v99.3.0"));

    hufu.assert_clean();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn building_from_source_without_rust_says_what_is_missing() {
    let dir = release_dir("no-rust");
    let mut env = env_with_base("update-no-rust", &dir.to_string_lossy());
    let empty = env.root.join("empty-path");
    std::fs::create_dir_all(&empty).unwrap();
    env.set("PATH", &empty.to_string_lossy());
    let hufu = Installed::new(&env);
    let r = hufu.hufu(&["update"]).expect(1);
    assert!(r.stderr.contains("https://rustup.rs"), "{}", r.stderr);
    assert!(hufu.untouched());
    assert!(hufu.leftovers().is_empty(), "{:?}", hufu.leftovers());
    let _ = std::fs::remove_dir_all(&dir);
}
