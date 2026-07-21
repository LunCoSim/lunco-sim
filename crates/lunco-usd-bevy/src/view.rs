//! `StageView` — composed reads over a **live** openusd `Stage` (Ph0′ substrate).
//!
//! This is the ONE composed-read source: typed reads straight against the live
//! (`!Send`) `Stage`, which every domain extractor reads through.
//!
//! Reads are default-time composed opinions (LIVRPS): references, sublayers,
//! variants, and inherits are resolved by the stage. (Time-sampled / animation
//! reads live with the animation projector, not here.)

use openusd::sdf::{Path as SdfPath, Value};
use openusd::usd::Stage;

/// A borrow of a live composed [`Stage`] offering [`UsdDataExt`]-equivalent typed
/// reads. `!Send` — construct per-system from a `NonSend` `CanonicalStage`.
///
/// [`UsdDataExt`]: crate::usd_data::UsdDataExt
pub struct StageView<'a> {
    stage: &'a Stage,
    /// Precomputed binary (glTF) arc sites, so `resolved_asset` can synthesize
    /// `lunco:resolvedAsset` off the LIVE stage without re-walking the arcs.
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
    /// `Token`, and `AssetPath` (the `@…@` form). Inherent helper for the reads
    /// whose value type is genuinely either (`lunco:resolvedAsset`, authored by
    /// the composer as a path but read as plain text).
    pub fn value_str(&self, prim: &SdfPath, name: &str) -> Option<String> {
        match self.stage.prim(prim.clone()).attribute(name).get::<Value>().ok().flatten()? {
            Value::String(s) => Some(s),
            Value::Token(t) => Some(t.to_string()),
            Value::AssetPath(a) => Some(a.as_str().to_string()),
            _ => None,
        }
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

    /// Attribute `name` on `prim` as a 3-vector (`double3`/`float3`). Mirrors the
    /// legacy `get_attribute_as_vec3` free helper.
    pub fn value_vec3(&self, prim: &SdfPath, name: &str) -> Option<[f64; 3]> {
        self.value::<[f64; 3]>(prim, name)
    }
}

// `rel_target`, `has_api_schema`, `prim_paths` and `children` live on
// [`UsdRead`] ONLY — never add an inherent method of the same name. A Rust
// inherent method silently shadows the trait method, so a duplicate pair can
// differ in return type and each call site picks whichever is in scope. One
// name, one definition.

#[cfg(all(test, not(target_arch = "wasm32")))]
mod compose_tests {
    //! Live-stage composition reads through `StageView` (the openusd composed
    //! Stage): the cross-file inherit/subLayer opinion the entity translator
    //! consumes must land on the vessel after full PCP composition.

    use super::StageView;
    use crate::compose::compose_file_to_stage;
    use crate::UsdRead;
    use openusd::sdf::Path as SdfPath;

    fn asset(rel: &str) -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets")
            .join(rel)
    }

    /// Spot-check the composed opinion the live reader must resolve (the
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

    /// A local `over` must win over the prim it overrides through a `references` arc.
    ///
    /// This is the exact composition shape the video campaign's episode scenes use:
    /// an episode scene `references` a base scene and then re-authors the look of one
    /// of its prims with a sibling `over`, so the base stays usable on its own (it
    /// doubles as the interactive driving tutorial) while the episode gets film
    /// lighting out of the same source.
    ///
    /// Asserted because it silently was NOT happening. MEASURED: episode 01 rendered
    /// its ground at the BASE scene's albedo — mean luma 135/255, a near-white slab —
    /// while its `over "Ground"` authored `(0.13, 0.125, 0.12)` plus a
    /// `material:binding` to the regolith shader. Both opinions were dropped. Episode
    /// 02, which authors the identical look as a plain `def` in its own scene,
    /// rendered at luma 63/255 under an identical sun and exposure.
    ///
    /// An inert `over` is the worst failure mode available here: the scene reads as
    /// though the look were authored, review passes, and the render quietly uses the
    /// base.
    #[test]
    fn local_over_wins_over_a_referenced_prims_opinion() {
        let dir = std::env::temp_dir().join("lunco_over_ref_test");
        std::fs::create_dir_all(&dir).expect("scratch dir");

        // The base: the shape of `assets/scenes/sandbox/lander_ops.usda`.
        std::fs::write(
            dir.join("base.usda"),
            "#usda 1.0\n\
             def Xform \"Base\"\n\
             {\n\
                 def Cube \"Ground\"\n\
                 {\n\
                     float lunco:test:albedo = 0.35\n\
                 }\n\
             }\n",
        )
        .expect("write base");

        // The episode: reference the base, then override that ground's look.
        std::fs::write(
            dir.join("episode.usda"),
            "#usda 1.0\n\
             def Xform \"Episode\" (\n\
                 prepend references = @./base.usda@</Base>\n\
             )\n\
             {\n\
                 over \"Ground\"\n\
                 {\n\
                     float lunco:test:albedo = 0.13\n\
                 }\n\
             }\n",
        )
        .expect("write episode");

        let stage = compose_file_to_stage(&dir.join("episode.usda")).expect("compose episode");
        let view = StageView::new(&stage);
        let ground = SdfPath::new("/Episode/Ground").unwrap();

        // First: does the referenced child exist at all under the referencing prim?
        // If this fails the `over` is moot — the reference arc itself never brought
        // the child across, which is a different (larger) bug.
        let children = view.children(&SdfPath::new("/Episode").unwrap());
        assert!(
            children.iter().any(|c| c.as_str().ends_with("/Ground")),
            "the `references` arc must bring the base's `Ground` child across; got {children:?}"
        );

        let albedo = view
            .value::<f32>(&ground, "lunco:test:albedo")
            .expect("composed ground must carry the attribute at all");

        assert!(
            (albedo - 0.13).abs() < 1e-4,
            "the local `over` must win over the referenced base opinion, got {albedo} \
             (0.35 means the `over` was dropped and the base opinion survived)"
        );
    }
}
