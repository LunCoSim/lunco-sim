//! The spawn/catalog metadata a `*.usda` authors on its own default prim —
//! read with **openusd's real parser**, on every platform.
//!
//! The catalog reads bytes through `lunco-assets` and parses them with the same
//! USD parser used by native and browser consumers:
//!
//! - **One parser** — the real one. `bool lunco:spawnable` is read as a `bool`,
//!   and a description containing an `=`, a quote, or a newline parses correctly
//!   as authored.
//! - **No generated metadata table.** The catalog always observes the asset
//!   bytes that it is asked to load.
//!
//! # The properties are real USD now
//!
//! `lunco:spawnable` is declared by **`LunCoCatalogAPI`**
//! (`lunco-usd/schema/schema.usda`), applied to the asset's default prim. It is
//! an explicit opt-in; placement is derived from standard `UsdPhysics` collision
//! geometry by the editor.
//!
//! `lunco:description` is not declared. USD already has this field: every prim
//! carries `doc` metadata, the standard description shown by usdview and other
//! USD tools.

use lunco_usd_bevy::DefaultPrim;

/// Spawn metadata authored on a `*.usda`'s default prim.
#[derive(Debug, Clone, PartialEq)]
pub struct SpawnMeta {
    /// `bool lunco:spawnable` — whether the file is a spawnable part.
    ///
    /// **Opt-in.** Default `false`: a file is offered in the palette only if it
    /// says it is a part.
    pub spawnable: bool,
    /// The prim's **`doc` metadata** — the blurb shown as a palette/Scenarios
    /// tooltip.
    ///
    /// USD's standard `doc` field, visible to usdview and other USD tools.
    pub description: Option<String>,
}

impl Default for SpawnMeta {
    fn default() -> Self {
        SpawnMeta {
            spawnable: false,
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
        // Typed `bool`, declared by `LunCoCatalogAPI` (see
        // lunco-usd/schema/schema.usda).
        spawnable: prim.scalar::<bool>("lunco:spawnable").unwrap_or(false),
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
    prepend apiSchemas = ["LunCoCatalogAPI"]
)
{
    uniform bool lunco:spawnable = true
}
"#;

    #[test]
    fn reads_typed_metadata_off_the_default_prim() {
        let m = parse_spawn_meta(SRC);
        assert!(m.spawnable);
        assert_eq!(m.description.as_deref(), Some("A rover."));
    }

    #[test]
    fn spawnable_defaults_to_false_when_unstated() {
        let src = "#usda 1.0\n(\n    defaultPrim = \"X\"\n)\n\ndef Xform \"X\"\n{\n}\n";
        let m = parse_spawn_meta(src);
        assert!(!m.spawnable);
        assert_eq!(m.description, None);
    }

    #[test]
    fn unparseable_source_is_not_spawnable() {
        assert_eq!(parse_spawn_meta("not usd at all"), SpawnMeta::default());
    }

    /// Punctuation in the `doc` value is preserved by USD parsing.
    #[test]
    fn description_survives_an_equals_sign() {
        let src = "#usda 1.0\n(\n    defaultPrim = \"X\"\n)\n\ndef Xform \"X\" (\n    doc = \"Set thrust = 1, then go.\"\n)\n{\n}\n";
        assert_eq!(
            parse_spawn_meta(src).description.as_deref(),
            Some("Set thrust = 1, then go.")
        );
    }

    /// A **multi-line** description. openusd's parser handles the triple-quoted
    /// form natively.
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
