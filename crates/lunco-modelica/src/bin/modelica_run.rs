//! `modelica_run` — headless Modelica simulation CLI.
//!
//! Compile a Modelica model and step it for a fixed duration, optionally
//! writing per-step variable values to a CSV. Reuses the same compile
//! path the workbench uses (`ModelicaCompiler` → `SimStepper`), so a
//! model that runs in the workbench runs here, and the rumoca cache
//! warmed by `msl_indexer --warm` benefits both.
//!
//! Mission/scenario files (input profiles over time, parameter overrides,
//! pass/fail verifiers) are intentionally **not** part of this binary —
//! that's a separate `mission_run` tool with its own `.mission.ron`
//! schema. This one is the minimal "compile + step + record" primitive
//! both the workbench and the future mission runner build on.
//!
//! ## Usage
//!
//! ```bash
//! modelica_run <FILE.mo> <CLASS> [OPTIONS]
//!
//!   <FILE.mo>          Path to the Modelica source file
//!   <CLASS>            Qualified class name to simulate (e.g. AnnotatedRocketStage.RocketStage)
//!
//! OPTIONS:
//!   -d, --duration SECS    Simulation duration in seconds [default: 10.0]
//!   -t, --dt SECS          Fixed-step timestep [default: 0.01]
//!       --output PATH      Write per-step CSV (header + one row per step)
//!       --input NAME=VAL   Set a runtime input value before step 0 (repeatable)
//!       --record VAR,VAR   Comma-separated variables to record (default: all observables)
//!   -v, --verbose          Per-step progress logging
//!   -h, --help             Show help
//! ```
//!
//! ## Example
//!
//! ```bash
//! # Run AnnotatedRocketStage.RocketStage for 10s, dump CSV
//! modelica_run \
//!     assets/models/AnnotatedRocketStage.mo \
//!     AnnotatedRocketStage.RocketStage \
//!     --output /tmp/rocket.csv
//!
//! # Run with a non-default valve command and a tighter timestep
//! modelica_run rocket.mo RocketStage \
//!     --duration 30 --dt 0.001 \
//!     --input valve_command=0.7 \
//!     --output /tmp/run.csv
//! ```

use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::time::Instant;

use lunco_modelica::ModelicaCompiler;
use rumoca_sim::{SimOptions, SimStepper};

/// CLI options. Hand-parsed (no `clap`) so the binary stays cheap to
/// build and link — same rationale as `msl_indexer`.
struct Options {
    file: PathBuf,
    class: String,
    duration: f64,
    dt: f64,
    output: Option<PathBuf>,
    inputs: Vec<(String, f64)>,
    record: Option<Vec<String>>,
    verbose: bool,
}

impl Options {
    fn parse() -> Self {
        let mut iter = std::env::args().skip(1);
        let mut positional: Vec<String> = Vec::new();
        let mut duration = 10.0f64;
        let mut dt = 0.01f64;
        let mut output: Option<PathBuf> = None;
        let mut inputs: Vec<(String, f64)> = Vec::new();
        let mut record: Option<Vec<String>> = None;
        let mut verbose = false;

        while let Some(arg) = iter.next() {
            match arg.as_str() {
                "-h" | "--help" => {
                    print_help();
                    std::process::exit(0);
                }
                "-v" | "--verbose" => verbose = true,
                "-d" | "--duration" => {
                    duration = parse_f64_arg("--duration", &mut iter);
                }
                "-t" | "--dt" => {
                    dt = parse_f64_arg("--dt", &mut iter);
                }
                "--output" => {
                    let v = iter.next().unwrap_or_else(|| die("--output requires a path"));
                    output = Some(PathBuf::from(v));
                }
                "--input" => {
                    let kv = iter.next().unwrap_or_else(|| die("--input requires NAME=VALUE"));
                    let (name, value) = kv.split_once('=').unwrap_or_else(|| {
                        die(&format!("--input expects NAME=VALUE, got `{kv}`"))
                    });
                    let v: f64 = value.parse().unwrap_or_else(|_| {
                        die(&format!("--input value `{value}` is not a number"))
                    });
                    inputs.push((name.to_string(), v));
                }
                "--record" => {
                    let list = iter.next().unwrap_or_else(|| {
                        die("--record requires a comma-separated variable list")
                    });
                    record = Some(
                        list.split(',')
                            .map(|s| s.trim().to_string())
                            // `time` is always emitted as the first
                            // column — silently de-dup if the user
                            // listed it explicitly so we don't double up.
                            .filter(|s| !s.is_empty() && s != "time")
                            .collect(),
                    );
                }
                other if other.starts_with('-') => {
                    die(&format!("unknown option `{other}` (use --help)"));
                }
                other => positional.push(other.to_string()),
            }
        }

        if positional.len() != 2 {
            eprintln!("error: expected exactly 2 positional args (FILE.mo and CLASS)");
            eprintln!();
            print_help();
            std::process::exit(2);
        }

        let file = PathBuf::from(&positional[0]);
        if !file.exists() {
            die(&format!("file `{}` does not exist", file.display()));
        }
        if duration <= 0.0 {
            die("--duration must be > 0");
        }
        if dt <= 0.0 {
            die("--dt must be > 0");
        }

        Self {
            file,
            class: positional[1].clone(),
            duration,
            dt,
            output,
            inputs,
            record,
            verbose,
        }
    }
}

fn parse_f64_arg(flag: &str, iter: &mut impl Iterator<Item = String>) -> f64 {
    let v = iter.next().unwrap_or_else(|| die(&format!("{flag} requires a number")));
    v.parse()
        .unwrap_or_else(|_| die(&format!("{flag} value `{v}` is not a number")))
}

fn die(msg: &str) -> ! {
    eprintln!("error: {msg}");
    std::process::exit(2);
}

fn print_help() {
    println!("modelica_run — headless Modelica simulation CLI");
    println!();
    println!("USAGE:");
    println!("  modelica_run <FILE.mo> <CLASS> [OPTIONS]");
    println!();
    println!("ARGS:");
    println!("  <FILE.mo>            Path to the Modelica source file");
    println!("  <CLASS>              Qualified class name to simulate");
    println!();
    println!("OPTIONS:");
    println!("  -d, --duration SECS  Simulation duration in seconds [default: 10.0]");
    println!("  -t, --dt SECS        Fixed-step timestep [default: 0.01]");
    println!("      --output PATH    Write per-step CSV (header + one row per step)");
    println!("      --input N=V      Set runtime input N to V before stepping (repeatable)");
    println!("      --record VARS    Comma-separated variables to record [default: all]");
    println!("  -v, --verbose        Per-step progress logging");
    println!("  -h, --help           Show this help");
    println!();
    println!("EXAMPLE:");
    println!("  modelica_run assets/models/AnnotatedRocketStage.mo \\");
    println!("      AnnotatedRocketStage.RocketStage \\");
    println!("      --duration 10 --output /tmp/rocket.csv");
}

fn main() {
    // Same one-liner as msl_indexer / ClassCachePlugin: route rumoca's
    // on-disk cache to the workspace's shared `.cache/rumoca`, so a
    // run here hits warm bytes from `msl_indexer --warm` and vice
    // versa. Honors an explicit `RUMOCA_CACHE_DIR` if the user set one.
    if std::env::var_os("RUMOCA_CACHE_DIR").is_none() {
        let target = lunco_assets::cache_dir().join("rumoca");
        std::env::set_var("RUMOCA_CACHE_DIR", &target);
        eprintln!(
            "[modelica_run] using rumoca cache at {}",
            target.display()
        );
    }

    let opts = Options::parse();

    let t_total = Instant::now();
    eprintln!(
        "[modelica_run] file={} class={} duration={}s dt={}s",
        opts.file.display(),
        opts.class,
        opts.duration,
        opts.dt
    );

    // Read the source. Cheap; the heavy lifting is the rumoca compile
    // that follows.
    let source = std::fs::read_to_string(&opts.file).unwrap_or_else(|e| {
        die(&format!("failed to read {}: {e}", opts.file.display()))
    });
    eprintln!("[modelica_run] read {} bytes", source.len());

    // Strip Modelica `input Real x = K` defaults into a separate map so
    // the stepper exposes them as runtime-settable input slots. Same
    // pre-processing the worker thread does (lib.rs ~894). Without it,
    // those declarations bake into the DAE as constants and `set_input`
    // can't reach them.
    let (stripped_source, input_defaults) = lunco_modelica::ast_extract::strip_input_defaults(&source);

    eprintln!("[modelica_run] compiling {} ...", opts.class);
    let t_compile = Instant::now();
    let mut compiler = ModelicaCompiler::new();
    let comp_res = match compiler.compile_str(&opts.class, &stripped_source, "model.mo") {
        Ok(r) => r,
        Err(e) => die(&format!("compile failed: {e}")),
    };
    eprintln!(
        "[modelica_run] compile done in {:.2}s",
        t_compile.elapsed().as_secs_f64()
    );

    // SimOptions: mirror what the workbench uses (lib.rs ~899) so
    // headless and interactive runs see the same numerics. atol/rtol
    // 1e-1 is loose for production but matches the workbench default;
    // override via env vars if you want tighter convergence.
    let mut stepper_opts = SimOptions::default();
    stepper_opts.atol = 1e-1;
    stepper_opts.rtol = 1e-1;

    let mut stepper = match SimStepper::new(&comp_res.dae, stepper_opts) {
        Ok(s) => s,
        Err(e) => die(&format!("stepper init failed: {e:?}")),
    };

    // Apply Modelica-source default values FIRST, then CLI --input
    // overrides. Same precedence as the workbench: source defaults
    // populate the slots, user-supplied values override.
    for (name, val) in &input_defaults {
        let _ = stepper.set_input(name, *val);
    }
    let mut applied_inputs: HashMap<String, f64> =
        input_defaults.iter().map(|(n, v)| (n.clone(), *v)).collect();
    for (name, val) in &opts.inputs {
        if !stepper.input_names().iter().any(|n| n == name) {
            eprintln!(
                "[modelica_run] WARN --input `{}` is not a known input of {} (known: {:?}); applying anyway",
                name, opts.class, stepper.input_names(),
            );
        }
        let _ = stepper.set_input(name, *val);
        applied_inputs.insert(name.clone(), *val);
    }
    if !applied_inputs.is_empty() {
        eprintln!("[modelica_run] inputs: {:?}", applied_inputs);
    }

    // Decide which variables to record. By default, record everything
    // the stepper observes at t=0 (matches `collect_stepper_observables`
    // semantics). User can pin a subset via --record to keep the CSV
    // narrow. Either way, capture the columns NOW so the CSV header is
    // stable for the whole run, even if the stepper's reported set
    // varies frame-to-frame (rumoca occasionally drops a NaN'd var).
    let initial_state: Vec<(String, f64)> = {
        stepper.state().values.into_iter()
            .filter(|(name, val)| val.is_finite() && name != "time")
            .collect()
    };

    let columns: Vec<String> = match &opts.record {
        Some(explicit) => explicit.clone(),
        None => initial_state.iter().map(|(n, _)| n.clone()).collect(),
    };

    if columns.is_empty() {
        die("no variables to record (initial state was empty and no --record given)");
    }
    eprintln!(
        "[modelica_run] recording {} variables: {}",
        columns.len(),
        if opts.verbose {
            columns.join(",")
        } else {
            columns.iter().take(8).cloned().collect::<Vec<_>>().join(",")
                + if columns.len() > 8 { ", ..." } else { "" }
        }
    );

    // Open the CSV writer up-front so any IO issue surfaces before
    // the user pays for a long simulation. `BufWriter` so per-step
    // writes don't hammer the syscall layer.
    let mut csv_writer: Option<std::io::BufWriter<std::fs::File>> = match &opts.output {
        Some(path) => {
            let f = std::fs::File::create(path).unwrap_or_else(|e| {
                die(&format!("failed to create {}: {e}", path.display()))
            });
            let mut w = std::io::BufWriter::new(f);
            // Header
            write!(w, "time").unwrap();
            for c in &columns {
                write!(w, ",{}", csv_escape(c)).unwrap();
            }
            writeln!(w).unwrap();
            // Initial sample (t=0)
            write!(w, "{:.9}", stepper.time()).unwrap();
            let init_map: HashMap<&str, f64> = initial_state
                .iter()
                .map(|(n, v)| (n.as_str(), *v))
                .collect();
            for c in &columns {
                let v = init_map.get(c.as_str()).copied().unwrap_or(f64::NAN);
                write!(w, ",{}", format_num(v)).unwrap();
            }
            writeln!(w).unwrap();
            Some(w)
        }
        None => None,
    };

    let total_steps = (opts.duration / opts.dt).round() as u64;
    eprintln!("[modelica_run] stepping: {} steps", total_steps);
    let t_run = Instant::now();
    let mut last_progress = Instant::now();
    let mut last_progress_step: u64 = 0;
    let mut steps_done: u64 = 0;
    let mut step_err: Option<String> = None;

    while stepper.time() < opts.duration - 1e-12 {
        if let Err(e) = stepper.step(opts.dt) {
            step_err = Some(format!("step failed at t={:.6}s: {e:?}", stepper.time()));
            break;
        }
        steps_done += 1;

        // Sample current state and write to CSV.
        if let Some(w) = csv_writer.as_mut() {
            let state: Vec<(String, f64)> = stepper.state().values.into_iter()
                .filter(|(_, v): &(String, f64)| v.is_finite())
                .collect();
            let map: HashMap<&str, f64> = state
                .iter()
                .map(|(n, v): &(String, f64)| (n.as_str(), *v))
                .collect();
            write!(w, "{:.9}", stepper.time()).unwrap();
            for c in &columns {
                let v = map.get(c.as_str()).copied().unwrap_or(f64::NAN);
                write!(w, ",{}", format_num(v)).unwrap();
            }
            writeln!(w).unwrap();
        }

        if opts.verbose {
            eprintln!(
                "[modelica_run] step {}/{} t={:.4}s",
                steps_done, total_steps, stepper.time()
            );
        } else if last_progress.elapsed() >= std::time::Duration::from_secs(1) {
            // One-second wall-clock progress tick. Show sim-time, real-
            // time-factor, ETA. Helps the user know if a 10-second sim
            // is going to take 10s or 10 minutes.
            let sim_elapsed = stepper.time();
            let wall_elapsed = t_run.elapsed().as_secs_f64();
            let rtf = if wall_elapsed > 0.0 { sim_elapsed / wall_elapsed } else { 0.0 };
            let eta_secs = if rtf > 1e-9 {
                (opts.duration - sim_elapsed) / rtf
            } else {
                f64::INFINITY
            };
            let steps_per_sec = (steps_done - last_progress_step) as f64
                / last_progress.elapsed().as_secs_f64();
            eprintln!(
                "[modelica_run] sim t={:.3}/{:.3}s ({:.0}%), {:.0} steps/s, RTF {:.2}x, ETA {:.1}s",
                sim_elapsed,
                opts.duration,
                100.0 * sim_elapsed / opts.duration,
                steps_per_sec,
                rtf,
                eta_secs,
            );
            last_progress = Instant::now();
            last_progress_step = steps_done;
        }
    }

    // Flush + close the CSV before logging the final summary, so a
    // user tailing the file sees the last row before "done".
    if let Some(w) = csv_writer.as_mut() {
        w.flush().unwrap();
    }

    let wall = t_run.elapsed().as_secs_f64();
    let rtf = if wall > 0.0 { stepper.time() / wall } else { 0.0 };
    eprintln!(
        "[modelica_run] stepping done: {} steps to t={:.3}s in {:.2}s wall (RTF {:.2}x)",
        steps_done,
        stepper.time(),
        wall,
        rtf
    );

    if let Some(err) = step_err {
        eprintln!("[modelica_run] WARN: simulation aborted — {err}");
    }
    if let Some(path) = &opts.output {
        eprintln!("[modelica_run] wrote CSV → {}", path.display());
    }
    eprintln!(
        "[modelica_run] all done in {:.2}s",
        t_total.elapsed().as_secs_f64()
    );
}

/// CSV-escape a header label. Conservative: quote if it contains any
/// of `,`, `"`, `\n`, `\r`; otherwise emit as-is.
fn csv_escape(s: &str) -> String {
    if s.chars().any(|c| matches!(c, ',' | '"' | '\n' | '\r')) {
        let escaped = s.replace('"', "\"\"");
        format!("\"{}\"", escaped)
    } else {
        s.to_string()
    }
}

/// Format a number for CSV output. `NaN`/`Inf` become empty cells —
/// most downstream tools (pandas, polars, GNU R) handle empty as NA
/// natively, while `nan` / `inf` strings throw type errors.
fn format_num(v: f64) -> String {
    if v.is_finite() {
        format!("{:.9}", v)
    } else {
        String::new()
    }
}
