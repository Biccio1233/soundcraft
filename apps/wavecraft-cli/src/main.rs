//! `wavecraft-cli`: WaveCraft without a window.
//!
//! ```text
//! wavecraft-cli run [--in FILE] [--cmd ID[=JSON]]… [--inspect]   commands on a file (`--in` opens it first)
//! wavecraft-cli script FILE [--in FILE] [--keep-going] [--inspect]   one `ID JSON` per line, `#` comments
//! wavecraft-cli commands [FILTER]                  list WaveCraft's commands
//! wavecraft-cli describe ID                        one command's details
//! wavecraft-cli plugins                            list the built-in effects for the rack
//! wavecraft-cli app [--port P] METHOD [JSON]        call a running wavecraft's control channel
//! wavecraft-cli mcp [--connect PORT] [--automation-read-root DIR] [--automation-write-root DIR]
//!                                                  MCP server on stdio. The two roots confine every file the
//!                                                  agent's commands read (open, presets) and write (export,
//!                                                  save) to those folders, links followed; a root left out
//!                                                  grants none of its access. They run the in-process
//!                                                  server, so they conflict with --connect.
//! ```
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod automation_args;

use serde_json::{Value, json};
use soundcraft_automation::mcp::{Server, WAVECRAFT};
use soundcraft_automation::{Backend, Headless, Remote};
use soundcraft_engine::Engine;
use soundcraft_engine::file_access::{self, AutomationRoots};
use std::io::Write;
use std::process::ExitCode;

const PRODUCT: &str = "WaveCraft";

/// Print without panicking on a closed pipe.
macro_rules! outln {
    ($($t:tt)*) => {{
        let mut o = std::io::stdout().lock();
        let _ = writeln!(o, $($t)*);
    }};
}

fn arg_value(args: &[String], flag: &str) -> Option<String> {
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned()
}

fn fail(msg: impl std::fmt::Display) -> ExitCode {
    eprintln!("wavecraft-cli: {msg}");
    ExitCode::FAILURE
}

/// The usage block of the module doc comment, up to its closing code fence.
fn help_text() -> String {
    include_str!("main.rs")
        .lines()
        .skip(3)
        .take_while(|l| l.trim_end() != "//! ```")
        .map(|l| l.trim_start_matches("//! "))
        .collect::<Vec<_>>()
        .join("\n")
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = args.first().cloned() else {
        eprintln!("usage: wavecraft-cli <run|script|commands|describe|plugins|app|mcp> …  (see --help)");
        return ExitCode::FAILURE;
    };
    let rest: Vec<String> = args.iter().skip(1).cloned().collect();
    let code = match cmd.as_str() {
        "--version" | "-V" | "version" => {
            outln!("WaveCraft CLI {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        "--help" | "-h" | "help" => {
            outln!("{}", help_text());
            ExitCode::SUCCESS
        }
        "run" => run(&rest),
        "script" => script(&rest),
        "commands" => commands(&rest),
        "describe" => describe(&rest),
        "plugins" => {
            for p in soundcraft_dsp::plugins().iter().filter(|p| !p.is_instrument) {
                outln!("{:<20} {:<22} {:?}", p.id, p.name, p.category);
            }
            ExitCode::SUCCESS
        }
        "app" => app(&rest),
        "mcp" => mcp(&rest),
        other => fail(format!("unknown subcommand `{other}`")),
    };
    soundcraft_engine::shutdown_plugin_hosts();
    code
}

/// An engine, with `--in FILE` opened when given.
fn load_engine(args: &[String]) -> Result<Engine, String> {
    let mut e = Engine::default();
    if let Some(path) = arg_value(args, "--in") {
        e.execute("wave.open", &json!({"path": path})).map_err(|err| format!("wave.open: {err}"))?;
    }
    Ok(e)
}

/// `--inspect` prints `wave.info` at the end.
fn finish(e: &mut Engine, args: &[String]) -> ExitCode {
    if args.iter().any(|a| a == "--inspect") {
        match e.execute("wave.info", &json!({})) {
            Ok(v) => outln!("{}", serde_json::to_string_pretty(&v).unwrap_or_default()),
            Err(err) => return fail(format!("wave.info: {err}")),
        }
    }
    ExitCode::SUCCESS
}

/// `ID` or `ID=JSON` → command id and params; malformed JSON is an error, not `{}`.
fn parse_cmd_spec(spec: &str) -> Result<(String, Value), String> {
    match spec.split_once('=') {
        Some((id, p)) => serde_json::from_str::<Value>(p).map(|v| (id.to_string(), v)).map_err(|err| format!("{id}: bad JSON: {err}")),
        None => Ok((spec.to_string(), json!({}))),
    }
}

fn run(args: &[String]) -> ExitCode {
    let mut e = match load_engine(args) {
        Ok(e) => e,
        Err(err) => return fail(err),
    };
    let mut i = 0;
    while i < args.len() {
        if args.get(i).is_some_and(|a| a == "--cmd")
            && let Some(spec) = args.get(i + 1)
        {
            let (id, params) = match parse_cmd_spec(spec) {
                Ok(v) => v,
                Err(err) => return fail(err),
            };
            match e.execute(&id, &params) {
                Ok(v) => outln!("{id}: {v}"),
                Err(err) => return fail(format!("{id}: {err}")),
            }
            i += 1;
        }
        i += 1;
    }
    finish(&mut e, args)
}

fn script(args: &[String]) -> ExitCode {
    let Some(path) = args.first() else { return fail("script FILE") };
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => return fail(format!("{path}: {e}")),
    };
    let mut e = match load_engine(args) {
        Ok(e) => e,
        Err(err) => return fail(err),
    };
    let keep = args.iter().any(|a| a == "--keep-going");
    for (n, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (id, p) = line.split_once(char::is_whitespace).unwrap_or((line, "{}"));
        let params = match serde_json::from_str::<Value>(p.trim()) {
            Ok(v) => v,
            Err(err) => return fail(format!("{path}:{}: bad JSON: {err}", n + 1)),
        };
        match e.execute(id, &params) {
            Ok(v) => outln!("{id}: {v}"),
            Err(err) => {
                eprintln!("{path}:{}: {id}: {err}", n + 1);
                if !keep {
                    return ExitCode::FAILURE;
                }
            }
        }
    }
    finish(&mut e, args)
}

/// The commands that make sense in WaveCraft: its own, the transport, undo and redo.
fn is_wavecraft_command(id: &str) -> bool {
    id.starts_with("wave.") || id.starts_with("transport.") || matches!(id, "edit.undo" | "edit.redo")
}

fn commands(args: &[String]) -> ExitCode {
    let filter = args.first().cloned().unwrap_or_default().to_ascii_lowercase();
    let e = Engine::default();
    for c in soundcraft_engine::command_specs().iter().filter(|c| is_wavecraft_command(c.id)) {
        if !filter.is_empty() && !c.id.contains(&filter) && !c.label.to_ascii_lowercase().contains(&filter) {
            continue;
        }
        let i = c.info(&e);
        outln!("{:<28} {:<26} {}", i.id, i.label, i.params);
    }
    ExitCode::SUCCESS
}

fn describe(args: &[String]) -> ExitCode {
    let Some(id) = args.first() else { return fail("describe ID") };
    match soundcraft_engine::find_command(id) {
        Some(c) => {
            outln!("{}", serde_json::to_string_pretty(&c.info(&Engine::default())).unwrap_or_default());
            ExitCode::SUCCESS
        }
        None => fail(format!("unknown command `{id}`")),
    }
}

/// Control-channel methods of the wavecraft app. Every other dotted id is an engine command.
fn is_control_method(method: &str) -> bool {
    matches!(
        method,
        "engine.execute"
            | "command"
            | "engine.commands"
            | "session.inspect"
            | "document.inspect"
            | "ui.inspect"
            | "ui.click"
            | "ui.drag"
            | "ui.key"
            | "ui.text"
            | "ui.screenshot"
            | "app.quit"
    )
}

fn app(args: &[String]) -> ExitCode {
    let Some(port) = arg_value(args, "--port").or_else(|| std::env::var("WAVECRAFT_CONTROL_PORT").ok()) else {
        return fail("app needs --port P (the port wavecraft printed) or WAVECRAFT_CONTROL_PORT");
    };
    let positional: Vec<&String> = args
        .iter()
        .enumerate()
        .filter(|(i, a)| !(a.starts_with("--") || (*i > 0 && args.get(i - 1).is_some_and(|p| p == "--port"))))
        .map(|(_, a)| a)
        .collect();
    let Some(method) = positional.first() else { return fail("app [--port P] METHOD [JSON]") };
    let params = match positional.get(1) {
        Some(p) => match serde_json::from_str::<Value>(p) {
            Ok(v) => v,
            Err(err) => return fail(format!("{method}: bad JSON: {err}")),
        },
        None => json!({}),
    };
    let (m, p) = if is_control_method(method) || !method.contains('.') {
        (method.to_string(), params)
    } else {
        ("engine.execute".to_string(), json!({"command": method, "params": params}))
    };
    let mut r = Remote::new(&port).for_product(PRODUCT);
    match r.call(&m, p) {
        Ok(v) => {
            outln!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
            ExitCode::SUCCESS
        }
        Err(e) => fail(e),
    }
}

/// What `mcp` was asked for.
struct McpArgs {
    connect: Option<String>,
    roots: Option<std::sync::Arc<AutomationRoots>>,
}

/// Parse the `mcp` options up front: a malformed or misspelled root flag must not leave the
/// server running unconfined.
fn parse_mcp_args(args: &[String]) -> Result<McpArgs, String> {
    let mut connect = None;
    let mut roots_args = automation_args::AutomationArgs::new(None, None);
    let mut it = args.iter().cloned();
    while let Some(a) = it.next() {
        if roots_args.consume(&a, &mut it)? {
            continue;
        }
        match a.as_str() {
            "--connect" => connect = Some(it.next().ok_or("--connect needs a port or host:port")?),
            other if other.starts_with("--automation-") => return Err(format!("unknown mcp option `{other}`")),
            _ => {}
        }
    }
    let roots = AutomationRoots::new(roots_args.read_root.as_deref(), roots_args.write_root.as_deref())?;
    if roots.is_some() && connect.is_some() {
        return Err("--automation-read-root and --automation-write-root confine the in-process server; a running app is not confined by them, \
so drop --connect (or use --connect without the roots)"
            .into());
    }
    Ok(McpArgs { connect, roots })
}

fn mcp(args: &[String]) -> ExitCode {
    let McpArgs { connect, roots } = match parse_mcp_args(args) {
        Ok(a) => a,
        Err(err) => return fail(err),
    };
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let r = if let Some(port) = connect {
        Server::with_profile(Remote::new(&port).for_product(PRODUCT), WAVECRAFT).serve(stdin.lock(), stdout.lock())
    } else {
        let mut h = Headless::new(Engine::default()).for_product(PRODUCT);
        if let Some(roots) = roots {
            // Every thread of this process, not only the server's: nothing reaches past the roots.
            file_access::confine_process(roots.clone());
            let show = |root: Option<&std::path::Path>| root.map_or_else(|| "none".to_string(), |r| r.display().to_string());
            eprintln!("wavecraft-cli: files confined: read root {}, write root {}", show(roots.read_root()), show(roots.write_root()));
            h = h.with_automation_roots(Some(roots));
        }
        Server::with_profile(h, WAVECRAFT).serve(stdin.lock(), stdout.lock())
    };
    match r {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => fail(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_text_first_and_last_lines() {
        let help = help_text();
        assert!(help.starts_with("wavecraft-cli run [--in FILE]"), "{help}");
        assert!(help.trim_end().ends_with("server, so they conflict with --connect."), "{help}");
    }

    #[test]
    fn only_wavecraft_commands_are_listed() {
        assert!(is_wavecraft_command("wave.open") && is_wavecraft_command("edit.undo"));
        assert!(!is_wavecraft_command("track.new") && !is_wavecraft_command("mix.insert"));
    }
}
