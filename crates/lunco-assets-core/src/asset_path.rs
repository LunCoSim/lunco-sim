//! Bevy asset-source adapters over the platform-neutral asset path algebra.

use bevy::asset::{AssetPath, io::AssetSourceId};

pub(crate) use lunco_assets_path::{
    has_scheme, is_safe_relative_components, is_safe_relative_path, relative_path, slashed,
    split_scheme, uri,
};
#[cfg(not(target_arch = "wasm32"))]
pub(crate) use lunco_assets_path::{is_anchored, normalize};

/// Rebuild the canonical `scheme://path` spelling represented by a Bevy asset
/// path. The default Bevy source is the engine `lunco://` library.
pub fn anchor_of(path: &AssetPath) -> String {
    let p = slashed(path.path());
    match path.source() {
        AssetSourceId::Name(name) => format!("{name}://{p}"),
        AssetSourceId::Default => p,
    }
}

/// Resolve a document-relative asset through the document's registered Bevy
/// source while preserving that source's authority.
pub fn source_relative_uri(path: &AssetPath, relative: &str) -> Option<String> {
    let relative = relative_path(relative)?;
    let relative = slashed(relative);
    match path.source() {
        AssetSourceId::Default => Some(crate::engine_asset_uri(&relative)),
        AssetSourceId::Name(name) if name.to_string() == crate::LUNCO_SCHEME => {
            Some(crate::engine_asset_uri(&relative))
        }
        AssetSourceId::Name(name) => {
            let path = slashed(path.path());
            let root = path.split('/').next().filter(|root| !root.is_empty())?;
            Some(uri(name, &format!("{root}/{relative}")))
        }
    }
}

/// Resolve a load reference through its originating asset source. Native file
/// URIs are admitted only inside that source's current Twin mount. This is a
/// path conversion; filesystem access and symlink validation remain on the
/// asset reader's I/O task. Labels must be attached to the returned typed path.
pub fn load_asset_path(
    reference: &str,
    origin: Option<&AssetPath<'_>>,
    roots: Option<&crate::TwinRoots>,
) -> Result<AssetPath<'static>, crate::TwinRootsError> {
    use crate::TwinRootsError;
    let invalid =
        |detail: String| TwinRootsError::AssetResolution(std::io::ErrorKind::InvalidInput, detail);
    let native =
        lunco_storage::file_uri_to_path(reference).map_err(|error| invalid(error.to_string()))?;
    if let Some(native) = native {
        let origin = origin.ok_or_else(|| {
            invalid(format!(
                "native asset `{reference}` has no originating asset source"
            ))
        })?;
        if origin.source() != &AssetSourceId::Name(crate::TWIN_SCHEME.into()) {
            return Err(invalid(format!(
                "native asset `{reference}` requires an originating Twin mount"
            )));
        }
        let source_path = slashed(origin.path());
        let (authority, _) = crate::split_twin_rel(&source_path)
            .ok_or_else(|| TwinRootsError::InvalidAuthority(source_path.clone()))?;
        let root = roots
            .ok_or(TwinRootsError::RegistryUnavailable)?
            .root_for(authority)?
            .ok_or_else(|| TwinRootsError::UnknownAuthority(authority.to_string()))?;
        // URI conversion owns Windows drive/UNC and verbatim-path spelling;
        // it needs no filesystem lookup on the projection thread.
        let root_uri =
            lunco_storage::file_path_to_uri(&root).map_err(|error| invalid(error.to_string()))?;
        let root = lunco_storage::file_uri_to_path(&root_uri)
            .map_err(|error| invalid(error.to_string()))?
            .ok_or_else(|| invalid("native Twin root has no file URI path".into()))?;
        let relative = native.strip_prefix(&root).map_err(|_| {
            invalid(format!(
                "native asset `{reference}` is outside originating Twin `{authority}`"
            ))
        })?;
        if relative.as_os_str().is_empty() || !is_safe_relative_components(relative) {
            return Err(invalid(format!("unsafe native Twin asset `{reference}`")));
        }
        return Ok(
            AssetPath::from_path_buf(std::path::Path::new(authority).join(relative))
                .with_source(crate::TWIN_SCHEME),
        );
    }
    let canonical = match origin {
        Some(origin) => lunco_assets_path::canonicalize(reference, &anchor_of(origin)),
        None => lunco_assets_path::canonicalize_root(reference),
    };
    let (source, path) = match split_scheme(&canonical) {
        Some((source, path)) => (AssetSourceId::Name(source.to_owned().into()), path),
        None => (AssetSourceId::Default, canonical.as_str()),
    };
    if path.is_empty() {
        return Err(invalid(format!(
            "asset `{canonical}` has no filesystem path"
        )));
    }
    Ok(AssetPath::from_path_buf(std::path::PathBuf::from(path)).with_source(source))
}

/// Convert an asset reference to the same-origin URL path used by the web
/// asset source.
pub fn web_url(reference: &str) -> String {
    let raw = slashed(reference);
    if raw.starts_with('/') || raw.starts_with("http://") || raw.starts_with("https://") {
        return raw;
    }
    let rel = crate::engine_asset_rel(&raw);
    if has_scheme(rel) {
        return rel.to_string();
    }
    let root = crate::ASSETS_DIR_NAME;
    if rel.starts_with(&format!("{root}/")) {
        rel.to_string()
    } else {
        format!("{root}/{rel}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn native_load_paths_keep_filesystem_characters_and_separate_labels() {
        let folder = tempfile::tempdir().expect("mount folder");
        let roots = crate::TwinRoots::default();
        let authority = roots.register("fixture", folder.path()).expect("mount");
        let root = roots
            .root_for(&authority)
            .expect("registry")
            .expect("live root");
        let origin = AssetPath::from_path_buf(std::path::Path::new(&authority).join("scene.usda"))
            .with_source(crate::TWIN_SCHEME);
        let relative = std::path::Path::new("textures").join("# 100% 月.png");
        let uri = lunco_storage::file_path_to_uri(&root.join(&relative)).expect("native URI");
        let path =
            load_asset_path(&uri, Some(&origin), Some(&roots)).expect("admitted native asset");
        assert_eq!(path.source(), origin.source());
        assert_eq!(
            path.path(),
            std::path::Path::new(&authority).join(&relative)
        );
        assert_eq!(path.label(), None);
        let labeled = path.clone().with_label("Mesh0/Primitive0");
        assert_eq!(labeled.path(), path.path());
        assert_eq!(labeled.label(), Some("Mesh0/Primitive0"));
        let sibling =
            lunco_storage::file_path_to_uri(&root.join("buffers/mesh.bin")).expect("sibling URI");
        let sibling =
            load_asset_path(&sibling, Some(&origin), Some(&roots)).expect("admitted sibling");
        assert_eq!(
            sibling.path(),
            std::path::Path::new(&authority).join("buffers/mesh.bin")
        );
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn native_load_paths_reject_missing_retired_and_foreign_ownership() {
        let folder = tempfile::tempdir().expect("mount folder");
        let outside = tempfile::tempdir().expect("other folder");
        let roots = crate::TwinRoots::default();
        let authority = roots.register("fixture", folder.path()).expect("mount");
        let origin = AssetPath::from_path_buf(std::path::Path::new(&authority).join("scene.usda"))
            .with_source(crate::TWIN_SCHEME);
        let root = roots
            .root_for(&authority)
            .expect("registry")
            .expect("live root");
        let uri = lunco_storage::file_path_to_uri(&root.join("mesh.glb")).expect("native URI");
        let foreign =
            lunco_storage::file_path_to_uri(&outside.path().join("mesh.glb")).expect("foreign URI");
        assert!(load_asset_path(&uri, None, Some(&roots)).is_err());
        assert!(
            load_asset_path(
                &uri,
                Some(&AssetPath::parse("lunco://scene.usda")),
                Some(&roots)
            )
            .is_err()
        );
        assert_eq!(
            load_asset_path(&uri, Some(&origin), None),
            Err(crate::TwinRootsError::RegistryUnavailable)
        );
        assert!(load_asset_path(&foreign, Some(&origin), Some(&roots)).is_err());
        let unknown = AssetPath::parse("twin://unknown/scene.usda");
        assert_eq!(
            load_asset_path(&uri, Some(&unknown), Some(&roots)),
            Err(crate::TwinRootsError::UnknownAuthority("unknown".into()))
        );
        roots.unregister_name(&authority).expect("unmount");
        roots.register("fixture", folder.path()).expect("reopen");
        assert_eq!(
            load_asset_path(&uri, Some(&origin), Some(&roots)),
            Err(crate::TwinRootsError::UnknownAuthority(authority))
        );
    }

    #[test]
    fn source_load_paths_preserve_literal_filename_characters() {
        let origin =
            AssetPath::from_path_buf(std::path::PathBuf::from("fixture/# 100% 月/main.usda"))
                .with_source(crate::TWIN_SCHEME);
        let path =
            load_asset_path("textures/# %.png", Some(&origin), None).expect("relative source path");
        assert_eq!(path.source(), origin.source());
        assert_eq!(
            path.path(),
            std::path::Path::new("fixture/# 100% 月/textures/# %.png")
        );
        assert_eq!(path.label(), None);
    }

    #[test]
    fn library_load_paths_do_not_require_a_twin_registry() {
        let origin = AssetPath::parse("lunco://scenes/main.usda");
        let path =
            load_asset_path("textures/albedo.png", Some(&origin), None).expect("library asset");
        assert_eq!(path, AssetPath::parse("lunco://scenes/textures/albedo.png"));
    }

    #[test]
    #[cfg(windows)]
    fn windows_native_load_paths_normalize_verbatim_mount_roots() {
        let folder = tempfile::tempdir().expect("mount folder");
        let roots = crate::TwinRoots::default();
        let authority = roots.register("fixture", folder.path()).expect("mount");
        let root = roots.root_for(&authority).expect("registry").expect("root");
        assert!(root.to_string_lossy().starts_with(r"\\?\"));
        let uri =
            lunco_storage::file_path_to_uri(&root.join("textures/# %.png")).expect("Windows URI");
        let origin = AssetPath::from_path_buf(std::path::Path::new(&authority).join("scene.usda"))
            .with_source(crate::TWIN_SCHEME);
        let path =
            load_asset_path(&uri, Some(&origin), Some(&roots)).expect("verbatim mount admission");
        assert_eq!(
            path.path(),
            std::path::Path::new(&authority).join("textures/# %.png")
        );
    }

    #[test]
    fn source_relative_uri_preserves_asset_authority() {
        let path = AssetPath::parse("twin://moonbase/scenes/main.usda").into_owned();
        assert_eq!(
            source_relative_uri(&path, "textures/albedo.png").as_deref(),
            Some("twin://moonbase/textures/albedo.png")
        );

        let library = AssetPath::parse("scenes/main.usda").into_owned();
        assert_eq!(
            source_relative_uri(&library, "textures/albedo.png").as_deref(),
            Some("lunco://textures/albedo.png")
        );
    }

    #[test]
    fn source_relative_uri_normalizes_windows_source_paths() {
        let path = AssetPath::parse(r"twin://moonbase\scenes\main.usda").into_owned();
        assert_eq!(
            source_relative_uri(&path, r"textures\albedo.png").as_deref(),
            Some("twin://moonbase/textures/albedo.png")
        );
    }

    #[test]
    fn anchor_of_normalizes_platform_separators() {
        let path = AssetPath::parse(r"twin://moonbase\scenes\main.usda").into_owned();
        assert_eq!(anchor_of(&path), "twin://moonbase/scenes/main.usda");
    }

    #[test]
    fn web_url_roots_library_paths_and_passes_addressable_ones_through() {
        assert_eq!(web_url("dem/site.tif"), "assets/dem/site.tif");
        assert_eq!(web_url("assets/dem/site.tif"), "assets/dem/site.tif");
        assert_eq!(web_url("lunco://dem/site.tif"), "assets/dem/site.tif");
        assert_eq!(web_url("https://h/x.tif"), "https://h/x.tif");
        assert_eq!(web_url("/abs/x.tif"), "/abs/x.tif");
        assert_eq!(web_url("twin://ep1/x.tif"), "twin://ep1/x.tif");
        assert_eq!(web_url("dem\\site.tif"), "assets/dem/site.tif");
    }
}
