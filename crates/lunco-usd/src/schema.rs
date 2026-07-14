//! The **schema registry** — what USD's `UsdSchemaRegistry` is for, scoped to
//! what we can answer today.
//!
//! A USD property's *type*, its *variability* (`uniform` vs `varying`) and
//! whether it is `custom` are not properties of the authoring call. They are
//! declared by the prim's **schema**. Authoring code that decides them per call
//! site is guessing, and it guesses wrong: `info:id` and `physics:axis` are
//! `uniform` in their schemas and we authored them `varying` for exactly this
//! reason — nothing in the codebase knew any better.
//!
//! So this module is the one place that knows. It reads **real schemas** — every
//! one of them, ours and USD's, from the same kind of file, through the same
//! parser:
//!
//! 1. **`luncoSchema`** — our own types, from the embedded [`GENERATED_SCHEMA`]
//!    (`../schema/generatedSchema.usda`), generated from `../schema/schema.usda`,
//!    a real codeless USD schema registrable by any USD runtime through
//!    `../schema/plugInfo.json`.
//!
//! 2. **Core USD** — [`CORE_SCHEMAS`]: OpenUSD's own `generatedSchema.usda` for
//!    `usd`, `usdGeom`, `usdShade`, `usdLux` and `usdPhysics`, vendored verbatim
//!    under `../schema/core/` (see the README there for provenance).
//!
//! A `generatedSchema.usda` is just USDA, and we already parse USDA. Core USD's
//! schema definitions were never unavailable to us — they simply weren't *read*.
//!
//! ### What this replaced
//!
//! A hand-written `CORE_UNIFORM` table: ten core properties we happened to author,
//! each typed out with the variability someone had looked up. It was there because
//! "Rust `openusd` has no schema registry", which was true and beside the point —
//! the fork has no schema registry, but it has a USDA parser, and a schema is a
//! USDA file.
//!
//! The table's failure mode was silence. Author a `SetAttribute` for a core
//! `uniform` property nobody had added to it, and the property was written
//! `varying` — no error, just a subtly wrong layer. That is precisely how
//! `info:id` and `physics:axis` came to be authored wrong in the first place. The
//! registry now knows all 202 core properties, not the 10 we remembered.
//!
//! ## `custom`, and why we still don't guess it
//!
//! In USD, `custom` marks a property that **no schema declares**. Now that core is
//! loaded it is tempting to say "not in the registry ⇒ custom" — still a trap. We
//! vendor five core modules, not all of them (no `usdRender`, `usdSkel`, `usdMedia`,
//! no third-party schema a scene might legitimately apply). Absence from this
//! registry means "we don't know", which is not the same as "no schema declares it".
//!
//! So we assert `custom` only where we can *know* it: inside the `lunco:` namespace,
//! which is ours. A `lunco:` property that `luncoSchema` does not declare is genuinely
//! custom — and that is correct, not a defect. Per-model simulation parameters
//! (`lunco:voltage`, `lunco:capacity`, …) vary per Modelica model, so no schema can
//! declare them; `custom` is exactly the right encoding.

use std::collections::HashMap;
use std::sync::OnceLock;

use openusd::sdf::{self, SpecType};

/// The generated `luncoSchema` definitions — the file a USD runtime registers.
/// Embedded so the registry needs no filesystem (it must work on wasm).
pub const GENERATED_SCHEMA: &str = include_str!("../schema/generatedSchema.usda");

/// OpenUSD's own schema definitions, vendored verbatim under `../schema/core/`.
///
/// These are the *real* `generatedSchema.usda` files that ship with USD — the same
/// artifacts `usdGenSchema` produces for us. Embedded (not read from disk) because
/// the registry must work on wasm, and because a schema the binary disagrees with
/// is worse than no schema.
///
/// Only the modules we actually author against. Adding one is a copy, not code:
/// drop the file in and add a line here.
const CORE_SCHEMAS: &[(&str, &str)] = &[
    ("usd", include_str!("../schema/core/usd.usda")),
    ("usdGeom", include_str!("../schema/core/usdGeom.usda")),
    ("usdShade", include_str!("../schema/core/usdShade.usda")),
    ("usdLux", include_str!("../schema/core/usdLux.usda")),
    ("usdPhysics", include_str!("../schema/core/usdPhysics.usda")),
];

/// A `typeName` field is authored as a token; accept a plain string too rather
/// than silently dropping the property.
fn token_or_string(v: sdf::Value) -> Option<String> {
    match v {
        sdf::Value::Token(t) => Some(t.to_string()),
        sdf::Value::String(s) => Some(s),
        _ => None,
    }
}

/// What a schema declares about one property.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PropertySpec {
    /// The schema's declared USD type name (`"float"`, `"uniform token"` → `"token"`).
    pub type_name: String,
    /// `uniform` or `varying`, per the schema.
    pub variability: sdf::Variability,
    /// The schema that declares it — `"LuncoTerrainAPI"`, `"UsdShadeShader"`.
    pub declared_by: String,
}

/// The parsed `luncoSchema` plus the core `uniform` table.
#[derive(Debug, Default)]
pub struct SchemaRegistry {
    /// Keyed by property NAME, not `(schema, name)`.
    ///
    /// Sound because every property here is namespaced — `lunco:env:exposureEv100`,
    /// `physics:axis` — so a name identifies a property globally. It is the same
    /// reason the schema forbids bare names: a terrain layer's bare `size` would
    /// collide with `UsdGeomCube`'s real `double size` both in this map and in USD
    /// itself.
    properties: HashMap<String, PropertySpec>,
    /// Concrete typed schemas (`LuncoEnvironment`, `LuncoPolicy`).
    prim_types: Vec<String>,
    /// Applied API schemas (`LuncoTerrainAPI`, …).
    api_schemas: Vec<String>,
}

impl SchemaRegistry {
    /// The process-wide registry, parsed once.
    pub fn global() -> &'static SchemaRegistry {
        static REGISTRY: OnceLock<SchemaRegistry> = OnceLock::new();
        REGISTRY.get_or_init(SchemaRegistry::load)
    }

    /// Parse [`CORE_SCHEMAS`] and [`GENERATED_SCHEMA`] — every schema, one parser.
    ///
    /// Core goes in FIRST so that if `luncoSchema` ever declared a name core also
    /// declares, ours would win. It never should: the schema forbids bare names
    /// precisely so a `lunco:`-namespaced property cannot collide with a core one.
    ///
    /// A malformed embedded schema is a build-time bug, not a runtime condition —
    /// it cannot vary per user. It is still tolerated rather than panicking (a
    /// missing schema degrades to "everything varying", the old behaviour), and the
    /// tests below assert every one of them parses and yields the properties we
    /// author.
    fn load() -> SchemaRegistry {
        let mut reg = SchemaRegistry::default();

        for (_module, src) in CORE_SCHEMAS {
            // Core prim types / API schemas are NOT recorded in `prim_types` /
            // `api_schemas`: those answer "which types does *luncoSchema* define",
            // which is what our own registration and validation ask. Only the
            // property declarations are folded in.
            reg.ingest(src, false);
        }
        reg.ingest(GENERATED_SCHEMA, true);
        reg
    }

    /// Fold one `generatedSchema.usda` into the registry. `own` records the file's
    /// prim types and API schemas as *ours* (see [`load`](Self::load)).
    fn ingest(&mut self, src: &str, own: bool) {
        let reg = self;
        let Ok(data) = lunco_usd_bevy::author::usda_to_data(src) else {
            return;
        };

        for (path, spec) in data.iter() {
            match spec.ty {
                SpecType::Prim => {
                    if !own {
                        continue;
                    }
                    let Some(class) = path.as_str().strip_prefix('/') else {
                        continue;
                    };
                    if class.contains('/') {
                        continue;
                    }
                    // `customData = { token apiSchemaType = "singleApply" }` is what
                    // usdGenSchema writes to distinguish an applied API schema from a
                    // concrete typed one.
                    let is_api = matches!(
                        spec.get("customData"),
                        Some(sdf::Value::Dictionary(d)) if d.contains_key("apiSchemaType")
                    );
                    if is_api {
                        reg.api_schemas.push(class.to_string());
                    } else {
                        reg.prim_types.push(class.to_string());
                    }
                }
                SpecType::Attribute => {
                    // `/LuncoTerrainAPI.lunco:terrain:windowM`
                    //   → (`/LuncoTerrainAPI`, "lunco:terrain:windowM")
                    let Some((prim, name)) = path.split_property() else {
                        continue;
                    };
                    let Some(prim) = prim.name() else { continue };
                    let Some(type_name) = spec.get("typeName").cloned().and_then(token_or_string) else {
                        continue;
                    };
                    reg.properties.insert(
                        name.to_string(),
                        PropertySpec {
                            type_name,
                            // Unauthored ⇒ `varying`, USD's default. `uniform` is
                            // the only variability USDA actually writes out.
                            variability: match spec.get("variability") {
                                Some(sdf::Value::Variability(v)) => *v,
                                _ => sdf::Variability::Varying,
                            },
                            declared_by: prim.to_string(),
                        },
                    );
                }
                _ => {}
            }
        }
    }

    /// What a schema declares about property `name`, or `None` when no schema this
    /// registry knows declares it.
    pub fn property(&self, name: &str) -> Option<&PropertySpec> {
        self.properties.get(name)
    }

    /// The variability to author `name` with — the schema's, else USD's default
    /// (`varying`).
    pub fn variability(&self, name: &str) -> sdf::Variability {
        self.property(name)
            .map(|p| p.variability)
            .unwrap_or(sdf::Variability::Varying)
    }

    /// Whether `name` must be authored `custom`.
    ///
    /// True only for a `lunco:`-namespaced property that `luncoSchema` does not
    /// declare — the one case we can *know* is custom. See the module docs for why
    /// we don't generalise this to every unknown property.
    pub fn is_custom(&self, name: &str) -> bool {
        name.starts_with("lunco:") && !self.properties.contains_key(name)
    }

    /// The concrete typed schemas `luncoSchema` defines.
    pub fn prim_types(&self) -> &[String] {
        &self.prim_types
    }

    /// The applied API schemas `luncoSchema` defines.
    pub fn api_schemas(&self) -> &[String] {
        &self.api_schemas
    }
}

/// Variability to author `name` with, per the schema. Convenience over
/// [`SchemaRegistry::global`].
pub fn variability_of(name: &str) -> sdf::Variability {
    SchemaRegistry::global().variability(name)
}

/// Whether `name` must be authored `custom`. Convenience over
/// [`SchemaRegistry::global`].
pub fn is_custom(name: &str) -> bool {
    SchemaRegistry::global().is_custom(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The embedded generated schema must parse — it is compiled in, so a failure
    /// here is a build-time defect, and a silently-empty registry would quietly
    /// restore the "everything is varying" bug.
    #[test]
    fn generated_schema_parses_and_registers_every_type() {
        let reg = SchemaRegistry::global();
        assert!(
            reg.prim_types().contains(&"LuncoEnvironment".to_string()),
            "typed schemas: {:?}",
            reg.prim_types()
        );
        assert!(reg.prim_types().contains(&"LuncoPolicy".to_string()));
        for api in [
            "LuncoTerrainAPI",
            "LuncoTerrainLayerAPI",
            "LuncoShadowAPI",
            "LuncoMaterialAPI",
        ] {
            assert!(
                reg.api_schemas().contains(&api.to_string()),
                "{api} missing from {:?}",
                reg.api_schemas()
            );
        }
    }

    /// The whole point: variability comes from the schema, not the call site.
    #[test]
    fn variability_is_read_from_the_schema() {
        // Declared `uniform` in luncoSchema.
        assert_eq!(variability_of("lunco:cameraMode"), sdf::Variability::Uniform);
        assert_eq!(variability_of("lunco:policy:seam"), sdf::Variability::Uniform);
        assert_eq!(variability_of("lunco:layer"), sdf::Variability::Uniform);
        // Declared `varying` in luncoSchema.
        assert_eq!(
            variability_of("lunco:env:exposureEv100"),
            sdf::Variability::Varying
        );
        // Core USD, read from OpenUSD's OWN generatedSchema — including the two the
        // audit caught being authored `varying`.
        assert_eq!(variability_of("info:id"), sdf::Variability::Uniform);
        assert_eq!(variability_of("physics:axis"), sdf::Variability::Uniform);
        assert_eq!(variability_of("xformOpOrder"), sdf::Variability::Uniform);
        assert_eq!(variability_of("subdivisionScheme"), sdf::Variability::Uniform);
        assert_eq!(variability_of("purpose"), sdf::Variability::Uniform);
        // `physics:mass` is `varying` — and we now KNOW that, rather than defaulting
        // to it because we'd never heard of the property. The hand table this
        // replaced could not tell those two cases apart, which is exactly why a core
        // `uniform` property missing from it was authored wrong in silence.
        assert_eq!(variability_of("physics:mass"), sdf::Variability::Varying);
        assert!(SchemaRegistry::global().property("physics:mass").is_some());
        // Genuinely unknown (no schema we vendor declares it) → USD's default.
        assert_eq!(variability_of("nonesuch:madeUp"), sdf::Variability::Varying);
        assert!(SchemaRegistry::global().property("nonesuch:madeUp").is_none());
    }

    /// Core USD types come from the real schema too — not a table of what someone
    /// remembered, and not left blank for the call site to assert.
    #[test]
    fn core_schema_declares_property_types() {
        let reg = SchemaRegistry::global();
        assert_eq!(reg.property("physics:mass").unwrap().type_name, "float");
        assert_eq!(reg.property("xformOpOrder").unwrap().type_name, "token[]");
        assert_eq!(reg.property("radius").unwrap().type_name, "double");
        // …and each cites the schema that actually declares it.
        assert_eq!(reg.property("subdivisionScheme").unwrap().declared_by, "Mesh");
    }

    /// Types come from the schema too, so a scene authoring the wrong type can be
    /// caught rather than silently coerced.
    #[test]
    fn schema_declares_property_types() {
        let reg = SchemaRegistry::global();
        assert_eq!(reg.property("lunco:env:exposureEv100").unwrap().type_name, "float");
        assert_eq!(
            reg.property("lunco:env:earthshineColor").unwrap().type_name,
            "color3f"
        );
        assert_eq!(reg.property("lunco:layer:seed").unwrap().type_name, "int64");
        assert_eq!(
            reg.property("lunco:layer:colliderRing").unwrap().type_name,
            "bool"
        );
        assert_eq!(
            reg.property("lunco:terrain:horizonShadows").unwrap().type_name,
            "bool"
        );
    }

    /// `custom` is asserted only where we can know it — inside our own namespace.
    /// Claiming it for core properties we simply don't have schemas for would be a
    /// lie, and this pins that distinction.
    #[test]
    fn custom_is_claimed_only_within_our_namespace() {
        // Declared by luncoSchema → NOT custom.
        assert!(!is_custom("lunco:env:exposureEv100"));
        assert!(!is_custom("lunco:layer:windowM"));
        assert!(!is_custom("lunco:terrain:horizonShadows"));
        // Ours, but no schema declares it — a per-model Modelica param. Genuinely
        // custom, and that is by design, not an oversight.
        assert!(is_custom("lunco:voltage"));
        assert!(is_custom("lunco:capacity"));
        // Core USD schema properties this registry has no schema for. We must NOT
        // call these custom.
        assert!(!is_custom("physics:mass"));
        assert!(!is_custom("inputs:intensity"));
        assert!(!is_custom("primvars:displayColor"));
    }
}
