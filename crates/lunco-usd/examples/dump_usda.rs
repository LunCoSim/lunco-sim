//! Dump parsed USDA structure for debugging.

use openusd::usda;
use std::env;

fn main() {
    let args: Vec<String> = env::args().collect();
    let path = args.get(1).expect("Usage: dump_usda <path.usda>");

    println!("Loading: {}", path);
    // Single-layer parse (uncomposed) — mirrors the old `TextReader::read`.
    let text = std::fs::read_to_string(path).expect("Failed to read USD file");
    let reader = usda::parse(&text).expect("Failed to parse USD file");
    
    println!("\n=== Parsed Prims ===\n");
    for (prim_path, spec) in reader.iter() {
        println!("Path: {}", prim_path);
        println!("  SpecType: {:?}", spec.ty);
        
        // Show all fields
        for (field_name, value) in &spec.fields {
            println!("  {}: {:?}", field_name, value);
        }
        println!();
    }
}
