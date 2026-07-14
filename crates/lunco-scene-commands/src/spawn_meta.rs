//! The spawn/catalog metadata a `*.usda` authors on its own default prim —
//! read with **openusd's real parser**, on every platform.
//!
//! # What this used to be
//!
//! A hand-rolled line scan (`line.split_once("lunco:spawnable")`), duplicated
//! three ways, and a `build.rs` that baked the results of that scan into the
//! binary as `BAKED_SPAWN_META` / `BAKED_DESCRIPTIONS` tables for the web.
//!
//! Every part of that existed to work around one thing: **a build script cannot
//! depend on the crate it builds**, so the bake could not use the USD stack, so
//! it needed a parser that was not the USD stack — and then native had to use
//! that same weaker parser too, or the two platforms would disagree about what a
//! file said about itself. (They already had: the description was once read one
//! way natively and baked another.)
//!
//! # Why it is gone
//!
//! We *ship* the assets. The browser can read them — over HTTP, from the same
//! bundle, at the same URL Bevy's `AssetServer` uses to load the file when it is
//! spawned. Reading them is [`lunco_assets::asset_read::read_asset_bytes`]; the
//! only thing the bake ever bought was skipping that read.
//!
//! So there is now ONE path: fetch the bytes, hand them to openusd. Which means:
//!
//! - **One parser** — the real one. `bool lunco:spawnable` reads as a `bool`,
//!   `float lunco:spawnLift` as a float, and a description containing an `=`, a
//!   quote, or a newline parses correctly instead of being mangled by a scan.
//! - **No stale table.** The bake was a *copy* of the assets' contents compiled
//!   into the binary. Edit an asset without rebuilding and the web silently
//!   served the old metadata; ship an asset the bake never saw and it was, by
//!   its own fallback, "not spawnable".
//! - **No `build.rs`** in this crate at all.
//!
//! # The properties are real USD now
//!
//! `lunco:spawnable` and `lunco:spawnLift` are declared by **`LuncoCatalogAPI`**
//! (`lunco-usd/schema/schema.usda`), applied to the asset's default prim. They were
//! undeclared names, and the assets disagreed about admitting it — `custom bool
//! lunco:spawnable` was honest, while `float lunco:spawnLift` was authored *without*
//! `custom`, claiming a schema that did not exist.
//!
//! `lunco:description` is **deleted**, not declared. USD already has this field: every
//! prim carries `doc` metadata — the standard "what is this thing" string that usdview
//! and every other DCC display. Inventing a `lunco:` attribute to hold exactly what
//! `doc` holds is the same mistake as `inputs:reflectance` (USD had `inputs:ior`) and
//! `primvars:materialType` (never primvar data). Declaring it would have made the
//! invention official instead of fixing it. It is now `doc = "..."`.

use lunco_usd_bevy::DefaultPrim;

/// Spawn metadata authored on a `*.usda`'s default prim.
#[derive(Debug, Clone, PartialEq)]
pub struct SpawnMeta {
    /// `bool lunco:spawnable` — whether the file is a spawnable part.
    ///
    /// **Opt-in.** Default `false`: a file is offered in the palette only if it
    /// says it is a part. This used to default to `true`, which meant the
    /// catalogue offered *every* USD asset in the project and every scene, mission
    /// and scenario had to remember to disclaim it — and three scenes had silently
    /// forgotten to, so they showed up as spawnable parts. A default that must be
    /// disclaimed everywhere is a default that leaks.
    pub spawnable: bool,
    /// `float lunco:spawnLift` — metres to lift the spawn point.
    pub lift: f32,
    /// The prim's **`doc` metadata** — the blurb shown as a palette/Scenarios
    /// tooltip.
    ///
    /// USD's own field, not ours. This was `custom string lunco:description`: a
    /// bespoke attribute invented to hold precisely what `doc` already holds, and
    /// which — being ours — no other tool could see. usdview shows `doc`.
    pub description: Option<String>,
}

impl Default for SpawnMeta {
    fn default() -> Self {
        SpawnMeta {
            spawnable: false,
            lift: 0.0,
            description: None,
        }
    }
}

/// Parse the catalog metadata out of a `*.usda`'s source.
///
/// A file that doesn't parse, or that declares no `defaultPrim`, yields
/// [`SpawnMeta::default`] — i.e. *not* spawnable. Unreadable is not a licence to
/// guess: a file that cannot state it is a part is not offered as one.
pub fn parse_spawn_meta(src: &str) -> SpawnMeta {
    let Some(prim) = DefaultPrim::parse(src) else {
        return SpawnMeta::default();
    };
    SpawnMeta {
        // Typed: `bool`, not the string "true". The scan this replaces accepted
        // `true` or `1` textually and would equally have accepted `truthy`.
        // Both are declared by `LuncoCatalogAPI` (see lunco-usd/schema/schema.usda).
        spawnable: prim.scalar::<bool>("lunco:spawnable").unwrap_or(false),
        lift: prim.real_f32("lunco:spawnLift").unwrap_or(0.0),
        // USD's `doc` prim metadata — NOT an attribute of ours. See the field doc.
        description: prim.documentation(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = r#"#usda 1.0
(
    defaultPrim = "Rover"
)

def Xform "Rover" (
    doc = "A rover."
    prepend apiSchemas = ["LuncoCatalogAPI"]
)
{
    uniform bool lunco:spawnable = true
    float lunco:spawnLift = 1.5
}
"#;

    #[test]
    fn reads_typed_metadata_off_the_default_prim() {
        let m = parse_spawn_meta(SRC);
        assert!(m.spawnable);
        assert_eq!(m.lift, 1.5);
        assert_eq!(m.description.as_deref(), Some("A rover."));
    }

    #[test]
    fn spawnable_defaults_to_false_when_unstated() {
        let src = "#usda 1.0\n(\n    defaultPrim = \"X\"\n)\n\ndef Xform \"X\"\n{\n}\n";
        let m = parse_spawn_meta(src);
        assert!(!m.spawnable);
        assert_eq!(m.lift, 0.0);
        assert_eq!(m.description, None);
    }

    #[test]
    fn unparseable_source_is_not_spawnable() {
        assert_eq!(parse_spawn_meta("not usd at all"), SpawnMeta::default());
    }

    /// `double lunco:spawnLift` must not be silently dropped for being authored
    /// in the other precision — the `real_f32` rule (see `UsdRead::real_f32`).
    #[test]
    fn lift_tolerates_double_authoring() {
        let src = "#usda 1.0\n(\n    defaultPrim = \"X\"\n)\n\ndef Xform \"X\"\n{\n    double lunco:spawnLift = 2\n}\n";
        assert_eq!(parse_spawn_meta(src).lift, 2.0);
    }

    /// The line scan this replaces split on `=` and took the first quoted run on
    /// the *line*, so a description containing an `=` came back truncated. A real
    /// parse treats the value as a value.
    #[test]
    fn description_survives_an_equals_sign() {
        let src = "#usda 1.0\n(\n    defaultPrim = \"X\"\n)\n\ndef Xform \"X\" (\n    doc = \"Set thrust = 1, then go.\"\n)\n{\n}\n";
        assert_eq!(
            parse_spawn_meta(src).description.as_deref(),
            Some("Set thrust = 1, then go.")
        );
    }

    /// A **multi-line** description. The line scan could not represent one at all
    /// — it read a line at a time, and its own doc said so ("a multi-line string
    /// value is not supported, which the tests pin rather than leave to be
    /// discovered"). openusd's parser handles the triple-quoted form natively.
    ///
    /// Note openusd's USDA dialect takes NO backslash escapes: the lexer keeps the
    /// raw bytes between the delimiters, and its writer correspondingly never emits
    /// an escape — it picks a delimiter the content cannot close (see
    /// `usda::writer::write_quoted`). Reader and writer agree, so a quote inside a
    /// description is expressed by the delimiter choice, exactly as here.
    #[test]
    fn description_can_span_lines_and_contain_quotes() {
        let src = "#usda 1.0\n(\n    defaultPrim = \"X\"\n)\n\ndef Xform \"X\" (\n    doc = \"\"\"Line one.\nThen \"go\".\"\"\"\n)\n{\n}\n";
        assert_eq!(
            parse_spawn_meta(src).description.as_deref(),
            Some("Line one.\nThen \"go\".")
        );
    }
}
