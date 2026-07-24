//! Every shipped `*.usda` must parse.
//!
//! This is the cheapest possible guard on the asset library, and it exists because
//! it was needed: a scripted migration inserted an attribute into a prim's
//! `variants = { … }` metadata dict instead of its body, and `skid_rover.usda`
//! stopped parsing. Exactly one unrelated test happened to load that one file and
//! caught it — which is luck, not cover. A corrupt asset should fail loudly here,
//! naming the file, not surface as a mystery composition error in whatever test
//! happens to touch it.

use std::path::{Path, PathBuf};

/// Every `.usda` under the engine asset library.
fn usda_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let p = entry.path();
        if p.is_dir() {
            usda_files(&p, out);
        } else if p.extension().and_then(|s| s.to_str()) == Some("usda") {
            out.push(p);
        }
    }
}

#[test]
fn every_shipped_usda_parses() {
    let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
    let mut files = Vec::new();
    usda_files(&assets, &mut files);
    files.sort();

    assert!(
        files.len() > 50,
        "expected to find the asset library at {} — found only {} files, so this \
         test is not actually checking anything",
        assets.display(),
        files.len()
    );

    let mut failures = Vec::new();
    for path in &files {
        let Ok(src) = std::fs::read_to_string(path) else {
            failures.push(format!("{}: unreadable", path.display()));
            continue;
        };
        if let Err(e) = openusd::usda::parse(&src) {
            failures.push(format!("{}: {e}", path.display()));
        }
    }

    assert!(
        failures.is_empty(),
        "{} of {} shipped .usda files do not parse:\n  {}",
        failures.len(),
        files.len(),
        failures.join("\n  ")
    );
}
