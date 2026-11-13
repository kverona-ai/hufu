//! `hufu update`: find a release, download and check it, and replace the installed program.
//!
//! No network or archive library is linked for this ([ADR-0013]). A download is one run of the
//! system's `curl` (or `wget`), an archive is unpacked by the system's `tar`. The SHA-256 check,
//! which is what the safety of an update rests on, is done here, and so is the version check of
//! the new program before it replaces the old one.
//!
//! A release is described by one small file, `hufu-manifest.json`, published next to its archives:
//! the version, the tag, and the SHA-256 sum of every archive. The newest release is found by
//! reading that file from `releases/latest/download/`; the archive is then taken from the tag the
//! manifest names, so a release published in between cannot mix two versions.
//!
//! [ADR-0013]: ../../../../doc/adr/0013-self-update-with-system-tools.md

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use hufu_contract::proc;
use serde::{Deserialize, Serialize};

use crate::error::{HufuError, IoContext, Result};

/// Where releases are published.
pub const REPO: &str = "kverona-ai/hufu";
/// The file that describes a release.
pub const MANIFEST: &str = "hufu-manifest.json";
/// Replaces GitHub as the place to download from: a URL or a local directory that holds the
/// manifest and the archives side by side. The install scripts read the same variable.
pub const BASE_ENV: &str = "HUFU_DOWNLOAD_BASE";

const DOWNLOAD_LIMIT: Duration = Duration::from_secs(30 * 60);

// ---------------------------------------------------------------------------------------------
// SHA-256
// ---------------------------------------------------------------------------------------------

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5, 0xd807aa98,
    0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786,
    0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8,
    0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13,
    0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819,
    0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a,
    0x5b9cca4f, 0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
    0xc67178f2,
];

/// SHA-256 as specified in FIPS 180-4.
pub struct Sha256 {
    state: [u32; 8],
    block: [u8; 64],
    filled: usize,
    length: u64,
}

impl Default for Sha256 {
    fn default() -> Self {
        Sha256 {
            state: [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19],
            block: [0; 64],
            filled: 0,
            length: 0,
        }
    }
}

impl Sha256 {
    pub fn update(&mut self, mut data: &[u8]) {
        self.length = self.length.wrapping_add(data.len() as u64);
        while !data.is_empty() {
            let take = (64 - self.filled).min(data.len());
            self.block[self.filled..self.filled + take].copy_from_slice(&data[..take]);
            self.filled += take;
            data = &data[take..];
            if self.filled == 64 {
                self.compress();
                self.filled = 0;
            }
        }
    }

    fn compress(&mut self) {
        let mut w = [0u32; 64];
        for (i, chunk) in self.block.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = self.state;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = h.wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (s, v) in self.state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *s = s.wrapping_add(v);
        }
    }

    /// The digest as lowercase hex.
    pub fn finish(mut self) -> String {
        let bits = self.length.wrapping_mul(8);
        self.update(&[0x80]);
        while self.filled != 56 {
            self.update(&[0]);
        }
        self.block[56..].copy_from_slice(&bits.to_be_bytes());
        self.compress();
        self.state.iter().map(|word| format!("{word:08x}")).collect()
    }
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hash = Sha256::default();
    hash.update(bytes);
    hash.finish()
}

pub fn sha256_file(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path).at(path)?;
    let mut hash = Sha256::default();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf).at(path)?;
        if n == 0 {
            return Ok(hash.finish());
        }
        hash.update(&buf[..n]);
    }
}

// ---------------------------------------------------------------------------------------------
// Versions
// ---------------------------------------------------------------------------------------------

/// One dot-separated part of a pre-release such as `rc.1`. Numbers sort before words.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Pre {
    Number(u64),
    Word(String),
}

/// A version as Hufu releases use them: `1.2.3` or `1.2.3-rc.1`, ordered as Semantic Versioning
/// orders them. Build metadata (`+...`) is accepted and ignored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Version {
    core: [u64; 3],
    pre: Vec<Pre>,
}

impl Version {
    /// Parses `1.2.3`, `v1.2.3` and `1.2.3-rc.1`.
    pub fn parse(text: &str) -> Option<Version> {
        let text = text.trim();
        let text = text.strip_prefix('v').unwrap_or(text);
        let text = text.split_once('+').map(|(v, _)| v).unwrap_or(text);
        let (core, pre) = match text.split_once('-') {
            Some((core, pre)) => (core, Some(pre)),
            None => (text, None),
        };
        let mut numbers = core
            .split('.')
            .map(|n| (!n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())).then(|| n.parse::<u64>().ok()).flatten());
        let core = [numbers.next()??, numbers.next()??, numbers.next()??];
        if numbers.next().is_some() {
            return None;
        }
        let mut parts = Vec::new();
        for part in pre.map(|p| p.split('.')).into_iter().flatten() {
            if part.is_empty() || !part.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
                return None;
            }
            parts.push(match part.parse::<u64>() {
                Ok(n) if part.bytes().all(|b| b.is_ascii_digit()) => Pre::Number(n),
                _ => Pre::Word(part.to_string()),
            });
        }
        Some(Version { core, pre: parts })
    }

    pub fn is_prerelease(&self) -> bool {
        !self.pre.is_empty()
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        self.core.cmp(&other.core).then_with(|| match (self.pre.is_empty(), other.pre.is_empty()) {
            (true, true) => Ordering::Equal,
            // A release comes after its own pre-releases.
            (true, false) => Ordering::Greater,
            (false, true) => Ordering::Less,
            (false, false) => self.pre.cmp(&other.pre),
        })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// The tag of a version given as `0.1.0` or `v0.1.0`. Refuses anything that is not a version, so
/// that the text can be put into a URL.
pub fn tag_for(version: &str) -> Result<String> {
    let version = version.trim();
    Version::parse(version)
        .map(|_| format!("v{}", version.strip_prefix('v').unwrap_or(version)))
        .ok_or_else(|| HufuError::usage(format!("`{version}` is not a version; expected one like 0.1.0 or v0.1.0")))
}

// ---------------------------------------------------------------------------------------------
// Platforms and assets
// ---------------------------------------------------------------------------------------------

/// The release target for an operating system and CPU, as named by [`std::env::consts`].
pub fn target_for(os: &str, arch: &str) -> Option<&'static str> {
    Some(match (os, arch) {
        // Linux releases are static (musl) and run on any distribution.
        ("linux", "x86_64") => "x86_64-unknown-linux-musl",
        ("linux", "aarch64") => "aarch64-unknown-linux-musl",
        ("macos", "x86_64") => "x86_64-apple-darwin",
        ("macos", "aarch64") => "aarch64-apple-darwin",
        ("windows", "x86_64") => "x86_64-pc-windows-msvc",
        ("windows", "aarch64") => "aarch64-pc-windows-msvc",
        _ => return None,
    })
}

/// The release target of this program, or `None` where no binary is published.
pub fn release_target() -> Option<&'static str> {
    target_for(std::env::consts::OS, std::env::consts::ARCH)
}

/// The archive that holds the binary for a target.
pub fn asset_name(target: &str) -> String {
    let ext = if target.contains("windows") { "zip" } else { "tar.gz" };
    format!("hufu-{target}.{ext}")
}

fn exe_name(target: &str) -> &'static str {
    if target.contains("windows") { "hufu.exe" } else { "hufu" }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Asset {
    pub sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
}

/// `hufu-manifest.json`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub version: String,
    pub tag: String,
    #[serde(default)]
    pub assets: BTreeMap<String, Asset>,
}

impl Manifest {
    pub fn parse(text: &str) -> Result<Manifest> {
        let bad = |why: String| HufuError::internal(format!("the release manifest is not usable: {why}"));
        let manifest: Manifest = serde_json::from_str(text).map_err(|e| bad(e.to_string()))?;
        if Version::parse(&manifest.version).is_none() {
            return Err(bad(format!("`{}` is not a version", manifest.version)));
        }
        // The tag becomes part of a URL.
        let tag_ok = !manifest.tag.is_empty()
            && manifest.tag.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_' | b'+'));
        if !tag_ok {
            return Err(bad(format!("`{}` is not a tag", manifest.tag)));
        }
        for (name, asset) in &manifest.assets {
            if asset.sha256.len() != 64 || !asset.sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(bad(format!("the checksum of {name} is not a SHA-256 sum")));
            }
        }
        Ok(manifest)
    }
}

// ---------------------------------------------------------------------------------------------
// Where releases come from
// ---------------------------------------------------------------------------------------------

/// Where the manifest and the archives are read from.
#[derive(Clone, Debug, PartialEq)]
pub enum Source {
    /// The project's GitHub releases.
    GitHub,
    /// A mirror: every file of one release under one URL.
    Mirror(String),
    /// A local directory with the files of one release.
    Dir(PathBuf),
}

enum Location {
    Url(String),
    File(PathBuf),
}

impl Source {
    /// GitHub, unless `HUFU_DOWNLOAD_BASE` names a mirror or a directory.
    pub fn from_env() -> Source {
        match std::env::var(BASE_ENV) {
            Ok(base) if !base.trim().is_empty() => Source::parse(base.trim()),
            _ => Source::GitHub,
        }
    }

    pub fn parse(base: &str) -> Source {
        if base.starts_with("http://") || base.starts_with("https://") {
            return Source::Mirror(base.trim_end_matches('/').to_string());
        }
        let path = base.strip_prefix("file://").unwrap_or(base);
        // file:///C:/dir on Windows.
        let drive = |p: &str| p.as_bytes().get(2) == Some(&b':');
        let path = if cfg!(windows) && path.starts_with('/') && drive(path) { &path[1..] } else { path };
        Source::Dir(proc::expand_tilde(path))
    }

    /// A release always comes from one place, so a mirror or a directory holds one version only.
    pub fn is_github(&self) -> bool {
        *self == Source::GitHub
    }

    /// For messages: where this source looks.
    pub fn describe(&self) -> String {
        match self {
            Source::GitHub => format!("https://github.com/{REPO}/releases"),
            Source::Mirror(url) => url.clone(),
            Source::Dir(dir) => dir.display().to_string(),
        }
    }

    fn file(&self, tag: Option<&str>, name: &str) -> Location {
        match (self, tag) {
            (Source::GitHub, Some(tag)) => {
                Location::Url(format!("https://github.com/{REPO}/releases/download/{tag}/{name}"))
            }
            (Source::GitHub, None) => {
                Location::Url(format!("https://github.com/{REPO}/releases/latest/download/{name}"))
            }
            (Source::Mirror(base), _) => Location::Url(format!("{base}/{name}")),
            (Source::Dir(dir), _) => Location::File(dir.join(name)),
        }
    }
}

impl Location {
    fn describe(&self) -> String {
        match self {
            Location::Url(url) => url.clone(),
            Location::File(path) => path.display().to_string(),
        }
    }
}

/// A system program by name. On Windows the copy that ships with the system is preferred: a
/// `tar` from another toolkit earlier on `PATH` may not read zip archives.
fn system_tool(name: &str) -> Option<PathBuf> {
    if cfg!(windows)
        && let Some(root) = std::env::var_os("SystemRoot").or_else(|| std::env::var_os("windir"))
    {
        let shipped = PathBuf::from(root).join("System32").join(format!("{name}.exe"));
        if shipped.is_file() {
            return Some(shipped);
        }
    }
    proc::which(name)
}

/// Fetches one file. `Ok(false)` means the server, or the directory, does not have it.
fn fetch(from: &Location, to: &Path) -> Result<bool> {
    let url = match from {
        Location::File(path) => {
            if !path.is_file() {
                return Ok(false);
            }
            std::fs::copy(path, to).at(path)?;
            return Ok(true);
        }
        Location::Url(url) => url,
    };
    let failed = |why: String| HufuError::internal(format!("cannot download {url}: {why}"));
    if let Some(curl) = system_tool("curl") {
        let mut cmd = Command::new(curl);
        // No --fail: the status code tells a missing file from a server in trouble.
        cmd.args(["-sS", "-L", "--retry", "2", "--connect-timeout", "20", "-w", "%{http_code}", "-o"]).arg(to).arg(url);
        let out = proc::run_capture(cmd, DOWNLOAD_LIMIT).map_err(|e| failed(e.to_string()))?;
        if out.timed_out {
            return Err(failed("it took too long".into()));
        }
        if !out.ok() {
            return Err(failed(proc::tail(out.stderr.trim(), 300)));
        }
        return match out.stdout.trim() {
            "200" => Ok(true),
            "404" => Ok(false),
            status => Err(failed(format!("the server answered with status {status}"))),
        };
    }
    if !cfg!(windows)
        && let Some(wget) = proc::which("wget")
    {
        let mut cmd = Command::new(wget);
        // --server-response prints the status lines, which tell a missing file from a refusal.
        cmd.args(["-q", "--server-response", "--tries=3", "--timeout=20", "-O"]).arg(to).arg(url);
        let out = proc::run_capture(cmd, DOWNLOAD_LIMIT).map_err(|e| failed(e.to_string()))?;
        if out.timed_out {
            return Err(failed("it took too long".into()));
        }
        // The last status line is the answer after any redirects.
        let status = out
            .stderr
            .lines()
            .filter_map(|line| line.trim().strip_prefix("HTTP/"))
            .filter_map(|line| line.split_whitespace().nth(1))
            .next_back()
            .unwrap_or("");
        return match (out.code, status) {
            (Some(0), _) => Ok(true),
            (_, "404") => Ok(false),
            (_, "") => Err(failed(format!("wget ended with {:?}", out.code))),
            (_, status) => Err(failed(format!("the server answered with status {status}"))),
        };
    }
    Err(HufuError::internal(if cfg!(windows) {
        "hufu update downloads with curl.exe, which is part of Windows 10 (1803) and later and was not found. Run the install script again instead"
    } else {
        "hufu update downloads with curl or wget, and neither is installed. Install one of them, or run the install script again"
    }))
}

// ---------------------------------------------------------------------------------------------
// Finding and downloading a release
// ---------------------------------------------------------------------------------------------

/// A published release, as its manifest describes it.
#[derive(Clone, Debug)]
pub struct Release {
    pub version: String,
    pub tag: String,
    /// The archive for this machine and its SHA-256 sum; `None` when the release has no binary
    /// for this platform.
    pub asset: Option<(String, String)>,
    source: Source,
}

/// Looks for a release: the wanted version, or the newest one. `Ok(None)` means that there is no
/// such release, which is different from not being able to ask.
pub fn find_release(source: &Source, wanted: Option<&str>, scratch: &Path) -> Result<Option<Release>> {
    let tag = wanted.map(tag_for).transpose()?;
    let from = source.file(tag.as_deref(), MANIFEST);
    let file = scratch.join(MANIFEST);
    if !fetch(&from, &file)? {
        return Ok(None);
    }
    let text = std::fs::read_to_string(&file).at(&file)?;
    let manifest =
        Manifest::parse(&text).map_err(|e| HufuError::internal(format!("{}: {}", from.describe(), e.message)))?;
    if let Some(tag) = &tag {
        let same = Version::parse(tag) == Version::parse(&manifest.version);
        if !same && source.is_github() {
            return Err(HufuError::internal(format!(
                "the manifest of release {tag} describes version {}; not installing it",
                manifest.version
            )));
        }
        if !same {
            // A mirror or a directory holds one release. It is not the one asked for.
            return Ok(None);
        }
    }
    let asset = release_target().map(asset_name).and_then(|name| {
        let sum = manifest.assets.get(&name)?.sha256.to_ascii_lowercase();
        Some((name, sum))
    });
    Ok(Some(Release { version: manifest.version, tag: manifest.tag, asset, source: source.clone() }))
}

/// Downloads the archive of a release, checks its SHA-256 sum against the manifest and unpacks
/// it. Returns the new program, still inside `scratch`.
pub fn download(release: &Release, scratch: &Path) -> Result<PathBuf> {
    let target = release_target();
    let (Some(target), Some((name, want))) = (target, &release.asset) else {
        return Err(HufuError::internal(format!(
            "release {} has no binary for this platform ({} {})",
            release.tag,
            std::env::consts::OS,
            std::env::consts::ARCH
        )));
    };
    let from = release.source.file(Some(&release.tag), name);
    let archive = scratch.join(name);
    if !fetch(&from, &archive)? {
        return Err(HufuError::internal(format!(
            "release {} lists {name}, but {} is missing",
            release.tag,
            from.describe()
        )));
    }
    let got = sha256_file(&archive)?;
    if &got != want {
        return Err(HufuError::internal(format!(
            "checksum mismatch for {name}: the release says {want}, the download has {got}. Nothing was changed"
        )));
    }
    let unpacked = scratch.join("unpacked");
    std::fs::create_dir_all(&unpacked).at(&unpacked)?;
    unpack(&archive, &unpacked)?;
    let exe = unpacked.join(format!("hufu-{target}")).join(exe_name(target));
    if !exe.is_file() {
        return Err(HufuError::internal(format!("{name} does not contain hufu-{target}/{}", exe_name(target))));
    }
    Ok(exe)
}

fn unpack(archive: &Path, into: &Path) -> Result<()> {
    let tar = system_tool("tar").ok_or_else(|| {
        HufuError::internal(if cfg!(windows) {
            "hufu update unpacks with tar.exe, which is part of Windows 10 (1803) and later and was not found. Run the install script again instead"
        } else {
            "hufu update unpacks with tar, which is not installed. Install it, or run the install script again"
        })
    })?;
    let mut cmd = Command::new(tar);
    // bsdtar, which Windows ships, reads zip archives; `-f` detects the format by itself.
    let is_zip = archive.extension().is_some_and(|e| e == "zip");
    cmd.arg(if is_zip { "-xf" } else { "-xzf" }).arg(archive).arg("-C").arg(into);
    let out = proc::run_capture(cmd, Duration::from_secs(300)).map_err(|e| HufuError::internal(format!("tar: {e}")))?;
    if !out.ok() {
        return Err(HufuError::internal(format!(
            "cannot unpack {}: {}",
            archive.display(),
            proc::tail(out.stderr.trim(), 300)
        )));
    }
    Ok(())
}

/// Builds Hufu from source with cargo, from a tag or from the main branch. Returns the new
/// program, still inside `scratch`. Cargo's progress goes to stderr.
pub fn build_from_source(tag: Option<&str>, scratch: &Path, quiet: bool) -> Result<PathBuf> {
    let cargo = proc::which("cargo").ok_or_else(|| {
        HufuError::internal(
            "building from source needs Rust: install it from https://rustup.rs and run `hufu update` again",
        )
    })?;
    let root = scratch.join("build");
    let mut cmd = proc::command_for(&cargo);
    cmd.args(["install", "--git", &format!("https://github.com/{REPO}"), "hufu-cli", "--locked", "--force", "--root"])
        .arg(&root);
    if let Some(tag) = tag {
        cmd.args(["--tag", tag]);
    }
    if quiet {
        cmd.arg("--quiet");
    }
    // Standard output belongs to the caller's own report.
    cmd.stdin(Stdio::null()).stdout(Stdio::null());
    let status = cmd.status().map_err(|e| HufuError::internal(format!("cannot start cargo: {e}")))?;
    if !status.success() {
        return Err(HufuError::internal("cargo could not build Hufu; see its messages above. Nothing was changed"));
    }
    let exe = root.join("bin").join(format!("hufu{}", std::env::consts::EXE_SUFFIX));
    if !exe.is_file() {
        return Err(HufuError::internal(format!("cargo reported success but {} is missing", exe.display())));
    }
    Ok(exe)
}

/// Runs a freshly obtained program and returns the version it reports. A program that does not
/// start on this machine must never replace one that does.
pub fn probe_version(exe: &Path) -> Result<String> {
    let mut cmd = Command::new(exe);
    cmd.arg("--version");
    let out = proc::run_capture(cmd, Duration::from_secs(30))
        .map_err(|e| HufuError::internal(format!("the new program does not start ({e}). Nothing was changed")))?;
    out.stdout.trim().strip_prefix("hufu ").filter(|_| out.ok()).map(|v| v.trim().to_string()).ok_or_else(|| {
        HufuError::internal("the new program does not answer `--version` as hufu does. Nothing was changed")
    })
}

// ---------------------------------------------------------------------------------------------
// Replacing the program
// ---------------------------------------------------------------------------------------------

/// A directory for one update, removed when dropped.
pub struct Scratch(PathBuf);

impl Scratch {
    pub fn new() -> Result<Scratch> {
        let dir = std::env::temp_dir().join(hufu_contract::time::new_id("hufu-update"));
        std::fs::create_dir_all(&dir).at(&dir)?;
        Ok(Scratch(dir))
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The file an update replaces: this program, with symbolic links followed.
pub fn installed_exe() -> Result<PathBuf> {
    let exe =
        std::env::current_exe().map_err(|e| HufuError::internal(format!("cannot find the hufu executable: {e}")))?;
    let real = exe.canonicalize().unwrap_or(exe);
    // Windows spells a canonical path `\\?\C:\...`; the short form means the same file.
    let text = real.to_string_lossy();
    Ok(match text.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with("UNC") => PathBuf::from(rest),
        _ => real,
    })
}

fn sibling(dest: &Path, tag: &str) -> PathBuf {
    let dir = dest.parent().unwrap_or(Path::new("."));
    dir.join(format!(".hufu.{tag}-{}{}", std::process::id(), std::env::consts::EXE_SUFFIX))
}

/// Fails early, before anything is downloaded, when the program's directory cannot be written.
pub fn check_writable(dest: &Path) -> Result<()> {
    let probe = sibling(dest, "probe");
    let made = std::fs::File::create(&probe);
    let _ = std::fs::remove_file(&probe);
    made.map(|_| ()).map_err(|e| {
        HufuError::internal(format!(
            "cannot write next to {} ({e}). Run `hufu update` as the user that installed Hufu, or run the install script again",
            dest.display()
        ))
    })
}

/// Puts `new` in the place of `dest`. The new file is first written next to the old one and then
/// renamed, so there is no moment at which `dest` is half written, and a process that is running
/// the old program keeps running it.
pub fn install(new: &Path, dest: &Path) -> Result<()> {
    let staged = sibling(dest, "new");
    let prepared = std::fs::copy(new, &staged).map(|_| ()).and_then(|()| make_executable(&staged));
    let done = prepared.and_then(|()| swap(&staged, dest));
    done.map_err(|e| {
        let _ = std::fs::remove_file(&staged);
        HufuError::internal(format!("cannot replace {}: {e}. The installed program was left as it was", dest.display()))
    })
}

#[cfg(unix)]
fn make_executable(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(not(windows))]
fn swap(staged: &Path, dest: &Path) -> std::io::Result<()> {
    std::fs::rename(staged, dest)
}

/// Windows does not let a running program be overwritten or deleted, but it lets it be renamed.
/// The old program is moved aside, the new one takes its name, and the old file is removed once
/// nothing runs from it any more (see [`sweep_old`]).
#[cfg(windows)]
fn swap(staged: &Path, dest: &Path) -> std::io::Result<()> {
    use crate::store::retry;
    let aside = old_name(dest);
    let had = dest.exists();
    if had {
        retry(|| std::fs::rename(dest, &aside))?;
    }
    // A scanner may hold the freshly written file for a moment.
    if let Err(e) = retry(|| std::fs::rename(staged, dest)) {
        if had {
            let _ = retry(|| std::fs::rename(&aside, dest));
        }
        return Err(e);
    }
    let _ = std::fs::remove_file(&aside);
    Ok(())
}

#[cfg(windows)]
fn old_name(dest: &Path) -> PathBuf {
    let dir = dest.parent().unwrap_or(Path::new("."));
    dir.join(format!("{}{}.exe", old_prefix(dest), hufu_contract::time::new_id("o").trim_start_matches("o_")))
}

fn old_prefix(dest: &Path) -> String {
    format!("{}.old-", dest.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "hufu".into()))
}

/// Removes programs that earlier updates moved aside (`hufu.old-*.exe`, Windows only) and that
/// nothing runs from any more. Returns the names of those that are still there.
pub fn sweep_old(dest: &Path) -> Vec<String> {
    let prefix = old_prefix(dest);
    let Some(entries) = dest.parent().and_then(|dir| std::fs::read_dir(dir).ok()) else {
        return Vec::new();
    };
    let mut left = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with(&prefix) && name.ends_with(".exe") && std::fs::remove_file(entry.path()).is_err() {
            left.push(name);
        }
    }
    left
}

/// The helper that an update leaves behind for a moment: it removes the programs in `known` as
/// soon as the processes that run from them have ended, the updater itself among them.
///
/// It gives up after a while, and at once when a program appears that it was not told about.
/// That means a later update has moved this helper's own program aside: nothing could remove
/// that file while this helper is running from it, and the later update has a helper of its own.
pub fn sweep_until_gone(dest: &Path, known: &[String]) {
    for _ in 0..60 {
        let left = sweep_old(dest);
        if left.is_empty() || left.iter().any(|name| !known.contains(name)) {
            return;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_matches_the_published_test_vectors() {
        assert_eq!(sha256_hex(b""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        assert_eq!(sha256_hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert_eq!(
            sha256_hex(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
        // One million times "a", fed in uneven pieces so that every buffer boundary is crossed.
        let mut hash = Sha256::default();
        let data = vec![b'a'; 1_000_000];
        let mut rest = &data[..];
        let mut step = 1;
        while !rest.is_empty() {
            let take = step.min(rest.len());
            hash.update(&rest[..take]);
            rest = &rest[take..];
            step = step * 3 % 1021 + 1;
        }
        assert_eq!(hash.finish(), "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0");
    }

    #[test]
    fn sha256_of_a_file_is_the_sha256_of_its_bytes() {
        let dir = Scratch::new().unwrap();
        let file = dir.path().join("blob");
        let bytes: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(&file, &bytes).unwrap();
        assert_eq!(sha256_file(&file).unwrap(), sha256_hex(&bytes));
    }

    #[test]
    fn versions_are_ordered_as_semantic_versioning_orders_them() {
        let ordered = [
            "0.1.0-alpha",
            "0.1.0-alpha.1",
            "0.1.0-alpha.beta",
            "0.1.0-beta",
            "0.1.0-beta.2",
            "0.1.0-beta.11",
            "0.1.0-rc.1",
            "0.1.0",
            "0.1.1",
            "0.2.0",
            "0.10.0",
            "1.0.0",
        ];
        for pair in ordered.windows(2) {
            let (a, b) = (Version::parse(pair[0]).unwrap(), Version::parse(pair[1]).unwrap());
            assert!(a < b, "{} must come before {}", pair[0], pair[1]);
            assert!(b > a);
        }
        assert_eq!(Version::parse("v1.2.3"), Version::parse("1.2.3"));
        assert_eq!(Version::parse("1.2.3+build.5"), Version::parse("1.2.3"));
        assert!(Version::parse("0.1.0-rc.1").unwrap().is_prerelease());
        assert!(!Version::parse("0.1.0").unwrap().is_prerelease());
        for bad in
            ["", "1", "1.2", "1.2.3.4", "1.2.x", "latest", "1.2.3-", "1.2.3-a..b", "1.2.3-a/b", "-1.2.3", "1.+2.3"]
        {
            assert!(Version::parse(bad).is_none(), "`{bad}` is not a version");
        }
    }

    #[test]
    fn a_tag_is_only_made_from_a_version() {
        assert_eq!(tag_for("0.1.0").unwrap(), "v0.1.0");
        assert_eq!(tag_for("v0.1.0-rc.1").unwrap(), "v0.1.0-rc.1");
        assert!(tag_for("../../other/repo").is_err());
        assert!(tag_for("main").is_err());
    }

    #[test]
    fn every_supported_platform_has_an_asset() {
        assert_eq!(asset_name(target_for("linux", "x86_64").unwrap()), "hufu-x86_64-unknown-linux-musl.tar.gz");
        assert_eq!(asset_name(target_for("linux", "aarch64").unwrap()), "hufu-aarch64-unknown-linux-musl.tar.gz");
        assert_eq!(asset_name(target_for("macos", "aarch64").unwrap()), "hufu-aarch64-apple-darwin.tar.gz");
        assert_eq!(asset_name(target_for("macos", "x86_64").unwrap()), "hufu-x86_64-apple-darwin.tar.gz");
        assert_eq!(asset_name(target_for("windows", "x86_64").unwrap()), "hufu-x86_64-pc-windows-msvc.zip");
        assert_eq!(asset_name(target_for("windows", "aarch64").unwrap()), "hufu-aarch64-pc-windows-msvc.zip");
        assert_eq!(target_for("freebsd", "x86_64"), None);
        assert_eq!(target_for("linux", "riscv64"), None);
        assert!(release_target().is_some(), "the tests run on a platform that releases are built for");
    }

    #[test]
    fn a_manifest_is_checked_before_it_is_believed() {
        let sum = "a".repeat(64);
        let good = format!(
            r#"{{"v":1,"version":"0.2.0","tag":"v0.2.0","assets":{{"hufu-x.tar.gz":{{"sha256":"{sum}","size":5}}}},"extra":true}}"#
        );
        let manifest = Manifest::parse(&good).unwrap();
        assert_eq!((manifest.version.as_str(), manifest.tag.as_str()), ("0.2.0", "v0.2.0"));
        assert_eq!(manifest.assets["hufu-x.tar.gz"].sha256, sum);
        for bad in [
            "not json".to_string(),
            r#"{"version":"0.2.0"}"#.to_string(),
            r#"{"version":"newest","tag":"v0.2.0"}"#.to_string(),
            r#"{"version":"0.2.0","tag":"v0.2.0/../../x"}"#.to_string(),
            r#"{"version":"0.2.0","tag":""}"#.to_string(),
            r#"{"version":"0.2.0","tag":"v0.2.0","assets":{"a":{"sha256":"abc"}}}"#.to_string(),
            format!(r#"{{"version":"0.2.0","tag":"v0.2.0","assets":{{"a":{{"sha256":"{}"}}}}}}"#, "g".repeat(64)),
        ] {
            assert!(Manifest::parse(&bad).is_err(), "must be refused: {bad}");
        }
    }

    #[test]
    fn the_download_base_can_be_a_mirror_or_a_directory() {
        assert_eq!(Source::parse("https://mirror.example/hufu/"), Source::Mirror("https://mirror.example/hufu".into()));
        assert_eq!(Source::parse("/srv/hufu"), Source::Dir(PathBuf::from("/srv/hufu")));
        #[cfg(not(windows))]
        assert_eq!(Source::parse("file:///srv/hufu"), Source::Dir(PathBuf::from("/srv/hufu")));
        #[cfg(windows)]
        assert_eq!(Source::parse("file:///C:/srv/hufu"), Source::Dir(PathBuf::from("C:/srv/hufu")));
        let url = |loc: Location| loc.describe();
        assert_eq!(
            url(Source::GitHub.file(None, MANIFEST)),
            "https://github.com/kverona-ai/hufu/releases/latest/download/hufu-manifest.json"
        );
        assert_eq!(
            url(Source::GitHub.file(Some("v0.2.0"), "hufu-x.zip")),
            "https://github.com/kverona-ai/hufu/releases/download/v0.2.0/hufu-x.zip"
        );
        assert_eq!(url(Source::parse("http://m/h").file(Some("v0.2.0"), "hufu-x.zip")), "http://m/h/hufu-x.zip");
    }

    /// A directory that stands in for a release: the manifest and one "archive".
    fn release_dir(scratch: &Path, version: &str, body: &[u8], sum: Option<&str>) -> PathBuf {
        let dir = scratch.join("release");
        std::fs::create_dir_all(&dir).unwrap();
        let name = asset_name(release_target().unwrap());
        std::fs::write(dir.join(&name), body).unwrap();
        let sum = sum.map(String::from).unwrap_or_else(|| sha256_hex(body));
        let manifest = serde_json::json!({"v": 1, "version": version, "tag": format!("v{version}"), "assets": {name: {"sha256": sum}}});
        std::fs::write(dir.join(MANIFEST), manifest.to_string()).unwrap();
        dir
    }

    #[test]
    fn a_release_is_found_in_a_directory_and_a_missing_one_is_not_an_error() {
        let scratch = Scratch::new().unwrap();
        let empty = Source::Dir(scratch.path().join("nothing-here"));
        assert!(find_release(&empty, None, scratch.path()).unwrap().is_none());

        let source = Source::Dir(release_dir(scratch.path(), "0.9.0", b"x", None));
        let found = find_release(&source, None, scratch.path()).unwrap().expect("a release");
        assert_eq!((found.version.as_str(), found.tag.as_str()), ("0.9.0", "v0.9.0"));
        assert!(found.asset.is_some());
        assert!(find_release(&source, Some("v0.9.0"), scratch.path()).unwrap().is_some());
        assert!(find_release(&source, Some("0.9.0"), scratch.path()).unwrap().is_some());
        // A directory holds one release; asking it for another finds nothing.
        assert!(find_release(&source, Some("0.8.0"), scratch.path()).unwrap().is_none());
        assert!(find_release(&source, Some("not-a-version"), scratch.path()).is_err());
    }

    #[test]
    fn a_download_with_the_wrong_checksum_is_refused() {
        let scratch = Scratch::new().unwrap();
        let source =
            Source::Dir(release_dir(scratch.path(), "0.9.0", b"not what was published", Some(&"0".repeat(64))));
        let release = find_release(&source, None, scratch.path()).unwrap().unwrap();
        let err = download(&release, scratch.path()).unwrap_err();
        assert!(err.message.contains("checksum mismatch"), "{}", err.message);
    }

    #[test]
    fn installing_replaces_the_file_and_leaves_nothing_behind() {
        let scratch = Scratch::new().unwrap();
        let bin = scratch.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let dest = bin.join(format!("hufu{}", std::env::consts::EXE_SUFFIX));
        let new = scratch.path().join("new");
        std::fs::write(&dest, b"old").unwrap();
        std::fs::write(&new, b"new").unwrap();
        check_writable(&dest).unwrap();
        install(&new, &dest).unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"new");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&dest).unwrap().permissions().mode() & 0o777, 0o755);
        }
        assert!(sweep_old(&dest).is_empty());
        let names: Vec<String> =
            std::fs::read_dir(&bin).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(names.len(), 1, "only the program itself may be left: {names:?}");

        // A first install, with nothing to replace, works the same way.
        let fresh = bin.join(format!("other{}", std::env::consts::EXE_SUFFIX));
        install(&new, &fresh).unwrap();
        assert_eq!(std::fs::read(&fresh).unwrap(), b"new");

        // What an update moved aside is removed, and a helper that meets a file it was not told
        // about steps back at once instead of waiting.
        let aside = bin.join("hufu.old-abc.exe");
        std::fs::write(&aside, b"old").unwrap();
        std::fs::write(bin.join("hufu.older.exe"), b"not ours").unwrap();
        assert!(sweep_old(&dest).is_empty());
        assert!(!aside.exists() && bin.join("hufu.older.exe").exists());
        sweep_until_gone(&dest, &[]);

        let missing = scratch.path().join("no-such-dir").join("hufu");
        assert!(check_writable(&missing).is_err());
        assert!(install(&new, &missing).is_err());
    }
}
