//! CLI tool for managing LunCoSim assets.
//!
//! Reads the engine's manifests from `assets/manifests/*.toml` (one file per
//! GROUP, named for it) and handles download, verification, and listing. A
//! Twin's own `Assets.toml` is reached with `-t <DIR>` instead.
//!
//! Usage:
//!   cargo run -p lunco-assets -- download            # download every group
//!   cargo run -p lunco-assets -- download -g modelica  # download one group
//!   cargo run -p lunco-assets -- process             # process all downloaded assets
//!   cargo run -p lunco-assets -- process -g celestial  # process one group
//!   cargo run -p lunco-assets -- list                # list every group
//!   cargo run -p lunco-assets -- list -g celestial     # list one group

// Native-only CLI on the documented `clippy.toml` allow-list — owns
// raw `std::fs` access to the on-disk asset cache.
#![allow(clippy::disallowed_methods)]

use std::path::PathBuf;
use lunco_assets::{download, process};

fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();

    if args.is_empty() {
        print_usage();
        return;
    }

    let mut group: Option<&str> = None;
    let mut action: Option<&str> = None;
    // `-a NAME` / `--asset NAME` — download only one asset by its key
    // (the header in Assets.toml: `[dejavu_sans]` → key "dejavu_sans"),
    // searching every crate's manifest. Saves re-downloading the full
    // workspace asset set just to refresh a single font / texture.
    let mut asset_key: Option<&str> = None;
    // `-t DIR` / `--twin DIR` — target a **Twin folder** instead of the
    // workspace. The folder's `Assets.toml` is read directly (no Cargo
    // workspace membership needed) and each asset's `dest` resolves against
    // the Twin root, so files download *into* the Twin. Lets a standalone
    // project (e.g. a school twin on disk) self-provision its terrain DTM,
    // models, etc. Mutually exclusive with `-p`; composes with `-a KEY` to
    // fetch/process a single Twin asset.
    let mut twin_dir: Option<&str> = None;
    // `--quality coarse|good` (process only) — quick-start knob. `coarse`
    // quarters each entry's `target_resolution` (floor 64) for a fast first
    // bake; `good` (default) bakes as authored. The bake key includes the
    // effective resolution, so switching quality rebakes exactly the
    // affected outputs and nothing silently serves the wrong tier.
    let mut quality: &str = "good";

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "download" | "list" | "process" => action = Some(args[i].as_str()),
            "-g" | "--group" => {
                i += 1;
                group = args.get(i).map(|s| s.as_str());
            }
            "-a" | "--asset" => {
                i += 1;
                asset_key = args.get(i).map(|s| s.as_str());
            }
            "-t" | "--twin" => {
                i += 1;
                twin_dir = args.get(i).map(|s| s.as_str());
            }
            "--quality" => {
                i += 1;
                match args.get(i).map(|s| s.as_str()) {
                    Some(q @ ("coarse" | "good")) => quality = q,
                    other => {
                        eprintln!(
                            "Error: --quality expects `coarse` or `good`, got {:?}",
                            other.unwrap_or("<nothing>")
                        );
                        return;
                    }
                }
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

    // `--twin` selects the Twin download/process/list path, which reads the
    // folder's own Assets.toml and resolves dests against the Twin root.
    // `-a KEY` composes with it: a school twin that lists every candidate
    // territory would otherwise pull multiple GB on each provisioning run.
    if let Some(dir) = twin_dir {
        let twin_root = PathBuf::from(dir);
        let result = match (action, asset_key) {
            ("download", Some(key)) => {
                download::download_one_for_twin(&twin_root, key).map_err(|e| e.to_string())
            }
            ("download", None) => {
                download::download_all_for_twin(&twin_root).map_err(|e| e.to_string())
            }
            ("process", key) => process_for_twin(&twin_root, key, quality),
            ("list", _) => download::list_for_twin(&twin_root).map_err(|e| e.to_string()),
            _ => unreachable!(),
        };
        if let Err(e) = result {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
        return;
    }

    let result = match action {
        "download" => {
            if let Some(key) = asset_key {
                // `-a` targets a single asset in any group. Takes precedence
                // over `-g` (one asset is more specific than one group).
                download::download_one_engine(key).map_err(|e| e.to_string())
            } else if let Some(g) = group {
                download::download_all_for_group(g).map_err(|e| e.to_string())
            } else {
                download::download_all_engine().map_err(|e| e.to_string())
            }
        }
        "process" => {
            if let Some(g) = group {
                process_group(g)
            } else {
                process_all_groups()
            }
        }
        "list" => {
            if let Some(g) = group {
                download::list_group(g).map_err(|e| e.to_string())
            } else {
                list_all_groups()
            }
        }
        _ => unreachable!(),
    };

    if let Err(e) = result {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}

/// Process every `[*.process]` entry in a folder's `Assets.toml`.
///
/// Each entry's *source* is wherever the download step put it — resolved
/// through the same `download::entry_dest_path` (authored `dest` inside the
/// Twin/cache, or the shared source pool), so the two steps can never
/// disagree. `twin_root` is forwarded to `process_asset` so
/// `output_root = "twin"` writes land inside the Twin; `None` for crates.
/// Optional `-a KEY` filter — the process-side twin of the download filter,
/// so a single territory can be re-baked without touching every other
/// entry's sources — and the `--quality` preset (`coarse` quarters
/// `target_resolution`, floor 64, for a quick first bake).
fn process_filtered(
    manifest_path: &std::path::Path,
    twin_root: Option<&std::path::Path>,
    only_key: Option<&str>,
    quality: &str,
) -> Result<(), String> {
    let manifest = download::AssetManifest::from_file(manifest_path)
        .map_err(|e| format!("Failed to read {}: {}", manifest_path.display(), e))?;

    if let Some(key) = only_key {
        if !manifest.assets.contains_key(key) {
            return Err(format!("no asset `{}` in {}", key, manifest_path.display()));
        }
    }

    let mut processed = 0;
    for (key, entry) in &manifest.assets {
        if only_key.is_some_and(|k| k != key) {
            continue;
        }
        if let Some(ref proc_cfg) = entry.process {
            let source_path = download::entry_dest_path(entry, twin_root);
            if !source_path.exists() {
                println!("  ⚠ {} source not found at {}, skipping", key, source_path.display());
                println!("    Run 'download' first.");
                continue;
            }
            println!("  processing {}...", key);
            // `coarse` is a derived config, not an edit: the bake key hashes
            // the EFFECTIVE config, so coarse and good outputs never alias.
            let mut cfg = proc_cfg.clone();
            if quality == "coarse" {
                if let Some([w, h]) = cfg.target_resolution {
                    cfg.target_resolution = Some([(w / 4).max(64), (h / 4).max(64)]);
                }
            }
            process::process_asset(&source_path, &cfg, twin_root)
                .map_err(|e| format!("Failed to process {}: {}", key, e))?;
            processed += 1;
        }
    }

    if processed == 0 {
        println!("  No assets with [process] section in {}", manifest_path.display());
    } else {
        println!("  {} asset(s) processed", processed);
    }

    Ok(())
}

/// Process one engine manifest group (`assets/manifests/<group>.toml`).
fn process_group(group: &str) -> Result<(), String> {
    println!("Processing `{group}`...");
    process_filtered(
        &lunco_assets::manifests_dir().join(format!("{group}.toml")),
        None,
        None,
        "good",
    )
}

/// Process every engine manifest group.
fn process_all_groups() -> Result<(), String> {
    for (group, _) in lunco_assets::engine_manifests() {
        process_group(&group)?;
    }
    Ok(())
}

/// List every engine manifest group.
fn list_all_groups() -> Result<(), String> {
    for (group, _) in lunco_assets::engine_manifests() {
        println!();
        if let Err(e) = download::list_group(&group) {
            eprintln!("  Error: {}", e);
        }
    }
    Ok(())
}

/// Process a Twin folder's `[*.process]` entries: sources + twin-targeted
/// outputs both resolve against the Twin root (the `--twin <DIR>` path).
/// `only_key` narrows to a single entry (`-a KEY`); `quality` is the
/// `--quality` preset.
fn process_for_twin(
    twin_root: &std::path::Path,
    only_key: Option<&str>,
    quality: &str,
) -> Result<(), String> {
    process_filtered(
        &twin_root.join("Assets.toml"),
        Some(twin_root),
        only_key,
        quality,
    )
}

fn print_usage() {
    println!("LunCoSim Asset Manager");
    println!();
    println!("Usage:");
    println!("  cargo run -p lunco-assets -- download              Download every declared asset");
    println!("  cargo run -p lunco-assets -- download -g GROUP     Download one manifest group");
    println!("  cargo run -p lunco-assets -- download -a KEY       Download a single asset by key");
    println!("  cargo run -p lunco-assets -- download -t DIR       Download a Twin folder's assets (into the Twin)");
    println!("  cargo run -p lunco-assets -- download -t DIR -a KEY  Download one Twin asset by key (skips the rest)");
    println!("  cargo run -p lunco-assets -- process  -t DIR -a KEY  Process one Twin asset by key");
    println!("  cargo run -p lunco-assets -- process  -t DIR --quality coarse   Quick-start bake (¼ resolution; re-run with `good` for full)");
    println!("  cargo run -p lunco-assets -- process               Process all downloaded assets");
    println!("  cargo run -p lunco-assets -- process -g GROUP      Process one manifest group");
    println!("  cargo run -p lunco-assets -- process  -t DIR       Process a Twin folder's assets");
    println!("  cargo run -p lunco-assets -- list                  List every declared asset");
    println!("  cargo run -p lunco-assets -- list -g GROUP         List one manifest group");
    println!("  cargo run -p lunco-assets -- list -t DIR           List a Twin folder's assets");
    println!();
    println!("Process kinds (in an Assets.toml [name.process] section):");
    println!("  kind = \"texture\"  resize/re-encode an image (PNG/JPEG/TIFF/...) [default]");
    println!("  kind = \"gltf\"     clean a .glb for Bevy 0.18 (needs Node/npx)");
    println!("  kind = \"dem\"      crop a square georeferenced float32 heightmap from a raw DTM (GeoTIFF or PDS3 .IMG)");
    println!("  kind = \"map\"      crop a co-registered ortho/shade/slope raster to the same ROI as an 8-bit PNG layer map");
    println!("  kind = \"normalmap\" derive a world-space normal-map PNG from a DTM crop");
    println!();
    println!("Examples:");
    println!("  cargo run -p lunco-assets -- download -p lunco-modelica");
    println!("  cargo run -p lunco-assets -- download -a dejavu_sans");
    println!("  cargo run -p lunco-assets -- download -t /path/to/my_twin");
    println!("  cargo run -p lunco-assets -- process  -t /path/to/my_twin");
    println!("  cargo run -p lunco-assets -- process -p lunco-celestial");
    println!("  cargo run -p lunco-assets -- list");
}
