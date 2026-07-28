//! The staleness gate for the schema pipeline: `schema.usda` is authored,
//! `generatedSchema.usda` is generated from it by
//! [`lunco_usd::schema_gen::generate_schema`], and this test is what keeps the
//! checked-in copy honest — the runtime registers and parses ONLY the generated
//! file, so a stale one silently un-declares whatever was added to the source.
//!
//! Regenerate with:
//!
//! ```text
//! LUNCO_REGEN_SCHEMA=1 cargo test -p lunco-usd --test schema_generation
//! ```
//!
//! The regen path lives inside this test rather than a `[[bin]]` on purpose: a
//! bin harness in this workspace poisons the crate's settings (known trap).

use std::path::PathBuf;

fn schema_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("schema")
}

#[test]
fn generated_schema_is_in_sync() {
    // Read from disk, not `include_str!`: the regen arm rewrites the file, and
    // a compile-time embed would compare against the pre-rewrite bytes.
    let authored =
        std::fs::read_to_string(schema_dir().join("schema.usda")).expect("read schema/schema.usda");
    let checked_in_path = schema_dir().join("generatedSchema.usda");
    let checked_in =
        std::fs::read_to_string(&checked_in_path).expect("read schema/generatedSchema.usda");

    let generated = lunco_usd::schema_gen::generate_schema(&authored)
        .expect("schema.usda must generate (it is the authored source of the registry)");

    if std::env::var_os("LUNCO_REGEN_SCHEMA").is_some() {
        if generated != checked_in {
            std::fs::write(&checked_in_path, &generated)
                .expect("rewrite schema/generatedSchema.usda");
            eprintln!(
                "[schema_generation] rewrote {} from schema.usda",
                checked_in_path.display()
            );
        } else {
            eprintln!("[schema_generation] generatedSchema.usda already in sync");
        }
        return;
    }

    assert!(
        generated == checked_in,
        "schema/generatedSchema.usda is STALE — it is generated from schema.usda \
         and never hand-edited. Run:\n\n    \
         LUNCO_REGEN_SCHEMA=1 cargo test -p lunco-usd --test schema_generation\n\n\
         and commit the rewritten file."
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
    let authored =
        std::fs::read_to_string(schema_dir().join("schema.usda")).expect("read schema/schema.usda");
    let generated =
        lunco_usd::schema_gen::generate_schema(&authored).expect("schema.usda must generate");
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
