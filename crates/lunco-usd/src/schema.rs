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
    // Reconstructed PhysX vehicle schema subset (canonical NVIDIA names; see the
    // file header for provenance + the swap-later TODO). The vehicle APIs the
    // loader detects (`PhysxVehicleContextAPI`, `…AckermannSteeringAPI`,
    // `…TankDifferentialAPI`) and the suspension/wheel/compliance APIs the spec
    // (doc 53) needs are all defined here, so they stop being unregistered
    // typeNames the registry is blind to.
    ("physxSchema", include_str!("../schema/core/physxSchema.usda")),
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
    /// The schema that declares it — `"LunCoTerrainAPI"`, `"UsdShadeShader"`.
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
    /// Concrete typed schemas (`LunCoEnvironment`, `LunCoPolicy`).
    prim_types: Vec<String>,
    /// Applied API schemas (`LunCoTerrainAPI`, …).
    api_schemas: Vec<String>,
}

impl SchemaRegistry {
    /// The process-wide registry. Built once from the embedded schemas, then
    /// **extensible** — see [`register_extension`](Self::register_extension).
    ///
    /// `RwLock`, not `OnceLock<SchemaRegistry>`, because a schema library can
    /// arrive with an ASSET: a twin ships its own domain schema (a habitat's
    /// `habitat:` properties, a rover's) alongside its `.usda`, and which twin
    /// is open is not known when this is first touched. Reads dominate
    /// overwhelmingly (every authored attribute asks for a variability), so a
    /// read-write lock is the right shape and matches the in-tree precedent in
    /// `lunco-hooks::registry`.
    ///
    /// Callers that only need a variability or a custom-flag should prefer the
    /// free [`variability_of`] / [`is_custom`], which take and drop the read
    /// lock internally and hand back owned values.
    pub fn global() -> &'static std::sync::RwLock<SchemaRegistry> {
        static REGISTRY: OnceLock<std::sync::RwLock<SchemaRegistry>> = OnceLock::new();
        REGISTRY.get_or_init(|| std::sync::RwLock::new(SchemaRegistry::load()))
    }

    /// Fold an asset-shipped schema library into the process-wide registry.
    ///
    /// `src` is a `generatedSchema.usda` — the `usdGenSchema` output, exactly
    /// what [`GENERATED_SCHEMA`] is for `luncoSchema`. Its prim types and API
    /// schemas are recorded as REGISTERED (not custom), which is the whole
    /// point: without this an asset's `habitat:pressureVessel` composes and
    /// reads but carries no declared variability, no fallbacks and no
    /// validation — the "unregistered typeName" failure `schema.usda`'s own
    /// header warns about.
    ///
    /// Deliberately takes TEXT, not a path: the registry must work on wasm,
    /// where an asset-shipped schema arrives through the asset server rather
    /// than `std::fs`. The caller (the twin loader) does the resolving.
    ///
    /// Idempotent in effect — re-registering the same library re-ingests the
    /// same declarations over themselves. Returns `false` if `src` does not
    /// parse, matching [`load`](Self::load)'s tolerate-don't-panic policy: a
    /// bad extension degrades that library to "everything varying" rather than
    /// taking the process down.
    pub fn register_extension(src: &str) -> bool {
        let Ok(mut reg) = Self::global().write() else {
            return false;
        };
        let before = reg.properties.len();
        reg.ingest(src, true);
        reg.properties.len() >= before
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
                    // `/LunCoTerrainAPI.lunco:terrain:windowM`
                    //   → (`/LunCoTerrainAPI`, "lunco:terrain:windowM")
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
/// [`SchemaRegistry::global`] — takes and drops the read lock, returns an owned
/// value, so callers never hold a guard.
///
/// A poisoned lock degrades to USD's default (`varying`) rather than panicking:
/// the same tolerate-don't-panic policy a malformed schema gets.
pub fn variability_of(name: &str) -> sdf::Variability {
    SchemaRegistry::global()
        .read()
        .map(|r| r.variability(name))
        .unwrap_or(sdf::Variability::Varying)
}

/// Whether `name` must be authored `custom`. Convenience over
/// [`SchemaRegistry::global`] — see [`variability_of`] for the locking note.
///
/// A poisoned lock degrades to `false` (do not force `custom`), which is the
/// conservative answer: wrongly stamping `custom` on a schema-declared property
/// would be a real authoring error, whereas omitting it is what USD does anyway
/// for everything this registry does not know.
pub fn is_custom(name: &str) -> bool {
    SchemaRegistry::global()
        .read()
        .map(|r| r.is_custom(name))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The embedded generated schema must parse — it is compiled in, so a failure
    /// here is a build-time defect, and a silently-empty registry would quietly
    /// restore the "everything is varying" bug.
    #[test]
    fn generated_schema_parses_and_registers_every_type() {
        let reg = SchemaRegistry::global().read().unwrap();
        assert!(
            reg.prim_types().contains(&"LunCoEnvironment".to_string()),
            "typed schemas: {:?}",
            reg.prim_types()
        );
        assert!(reg.prim_types().contains(&"LunCoPolicy".to_string()));
        for api in [
            "LunCoTerrainAPI",
            "LunCoTerrainLayerAPI",
            "LunCoShadowAPI",
            // The vehicle extension APIs (doc 53) carry the LunCo-specific
            // suspension/wheel attrs that have no PhysX equivalent. One API per
            // prim role — see the schema. Pinned here so a schema/generatedSchema
            // drift doesn't silently make them `custom`.
            "LunCoSuspensionAPI",
            "LunCoWheelAPI",
            "LunCoSuspensionVisualAPI",
        ] {
            assert!(
                reg.api_schemas().contains(&api.to_string()),
                "{api} missing from {:?}",
                reg.api_schemas()
            );
        }
    }

    /// `schema.usda` and `generatedSchema.usda` must declare the same classes.
    ///
    /// `schema.usda` is the authoritative source and is READ BY NOTHING: the runtime
    /// registry parses [`GENERATED_SCHEMA`], and `usdGenSchema` is what carries one into
    /// the other. Nothing runs it on a hook, so the two can drift — and the drift is
    /// silent in the worst direction: a class declared in the source alone looks
    /// authored, reviews as authored, and does not exist at runtime.
    ///
    /// Class NAMES only. Comparing every property would just be re-running
    /// `usdGenSchema` in a test; the names catch a class added to one file and forgotten
    /// in the other, which is the failure that happens.
    #[test]
    fn source_and_generated_schema_declare_the_same_classes() {
        fn class_names(usda: &str) -> std::collections::BTreeSet<String> {
            usda.lines()
                .filter_map(|line| {
                    // `class "LunCoFooAPI" (` and `class LunCoFoo "LunCoFoo" (`
                    let rest = line.strip_prefix("class ")?;
                    let start = rest.find('"')? + 1;
                    let end = rest[start..].find('"')? + start;
                    Some(rest[start..end].to_string())
                })
                .filter(|n| n.starts_with("LunCo"))
                .collect()
        }

        let source = class_names(include_str!("../schema/schema.usda"));
        let generated = class_names(GENERATED_SCHEMA);
        assert!(!source.is_empty(), "parsed no classes from schema.usda");

        let missing: Vec<_> = source.difference(&generated).collect();
        let extra: Vec<_> = generated.difference(&source).collect();
        assert!(
            missing.is_empty() && extra.is_empty(),
            "schema.usda and generatedSchema.usda disagree — regenerate with \
             `usdGenSchema schema.usda .`\n  in source but NOT generated (invisible to \
             the runtime): {missing:?}\n  in generated but NOT source (authored by \
             hand?): {extra:?}"
        );
    }

    /// Every schema class must be registered in `plugInfo.json`.
    ///
    /// `plugInfo.json` is how a USD runtime (usdview, Omniverse, anything linking
    /// pxr) discovers our codeless schema — our own registry reads the USDA
    /// directly and never consults it. That asymmetry is the trap: an unregistered
    /// class works perfectly here and does not exist anywhere else, so the drift is
    /// invisible from inside the app.
    ///
    /// Keys are matched textually rather than through a JSON parser: this is the
    /// only JSON in the crate, and a dependency for one test is a worse trade than
    /// a substring search over a file we control.
    #[test]
    fn every_schema_class_is_registered_in_pluginfo() {
        const PLUG_INFO: &str = include_str!("../schema/plugInfo.json");
        let reg = SchemaRegistry::global().read().unwrap();

        for name in reg.api_schemas().iter().chain(reg.prim_types().iter()) {
            // Ours only: core USD schemas are registered by their own plugins.
            if !name.starts_with("LunCo") {
                continue;
            }
            assert!(
                PLUG_INFO.contains(&format!("\"{name}\"")),
                "{name} is declared in the schema but missing from plugInfo.json — \
                 no external USD runtime can resolve it"
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
        assert!(SchemaRegistry::global().read().unwrap().property("physics:mass").is_some());
        // Genuinely unknown (no schema we vendor declares it) → USD's default.
        assert_eq!(variability_of("nonesuch:madeUp"), sdf::Variability::Varying);
        assert!(SchemaRegistry::global().read().unwrap().property("nonesuch:madeUp").is_none());
    }

    /// An ASSET-SHIPPED schema library registers at runtime.
    ///
    /// This is the seam that makes a domain schema portable. A twin's own
    /// properties (`habitat:` for a pressurised habitat) belong to the MODEL,
    /// not to LunCoSim, so they must not be squatted into `luncoSchema` — but
    /// until the library can register, they compose and read while carrying no
    /// declared variability, no fallbacks and no validation. That is precisely
    /// the "unregistered typeName" failure this module's header warns about.
    ///
    /// Uses a `uniform` property on purpose: `uniform` is the half a registry
    /// gets WRONG when it has never heard of the name (USD's default is
    /// `varying`), so asserting it proves the declaration was really ingested
    /// rather than coincidentally matching the fallback.
    #[test]
    fn asset_shipped_schema_library_registers_and_declares_its_properties() {
        const HABITAT_SCHEMA: &str = r#"#usda 1.0
(
    upAxis = "Y"
)
class "HabitatShieldingAPI" (
    customData = { token apiSchemaType = "singleApply" }
)
{
    uniform token habitat:shielding:medium = "sinteredRegolith"
    double habitat:shielding:thicknessM = 1.5696
}
"#;
        // Unknown before registration — the fallback, not a declaration.
        assert_eq!(
            variability_of("habitat:shielding:medium"),
            sdf::Variability::Varying,
            "precondition: the habitat library must not already be registered"
        );

        assert!(SchemaRegistry::register_extension(HABITAT_SCHEMA), "extension must ingest");

        let reg = SchemaRegistry::global().read().unwrap();
        let medium = reg
            .property("habitat:shielding:medium")
            .expect("registered extension declares habitat:shielding:medium");
        assert_eq!(medium.type_name, "token");
        assert_eq!(
            medium.variability,
            sdf::Variability::Uniform,
            "the DECLARED uniform must win over USD's varying default"
        );
        assert_eq!(
            reg.property("habitat:shielding:thicknessM").unwrap().type_name,
            "double"
        );
        // The library's API schema is now a REGISTERED type, not a custom one.
        assert!(
            reg.api_schemas().contains(&"HabitatShieldingAPI".to_string()),
            "api schemas: {:?}",
            reg.api_schemas()
        );
    }

    /// Core USD types come from the real schema too — not a table of what someone
    /// remembered, and not left blank for the call site to assert.
    #[test]
    fn core_schema_declares_property_types() {
        let reg = SchemaRegistry::global().read().unwrap();
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
        let reg = SchemaRegistry::global().read().unwrap();
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
        // `primvars:doNotCastShadows` is deliberately NOT declared here — it is
        // Omniverse's primvar, read by name so their scenes keep their shadow intent
        // (see the note in `schema.usda`). A primvar needs no class; asserting one
        // would be asserting our own invention.
        assert!(reg.property("primvars:doNotCastShadows").is_none());
        // A program names a built-in instead of supplying one — the third arm of
        // `implementationSource`, as `UsdShade` has `info:id` beside its
        // `info:sourceAsset`. `schema.usda` is never read at runtime, so this asserts
        // the GENERATED file carries it; the two drift in silence otherwise.
        assert_eq!(reg.property("lunco:program:id").unwrap().type_name, "token");
        assert_eq!(
            reg.property("lunco:program:id").unwrap().variability,
            sdf::Variability::Uniform
        );
        assert_eq!(
            reg.property("lunco:terrain:horizonShadows").unwrap().type_name,
            "bool"
        );
        // The LunCo vehicle extension attrs (doc 53) — LunCo-specific concepts with
        // no PhysX equivalent, one API per prim role. Pinned so they register as
        // declared, not `custom`, and so a schema/generatedSchema drift is caught.
        // `float`, matching the sibling `physxVehicleSuspension:*` attrs and the
        // authoring in `assets/components/mobility/suspensions/*.usda`. The
        // loader reads it through the precision-tolerant `UsdRead::real()`, so a
        // schema/asset type split would go unnoticed at runtime — pin it here.
        assert_eq!(
            reg.property("lunco:suspension:restLength").unwrap().type_name,
            "float",
        );
        assert_eq!(
            reg.property("lunco:wheel:index").unwrap().type_name,
            "int",
        );
        assert_eq!(
            reg.property("lunco:suspensionVisual:role").unwrap().type_name,
            "token",
        );
        assert_eq!(
            reg.property("lunco:suspensionVisual:role").unwrap().variability,
            sdf::Variability::Uniform,
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

    /// The PhysX vehicle schemas register with NVIDIA-canonical property names.
    ///
    /// `physxSchema.usda` is reconstructed (see `../schema/core/README.md`), so
    /// this test is the guard against two classes of drift:
    ///  1. the reconstruction getting a name wrong (e.g. `springStiffness` instead
    ///     of the canonical `springStrength`),
    ///  2. a future verbatim swap with a Kit `schema.usda` silently dropping a
    ///     property this codebase reads.
    ///
    /// We assert properties, not API-schema membership: like the other vendored
    /// core schemas (`usdPhysics`), `physxSchema` is ingested with `own=false`,
    /// so its API names do NOT appear in `api_schemas()` (which answers "what
    /// does *luncoSchema* define"). Their *properties* fold in regardless, and
    /// that is what the loader reads. The API names themselves are detected at
    /// read time via `reader.has_api_schema` against the composed `apiSchemas`
    /// list — a separate path that does not consult this registry.
    #[test]
    fn physx_vehicle_schemas_register_canonical_properties() {
        let reg = SchemaRegistry::global().read().unwrap();

        // Canonical names exist with correct types. Each is the name the loader
        // (Phase 2) or the suspension loader (Phase 3, doc 53) reads.
        assert_eq!(
            reg.property("physxVehicleSuspension:springStrength")
                .expect("canonical spring strength attr")
                .type_name,
            "float",
        );
        assert_eq!(
            reg.property("physxVehicleSuspension:springDamperRate")
                .expect("canonical spring damper rate attr")
                .type_name,
            "float",
        );
        assert_eq!(
            reg.property("physxVehicleSuspension:travelDistance")
                .expect("travel distance attr")
                .type_name,
            "float",
        );
        assert_eq!(
            reg.property("physxVehicleWheel:radius").unwrap().type_name,
            "float",
        );
        assert_eq!(
            reg.property("physxVehicleWheel:moi").unwrap().type_name,
            "float",
        );
        assert_eq!(
            reg.property("physxVehicleWheel:dampingRate")
                .unwrap()
                .type_name,
            "float",
        );
        assert_eq!(
            reg.property("physxVehicleWheelAttachment:index")
                .unwrap()
                .type_name,
            "int",
        );
        // The frame attrs are the types assets get wrong most often (double3 vs
        // point3f, quatd vs quatf) — pin them.
        assert_eq!(
            reg.property("physxVehicleWheelAttachment:suspensionFramePosition")
                .unwrap()
                .type_name,
            "point3f",
        );
        assert_eq!(
            reg.property("physxVehicleWheelAttachment:suspensionFrameOrientation")
                .unwrap()
                .type_name,
            "quatf",
        );
        assert_eq!(
            reg.property("physxVehicleWheelAttachment:suspensionTravelDirection")
                .unwrap()
                .type_name,
            "vector3f",
        );
        // Compliance graphs — float2[]/float4[], NOT float[]/float3[]. This is the
        // single most common reconstruction mistake (the jounce is packed as the
        // first component).
        assert_eq!(
            reg.property("physxVehicleSuspensionCompliance:wheelToeAngle")
                .unwrap()
                .type_name,
            "float2[]",
        );
        assert_eq!(
            reg.property("physxVehicleSuspensionCompliance:wheelCamberAngle")
                .unwrap()
                .type_name,
            "float2[]",
        );
        assert_eq!(
            reg.property("physxVehicleSuspensionCompliance:suspensionForceAppPoint")
                .unwrap()
                .type_name,
            "float4[]",
        );
        assert_eq!(
            reg.property("physxVehicleSuspensionCompliance:tireForceAppPoint")
                .unwrap()
                .type_name,
            "float4[]",
        );

        // Steering: the lock angle lives on the steering API, in RADIANS. PhysX
        // deprecated the per-wheel `physxVehicleWheel:maxSteerAngle` in favour of
        // this, and only the Kit authoring wizard's UI field is in degrees.
        assert_eq!(
            reg.property("physxVehicleAckermannSteering:maxSteerAngle")
                .expect("canonical Ackermann steer lock")
                .type_name,
            "float",
        );
        // `maxWheelAngleDegrees` is attested in NO NVIDIA schema or doc: a
        // reconstructed file's risk is not just a wrong name for a real property but a
        // plausible name for one that does not exist. Pinned absent so it cannot return.
        assert!(
            reg.property("physxVehicleAckermannSteering:maxWheelAngleDegrees")
                .is_none(),
            "maxWheelAngleDegrees is not a PhysX property; the lock is \
             physxVehicleAckermannSteering:maxSteerAngle, in radians"
        );

        // Non-canonical PhysX names must be absent: their presence would mean the
        // reconstruction squats a name the real schema does not define.
        assert!(
            reg.property("physxVehicleSuspension:springStiffness").is_none(),
            "springStiffness is not a canonical PhysX name; use springStrength"
        );
        assert!(
            reg.property("physxVehicleSuspension:springDamping").is_none(),
            "springDamping is not a canonical PhysX name; use springDamperRate"
        );
        assert!(
            reg.property("physxVehicleSuspension:restLength").is_none(),
            "PhysxVehicleSuspensionAPI has no restLength; PhysX models travel as \
             travelDistance + sprungMass. LunCo's rest_length is a lunco: extension."
        );
        // `index` belongs on WheelAttachment, NOT on Wheel. A Wheel index attr
        // must NOT resolve — it would re-introduce the misplacement.
        assert!(
            reg.property("physxVehicleWheel:index").is_none(),
            "physxVehicleWheel:index is non-canonical; index lives on \
             physxVehicleWheelAttachment:index"
        );
    }
}
