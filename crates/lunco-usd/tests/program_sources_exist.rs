//! Every program names a file that is there.
//!
//! A program's source is an `asset`, so it is a real reference: the resolver sees it,
//! packaging carries it, and this test can check it. A binding that names a file which
//! does not exist is a vehicle with no behaviour and no error — and on a
//! case-sensitive filesystem, `battery.mo` and `Battery.mo` are not the same file.

use std::path::{Path, PathBuf};

fn assets_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("assets")
}

fn usda_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            usda_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "usda") {
            out.push(path);
        }
    }
}

#[test]
fn every_program_source_asset_exists() {
    let root = assets_root();
    let mut files = Vec::new();
    usda_files(&root, &mut files);
    assert!(!files.is_empty(), "found no .usda under assets/");

    let mut missing = Vec::new();
    for file in &files {
        let Ok(text) = std::fs::read_to_string(file) else {
            continue;
        };
        for (n, line) in text.lines().enumerate() {
            // A comment explains; it does not bind.
            let code = line.split('#').next().unwrap_or("");
            if !code.contains("info:sourceAsset") {
                continue;
            }
            // `uniform asset info:sourceAsset = @models/Lander.mo@`
            let Some(rel) = code
                .split_once('@')
                .and_then(|(_, rest)| rest.split_once('@'))
                .map(|(rel, _)| rel)
                .filter(|rel| !rel.is_empty())
            else {
                continue;
            };
            // Both spellings resolve to the same place and both are correct:
            //
            //   @models/Battery.mo@         — asset-root relative
            //   @lunco://models/Battery.mo@ — the asset-root SCHEME
            //
            // The scheme is the location-independent one (`register_lunco_asset_sources`
            // registers it, and `asset_server.load` honours it), so it is what a shipped
            // asset should use when the scene may be mounted from a twin — a bare path
            // would then resolve against the twin instead of the engine's assets. Strip
            // it and check the same file either way; anything else here is a real path.
            let rel = rel.strip_prefix("lunco://").unwrap_or(rel);
            if !root.join(rel).exists() {
                missing.push(format!(
                    "{}:{} names `{}`, which is not under assets/",
                    file.display(),
                    n + 1,
                    rel,
                ));
            }
        }
    }

    assert!(
        missing.is_empty(),
        "{} program source(s) name a file that is not there:\n\n{}\n",
        missing.len(),
        missing.join("\n"),
    );
}
