//! Asset-backed OpenUSD composition.
//!
//! `lunco-assets-core` owns canonical asset identities and storage locations. This
//! crate owns the USD meaning of those bytes: sublayers, references, payloads,
//! variants, and OpenUSD stage assembly. It is deliberately below the Bevy
//! projector and the simulation umbrella, so tutorials and headless tools can
//! consume a composed stage without creating an upward dependency cycle.

mod resolver;

pub mod recipe;

use recipe::StageClosureLimits;
#[cfg(not(target_arch = "wasm32"))]
use recipe::StageDependencyDiagnostic;

#[cfg(not(target_arch = "wasm32"))]
use std::collections::HashMap;
use std::path::Path;

use anyhow::{Result, anyhow};
use openusd::ar::ResolvedPath;
use openusd::sdf::Data;
use openusd::usd::Stage;
use openusd::usda;

/// Maximum nesting accepted by the USDA front-end before handing the source to
/// openusd's recursive parser.  A malformed or hostile layer must fail as data;
/// it must never be able to consume the process stack.
pub const MAX_USDA_NESTING: usize = 128;

/// Validate delimiter nesting without invoking the recursive USDA parser.
///
/// USDA uses braces for prim/variant bodies, brackets for arrays and
/// parentheses for metadata.  Delimiters inside comments, strings, and asset
/// path literals are data, not structure, so they are skipped.  This is a
/// deliberately small preflight rather than a second parser: syntax remains
/// owned by openusd, while the resource bound is enforced at our asset edge.
pub fn validate_usda_nesting(text: &str) -> Result<()> {
    let bytes = text.as_bytes();
    let mut stack = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'#' => {
                i += 1;
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'"' => {
                let triple = bytes.get(i..i + 3) == Some(b"\"\"\"");
                let terminator = if triple {
                    b"\"\"\"".as_slice()
                } else {
                    b"\"".as_slice()
                };
                i += terminator.len();
                while i < bytes.len() {
                    if !triple && bytes[i] == b'\\' {
                        i = (i + 2).min(bytes.len());
                    } else if bytes.get(i..i + terminator.len()) == Some(terminator) {
                        i += terminator.len();
                        break;
                    } else {
                        i += 1;
                    }
                }
            }
            b'@' => {
                // Asset literals are delimited by the next unescaped `@`.
                i += 1;
                while i < bytes.len() {
                    if bytes[i] == b'\\' {
                        i = (i + 2).min(bytes.len());
                    } else if bytes[i] == b'@' {
                        i += 1;
                        break;
                    } else {
                        i += 1;
                    }
                }
            }
            open @ (b'{' | b'[' | b'(') => {
                stack.push(open);
                if stack.len() > MAX_USDA_NESTING {
                    anyhow::bail!(
                        "USDA nesting exceeds the safety limit of {} levels",
                        MAX_USDA_NESTING
                    );
                }
                i += 1;
            }
            close @ (b'}' | b']' | b')') => {
                let expected = match close {
                    b'}' => b'{',
                    b']' => b'[',
                    b')' => b'(',
                    _ => unreachable!(),
                };
                if stack.pop() != Some(expected) {
                    anyhow::bail!("unbalanced USDA delimiter");
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    if !stack.is_empty() {
        anyhow::bail!("unclosed USDA delimiter");
    }
    Ok(())
}

/// Parse one USDA layer after applying the stack-safety preflight.
pub fn parse_usda(text: &str) -> Result<Data> {
    validate_usda_nesting(text)?;
    usda::parse(text).map_err(|e| anyhow!("USD parse error: {e}"))
}

/// Validate one layer-closure accounting snapshot against the shared safety
/// policy used by every USD fetch adapter.
pub fn check_stage_closure_limits(
    limits: &StageClosureLimits,
    layer_count: usize,
    depth: usize,
    dependency_count: usize,
    byte_count: usize,
) -> Result<()> {
    if layer_count > limits.max_layers {
        anyhow::bail!(
            "USD layer closure contains {layer_count} discovered layers; the limit is {}",
            limits.max_layers
        );
    }
    if depth > limits.max_depth {
        anyhow::bail!(
            "USD layer closure reaches depth {depth}; the limit is {}",
            limits.max_depth
        );
    }
    if dependency_count > limits.max_dependencies_per_layer {
        anyhow::bail!(
            "USD layer declares {dependency_count} composition dependencies; the limit is {}",
            limits.max_dependencies_per_layer
        );
    }
    if byte_count > limits.max_bytes {
        anyhow::bail!(
            "USD layer closure retains {byte_count} bytes; the limit is {}",
            limits.max_bytes
        );
    }
    Ok(())
}

pub use resolver::{
    LuncoUsdResolver, ResolverDiagnostics, SharedLayerBytes, canonicalize_at, is_binary_asset,
};

/// True when `path` is a USD layer that can declare further asset dependencies.
pub fn is_usd_layer(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|extension| extension.to_str())
            .map(|extension| extension.to_ascii_lowercase())
            .as_deref(),
        Some("usd" | "usda" | "usdc")
    )
}

/// Extract every raw dependency declared by a USDA layer, including
/// asset-valued attributes as well as composition arcs.
///
/// This is USD interpretation only. Traversal and storage access remain in
/// `lunco-assets-core`.
pub fn layer_dependency_arcs(text: &str) -> Option<Vec<String>> {
    let data = parse_usda(text).ok()?;
    Some(
        data.composition_asset_dependencies()
            .into_iter()
            .chain(data.asset_dependencies())
            .collect(),
    )
}

/// Compose a native USDA file using the canonical LunCo asset traversal.
///
/// The root is promoted to `lunco://` when it lives below an `assets/` root, so
/// all arcs in the closure use the same canonical identity space.
pub fn compose_file_to_stage(path: &Path) -> Result<Stage> {
    let assets_root = lunco_assets_core::shipped_asset_root(path);
    compose_file_to_stage_with_assets(path, assets_root)
}

/// Compose an authored file with an explicit shipped-asset root. Twin/campaign
/// callers use this when the scene itself is outside the library but its
/// `lunco://` arcs still target the engine asset source.
pub fn compose_file_to_stage_with_assets(path: &Path, assets_root: Option<&Path>) -> Result<Stage> {
    compose_file_to_stage_with_roots(path, assets_root, None)
}

/// Compose a document with the asset roots that belong to its provenance.
/// `twin_root` is used only for `twin://` arcs; the engine library remains
/// resolved through `assets_root`. Keeping both roots explicit prevents a
/// custom Twin from silently falling back to the process working directory.
///
/// This synchronous byte-assembly API is native-only. Browser USD loading
/// goes through `lunco-usd-bevy`'s `LoadContext`, which is the async asset
/// boundary that can fetch `lunco://` and `twin://` resources without touching
/// a filesystem. Returning an explicit error here keeps the target boundary
/// honest instead of exposing a native reader that cannot work on wasm.
#[cfg(not(target_arch = "wasm32"))]
pub fn compose_file_to_stage_with_roots(
    path: &Path,
    assets_root: Option<&Path>,
    twin_root: Option<&Path>,
) -> Result<Stage> {
    let absolute = lunco_storage::canonicalize_file_path(path)
        .map_err(|error| anyhow!("cannot resolve USD root {}: {error}", path.display()))?;
    let source_uri = lunco_storage::file_path_to_uri(&absolute)
        .map_err(|error| anyhow!("cannot identify USD root {}: {error}", path.display()))?;
    let root_id = match assets_root.and_then(|root| path.strip_prefix(root).ok()) {
        Some(rel) => lunco_assets_core::engine_asset_uri(&lunco_assets_path::slashed(rel)),
        None => source_uri.clone(),
    };
    let limits = StageClosureLimits::default();
    let root_bytes = lunco_assets_core::read_asset_bytes_bounded_with_twin_root(
        &source_uri,
        assets_root,
        twin_root,
        limits.max_bytes,
    )
    .map_err(|error| anyhow!("cannot read {}: {error}", path.display()))?;
    let recipe =
        recipe_from_bytes_with_roots(&root_id, root_bytes, assets_root, twin_root, limits)?;
    compose_recipe_to_stage(recipe).map(|(stage, _)| stage)
}

/// Compose current in-memory layer source against the same asset resolver used
/// for file-backed stages. Document authoring uses this when no live canonical
/// stage is attached, so referenced schemas and dynamic API ports keep their
/// normal USD composition semantics during typed preflight.
#[cfg(not(target_arch = "wasm32"))]
pub fn compose_source_to_stage_with_roots(
    root_id: &str,
    source: &str,
    assets_root: Option<&Path>,
    twin_root: Option<&Path>,
) -> Result<(Stage, Vec<StageDependencyDiagnostic>)> {
    let recipe = recipe_from_source_with_roots(root_id, source, assets_root, twin_root)?;
    compose_recipe_to_stage(recipe)
}

#[cfg(not(target_arch = "wasm32"))]
fn compose_recipe_to_stage(
    recipe: recipe::StageRecipe,
) -> Result<(Stage, Vec<StageDependencyDiagnostic>)> {
    let diagnostics = recipe.dependency_diagnostics;
    let resolver = LuncoUsdResolver::new(recipe.bytes)?;
    let resolver_diagnostics = resolver.diagnostics();
    let stage = Stage::builder().resolver(resolver).open(&recipe.root_id);
    resolver_diagnostics.check()?;
    let stage = stage.map_err(|error| anyhow!("USD composition error: {error}"))?;
    Ok((stage, diagnostics))
}

/// Resolve the transitive USD layer closure for an in-memory root layer using
/// the asset roots belonging to its document. The returned recipe can be
/// attached to a USD document so each typed edit composes its current root
/// opinions through the same dependency resolver.
#[cfg(not(target_arch = "wasm32"))]
pub fn recipe_from_source_with_roots(
    root_id: &str,
    source: &str,
    assets_root: Option<&Path>,
    twin_root: Option<&Path>,
) -> Result<recipe::StageRecipe> {
    recipe_from_bytes_with_roots(
        root_id,
        source.as_bytes().to_vec(),
        assets_root,
        twin_root,
        StageClosureLimits::default(),
    )
}

/// Resolve a native text-layer closure from bytes already read by its source
/// owner. OpenUSD interprets the layer; binary input fails the text dependency
/// inspection boundary with an explicit diagnostic.
#[cfg(not(target_arch = "wasm32"))]
pub fn recipe_from_bytes_with_roots(
    root_id: &str,
    root_bytes: Vec<u8>,
    assets_root: Option<&Path>,
    twin_root: Option<&Path>,
    limits: StageClosureLimits,
) -> Result<recipe::StageRecipe> {
    let root_id = canonicalize_at(root_id, None)?;
    check_stage_closure_limits(&limits, 1, 0, 0, root_bytes.len())?;
    let mut total_bytes = root_bytes.len();
    let mut bytes = HashMap::from([(root_id.to_owned(), root_bytes)]);
    let mut seen = std::collections::HashSet::from([root_id.to_owned()]);
    let mut queue = vec![(root_id.to_owned(), 0_usize)];
    let mut diagnostics = Vec::new();
    while let Some((id, depth)) = queue.pop() {
        let raw = bytes.get(&id).expect("queued USD layer is present");
        let child_ids = child_layer_ids(&id, raw)?;
        check_stage_closure_limits(&limits, seen.len(), depth, child_ids.len(), total_bytes)?;
        for child_id in child_ids {
            if !seen.insert(child_id.clone()) {
                continue;
            }
            let child_depth = depth + 1;
            check_stage_closure_limits(&limits, seen.len(), child_depth, 0, total_bytes)?;
            let child = lunco_assets_core::read_asset_bytes_bounded_with_twin_root(
                &child_id,
                assets_root,
                twin_root,
                limits.max_bytes.saturating_sub(total_bytes),
            );
            let child = match child {
                Ok(child) => child,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    diagnostics.push(StageDependencyDiagnostic::missing(
                        id.clone(),
                        child_id.clone(),
                    ));
                    continue;
                }
                Err(error) => {
                    return Err(anyhow!(
                        "failed to fetch USD composition dependency {child_id} for {id}: {error}"
                    ));
                }
            };
            total_bytes = total_bytes
                .checked_add(child.len())
                .ok_or_else(|| anyhow!("USD layer closure byte count overflowed"))?;
            check_stage_closure_limits(&limits, seen.len(), child_depth, 0, total_bytes)?;
            bytes.insert(child_id.clone(), child);
            queue.push((child_id, child_depth));
        }
    }
    let mut recipe = recipe::StageRecipe::new(root_id, bytes);
    recipe.dependency_diagnostics = diagnostics;
    Ok(recipe)
}

#[cfg(target_arch = "wasm32")]
pub fn compose_file_to_stage_with_roots(
    path: &Path,
    assets_root: Option<&Path>,
    twin_root: Option<&Path>,
) -> Result<Stage> {
    let _ = (path, assets_root, twin_root);
    Err(anyhow!(
        "synchronous USD composition is unavailable on wasm; load the stage through lunco-usd-bevy"
    ))
}

/// Discover a USDA layer's non-binary composition dependencies in canonical
/// asset identity space. Fetch adapters use this; they do not parse arcs.
pub fn child_layer_ids(id: &str, raw: &[u8]) -> Result<Vec<String>> {
    let text = std::str::from_utf8(raw).map_err(|e| anyhow!("layer {id} is not UTF-8: {e}"))?;
    let data = parse_usda(text).map_err(|e| anyhow!("USD parse error in {id}: {e}"))?;
    let anchor = ResolvedPath::new(id);
    canonicalize_at(id, None)?;
    let mut children = Vec::new();
    for arc in data.composition_asset_dependencies() {
        let canonical = canonicalize_at(&arc, Some(&anchor))?;
        if !is_binary_asset(&canonical) {
            children.push(canonical);
        }
    }
    Ok(children)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scalar_asset_identifiers_follow_the_strongest_contributing_layer() {
        let root = "twin://fixture/scenes/root.usda";
        let child = "twin://fixture/scenes/parts/child.usda";
        let bytes = std::collections::HashMap::from([
            (
                root.to_owned(),
                br#"#usda 1.0
                def Scope "Instance" (prepend references = @parts/child.usda@</Source>) {
                    asset override = @parent.png@
                    asset blocked = None
                }
            "#
                .to_vec(),
            ),
            (
                child.to_owned(),
                br#"#usda 1.0
                (expressionVariables = { string NAME = "image" })
                def Scope "Source" {
                    asset image = @textures/image # %.png@
                    asset override = @child.png@
                    asset blocked = @blocked.png@
                    asset[] images = [@textures/a.png@, @textures/missing.png@]
                    asset expression = @`"textures/${NAME}.png"`@
                    asset binary = @meshes/model.glb@
                }
            "#
                .to_vec(),
            ),
        ]);
        let stage = Stage::builder()
            .resolver(LuncoUsdResolver::new(bytes).unwrap())
            .open(root)
            .unwrap();
        let asset = |name: &str| {
            stage
                .attribute(openusd::sdf::Path::new(&format!("/Instance.{name}")).unwrap())
                .get::<openusd::sdf::AssetPath>()
                .unwrap()
        };
        let image = asset("image").unwrap();
        assert_eq!(image.as_str(), "textures/image # %.png");
        assert_eq!(
            image.canonical_identifier(),
            Some("twin://fixture/scenes/parts/textures/image # %.png")
        );
        assert_eq!(
            image.resolved_path(),
            None,
            "identity does not require payload existence"
        );
        assert_eq!(
            asset("override").unwrap().canonical_identifier(),
            Some("twin://fixture/scenes/parent.png")
        );
        assert_eq!(asset("blocked"), None);
        let images = stage
            .attribute(openusd::sdf::Path::new("/Instance.images").unwrap())
            .get::<Vec<openusd::sdf::AssetPath>>()
            .unwrap()
            .unwrap();
        assert_eq!(
            images[1].canonical_identifier(),
            Some("twin://fixture/scenes/parts/textures/missing.png")
        );
        let expression = asset("expression").unwrap();
        assert_eq!(expression.as_str(), "`\"textures/${NAME}.png\"`");
        assert_eq!(
            expression.canonical_identifier(),
            Some("twin://fixture/scenes/parts/textures/image.png")
        );
        assert_eq!(
            asset("binary").unwrap().canonical_identifier(),
            Some("twin://fixture/scenes/parts/meshes/model.glb")
        );
    }

    #[test]
    fn child_layer_ids_normalize_windows_separators_in_twin_uris() {
        let source = r#"#usda 1.0
(
    subLayers = [
        @twin://fixture\sim\scenes\traverse.usda@
    ]
)
"#;

        assert_eq!(
            child_layer_ids("twin://fixture/sim/scenes/entry.usda", source.as_bytes())
                .expect("valid scene layer"),
            vec!["twin://fixture/sim/scenes/traverse.usda"]
        );
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn compose_reads_a_windows_authored_twin_sublayer_on_every_native_os() {
        let temp = tempfile::tempdir().expect("temporary Twin root");
        let scene = temp.path().join("sim/scenes/entry.usda");
        let world = temp.path().join("sim/scenes/traverse.usda");
        lunco_storage::ensure_directory_sync(scene.parent().expect("scene parent"))
            .expect("create scene parent");
        lunco_storage::ensure_directory_sync(world.parent().expect("world parent"))
            .expect("create world parent");
        lunco_storage::write_file_sync(
            &scene,
            br#"#usda 1.0
(
    subLayers = [
        @twin://fixture\sim\scenes\traverse.usda@
    ]
)
"#,
        )
        .expect("write scene");
        lunco_storage::write_file_sync(&world, b"#usda 1.0\ndef Scope \"Traverse\" {}\n")
            .expect("write world");

        compose_file_to_stage_with_roots(&scene, None, Some(temp.path()))
            .expect("a Windows-authored Twin URI composes on this OS");
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn external_twin_composition_reads_authored_engine_layers() {
        let temp = tempfile::tempdir().expect("temporary Twin root");
        let scene = temp.path().join("scenes/external_twin_surface_ops.usda");
        lunco_storage::ensure_directory_sync(scene.parent().expect("scene parent"))
            .expect("create scene parent");
        lunco_storage::write_file_sync(
            &scene,
            br#"#usda 1.0
(
    subLayers = [
        @lunco://scenes\base\lunar_surface.usda@
    ]
)
"#,
        )
        .expect("write external Twin scene");

        compose_file_to_stage_with_roots(&scene, None, Some(temp.path()))
            .expect("external Twin can resolve authored engine layers");
    }

    #[test]
    fn rejects_deep_nesting_before_recursive_parser() {
        let mut source = String::from("#usda 1.0\n");
        for depth in 0..=MAX_USDA_NESTING {
            source.push_str(&format!("def Xform \"P{depth}\" {{\n"));
        }
        for _ in 0..=MAX_USDA_NESTING {
            source.push_str("}\n");
        }
        let error = parse_usda(&source).expect_err("deep USDA must be rejected");
        assert!(error.to_string().contains("nesting exceeds"));
    }

    #[test]
    fn delimiters_in_comments_strings_and_assets_are_not_structure() {
        let source = r#"#usda 1.0
# { [ ( } ] )
def Xform "World"
{
    custom string note = "{ [ ( } ] )"
    custom asset source = @asset/{nested}/mesh.usd@
}
"#;
        validate_usda_nesting(source).expect("quoted and commented delimiters are data");
        parse_usda(source).expect("valid USDA remains parseable");
    }

    #[test]
    fn closure_limits_reject_unbounded_graph_dimensions() {
        let limits = StageClosureLimits {
            max_layers: 2,
            max_dependencies_per_layer: 2,
            max_depth: 2,
            max_bytes: 8,
            max_parallel_reads: 2,
        };

        assert!(check_stage_closure_limits(&limits, 3, 0, 0, 0).is_err());
        assert!(check_stage_closure_limits(&limits, 1, 3, 0, 0).is_err());
        assert!(check_stage_closure_limits(&limits, 1, 0, 3, 0).is_err());
        assert!(check_stage_closure_limits(&limits, 1, 0, 0, 9).is_err());
        check_stage_closure_limits(&limits, 2, 2, 2, 8).expect("boundary values are permitted");
    }
}
