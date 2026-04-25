//! Standalone CLI tester for Modelica models using Rumoca.
//!
//! Provides a way to validate engineering models independently from the Bevy engine.

use lunco_modelica::ModelicaCompiler;
use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        println!("Usage: modelica_tester <model_path.mo> [model_name]");
        return Ok(());
    }

    let model_path = PathBuf::from(&args[1]);
    let model_name = args.get(2).cloned().unwrap_or_else(|| {
        model_path.file_stem().unwrap().to_str().unwrap().to_string()
    });

    println!("--- Modelica Tester ---");
    println!("Loading: {:?}", model_path);
    println!("Model Name: {}", model_name);

    // 1. Compile
    let model_path_str = model_path.to_str().ok_or_else(|| anyhow::anyhow!("Invalid path encoding"))?;
    let source = std::fs::read_to_string(model_path_str)
        .map_err(|e| anyhow::anyhow!("Failed to read model file: {e}"))?;
    let mut compiler = ModelicaCompiler::new();
    let result = compiler.compile_str(&model_name, &source, model_path_str)
        .map_err(|e| anyhow::anyhow!("Failed to compile Modelica model: {e}"))?;

    println!("Successfully compiled to DAE IR.");

    // 2. Export DAE IR
    let json_ir = serde_json::to_string(&result.dae)
        .map_err(|e| anyhow::anyhow!("Failed to export DAE IR: {e}"))?;
    println!("DAE IR Size: {} bytes", json_ir.len());

    // 3. Dump DAE state list + equations before stepper init.
    println!("\n--- DAE states ---");
    for (name, v) in result.dae.states.iter() {
        println!("  {} (size={})", name, v.size());
    }
    println!("--- DAE equations ({}) ---", result.dae.f_x.len());
    for (i, eq) in result.dae.f_x.iter().enumerate() {
        println!("  [{i}] origin={} scalar_count={}", eq.origin, eq.scalar_count);
    }

    // 4. Build a stepper and actually step it. Tolerances + dt +
    //    step count overridable via env vars so we can sweep them
    //    from the shell to find combinations that succeed.
    let atol = env_f64("ATOL", 1e-3);
    let rtol = env_f64("RTOL", 1e-3);
    let dt = env_f64("DT", 0.01);
    let n_steps: usize = std::env::var("N")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(100);
    let t_end_hint = dt * n_steps as f64;
    println!(
        "\n--- Building stepper (atol={:.1e} rtol={:.1e} dt={:.1e} n={}) ---",
        atol, rtol, dt, n_steps
    );
    let mut opts = rumoca_sim::StepperOptions::default();
    opts.atol = atol;
    opts.rtol = rtol;
    let mut stepper = match rumoca_sim::SimStepper::new(&result.dae, opts) {
        Ok(s) => {
            println!("Stepper built OK.");
            s
        }
        Err(e) => {
            println!("Stepper init FAILED: {e:?}");
            return Ok(());
        }
    };

    println!("Stepping to t~={:.3}s...", t_end_hint);
    let t0 = web_time::Instant::now();
    let mut failed_at = None;
    for i in 0..n_steps {
        if let Err(e) = stepper.step(dt) {
            failed_at = Some((i, stepper.time(), e));
            break;
        }
    }
    let dt_total = t0.elapsed().as_secs_f64();
    match failed_at {
        Some((i, t, e)) => println!(
            "STEP FAIL at step {i} (sim_t={:.6}s) after {:.2}s wall: {e:?}",
            t, dt_total
        ),
        None => println!(
            "All {n_steps} steps OK (sim_t={:.3}s, {:.2}s wall).",
            stepper.time(),
            dt_total
        ),
    }

    Ok(())
}

fn env_f64(name: &str, default: f64) -> f64 {
    std::env::var(name)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(default)
}
