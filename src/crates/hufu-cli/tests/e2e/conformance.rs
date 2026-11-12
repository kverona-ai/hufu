//! The adapter contract (C1 to C12) against every built-in adapter.

use hufu_contract::{AdapterRegistry, PermissionRequests, Profile};
use hufu_testing::conformance::{Subject, run_all};

use crate::common::{Env, HUFU};

fn check(flavor: &str, tune: fn(&mut Profile)) {
    let env = Env::new(&format!("conf-{flavor}"));
    let scratch = env.root.join("scratch");
    std::fs::create_dir_all(&scratch).unwrap();
    let registry = hufu_adapters::Registry::builtin();
    let kind = hufu_testing::adapter_of(flavor);
    let adapter = registry.get(kind).unwrap_or_else(|| panic!("no adapter `{kind}`"));
    let subject = Subject {
        adapter,
        fake: vec![HUFU.to_string(), "__fake-agent".into(), flavor.to_string()],
        workspace: env.work.clone(),
        scratch,
        tune,
    };
    let failed: Vec<String> =
        run_all(&subject).into_iter().filter_map(|(id, r)| r.err().map(|e| format!("{id}: {e}"))).collect();
    assert!(failed.is_empty(), "adapter `{kind}` breaks the contract:\n{}", failed.join("\n"));
}

fn plain(_: &mut Profile) {}

fn verified(p: &mut Profile) {
    p.permission_requests = PermissionRequests::Verified;
}

#[test]
fn codex_cli() {
    check("codex", plain);
}

#[test]
fn agy_cli() {
    check("agy", plain);
}

#[test]
fn cursor_cli() {
    check("cursor", plain);
}

#[test]
fn claude_cli() {
    check("claude", plain);
}

#[test]
fn kimi_cli() {
    check("kimi", plain);
}

#[test]
fn pi() {
    check("pi", plain);
}

#[test]
fn acp() {
    check("acp", verified);
}

#[test]
fn external() {
    check("external", plain);
}
