//! `sandbox rhai` — a tiny stdin→HTTP rhai REPL / one-shot client for driving a
//! ALREADY-RUNNING sandbox over its `--api` port.
//!
//! This is a *client*, not the app: `sandbox rhai` does NOT open a window — it
//! connects to a sandbox that's already listening (started with `--api`) and
//! sends each snippet as a [`RunRhai`] command, which the running app compiles
//! against the full prelude and executes with live `World` access next tick. So
//! you can script the live sim from a shell:
//!
//! ```text
//! # interactive REPL
//! sandbox rhai --api 4101
//! rhai> restart_scene(); pause();          # reload the scene then freeze it
//! rhai> set_camera("OrbitView")
//!
//! # one-shot
//! sandbox rhai -e 'load_scene("scenes/sandbox/lander_cinematic.usda"); pause()'
//!
//! # pipe a whole script (sent as ONE snippet, so multi-line blocks work)
//! cat cutscene.rhai | sandbox rhai
//! ```
//!
//! Snippet output (`print`/`notify`) surfaces in the running app's log / HUD, not
//! here — the HTTP reply is the command ack. That's fine for the primary use:
//! sending ordered command sequences (the thing the one-fetch-at-a-time API made
//! awkward).

use std::io::{self, BufRead, IsTerminal, Read, Write};
use std::net::TcpStream;

/// If the process was invoked as `sandbox rhai [...]`, run the REPL/one-shot
/// client and return `true` (the caller should exit WITHOUT launching the GUI).
/// Returns `false` for a normal launch so `main` falls through to the app.
pub fn run_if_requested() -> bool {
    let args: Vec<String> = std::env::args().collect();
    if !args.iter().skip(1).any(|a| a == "rhai") {
        return false;
    }

    let mut port = lunco_core::session::DEFAULT_API_PORT;
    let mut one_shot: Option<String> = None;
    let mut file: Option<String> = None;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--api" | "--port" => {
                if let Some(p) = args.get(i + 1).and_then(|s| s.parse().ok()) {
                    port = p;
                }
                i += 1;
            }
            "-e" | "--eval" => {
                one_shot = args.get(i + 1).cloned();
                i += 1;
            }
            "-f" | "--file" => {
                file = args.get(i + 1).cloned();
                i += 1;
            }
            _ => {}
        }
        i += 1;
    }

    run(port, one_shot, file);
    true
}

fn run(port: u16, one_shot: Option<String>, file: Option<String>) {
    if let Some(code) = one_shot {
        submit(port, &code);
        return;
    }
    if let Some(path) = file {
        match std::fs::read_to_string(&path) {
            Ok(src) => submit(port, &src),
            Err(e) => eprintln!("rhai: cannot read {path}: {e}"),
        }
        return;
    }

    let stdin = io::stdin();
    if !stdin.is_terminal() {
        // Piped input: read the whole script and send it as ONE snippet so
        // multi-line blocks (`seq([...])`) stay intact.
        let mut src = String::new();
        if stdin.lock().read_to_string(&mut src).is_ok() && !src.trim().is_empty() {
            submit(port, &src);
        }
        return;
    }

    // Interactive line REPL.
    eprintln!(
        "LunCo rhai REPL → 127.0.0.1:{port}  (prelude loaded — try `pause()`, \
         `restart_scene()`, `set_camera(\"OrbitView\")`.  Ctrl-D / :q to quit)"
    );
    let mut line = String::new();
    loop {
        eprint!("rhai> ");
        io::stderr().flush().ok();
        line.clear();
        match stdin.lock().read_line(&mut line) {
            Ok(0) => {
                eprintln!();
                break;
            }
            Ok(_) => {}
            Err(_) => break,
        }
        let code = line.trim();
        if code.is_empty() {
            continue;
        }
        if matches!(code, ":q" | "quit" | "exit") {
            break;
        }
        submit(port, code);
    }
}

fn submit(port: u16, code: &str) {
    match post(port, code) {
        Ok(body) => println!("{body}"),
        Err(e) => {
            eprintln!("rhai: request failed (is a sandbox running with --api {port}?): {e}");
        }
    }
}

/// POST a `RunRhai` command carrying `code` to the running sandbox's HTTP API and
/// return the response body. Dependency-free raw HTTP over `TcpStream` — this is
/// a localhost dev tool, not a general HTTP client.
fn post(port: u16, code: &str) -> io::Result<String> {
    let body = format!(
        r#"{{"command":"RunRhai","params":{{"code":{}}}}}"#,
        json_str(code)
    );
    let req = format!(
        "POST /api/commands HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\n\
         Content-Length: {len}\r\nConnection: close\r\n\r\n{body}",
        len = body.len(),
    );
    let mut stream = TcpStream::connect(("127.0.0.1", port))?;
    stream.write_all(req.as_bytes())?;
    let mut resp = String::new();
    stream.read_to_string(&mut resp)?;
    Ok(resp
        .split_once("\r\n\r\n")
        .map(|(_, b)| b)
        .unwrap_or(&resp)
        .trim()
        .to_string())
}

/// Minimal JSON string encoder (dependency-free) — enough to embed a rhai snippet
/// as a JSON string value.
fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
