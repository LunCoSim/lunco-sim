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
//! to an empty stub during composition, and [`discover_binary_sites`] records
//! their URI so `StageView::resolved_asset` can surface a synthesized
//! `lunco:resolvedAsset` on the composed prim (openusd has no `SdfFileFormat`
//! plugin system). Binary arcs authored in the root layer (our only case — the
//! Perseverance glTF) keep their composed prim path; binary arcs authored
//! *inside* a referenced sub-asset are not remapped and would be skipped (none
//! exist today).

use std::collections::HashMap;

use anyhow::{anyhow, Result};
use bevy::asset::{AssetPath, LoadContext};
use openusd::ar::ResolvedPath;
use openusd::sdf::{Path as SdfPath, Value};
use openusd::usd::{PrimPredicate, Stage};
use openusd::usda;

use lunco_assets::asset_path::{canonicalize, canonicalize_root};

use crate::canonical::StageRecipe;
use crate::resolver::{
    canonicalize_at, is_binary_asset, LuncoUsdResolver,
    SharedLayerBytes,
};

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
        let raw = bytes.get(&id).cloned().expect("queued id is present in map");
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
            let fetched = load_context
                .read_asset_bytes(AssetPath::parse(&child_id).into_owned())
                .await
                .map_err(|e| anyhow!("failed to fetch sublayer {child_id}: {e}"))?;
            bytes.insert(child_id.clone(), fetched);
            queue.push(child_id);
        }
    }

    Ok(StageRecipe { root_id, bytes })
}

/// The closure ids a layer's bytes reference, canonicalized against that layer as
/// anchor — the discovery half of the pre-fetch BFS, shared by both fetchers
/// ([`fetch_layer_closure`] over Bevy's `AssetServer`, [`compose_file_to_stage`]
/// over the filesystem). Only they differ in how bytes arrive; *which* layers a
/// closure needs is one rule, so it lives in one place.
///
/// Only the non-binary `.usda` closure is walked: binary-asset arcs (glTF/…) are
/// discovered post-composition by [`discover_binary_sites`] so an arc authored
/// inside a referenced `.usda` wrapper anchors on its COMPOSED prim.
fn child_layer_ids(id: &str, raw: &[u8]) -> Result<Vec<String>> {
    let text = std::str::from_utf8(raw).map_err(|e| anyhow!("layer {id} is not UTF-8: {e}"))?;
    let data = usda::parse(text).map_err(|e| anyhow!("USD parse error in {id}: {e}"))?;
    let anchor = ResolvedPath::new(id);
    Ok(
        crate::closure::discover_arcs(&data, crate::closure::ArcFilter::LayersOnly)
            .iter()
            .map(|child| canonicalize_at(child, Some(&anchor)))
            .collect(),
    )
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


/// Discover every binary-asset arc (glTF/OBJ/STL, per [`is_binary_asset`])
/// authored across the composed stage's loaded layers, keyed by its authoring
/// site `(layer identifier, spec path)` — the coordinates
/// [`openusd::usd::Prim::prim_stack`] reports.
/// [`StageView::resolved_asset`](crate::view::StageView::resolved_asset) matches
/// these against a composed prim's stack to synthesize `lunco:resolvedAsset` on
/// the COMPOSED prim, so a glTF `payload`/`reference` authored inside a
/// referenced `.usda` wrapper surfaces on the composed prim — not only arcs
/// authored directly in the root layer.
pub(crate) type BinarySites = HashMap<(String, SdfPath), String>;

pub(crate) fn discover_binary_sites(stage: &Stage) -> BinarySites {
    let mut sites: HashMap<(String, SdfPath), String> = HashMap::new();
    // Force every reachable reference/payload layer to load so `layer_identifiers()`
    // sees the whole stack. `flatten_stage` gets this for free by traversing first;
    // called standalone (canonical build) a binary arc authored in a referenced /
    // payload wrapper would otherwise be missed (its layer isn't loaded yet).
    let _ = stage.traverse(PrimPredicate::DEFAULT, |_| {});
    for layer_id in stage.layer_identifiers() {
        let Some(layer) = stage.layer(&layer_id) else { continue };
        let data = layer.data();
        let anchor = ResolvedPath::new(&layer_id);
        for path in data.spec_paths() {
            let mut arcs: Vec<String> = Vec::new();
            if let Ok(Some(v)) = data.try_field(&path, "references") {
                if let Value::ReferenceListOp(op) = v.as_ref() {
                    arcs.extend(op.iter().filter(|r| !r.asset_path.is_empty()).map(|r| r.asset_path.clone()));
                }
            }
            if let Ok(Some(v)) = data.try_field(&path, "payload") {
                match v.as_ref() {
                    Value::Payload(p) if !p.asset_path.is_empty() => arcs.push(p.asset_path.clone()),
                    Value::PayloadListOp(op) => {
                        arcs.extend(op.iter().filter(|p| !p.asset_path.is_empty()).map(|p| p.asset_path.clone()))
                    }
                    _ => {}
                }
            }
            for ap in arcs {
                if is_binary_asset(&ap) {
                    sites.insert((layer_id.clone(), path.clone()), canonicalize_at(&ap, Some(&anchor)));
                }
            }
        }
    }
    sites
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
    // Anchor the root at `lunco://` when the file lives under a shipped-asset
    // root. `canonicalize` passes `scheme://` ids through and PRESERVES the scheme
    // when anchoring a relative child, so one `lunco://` root makes every id in the
    // closure uniformly `lunco://` — a single resolution rule for the whole walk.
    let assets_root = lunco_assets::shipped_asset_root(path);
    let root_id = match assets_root.and_then(|root| path.strip_prefix(root).ok()) {
        Some(rel) => {
            lunco_assets::engine_asset_uri(&rel.to_string_lossy().replace('\\', "/"))
        }
        // NOT the raw path: every id in the map must be keyed by `canonicalize`,
        // the same function the resolver's `create_identifier` applies, or the
        // lookup misses and composition fails to resolve its own root layer.
        None => canonicalize_root(&path.to_string_lossy()),
    };

    let root_bytes =
        std::fs::read(path).map_err(|e| anyhow!("cannot read {}: {e}", path.display()))?;
    let mut bytes: HashMap<String, Vec<u8>> = HashMap::new();
    bytes.insert(root_id.clone(), root_bytes);
    let mut queue = vec![root_id.clone()];

    while let Some(id) = queue.pop() {
        let raw = bytes.get(&id).cloned().expect("queued id is present in map");
        for child_id in child_layer_ids(&id, &raw)? {
            if bytes.contains_key(&child_id) {
                continue;
            }
            let file = id_to_disk_path(&child_id, assets_root)?;
            let fetched = std::fs::read(&file).map_err(|e| {
                anyhow!("failed to fetch sublayer {child_id} from {}: {e}", file.display())
            })?;
            bytes.insert(child_id.clone(), fetched);
            queue.push(child_id);
        }
    }

    Ok(build_stage_with_resolver(&StageRecipe { root_id, bytes })?.0)
}

/// Where an id's bytes live on disk, as an error rather than an `Option` — the
/// mapping itself belongs to `lunco-assets` (it is asset-location knowledge, not
/// USD composition); this only supplies the composition-side diagnostic.
fn id_to_disk_path(
    id: &str,
    assets_root: Option<&std::path::Path>,
) -> Result<std::path::PathBuf> {
    lunco_assets::id_to_disk_path(id, assets_root).ok_or_else(|| {
        anyhow!("`{id}` is a shipped-asset ref, but the composed file is outside any `assets/` root")
    })
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
class \"_RoverControl\"\n{\n    def \"Controls\"\n    {\n        def \"forward\"\n        {\n            uniform string lunco:port = \"throttle\"\n            uniform double lunco:scale = 1\n        }\n    }\n}\n\
def Xform \"Rover\" (\n    inherits = </_RoverControl>\n)\n{\n}\n";
        let stage = build_stage_from_closure(&crate::StageRecipe::from_source("inherits.usda", usda))
            .expect("compose");
        let view = StageView::new(&stage);
        let fwd = SdfPath::new("/Rover/Controls/forward").unwrap();
        assert_eq!(
            view.value::<String>(&fwd, "lunco:port").as_deref(),
            Some("throttle"),
            "inherited Controls child must appear under /Rover with its attrs"
        );
        assert_eq!(view.value::<f64>(&fwd, "lunco:scale"), Some(1.0));
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
            "#usda 1.0\nclass \"_RoverControl\"\n{\n    def \"Controls\"\n    {\n        def \"forward\"\n        {\n            uniform string lunco:port = \"throttle\"\n            uniform double lunco:scale = 1\n        }\n    }\n}\n",
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
        assert_eq!(view.value::<f64>(&fwd, "lunco:scale"), Some(1.0));
    }

    /// The two harder composition paths, on the real `lander_test.usda`, where both
    /// vehicles now arrive by `references`: (a) a lander whose asset references the
    /// shared control profile — a reference nested INSIDE a reference, which must
    /// still land on the composed `/LanderTest/Lander/Controls`; (b) a rover pulled
    /// in by `references` whose OWN `subLayers`+`inherits` must compose THROUGH the
    /// reference arc.
    #[test]
    fn lander_scene_composes_nested_and_referenced_control_profiles() {
        let scene = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/scenes/sandbox/lander_test.usda");
        let stage = compose_file_to_stage(&scene).expect("compose lander_test.usda");
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

    /// A glTF `payload` authored inside a REFERENCED `.usda` wrapper must surface
    /// `lunco:resolvedAsset` on the COMPOSED prim (`/Scene/Bldg/Visual`), not the
    /// wrapper-local path it was written to. This is what lets a scene keep USD as
    /// the source of truth — `scene → .usda → .glb` — and still render the glTF
    /// (and fire the failure placeholder) exactly like a glb referenced directly.
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
        // storage-based compose path, not the deleted native-fs shim.
        let root_id = canonicalize_root("scene.usda");
        let wrapper_id = canonicalize("wrapper.usda", &root_id);
        let bytes = HashMap::from([
            (root_id.clone(), scene.as_bytes().to_vec()),
            (wrapper_id, wrapper.as_bytes().to_vec()),
        ]);
        let stage = build_stage_from_closure(&crate::StageRecipe { root_id, bytes })
            .expect("compose scene→wrapper→glb");
        // A `CanonicalStage` precomputes the binary-arc sites the live
        // `resolved_asset` synth reads (a bare `StageView::new` carries none).
        let cs = CanonicalStage::from_stage(stage, "scene.usda");

        let visual = SdfPath::new("/Scene/Bldg/Visual").unwrap();
        let resolved = cs
            .view()
            .resolved_asset(&visual)
            .expect("resolvedAsset must be synthesized on the composed Visual prim");
        assert!(
            resolved.ends_with("model.glb"),
            "resolvedAsset should point at the wrapper-co-located glb, got {resolved}"
        );
    }
}

