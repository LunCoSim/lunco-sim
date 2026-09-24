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
//! 1. **`luncoSchema`** — our own generated USDA schema source under the
//!    runtime `schemas/lunco` asset namespace, generated from `../schema/schema.usda`.
//! 2. **Core USD** — generated USDA schema sources under the runtime
//!    `schemas/core` asset namespace, supplied by the OpenUSD schema artifacts
//!    used by this checkout.
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

use crate::author::usda_to_data;
use lunco_usd_data::metadata::AttrUiHint;
use openusd::sdf::{self, SpecType};

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
    Length {
        /// Number of stage linear units represented by one authored unit.
        stage_units_per_unit: f64,
    },
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
/// Available entries are stamped as core sources arrive. Missing entries are
/// checked only after the runtime asset pipeline finishes loading every vendored
/// core schema, so an unrelated schema file cannot look like a dead declaration.
const CORE_LINEAR_UNITS: &[(&str, &str, LinearUnit)] = &[
    // Gprim dimensions — plain lengths in stage linear units.
    (
        "Sphere",
        "radius",
        LinearUnit::Length {
            stage_units_per_unit: 1.0,
        },
    ),
    (
        "Cube",
        "size",
        LinearUnit::Length {
            stage_units_per_unit: 1.0,
        },
    ),
    (
        "Cylinder",
        "radius",
        LinearUnit::Length {
            stage_units_per_unit: 1.0,
        },
    ),
    (
        "Cylinder",
        "height",
        LinearUnit::Length {
            stage_units_per_unit: 1.0,
        },
    ),
    (
        "Cone",
        "radius",
        LinearUnit::Length {
            stage_units_per_unit: 1.0,
        },
    ),
    (
        "Cone",
        "height",
        LinearUnit::Length {
            stage_units_per_unit: 1.0,
        },
    ),
    (
        "Capsule",
        "radius",
        LinearUnit::Length {
            stage_units_per_unit: 1.0,
        },
    ),
    (
        "Capsule",
        "height",
        LinearUnit::Length {
            stage_units_per_unit: 1.0,
        },
    ),
    // `Cylinder_1` / `Capsule_1` are USD's own axis-agnostic successors, declared
    // in the same file. Omitting them would leave the successor schema silently
    // unannotated while its predecessor resolved. NOTE the successors have no
    // `radius` — they split it into `radiusTop`/`radiusBottom`; naming `radius`
    // here made the entry dead and warned on every palette spawn.
    (
        "Cylinder_1",
        "radiusTop",
        LinearUnit::Length {
            stage_units_per_unit: 1.0,
        },
    ),
    (
        "Cylinder_1",
        "radiusBottom",
        LinearUnit::Length {
            stage_units_per_unit: 1.0,
        },
    ),
    (
        "Cylinder_1",
        "height",
        LinearUnit::Length {
            stage_units_per_unit: 1.0,
        },
    ),
    (
        "Capsule_1",
        "radiusTop",
        LinearUnit::Length {
            stage_units_per_unit: 1.0,
        },
    ),
    (
        "Capsule_1",
        "radiusBottom",
        LinearUnit::Length {
            stage_units_per_unit: 1.0,
        },
    ),
    (
        "Capsule_1",
        "height",
        LinearUnit::Length {
            stage_units_per_unit: 1.0,
        },
    ),
    (
        "Plane",
        "width",
        LinearUnit::Length {
            stage_units_per_unit: 1.0,
        },
    ),
    (
        "Plane",
        "length",
        LinearUnit::Length {
            stage_units_per_unit: 1.0,
        },
    ),
    // `UsdGeomCamera` states its focal length and aperture in TENTHS of a world
    // unit, so that the schema's default 50 / 20.955 / 15.2908 read as the
    // photographer's millimetres on a stage authored in centimetres. This is USD's
    // documented convention, not a rounding of ours — see the class documentation
    // in `usdGeom`'s generated schema.
    (
        "Camera",
        "focalLength",
        LinearUnit::Length {
            stage_units_per_unit: 0.1,
        },
    ),
    (
        "Camera",
        "horizontalAperture",
        LinearUnit::Length {
            stage_units_per_unit: 0.1,
        },
    ),
    (
        "Camera",
        "verticalAperture",
        LinearUnit::Length {
            stage_units_per_unit: 0.1,
        },
    ),
    (
        "Camera",
        "horizontalApertureOffset",
        LinearUnit::Length {
            stage_units_per_unit: 0.1,
        },
    ),
    (
        "Camera",
        "verticalApertureOffset",
        LinearUnit::Length {
            stage_units_per_unit: 0.1,
        },
    ),
    // `focusDistance` is a distance in the scene, not through the lens, so it is an
    // ordinary world-unit length — the exception that makes the tenths above easy
    // to get wrong.
    (
        "Camera",
        "focusDistance",
        LinearUnit::Length {
            stage_units_per_unit: 1.0,
        },
    ),
];

/// What a schema declares about one property.
#[derive(Debug, Clone, PartialEq)]
pub struct PropertySpec {
    /// The schema's declared USD type name (`"float"`, `"uniform token"` → `"token"`).
    pub type_name: String,
    /// Schema fallback value, if the declaration authors one.
    pub default_value: Option<sdf::Value>,
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
    pub ui_hint: Option<AttrUiHint>,
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
    /// Applied API schemas declared as built-ins by a concrete typed schema.
    typed_api_schemas: HashMap<String, Vec<String>>,
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
    /// The process-wide registry. It starts empty and is populated by the
    /// runtime schema asset pipeline, then remains **extensible** — see
    /// [`register_extension`](Self::register_extension).
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
        REGISTRY.get_or_init(|| std::sync::RwLock::new(SchemaRegistry::default()))
    }

    /// Fold an asset-shipped schema library into the process-wide registry.
    ///
    /// `src` is a `generatedSchema.usda` — the `usdGenSchema` output for
    /// `luncoSchema`. Its prim types and API
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
    /// parse, so the owning asset loader can report the invalid schema without
    /// mutating the registry.
    pub fn register_extension(src: &str) -> bool {
        Self::register_source(src, true)
    }

    /// Register one OpenUSD core schema source without claiming its prim types
    /// as project-owned. Core declarations receive the shared linear-unit facts
    /// after they are admitted. Missing table entries are validated separately,
    /// after the runtime has loaded the complete vendored core-schema set.
    pub fn register_core_extension(src: &str) -> bool {
        let registered = Self::register_source(src, false);
        if registered {
            if let Ok(mut registry) = Self::global().write() {
                registry.apply_core_linear_units();
            }
        }
        registered
    }

    /// Validate the linear-unit table after every vendored core schema loaded.
    ///
    /// Core schemas arrive as separate assets. A property absent from the current
    /// partial registry is not dead until the runtime asset pipeline finishes.
    pub fn validate_core_linear_units() {
        let Ok(registry) = Self::global().read() else {
            bevy::log::error!(
                "[schema] could not validate core linear units: registry lock is poisoned"
            );
            return;
        };
        for (schema, name) in registry.missing_core_linear_units() {
            bevy::log::warn!(
                "[schema] linear-unit table names {schema}.{name}, which no vendored \
                 core schema declares — the entry is dead and the unit is unknown"
            );
        }
    }

    fn register_source(src: &str, own: bool) -> bool {
        if usda_to_data(src).is_err() {
            return false;
        }
        let Ok(mut reg) = Self::global().write() else {
            return false;
        };
        reg.ingest(src, own)
    }

    /// Stamp available [`CORE_LINEAR_UNITS`] onto declarations already admitted.
    fn apply_core_linear_units(&mut self) {
        for (schema, name, unit) in CORE_LINEAR_UNITS {
            if let Some(prop) = self
                .properties
                .get_mut(&(schema.to_string(), name.to_string()))
            {
                prop.linear = *unit;
            }
        }
    }

    /// Find table entries absent from the complete core registry.
    fn missing_core_linear_units(&self) -> impl Iterator<Item = (&'static str, &'static str)> + '_ {
        CORE_LINEAR_UNITS.iter().filter_map(|(schema, name, _)| {
            (!self
                .properties
                .contains_key(&(schema.to_string(), name.to_string())))
            .then_some((*schema, *name))
        })
    }

    /// Fold one `generatedSchema.usda` into the registry. `own` records the file's
    /// prim types and API schemas as *ours* (see [`load`](Self::load)).
    fn ingest(&mut self, src: &str, own: bool) -> bool {
        let reg = self;
        let Ok(data) = usda_to_data(src) else {
            return false;
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
                    if let Some(sdf::Value::TokenVec(schemas)) = spec.get("apiSchemas") {
                        reg.typed_api_schemas.insert(
                            class.to_string(),
                            schemas.iter().map(ToString::to_string).collect(),
                        );
                    }
                    if !own {
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
                    let Some(type_name) = spec.get("typeName").cloned().and_then(token_or_string)
                    else {
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
                            let linear =
                                match d.get("lunco:unit").cloned().and_then(token_or_string) {
                                    None => LinearUnit::None,
                                    Some(u) if u == "length" => LinearUnit::Length {
                                        stage_units_per_unit: 1.0,
                                    },
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
                            (linear, AttrUiHint::from_dict(d))
                        }
                        _ => (LinearUnit::None, None),
                    };
                    let prop = PropertySpec {
                        type_name,
                        default_value: spec.get("default").cloned(),
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
                        if let Some(prev) =
                            reg.properties.get(&(prev_schema.clone(), name.to_string()))
                        {
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
        true
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

    /// Resolve a property's schema fallback for one composed prim declaration.
    /// A concrete typed-schema fallback has precedence over its applied API
    /// schemas; applied schemas are checked in reverse composition order.
    pub fn fallback_value(
        &self,
        prim_type: &str,
        api_schemas: &[String],
        name: &str,
    ) -> Option<&sdf::Value> {
        self.property_in(prim_type, name)
            .and_then(|property| property.default_value.as_ref())
            .or_else(|| {
                api_schemas
                    .iter()
                    .rev()
                    .chain(
                        self.typed_api_schemas
                            .get(prim_type)
                            .into_iter()
                            .flatten()
                            .rev(),
                    )
                    .find_map(|schema| {
                        self.property_in(schema, name)
                            .and_then(|property| property.default_value.as_ref())
                    })
            })
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
    pub fn ui_hint(&self, name: &str) -> Option<AttrUiHint> {
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
pub fn ui_hint_of(name: &str) -> Option<AttrUiHint> {
    SchemaRegistry::global()
        .read()
        .ok()
        .and_then(|r| r.ui_hint(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    const INLINE_SCHEMA: &str = r#"#usda 1.0
(
    upAxis = "Y"
)
class "ProbeAPI" (
    customData = { token apiSchemaType = "singleApply" }
)
{
    uniform token probe:label = "x"
    double probe:length = 2.0 (
        customData = { string lunco:unit = "length" }
    )
}
"#;

    #[test]
    fn inline_schema_is_typed_and_unit_annotated() {
        let mut registry = SchemaRegistry::default();
        assert!(registry.ingest(INLINE_SCHEMA, true));
        assert!(registry.api_schemas().contains(&"ProbeAPI".to_owned()));

        let label = registry
            .property("probe:label")
            .expect("inline schema property");
        assert_eq!(label.type_name, "token");
        assert_eq!(label.variability, sdf::Variability::Uniform);
        assert_eq!(
            registry.linear_unit("ProbeAPI", "probe:length"),
            LinearUnit::Length {
                stage_units_per_unit: 1.0,
            }
        );
        assert!(registry.is_custom("lunco:unknown"));
        assert!(!registry.is_custom("physics:mass"));
    }

    #[test]
    fn invalid_schema_is_rejected_without_mutating_registry() {
        let mut registry = SchemaRegistry::default();
        assert!(!registry.ingest("not USDA", true));
        assert!(registry.prim_types().is_empty());
        assert!(registry.api_schemas().is_empty());
    }

    #[test]
    fn core_linear_units_apply_across_incremental_core_sources() {
        let mut registry = SchemaRegistry::default();
        let camera = r#"#usda 1.0
class "Camera"
{
    float focalLength = 50
}
"#;
        let sphere = r#"#usda 1.0
class "Sphere"
{
    double radius = 1
}
"#;

        assert!(registry.ingest(camera, false));
        registry.apply_core_linear_units();
        assert_eq!(
            registry.linear_unit("Camera", "focalLength"),
            LinearUnit::Length {
                stage_units_per_unit: 0.1,
            }
        );
        assert!(
            registry
                .missing_core_linear_units()
                .any(|missing| missing == ("Sphere", "radius"))
        );

        assert!(registry.ingest(sphere, false));
        registry.apply_core_linear_units();
        assert_eq!(
            registry.linear_unit("Sphere", "radius"),
            LinearUnit::Length {
                stage_units_per_unit: 1.0,
            }
        );
        assert!(
            !registry
                .missing_core_linear_units()
                .any(|missing| missing == ("Camera", "focalLength")
                    || missing == ("Sphere", "radius"))
        );
    }
}
