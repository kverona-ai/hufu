//! Workspaces: finding what a job changed, and running it in a fresh git worktree.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use hufu_contract::proc::{self, Captured};

use crate::config::Home;
use crate::error::{HufuError, Result};

/// Runs git in `cwd`. `None` when git itself cannot be started.
pub fn git(cwd: &Path, args: &[&str]) -> Option<Captured> {
    let mut cmd = Command::new("git");
    cmd.arg("-c").arg("core.quotepath=off").args(args).current_dir(cwd);
    cmd.env("GIT_TERMINAL_PROMPT", "0").env("GIT_OPTIONAL_LOCKS", "0");
    proc::run_capture(cmd, Duration::from_secs(120)).ok()
}

fn git_ok(cwd: &Path, args: &[&str]) -> Option<String> {
    git(cwd, args).filter(Captured::ok).map(|c| c.stdout.trim_end().to_string())
}

/// The top of the work tree that contains `cwd`, if it is in one.
pub fn repo_root(cwd: &Path) -> Option<PathBuf> {
    git_ok(cwd, &["rev-parse", "--show-toplevel"]).filter(|s| !s.is_empty()).map(PathBuf::from)
}

pub fn head(cwd: &Path) -> Option<String> {
    git_ok(cwd, &["rev-parse", "HEAD"]).filter(|s| !s.is_empty())
}

/// The state of a work tree at one moment: its commit and a fingerprint of every dirty file.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    pub root: PathBuf,
    pub head: Option<String>,
    pub dirty: BTreeMap<String, String>,
}

/// The most files a directory outside git may hold for Hufu to fingerprint it.
const PLAIN_LIMIT: usize = 20_000;

fn stamp_of(path: &Path) -> String {
    match std::fs::metadata(path) {
        Ok(m) => {
            let modified = m
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            format!("{}:{modified}", m.len())
        }
        Err(_) => "gone".into(),
    }
}

fn slashes(text: &str) -> String {
    if cfg!(windows) { text.replace('\\', "/") } else { text.to_string() }
}

/// A path as the agent named it, relative to the workspace when it lies inside, with `/`.
pub fn relative_to(root: &Path, path: &str) -> String {
    let p = Path::new(path);
    let inside = p.strip_prefix(root).map(Path::to_path_buf).ok().or_else(|| {
        let canonical = root.canonicalize().ok()?;
        p.strip_prefix(&canonical).map(Path::to_path_buf).ok()
    });
    match inside {
        Some(rel) => slashes(&rel.to_string_lossy()),
        None => slashes(path),
    }
}

/// Fingerprints every file of a directory that is not under git. `None` when it is too large.
fn snapshot_plain(cwd: &Path) -> Option<Snapshot> {
    let mut dirty = BTreeMap::new();
    let mut pending = vec![cwd.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).ok()?.flatten() {
            let path = entry.path();
            let kind = entry.file_type().ok()?;
            if kind.is_dir() {
                pending.push(path);
            } else {
                if dirty.len() >= PLAIN_LIMIT {
                    return None;
                }
                dirty.insert(slashes(&path.strip_prefix(cwd).unwrap_or(&path).to_string_lossy()), stamp_of(&path));
            }
        }
    }
    Some(Snapshot { root: cwd.to_path_buf(), head: None, dirty })
}

/// Takes a snapshot: through git inside a work tree, by fingerprinting the files elsewhere.
/// `None` when neither is possible (a very large directory outside git).
pub fn snapshot(cwd: &Path) -> Option<Snapshot> {
    let Some(root) = repo_root(cwd) else {
        return snapshot_plain(cwd);
    };
    let status = git(&root, &["status", "--porcelain=v1", "-z", "--untracked-files=all"]).filter(Captured::ok)?;
    let mut dirty = BTreeMap::new();
    let mut entries = status.stdout.split('\0');
    while let Some(entry) = entries.next() {
        if entry.len() < 4 {
            continue;
        }
        let (code, path) = entry.split_at(3);
        if code.starts_with('R') || code.starts_with('C') {
            entries.next(); // the original path of a rename or copy
        }
        let stamp = stamp_of(&root.join(path));
        dirty.insert(path.to_string(), format!("{}|{stamp}", code.trim()));
    }
    Some(Snapshot { head: head(&root), root, dirty })
}

/// The files that differ between two snapshots of the same work tree, relative to its root.
pub fn changed(before: &Snapshot, after: &Snapshot) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (path, mark) in &after.dirty {
        if before.dirty.get(path) != Some(mark) {
            out.push(path.clone());
        }
    }
    for path in before.dirty.keys() {
        if !after.dirty.contains_key(path) {
            out.push(path.clone());
        }
    }
    if let (Some(a), Some(b)) = (&before.head, &after.head)
        && a != b
        && let Some(names) = git_ok(&after.root, &["diff", "--name-only", a, b])
    {
        out.extend(names.lines().filter(|l| !l.is_empty()).map(str::to_string));
    }
    out.sort();
    out.dedup();
    out
}

/// A worktree made for one job.
#[derive(Clone, Debug, PartialEq)]
pub struct Worktree {
    pub repo: PathBuf,
    /// The worktree's root.
    pub path: PathBuf,
    /// Where the agent runs: the same sub-directory the task named, inside the worktree.
    pub run_dir: PathBuf,
    pub branch: String,
    pub base: String,
}

/// Creates `hufu/<job>` at the current commit, checked out in a worktree under `$HUFU_HOME`.
pub fn create_worktree(home: &Home, cwd: &Path, job: &str) -> Result<Worktree> {
    let repo = repo_root(cwd).ok_or_else(|| {
        HufuError::usage(format!("{} is not inside a git repository, which `isolation: worktree` needs", cwd.display()))
    })?;
    let base = head(&repo).ok_or_else(|| {
        HufuError::usage("the repository has no commit yet; `isolation: worktree` needs one to branch from")
    })?;
    let path = home.worktrees().join(job);
    std::fs::create_dir_all(home.worktrees()).map_err(|e| HufuError::internal(e.to_string()))?;
    let branch = format!("hufu/{job}");
    let target = path.display().to_string();
    let out = git(&repo, &["worktree", "add", "-b", &branch, &target, &base])
        .ok_or_else(|| HufuError::internal("git could not be started"))?;
    if !out.ok() {
        return Err(HufuError::internal(format!("git worktree add failed: {}", proc::tail(&out.stderr, 500))));
    }
    let real_cwd = std::fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf());
    let real_repo = std::fs::canonicalize(&repo).unwrap_or_else(|_| repo.clone());
    let run_dir = match real_cwd.strip_prefix(&real_repo) {
        Ok(rel) if !rel.as_os_str().is_empty() => path.join(rel),
        _ => path.clone(),
    };
    Ok(Worktree { repo, path, run_dir, branch, base })
}

/// Commits whatever the agent left uncommitted, so that the branch holds the whole result.
/// Returns the new commit, or `None` when there was nothing to commit.
pub fn commit_all(worktree: &Path, message: &str) -> Option<String> {
    let status = git_ok(worktree, &["status", "--porcelain"])?;
    if status.trim().is_empty() {
        return None;
    }
    git(worktree, &["add", "-A"])?;
    let has_identity = git_ok(worktree, &["config", "user.email"]).map(|e| !e.is_empty()).unwrap_or(false);
    let mut args: Vec<&str> = Vec::new();
    if !has_identity {
        args.extend(["-c", "user.name=hufu", "-c", "user.email=hufu@localhost"]);
    }
    args.extend(["commit", "--no-verify", "-q", "-m", message]);
    git(worktree, &args).filter(Captured::ok)?;
    head(worktree)
}

/// Files changed on a worktree branch since its base commit, including uncommitted ones.
pub fn changed_since(worktree: &Path, base: &str) -> Vec<String> {
    let mut out: Vec<String> = git_ok(worktree, &["diff", "--name-only", base])
        .map(|s| s.lines().filter(|l| !l.is_empty()).map(str::to_string).collect())
        .unwrap_or_default();
    if let Some(untracked) = git_ok(worktree, &["ls-files", "--others", "--exclude-standard"]) {
        out.extend(untracked.lines().filter(|l| !l.is_empty()).map(str::to_string));
    }
    out.sort();
    out.dedup();
    out
}

/// Removes a job's worktree. Its branch is kept.
pub fn remove_worktree(repo: &Path, path: &Path) -> bool {
    let target = path.display().to_string();
    let removed = git(repo, &["worktree", "remove", "--force", &target]).map(|c| c.ok()).unwrap_or(false);
    if !removed && path.exists() {
        let _ = std::fs::remove_dir_all(path);
        let _ = git(repo, &["worktree", "prune"]);
    }
    !path.exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> Option<PathBuf> {
        let dir = std::env::temp_dir().join(format!("hufu-ws-{}", hufu_contract::time::new_id("t")));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        git(&dir, &["init", "-q"])?;
        std::fs::write(dir.join("src/a.txt"), "one\n").unwrap();
        std::fs::write(dir.join("dirty.txt"), "already dirty\n").unwrap();
        git(&dir, &["add", "src/a.txt"])?;
        let done =
            git(&dir, &["-c", "user.name=t", "-c", "user.email=t@example.invalid", "commit", "-q", "-m", "init"])?;
        done.ok().then_some(dir)
    }

    #[test]
    fn snapshots_report_only_what_changed_in_between() {
        let Some(dir) = repo() else { return };
        let before = snapshot(&dir).unwrap();
        assert!(before.dirty.contains_key("dirty.txt"));
        std::fs::write(dir.join("src/a.txt"), "two\n").unwrap();
        std::fs::write(dir.join("new file.txt"), "x").unwrap();
        let after = snapshot(&dir).unwrap();
        assert_eq!(
            changed(&before, &after),
            ["new file.txt", "src/a.txt"],
            "a file that was already dirty and untouched is not reported"
        );
        assert!(changed(&after, &after).is_empty());
        assert!(snapshot(&std::env::temp_dir().join("hufu-definitely-not-a-repo")).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn worktrees_branch_from_head_and_collect_the_work() {
        let Some(dir) = repo() else { return };
        let home = Home::at(dir.join(".home"));
        let wt = create_worktree(&home, &dir.join("src"), "j_test1").unwrap();
        assert_eq!(wt.branch, "hufu/j_test1");
        assert!(wt.run_dir.ends_with("src") && wt.run_dir.join("a.txt").exists());
        assert!(!wt.path.join("dirty.txt").exists(), "uncommitted files of the main tree do not leak in");
        std::fs::write(wt.run_dir.join("b.txt"), "new\n").unwrap();
        assert_eq!(changed_since(&wt.path, &wt.base), ["src/b.txt"]);
        assert!(commit_all(&wt.path, "hufu: test").is_some());
        assert!(commit_all(&wt.path, "hufu: nothing").is_none());
        assert_eq!(changed_since(&wt.path, &wt.base), ["src/b.txt"]);
        assert!(remove_worktree(&wt.repo, &wt.path));
        assert!(git_ok(&dir, &["rev-parse", "--verify", "hufu/j_test1"]).is_some(), "the branch survives");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
