//! `StageView` — composed reads over a **live** openusd `Stage` (Ph0′ substrate).
//!
//! The legacy pipeline flattens a composed `Stage` into a Send-safe [`sdf::Data`]
//! and reads it through [`UsdDataExt`](crate::usd_data::UsdDataExt). `StageView`
//! offers the SAME typed reads directly against the live (`!Send`) `Stage`, so the
//! domain extractors can be re-pointed from flattened data to the canonical stage
//! with no change in semantics — proven by the parity tests in [`crate::compose`]
//! / this module.
//!
//! Reads are default-time composed opinions (LIVRPS): references, sublayers,
//! variants, and inherits are resolved by the stage, exactly as the flattened
//! reader saw them post-flatten. (Time-sampled / animation reads migrate with the
//! animation projector in a later slice — not needed for S1 parity.)

use openusd::sdf::{Path as SdfPath, Value};
use openusd::usd::{PrimPredicate, Stage};

/// A borrow of a live composed [`Stage`] offering [`UsdDataExt`]-equivalent typed
/// reads. `!Send` — construct per-system from a `NonSend` `CanonicalStage`.
///
/// [`UsdDataExt`]: crate::usd_data::UsdDataExt
pub struct StageView<'a> {
    stage: &'a Stage,
    /// Precomputed binary (glTF) arc sites, so `resolved_asset` can synthesize
    /// `lunco:resolvedAsset` off the LIVE stage the way `flatten_stage` does.
    /// `None` for a bare `StageView::new` (tests / non-canonical reads).
    binary_sites: Option<&'a crate::compose::BinarySites>,
}

impl<'a> StageView<'a> {
    pub fn new(stage: &'a Stage) -> Self {
        Self { stage, binary_sites: None }
    }

    /// Construct with the stage's precomputed binary-arc sites (from
    /// [`CanonicalStage`](crate::CanonicalStage)), enabling `resolved_asset`.
    pub(crate) fn with_binary_sites(
        stage: &'a Stage,
        sites: &'a crate::compose::BinarySites,
    ) -> Self {
        Self { stage, binary_sites: Some(sites) }
    }

    /// The underlying stage (escape hatch for reads not yet wrapped).
    pub fn stage(&self) -> &Stage {
        self.stage
    }

    /// The precomputed binary-arc sites, if this view carries them (used by the
    /// `UsdRead::resolved_asset` synth).
    pub(crate) fn binary_sites(&self) -> Option<&crate::compose::BinarySites> {
        self.binary_sites
    }

    /// A prim's composed `typeName` (e.g. `"Xform"`, `"Mesh"`), if any.
    /// Mirrors [`UsdDataExt::prim_type_name`](crate::usd_data::UsdDataExt::prim_type_name).
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
    /// [`UsdDataExt::prim_attribute_value`](crate::usd_data::UsdDataExt::prim_attribute_value).
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
    /// `Token`, and `AssetPath` (the `@…@` form). Mirrors the legacy
    /// `get_attribute_as_string` free helper.
    pub fn value_str(&self, prim: &SdfPath, name: &str) -> Option<String> {
        match self.stage.prim(prim.clone()).attribute(name).get::<Value>().ok().flatten()? {
            Value::String(s) => Some(s),
            Value::Token(t) => Some(t.to_string()),
            Value::AssetPath(a) => Some(a.as_str().to_string()),
            _ => None,
        }
    }

    /// Immediate composed prim children of `prim`.
    /// Mirrors [`UsdDataExt::prim_children`](crate::usd_data::UsdDataExt::prim_children).
    pub fn prim_children(&self, prim: &SdfPath) -> Vec<SdfPath> {
        self.stage
            .prim(prim.clone())
            .children()
            .map(|cs| cs.iter().map(|c| c.path().clone()).collect())
            .unwrap_or_default()
    }

    /// Composed, path-translated targets of relationship `name` on `prim` (the
    /// PCP-resolved targets the flattened reader stored under `targetPaths`).
    pub fn rel_targets(&self, prim: &SdfPath, name: &str) -> Vec<SdfPath> {
        self.stage
            .prim(prim.clone())
            .relationship(name)
            .targets()
            .unwrap_or_default()
    }

    /// First composed target of relationship `name` on `prim` (`None` if
    /// absent/empty). Mirrors the legacy `read_rel_target` free helper.
    pub fn rel_target(&self, prim: &SdfPath, name: &str) -> Option<SdfPath> {
        self.rel_targets(prim, name).into_iter().next()
    }

    /// Whether `prim` applies the named API schema, by exact token match against
    /// its composed `apiSchemas`. Mirrors the `has_api_schema` free helper — the
    /// read `lunco-usd-avian`'s physics extractor uses for body/collider/terrain
    /// detection (`PhysicsRigidBodyAPI` / `PhysicsCollisionAPI` / `PhysxTerrainAPI`).
    pub fn has_api_schema(&self, prim: &SdfPath, schema_name: &str) -> bool {
        self.stage
            .prim(prim.clone())
            .api_schemas()
            .map(|v| v.iter().any(|s| s.as_str() == schema_name))
            .unwrap_or(false)
    }

    /// Attribute `name` on `prim` as a 3-vector (`double3`/`float3`). Mirrors the
    /// legacy `get_attribute_as_vec3` free helper.
    pub fn value_vec3(&self, prim: &SdfPath, name: &str) -> Option<[f64; 3]> {
        self.value::<[f64; 3]>(prim, name)
    }

    /// Every live (active, defined, non-abstract) composed prim path, in
    /// traversal order — the same set the flattened reader contains.
    pub fn prim_paths(&self) -> Vec<SdfPath> {
        let mut paths = Vec::new();
        let _ = self
            .stage
            .traverse(PrimPredicate::DEFAULT, |p| paths.push(p.clone()));
        paths
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod parity_tests {
    //! S1 parity: `StageView` (live composed stage) must return the SAME reads as
    //! `UsdDataExt` (the flattened `sdf::Data`). This is the invariant that lets
    //! the domain extractors be re-pointed off the flatten with no behaviour
    //! change (Ph0′ / impl-spec S1).

    use super::StageView;
    use crate::compose::{compose_file_to_stage, flatten_stage};
    use crate::usd_data::UsdDataExt;
    use openusd::sdf::{Path as SdfPath, SpecType, Value};
    use std::collections::BTreeSet;

    fn asset(rel: &str) -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets")
            .join(rel)
    }

    /// `lunco:resolvedAsset` is SYNTHESIZED at flatten time from binary (glTF)
    /// arcs — it is not an authored stage attribute, so a live-stage read returns
    /// nothing. Excluded here; the projector re-synthesizes it in a later slice.
    const FLATTEN_ONLY_ATTRS: &[&str] = &["lunco:resolvedAsset"];

    fn assert_parity(path: &std::path::Path) {
        let stage = compose_file_to_stage(path).expect("compose stage");
        let flat = flatten_stage(&stage).expect("flatten");
        let view = StageView::new(&stage);

        // 1. Same live prim set.
        let view_prims: BTreeSet<String> =
            view.prim_paths().iter().map(|p| p.to_string()).collect();
        let flat_prims: BTreeSet<String> = flat
            .iter()
            .filter(|(_, s)| s.ty == SpecType::Prim)
            .map(|(p, _)| p.to_string())
            .collect();
        assert_eq!(view_prims, flat_prims, "prim set must match for {path:?}");

        // 2. Same typeName per prim.
        for p in view.prim_paths() {
            assert_eq!(
                view.prim_type_name(&p),
                flat.prim_type_name(&p),
                "typeName mismatch at {p} in {path:?}"
            );
        }

        // 2b. Same apiSchemas per prim (the physics-extractor detection read).
        for p in view.prim_paths() {
            let flat_schemas: Vec<String> = match flat.field(&p, "apiSchemas") {
                Some(Value::TokenVec(v)) => v.iter().map(|t| t.to_string()).collect(),
                Some(Value::Token(t)) => vec![t.to_string()],
                Some(Value::String(s)) => vec![s.clone()],
                _ => Vec::new(),
            };
            for schema in &flat_schemas {
                assert!(
                    view.has_api_schema(&p, schema),
                    "StageView missing apiSchema `{schema}` at {p} in {path:?}"
                );
            }
            assert!(
                !view.has_api_schema(&p, "__NoSuchSchema__"),
                "false-positive apiSchema at {p}"
            );
        }

        // 3. Same default value for every flattened attribute (Debug-string
        //    compare, independent of Value: PartialEq).
        for (attr_path, spec) in flat.iter() {
            if spec.ty != SpecType::Attribute {
                continue;
            }
            let s = attr_path.to_string();
            let Some((prim_s, name)) = s.rsplit_once('.') else {
                continue;
            };
            if FLATTEN_ONLY_ATTRS.contains(&name) {
                continue;
            }
            let Ok(prim) = SdfPath::new(prim_s) else {
                continue;
            };
            let flat_v = flat.prim_attribute_value::<Value>(&prim, name);
            let view_v = view.value::<Value>(&prim, name);
            assert_eq!(
                format!("{view_v:?}"),
                format!("{flat_v:?}"),
                "attr `{s}` default mismatch (view vs flat) in {path:?}"
            );
        }
    }

    /// Real shipped rover: `subLayers` + `inherits` pulling a control profile
    /// from another file — exercises cross-file sublayer + inherit composition.
    #[test]
    fn stageview_matches_flatten_skid_rover() {
        assert_parity(&asset("vessels/rovers/skid_rover.usda"));
    }

    /// Sandbox scene: inline `inherits` through its own subLayer + a `references`
    /// arc whose own subLayer+inherits composes through the reference.
    #[test]
    fn stageview_matches_flatten_lander_scene() {
        assert_parity(&asset("scenes/sandbox/lander_test.usda"));
    }

    /// Spot-check the composed opinion both readers must agree on (the
    /// cross-file inherit landing `lunco:port = throttle` on the vessel).
    #[test]
    fn stageview_reads_composed_inherit_opinion() {
        let stage = compose_file_to_stage(&asset("vessels/rovers/skid_rover.usda"))
            .expect("compose stage");
        let view = StageView::new(&stage);
        let fwd = SdfPath::new("/SkidRover/Controls/forward").unwrap();
        assert_eq!(
            view.value_str(&fwd, "lunco:port").as_deref(),
            Some("throttle"),
            "StageView must read the cross-file inherited composed opinion"
        );
    }
}
