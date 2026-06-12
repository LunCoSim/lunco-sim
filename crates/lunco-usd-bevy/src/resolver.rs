//! In-memory `ar::Resolver` for openusd composition over our asset pipeline.
//!
//! openusd 0.5.0 composes a stage by calling a **synchronous** [`ar::Resolver`]
//! to resolve + open every `@asset@` arc. `ar::DefaultResolver` uses `std::fs`
//! unconditionally — wrong on wasm and wrong for our `lunco-lib://` scheme. We
//! supply [`LuncoUsdResolver`], a pure in-memory byte-map: the loader pre-fetches
//! every transitively-referenced `.usda` through Bevy's `AssetServer`
//! (`LoadContext::read_asset_bytes`, native + wasm) and hands the bytes here. The
//! composition core never touches the filesystem (confirmed: all production
//! `std::fs` in openusd lives in `ar::DefaultResolver`, which we don't use, and
//! the `get_modification_timestamp` default, which we override).
//!
//! Identifiers (`create_identifier`) and the loader's pre-fetch BFS share ONE
//! [`canonicalize`] so the id a layer is fetched under is byte-identical to the
//! id openusd's collector later passes to `resolve` — a mismatch would surface
//! as a spurious "failed to resolve asset path" error.

use std::collections::HashMap;
use std::io::{self, Cursor};
use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

use openusd::ar::{self, Asset, ResolvedPath};

/// File extensions openusd cannot parse as USD layers — non-USD binary assets
/// referenced through `payload`/`references` (glTF, OBJ, STL). Pixar handles
/// these via `SdfFileFormat` plugins (`UsdGltf`, …); openusd-rs has no plugin
/// system, so we route them to an empty stub layer during composition and
/// surface the resolved URI as a `lunco:resolvedAsset` attribute for the Bevy
/// side to load through `AssetServer`. Matched case-insensitively.
pub(crate) const BINARY_ASSET_EXTENSIONS: &[&str] = &["glb", "gltf", "obj", "stl"];

/// Identifier every binary asset is mapped to. Ends in `.usda` so openusd's
/// `open_layer` parses it as text; [`LuncoUsdResolver::open_asset`] returns an
/// empty USD layer for it, so the binary arc composes to nothing (its URI is
/// recovered separately from the prim's authored `payload`/`references`).
pub(crate) const BINARY_STUB_ID: &str = "__lunco_binary_stub__.usda";

const EMPTY_USDA: &[u8] = b"#usda 1.0\n";

/// Resolve `..` / `.` segments without touching the filesystem (wasm-safe).
pub(crate) fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::ParentDir => {
                // Pop a real directory segment, but PRESERVE a leading `..`
                // that has nothing to pop (e.g. `../../assets/...`) — otherwise
                // relative anchors resolve to the wrong place.
                if matches!(out.components().next_back(), Some(Component::Normal(_))) {
                    out.pop();
                } else {
                    out.push("..");
                }
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// True if `asset_path` names a non-USD binary asset (see
/// [`BINARY_ASSET_EXTENSIONS`]). Strips URL query (`?…`) / fragment (`#…`)
/// first — the NASA Perseverance URL carries an `?emrc=…` query.
pub(crate) fn is_binary_asset(asset_path: &str) -> bool {
    let stem = asset_path
        .split('?')
        .next()
        .unwrap_or(asset_path)
        .split('#')
        .next()
        .unwrap_or(asset_path);
    if let Some(dot) = stem.rfind('.') {
        let ext = &stem[dot + 1..];
        BINARY_ASSET_EXTENSIONS.iter().any(|known| known.eq_ignore_ascii_case(ext))
    } else {
        false
    }
}

/// THE shared canonicalization. Maps an authored asset path + its referencing
/// layer (`anchor`) to a stable identifier == an asset-source-relative path
/// (the form `LoadContext::read_asset_bytes` expects). Three forms:
///   * `scheme://…` → passthrough (Bevy `AssetServer` source handles it).
///   * `/…` (legacy absolute-from-assets-root) → strip the leading slash.
///   * relative → resolved against the anchor layer's directory.
///
/// A relative ref inside a layer that itself lives under a `scheme://` source
/// (e.g. a rover loaded from `lunco://vessels/rovers/skid_rover.usda` that
/// references `../../components/mobility/wheel.usda`) must STAY under that
/// source. `Path` normalization collapses `scheme://` → `scheme:/` (losing the
/// source), so we split the scheme off the anchor, resolve the path part, and
/// reattach the `scheme://` prefix.
///
/// MUST stay identical between the pre-fetch BFS and the resolver (R-canon).
pub(crate) fn canonicalize(asset_path: &str, anchor: Option<&ResolvedPath>) -> String {
    if asset_path.contains("://") {
        return asset_path.to_string();
    }
    if let Some(rest) = asset_path.strip_prefix('/') {
        return normalize(Path::new(rest)).to_string_lossy().into_owned();
    }
    // Split a `scheme://` prefix off the anchor before Path-based resolution.
    let anchor_str = anchor.and_then(|a| a.to_str()).unwrap_or_default();
    let (scheme, anchor_path) = match anchor_str.split_once("://") {
        Some((s, rest)) => (Some(s), rest),
        None => (None, anchor_str),
    };
    let base = Path::new(anchor_path)
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    let resolved = normalize(&base.join(asset_path)).to_string_lossy().into_owned();
    match scheme {
        Some(s) => format!("{s}://{resolved}"),
        None => resolved,
    }
}

/// Resolve a binary asset path to a URI the Bevy `AssetServer` can load
/// (consumed via the synthesized `lunco:resolvedAsset`). `scheme://` passes
/// through; everything else is treated as an asset-source-relative path.
///
// TODO(glb-composability): binary assets (`.glb`/`.gltf`/…) are currently a
// side-channel — stubbed out of USD composition and surfaced via
// `lunco:resolvedAsset` for Bevy's glTF loader. The *proper* USD way is a
// glTF `SdfFileFormat` (dynamic file format) that composes the glb into the
// stage as real `Mesh` geometry — no special-case, no `resolvedAsset`.
//   * External tools (Blender/usdview): adopt Adobe's open-source
//     `USD-Fileformat-plugins` (glTF/FBX/OBJ/STL/PLY SdfFileFormat plugins)
//     via `PXR_PLUGINPATH` — config only, no engine code. See
//     `docs/architecture/21-domain-usd.md` (interop note).
//   * Our engine (pure-Rust `openusd`, no C++ plugin system): mirror it with a
//     small glTF→USD-layer shim in `compose.rs` (points/indices/normals/uvs →
//     `Mesh` specs) fed to the composer instead of `discover_arcs` stubbing.
// Until then the binary side-channel is retained — it works native + web.
pub(crate) fn resolve_binary_uri(asset_path: &str, anchor: Option<&ResolvedPath>) -> String {
    if asset_path.contains("://") {
        return asset_path.to_string();
    }
    canonicalize(asset_path, anchor)
}

/// In-memory resolver over pre-fetched layer bytes, keyed by [`canonicalize`]d
/// identifier. Binary assets resolve to an empty stub (see [`BINARY_STUB_ID`]).
pub(crate) struct LuncoUsdResolver {
    bytes: HashMap<String, Vec<u8>>,
}

impl LuncoUsdResolver {
    pub(crate) fn new(bytes: HashMap<String, Vec<u8>>) -> Self {
        Self { bytes }
    }
}

impl ar::Resolver for LuncoUsdResolver {
    fn create_identifier(&self, asset_path: &str, anchor: Option<&ResolvedPath>) -> String {
        if is_binary_asset(asset_path) {
            return BINARY_STUB_ID.to_string();
        }
        canonicalize(asset_path, anchor)
    }

    fn resolve(&self, asset_path: &str) -> Option<ResolvedPath> {
        if asset_path == BINARY_STUB_ID || self.bytes.contains_key(asset_path) {
            Some(ResolvedPath::new(asset_path))
        } else {
            None
        }
    }

    fn resolve_for_new_asset(&self, asset_path: &str) -> Option<ResolvedPath> {
        Some(ResolvedPath::new(asset_path))
    }

    fn open_asset(&self, resolved_path: &ResolvedPath) -> io::Result<Box<dyn Asset>> {
        let key = resolved_path.to_str().unwrap_or_default();
        if key == BINARY_STUB_ID {
            return Ok(Box::new(Cursor::new(EMPTY_USDA.to_vec())));
        }
        self.bytes
            .get(key)
            .map(|b| Box::new(Cursor::new(b.clone())) as Box<dyn Asset>)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, key.to_string()))
    }

    /// Override the one fs-touching default (`fs::metadata`) so composition is
    /// 100% filesystem-free.
    fn get_modification_timestamp(&self, _asset_path: &str, _resolved_path: &ResolvedPath) -> Option<SystemTime> {
        None
    }
}
