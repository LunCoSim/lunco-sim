//! Compose a USD stage with openusd, then bake the composed result back into a
//! flat [`sdf::Data`] for the downstream visual / physics / cosim readers.
//!
//! Pipeline:
//!  1. **Pre-fetch BFS** — discover every transitively-referenced `.usda` and
//!     fetch its bytes via `LoadContext::read_asset_bytes` (native + wasm, routed
//!     through Bevy's `AssetServer` + our registered sources). openusd's
//!     resolver is synchronous, so all async fetching happens here, up front.
//!     The BFS also records binary-asset arcs (glTF/…) for synthesis below.
//!  2. **Compose** — `Stage::builder().resolver(LuncoUsdResolver).open(root)`
//!     runs the real PCP engine: references, payloads, variant selection, and
//!     relationship-target translation — all filesystem-free.
//!  3. **Flatten** — traverse the composed stage and write each prim's composed
//!     metadata, attribute defaults, and *translated* relationship targets into
//!     a `HashMap<Path, Spec>` → [`TextReader::from_data`]. Downstream keeps
//!     reading the same flat interface it always has.
//!
//! Binary assets (`.glb`/`.gltf`/…) are not USD layers: the resolver routes them
//! to an empty stub during composition, and the BFS records their URI so we can
//! surface a synthesized `lunco:resolvedAsset` attribute here (openusd has no
//! `SdfFileFormat` plugin system). Binary arcs authored in the root layer (our
//! only case — the Perseverance glTF) keep their composed prim path; binary arcs
//! authored *inside* a referenced sub-asset are not remapped and would be
//! skipped (none exist today).

use std::collections::HashMap;

use anyhow::{anyhow, Result};
use bevy::asset::{AssetPath, LoadContext};
use openusd::ar::ResolvedPath;
use openusd::sdf::{self, AbstractData, Path as SdfPath, PathListOp, SpecData, SpecType, Value};
use openusd::usd::{PrimPredicate, Stage};
use openusd::usda;

use crate::resolver::{canonicalize, is_binary_asset, resolve_binary_uri, LuncoUsdResolver};

/// Fetch the full transitive `.usda` closure, compose it, and flatten the
/// composed stage to a `TextReader`. `root_asset_path` is the asset-source
/// relative path of the layer being loaded (e.g.
/// `scenes/sandbox/sandbox_scene.usda`); `root_bytes` are its raw bytes.
pub(crate) async fn compose_to_data(
    load_context: &mut LoadContext<'_>,
    root_asset_path: &str,
    root_bytes: Vec<u8>,
) -> Result<sdf::Data> {
    let root_id = canonicalize(root_asset_path, None);

    // 1. Pre-fetch BFS — keyed by the SAME canonical id the resolver will use.
    let mut bytes: HashMap<String, Vec<u8>> = HashMap::new();
    bytes.insert(root_id.clone(), root_bytes);
    let mut queue = vec![root_id.clone()];

    while let Some(id) = queue.pop() {
        let raw = bytes.get(&id).cloned().expect("queued id is present in map");
        let text = String::from_utf8(raw).map_err(|e| anyhow!("layer {id} is not UTF-8: {e}"))?;
        let data = usda::parse(&text).map_err(|e| anyhow!("USD parse error in {id}: {e}"))?;

        // Only the non-binary `.usda` closure is pre-fetched here; binary-asset
        // arcs (glTF/…) are discovered post-composition by `discover_binary_sites`
        // (driven by `flatten_stage`) so an arc authored inside a referenced
        // `.usda` wrapper anchors on its COMPOSED prim.
        let anchor = ResolvedPath::new(&id);
        for child_asset in discover_arcs(&data) {
            let child_id = canonicalize(&child_asset, Some(&anchor));
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

    // 2. Compose (filesystem-free).
    let resolver = LuncoUsdResolver::new(bytes);
    let stage = Stage::builder()
        .resolver(resolver)
        .open(&root_id)
        .map_err(|e| anyhow!("USD composition error: {e}"))?;

    // 3. Flatten composed stage → TextReader. `flatten_stage` discovers binary
    //    arcs from the composed layer stack and anchors their URIs on the
    //    composed prims via each prim's composition stack.
    flatten_stage(&stage)
}

/// Walk a parsed layer's specs and collect the non-binary `.usda`
/// references/payloads/sublayers to pre-fetch. Binary-asset arcs (glTF/…) are
/// skipped here — they are not USD layers to fetch; their `lunco:resolvedAsset`
/// URI is synthesized post-composition by [`discover_binary_sites`]. Iterating
/// ALL specs (not just the live prim tree) catches references authored inside
/// variant blocks (stored at decorated paths).
fn discover_arcs(data: &sdf::Data) -> Vec<String> {
    let mut fetch = Vec::new();

    if let Some(root) = data.spec(&SdfPath::abs_root()) {
        if let Some(Value::StringVec(subs)) = root.get("subLayers") {
            fetch.extend(subs.iter().filter(|s| !s.is_empty()).cloned());
        }
    }

    for (_path, spec) in data.iter() {
        let mut arcs: Vec<String> = Vec::new();
        if let Some(Value::ReferenceListOp(op)) = spec.get("references") {
            arcs.extend(op.iter().filter(|r| !r.asset_path.is_empty()).map(|r| r.asset_path.clone()));
        }
        match spec.get("payload") {
            Some(Value::Payload(p)) if !p.asset_path.is_empty() => arcs.push(p.asset_path.clone()),
            Some(Value::PayloadListOp(op)) => {
                arcs.extend(op.iter().filter(|p| !p.asset_path.is_empty()).map(|p| p.asset_path.clone()))
            }
            _ => {}
        }
        // Binary arcs (`.glb`/`.gltf`/…) are not fetchable USD layers — the
        // resolver stubs them during composition; skip them from the fetch set.
        fetch.extend(arcs.into_iter().filter(|ap| !is_binary_asset(ap)));
    }

    fetch
}

/// Bake the composed stage into a flat `HashMap<Path, Spec>` → `TextReader`.
/// Binary-asset arcs (glTF/OBJ/STL) authored anywhere in the composed layer
/// stack surface as `lunco:resolvedAsset` attributes, anchored on their COMPOSED
/// prim (see [`discover_binary_sites`]).
fn flatten_stage(stage: &Stage) -> Result<sdf::Data> {
    let mut data: HashMap<SdfPath, SpecData> = HashMap::new();

    // Pseudo-root: carries `defaultPrim` (deferred root-prim resolution) and the
    // top-level prim list.
    let mut root_spec = SpecData::new(SpecType::PseudoRoot);
    if let Some(dp) = stage.default_prim() {
        root_spec.add("defaultPrim", Value::Token(dp));
    }
    if let Ok(tops) = stage.root_prims() {
        root_spec.add("primChildren", Value::TokenVec(tops));
    }
    // Stage `timeCodesPerSecond`: the animation sampler maps sim-seconds onto
    // time codes (`code = seconds * tcps`). openusd's accessor falls back to
    // `framesPerSecond`, then 24 (USD spec), so this is always populated and
    // the runtime reader (`stage_time_codes_per_second`) never has to guess.
    root_spec.add(
        "timeCodesPerSecond",
        Value::Double(stage.time_codes_per_second()),
    );
    data.insert(SdfPath::abs_root(), root_spec);

    // Collect every composed prim path first (traverse takes an FnMut, so we
    // can't `?` inside it). `DEFAULT` already filters inactive/abstract prims,
    // so the flattened reader contains only live geometry.
    let mut paths: Vec<SdfPath> = Vec::new();
    stage
        .traverse(PrimPredicate::DEFAULT, |p| paths.push(p.clone()))
        .map_err(|e| anyhow!("stage traversal failed: {e}"))?;

    // Binary-asset arcs keyed by authoring site `(layer id, spec path)` — the
    // coordinates `Prim::prim_stack` reports. The traversal above forced every
    // reachable reference/payload layer to load, so the whole stack is visible.
    let binary_sites = discover_binary_sites(stage);

    for path in &paths {
        let prim = stage.prim(path.clone());

        let mut spec = SpecData::new(SpecType::Prim);
        if let Some(tn) = prim.type_name().map_err(|e| anyhow!("{path} typeName: {e}"))? {
            spec.add("typeName", Value::Token(tn));
        }
        let apis = prim.api_schemas().unwrap_or_default();
        if !apis.is_empty() {
            spec.add("apiSchemas", Value::TokenVec(apis));
        }
        let children = prim.child_names().unwrap_or_default();
        spec.add("primChildren", Value::TokenVec(children));
        data.insert(path.clone(), spec);

        // Attribute composed values: the default-time opinion (under `default`,
        // where `prim_attribute_value` reads) AND the composed `timeSamples`
        // (under `timeSamples`, where the animation sampler reads).
        for attr in prim.attributes().unwrap_or_default() {
            let mut a = SpecData::new(SpecType::Attribute);
            let mut authored = false;
            if let Some(v) = attr.get::<Value>().map_err(|e| anyhow!("{} default: {e}", attr.path()))? {
                a.add("default", v);
                authored = true;
            }
            // Composed `timeSamples` — the animation read path. PCP has already
            // retimed them through any sublayer / reference `LayerOffset`, so the
            // sampler sees stage-time codes. Without this, animated attributes on
            // *composed* stages (the asset-loader path) silently lost their
            // samples — only single-layer `usda::parse` stages kept them. An
            // attribute may carry samples but no `default`, so this is also what
            // keeps a samples-only attribute from being dropped entirely.
            if let Ok(Some(samples)) = attr.time_samples() {
                if !samples.is_empty() {
                    a.add("timeSamples", Value::TimeSamples(samples));
                    authored = true;
                }
            }
            if authored {
                data.insert(attr.path().clone(), a);
            }
        }

        // Relationship targets — already path-translated through reference +
        // variant by the PCP engine. THIS is the whole point of the migration.
        for rel in prim.relationships().unwrap_or_default() {
            let targets = rel.targets().unwrap_or_default();
            if !targets.is_empty() {
                let mut r = SpecData::new(SpecType::Relationship);
                r.add("targetPaths", Value::PathListOp(PathListOp::explicit(targets)));
                data.insert(rel.path().clone(), r);
            }
        }

        // glTF / binary-asset shim: if any site in THIS prim's composition stack
        // authored a binary arc, surface its URI as `lunco:resolvedAsset` on this
        // composed prim (unless already authored). `prim_stack` reports each
        // contributing `(layer id, spec path)`, so an arc authored in a
        // referenced `.usda` wrapper lands here — at the composed path — not at
        // the wrapper-local path it was written to. The Bevy loader keys the
        // glTF `SceneRoot` + failure placeholder off this attribute.
        if !binary_sites.is_empty() {
            if let Ok(stack) = prim.prim_stack() {
                if let Some(uri) = stack.iter().find_map(|site| binary_sites.get(site)) {
                    if let Ok(attr_path) = path.append_property("lunco:resolvedAsset") {
                        data.entry(attr_path).or_insert_with(|| {
                            let mut a = SpecData::new(SpecType::Attribute);
                            a.add("default", Value::AssetPath(uri.clone().into()));
                            a
                        });
                    }
                }
            }
        }
    }

    Ok(sdf::Data::from_specs(data))
}

/// Discover every binary-asset arc (glTF/OBJ/STL, per [`is_binary_asset`])
/// authored across the composed stage's loaded layers, keyed by its authoring
/// site `(layer identifier, spec path)` — the coordinates
/// [`openusd::usd::Prim::prim_stack`] reports. [`flatten_stage`] matches these
/// against each composed prim's stack to anchor `lunco:resolvedAsset` on the
/// COMPOSED prim, so a glTF `payload`/`reference` authored inside a referenced
/// `.usda` wrapper surfaces on the composed prim — not only arcs authored
/// directly in the root layer, as the previous layer-local match required.
fn discover_binary_sites(stage: &Stage) -> HashMap<(String, SdfPath), String> {
    let mut sites: HashMap<(String, SdfPath), String> = HashMap::new();
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
                    sites.insert((layer_id.clone(), path.clone()), resolve_binary_uri(&ap, Some(&anchor)));
                }
            }
        }
    }
    sites
}

/// Synchronous, **native-filesystem** compose for the USD doc viewport preview:
/// the root layer is an in-memory `source` string; referenced sublayers are read
/// from disk relative to `base_dir`. Returns the flattened composed stage, or
/// `None` if composition fails (the caller falls back to the raw root layer).
///
/// Native-only — on wasm there is no synchronous filesystem, and the viewport
/// preview historically fell back to the raw layer there. The async,
/// `AssetServer`-driven path ([`compose_to_textreader`]) is what the actual
/// asset loader uses on both platforms.
#[cfg(not(target_arch = "wasm32"))]
pub fn compose_native_fs(source: &str, base_dir: &std::path::Path) -> Option<sdf::Data> {
    use crate::resolver::normalize;

    // Synthetic absolute id for the in-memory root, placed under `base_dir` so
    // relative references anchor correctly.
    let root_id = normalize(&base_dir.join("__lunco_inmemory_root__.usda"))
        .to_string_lossy()
        .into_owned();
    let resolver = FsResolver {
        root_id: root_id.clone(),
        root_bytes: source.as_bytes().to_vec(),
    };
    let stage = Stage::builder().resolver(resolver).open(&root_id).ok()?;

    // `flatten_stage` discovers binary-asset arcs (glTF payloads/references)
    // across the composed layer stack and anchors each on its composed prim.
    flatten_stage(&stage).ok()
}

#[cfg(target_arch = "wasm32")]
pub fn compose_native_fs(_source: &str, _base_dir: &std::path::Path) -> Option<sdf::Data> {
    None
}

/// Compose a USD layer **from disk** through the real openusd PCP engine
/// ([`Stage::open`], backed by [`openusd::ar::DefaultResolver`]) and flatten the
/// composed result to [`sdf::Data`]. Native + synchronous: for tests and tools
/// that load a real on-disk `.usda` with every reference resolved — distinct
/// from the async `AssetServer`-driven loader ([`compose_to_data`]) and the
/// in-memory-root viewport shim ([`compose_native_fs`]). `DefaultResolver`
/// anchors each relative reference to its own layer's directory, so the on-disk
/// reference tree resolves exactly as authored.
#[cfg(not(target_arch = "wasm32"))]
pub fn compose_file(path: &std::path::Path) -> Result<sdf::Data> {
    let id = path
        .to_str()
        .ok_or_else(|| anyhow!("non-UTF8 USD path: {path:?}"))?;
    let stage = Stage::open(id).map_err(|e| anyhow!("USD composition error for {id}: {e}"))?;
    // `flatten_stage` surfaces binary-asset arcs (glTF/…) as `lunco:resolvedAsset`
    // across the whole composed layer stack, anchored on each composed prim.
    flatten_stage(&stage)
}

/// Resolver for the native-fs viewport path: the root layer is held in memory;
/// every other layer is read from disk. Binary assets route to the empty stub.
#[cfg(not(target_arch = "wasm32"))]
struct FsResolver {
    root_id: String,
    root_bytes: Vec<u8>,
}

#[cfg(not(target_arch = "wasm32"))]
impl openusd::ar::Resolver for FsResolver {
    fn create_identifier(&self, asset_path: &str, anchor: Option<&ResolvedPath>) -> String {
        use crate::resolver::{is_binary_asset, normalize, BINARY_STUB_ID};
        if is_binary_asset(asset_path) {
            return BINARY_STUB_ID.to_string();
        }
        if asset_path.contains("://") {
            return asset_path.to_string();
        }
        let p = std::path::Path::new(asset_path);
        let joined = if p.is_absolute() {
            p.to_path_buf()
        } else {
            anchor
                .and_then(|a| a.parent())
                .map(|d| d.join(p))
                .unwrap_or_else(|| p.to_path_buf())
        };
        normalize(&joined).to_string_lossy().into_owned()
    }

    fn resolve(&self, asset_path: &str) -> Option<ResolvedPath> {
        use crate::resolver::BINARY_STUB_ID;
        if asset_path == self.root_id || asset_path == BINARY_STUB_ID || std::path::Path::new(asset_path).exists() {
            Some(ResolvedPath::new(asset_path))
        } else {
            None
        }
    }

    fn resolve_for_new_asset(&self, asset_path: &str) -> Option<ResolvedPath> {
        Some(ResolvedPath::new(asset_path))
    }

    fn open_asset(&self, resolved_path: &ResolvedPath) -> std::io::Result<Box<dyn openusd::ar::Asset>> {
        use crate::resolver::BINARY_STUB_ID;
        let key = resolved_path.to_str().unwrap_or_default();
        if key == self.root_id {
            return Ok(Box::new(std::io::Cursor::new(self.root_bytes.clone())));
        }
        if key == BINARY_STUB_ID {
            return Ok(Box::new(std::io::Cursor::new(b"#usda 1.0\n".to_vec())));
        }
        let bytes = std::fs::read(key)?;
        Ok(Box::new(std::io::Cursor::new(bytes)))
    }

    fn get_modification_timestamp(&self, _asset_path: &str, _resolved_path: &ResolvedPath) -> Option<std::time::SystemTime> {
        None
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod inherits_compose_tests {
    use super::*;
    use crate::usd_data::UsdDataExt;

    /// De-risk the control-profile design: a `class` carrying a `Controls` child
    /// scope, `inherits`-ed by a vessel prim, must land those child prims (with
    /// their attrs) under the vessel after full PCP flatten — so the entity
    /// translator can walk `<Vessel>/Controls/<intent>` to build a `ControlBinding`.
    #[test]
    fn inherits_from_class_brings_child_prims_into_flattened_data() {
        let usda = "#usda 1.0\n\
class \"_RoverControl\"\n{\n    def \"Controls\"\n    {\n        def \"forward\"\n        {\n            uniform string lunco:port = \"throttle\"\n            uniform double lunco:scale = 1\n        }\n    }\n}\n\
def Xform \"Rover\" (\n    inherits = </_RoverControl>\n)\n{\n}\n";
        let data = compose_native_fs(usda, std::path::Path::new("/tmp")).expect("compose+flatten");
        let fwd = SdfPath::new("/Rover/Controls/forward").unwrap();
        assert_eq!(
            data.prim_attribute_value::<String>(&fwd, "lunco:port").as_deref(),
            Some("throttle"),
            "inherited Controls child must appear under /Rover with its attrs"
        );
        assert_eq!(data.prim_attribute_value::<f64>(&fwd, "lunco:scale"), Some(1.0));
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
        let data = compose_file(&rover).expect("compose_file");
        let fwd = SdfPath::new("/SkidRover/Controls/forward").unwrap();
        assert_eq!(
            data.prim_attribute_value::<String>(&fwd, "lunco:port").as_deref(),
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
        let data = compose_file(&asset).expect("compose skid_rover.usda");
        let fwd = SdfPath::new("/SkidRover/Controls/forward").unwrap();
        assert_eq!(
            data.prim_attribute_value::<String>(&fwd, "lunco:port").as_deref(),
            Some("throttle"),
            "skid_rover must inherit the rover control profile's Controls scope"
        );
        assert_eq!(data.prim_attribute_value::<f64>(&fwd, "lunco:scale"), Some(1.0));
    }

    /// The two harder composition paths, on the real `lander_test.usda`:
    /// (a) an INLINE lander prim inheriting `_LanderControl` via the scene's own
    ///     `subLayers`; (b) a rover pulled in by `references` whose OWN
    ///     `subLayers`+`inherits` must still compose THROUGH the reference arc.
    #[test]
    fn lander_scene_composes_inline_and_referenced_control_profiles() {
        let scene = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/scenes/sandbox/lander_test.usda");
        let data = compose_file(&scene).expect("compose lander_test.usda");

        // (a) inline lander inherits _LanderControl through the scene subLayer.
        let lander_fwd = SdfPath::new("/LanderTest/Lander/Controls/forward").unwrap();
        assert_eq!(
            data.prim_attribute_value::<String>(&lander_fwd, "lunco:port").as_deref(),
            Some("pitch"),
            "inline lander must inherit the lander control profile"
        );
        // (b) referenced rover's subLayer+inherits composes through the ref arc.
        let rover_fwd = SdfPath::new("/LanderTest/SkidRover/Controls/forward").unwrap();
        assert_eq!(
            data.prim_attribute_value::<String>(&rover_fwd, "lunco:port").as_deref(),
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
        let dir = std::env::temp_dir().join("lunco_glb_wrapper_compose_test");
        std::fs::create_dir_all(&dir).unwrap();
        // Wrapper: a `Structure` defaultPrim whose `Visual` child carries the glb
        // payload — the Perseverance "usda → glb" shape.
        std::fs::write(
            dir.join("wrapper.usda"),
            "#usda 1.0\n(\n    defaultPrim = \"Structure\"\n)\ndef Xform \"Structure\"\n{\n    def Xform \"Visual\" (\n        prepend payload = @model.glb@\n    )\n    {\n        string lunco:assetMode = \"scene\"\n    }\n}\n",
        )
        .unwrap();
        // Scene references the wrapper — no direct glb embedding in the scene.
        let scene = "#usda 1.0\ndef Xform \"Scene\"\n{\n    def Xform \"Bldg\" (\n        prepend references = @wrapper.usda@\n    )\n    {\n    }\n}\n";
        let data = compose_native_fs(scene, &dir).expect("compose scene→wrapper→glb");

        let visual = SdfPath::new("/Scene/Bldg/Visual").unwrap();
        let attr = visual.append_property("lunco:resolvedAsset").unwrap();
        let spec = data
            .spec(&attr)
            .expect("resolvedAsset must be synthesized on the composed Visual prim");
        match spec.get("default") {
            Some(Value::AssetPath(a)) => assert!(
                a.as_str().ends_with("model.glb"),
                "resolvedAsset should point at the wrapper-co-located glb, got {}",
                a.as_str()
            ),
            other => panic!("resolvedAsset should be an AssetPath, got {other:?}"),
        }
    }
}
