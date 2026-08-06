//! The staleness gate for the schema pipeline: `schema.usda` is authored and
//! `scripts/gen_schema.py` derives `generatedSchema.usda` plus `plugInfo.json`.
//! This test keeps both checked-in artifacts honest — the runtime registers and
//! parses ONLY the generated file, so a stale one silently un-declares whatever
//! was added to the source.
//!
//! Verify with:
//!
//! ```text
//! python3 scripts/gen_schema.py --check
//! ```

use std::path::PathBuf;

fn schema_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("schema")
}

#[test]
fn generated_schema_is_in_sync() {
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let status = std::process::Command::new("python3")
        .args(["scripts/gen_schema.py", "--check"])
        .current_dir(workspace)
        .status()
        .expect("python3 is required to verify the generated USD schema artifacts");
    assert!(
        status.success(),
        "schema artifacts are stale; regenerate them with python3 scripts/gen_schema.py"
    );
}

/// Every class the schema declares must be registered in `plugInfo.json` — the
/// generated file is what external USD runtimes load THROUGH plugInfo, so a
/// class added to the schema but not the manifest exists here and nowhere else.
/// (The registry-side twin of this check lives in `src/schema.rs`; this one
/// runs against the freshly generated text, so it fails in the same run that
/// regenerates.)
#[test]
fn every_generated_class_is_in_pluginfo() {
    let generated = std::fs::read_to_string(schema_dir().join("generatedSchema.usda"))
        .expect("read schema/generatedSchema.usda");
    let plug_info = std::fs::read_to_string(schema_dir().join("plugInfo.json"))
        .expect("read schema/plugInfo.json");

    let mut checked = 0usize;
    for line in generated.lines() {
        let Some(rest) = line.strip_prefix("class ") else {
            continue;
        };
        let Some(start) = rest.find('"') else {
            continue;
        };
        let Some(end) = rest[start + 1..].find('"') else {
            continue;
        };
        let name = &rest[start + 1..start + 1 + end];
        if !name.starts_with("LunCo") {
            continue;
        }
        checked += 1;
        assert!(
            plug_info.contains(&format!("\"{name}\"")),
            "{name} is declared in the schema but missing from plugInfo.json — \
             no external USD runtime can resolve it"
        );
    }
    assert!(checked > 20, "implausibly few classes scanned ({checked})");
}
