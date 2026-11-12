//! Concurrency slots: one small file per running job. They enforce the global limit, the
//! per-profile limits and "one computer-use job per display", with or without the daemon.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use hufu_contract::proc;
use serde::{Deserialize, Serialize};

use crate::config::Home;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SlotInfo {
    pub job: String,
    pub pid: u32,
    pub profile: String,
    /// Counts against the global job limit. Child jobs do not: they are part of their parent.
    #[serde(default = "yes")]
    pub counted: bool,
    /// Holds the display for computer use.
    #[serde(default)]
    pub display: bool,
}

fn yes() -> bool {
    true
}

/// What a job needs before it may start.
#[derive(Clone, Debug)]
pub struct Want<'a> {
    pub job: &'a str,
    pub profile: &'a str,
    pub counted: bool,
    pub display: bool,
    pub max_jobs: usize,
    pub max_for_profile: Option<usize>,
    pub max_display: usize,
}

/// A held slot. Dropping it frees the slot.
#[derive(Debug)]
pub struct Slot {
    path: PathBuf,
}

impl Drop for Slot {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

struct DirLock(PathBuf);

impl Drop for DirLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn lock(dir: &Path) -> Option<DirLock> {
    let path = dir.join(".lock");
    for _ in 0..200 {
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(_) => return Some(DirLock(path)),
            Err(_) => {
                // A lock is held for microseconds. One that is seconds old belongs to a dead process.
                let stale = std::fs::metadata(&path)
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| SystemTime::now().duration_since(t).ok())
                    .map(|age| age > Duration::from_secs(5))
                    .unwrap_or(false);
                if stale {
                    let _ = std::fs::remove_file(&path);
                } else {
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
        }
    }
    None
}

/// The slots of live runners. Files left behind by dead processes are removed on the way.
pub fn live(home: &Home) -> Vec<SlotInfo> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(home.slots()).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.extension().map(|e| e != "slot").unwrap_or(true) {
            continue;
        }
        match std::fs::read(&path).ok().and_then(|b| serde_json::from_slice::<SlotInfo>(&b).ok()) {
            Some(info) if proc::pid_alive(info.pid) => out.push(info),
            Some(_) => {
                let _ = std::fs::remove_file(&path);
            }
            None => {}
        }
    }
    out
}

/// Whether the limits leave room for this job right now.
pub fn has_room(slots: &[SlotInfo], want: &Want) -> bool {
    if want.counted && slots.iter().filter(|s| s.counted).count() >= want.max_jobs {
        return false;
    }
    if let Some(max) = want.max_for_profile {
        if slots.iter().filter(|s| s.profile == want.profile).count() >= max {
            return false;
        }
    }
    if want.display && slots.iter().filter(|s| s.display).count() >= want.max_display {
        return false;
    }
    true
}

/// Takes a slot if the limits allow it. Returns `None` when the job has to keep waiting.
pub fn try_acquire(home: &Home, want: &Want) -> Option<Slot> {
    let dir = home.slots();
    std::fs::create_dir_all(&dir).ok()?;
    let _guard = lock(&dir)?;
    let slots: Vec<SlotInfo> = live(home).into_iter().filter(|s| s.job != want.job).collect();
    if !has_room(&slots, want) {
        return None;
    }
    let info = SlotInfo {
        job: want.job.to_string(),
        pid: std::process::id(),
        profile: want.profile.to_string(),
        counted: want.counted,
        display: want.display,
    };
    let path = dir.join(format!("{}.slot", want.job));
    std::fs::write(&path, serde_json::to_vec(&info).ok()?).ok()?;
    Some(Slot { path })
}

/// How many live jobs each profile is running, for least-busy pool selection.
pub fn load_of(home: &Home, profile: &str) -> usize {
    live(home).iter().filter(|s| s.profile == profile).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn want<'a>(job: &'a str, profile: &'a str) -> Want<'a> {
        Want { job, profile, counted: true, display: false, max_jobs: 2, max_for_profile: None, max_display: 1 }
    }

    #[test]
    fn limits_are_enforced_and_released_on_drop() {
        let home = Home::at(std::env::temp_dir().join(format!("hufu-slots-{}", hufu_contract::time::new_id("t"))));
        let a = try_acquire(&home, &want("j_a", "p")).expect("first slot");
        let _b = try_acquire(&home, &want("j_b", "p")).expect("second slot");
        assert!(try_acquire(&home, &want("j_c", "p")).is_none(), "global limit of 2");
        let child = Want { counted: false, ..want("j_child", "p") };
        let _c = try_acquire(&home, &child).expect("child jobs do not count against the global limit");
        drop(a);
        let _d = try_acquire(&home, &want("j_d", "p")).expect("a freed slot can be taken");
        assert_eq!(load_of(&home, "p"), 3);
        let _ = std::fs::remove_dir_all(&home.root);
    }

    #[test]
    fn per_profile_and_display_limits() {
        let home = Home::at(std::env::temp_dir().join(format!("hufu-slots-{}", hufu_contract::time::new_id("t"))));
        let limited = |job| Want { max_jobs: 10, max_for_profile: Some(1), ..want(job, "codex") };
        let _a = try_acquire(&home, &limited("j_a")).unwrap();
        assert!(try_acquire(&home, &limited("j_b")).is_none());
        let screen = |job| Want { max_jobs: 10, display: true, ..want(job, "operator") };
        let _s = try_acquire(&home, &screen("j_s")).unwrap();
        assert!(try_acquire(&home, &screen("j_t")).is_none(), "one computer-use job per display");
        let _ = std::fs::remove_dir_all(&home.root);
    }

    #[test]
    fn slots_of_dead_processes_are_reclaimed() {
        let home = Home::at(std::env::temp_dir().join(format!("hufu-slots-{}", hufu_contract::time::new_id("t"))));
        std::fs::create_dir_all(home.slots()).unwrap();
        let dead = SlotInfo { job: "j_dead".into(), pid: 0, profile: "p".into(), counted: true, display: false };
        std::fs::write(home.slots().join("j_dead.slot"), serde_json::to_vec(&dead).unwrap()).unwrap();
        assert!(live(&home).is_empty());
        assert!(!home.slots().join("j_dead.slot").exists());
        let _ = std::fs::remove_dir_all(&home.root);
    }
}
