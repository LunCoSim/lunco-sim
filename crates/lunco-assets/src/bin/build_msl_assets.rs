//! Build-time MSL bundler for the web target.
//!
//! Reads the on-disk MSL tree (whatever `lunco_assets::msl_source_root_path`
//! points at on this host), packs every `.mo` source file into a tarball,
//! zstd-compresses it, hashes the result, and emits both the bundle and a
//! manifest into the chosen output directory.
//!
//! Output layout:
//!
//! ```text
//! <out>/
//!   manifest.json           # { msl_root_marker, sources_blob, sources_sha256, ... }
//!   sources-<sha8>.tar.zst  # tar of *.mo files relative to MSL root
//! ```
//!
//! The wasm runtime fetches `manifest.json` first, then the blob whose name
//! is in the manifest. Hashed filenames mean a deploy can never serve a stale
//! manifest against a freshened blob.
//!
//! ## Usage
//!
//! ```bash
//! cargo run -p lunco-assets --bin build_msl_assets -- \
//!     --out dist/modelica_workbench_web/msl
//! ```
//!
//! No CLI deps — uses bare `std::env::args` to keep this binary cheap to
//! compile.
//!
//! ## What gets packed
//!
//! - All `.mo` files under MSL root (recursive).
//! - Top-level `.mo` files like `Complex.mo`, `ObsoleteModelica4.mo`.
//! - Skipped: `Resources/` images and matrix data. Step 1b will add these
//!   once we know the wasm runtime needs them — most compile paths don't.

#![cfg(not(target_arch = "wasm32"))]

use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::{BufWriter, Read};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// Tag stamped into the manifest's `rumoca_artifact_tag` field. The
/// wasm runtime refuses to deserialise a parsed bundle whose tag
/// doesn't match its compiled-in expectation — the bincode'd
/// `StoredDefinition` layout is rumoca-version sensitive. Bump this
/// whenever the rumoca version we point at changes its AST shape.
const RUMOCA_ARTIFACT_TAG: &str = "rumoca-0.8.12-wasm-asset-loader";

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut out_dir: Option<PathBuf> = None;
    let mut msl_root_override: Option<PathBuf> = None;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--out" => {
                i += 1;
                out_dir = Some(PathBuf::from(&args[i]));
            }
            "--msl-root" => {
                i += 1;
                msl_root_override = Some(PathBuf::from(&args[i]));
            }
            "-h" | "--help" => {
                eprintln!("usage: build_msl_assets --out <dir> [--msl-root <dir>]");
                return;
            }
            other => {
                eprintln!("unknown argument: {other}");
                std::process::exit(2);
            }
        }
        i += 1;
    }
    let Some(out_dir) = out_dir else {
        eprintln!("error: --out is required");
        std::process::exit(2);
    };

    let msl_root = msl_root_override
        .or_else(lunco_assets::msl_source_root_path)
        .unwrap_or_else(|| {
            eprintln!(
                "error: no MSL tree on disk (run `lunco-assets -- download` first \
                 or pass --msl-root)"
            );
            std::process::exit(1);
        });
    eprintln!("MSL root: {}", msl_root.display());
    eprintln!("Output:   {}", out_dir.display());

    fs::create_dir_all(&out_dir).expect("create out dir");

    let mut entries: Vec<PathBuf> = Vec::new();
    collect_mo_files(&msl_root, &msl_root, &mut entries);
    // Also pack the precomputed palette index if present. The web
    // runtime reads it via `MslAssetSource::read("msl_index.json")` to
    // populate `msl_component_library()` — without this the palette
    // ships empty on wasm.
    let index_path = msl_root.join("msl_index.json");
    if index_path.is_file() {
        entries.push(index_path);
    } else {
        eprintln!(
            "warning: {} not present — palette will be empty on web. \
             Run `cargo run -p lunco-modelica --bin msl_indexer` first.",
            index_path.display()
        );
    }
    entries.sort(); // deterministic tar order → reproducible hash
    eprintln!("found {} files to pack", entries.len());

    // Tar → zstd → write to a temp file, then rename to hashed final path.
    let tmp_path = out_dir.join("sources.tar.zst.tmp");
    let total_uncompressed = pack(&msl_root, &entries, &tmp_path);

    let sha = file_sha256(&tmp_path);
    let short = &sha[..16];
    let final_name = format!("sources-{short}.tar.zst");
    let final_path = out_dir.join(&final_name);
    fs::rename(&tmp_path, &final_path).expect("rename to final");
    let compressed_size = fs::metadata(&final_path).expect("stat final").len();

    // Pre-parse every .mo source into a `StoredDefinition`, bincode-
    // serialise the `Vec<(uri, StoredDefinition)>`, and zstd-compress.
    // The wasm runtime fetches this and deserialises directly into
    // rumoca via `Session::replace_parsed_source_set` — the alternative
    // (per-file parse on the page) is ~27 minutes for 2670 files.
    eprintln!("parsing {} files for the pre-parsed bundle…", entries.len());
    let parsed = pre_parse(&msl_root, &entries);
    let parsed_count = parsed.len();
    eprintln!("parsed {parsed_count} / {} files", entries.len());

    let parsed_tmp = out_dir.join("parsed.bin.zst.tmp");
    let parsed_uncompressed_size = serialise_parsed(&parsed, &parsed_tmp);
    let parsed_sha = file_sha256(&parsed_tmp);
    let parsed_short = &parsed_sha[..16];
    let parsed_final_name = format!("parsed-{parsed_short}.bin.zst");
    let parsed_final_path = out_dir.join(&parsed_final_name);
    fs::rename(&parsed_tmp, &parsed_final_path).expect("rename parsed");
    let parsed_compressed_size = fs::metadata(&parsed_final_path).expect("stat parsed").len();

    let manifest = serde_json::json!({
        "schema_version": 1,
        "sources": {
            "filename": final_name,
            "sha256": sha,
            "uncompressed_bytes": total_uncompressed,
            "compressed_bytes": compressed_size,
            "file_count": entries.len(),
        },
        "parsed": {
            "filename": parsed_final_name,
            "sha256": parsed_sha,
            "uncompressed_bytes": parsed_uncompressed_size,
            "compressed_bytes": parsed_compressed_size,
            "file_count": parsed_count,
        },
        "rumoca_artifact_tag": RUMOCA_ARTIFACT_TAG,
        "msl_root_marker": "Modelica/package.mo",
    });
    let manifest_path = out_dir.join("manifest.json");
    let manifest_bytes = serde_json::to_vec_pretty(&manifest).expect("serialise manifest");
    fs::write(&manifest_path, &manifest_bytes).expect("write manifest");

    eprintln!(
        "wrote {} ({:.1} MB compressed, {:.1} MB uncompressed)",
        final_name,
        compressed_size as f64 / 1_048_576.0,
        total_uncompressed as f64 / 1_048_576.0,
    );
    eprintln!(
        "wrote {} ({:.1} MB compressed, {:.1} MB uncompressed; {} docs)",
        parsed_final_name,
        parsed_compressed_size as f64 / 1_048_576.0,
        parsed_uncompressed_size as f64 / 1_048_576.0,
        parsed_count,
    );
    eprintln!("wrote manifest.json");
}

/// Parse every MSL `.mo` file into `(uri, StoredDefinition)`. URIs use
/// MSL-relative forward-slash paths so they match what the wasm runtime
/// will reconstruct from the source bundle; rumoca treats these as
/// stable identifiers (not real filesystem paths).
fn pre_parse(
    msl_root: &Path,
    entries: &[PathBuf],
) -> Vec<(String, rumoca_ir_ast::StoredDefinition)> {
    let mut out = Vec::with_capacity(entries.len());
    // Only `.mo` files are Modelica source; the bundle may also carry
    // ancillary files (e.g. `msl_index.json` for the palette) that we
    // pack as bytes but don't try to parse here.
    let mo_entries: Vec<&PathBuf> = entries
        .iter()
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("mo"))
        .collect();
    let total = mo_entries.len();
    let mut last_pct: usize = usize::MAX;
    for (i, path) in mo_entries.iter().enumerate() {
        let rel = path.strip_prefix(msl_root).expect("entry under msl root");
        let uri = rel.to_string_lossy().replace('\\', "/");
        let source = match fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("  skip {uri}: read failed: {e}");
                continue;
            }
        };
        match rumoca_phase_parse::parse_to_ast(&source, &uri) {
            Ok(def) => out.push((uri, def)),
            Err(e) => eprintln!("  skip {uri}: parse failed: {e}"),
        }
        let pct = (i + 1) * 20 / total; // 5% buckets
        if pct != last_pct {
            eprintln!("  parse: {} / {total} ({}%)", i + 1, pct * 5);
            last_pct = pct;
        }
    }
    out
}

fn serialise_parsed(parsed: &[(String, rumoca_ir_ast::StoredDefinition)], dest: &Path) -> u64 {
    let raw = bincode::serialize(parsed).expect("bincode serialise parsed");
    let uncompressed = raw.len() as u64;
    let file = File::create(dest).expect("create parsed tmp");
    let buf = BufWriter::new(file);
    let mut zstd_w = zstd::Encoder::new(buf, 19).expect("zstd encoder");
    use std::io::Write as _;
    zstd_w.write_all(&raw).expect("write parsed bincode");
    zstd_w.finish().expect("zstd finish parsed");
    uncompressed
}

fn collect_mo_files(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for entry in rd.flatten() {
        let path = entry.path();
        if path.is_dir() {
            // Skip rumoca's own caches inside the MSL tree, if present.
            let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
            if matches!(name, ".cache" | "target" | "Resources") {
                continue;
            }
            collect_mo_files(root, &path, out);
        } else if path.extension().and_then(|s| s.to_str()) == Some("mo") {
            out.push(path);
        }
    }
}

fn pack(msl_root: &Path, entries: &[PathBuf], dest: &Path) -> u64 {
    let file = File::create(dest).expect("create tmp tar");
    let buf = BufWriter::new(file);
    // Level 19 = strong compression; we're producing this once per build.
    let zstd_w = zstd::Encoder::new(buf, 19)
        .expect("zstd encoder")
        .auto_finish();
    let mut tar_w = tar::Builder::new(zstd_w);

    let mut total: u64 = 0;
    for path in entries {
        let mut f = File::open(path).expect("open .mo");
        let mut bytes = Vec::new();
        f.read_to_end(&mut bytes).expect("read .mo");
        total += bytes.len() as u64;

        let rel = path.strip_prefix(msl_root).expect("entry under msl root");
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        // Zero out mtime so the bundle hash is reproducible across builds
        // of the same MSL tree.
        header.set_mtime(0);
        header.set_cksum();
        tar_w
            .append_data(&mut header, rel, &bytes[..])
            .expect("tar append");
    }
    tar_w.finish().expect("tar finish");
    total
}

fn file_sha256(path: &Path) -> String {
    let mut f = File::open(path).expect("open for hashing");
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 65536];
    loop {
        let n = f.read(&mut buf).expect("read for hashing");
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    hex_lower(&hasher.finalize())
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        write!(&mut s, "{b:02x}").unwrap();
    }
    s
}
