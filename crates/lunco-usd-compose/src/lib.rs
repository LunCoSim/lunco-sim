//! Asset-backed OpenUSD composition.
//!
//! `lunco-assets` owns canonical asset identities and storage locations. This
//! crate owns the USD meaning of those bytes: sublayers, references, payloads,
//! variants, and OpenUSD stage assembly. It is deliberately below the Bevy
//! projector and the simulation umbrella, so tutorials and headless tools can
//! consume a composed stage without creating an upward dependency cycle.

mod resolver;

#[cfg(not(target_arch = "wasm32"))]
use std::collections::HashMap;
use std::path::Path;

use anyhow::{anyhow, Result};
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

pub use resolver::{canonicalize_at, is_binary_asset, LuncoUsdResolver, SharedLayerBytes};

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
/// `lunco-assets`.
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
    let assets_root = lunco_assets::shipped_asset_root(path);
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
    let root_id = match assets_root.and_then(|root| path.strip_prefix(root).ok()) {
        Some(rel) => lunco_assets::engine_asset_uri(&lunco_assets::asset_path::slashed(rel)),
        None => lunco_assets::asset_path::canonicalize_root(&path.to_string_lossy()),
    };
    let root_bytes = lunco_assets::read_asset_file_bytes(path)
        .map_err(|e| anyhow!("cannot read {}: {e}", path.display()))?;
    let mut bytes = HashMap::from([(root_id.clone(), root_bytes)]);
    let mut queue = vec![root_id.clone()];
    while let Some(id) = queue.pop() {
        let raw = bytes
            .get(&id)
            .cloned()
            .expect("queued USD layer is present");
        for child_id in child_layer_ids(&id, &raw)? {
            if bytes.contains_key(&child_id) {
                continue;
            }
            let child =
                lunco_assets::read_asset_bytes_with_twin_root(&child_id, assets_root, twin_root)
                    .map_err(|e| {
                        anyhow!(
                            "failed to fetch sublayer {child_id} for {}: {e}",
                            path.display()
                        )
                    })?;
            bytes.insert(child_id.clone(), child);
            queue.push(child_id);
        }
    }
    Stage::builder()
        .resolver(LuncoUsdResolver::new(bytes))
        .open(&root_id)
        .map_err(|e| anyhow!("USD composition error: {e}"))
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
    Ok(data
        .composition_asset_dependencies()
        .into_iter()
        .filter(|arc| !is_binary_asset(arc))
        .map(|arc| canonicalize_at(&arc, Some(&anchor)))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
