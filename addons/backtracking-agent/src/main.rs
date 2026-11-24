//! Backtracking Agent: a Backtracking add-on that lets MCP clients (AI agents such as Claude
//! Code or Claude Desktop) read Backtracking's state, pause and start capture, select, save
//! files and change sources, through the add-on contract's `control` capability.
//!
//! One program, two modes:
//!
//! - Started by Backtracking (the package's `run`): connects as an add-on and serves MCP over
//!   Streamable HTTP on 127.0.0.1, with a bearer token, writing the address and token to
//!   `endpoint.json` in its data folder.
//! - `--stdio [--endpoint <file>]`: what an MCP client starts. Speaks MCP on stdin and stdout
//!   and passes every tool call to the running add-on.

mod addon;
mod bridge;
mod compact;
mod endpoint;
mod http;
mod platform;
mod stdio;
mod tools;

use std::path::PathBuf;

/// The add-on's id, as in `package/addon.json`.
pub const ID: &str = "com.kamelkyp.backtracking.agent";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

const USAGE: &str = "Backtracking Agent lets MCP clients control Backtracking.

Backtracking starts this program itself once the add-on is installed and enabled.
MCP clients start it like this:

    backtracking-agent --stdio [--endpoint <endpoint.json>]

--endpoint, or the BACKTRACKING_AGENT_ENDPOINT environment variable, names the connection
file of a Backtracking other than the one this program was installed into.";

fn main() {
    let mut args = std::env::args_os().skip(1);
    let mut stdio = false;
    let mut endpoint = None;
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--stdio") => stdio = true,
            Some("--endpoint") => match args.next() {
                Some(path) => endpoint = Some(PathBuf::from(path)),
                None => fail("--endpoint needs a file"),
            },
            Some("--help" | "-h") => {
                println!("{USAGE}");
                return;
            }
            Some("--version") => {
                println!("backtracking-agent {VERSION}");
                return;
            }
            _ => fail(&format!("Unknown argument {arg:?}\n\n{USAGE}")),
        }
    }
    let result = if stdio {
        let program = std::env::current_exe().unwrap_or_default();
        stdio::run(endpoint::locate(endpoint, &program))
    } else if std::env::var_os("BACKTRACKING_ADDON_PORT").is_some() {
        addon::run()
    } else {
        fail(USAGE);
    };
    if let Err(error) = result {
        fail(&format!("Backtracking Agent: {error}"));
    }
}

fn fail(text: &str) -> ! {
    eprintln!("{text}");
    std::process::exit(2);
}
