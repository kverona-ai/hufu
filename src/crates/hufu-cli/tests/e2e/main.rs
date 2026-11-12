//! End-to-end tests: the real `hufu` binary, real detached runners, and fake agents that speak
//! each supported agent's output format. No network and no real agent is involved.

mod common;

mod computer_use;
mod configure;
mod conformance;
mod daemon;
mod jobs;
mod mcp;
mod missions;
