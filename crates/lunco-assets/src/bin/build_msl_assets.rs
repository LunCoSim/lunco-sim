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
//!     --out dist/lunica/msl
//! ```
//!
//! To ship third-party libraries alongside MSL in the same bundle (so they
//! resolve on web exactly like native does from `cache_dir()`):
//!
//! ```bash
//! cargo run -p lunco-assets --bin build_msl_assets -- \
//!     --out dist/lunica/msl \
//!     --extra-root ~/.cache/luncosim/thermofluidstream \
//!     --discover-extras
//! ```
//!
//! No CLI deps — uses bare `std::env::args` to keep this binary cheap to
//! compile.
//!
//! ## What gets packed
//!
//! - All `.mo` files under MSL root (recursive).
//! - Top-level `.mo` files like `Complex.mo`, `ObsoleteModelica4.mo`.
//! - All `.mo` files under each `--extra-root` (and, with `--discover-extras`,
//!   every third-party library found in `cache_dir()`). Each root's files are
//!   stored under their own top-level package dir (`Modelica/…`, `Buildings/…`),
//!   so a single combined tar + parsed bundle yields one in-memory source whose
//!   keys never collide. The web resolver iterates roots automatically — no
//!   wasm-side change is needed.
//! - Skipped: `Resources/` images and matrix data. Step 1b will add these
//!   once we know the wasm runtime needs them — most compile paths don't.
//! - Skipped: any top-level package named by `--exclude <name>` (repeatable;
//!   `<name>*` is a prefix match). Used to drop the MSL distribution's in-tree
//!   test suites (`--exclude 'ModelicaTest*'`) — they ship inside the MSL tree
//!   but aren't part of the library. Applied only at each root's top level.

#![cfg(not(target_arch = "wasm32"))]
// Native-only build-time bundler on the documented `clippy.toml`
// allow-list — owns raw `std::fs` access to the on-disk MSL tree.
#![allow(clippy::disallowed_methods)]

use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::{BufWriter, Read};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// Tag stamped into the manifest's `rumoca_artifact_tag` field. Shared with the
/// runtime via [`lunco_assets::msl::EXPECTED_RUMOCA_ARTIFACT_TAG`] so producer
/// and consumer can't drift; the runtime refuses a parsed bundle whose tag
/// doesn't match (the bincode'd `StoredDefinition` layout is rumoca-version
/// sensitive). Bump the shared const when the rumoca AST shape changes.
use lunco_assets::msl::EXPECTED_RUMOCA_ARTIFACT_TAG as RUMOCA_ARTIFACT_TAG;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut out_dir: Option<PathBuf> = None;
    let mut msl_root_override: Option<PathBuf> = None;
    let mut extra_roots: Vec<PathBuf> = Vec::new();
    let mut discover_extras = false;
    let mut exclude: Vec<String> = Vec::new();
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
            "--extra-root" => {
                i += 1;
                extra_roots.push(PathBuf::from(&args[i]));
            }
            "--discover-extras" => {
                discover_extras = true;
            }
            "--exclude" => {
                i += 1;
                exclude.push(args[i].clone());
            }
            "-h" | "--help" => {
                eprintln!(
                    "usage: build_msl_assets --out <dir> [--msl-root <dir>] \
                     [--extra-root <dir>]... [--discover-extras] [--exclude <name>]..."
                );
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

    // The bundle's first root is always MSL; extras are appended in a stable
    // order. Mirrors native's `sources_with_extras`: primary MSL root +
    // third-party libs discovered under `cache_dir()`.
    if discover_extras {
        for root in discover_extra_roots() {
            if !extra_roots.contains(&root) {
                extra_roots.push(root);
            }
        }
    }
    let mut roots: Vec<PathBuf> = vec![msl_root.clone()];
    for r in extra_roots {
        eprintln!("extra root: {}", r.display());
        roots.push(r);
    }

    fs::create_dir_all(&out_dir).expect("create out dir");

    // Entries carry their OWN root so the tar/URI key is computed relative to
    // it — each root contributes its own top-level package dir as a namespace
    // (`Modelica/…`, `Buildings/…`). A single combined tar + parsed set holds
    // them all; the web resolver iterates roots, so keys must not collide.
    if !exclude.is_empty() {
        eprintln!("excluding top-level packages: {}", exclude.join(", "));
    }
    let mut entries: Vec<(PathBuf, PathBuf)> = Vec::new();
    for root in &roots {
        let mut files: Vec<PathBuf> = Vec::new();
        collect_mo_files(root, root, &exclude, &mut files);
        for f in files {
            entries.push((root.clone(), f));
        }
    }
    // Also pack the precomputed palette index if present (MSL root only). The
    // web runtime reads it via `MslAssetSource::read("msl_index.json")` to
    // populate `msl_component_library()` — without this the palette ships
    // empty on wasm.
    let index_path = msl_root.join("msl_index.json");
    if index_path.is_file() {
        entries.push((msl_root.clone(), index_path));
    } else {
        eprintln!(
            "warning: {} not present — palette will be empty on web. \
             Run `cargo run -p lunco-modelica --bin msl_indexer` first.",
            index_path.display()
        );
    }
    // Deterministic tar order → reproducible hash. Sort by the root-relative
    // key (NOT absolute path): extra roots live at machine-specific absolute
    // paths, but their relative keys are stable.
    entries.sort_by(|(ra, pa), (rb, pb)| rel_key(ra, pa).cmp(&rel_key(rb, pb)));
    // Guard against two roots claiming the same relative key (e.g. an extra
    // root that also ships a `Modelica/` tree) — last-writer-wins in the tar
    // is silent corruption, so drop dups loudly.
    let mut seen = std::collections::HashSet::new();
    entries.retain(|(r, p)| {
        let key = rel_key(r, p);
        if seen.insert(key.clone()) {
            true
        } else {
            eprintln!("warning: duplicate bundle key {key} — dropping later copy");
            false
        }
    });
    eprintln!(
        "found {} files to pack across {} root(s)",
        entries.len(),
        roots.len()
    );

    // Tar → zstd → write to a temp file, then rename to hashed final path.
    let tmp_path = out_dir.join("sources.tar.zst.tmp");
    let total_uncompressed = pack(&entries, &tmp_path);

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
    let parsed = pre_parse(&entries);
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
        // Top-level packages filtered out of this bundle (e.g. `ModelicaTest*`).
        // Recorded so the bundle is self-describing and the build script can
        // tell whether a cached bundle matches the requested exclude config.
        "excluded_packages": exclude,
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

/// Parse every `.mo` file into `(uri, StoredDefinition)`. URIs use
/// root-relative forward-slash paths so they match what the wasm runtime
/// will reconstruct from the source bundle; rumoca treats these as
/// stable identifiers (not real filesystem paths). Each entry is stripped
/// against its own root, so files from different library roots keep their
/// distinct top-level package namespace.
fn pre_parse(
    entries: &[(PathBuf, PathBuf)],
) -> Vec<(String, rumoca_ir_ast::StoredDefinition)> {
    use rayon::prelude::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // Only `.mo` files are Modelica source; the bundle may also carry
    // ancillary files (e.g. `msl_index.json` for the palette) that we
    // pack as bytes but don't try to parse here.
    let mo_entries: Vec<&(PathBuf, PathBuf)> = entries
        .iter()
        .filter(|(_, p)| p.extension().and_then(|s| s.to_str()) == Some("mo"))
        .collect();
    let total = mo_entries.len();
    eprintln!(
        "  parsing {total} .mo files across {} threads…",
        rayon::current_num_threads()
    );

    // Parse in parallel — each file is independent and `parse_to_ast` is
    // pure. `.map(...).collect::<Vec<_>>()` over an *indexed* parallel
    // iterator preserves input order exactly, so flattening the `Option`s
    // yields a deterministic bundle (stable sha256 → the build's
    // content-addressed `parsed-<sha>.bin.zst` skip logic keeps working).
    let done = AtomicUsize::new(0);
    let parsed: Vec<Option<(String, rumoca_ir_ast::StoredDefinition)>> = mo_entries
        .par_iter()
        .map(|(root, path)| {
            let uri = rel_key(root, path);
            let n = done.fetch_add(1, Ordering::Relaxed) + 1;
            if n % 500 == 0 || n == total {
                eprintln!("  parsed {n} / {total}");
            }
            let source = match fs::read_to_string(path) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("  skip {uri}: read failed: {e}");
                    return None;
                }
            };
            match rumoca_phase_parse::parse_to_ast(&source, &uri) {
                Ok(def) => Some((uri, def)),
                Err(e) => {
                    eprintln!("  skip {uri}: parse failed: {e}");
                    None
                }
            }
        })
        .collect();
    parsed.into_iter().flatten().collect()
}

fn serialise_parsed(parsed: &[(String, rumoca_ir_ast::StoredDefinition)], dest: &Path) -> u64 {
    let raw = bincode::serde::encode_to_vec(parsed, bincode::config::standard())
        .expect("bincode serialise parsed");
    let uncompressed = raw.len() as u64;
    let file = File::create(dest).expect("create parsed tmp");
    let buf = BufWriter::new(file);
    let mut zstd_w = zstd::Encoder::new(buf, 19).expect("zstd encoder");
    use std::io::Write as _;
    zstd_w.write_all(&raw).expect("write parsed bincode");
    zstd_w.finish().expect("zstd finish parsed");
    uncompressed
}

/// Walk `dir` (under `root`), collecting every `.mo` file.
///
/// `exclude` drops top-level packages by name — applied ONLY at the root level
/// (`dir == root`) so a coincidentally-named nested package is never affected.
/// A pattern ending in `*` is a prefix match (e.g. `ModelicaTest*` catches the
/// `ModelicaTest/` dir plus the `ModelicaTestConversion4.mo` /
/// `ModelicaTestOverdetermined.mo` siblings, and any future `ModelicaTest…`
/// suite an MSL bump adds). A top-level `.mo`'s package name is its file stem.
fn collect_mo_files(root: &Path, dir: &Path, exclude: &[String], out: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    let at_top = dir == root;
    for entry in rd.flatten() {
        let path = entry.path();
        let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
        if at_top {
            // Top-level package name: dir name as-is, or the `.mo` file stem.
            let pkg = name.strip_suffix(".mo").unwrap_or(name);
            if exclude.iter().any(|p| pkg_matches(pkg, p)) {
                continue;
            }
        }
        if path.is_dir() {
            // Skip rumoca's own caches inside the MSL tree, if present.
            if matches!(name, ".cache" | "target" | "Resources") {
                continue;
            }
            collect_mo_files(root, &path, exclude, out);
        } else if path.extension().and_then(|s| s.to_str()) == Some("mo") {
            out.push(path);
        }
    }
}

/// Exact match, or prefix match when `pattern` ends in `*`.
fn pkg_matches(pkg: &str, pattern: &str) -> bool {
    match pattern.strip_suffix('*') {
        Some(prefix) => pkg.starts_with(prefix),
        None => pkg == pattern,
    }
}

fn pack(entries: &[(PathBuf, PathBuf)], dest: &Path) -> u64 {
    let file = File::create(dest).expect("create tmp tar");
    let buf = BufWriter::new(file);
    // Level 19 = strong compression; we're producing this once per build.
    let zstd_w = zstd::Encoder::new(buf, 19)
        .expect("zstd encoder")
        .auto_finish();
    let mut tar_w = tar::Builder::new(zstd_w);

    let mut total: u64 = 0;
    for (root, path) in entries {
        let mut f = File::open(path).expect("open .mo");
        let mut bytes = Vec::new();
        f.read_to_end(&mut bytes).expect("read .mo");
        total += bytes.len() as u64;

        let rel = rel_key(root, path);
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        // Zero out mtime so the bundle hash is reproducible across builds
        // of the same MSL tree.
        header.set_mtime(0);
        header.set_cksum();
        tar_w
            .append_data(&mut header, &rel, &bytes[..])
            .expect("tar append");
    }
    tar_w.finish().expect("tar finish");
    total
}

/// Bundle key for a file: its path relative to its own library root, with
/// forward slashes. This is the tar entry name AND the rumoca URI, and is
/// what the web resolver matches against (`MslInMemory.files` keys).
fn rel_key(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .expect("entry under its root")
        .to_string_lossy()
        .replace('\\', "/")
}

/// Discover third-party library roots under `cache_dir()`, mirroring native's
/// `discover_third_party_libs`: each direct cache subdirectory that contains a
/// `<Package>/package.mo` is a library root. Skips `msl` (the primary) and
/// dot-dirs. Returns the root dirs (the parent of the package dir), sorted.
fn discover_extra_roots() -> Vec<PathBuf> {
    let cache = lunco_assets::cache_dir();
    let mut roots: Vec<PathBuf> = Vec::new();
    let Ok(rd) = fs::read_dir(&cache) else {
        return roots;
    };
    for entry in rd.flatten() {
        let p = entry.path();
        if !p.is_dir() {
            continue;
        }
        let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
        if name == "msl" || name.starts_with('.') {
            continue;
        }
        let Ok(inner) = fs::read_dir(&p) else { continue };
        let has_pkg = inner.flatten().any(|sub| {
            let sp = sub.path();
            sp.is_dir() && sp.join("package.mo").is_file()
        });
        if has_pkg {
            roots.push(p);
        }
    }
    roots.sort();
    roots
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
