//! In-memory `ar::Resolver` for openusd composition over our asset pipeline.
//!
//! openusd 0.5.0 composes a stage by calling a **synchronous** [`ar::Resolver`]
//! to resolve + open every `@asset@` arc. `ar::DefaultResolver` uses `std::fs`
//! unconditionally — wrong on wasm and wrong for our `lunco://` scheme. We
//! supply [`LuncoUsdResolver`], a pure in-memory byte-map: the loader pre-fetches
//! every transitively-referenced `.usda` through Bevy's `AssetServer`
//! (`LoadContext::read_asset_bytes`, native + wasm) and hands the bytes here. The
//! composition core never touches the filesystem (confirmed: all production
//! `std::fs` in openusd lives in `ar::DefaultResolver`, which we don't use, and
//! the `get_modification_timestamp` default, which we override).
//!
//! Identifiers and the loader's pre-fetch BFS share [`canonicalize_at`], so a
//! fetched layer has the same identity OpenUSD later resolves. Native roots use
//! standard file URIs; logical asset sources use the asset-path algebra.
//! Invalid identifiers fail at layer admission before lazy composition.

use std::cell::RefCell;
use std::collections::HashMap;
use std::io::{self, Cursor};
use std::rc::Rc;
use std::time::SystemTime;

use crate::validate_usda_nesting;
use openusd::ar::{self, Asset, ResolvedPath};

use lunco_assets_path::{canonicalize, canonicalize_root};

/// The layer-byte map a [`LuncoUsdResolver`] resolves against, wrapped for
/// **shared interior mutability**. openusd captures the resolver at stage-build
/// time with no API to swap it, but composition is demand-driven (lazy): a
/// reference authored onto a live stage isn't resolved until the referencing
/// prim is (re)indexed. So a resolver backed by this shared map can gain new
/// layer bytes at runtime — inject the closure of a spawned asset here, then
/// author the `references` arc, and PCP composes it on the next read. `Rc`
/// (not `Arc`) because the composed `Stage` is `!Send` and lives on the main
/// thread; the resolver shares its thread.
pub type SharedLayerBytes = Rc<RefCell<HashMap<String, Vec<u8>>>>;

/// File extensions openusd cannot parse as USD layers — non-USD binary assets
/// referenced through `payload`/`references`. Pixar handles these via
/// `SdfFileFormat` plugins (`UsdGltf`, …); openusd-rs has no plugin system, so
/// the resolver routes them to an empty composition stub. The render projection
/// reads the authored arc directly from the live prim stack. Matched
/// case-insensitively.
pub(crate) const BINARY_ASSET_EXTENSIONS: &[&str] = &["glb", "gltf", "obj", "stl"];

/// Identifier every binary asset is mapped to. Ends in `.usda` so openusd's
/// `open_layer` parses it as text; [`LuncoUsdResolver::open_asset`] returns an
/// empty USD layer for it, so the binary arc composes to nothing (its URI is
/// recovered separately from the prim's authored `payload`/`references`).
pub(crate) const BINARY_STUB_ID: &str = "__lunco_binary_stub__.usda";

const EMPTY_USDA: &[u8] = b"#usda 1.0\n";

/// True if `asset_path` names a non-USD binary asset (see
/// [`BINARY_ASSET_EXTENSIONS`]). HTTP URLs use their URL pathname; internal
/// asset identifiers and native filenames retain literal `#`, `?` and `%`.
pub fn is_binary_asset(asset_path: &str) -> bool {
    let is_http = lunco_assets_path::split_scheme(asset_path).is_some_and(|(scheme, _)| {
        scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https")
    });
    let parsed = if is_http {
        match url::Url::parse(asset_path) {
            Ok(url) => Some(url),
            Err(_) => return false,
        }
    } else {
        None
    };
    let stem = parsed.as_ref().map_or(asset_path, url::Url::path);
    if let Some(dot) = stem.rfind('.') {
        let ext = &stem[dot + 1..];
        BINARY_ASSET_EXTENSIONS
            .iter()
            .any(|known| known.eq_ignore_ascii_case(ext))
    } else {
        false
    }
}

/// Resolve an authored asset identifier against its document.
///
/// Asset-source identities use `lunco-assets-path`. Native file identities use
/// the storage owner's standard file-URI contract and URL path segments, which
/// preserve drive/share roots and encode literal filenames on every platform.
pub fn canonicalize_at(asset_path: &str, anchor: Option<&ResolvedPath>) -> anyhow::Result<String> {
    if let Some(path) = lunco_storage::file_uri_to_path(asset_path)? {
        return Ok(lunco_storage::file_path_to_uri(&path)?);
    }
    let Some(anchor) = anchor else {
        return Ok(canonicalize_root(asset_path));
    };
    let anchor = anchor
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("USD anchor is not UTF-8"))?;
    if lunco_assets_path::is_anchored(&lunco_assets_path::slashed(asset_path)) {
        return Ok(canonicalize_root(asset_path));
    }
    if let Some(path) = lunco_storage::file_uri_to_path(anchor)? {
        let mut url = url::Url::parse(&lunco_storage::file_path_to_uri(&path)?)?;
        let path_segments = url
            .path_segments()
            .ok_or_else(|| anyhow::anyhow!("invalid file URI anchor: {anchor}"))?
            .collect::<Vec<_>>();
        let first = path_segments.first().copied().unwrap_or_default();
        let drive = first.len() == 2
            && first.as_bytes()[0].is_ascii_alphabetic()
            && first.as_bytes()[1] == b':';
        let root_depth = usize::from(url.host_str().is_some() || drive);
        let mut depth = path_segments.len().saturating_sub(1);
        {
            let mut segments = url.path_segments_mut().map_err(|_| {
                anyhow::anyhow!("file URI cannot anchor asset {asset_path:?}: {anchor}")
            })?;
            segments.pop();
            for segment in lunco_assets_path::slashed(asset_path).split('/') {
                match segment {
                    "" | "." => {}
                    ".." => {
                        if depth > root_depth {
                            segments.pop();
                            depth -= 1;
                        }
                    }
                    segment => {
                        segments.push(segment);
                        depth += 1;
                    }
                }
            }
        }
        let id = url.to_string();
        lunco_storage::file_uri_to_path(&id)?;
        return Ok(id);
    }
    Ok(canonicalize(asset_path, anchor))
}

/// Diagnostics retained across OpenUSD's infallible identifier callback.
/// Stage admission checks this handle before returning a composed stage.
#[derive(Clone, Default)]
pub struct ResolverDiagnostics(Rc<RefCell<Vec<(String, String)>>>);

impl ResolverDiagnostics {
    pub fn check(&self) -> anyhow::Result<()> {
        let diagnostics = self.0.borrow();
        if diagnostics.is_empty() {
            return Ok(());
        }
        anyhow::bail!(
            "invalid USD asset identifiers: {}",
            diagnostics
                .iter()
                .map(|(id, error)| format!("{id:?}: {error}"))
                .collect::<Vec<_>>()
                .join("; ")
        )
    }
}

// TODO(glb-composability): binary assets (`.glb`/`.gltf`) remain an explicit
// render projection because the pure-Rust openusd fork has no
// `SdfFileFormat` plugin system. The standard payload/reference is composed as
// an empty stub and the render reader loads the authored URI through Bevy.
//   * External tools (Blender/usdview): adopt Adobe's open-source
//     `USD-Fileformat-plugins` (glTF/FBX/OBJ/STL/PLY SdfFileFormat plugins)
//     via `PXR_PLUGINPATH` — config only, no engine code. See
//     `docs/architecture/21-domain-usd.md` (interop note).
//   * Our engine (pure-Rust `openusd`, no C++ plugin system): mirror it with a
//     small glTF→USD-layer adapter in `compose.rs` (points/indices/normals/uvs →
//     `Mesh` specs) fed to the composer instead of stubbing. This is an
//     interop enhancement, not a reason to duplicate asset identity in USD.

/// In-memory resolver over pre-fetched layer bytes, keyed by [`canonicalize`]d
/// identifier. Binary assets resolve to an empty stub (see [`BINARY_STUB_ID`]).
/// The byte map is [`SharedLayerBytes`] so the owning [`CanonicalStage`] can
/// inject a spawned asset's layer closure at runtime and have a
/// subsequently-authored reference compose (demand-driven resolution).
pub struct LuncoUsdResolver {
    bytes: SharedLayerBytes,
    diagnostics: ResolverDiagnostics,
}

impl LuncoUsdResolver {
    /// Admit layer identifiers and authored arcs before OpenUSD's lazy reads.
    pub fn new(bytes: HashMap<String, Vec<u8>>) -> anyhow::Result<Self> {
        for (id, raw) in &bytes {
            crate::child_layer_ids(id, raw)?;
        }
        Ok(Self {
            bytes: Rc::new(RefCell::new(bytes)),
            diagnostics: ResolverDiagnostics::default(),
        })
    }

    /// Capture identifier errors across the infallible OpenUSD callback.
    pub fn diagnostics(&self) -> ResolverDiagnostics {
        self.diagnostics.clone()
    }

    /// Clone the byte-map handle for validated runtime layer injection.
    pub fn shared(&self) -> SharedLayerBytes {
        self.bytes.clone()
    }
}

impl ar::Resolver for LuncoUsdResolver {
    fn create_identifier(&self, asset_path: &str, anchor: Option<&ResolvedPath>) -> String {
        match canonicalize_at(asset_path, anchor) {
            Ok(id) => id,
            Err(error) => {
                // The trait cannot return an error. Retain the authored spelling
                // for its diagnostic and make resolution explicitly fail.
                self.diagnostics
                    .0
                    .borrow_mut()
                    .push((asset_path.to_owned(), error.to_string()));
                asset_path.to_owned()
            }
        }
    }

    fn resolve(&self, asset_path: &str) -> Option<ResolvedPath> {
        if self
            .diagnostics
            .0
            .borrow()
            .iter()
            .any(|(id, _)| id == asset_path)
        {
            return None;
        }
        if is_binary_asset(asset_path) {
            Some(ResolvedPath::new(BINARY_STUB_ID))
        } else if asset_path == BINARY_STUB_ID || self.bytes.borrow().contains_key(asset_path) {
            Some(ResolvedPath::new(asset_path))
        } else {
            None
        }
    }

    fn resolve_for_new_asset(&self, asset_path: &str) -> Option<ResolvedPath> {
        match canonicalize_at(asset_path, None) {
            Ok(id) => Some(ResolvedPath::new(id)),
            Err(error) => {
                self.diagnostics
                    .0
                    .borrow_mut()
                    .push((asset_path.to_owned(), error.to_string()));
                None
            }
        }
    }

    fn open_asset(&self, resolved_path: &ResolvedPath) -> io::Result<Box<dyn Asset>> {
        let key = resolved_path.to_str().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "USD asset identifier is not UTF-8",
            )
        })?;
        if key == BINARY_STUB_ID {
            return Ok(Box::new(Cursor::new(EMPTY_USDA.to_vec())));
        }
        let bytes = self
            .bytes
            .borrow()
            .get(key)
            .cloned()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, key.to_string()))?;
        if let Ok(text) = std::str::from_utf8(&bytes) {
            validate_usda_nesting(text).map_err(|error| {
                io::Error::new(io::ErrorKind::InvalidData, format!("{key}: {error}"))
            })?;
        }
        Ok(Box::new(Cursor::new(bytes)))
    }

    /// Override the one fs-touching default (`fs::metadata`) so composition is
    /// 100% filesystem-free.
    fn get_modification_timestamp(
        &self,
        _asset_path: &str,
        _resolved_path: &ResolvedPath,
    ) -> Option<SystemTime> {
        None
    }
}

#[cfg(test)]
mod binary_classification_tests {
    use super::*;

    #[test]
    fn binary_classification_preserves_logical_filename_suffixes() {
        for address in [
            "twin://fixture/models/model # %.glb",
            "lunco://models/model ? %.GLTF",
            "twin://fixture/models/model.usda#payload.glb",
            r"C:\models\model # %.stl",
            "https://example.invalid/model.glb?version=2#view",
        ] {
            assert!(is_binary_asset(address), "{address}");
        }
        for address in [
            "twin://fixture/models/model.glb#source.usda",
            "lunco://models/model.glb?source.usda",
            "https://example.invalid/model.usda?next=model.glb#view.glb",
        ] {
            assert!(!is_binary_asset(address), "{address}");
        }
        let source = br#"#usda 1.0
            def Scope "Mesh" (prepend references = @models/model # %.glb@) {}
        "#;
        assert!(
            crate::child_layer_ids("twin://fixture/root.usda", source)
                .unwrap()
                .is_empty()
        );
        let source = br#"#usda 1.0
            def Scope "Part" (prepend references = @models/model.glb#source.usda@) {}
        "#;
        assert_eq!(
            crate::child_layer_ids("twin://fixture/root.usda", source).unwrap(),
            ["twin://fixture/models/model.glb#source.usda"]
        );
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    #[test]
    fn native_file_anchors_preserve_roots_and_encode_relative_filenames() {
        let root = std::env::temp_dir()
            .join("composition café")
            .join("scene.usda");
        let anchor = ResolvedPath::new(lunco_storage::file_path_to_uri(&root).unwrap());
        let id = canonicalize_at(r"folder\..\available #%.usda", Some(&anchor)).unwrap();
        let native = lunco_storage::file_uri_to_path(&id).unwrap().unwrap();
        assert_eq!(native, root.parent().unwrap().join("available #%.usda"));
        assert_eq!(
            canonicalize_at("/scenes/asset.usda", Some(&anchor)).unwrap(),
            "scenes/asset.usda"
        );
        assert!(canonicalize_at("file:///bad%00.usda", Some(&anchor)).is_err());
        let binary = canonicalize_at("mesh#v1.glb", Some(&anchor)).unwrap();
        assert!(is_binary_asset(&binary));
        let source = b"#usda 1.0\ndef Xform \"Mesh\" (prepend references = @mesh#v1.glb@) {}\n";
        assert!(
            crate::child_layer_ids(anchor.to_str().unwrap(), source)
                .unwrap()
                .is_empty()
        );

        #[cfg(windows)]
        for (native, expected) in [
            (r"C:\dir\scene.usda", "file:///C:/dir/child.usda"),
            (r"\\?\C:\dir\scene.usda", "file:///C:/dir/child.usda"),
            (
                r"\\server\share\dir\scene.usda",
                "file://server/share/dir/child.usda",
            ),
            (
                r"\\?\UNC\server\share\dir\scene.usda",
                "file://server/share/dir/child.usda",
            ),
        ] {
            let anchor = ResolvedPath::new(
                lunco_storage::file_path_to_uri(std::path::Path::new(native)).unwrap(),
            );
            assert_eq!(
                canonicalize_at("child.usda", Some(&anchor)).unwrap(),
                expected
            );
            let at_root = canonicalize_at("../../../../child.usda", Some(&anchor)).unwrap();
            assert!(
                at_root == "file:///C:/child.usda" || at_root == "file://server/share/child.usda",
                "{at_root}"
            );
        }
    }

    #[test]
    fn infallible_resolver_adapter_retains_invalid_identifier_diagnostic() {
        use openusd::ar::Resolver;
        let invalid = "file:///invalid%00.usda";
        assert!(
            LuncoUsdResolver::new(HashMap::from([(invalid.to_owned(), EMPTY_USDA.to_vec())]))
                .is_err()
        );
        let resolver = LuncoUsdResolver::new(HashMap::new()).unwrap();
        let id = resolver.create_identifier(invalid, None);
        assert_eq!(id, invalid);
        assert!(resolver.resolve(&id).is_none());
        assert!(resolver.resolve_for_new_asset(invalid).is_none());
        let error = resolver.diagnostics().check().unwrap_err().to_string();
        assert!(error.contains(invalid), "{error}");
    }
}
