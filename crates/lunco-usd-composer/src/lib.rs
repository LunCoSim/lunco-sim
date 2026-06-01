use openusd::usda::TextReader;
use openusd::sdf::{self, SpecType, Value};
use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use anyhow::{bail, Result};

/// Resolve `..` / `.` segments in a path *without* touching the
/// filesystem. Needed for wasm where canonicalisation has no fs to
/// consult, and useful on native to keep `processed` cache keys
/// deduplicated across reference chains.
pub fn normalize_path(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::ParentDir => { out.pop(); }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// File extensions the composer recognises as **non-USD binary assets**
/// referenced through `payload`/`references`.
///
/// Pixar's USD distribution handles these via the `UsdGltf` /
/// `UsdObj` SdfFileFormat plugins — when a reference points at a
/// `.glb`, the plugin transparently parses the glTF and exposes it as
/// a USD prim subtree. We don't have a plugin system in
/// `openusd-rs 0.2`, so the composer instead **detects** the
/// extension, skips the "read as USD text" path, and synthesises a
/// `lunco:resolvedAsset` attribute on the referencing prim. The Bevy
/// side (`sync_usd_visuals`) reads that attribute and loads the
/// binary through `AssetServer` — Mesh3d for `mesh` mode, SceneRoot
/// for `scene` mode.
///
/// Order doesn't matter; matched case-insensitively.
const BINARY_ASSET_EXTENSIONS: &[&str] = &["glb", "gltf", "obj", "stl"];

/// Returns `true` if `asset_path` ends in one of the
/// [`BINARY_ASSET_EXTENSIONS`]. Strips off URL query strings (`?…`)
/// and fragments (`#…`) before testing — the NASA Perseverance URL
/// for example carries an `?emrc=…` query that would otherwise
/// shadow the `.glb` extension.
fn is_binary_asset(asset_path: &str) -> bool {
    let stem = asset_path
        .split('?').next().unwrap_or(asset_path)
        .split('#').next().unwrap_or(asset_path);
    if let Some(dot) = stem.rfind('.') {
        let ext = &stem[dot + 1..];
        BINARY_ASSET_EXTENSIONS.iter().any(|known| known.eq_ignore_ascii_case(ext))
    } else {
        false
    }
}

/// Resolves an asset_path string per LunCoSim USD conventions.
///
/// - **URI scheme** (`lunco-lib://...`, `http://...`): pass through
///   verbatim — the resolver registered with Bevy's `AssetServer`
///   handles it. This is the shape recommended for shipped fixtures.
/// - **`/`-prefixed**: absolute from the workspace `assets/` root
///   (matches existing `lunco-usd-composer` reference resolution).
///   Returned as a leading-slash string so `Bevy`'s default `assets://`
///   source resolves it.
/// - **plain relative**: relative to the layer's parent directory,
///   joined and converted to a string.
fn resolve_asset_uri(asset_path: &str, asset_root: &Path, current_dir: &Path) -> String {
    if asset_path.contains("://") {
        return asset_path.to_string();
    }
    if let Some(rest) = asset_path.strip_prefix('/') {
        // Absolute-from-asset-root: stay as a workspace-relative path
        // string. The Bevy default source roots at `assets/`, so a
        // leading-slash form matches existing USD references.
        return asset_root.join(rest).to_string_lossy().to_string();
    }
    current_dir.join(asset_path).to_string_lossy().to_string()
}

/// Find the assets root by walking up from the starting directory
/// until we find a directory containing an "assets" subdirectory.
fn find_assets_root(start: &Path) -> PathBuf {
    // Make absolute so ancestor walking works correctly
    let abs_start = if start.is_absolute() {
        start.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(start))
            .unwrap_or_else(|_| start.to_path_buf())
    };
    // Walk up from the starting directory to find the assets root
    for ancestor in abs_start.ancestors() {
        let assets = ancestor.join("assets");
        if assets.exists() && assets.is_dir() {
            return assets;
        }
    }
    // Fallback: use the original start
    start.to_path_buf()
}

/// The `UsdComposer` is responsible for high-level USD operations like
/// composition, reference resolution, and stage flattening.
///
/// This sits above the Sdf-layer (parsing) and implements Pcp-like
/// (Prim Composition Propagation) logic.
pub struct UsdComposer;

/// Sublayer reader strategy. The default reads from the local
/// filesystem via `TextReader::read`; the wasm asset loader injects a
/// pre-fetched-bytes map so HTTP-fetched layers feed back in
/// synchronously.
pub type SublayerFetcher<'a> = &'a mut dyn FnMut(&Path) -> Result<TextReader>;

/// Walk a parsed USDA layer's references/payloads and return every
/// resolved sublayer path (USD-text only, binary assets skipped). Used
/// by the wasm asset loader to discover the transitive sublayer set so
/// it can async-fetch them all via the bevy `AssetServer` before
/// invoking the (sync) composer.
pub fn collect_sublayer_paths(
    reader: &TextReader,
    base_dir: &Path,
) -> Vec<PathBuf> {
    let asset_root = find_assets_root(base_dir);
    let mut out = Vec::new();
    for (_path, spec) in reader.iter() {
        let mut asset_paths: Vec<String> = Vec::new();
        if let Some(Value::ReferenceListOp(list_op)) = spec.fields.get(sdf::schema::FieldKey::References.as_str()) {
            for r in list_op.explicit_items.iter()
                .chain(list_op.added_items.iter())
                .chain(list_op.prepended_items.iter())
                .chain(list_op.appended_items.iter())
            {
                if !r.asset_path.is_empty() { asset_paths.push(r.asset_path.clone()); }
            }
        }
        if let Some(Value::PayloadListOp(list_op)) = spec.fields.get(sdf::schema::FieldKey::Payload.as_str()) {
            for p in list_op.explicit_items.iter()
                .chain(list_op.added_items.iter())
                .chain(list_op.prepended_items.iter())
                .chain(list_op.appended_items.iter())
            {
                if !p.asset_path.is_empty() { asset_paths.push(p.asset_path.clone()); }
            }
        }
        for asset_path in asset_paths {
            if is_binary_asset(&asset_path) || asset_path.contains("://") { continue; }
            let resolved = if let Some(rest) = asset_path.strip_prefix('/') {
                asset_root.join(rest)
            } else {
                base_dir.join(&asset_path)
            };
            out.push(normalize_path(&resolved));
        }
    }
    out
}

impl UsdComposer {
    /// Recursively resolves all references in the given reader and merges them
    /// into a single flattened layer. Uses the local filesystem for sublayer reads.
    pub fn flatten(reader: &TextReader, base_dir: &Path) -> Result<TextReader> {
        let mut fetcher = |p: &Path| TextReader::read(p);
        Self::flatten_with_fetcher(reader, base_dir, &mut fetcher)
    }

    /// Like [`Self::flatten`], but reads referenced sublayers through a
    /// caller-provided closure. Use this in environments without a
    /// blocking filesystem (e.g. wasm), where the loader pre-fetches
    /// every transitively-referenced `.usda` via the host's async asset
    /// pipeline and the closure just hands them back from an
    /// in-memory map.
    pub fn flatten_with_fetcher(
        reader: &TextReader,
        base_dir: &Path,
        fetcher: SublayerFetcher<'_>,
    ) -> Result<TextReader> {
        let mut data_map: HashMap<sdf::Path, sdf::Spec> = reader.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        // Per-file composed-data cache (compose once, reuse for every referencer)
        // + the active-chain set used only to break reference cycles.
        let mut cache: HashMap<PathBuf, HashMap<sdf::Path, sdf::Spec>> = HashMap::new();
        let mut in_progress: HashSet<PathBuf> = HashSet::new();
        let usd_root = find_assets_root(base_dir);
        Self::flatten_recursive(&mut data_map, &usd_root, base_dir, &mut cache, &mut in_progress, fetcher)?;
        Ok(TextReader::from_data(data_map))
    }

    fn flatten_recursive(
        data_map: &mut HashMap<sdf::Path, sdf::Spec>,
        asset_root: &Path,
        current_dir: &Path,
        cache: &mut HashMap<PathBuf, HashMap<sdf::Path, sdf::Spec>>,
        in_progress: &mut HashSet<PathBuf>,
        fetcher: SublayerFetcher<'_>
    ) -> Result<()> {
        // Collect all prim paths and prepare merges
        let prim_paths: Vec<sdf::Path> = data_map.keys().cloned().collect();
        let mut pending_merges: Vec<(sdf::Path, sdf::Path, HashMap<sdf::Path, sdf::Spec>)> = Vec::new();
        // Binary-asset payloads/references that bypass USD-text composition
        // and instead surface as `lunco:resolvedAsset` attributes for the
        // Bevy side to load through `AssetServer`. Pixar's USD does this
        // via SdfFileFormat plugins (`UsdGltf` etc.); we approximate.
        let mut binary_assets: Vec<(sdf::Path, String)> = Vec::new();

        for path in prim_paths {
            let spec = data_map.get(&path);
            let Some(spec) = spec else { continue; };

            // Collect every external-asset string from both the
            // `references` and `payload` fields. References compose
            // eagerly; payloads compose lazily in real USD, but for
            // our purposes (no payload mask, single load step) the
            // distinction collapses — both feed the same merge path.
            let mut asset_paths: Vec<(String, sdf::Path)> = Vec::new();
            if let Some(Value::ReferenceListOp(list_op)) = spec.fields.get(sdf::schema::FieldKey::References.as_str()) {
                let mut refs = list_op.explicit_items.clone();
                refs.extend(list_op.added_items.clone());
                refs.extend(list_op.prepended_items.clone());
                refs.extend(list_op.appended_items.clone());
                for r in refs {
                    asset_paths.push((r.asset_path, r.prim_path));
                }
            }
            if let Some(Value::PayloadListOp(list_op)) = spec.fields.get(sdf::schema::FieldKey::Payload.as_str()) {
                let mut pls = list_op.explicit_items.clone();
                pls.extend(list_op.added_items.clone());
                pls.extend(list_op.prepended_items.clone());
                pls.extend(list_op.appended_items.clone());
                for p in pls {
                    asset_paths.push((p.asset_path, p.prim_path));
                }
            }

            for (asset_path, ref_prim_path) in asset_paths {
                // Binary asset path: skip the USD-text read entirely
                // and stash the resolved URI on the prim. The Bevy
                // side reads it from `lunco:resolvedAsset` and loads
                // the file via `AssetServer`.
                if !asset_path.is_empty() && is_binary_asset(&asset_path) {
                    let resolved = resolve_asset_uri(&asset_path, asset_root, current_dir);
                    binary_assets.push((path.clone(), resolved));
                    continue;
                }

                let (ref_data, source_root) = if asset_path.is_empty() {
                    // INTERNAL REFERENCE
                    if ref_prim_path.is_empty() { continue; }
                    (data_map.clone(), ref_prim_path.clone())
                } else {
                    // EXTERNAL USD REFERENCE
                    // "/"-prefixed paths are absolute from USD assets root
                    let ref_path = if asset_path.starts_with('/') {
                        let stripped = asset_path.strip_prefix('/').unwrap();
                        normalize_path(&asset_root.join(stripped))
                    } else {
                        normalize_path(&current_dir.join(&asset_path))
                    };

                    let ref_current_dir = ref_path
                        .parent()
                        .unwrap_or_else(|| Path::new("."))
                        .to_path_buf();

                    // Fully-composed sub-data for the referenced file. **Memoised
                    // per file**: a file referenced by many prims (e.g.
                    // `wheel.usda` under every rover, or a rover referenced as
                    // several instances) is composed ONCE and the composed result
                    // reused for every referencer. The previous global "processed"
                    // set instead composed the file for the *first* referencer and
                    // handed every later referencer the *raw* sub-data — dropping
                    // that file's own nested references (so only one rover's wheels
                    // received `wheel.usda`'s primvars, nondeterministically by
                    // HashMap order). `in_progress` breaks reference *cycles* within
                    // the current chain without suppressing legitimate reuse.
                    let sub_data: HashMap<sdf::Path, sdf::Spec> =
                        if let Some(cached) = cache.get(&ref_path) {
                            cached.clone()
                        } else {
                            let sub_reader = fetcher(&ref_path)?;
                            let mut sub_data: HashMap<sdf::Path, sdf::Spec> =
                                sub_reader.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                            if in_progress.insert(ref_path.clone()) {
                                Self::flatten_recursive(
                                    &mut sub_data, asset_root, &ref_current_dir,
                                    cache, in_progress, fetcher,
                                )?;
                                in_progress.remove(&ref_path);
                                cache.insert(ref_path.clone(), sub_data.clone());
                            }
                            // else: cyclic reference up the current chain — use the
                            // raw sub-data (one level, uncached) to break the loop.
                            sub_data
                        };

                    let root = if ref_prim_path.is_empty() {
                        Self::get_default_prim_from_data(&sub_data).ok_or_else(|| {
                            anyhow::anyhow!("No defaultPrim in referenced file {}", asset_path)
                        })?
                    } else {
                        ref_prim_path.clone()
                    };
                    (sub_data, root)
                };

                pending_merges.push((path.clone(), source_root, ref_data));
            }
        }

        // Materialise binary-asset URIs as `lunco:resolvedAsset`
        // attribute specs on the referencing prim. We don't add the
        // attribute to `propertyChildren` — `prim_attribute_value`
        // resolves attributes by direct path lookup, not by walking
        // children, so it'd be ceremony with no consumer. Keeps the
        // synthesis minimally invasive.
        for (prim_path, resolved) in binary_assets {
            let attr_path = match prim_path.append_property("lunco:resolvedAsset") {
                Ok(p) => p,
                Err(_) => continue,
            };
            // Only synthesise if not already authored — a hand-written
            // `lunco:resolvedAsset` overrides composer detection.
            if data_map.contains_key(&attr_path) { continue; }
            let mut attr_spec = sdf::Spec::new(SpecType::Attribute);
            attr_spec.fields.insert(
                sdf::schema::FieldKey::Default.as_str().to_string(),
                Value::AssetPath(resolved),
            );
            data_map.insert(attr_path, attr_spec);
        }

        // Apply merges: Weak-merge strategy (Local opinions win)
        for (target_root, source_root, ref_data) in pending_merges {
            let child_key = sdf::schema::ChildrenKey::PrimChildren.as_str();

            // 1. Merge the referenced prim's attributes into the target
            if let Some(source_root_spec) = ref_data.get(&source_root) {
                let target_spec = data_map.get_mut(&target_root);
                if let Some(target_spec) = target_spec {
                    for (field_name, field_value) in &source_root_spec.fields {
                        if field_name == child_key {
                            if let Value::TokenVec(source_children) = field_value {
                                let mut children = if let Some(Value::TokenVec(existing)) = target_spec.fields.get(child_key) {
                                    existing.clone()
                                } else {
                                    Vec::new()
                                };
                                for child in source_children {
                                    if !children.contains(child) {
                                        children.push(child.clone());
                                    }
                                }
                                target_spec.fields.insert(child_key.to_string(), Value::TokenVec(children));
                            }
                            continue;
                        }
                        // Weak merge: Local opinions win
                        target_spec.fields.entry(field_name.to_string()).or_insert_with(|| field_value.clone());
                    }
                }
            }

            // 2. Copy over all remapped descendants
            for (source_path, source_spec) in ref_data {
                if source_path == source_root { continue; }

                if let Ok(remapped_path) = Self::remap_path(&source_root, &target_root, &source_path) {
                    let target_spec = data_map.entry(remapped_path).or_insert_with(|| sdf::Spec::new(source_spec.ty));
                    for (field_name, field_value) in source_spec.fields {
                        target_spec.fields.entry(field_name).or_insert(field_value);
                    }
                }
            }
        }

        Ok(())
    }

    /// Gets the defaultPrim from the reader's root spec.
    pub fn get_default_prim(reader: &TextReader) -> Option<sdf::Path> {
        Self::get_default_prim_from_data(&reader.iter().map(|(k, v)| (k.clone(), v.clone())).collect::<HashMap<_, _>>())
    }

    fn get_default_prim_from_data(data: &HashMap<sdf::Path, sdf::Spec>) -> Option<sdf::Path> {
        if let Some(root_spec) = data.get(&sdf::Path::abs_root()) {
            if let Some(Value::Token(name)) = root_spec.fields.get(sdf::schema::FieldKey::DefaultPrim.as_str()) {
                return sdf::Path::new(name).ok();
            }
        }
        None
    }

    /// Remaps a path from a referenced layer's namespace to the current stage's namespace.
    fn remap_path(source_root: &sdf::Path, target_root: &sdf::Path, source_path: &sdf::Path) -> Result<sdf::Path> {
        let source_str = source_path.as_str();
        let root_str = source_root.as_str();

        if source_str == root_str {
            return Ok(target_root.clone());
        }

        if source_str.starts_with(root_str) {
            let mut relative = &source_str[root_str.len()..];
            let target_str = target_root.as_str();

            let new_path_str = if relative.starts_with('.') {
                format!("{}{}", target_str, relative)
            } else {
                if relative.starts_with('/') {
                    relative = &relative[1..];
                }
                if target_str == "/" {
                    format!("/{}", relative)
                } else {
                    format!("{}/{}", target_str, relative)
                }
            };
            sdf::Path::new(&new_path_str)
        } else {
            bail!("Path {} not under root {}", source_str, root_str)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binary_asset_detection() {
        assert!(is_binary_asset("body.glb"));
        assert!(is_binary_asset("BODY.GLB"));
        assert!(is_binary_asset("./models/body.gltf"));
        assert!(is_binary_asset("lunco-lib://models/perseverance.glb"));
        assert!(is_binary_asset("/models/x.obj"));
        // Real-world URL with a query string — must look at the
        // path component, not the whole string.
        assert!(is_binary_asset(
            "https://nasa.example/m2020.glb?emrc=69f2a834c1972"
        ));
        // USD references should NOT match.
        assert!(!is_binary_asset("/vessels/rovers/skid_rover.usda"));
        assert!(!is_binary_asset("body.usd"));
        assert!(!is_binary_asset(""));
    }

    /// Regression: a file referenced by multiple prims (directly, and through
    /// a chain) must be fully composed for EVERY referencer — not composed for
    /// the first and handed raw to the rest. This reproduces the bug where only
    /// one rover instance's wheels received `wheel.usda`'s primvars (the rest
    /// read `materialType = None` and rendered plain), nondeterministically by
    /// HashMap iteration order.
    #[test]
    fn nested_reference_composes_for_every_instance() {
        use std::collections::HashMap;

        fn parse(text: &str) -> TextReader {
            let mut parser = openusd::usda::parser::Parser::new(text);
            TextReader::from_data(parser.parse().expect("parse"))
        }

        // Leaf component carrying a distinctive primvar.
        let wheel = "#usda 1.0\n\
            def Cylinder \"Wheel\"\n{\n    string primvars:materialType = \"shader\"\n}\n";
        // References the leaf TWICE (two wheels).
        let rover = "#usda 1.0\n(\n    defaultPrim = \"Rover\"\n)\n\
            def Xform \"Rover\"\n{\n\
            def Cylinder \"Wheel_FL\" (prepend references = @wheel.usda@</Wheel>)\n{\n}\n\
            def Cylinder \"Wheel_FR\" (prepend references = @wheel.usda@</Wheel>)\n{\n}\n}\n";
        // References the rover TWICE (two instances) → nested, multi-instance.
        let scene = "#usda 1.0\n\
            def Xform \"SceneRoot\"\n{\n\
            def Xform \"Rover_A\" (prepend references = @rover.usda@</Rover>)\n{\n}\n\
            def Xform \"Rover_B\" (prepend references = @rover.usda@</Rover>)\n{\n}\n}\n";

        let base = PathBuf::from("/assets/scenes");
        let mut files: HashMap<PathBuf, TextReader> = HashMap::new();
        files.insert(normalize_path(&base.join("wheel.usda")), parse(wheel));
        files.insert(normalize_path(&base.join("rover.usda")), parse(rover));

        let mut fetcher = |p: &Path| -> Result<TextReader> {
            files
                .get(&normalize_path(p))
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("missing sublayer {:?}", p))
        };

        let scene_reader = parse(scene);
        let composed =
            UsdComposer::flatten_with_fetcher(&scene_reader, &base, &mut fetcher).expect("flatten");

        // Every leaf wheel — both wheels of BOTH rovers — must have composed the
        // primvar. Pre-fix, only one rover's wheels did.
        for path in [
            "/SceneRoot/Rover_A/Wheel_FL",
            "/SceneRoot/Rover_A/Wheel_FR",
            "/SceneRoot/Rover_B/Wheel_FL",
            "/SceneRoot/Rover_B/Wheel_FR",
        ] {
            let sdf_path = sdf::Path::new(path).expect("sdf path");
            let mat: Option<String> =
                composed.prim_attribute_value(&sdf_path, "primvars:materialType");
            assert_eq!(
                mat.as_deref(),
                Some("shader"),
                "wheel {path} did not receive wheel.usda's composed primvar"
            );
        }
    }

    #[test]
    fn resolve_uri_passes_through_schemes() {
        let asset_root = Path::new("/ws/assets");
        let layer_dir = Path::new("/ws/assets/scenes");
        assert_eq!(
            resolve_asset_uri("lunco-lib://models/x.glb", asset_root, layer_dir),
            "lunco-lib://models/x.glb"
        );
        assert_eq!(
            resolve_asset_uri("https://nasa/x.glb", asset_root, layer_dir),
            "https://nasa/x.glb"
        );
    }

    #[test]
    fn resolve_uri_handles_relative_and_absolute() {
        let asset_root = Path::new("/ws/assets");
        let layer_dir = Path::new("/ws/assets/scenes");
        // /-prefixed → resolved against the asset root.
        assert_eq!(
            resolve_asset_uri("/models/x.glb", asset_root, layer_dir),
            "/ws/assets/models/x.glb"
        );
        // Plain relative → resolved against the layer's directory.
        assert_eq!(
            resolve_asset_uri("./body.glb", asset_root, layer_dir),
            "/ws/assets/scenes/./body.glb"
        );
    }
}
