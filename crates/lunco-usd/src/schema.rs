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

/// How an attribute's scalar value relates to a length in the stage's linear units.
///
/// USD has no role type for scalar lengths — `radius` is a bare `double` — so this
/// is carried per (schema, property) instead, which is the only place the fact is
/// actually known.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum LinearUnit {
    /// Not a linear quantity. The DEFAULT and the safe answer: an unannotated
    /// attribute is left alone rather than guessed at.
    #[default]
    None,
    /// A length. `stage_units_per_unit` is how many stage linear units one
    /// authored unit represents — `1.0` for ordinary lengths, `0.1` for
    /// `UsdGeomCamera`'s focal length and apertures, which USD defines in TENTHS
    /// of a world unit.
    Length { stage_units_per_unit: f64 },
}

/// The linear-unit facts core USD states in prose but encodes in no type.
///
/// `point3f`/`vector3f`/`normal3f` carry their role in the type system and are
/// handled by the role types. A `UsdGeomSphere`'s `radius` is a bare `double`, and
/// on a stage whose `metersPerUnit` is `0.01` it must be authored in centimetres —
/// a fact that lives in the schema's documentation and nowhere a program can read.
/// Keyed by `(schema, property)` for the same reason the registry itself is: core
/// declares `radius` and `height` on four different gprims.
///
/// Every entry is checked against the vendored `generatedSchema.usda` this registry
/// loads; a name core does not declare does not belong here.
const CORE_LINEAR_UNITS: &[(&str, &str, LinearUnit)] = &[
    // Gprim dimensions — plain lengths in stage linear units.
    ("Sphere", "radius", LinearUnit::Length { stage_units_per_unit: 1.0 }),
    ("Cube", "size", LinearUnit::Length { stage_units_per_unit: 1.0 }),
    ("Cylinder", "radius", LinearUnit::Length { stage_units_per_unit: 1.0 }),
    ("Cylinder", "height", LinearUnit::Length { stage_units_per_unit: 1.0 }),
    ("Cone", "radius", LinearUnit::Length { stage_units_per_unit: 1.0 }),
    ("Cone", "height", LinearUnit::Length { stage_units_per_unit: 1.0 }),
    ("Capsule", "radius", LinearUnit::Length { stage_units_per_unit: 1.0 }),
    ("Capsule", "height", LinearUnit::Length { stage_units_per_unit: 1.0 }),
    // `Cylinder_1` / `Capsule_1` are USD's own axis-agnostic successors, declared
    // in the same file. Omitting them would leave the successor schema silently
    // unannotated while its predecessor resolved.
    ("Cylinder_1", "radius", LinearUnit::Length { stage_units_per_unit: 1.0 }),
    ("Cylinder_1", "height", LinearUnit::Length { stage_units_per_unit: 1.0 }),
    ("Capsule_1", "radius", LinearUnit::Length { stage_units_per_unit: 1.0 }),
    ("Capsule_1", "height", LinearUnit::Length { stage_units_per_unit: 1.0 }),
    ("Plane", "width", LinearUnit::Length { stage_units_per_unit: 1.0 }),
    ("Plane", "length", LinearUnit::Length { stage_units_per_unit: 1.0 }),
    // `UsdGeomCamera` states its focal length and aperture in TENTHS of a world
    // unit, so that the schema's default 50 / 20.955 / 15.2908 read as the
    // photographer's millimetres on a stage authored in centimetres. This is USD's
    // documented convention, not a rounding of ours — see the class documentation
    // in `usdGeom`'s generated schema.
    ("Camera", "focalLength", LinearUnit::Length { stage_units_per_unit: 0.1 }),
    ("Camera", "horizontalAperture", LinearUnit::Length { stage_units_per_unit: 0.1 }),
    ("Camera", "verticalAperture", LinearUnit::Length { stage_units_per_unit: 0.1 }),
    ("Camera", "horizontalApertureOffset", LinearUnit::Length { stage_units_per_unit: 0.1 }),
    ("Camera", "verticalApertureOffset", LinearUnit::Length { stage_units_per_unit: 0.1 }),
    // `focusDistance` is a distance in the scene, not through the lens, so it is an
    // ordinary world-unit length — the exception that makes the tenths above easy
    // to get wrong.
    ("Camera", "focusDistance", LinearUnit::Length { stage_units_per_unit: 1.0 }),
];

/// What a schema declares about one property.
#[derive(Debug, Clone, PartialEq)]
pub struct PropertySpec {
    /// The schema's declared USD type name (`"float"`, `"uniform token"` → `"token"`).
    pub type_name: String,
    /// `uniform` or `varying`, per the schema.
    pub variability: sdf::Variability,
    /// The schema that declares it — `"LunCoTerrainAPI"`, `"UsdShadeShader"`.
    pub declared_by: String,
    /// Whether the value is a length, and in what multiple of the stage's linear
    /// unit. From [`CORE_LINEAR_UNITS`] for core USD, from a `lunco:unit` entry in
    /// the property's `customData` for schemas of ours.
    pub linear: LinearUnit,
    /// Slider bounds/unit the SCHEMA declares for this property
    /// (`customData { min, max, unit }` on the schema attribute). The
    /// schema-level default for every asset that composes the schema; a
    /// per-asset authored `customData` still overrides it
    /// (`produce_usd_param_view` asks the composed attribute first).
    pub ui_hint: Option<lunco_usd_bevy::AttrUiHint>,
}

/// The parsed `luncoSchema` plus the core `uniform` table.
#[derive(Debug, Default)]
pub struct SchemaRegistry {
    /// Every declaration, keyed by `(declaring schema, property name)`.
    ///
    /// The declaring schema is part of the key because a property name is not
    /// unique in USD: core schemas declare bare names across files (`radius`,
    /// `axis`, `basis`, …) with types that need not agree, and an asset-shipped
    /// library may legitimately declare a name core already uses. Keyed by name
    /// alone, the second file to declare a name ERASED the first — the registry
    /// then answered for a schema the prim does not even apply.
    properties: HashMap<(String, String), PropertySpec>,
    /// Which declaration answers a lookup by BARE NAME — the shape almost every
    /// caller has, because an authoring call site knows the property it is
    /// writing and not the schema that declared it.
    ///
    /// PRECEDENCE. A declaration from one of *our* schemas (`luncoSchema` and
    /// asset-shipped libraries, ingested with `own`) outranks any vendored core
    /// declaration, because those are the namespaces we control and can be held
    /// to. Within a tier the most recently ingested declaration wins, which is
    /// what makes re-registering an extension refresh it rather than be ignored.
    /// Divergent redeclarations are still warned at ingest: the tie-break makes
    /// the answer *defined*, it does not make the collision *fine*.
    ///
    /// Callers that must have a specific schema's answer ask
    /// [`property_in`](SchemaRegistry::property_in) and skip this index.
    by_name: HashMap<String, (String, bool)>,
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
    /// Core goes in FIRST, though the outcome no longer depends on it: ours
    /// outranks core by precedence, not by ingest order. It should never come up
    /// anyway — the schema forbids bare names precisely so a `lunco:`-namespaced
    /// property cannot collide with a core one.
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
        reg.apply_core_linear_units();
        reg.ingest(GENERATED_SCHEMA, true);
        reg
    }

    /// Stamp [`CORE_LINEAR_UNITS`] onto the declarations core USD just contributed.
    ///
    /// Applied after the core ingest rather than consulted at lookup time so that a
    /// table entry naming a property core does not declare cannot sit there unnoticed:
    /// the only way to be sure the fact attaches to a real declaration is to attach it.
    fn apply_core_linear_units(&mut self) {
        for (schema, name, unit) in CORE_LINEAR_UNITS {
            match self
                .properties
                .get_mut(&(schema.to_string(), name.to_string()))
            {
                Some(prop) => prop.linear = *unit,
                None => bevy::log::warn!(
                    "[schema] linear-unit table names {schema}.{name}, which no vendored \
                     core schema declares — the entry is dead and the unit is unknown"
                ),
            }
        }
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
                    // `customData` is USD's per-spec escape hatch: a dictionary any
                    // schema may carry, needing no plugInfo registration and surviving
                    // `usdGenSchema` verbatim. It is where a `lunco:` property states
                    // the one thing USD's type system cannot — that its scalar is a
                    // length. Core USD annotates nothing this way, so it is stamped
                    // from `CORE_LINEAR_UNITS` instead.
                    let (linear, ui_hint) = match spec.get("customData") {
                        Some(sdf::Value::Dictionary(d)) => {
                            let linear = match d.get("lunco:unit").cloned().and_then(token_or_string)
                            {
                                None => LinearUnit::None,
                                Some(u) if u == "length" => {
                                    LinearUnit::Length { stage_units_per_unit: 1.0 }
                                }
                                // A typo here would otherwise degrade to "not a length"
                                // in silence, which is indistinguishable from an
                                // attribute nobody annotated.
                                Some(other) => {
                                    bevy::log::warn!(
                                        "[schema] {}.{name}: unrecognised lunco:unit \
                                         '{other}' — treated as not a linear quantity",
                                        prim,
                                    );
                                    LinearUnit::None
                                }
                            };
                            // Schema-declared slider bounds — the ONE decoder
                            // (`AttrUiHint::from_dict`) shared with the composed-
                            // stage per-asset read.
                            (linear, lunco_usd_bevy::AttrUiHint::from_dict(d))
                        }
                        _ => (LinearUnit::None, None),
                    };
                    let prop = PropertySpec {
                        type_name,
                        linear,
                        ui_hint,
                        // Unauthored ⇒ `varying`, USD's default. `uniform` is
                        // the only variability USDA actually writes out.
                        variability: match spec.get("variability") {
                            Some(sdf::Value::Variability(v)) => *v,
                            _ => sdf::Variability::Varying,
                        },
                        declared_by: prim.to_string(),
                    };
                    // Both declarations are KEPT — only which one answers a
                    // bare-name lookup is a choice, and it follows the precedence
                    // documented on `by_name`. The warning stays for a genuine
                    // divergence (same name, different type or variability from a
                    // different schema): the tie-break resolves it, nobody
                    // authored it deliberately.
                    if let Some((prev_schema, _)) = reg.by_name.get(name) {
                        if let Some(prev) = reg.properties.get(&(prev_schema.clone(), name.to_string())) {
                            if *prev_schema != prop.declared_by
                                && (prev.type_name != prop.type_name
                                    || prev.variability != prop.variability)
                            {
                                bevy::log::warn!(
                                    "[schema] property '{name}': {} declares {} {:?}, \
                                     {} declares {} {:?} — lookups by name resolve to \
                                     the higher-precedence declaration",
                                    prop.declared_by,
                                    prop.type_name,
                                    prop.variability,
                                    prev.declared_by,
                                    prev.type_name,
                                    prev.variability,
                                );
                            }
                        }
                    }
                    // Ours outranks vendored core; within a tier the newest wins.
                    let wins = match reg.by_name.get(name) {
                        Some((_, prev_own)) => own || !*prev_own,
                        None => true,
                    };
                    if wins {
                        reg.by_name
                            .insert(name.to_string(), (prop.declared_by.clone(), own));
                    }
                    reg.properties
                        .insert((prop.declared_by.clone(), name.to_string()), prop);
                }
                _ => {}
            }
        }
    }

    /// What a schema declares about property `name`, or `None` when no schema this
    /// registry knows declares it.
    ///
    /// When several schemas declare the name, this answers with the
    /// highest-precedence one — see the rule on `by_name`. Use
    /// [`property_in`](Self::property_in) when the prim's schema is known and the
    /// answer must be that schema's.
    pub fn property(&self, name: &str) -> Option<&PropertySpec> {
        let (schema, _) = self.by_name.get(name)?;
        self.properties.get(&(schema.clone(), name.to_string()))
    }

    /// What `schema` specifically declares about `name` — no precedence, no
    /// fallback to another schema's declaration of the same name.
    pub fn property_in(&self, schema: &str, name: &str) -> Option<&PropertySpec> {
        self.properties.get(&(schema.to_string(), name.to_string()))
    }

    /// Whether `schema`'s `name` is a length, and in what multiple of the stage's
    /// linear unit. [`LinearUnit::None`] when the pair is unknown or unannotated —
    /// a unit conversion must never be invented for a property nobody described.
    ///
    /// Takes the schema explicitly, not a bare name: `radius` is a length on
    /// `Sphere` and on `Cylinder`, and there is no reason a future schema's
    /// `radius` must be one at all.
    pub fn linear_unit(&self, schema: &str, name: &str) -> LinearUnit {
        self.property_in(schema, name)
            .map(|p| p.linear)
            .unwrap_or_default()
    }

    /// Every schema this registry knows that declares `name`, in no fixed order.
    /// More than one is normal for core USD bare names, not an error.
    pub fn declaring_schemas(&self, name: &str) -> Vec<&str> {
        self.properties
            .keys()
            .filter(|(_, n)| n == name)
            .map(|(schema, _)| schema.as_str())
            .collect()
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
        name.starts_with("lunco:") && !self.by_name.contains_key(name)
    }

    /// The slider hint the schema declares for `name` (bare-name precedence
    /// lookup, same rule as [`property`](Self::property)). Per-asset authored
    /// `customData` still overrides — callers ask the composed attribute first
    /// and fall back here.
    pub fn ui_hint(&self, name: &str) -> Option<lunco_usd_bevy::AttrUiHint> {
        self.property(name).and_then(|p| p.ui_hint.clone())
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

/// Schema-declared slider hint for `name`. Convenience over
/// [`SchemaRegistry::global`] — see [`variability_of`] for the locking note.
/// A poisoned lock degrades to `None` (no derived slider), never a panic.
pub fn ui_hint_of(name: &str) -> Option<lunco_usd_bevy::AttrUiHint> {
    SchemaRegistry::global()
        .read()
        .ok()
        .and_then(|r| r.ui_hint(name))
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
             `python3 scripts/gen_schema.py`\n  in source but NOT generated (invisible to \
             the runtime): {missing:?}\n  in generated but NOT source (authored by \
             hand?): {extra:?}"
        );
    }

    /// Schema-declared UI hints must survive regeneration: the hints are
    /// authored in `schema.usda` but the runtime loads GENERATED_SCHEMA, so a
    /// forgotten `python3 scripts/gen_schema.py` would silently strip every
    /// derived slider. Pin one representative hint per wheel-domain API.
    #[test]
    fn wheel_schema_declares_ui_hints() {
        let reg = SchemaRegistry::global().read().unwrap();
        for name in [
            "lunco:wheel:driveDamping",
            "lunco:wheel:stallTorqueGain",
            "lunco:wheel:contactGripStiffness",
            "lunco:wheel:driveForcePerNormal",
            "lunco:suspension:restLength",
            "lunco:tire:frictionCoefficient",
        ] {
            let hint = reg.ui_hint(name).unwrap_or_else(|| {
                panic!("{name} declares no schema-level UI hint — regenerate with gen_schema.py")
            });
            let (min, max) = (hint.min.expect("min"), hint.max.expect("max"));
            assert!(max > min, "{name}: degenerate hint range {min}..{max}");
        }
        // And the per-asset override contract: an authored customData beats the
        // schema hint (produce_usd_param_view asks the composed attr FIRST) —
        // nothing to assert here at registry level, but the registry must not
        // invent hints for un-annotated names.
        assert!(reg.ui_hint("lunco:wheel:index").is_none(),
            "lunco:wheel:index should carry no slider hint (wiring identity, not a knob)");
    }

    /// The physxVehicle attributes the wheel reader requires must ALSO surface
    /// sliders — they live in our reconstructed `core/physxSchema.usda`, and the
    /// sibling test above only covers `luncoSchema`.
    ///
    /// This exists because they were silently INERT: each attribute carried TWO
    /// `customData = {…}` blocks (bounds, then `userDocBrief`), and a spec field
    /// is overwrite-in-place (`sdf::SpecData::add`), so the second erased the
    /// first and `ui_hint` returned `None` for every one of them. Nothing failed
    /// — the sliders just never appeared. ONE `customData` block per attribute is
    /// the rule; this test is what makes breaking it loud.
    #[test]
    fn physx_vehicle_schema_declares_ui_hints() {
        let reg = SchemaRegistry::global().read().unwrap();
        for name in [
            "physxVehicleWheel:radius",
            "physxVehicleWheel:maxBrakeTorque",
            "physxVehicleWheel:dampingRate",
            "physxVehicleWheel:moi",
            "physxVehicleEngine:peakTorque",
            "physxVehicleEngine:maxRotationSpeed",
            "physxVehicleTire:longitudinalStiffness",
            "physxVehicleSuspension:springStrength",
            "physxVehicleSuspension:springDamperRate",
            "physxVehicleAckermannSteering:maxSteerAngle",
        ] {
            let hint = reg.ui_hint(name).unwrap_or_else(|| {
                panic!(
                    "{name} declares no schema-level UI hint — check core/physxSchema.usda \
                     for a SECOND customData block silently overwriting the bounds one"
                )
            });
            let (min, max) = (hint.min.expect("min"), hint.max.expect("max"));
            assert!(max > min, "{name}: degenerate hint range {min}..{max}");
        }
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

    /// Two schemas declaring the same name COEXIST — the second no longer erases
    /// the first, and each can still be asked for its own answer.
    ///
    /// `radius` is the real case: `usdGeom` declares it on Sphere, Cylinder, Cone
    /// and Capsule. Under name-only keying the registry held exactly one of them
    /// and answered with it for every prim type.
    #[test]
    fn same_name_in_two_schemas_coexists() {
        let reg = SchemaRegistry::global().read().unwrap();
        let declarers = reg.declaring_schemas("radius");
        assert!(
            declarers.len() > 1,
            "radius is declared by several core schemas, got {declarers:?}"
        );
        for schema in declarers {
            assert_eq!(reg.property_in(schema, "radius").unwrap().type_name, "double");
        }
        // The bare-name lookup still answers, and answers with a real declarer.
        let resolved = reg.property("radius").unwrap();
        assert!(reg.property_in(&resolved.declared_by, "radius").is_some());
    }

    /// Our own declarations outrank vendored core for a bare-name lookup, whatever
    /// the ingest order — the precedence rule, pinned.
    #[test]
    fn our_schema_outranks_core_for_bare_name_lookup() {
        const SQUATTER: &str = r#"#usda 1.0
(
    upAxis = "Y"
)
class "SquatterAPI" (
    customData = { token apiSchemaType = "singleApply" }
)
{
    uniform token squatter:probe = "x"
}
"#;
        assert!(SchemaRegistry::register_extension(SQUATTER));
        let reg = SchemaRegistry::global().read().unwrap();
        assert_eq!(
            reg.property("squatter:probe").unwrap().declared_by,
            "SquatterAPI"
        );
        // Core's own declarations are untouched by the extension.
        assert_eq!(reg.property("physics:mass").unwrap().type_name, "float");
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
        // `info:implementationSource`, `UsdShade`'s own property which
        // `LunCoProgram` declares directly. `schema.usda` is never read at
        // runtime, so this asserts the GENERATED file carries it; the two drift
        // in silence otherwise.
        assert_eq!(reg.property("info:id").unwrap().type_name, "token");
        assert_eq!(
            reg.property("info:id").unwrap().variability,
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

    /// A scalar length is knowable only from the schema, so the registry must be
    /// where it is known.
    ///
    /// The pairs below are all bare `double`/`float` in USD's type system — nothing
    /// about the authored value says "metres". Without this, code scaling a stage
    /// authored in centimetres has to carry its own list of which numbers are
    /// distances, which is the hand-table failure the module header describes.
    #[test]
    fn linear_units_come_from_the_schema() {
        let reg = SchemaRegistry::global().read().unwrap();

        assert_eq!(
            reg.linear_unit("Sphere", "radius"),
            LinearUnit::Length { stage_units_per_unit: 1.0 }
        );
        assert_eq!(
            reg.linear_unit("Plane", "width"),
            LinearUnit::Length { stage_units_per_unit: 1.0 }
        );

        // `UsdGeomCamera` defines these in TENTHS of a world unit. Getting the factor
        // wrong is a 10x field of view, and nothing about the `float` says so.
        assert_eq!(
            reg.linear_unit("Camera", "focalLength"),
            LinearUnit::Length { stage_units_per_unit: 0.1 }
        );
        assert_eq!(
            reg.linear_unit("Camera", "horizontalAperture"),
            LinearUnit::Length { stage_units_per_unit: 0.1 }
        );
        // …but the focus distance is a distance in the SCENE, so it is not.
        assert_eq!(
            reg.linear_unit("Camera", "focusDistance"),
            LinearUnit::Length { stage_units_per_unit: 1.0 }
        );

        // Unannotated properties are left alone rather than guessed at — including
        // one that is dimensionless (`fStop`) and one that is a real quantity in
        // units this mechanism does not cover (`physics:mass`).
        assert_eq!(reg.linear_unit("Camera", "fStop"), LinearUnit::None);
        assert_eq!(reg.linear_unit("MassAPI", "physics:mass"), LinearUnit::None);
        // An unknown pair answers None too: no unit is ever invented.
        assert_eq!(reg.linear_unit("Sphere", "nonesuch"), LinearUnit::None);
        assert_eq!(reg.linear_unit("Nonesuch", "radius"), LinearUnit::None);
    }

    /// The reason the unit is keyed by `(schema, name)` and not by name: `radius` is
    /// declared by four core gprims, and each must be able to answer for itself.
    #[test]
    fn same_name_in_two_schemas_carries_its_own_unit() {
        let reg = SchemaRegistry::global().read().unwrap();
        for schema in ["Sphere", "Cylinder", "Cone", "Capsule"] {
            assert_eq!(
                reg.linear_unit(schema, "radius"),
                LinearUnit::Length { stage_units_per_unit: 1.0 },
                "{schema}.radius must resolve independently"
            );
        }
        // A name-keyed answer could not distinguish these: `height` is a length on
        // Cylinder, while `Camera.focalLength` — also a length — is in a different
        // multiple entirely.
        assert_eq!(
            reg.linear_unit("Cylinder", "height"),
            LinearUnit::Length { stage_units_per_unit: 1.0 }
        );
        assert_ne!(
            reg.linear_unit("Camera", "focalLength"),
            reg.linear_unit("Cylinder", "height")
        );
    }

    /// Our own schemas self-describe through `customData`, USD's per-spec escape
    /// hatch — no plugInfo registration, no parallel table to keep in step.
    #[test]
    fn our_schemas_declare_their_lengths_in_custom_data() {
        const RIG_SCHEMA: &str = r#"#usda 1.0
(
    upAxis = "Y"
)
class "RigUnitsAPI" (
    customData = { token apiSchemaType = "singleApply" }
)
{
    double rig:boomLength = 3.2 (
        customData = {
            string lunco:unit = "length"
        }
    )
    double rig:gearRatio = 4
    double rig:mystery = 1 (
        customData = {
            string lunco:unit = "furlong"
        }
    )
}
"#;
        assert!(SchemaRegistry::register_extension(RIG_SCHEMA));
        let reg = SchemaRegistry::global().read().unwrap();

        assert_eq!(
            reg.linear_unit("RigUnitsAPI", "rig:boomLength"),
            LinearUnit::Length { stage_units_per_unit: 1.0 }
        );
        // Declared, but says nothing about units — so nothing is claimed.
        assert_eq!(reg.linear_unit("RigUnitsAPI", "rig:gearRatio"), LinearUnit::None);
        // An unrecognised value is not a length either; it warns at ingest so the
        // typo is visible rather than silently reading as "unannotated".
        assert_eq!(reg.linear_unit("RigUnitsAPI", "rig:mystery"), LinearUnit::None);
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
