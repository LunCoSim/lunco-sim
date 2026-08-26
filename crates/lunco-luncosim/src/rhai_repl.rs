//! `luncosim rhai` — a tiny stdin→HTTP rhai REPL / one-shot client for driving a
//! ALREADY-RUNNING luncosim over its `--api` port.
//!
//! This is a *client*, not the app: `luncosim rhai` does NOT open a window — it
//! connects to a luncosim that's already listening (started with `--api`) and
//! sends each snippet as a [`RunRhai`] command, which the running app compiles
//! against the full prelude and executes with live `World` access next tick. So
//! you can script the live sim from a shell:
//!
//! ```text
//! # interactive REPL
//! luncosim rhai --api 4101
//! rhai> restart_scene(); pause();          # reload the scene then freeze it
//! rhai> set_camera("OrbitView")
//!
//! # one-shot
//! luncosim rhai --api 4101 -e 'load_scene("scenes/luncosim/lander_cinematic.usda"); pause()'
//!
//! # pipe a whole script (sent as ONE snippet, so multi-line blocks work)
//! cat cutscene.rhai | luncosim rhai --api 4101
//!
//! # shell-harness output (stdout only, nonzero on transport/Rhai failure)
//! luncosim rhai --api 4101 --stdout -e 'print("TESTS_OK 1")'
//! ```
//!
//! Snippet output (`print`/`notify`) is captured in the HTTP reply. The default
//! mode prints the complete acknowledgement; `--stdout` prints only the captured
//! script output for shell harnesses. That's fine for the primary use:
//! sending ordered command sequences (the thing the one-fetch-at-a-time API made
//! awkward).

use std::io::{self, BufRead, IsTerminal, Read, Write};
use std::net::TcpStream;

#[derive(Clone, Copy, Debug, Default)]
enum OutputMode {
    #[default]
    Json,
    Stdout,
}

/// If the process was invoked as `luncosim rhai [...]`, run the REPL/one-shot
/// client and return its exit status. Returns `None` for a normal launch so
/// `main` falls through to the app.
pub fn run_if_requested() -> Option<i32> {
    let args: Vec<String> = std::env::args().collect();
    if !args.iter().skip(1).any(|a| a == "rhai") {
        return None;
    }

    let mut port = lunco_core::session::DEFAULT_API_PORT;
    let mut one_shot: Option<String> = None;
    let mut file: Option<String> = None;
    let mut output = OutputMode::Json;
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
            "--stdout" => output = OutputMode::Stdout,
            _ => {}
        }
        i += 1;
    }

    Some(run(port, one_shot, file, output))
}

fn run(port: u16, one_shot: Option<String>, file: Option<String>, output: OutputMode) -> i32 {
    if let Some(code) = one_shot {
        return submit(port, &code, output);
    }
    if let Some(path) = file {
        match std::fs::read_to_string(&path) {
            Ok(src) => return submit(port, &src, output),
            Err(e) => {
                eprintln!("rhai: cannot read {path}: {e}");
                return 2;
            }
        }
    }

    let stdin = io::stdin();
    if !stdin.is_terminal() {
        // Piped input: read the whole script and send it as ONE snippet so
        // multi-line blocks (`seq([...])`) stay intact.
        let mut src = String::new();
        if stdin.lock().read_to_string(&mut src).is_ok() && !src.trim().is_empty() {
            return submit(port, &src, output);
        }
        return 0;
    }

    // Interactive line REPL.
    eprintln!(
        "LunCo rhai REPL → 127.0.0.1:{port}  (prelude loaded — try `pause()`, \
         `restart_scene()`, `set_camera(\"OrbitView\")`.  Ctrl-D / :q to quit)"
    );
    let mut line = String::new();
    let mut status = 0;
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
        status = status.max(submit(port, code, output));
    }
    status
}

fn submit(port: u16, code: &str, output: OutputMode) -> i32 {
    match post(port, code) {
        Ok(body) => match output {
            OutputMode::Json => {
                println!("{body}");
                response_status(&body, false)
            }
            OutputMode::Stdout => response_status(&body, true),
        },
        Err(e) => {
            eprintln!("rhai: request failed (is a luncosim running with --api {port}?): {e}");
            2
        }
    }
}

/// Decode the one API response shape owned by `RunRhai`. Keeping this in the
/// native client means shell callers do not need another JSON/Rhai transport
/// implementation. `--stdout` prints exactly the captured script output.
fn response_status(body: &str, stdout_only: bool) -> i32 {
    let Ok(response) = serde_json::from_str::<serde_json::Value>(body) else {
        eprintln!("rhai: API returned invalid JSON: {body}");
        return 2;
    };
    if let Some(error) = response.get("error") {
        eprintln!("RHAI_ERROR: {error}");
        return 4;
    }
    let Some(stdout) = response
        .get("data")
        .and_then(|data| data.get("stdout"))
        .and_then(serde_json::Value::as_str)
    else {
        eprintln!("rhai: RunRhai returned no captured stdout: {response}");
        return 2;
    };
    if stdout_only && !stdout.is_empty() {
        print!("{stdout}");
        if !stdout.ends_with('\n') {
            println!();
        }
    }
    0
}

/// POST a `RunRhai` command carrying `code` to the running luncosim's HTTP API and
/// return the response body. Dependency-free raw HTTP over `TcpStream` — this is
/// a localhost dev tool, not a general HTTP client.
fn post(port: u16, code: &str) -> io::Result<String> {
    let body = format!(
        r#"{{"type":"ExecuteCommand","command":"RunRhai","params":{{"code":{}}}}}"#,
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
