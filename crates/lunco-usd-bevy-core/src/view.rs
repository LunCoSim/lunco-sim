//! `StageView` — composed reads over a **live** openusd `Stage` (Ph0′ substrate).
//!
//! This is the ONE composed-read source: typed reads straight against the live
//! (`!Send`) `Stage`, which every domain extractor reads through.
//!
//! Reads are default-time composed opinions (LIVRPS): references, sublayers,
//! variants, and inherits are resolved by the stage. (Time-sampled / animation
//! reads live with the animation projector, not here.)

use openusd::sdf::{Path as SdfPath, Value};
use openusd::usd::{compute_included_paths, Collection, PrimPredicate, Stage};

/// A borrow of a live composed [`Stage`] offering [`UsdDataExt`]-equivalent typed
/// reads. `!Send` — construct per-system from the runtime adapter's `NonSend`
/// canonical stage.
///
/// [`UsdDataExt`]: lunco_usd_data::usd_data::UsdDataExt
pub struct StageView<'a> {
    stage: &'a Stage,
}

impl<'a> StageView<'a> {
    pub fn new(stage: &'a Stage) -> Self {
        Self { stage }
    }

    /// The underlying stage (escape hatch for reads not yet wrapped).
    pub fn stage(&self) -> &Stage {
        self.stage
    }

    /// A prim's composed `typeName` (e.g. `"Xform"`, `"Mesh"`), if any.
    /// Mirrors [`UsdDataExt::prim_type_name`](lunco_usd_data::usd_data::UsdDataExt::prim_type_name).
    pub fn prim_type_name(&self, prim: &SdfPath) -> Option<String> {
        self.stage
            .prim(prim.clone())
            .type_name()
            .ok()
            .flatten()
            .map(|t| t.to_string())
    }

    /// The default-time composed value of attribute `name` on `prim`, typed as
    /// `T`. Mirrors
    /// [`UsdDataExt::prim_attribute_value`](lunco_usd_data::usd_data::UsdDataExt::prim_attribute_value).
    pub fn value<T>(&self, prim: &SdfPath, name: &str) -> Option<T>
    where
        T: TryFrom<Value>,
        T::Error: std::error::Error + Send + Sync + 'static,
    {
        self.stage
            .prim(prim.clone())
            .attribute(name)
            .get::<T>()
            .ok()
            .flatten()
    }

    /// Attribute `name` on `prim` coerced to a string — handles `String`,
    /// `Token`, and `AssetPath` (the `@…@` form). Inherent helper for the reads
    /// whose value type is genuinely either a path authored as an asset value
    /// or plain text.
    pub fn value_str(&self, prim: &SdfPath, name: &str) -> Option<String> {
        match self
            .stage
            .prim(prim.clone())
            .attribute(name)
            .get::<Value>()
            .ok()
            .flatten()?
        {
            Value::String(s) => Some(s),
            Value::Token(t) => Some(t.to_string()),
            Value::AssetPath(a) => Some(a.as_str().to_string()),
            _ => None,
        }
    }

    /// Composed, path-translated targets of relationship `name` on `prim`.
    pub fn rel_targets(&self, prim: &SdfPath, name: &str) -> Vec<SdfPath> {
        self.stage
            .prim(prim.clone())
            .relationship(name)
            .targets()
            .unwrap_or_default()
    }

    /// Composed members of a standard USD collection.
    ///
    /// `explicitOnly` can use the relationship targets directly. Every other
    /// expansion rule goes through OpenUSD so excludes, subtree expansion,
    /// references, and variants retain standard semantics.
    pub fn collection_members(
        &self,
        prim: &SdfPath,
        instance_name: &str,
    ) -> Result<Vec<SdfPath>, String> {
        let expansion = format!("collection:{instance_name}:expansionRule");
        let includes = format!("collection:{instance_name}:includes");
        if self.value_str(prim, &expansion).as_deref() == Some("explicitOnly") {
            return Ok(self.rel_targets(prim, &includes));
        }

        let collection = Collection::new(prim.clone(), instance_name);
        let query = collection
            .compute_membership_query(self.stage)
            .map_err(|error| format!("could not compute collection membership: {error}"))?;
        compute_included_paths(self.stage, &query, PrimPredicate::DEFAULT)
            .map_err(|error| format!("could not expand collection membership: {error}"))
    }

    /// Attribute `name` on `prim` as a 3-vector (`double3`/`float3`).
    pub fn value_vec3(&self, prim: &SdfPath, name: &str) -> Option<[f64; 3]> {
        self.value::<[f64; 3]>(prim, name)
    }
}

// `rel_target`, `has_api_schema`, `prim_paths` and `children` live on
// [`UsdRead`] ONLY — never add an inherent method of the same name. A Rust
// inherent method silently shadows the trait method, so a duplicate pair can
// differ in return type and each call site picks whichever is in scope. One
// name, one definition.
