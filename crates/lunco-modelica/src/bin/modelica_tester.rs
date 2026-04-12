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

    // 3. Simple Mock Simulation Step
    println!("Starting mock simulation (t=0.0 to t=1.0)...");

    let mut current_time = 0.0;
    let dt = 0.1;

    while current_time < 1.0 {
        // TODO: Use rumoca-sim to actually step the model
        current_time += dt;
        println!("  Step: t = {:.2}", current_time);
    }

    println!("Simulation complete.");

    Ok(())
}
