//! Dump parsed USDA structure for debugging.

use openusd::sdf::AbstractData;
use openusd::usda::TextReader;
use std::env;

fn main() {
    let args: Vec<String> = env::args().collect();
    let path = args.get(1).expect("Usage: dump_usda <path.usda>");
    
    println!("Loading: {}", path);
    let reader = TextReader::read(path).expect("Failed to read USD file");
    
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
