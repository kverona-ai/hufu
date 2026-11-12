//! Agent adapters for Hufu.
//!
//! Every adapter implements [`hufu_contract::Adapter`] and nothing else: this crate does not
//! depend on `hufu-core`. Headless CLI adapters share the helpers in [`common`]; ACP agents are
//! all driven by the one generic client in [`acp`].

pub mod acp;
pub mod agy_cli;
pub mod claude_cli;
pub mod codex_cli;
pub mod common;
pub mod cursor_cli;
pub mod external;
pub mod kimi_cli;
pub mod pi;

use hufu_contract::{Adapter, AdapterRegistry};

/// All adapters built into Hufu.
pub struct Registry {
    adapters: Vec<Box<dyn Adapter>>,
}

impl Registry {
    pub fn builtin() -> Registry {
        Registry {
            adapters: vec![
                Box::new(codex_cli::CodexCli),
                Box::new(agy_cli::AgyCli),
                Box::new(cursor_cli::CursorCli),
                Box::new(kimi_cli::KimiCli),
                Box::new(pi::Pi),
                Box::new(claude_cli::ClaudeCli),
                Box::new(acp::Acp),
                Box::new(external::External),
            ],
        }
    }
}

impl AdapterRegistry for Registry {
    fn get(&self, kind: &str) -> Option<&dyn Adapter> {
        self.adapters.iter().find(|a| a.kind() == kind).map(|a| a.as_ref())
    }

    fn kinds(&self) -> Vec<&'static str> {
        self.adapters.iter().map(|a| a.kind()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_documented_adapter_kind_is_registered() {
        let r = Registry::builtin();
        for kind in ["acp", "codex-cli", "agy-cli", "cursor-cli", "kimi-cli", "pi", "claude-cli", "external"] {
            assert!(r.get(kind).is_some(), "missing adapter {kind}");
        }
        assert!(r.get("nope").is_none());
    }
}
