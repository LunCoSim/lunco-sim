//! **Schema generation** — `schema.usda` (authored) → `generatedSchema.usda`
//! (registered/parsed).
//!
//! `schema.usda` is THE authored source. `generatedSchema.usda` is what
//! `plugInfo.json` registers for external USD runtimes and what
//! [`crate::schema`]'s registry ingests at runtime — and it is GENERATED, never
//! hand-edited. Historically it was hand-copied ("regenerate with
//! `usdGenSchema`", which nobody ran, or `scripts/gen_schema.py`, which was
//! never written), and the two drifted in the worst direction: ~75 authored
//! `lunco:` attributes existed in Rust with no declaration, serializing as
//! untyped `custom`.
//!
//! ## The transformation
//!
//! Exactly what `usdGenSchema` does to a codeless schema, learned from the
//! delta between the two checked-in files and reproduced byte-for-byte:
//!
//! 1. the authored **layer metadata block** (doc comment + `subLayers`) is
//!    replaced by the fixed generated header ([`GENERATED_HEADER`]);
//! 2. every **`inherits = </…>` line** is dropped from class metadata — the
//!    generated form is flat; base-class identity lives in `plugInfo.json`'s
//!    `bases` instead;
//! 3. everything else — classes, properties, fallbacks, `customData`
//!    (`apiSchemaType`, UI hints, `lunco:unit`), `allowedTokens`, docs, and the
//!    explanatory `#` comments between classes — carries over verbatim.
//!
//! The emitter is deliberately textual (a parser round-trip would destroy the
//! comments and docstring formatting the schema file is half made of); the
//! **verifier is the real parser**: the output must parse through
//! `lunco_usd_bevy::author::usda_to_data` — the exact function the runtime
//! registry ingests it with — and declare the same classes the source does.
//!
//! ## Staleness gate
//!
//! `tests/schema_generation.rs::generated_schema_is_in_sync` regenerates in
//! memory and asserts byte equality with the checked-in file. Regenerate with:
//!
//! ```text
//! LUNCO_REGEN_SCHEMA=1 cargo test -p lunco-usd --test schema_generation
//! ```
//!
//! (An env-var-gated rewrite inside the test, not a `[[bin]]`: a bin harness
//! poisons the crate's settings — see the settings-poisoning lore.)

use std::collections::BTreeSet;

/// The fixed layer header of the generated file — the ONE part not carried
/// over from the source, since the source's header documents authoring and the
/// generated file must document that it is generated.
const GENERATED_HEADER: &str = r#"#usda 1.0
(
    "Generated schema for luncoSchema"
    """
    GENERATED — do not edit. Regenerate with
        LUNCO_REGEN_SCHEMA=1 cargo test -p lunco-usd --test schema_generation
    after editing `schema.usda`, which is the authoritative source
    (`lunco_usd::schema_gen::generate_schema` is the generator).

    This is the file a USD runtime actually registers (via `plugInfo.json`), and
    the file `lunco_usd::schema` parses to answer "what does this property's
    schema declare?" — its type, its variability, its UI hint, and whether it is
    `custom`.
    """
)
"#;

/// Generate `generatedSchema.usda` text from the authored `schema.usda` text.
///
/// Pure — no filesystem. Errors are strings meant for a failing test/CI log:
/// they name what is malformed rather than panicking.
pub fn generate_schema(authored: &str) -> Result<String, String> {
    let body = strip_layer_metadata(authored)?;

    let mut out = String::with_capacity(GENERATED_HEADER.len() + body.len());
    out.push_str(GENERATED_HEADER);
    for line in body.split_inclusive('\n') {
        if is_inherits_line(line) {
            continue;
        }
        out.push_str(line);
    }
    // `split_inclusive` preserves the source's trailing-newline state exactly;
    // nothing to fix up here.

    verify(authored, &out)?;
    Ok(out)
}

/// Everything after the authored layer-metadata block: skip the `#usda 1.0`
/// line and the `( … )` block that follows it, returning the rest verbatim
/// (starting with the newline that separates the block from the first class).
fn strip_layer_metadata(authored: &str) -> Result<&str, String> {
    let after_magic = authored
        .strip_prefix("#usda 1.0\n")
        .ok_or("schema.usda must start with `#usda 1.0`")?;
    if !after_magic.starts_with("(\n") {
        return Err("schema.usda must open a layer-metadata block on line 2".into());
    }
    // The block closes at the first `)` at column 0 — nothing inside layer
    // metadata (docstring, subLayers) puts a bare `)` at column 0.
    let close = after_magic
        .find("\n)\n")
        .ok_or("unterminated layer-metadata block (no `)` at column 0)")?;
    Ok(&after_magic[close + "\n)\n".len()..])
}

/// A class-metadata `inherits = </APISchemaBase>` / `</Typed>` line — the one
/// line kind the generated form drops. Matched narrowly (whole trimmed line)
/// so prose inside docstrings can never be eaten.
fn is_inherits_line(line: &str) -> bool {
    let t = line.trim();
    t.strip_prefix("inherits = </")
        .and_then(|rest| rest.strip_suffix('>'))
        .is_some_and(|name| !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric()))
}

/// The generated text must be REAL, REGISTRABLE USDA: parse it through the
/// exact function the runtime registry ingests it with, and cross-check that
/// it declares precisely the classes the source declares.
fn verify(authored: &str, generated: &str) -> Result<(), String> {
    let data = lunco_usd_bevy::author::usda_to_data(generated)
        .map_err(|e| format!("generated schema does not parse as USDA: {e:?}"))?;

    // Root prim specs of the generated layer, via the parser (not text).
    let parsed_classes: BTreeSet<String> = data
        .iter()
        .filter(|(_, spec)| spec.ty == openusd::sdf::SpecType::Prim)
        .filter_map(|(path, _)| {
            let class = path.as_str().strip_prefix('/')?;
            (!class.contains('/') && class.starts_with("LunCo")).then(|| class.to_string())
        })
        .collect();

    // Class names of the SOURCE, by line scan — the authored file is the one
    // form the runtime parser never consumes (its `inherits`/`subLayers` are
    // authoring-side), so its class list is read the same way
    // `schema::tests::source_and_generated_schema_declare_the_same_classes`
    // reads it.
    let source_classes: BTreeSet<String> = authored
        .lines()
        .filter_map(|line| {
            // `class "LunCoFooAPI" (` and `class LunCoFoo "LunCoFoo" (`
            let rest = line.strip_prefix("class ")?;
            let start = rest.find('"')? + 1;
            let end = rest[start..].find('"')? + start;
            Some(rest[start..end].to_string())
        })
        .filter(|n| n.starts_with("LunCo"))
        .collect();

    if source_classes.is_empty() {
        return Err("no LunCo classes found in schema.usda".into());
    }
    if parsed_classes != source_classes {
        let missing: Vec<_> = source_classes.difference(&parsed_classes).collect();
        let extra: Vec<_> = parsed_classes.difference(&source_classes).collect();
        return Err(format!(
            "generated schema class set diverges from source — lost in generation: \
             {missing:?}; invented by generation: {extra:?}"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inherits_lines_are_matched_narrowly() {
        assert!(is_inherits_line("    inherits = </APISchemaBase>\n"));
        assert!(is_inherits_line("    inherits = </Typed>"));
        // Prose and near-misses must survive.
        assert!(!is_inherits_line("    the class inherits = </Typed> style"));
        assert!(!is_inherits_line("    inherits = </>"));
        assert!(!is_inherits_line("    inherits = </A/B>"));
    }

    /// The generator run on the checked-in source must reproduce the checked-in
    /// generated file byte-for-byte. (The integration test in
    /// `tests/schema_generation.rs` is the regen-capable gate; this pins the
    /// same fact where `cargo test -p lunco-usd` alone will see it.)
    #[test]
    fn generator_reproduces_checked_in_generated_schema() {
        let generated = generate_schema(include_str!("../schema/schema.usda"))
            .expect("schema.usda must generate");
        assert!(
            generated == include_str!("../schema/generatedSchema.usda"),
            "generatedSchema.usda is stale — run \
             `LUNCO_REGEN_SCHEMA=1 cargo test -p lunco-usd --test schema_generation`"
        );
    }
}
