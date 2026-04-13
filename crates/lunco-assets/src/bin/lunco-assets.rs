//! CLI tool for managing LunCoSim assets.
//!
//! Reads `Assets.toml` files from each crate and handles download, verification, and listing.
//!
//! Usage:
//!   cargo run -p lunco-assets -- download          # download all workspace assets
//!   cargo run -p lunco-assets -- download -p lunco-modelica  # download for one crate
//!   cargo run -p lunco-assets -- process           # process all downloaded assets
//!   cargo run -p lunco-assets -- process -p lunco-celestial  # process one crate
//!   cargo run -p lunco-assets -- list              # list all workspace assets
//!   cargo run -p lunco-assets -- list -p lunco-celestial     # list for one crate

use std::path::PathBuf;
use lunco_assets::{download, process};

fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();

    if args.is_empty() {
        print_usage();
        return;
    }

    let mut crate_name: Option<&str> = None;
    let mut workspace_root: Option<&str> = None;
    let mut action: Option<&str> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "download" | "list" | "process" => action = Some(args[i].as_str()),
            "-p" | "--package" => {
                i += 1;
                crate_name = args.get(i).map(|s| s.as_str());
            }
            "--workspace-root" => {
                i += 1;
                workspace_root = args.get(i).map(|s| s.as_str());
            }
            "--help" | "-h" => {
                print_usage();
                return;
            }
            _ => {
                eprintln!("Unknown argument: {}", args[i]);
                print_usage();
                return;
            }
        }
        i += 1;
    }

    let Some(action) = action else {
        eprintln!("Error: no action specified");
        print_usage();
        return;
    };

    // Resolve workspace root: current dir's parent containing Cargo.toml with [workspace]
    let ws_root = resolve_workspace_root(workspace_root);

    let result = match action {
        "download" => {
            if let Some(name) = crate_name {
                let crate_dir = ws_root.join(format!("crates/{}", name));
                download::download_all_for_crate(&crate_dir)
                    .map_err(|e| e.to_string())
            } else {
                download::download_all_workspace(&ws_root)
                    .map_err(|e| e.to_string())
            }
        }
        "process" => {
            if let Some(name) = crate_name {
                let crate_dir = ws_root.join(format!("crates/{}", name));
                process_all_for_crate(&crate_dir)
                    .map_err(|e| e.to_string())
            } else {
                process_all_workspace(&ws_root)
                    .map_err(|e| e.to_string())
            }
        }
        "list" => {
            if let Some(name) = crate_name {
                let crate_dir = ws_root.join(format!("crates/{}", name));
                download::list_for_crate(&crate_dir)
                    .map_err(|e| e.to_string())
            } else {
                // List all
                list_all_workspace(&ws_root)
                    .map_err(|e| e.to_string())
            }
        }
        _ => unreachable!(),
    };

    if let Err(e) = result {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}

fn process_all_for_crate(crate_dir: &PathBuf) -> Result<(), String> {
    let manifest = download::AssetManifest::from_crate_dir(crate_dir)
        .map_err(|e| format!("Failed to read Assets.toml: {}", e))?;

    let mut processed = 0;
    for (key, entry) in &manifest.assets {
        if let Some(ref proc_cfg) = entry.process {
            let source_path = lunco_assets::cache_dir().join(&entry.dest);
            if !source_path.exists() {
                println!("  ⚠ {} source not found at {}, skipping", key, source_path.display());
                println!("    Run 'download' first.");
                continue;
            }
            println!("  processing {}...", key);
            process::process_texture(&source_path, proc_cfg)
                .map_err(|e| format!("Failed to process {}: {}", key, e))?;
            processed += 1;
        }
    }

    if processed == 0 {
        println!("  No assets with [process] section in {}", crate_dir.join("Assets.toml").display());
    } else {
        println!("  {} asset(s) processed", processed);
    }

    Ok(())
}

fn process_all_workspace(ws_root: &PathBuf) -> Result<(), String> {
    let cargo_toml = ws_root.join("Cargo.toml");
    let content = std::fs::read_to_string(&cargo_toml)
        .map_err(|e| format!("Failed to read Cargo.toml: {}", e))?;
    let workspace: toml::Value = toml::from_str(&content)
        .map_err(|e| format!("Failed to parse Cargo.toml: {}", e))?;

    let members = workspace["workspace"]["members"]
        .as_array()
        .ok_or_else(|| "No workspace.members in Cargo.toml".to_string())?;

    for member in members {
        if let Some(path) = member.as_str() {
            let crate_dir = ws_root.join(path);
            if crate_dir.join("Assets.toml").exists() {
                process_all_for_crate(&crate_dir)?;
            }
        }
    }

    Ok(())
}

fn resolve_workspace_root(override_path: Option<&str>) -> PathBuf {
    if let Some(p) = override_path {
        return PathBuf::from(p);
    }

    // Walk up from current dir looking for Cargo.toml with [workspace]
    let mut dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    loop {
        let cargo_toml = dir.join("Cargo.toml");
        if cargo_toml.exists() {
            if let Ok(content) = std::fs::read_to_string(&cargo_toml) {
                if content.contains("[workspace]") {
                    return dir;
                }
            }
        }

        if !dir.pop() {
            // Fallback to current dir
            return std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        }
    }
}

fn list_all_workspace(ws_root: &PathBuf) -> Result<(), String> {
    let cargo_toml = ws_root.join("Cargo.toml");
    let content = std::fs::read_to_string(&cargo_toml)
        .map_err(|e| format!("Failed to read Cargo.toml: {}", e))?;
    let workspace: toml::Value = toml::from_str(&content)
        .map_err(|e| format!("Failed to parse Cargo.toml: {}", e))?;

    let members = workspace["workspace"]["members"]
        .as_array()
        .ok_or_else(|| "No workspace.members in Cargo.toml".to_string())?;

    for member in members {
        if let Some(path) = member.as_str() {
            let crate_dir = ws_root.join(path);
            if crate_dir.join("Assets.toml").exists() {
                println!();
                if let Err(e) = download::list_for_crate(&crate_dir) {
                    eprintln!("  Error: {}", e);
                }
            }
        }
    }

    Ok(())
}

fn print_usage() {
    println!("LunCoSim Asset Manager");
    println!();
    println!("Usage:");
    println!("  cargo run -p lunco-assets -- download          Download all workspace assets");
    println!("  cargo run -p lunco-assets -- download -p NAME  Download for a specific crate");
    println!("  cargo run -p lunco-assets -- process           Process all downloaded assets");
    println!("  cargo run -p lunco-assets -- process -p NAME   Process assets for a crate");
    println!("  cargo run -p lunco-assets -- list              List all workspace assets");
    println!("  cargo run -p lunco-assets -- list -p NAME      List assets for a crate");
    println!();
    println!("Examples:");
    println!("  cargo run -p lunco-assets -- download -p lunco-modelica");
    println!("  cargo run -p lunco-assets -- process -p lunco-celestial");
    println!("  cargo run -p lunco-assets -- list");
}
