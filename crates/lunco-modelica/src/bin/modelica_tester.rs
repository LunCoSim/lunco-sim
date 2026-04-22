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

    // 4. Try to build a stepper — this runs `prepare_dae` where the
    // MissingStateEquation error is raised.
    println!("\n--- Building stepper ---");
    let mut opts = rumoca_sim::StepperOptions::default();
    opts.atol = 1e-3;
    opts.rtol = 1e-3;
    match rumoca_sim::SimStepper::new(&result.dae, opts) {
        Ok(_s) => println!("Stepper built OK."),
        Err(e) => println!("Stepper init failed: {e:?}"),
    }

    Ok(())
}
