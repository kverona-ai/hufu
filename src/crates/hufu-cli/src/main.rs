//! `hufu`: dispatch work to any coding agent, from the command line or over MCP.

mod cli;
mod commands;
mod render;

use clap::Parser;
use hufu_core::error::exit;

fn main() {
    let argv: Vec<String> = std::env::args().collect();
    // The fake agents take their arguments verbatim, exactly as a real agent would.
    if argv.get(1).map(String::as_str) == Some("__fake-agent") {
        std::process::exit(hufu_testing::fake_agent_main(&argv[2..]));
    }
    let args = match cli::Cli::try_parse() {
        Ok(args) => args,
        Err(e) => {
            let code = if e.use_stderr() { exit::USAGE } else { exit::OK };
            let _ = e.print();
            std::process::exit(code);
        }
    };
    let json = args.json;
    let code = match commands::run(args) {
        Ok(code) => code,
        Err(e) => {
            if json {
                println!("{}", serde_json::json!({"v": 1, "error": {"kind": e.kind_str(), "message": e.message}}));
            } else {
                eprintln!("hufu: {}", e.message);
            }
            e.exit_code()
        }
    };
    std::process::exit(code);
}
