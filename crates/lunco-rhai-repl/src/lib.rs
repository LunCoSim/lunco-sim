//! Command-line frontend for the production `RunRhai` API command.
//!
//! This package does not evaluate Rhai and does not implement HTTP. The live
//! application owns evaluation through `lunco-scripting`; the generic
//! `lunco-api-client` owns endpoint and response transport. This package only
//! maps terminal input to the reflected `RunRhai` command and presents its
//! captured output.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use lunco_api_client::{ApiClient, ApiEndpoint};
use lunco_api_contracts::{ApiRequestEnvelope, ApiResponseEnvelope, DEFAULT_API_PORT};
use std::io::{self, BufRead, IsTerminal, Read, Write};
use std::path::Path;

#[derive(Clone, Copy, Debug, Default)]
enum OutputMode {
    #[default]
    Json,
    Stdout,
}

/// If the process was invoked as `luncosim rhai [...]`, run the client and
/// return its process exit status. Returns `None` for a normal application
/// launch so the caller can continue with its app entry point.
pub fn run_if_requested() -> Option<i32> {
    let args: Vec<String> = std::env::args().collect();
    if !args.iter().skip(1).any(|arg| arg == "rhai") {
        return None;
    }

    let mut endpoint = ApiEndpoint::loopback(DEFAULT_API_PORT);
    let mut one_shot = None;
    let mut file = None;
    let mut output = OutputMode::Json;
    let mut index = 1;
    while index < args.len() {
        match args[index].as_str() {
            "--api" | "--port" => {
                if let Some(value) = args.get(index + 1).filter(|value| !value.starts_with('-')) {
                    let port = match value.parse::<u16>() {
                        Ok(port) => port,
                        Err(error) => {
                            eprintln!("rhai: invalid API port '{value}': {error}");
                            return Some(2);
                        }
                    };
                    endpoint = ApiEndpoint::loopback(port);
                    index += 1;
                }
            }
            "--api-url" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("rhai: --api-url requires a URL");
                    return Some(2);
                };
                if value.starts_with('-') {
                    eprintln!("rhai: --api-url requires a URL");
                    return Some(2);
                }
                endpoint = ApiEndpoint::new(value);
                index += 1;
            }
            "-e" | "--eval" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("rhai: {0} requires a script", args[index]);
                    return Some(2);
                };
                one_shot = Some(value.clone());
                index += 1;
            }
            "-f" | "--file" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("rhai: --file requires a path");
                    return Some(2);
                };
                file = Some(value.clone());
                index += 1;
            }
            "--stdout" => output = OutputMode::Stdout,
            _ => {}
        }
        index += 1;
    }

    Some(run(&endpoint, one_shot, file, output))
}

fn run(
    endpoint: &ApiEndpoint,
    one_shot: Option<String>,
    file: Option<String>,
    output: OutputMode,
) -> i32 {
    let client = ApiClient::new(endpoint.clone());
    if let Some(code) = one_shot {
        return submit(&client, &code, output);
    }
    if let Some(path) = file {
        match lunco_storage::read_file_sync(Path::new(&path)) {
            Ok(bytes) => match String::from_utf8(bytes) {
                Ok(source) => return submit(&client, &source, output),
                Err(_) => {
                    eprintln!("rhai: cannot read {path}: file is not UTF-8");
                    return 2;
                }
            },
            Err(error) => {
                eprintln!("rhai: cannot read {path}: {error}");
                return 2;
            }
        }
    }

    let stdin = io::stdin();
    if !stdin.is_terminal() {
        let mut source = String::new();
        match stdin.lock().read_to_string(&mut source) {
            Ok(_) if !source.trim().is_empty() => return submit(&client, &source, output),
            Ok(_) => {}
            Err(error) => {
                eprintln!("rhai: cannot read stdin: {error}");
                return 2;
            }
        }
        return 0;
    }

    eprintln!(
        "LunCo Rhai REPL → {}  (prelude loaded — try `pause()`, `restart_scene()`. Ctrl-D / :q to quit)",
        client.endpoint()
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
            Err(error) => {
                eprintln!("rhai: cannot read stdin: {error}");
                status = 2;
                break;
            }
        }
        let code = line.trim();
        if code.is_empty() {
            continue;
        }
        if matches!(code, ":q" | "quit" | "exit") {
            break;
        }
        status = status.max(submit(&client, code, output));
    }
    status
}

fn submit(client: &ApiClient, code: &str, output: OutputMode) -> i32 {
    let request =
        ApiRequestEnvelope::execute_command("RunRhai", serde_json::json!({ "code": code }));
    match client.execute(&request) {
        Ok(response) => match output {
            OutputMode::Json => {
                match serde_json::to_string(&response) {
                    Ok(body) => println!("{body}"),
                    Err(error) => {
                        eprintln!("rhai: cannot encode API response: {error}");
                        return 2;
                    }
                }
                response_status(&response, false)
            }
            OutputMode::Stdout => response_status(&response, true),
        },
        Err(error) => {
            eprintln!("rhai: request to {} failed: {error}", client.endpoint());
            2
        }
    }
}

fn response_status(response: &ApiResponseEnvelope, stdout_only: bool) -> i32 {
    if let Some(error) = response.error.as_deref() {
        eprintln!("RHAI_ERROR: {error}");
        return 4;
    }
    let Some(stdout) = response
        .data
        .as_ref()
        .and_then(|data| data.get("stdout"))
        .and_then(serde_json::Value::as_str)
    else {
        eprintln!("rhai: RunRhai returned no captured stdout: {response:?}");
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
