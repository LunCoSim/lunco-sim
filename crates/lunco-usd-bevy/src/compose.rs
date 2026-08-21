//! Compose a USD stage with openusd from an in-memory layer closure, for the
//! runtime's live [`CanonicalStage`](crate::canonical::CanonicalStage) — read
//! through [`StageView`](crate::view::StageView), never flattened.
//!
//! Pipeline:
//!  1. **Pre-fetch BFS** ([`fetch_layer_closure`]) — discover every
//!     transitively-referenced `.usda` and fetch its bytes via
//!     `LoadContext::read_asset_bytes` (native + wasm, routed through Bevy's
//!     `AssetServer` + our registered sources). openusd's resolver is
//!     synchronous, so all async fetching happens here, up front.
//!  2. **Compose** ([`build_stage_with_resolver`]) —
//!     `Stage::builder().resolver(LuncoUsdResolver).open(root)` runs the real PCP
//!     engine: references, payloads, variant selection, relationship-target
//!     translation — all filesystem-free. The composed `Stage` is the runtime
//!     source of truth; downstream reads it via `StageView`.
//!
//! Binary assets (`.glb`/`.gltf`/…) are not USD layers: the resolver routes them
//! to an empty composition stub, while the render projection reads the
//! authored binary arc directly from the live prim stack. That keeps the USD
//! payload/reference as the only asset identity and means live authoring and
//! referenced wrappers use the same path (openusd has no `SdfFileFormat` plugin
//! system).

use std::collections::HashMap;

use anyhow::{anyhow, Result};
use bevy::asset::{AssetPath, LoadContext};
#[cfg(test)]
use openusd::sdf::Path as SdfPath;
use openusd::usd::Stage;

use lunco_assets::asset_path::canonicalize_root;

use crate::canonical::StageRecipe;
use lunco_usd_compose::{child_layer_ids, LuncoUsdResolver, SharedLayerBytes};

/// Async BFS that fetches the full transitive `.usda` layer closure into an
/// in-memory, `Send` [`StageRecipe`] — the **fetch** half of the loader's compose path.
/// Split out so the (main-thread, `!Send`) `Stage` build can be deferred: an
/// asset loader fetches the recipe off-thread, then a main-thread system builds
/// the canonical `Stage` from it (Ph0′ [`CanonicalStage::from_recipe`]).
///
/// [`CanonicalStage::from_recipe`]: crate::canonical::CanonicalStage::from_recipe
pub(crate) async fn fetch_layer_closure(
    load_context: &mut LoadContext<'_>,
    root_asset_path: &str,
    root_bytes: Vec<u8>,
) -> Result<StageRecipe> {
    let root_id = canonicalize_root(root_asset_path);

    // 1. Pre-fetch BFS — keyed by the SAME canonical id the resolver will use.
    let mut bytes: HashMap<String, Vec<u8>> = HashMap::new();
    bytes.insert(root_id.clone(), root_bytes);
    let mut queue = vec![root_id.clone()];

    while let Some(id) = queue.pop() {
        let raw = bytes
            .get(&id)
            .cloned()
            .expect("queued id is present in map");
        for child_id in child_layer_ids(&id, &raw)? {
            if bytes.contains_key(&child_id) {
                continue;
            }
            // Parse `child_id` as an `AssetPath` (NOT a `PathBuf`): only the
            // string form parses a `source://` scheme into an asset source.
            // `PathBuf::from("lunco://vessels/…")` keeps the whole string as a
            // default-source relative path → `assets/lunco://vessels/…` →
            // "Path not found". `AssetPath::parse` routes `lunco://…` to the
            // registered `lunco` source; plain relative ids stay default-source.
            let fetched = match load_context
                .read_asset_bytes(AssetPath::parse(&child_id).into_owned())
                .await
            {
                Ok(fetched) => fetched,
                Err(e) => {
                    return Err(anyhow!(
                        "USD composition dependency `{child_id}` referenced by `{id}` could not \
                         be fetched: {e}"
                    ));
                }
            };
            bytes.insert(child_id.clone(), fetched);
            queue.push(child_id);
        }
    }

    Ok(StageRecipe { root_id, bytes })
}

/// Test-only convenience: the composed [`Stage`] alone, discarding the resolver
/// handle. Production builds go through [`build_stage_with_resolver`] (via
/// [`CanonicalStage::from_recipe`](crate::canonical::CanonicalStage::from_recipe))
/// so runtime referenced spawns can inject layer bytes into the live resolver.
#[cfg(test)]
pub(crate) fn build_stage_from_closure(recipe: &StageRecipe) -> Result<Stage> {
    Ok(build_stage_with_resolver(recipe)?.0)
}

/// Like [`build_stage_from_closure`], but also returns the resolver's
/// [`SharedLayerBytes`] handle so the caller (the [`CanonicalStage`]) can inject
/// additional layer closures at runtime — the substrate for authoring a
/// **referenced spawn** onto a live stage: add the spawned asset's bytes here,
/// then author the `references` arc, and PCP composes the subtree on the next
/// read (demand-driven resolution).
///
/// [`CanonicalStage`]: crate::canonical::CanonicalStage
pub(crate) fn build_stage_with_resolver(recipe: &StageRecipe) -> Result<(Stage, SharedLayerBytes)> {
    let resolver = LuncoUsdResolver::new(recipe.bytes.clone());
    let shared = resolver.shared();
    let stage = Stage::builder()
        .resolver(resolver)
        .open(&recipe.root_id)
        .map_err(|e| anyhow!("USD composition error: {e}"))?;
    Ok((stage, shared))
}

/// Compose a USD layer from disk into a **live** [`Stage`] (read through
/// [`StageView`](crate::view::StageView), the production read path). Native +
/// synchronous, backed by [`openusd::ar::DefaultResolver`] — for tests and tools
/// that load a real on-disk `.usda` with every reference resolved, distinct from
/// the async `AssetServer`-driven loader (the storage-based recipe path).
/// `DefaultResolver` anchors each relative reference to its own layer's
/// directory, so the on-disk reference tree resolves exactly as authored.
#[cfg(not(target_arch = "wasm32"))]
pub fn compose_file_to_stage(path: &std::path::Path) -> Result<Stage> {
    lunco_usd_compose::compose_file_to_stage(path)
}

/// Compose an on-disk USD layer while resolving `lunco://` references against
/// an explicitly supplied shipped-asset root.
///
/// External Twin and campaign files do not live below the engine's `assets/`
/// directory, so their path cannot reveal where `lunco://` is mounted. Runtime
/// gets that mount from `AssetServer`; parse-only tools pass the same root here.
#[cfg(not(target_arch = "wasm32"))]
pub fn compose_file_to_stage_with_assets(
    path: &std::path::Path,
    assets_root: Option<&std::path::Path>,
) -> Result<Stage> {
    lunco_usd_compose::compose_file_to_stage_with_assets(path, assets_root)
}

// Writes USDA fixtures to a temp dir and composes them from disk. Native-only
// test code — the `std::fs` ban guards wasm *runtime* paths, and `clippy.toml`
// already names tests as exempt (cargo has no path-scoped lint config, so the
// exemption has to be written out).
#[cfg(all(test, not(target_arch = "wasm32")))]
#[allow(clippy::disallowed_methods)]
mod inherits_compose_tests {
    use super::*;
    use crate::{CanonicalStage, StageView, UsdRead};

    /// De-risk the control-profile design: a `class` carrying a `Controls` child
    /// scope, `inherits`-ed by a vessel prim, must land those child prims (with
    /// their attrs) under the vessel after full PCP flatten — so the entity
    /// translator can walk `<Vessel>/Controls/<intent>` to build a `ControlBinding`.
    #[test]
    fn inherits_from_class_brings_child_prims_into_flattened_data() {
        let usda = "#usda 1.0\n\
class \"_RoverControl\"\n{\n    def \"Controls\"\n    {\n        def \"forward\"\n        {\n            uniform string lunco:port = \"throttle\"\n            uniform double lunco:factor = 1\n        }\n    }\n}\n\
def Xform \"Rover\" (\n    inherits = </_RoverControl>\n)\n{\n}\n";
        let stage =
            build_stage_from_closure(&crate::StageRecipe::from_source("inherits.usda", usda))
                .expect("compose");
        let view = StageView::new(&stage);
        let fwd = SdfPath::new("/Rover/Controls/forward").unwrap();
        assert_eq!(
            view.value::<String>(&fwd, "lunco:port").as_deref(),
            Some("throttle"),
            "inherited Controls child must appear under /Rover with its attrs"
        );
        assert_eq!(view.value::<f64>(&fwd, "lunco:factor"), Some(1.0));
    }

    /// The real delivery mechanism: a vessel in one file pulls a control-profile
    /// `class` from ANOTHER file via `subLayers`, then `inherits` it — the
    /// `Controls` child scope must compose onto the vessel. Proves rovers/landers
    /// can share one profile file (DRY) without repeating bindings per asset.
    #[test]
    fn cross_file_sublayer_inherits_composes() {
        let dir = std::env::temp_dir().join("lunco_ctrl_profile_compose_test");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("control_profiles.usda"),
            "#usda 1.0\nclass \"_RoverControl\"\n{\n    def \"Controls\"\n    {\n        def \"forward\"\n        {\n            uniform string lunco:port = \"throttle\"\n            uniform double lunco:factor = 1\n        }\n    }\n}\n",
        )
        .unwrap();
        let rover = dir.join("rover.usda");
        std::fs::write(
            &rover,
            "#usda 1.0\n(\n    subLayers = [@./control_profiles.usda@]\n)\ndef Xform \"SkidRover\" (\n    inherits = </_RoverControl>\n)\n{\n}\n",
        )
        .unwrap();
        let stage = compose_file_to_stage(&rover).expect("compose stage");
        let view = StageView::new(&stage);
        let fwd = SdfPath::new("/SkidRover/Controls/forward").unwrap();
        assert_eq!(
            view.value::<String>(&fwd, "lunco:port").as_deref(),
            Some("throttle"),
            "cross-file subLayers+inherits must land the Controls scope on the vessel"
        );
    }

    /// End-to-end: the shipped `skid_rover.usda` inherits `_RoverControl` from
    /// the shared `control_profiles.usda`, so its composed form must carry
    /// `/SkidRover/Controls/forward` → `throttle`. Guards the real asset wiring.
    #[test]
    fn skid_rover_asset_inherits_control_profile() {
        let asset = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/vessels/rovers/skid_rover.usda");
        let stage = compose_file_to_stage(&asset).expect("compose skid_rover.usda");
        let view = StageView::new(&stage);
        let fwd = SdfPath::new("/SkidRover/Controls/forward").unwrap();
        assert_eq!(
            view.value::<String>(&fwd, "lunco:port").as_deref(),
            Some("throttle"),
            "skid_rover must inherit the rover control profile's Controls scope"
        );
        assert_eq!(view.value::<f64>(&fwd, "lunco:factor"), Some(1.0));
    }

    /// The two harder composition paths, on the real `lander_ops.usda`, where both
    /// vehicles now arrive by `references`: (a) a lander whose asset references the
    /// shared control profile — a reference nested INSIDE a reference, which must
    /// still land on the composed `/LanderTest/Lander/Controls`; (b) a rover pulled
    /// in by `references` whose OWN `subLayers`+`inherits` must compose THROUGH the
    /// reference arc.
    #[test]
    fn lander_scene_composes_nested_and_referenced_control_profiles() {
        let scene = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/scenes/luncosim/lander_ops.usda");
        let stage = compose_file_to_stage(&scene).expect("compose lander_ops.usda");
        let view = StageView::new(&stage);

        // (a) the lander asset's own Controls reference resolves through the arc
        //     that pulled the lander into the scene.
        let lander_fwd = SdfPath::new("/LanderTest/Lander/Controls/forward").unwrap();
        assert_eq!(
            view.value::<String>(&lander_fwd, "lunco:port").as_deref(),
            Some("pitch"),
            "referenced lander must carry the lander control profile through the reference"
        );
        // (b) referenced rover's subLayer+inherits composes through the ref arc.
        let rover_fwd = SdfPath::new("/LanderTest/SkidRover/Controls/forward").unwrap();
        assert_eq!(
            view.value::<String>(&rover_fwd, "lunco:port").as_deref(),
            Some("throttle"),
            "referenced rover must carry its inherited Controls through the reference"
        );
    }

    /// A binary `payload` authored inside a REFERENCED `.usda` wrapper must be
    /// found on the COMPOSED prim (`/Scene/Bldg/Visual`), with its URI anchored
    /// at the wrapper layer. This keeps USD as the source of truth —
    /// `scene → .usda → .glb` — while the render projection reads the live arc.
    #[test]
    fn glb_payload_in_referenced_wrapper_anchors_on_composed_prim() {
        // Wrapper: a `Structure` defaultPrim whose `Visual` child carries the glb
        // payload — the Perseverance "usda → glb" shape.
        let wrapper = "#usda 1.0\n(\n    defaultPrim = \"Structure\"\n)\ndef Xform \"Structure\"\n{\n    def Xform \"Visual\" (\n        prepend payload = @model.glb@\n    )\n    {\n        string lunco:assetMode = \"scene\"\n    }\n}\n";
        // Scene references the wrapper — no direct glb embedding in the scene.
        let scene = "#usda 1.0\ndef Xform \"Scene\"\n{\n    def Xform \"Bldg\" (\n        prepend references = @wrapper.usda@\n    )\n    {\n    }\n}\n";
        // Build the two-layer closure keyed exactly as the async loader's resolver
        // does (`canonicalize`), so the scene's `@wrapper.usda@` reference resolves
        // to the wrapper bytes and the `@model.glb@` payload is stubbed — the
        // storage-based compose path, not the removed native-fs path.
        let root_id = canonicalize_root("scene.usda");
        let wrapper_id = lunco_assets::asset_path::canonicalize("wrapper.usda", &root_id);
        let bytes = HashMap::from([
            (root_id.clone(), scene.as_bytes().to_vec()),
            (wrapper_id, wrapper.as_bytes().to_vec()),
        ]);
        let stage = build_stage_from_closure(&crate::StageRecipe { root_id, bytes })
            .expect("compose scene→wrapper→glb");
        let cs = CanonicalStage::from_stage(stage, "scene.usda");

        let visual = SdfPath::new("/Scene/Bldg/Visual").unwrap();
        let resolved = cs
            .view()
            .binary_asset_uri(&visual)
            .expect("binary payload must be read from the composed Visual prim");
        assert!(
            resolved.ends_with("model.glb"),
            "binary asset URI should point at the wrapper-co-located glb, got {resolved}"
        );
    }

    #[test]
    fn binary_asset_uri_rejects_multiple_authored_binary_arcs() {
        let source = "#usda 1.0\n\
def Xform \"Visual\" (\n\
    prepend payload = @model.glb@\n\
    prepend references = @alternate.glb@\n\
)\n\
{\n\
}\n";
        let stage =
            build_stage_from_closure(&crate::StageRecipe::from_source("scene.usda", source))
                .expect("compose binary arcs");
        let view = StageView::new(&stage);
        let visual = SdfPath::new("/Visual").unwrap();

        assert!(
            view.binary_asset_uri(&visual).is_none(),
            "ambiguous binary arcs must not choose an arbitrary asset"
        );
    }

    #[test]
    fn binary_asset_uri_tracks_live_reference_authoring() {
        let source = "#usda 1.0\ndef Xform \"Visual\" {}\n";
        let recipe = crate::StageRecipe::from_source("scene.usda", source);
        let stage = CanonicalStage::from_recipe(&recipe).expect("compose live binary-arc fixture");
        let visual = SdfPath::new("/Visual").unwrap();

        assert!(stage.view().binary_asset_uri(&visual).is_none());
        stage
            .projector()
            .author_reference(&visual, "first.glb")
            .expect("author first live binary reference");
        let first = stage
            .view()
            .binary_asset_uri(&visual)
            .expect("live binary reference must be visible immediately");
        assert!(first.ends_with("first.glb"), "got {first}");

        stage
            .projector()
            .author_reference(&visual, "second.glb")
            .expect("author second live binary reference");
        assert!(
            stage.view().binary_asset_uri(&visual).is_none(),
            "adding a second live binary reference must become an explicit ambiguity"
        );
    }
}
