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
//! So this module is the one place that knows. It has two sources, and the split
//! matters:
//!
//! 1. **`luncoSchema`** — our own types, parsed at first use from the embedded
//!    [`GENERATED_SCHEMA`] (`../schema/generatedSchema.usda`). That file is
//!    generated from `../schema/schema.usda`, which is a real, codeless USD
//!    schema — registrable by any USD runtime through `../schema/plugInfo.json`.
//!    It is authoritative: to add a property, edit the schema, not this code.
//!
//! 2. **[`CORE_UNIFORM`]** — the handful of *core* USD properties we author that
//!    are `uniform`. Core schemas live in OpenUSD, not here, and Rust `openusd`
//!    has no schema registry to load them from, so this is a curated table. Each
//!    entry cites the schema that declares it.
//!
//! ## `custom`, and why we don't guess it
//!
//! In USD, `custom` marks a property that **no schema declares**. It would be
//! tempting to say "not in the registry ⇒ custom", but that is a trap: this
//! registry does not know the core schemas, so `physics:mass` and
//! `inputs:intensity` are absent from it despite being perfectly ordinary schema
//! properties. Marking them `custom` would be a lie.
//!
//! We therefore assert `custom` only where we can *know* it: inside the `lunco:`
//! namespace, which is ours. A `lunco:` property that `luncoSchema` does not
//! declare is genuinely custom — and that is not a defect. Per-model simulation
//! parameters (`lunco:voltage`, `lunco:capacity`, …) vary per Modelica model, so
//! no schema can declare them; `custom` is precisely the right encoding.
//!
//! Outside `lunco:`, we leave `custom` unauthored (i.e. non-custom, USD's
//! default), which is correct for every core property we author.
//!
//! TODO(openusd): when the fork grows a real `UsdSchemaRegistry` that loads
//! `plugInfo.json` (ours *and* core), [`CORE_UNIFORM`] and this hand-rolled
//! parse both collapse into a delegation to it.

use std::collections::HashMap;
use std::sync::OnceLock;

use openusd::sdf::{self, SpecType};

/// The generated `luncoSchema` definitions — the file a USD runtime registers.
/// Embedded so the registry needs no filesystem (it must work on wasm).
pub const GENERATED_SCHEMA: &str = include_str!("../schema/generatedSchema.usda");

/// Core USD properties we author that their schema declares `uniform`.
///
/// Not exhaustive over core USD — exhaustive over *what LunCoSim authors*. Adding
/// a `SetAttribute` for a new core `uniform` property means adding it here, or it
/// is silently authored `varying`, which is the bug this table exists to close.
const CORE_UNIFORM: &[(&str, &str)] = &[
    ("info:id", "UsdShadeShader"),
    ("info:implementationSource", "UsdShadeShader"),
    ("xformOpOrder", "UsdGeomXformable"),
    ("purpose", "UsdGeomImageable"),
    ("subdivisionScheme", "UsdGeomMesh"),
    ("physics:axis", "UsdPhysicsRevoluteJoint / PrismaticJoint"),
    ("physics:approximation", "UsdPhysicsMeshCollisionAPI"),
    ("physics:excludeFromArticulation", "UsdPhysicsJoint"),
    ("physics:startsAsleep", "UsdPhysicsRigidBodyAPI"),
    ("inputs:texture:format", "UsdLuxDomeLight"),
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

    /// Parse [`GENERATED_SCHEMA`] and fold in [`CORE_UNIFORM`].
    ///
    /// A malformed generated schema is a build-time bug, not a runtime condition
    /// — it is embedded, so it cannot vary per user. It is still tolerated rather
    /// than panicking (an empty registry degrades to "everything varying", i.e.
    /// today's behaviour), and the unit tests below assert it parses.
    fn load() -> SchemaRegistry {
        let mut reg = SchemaRegistry::default();

        for (name, schema) in CORE_UNIFORM {
            reg.properties.insert(
                (*name).to_string(),
                PropertySpec {
                    // Core types are declared by the caller (SetAttribute carries a
                    // type_name); we only claim variability for them.
                    type_name: String::new(),
                    variability: sdf::Variability::Uniform,
                    declared_by: (*schema).to_string(),
                },
            );
        }

        let Ok(data) = lunco_usd_bevy::author::usda_to_data(GENERATED_SCHEMA) else {
            return reg;
        };

        for (path, spec) in data.iter() {
            match spec.ty {
                SpecType::Prim => {
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
        reg
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
        assert_eq!(variability_of("lunco:material:type"), sdf::Variability::Uniform);
        assert_eq!(variability_of("lunco:policy:seam"), sdf::Variability::Uniform);
        assert_eq!(variability_of("lunco:layer"), sdf::Variability::Uniform);
        // Declared `varying` in luncoSchema.
        assert_eq!(
            variability_of("lunco:env:exposureEv100"),
            sdf::Variability::Varying
        );
        // Core USD, from CORE_UNIFORM — the two the audit caught being authored
        // `varying`.
        assert_eq!(variability_of("info:id"), sdf::Variability::Uniform);
        assert_eq!(variability_of("physics:axis"), sdf::Variability::Uniform);
        // Unknown → USD's default.
        assert_eq!(variability_of("physics:mass"), sdf::Variability::Varying);
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
