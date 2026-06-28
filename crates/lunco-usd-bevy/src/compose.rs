//! Compose a USD stage with openusd 0.5.0, then bake the composed result back
//! into a flat [`TextReader`] for the downstream visual / physics / cosim
//! readers.
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
use openusd::sdf::{self, Path as SdfPath, PathListOp, SpecData, SpecType, Value};
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
    // (composed-prim-path, load-uri) for every binary asset arc discovered.
    let mut binary_assets: Vec<(SdfPath, String)> = Vec::new();

    while let Some(id) = queue.pop() {
        let raw = bytes.get(&id).cloned().expect("queued id is present in map");
        let text = String::from_utf8(raw).map_err(|e| anyhow!("layer {id} is not UTF-8: {e}"))?;
        let data = usda::parse(&text).map_err(|e| anyhow!("USD parse error in {id}: {e}"))?;

        let anchor = ResolvedPath::new(&id);
        let (refs, binaries) = discover_arcs(&data, &anchor);
        binary_assets.extend(binaries);
        for child_asset in refs {
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

    // 3. Flatten composed stage → TextReader, injecting binary-asset URIs.
    flatten_stage(&stage, &binary_assets)
}

/// Walk a parsed layer's specs and classify every external arc: non-binary
/// `.usda` references/payloads/sublayers to fetch, and binary-asset arcs
/// (glTF/…) recorded as `(prim_path, load_uri)` for `lunco:resolvedAsset`
/// synthesis. Iterating ALL specs (not just the live prim tree) catches
/// references authored inside variant blocks (stored at decorated paths).
fn discover_arcs(data: &sdf::Data, anchor: &ResolvedPath) -> (Vec<String>, Vec<(SdfPath, String)>) {
    let mut fetch = Vec::new();
    let mut binary = Vec::new();

    if let Some(root) = data.spec(&SdfPath::abs_root()) {
        if let Some(Value::StringVec(subs)) = root.get("subLayers") {
            fetch.extend(subs.iter().filter(|s| !s.is_empty()).cloned());
        }
    }

    for (path, spec) in data.iter() {
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
        for ap in arcs {
            if is_binary_asset(&ap) {
                binary.push((path.clone(), resolve_binary_uri(&ap, Some(anchor))));
            } else {
                fetch.push(ap);
            }
        }
    }

    (fetch, binary)
}

/// Bake the composed stage into a flat `HashMap<Path, Spec>` → `TextReader`.
/// `binary_assets` are `(prim_path, uri)` pairs to surface as
/// `lunco:resolvedAsset` attributes.
fn flatten_stage(stage: &Stage, binary_assets: &[(SdfPath, String)]) -> Result<sdf::Data> {
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
    data.insert(SdfPath::abs_root(), root_spec);

    // Collect every composed prim path first (traverse takes an FnMut, so we
    // can't `?` inside it). `DEFAULT` already filters inactive/abstract prims,
    // so the flattened reader contains only live geometry.
    let mut paths: Vec<SdfPath> = Vec::new();
    stage
        .traverse(PrimPredicate::DEFAULT, |p| paths.push(p.clone()))
        .map_err(|e| anyhow!("stage traversal failed: {e}"))?;

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

        // Attribute composed defaults (under the `default` field, where
        // `prim_attribute_value` reads them).
        for attr in prim.attributes().unwrap_or_default() {
            if let Some(v) = attr.get::<Value>().map_err(|e| anyhow!("{} default: {e}", attr.path()))? {
                let mut a = SpecData::new(SpecType::Attribute);
                a.add("default", v);
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
    }

    // glTF / binary-asset shim: surface each authored binary arc's URI as a
    // `lunco:resolvedAsset` attribute on its (composed) prim, unless one was
    // already authored. Skip arcs whose prim didn't survive composition.
    for (path, uri) in binary_assets {
        if !data.contains_key(path) {
            continue;
        }
        if let Ok(attr_path) = path.append_property("lunco:resolvedAsset") {
            data.entry(attr_path).or_insert_with(|| {
                let mut a = SpecData::new(SpecType::Attribute);
                a.add("default", Value::AssetPath(uri.clone().into()));
                a
            });
        }
    }

    Ok(sdf::Data::from_specs(data))
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

    // Discover binary-asset arcs (glTF payloads/references) authored in the root
    // layer so their `lunco:resolvedAsset` URI is synthesized. Anchor is unused
    // for the `scheme://` URIs this surfaces.
    let binary = usda::parse(source)
        .map(|data| discover_arcs(&data, &ResolvedPath::new("")).1)
        .unwrap_or_default();

    flatten_stage(&stage, &binary).ok()
}

#[cfg(target_arch = "wasm32")]
pub fn compose_native_fs(_source: &str, _base_dir: &std::path::Path) -> Option<sdf::Data> {
    None
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
