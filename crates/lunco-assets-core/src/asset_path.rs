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

/// Native addresses prepared on an I/O worker for one originating asset source.
/// Entries retain failures so invalid authored references fail at their consumer.
#[derive(Clone, Debug, Default)]
pub struct PreparedAssetPaths {
    origin: Option<AssetPath<'static>>,
    entries: std::collections::HashMap<String, Result<AssetPath<'static>, crate::TwinRootsError>>,
}

impl PreparedAssetPaths {
    /// Empty preparation bound to the current source, with no filesystem access.
    pub fn for_origin(origin: Option<AssetPath<'static>>) -> Self {
        Self {
            origin,
            entries: Default::default(),
        }
    }

    /// Canonicalize native references. Call only from the asset/preparation worker;
    /// the runtime resolves this immutable table without filesystem access.
    pub fn prepare_on_worker(
        references: impl IntoIterator<Item = String>,
        origin: Option<AssetPath<'static>>,
        roots: Option<&crate::TwinRoots>,
    ) -> Self {
        let entries = references
            .into_iter()
            .map(|reference| {
                let result = prepare_native_path(&reference, origin.as_ref(), roots);
                (reference, result)
            })
            .collect();
        Self { origin, entries }
    }

    /// Recheck mount admission before an asynchronous result is published.
    pub fn validate_owner(
        &self,
        roots: Option<&crate::TwinRoots>,
    ) -> Result<(), crate::TwinRootsError> {
        if !self.entries.is_empty() {
            native_origin_root("native preparation", self.origin.as_ref(), roots)?;
        }
        Ok(())
    }

    /// Whether this table belongs to the same exact Twin mount as the consumer.
    pub fn is_for_origin(&self, origin: Option<&AssetPath<'_>>) -> bool {
        same_native_mount(self.origin.as_ref(), origin)
    }

    /// Whether this exact authored input has a terminal preparation result.
    pub fn contains(&self, reference: &str) -> bool {
        self.entries.contains_key(reference)
    }

    /// Extend a table only with preparation for the same exact Twin mount.
    pub fn merge(&mut self, prepared: Self) -> Result<(), crate::TwinRootsError> {
        if !same_native_mount(self.origin.as_ref(), prepared.origin.as_ref()) {
            return Err(invalid_asset(
                "native asset preparation changed its originating source",
            ));
        }
        self.entries.extend(prepared.entries);
        Ok(())
    }
}

fn same_native_mount(left: Option<&AssetPath<'_>>, right: Option<&AssetPath<'_>>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) if left.source() == right.source() => {
            let left = slashed(left.path());
            let right = slashed(right.path());
            crate::split_twin_rel(&left).map(|(authority, _)| authority)
                == crate::split_twin_rel(&right).map(|(authority, _)| authority)
        }
        (None, None) => true,
        _ => false,
    }
}

fn invalid_asset(detail: impl Into<String>) -> crate::TwinRootsError {
    crate::TwinRootsError::AssetResolution(std::io::ErrorKind::InvalidInput, detail.into())
}

fn native_origin_root(
    reference: &str,
    origin: Option<&AssetPath<'_>>,
    roots: Option<&crate::TwinRoots>,
) -> Result<(String, std::path::PathBuf), crate::TwinRootsError> {
    use crate::TwinRootsError;
    let origin = origin.ok_or_else(|| {
        invalid_asset(format!(
            "native asset `{reference}` has no originating asset source"
        ))
    })?;
    if origin.source() != &AssetSourceId::Name(crate::TWIN_SCHEME.into()) {
        return Err(invalid_asset(format!(
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
    Ok((authority.to_owned(), root))
}

fn prepare_native_path(
    reference: &str,
    origin: Option<&AssetPath<'_>>,
    roots: Option<&crate::TwinRoots>,
) -> Result<AssetPath<'static>, crate::TwinRootsError> {
    let native = lunco_storage::file_uri_to_path(reference)
        .map_err(|error| invalid_asset(error.to_string()))?
        .ok_or_else(|| invalid_asset(format!("asset `{reference}` is not a native file URI")))?;
    let (authority, root) = native_origin_root(reference, origin, roots)?;
    // Canonical paths preserve platform filesystem semantics, including Windows
    // case-sensitive directories, junctions, UNC shares and verbatim prefixes.
    let canonical = |path: &std::path::Path| {
        lunco_storage::canonicalize_file_path(path).map_err(|error| {
            let kind = match &error {
                lunco_storage::StorageError::Io(error) => error.kind(),
                _ => std::io::ErrorKind::InvalidInput,
            };
            crate::TwinRootsError::AssetResolution(kind, format!("{}: {error}", path.display()))
        })
    };
    let root = canonical(&root)?;
    let native = canonical(&native)?;
    let relative = native.strip_prefix(&root).map_err(|_| {
        invalid_asset(format!(
            "native asset `{reference}` is outside originating Twin `{authority}`"
        ))
    })?;
    if relative.as_os_str().is_empty() || !is_safe_relative_components(relative) {
        return Err(invalid_asset(format!(
            "unsafe native Twin asset `{reference}`"
        )));
    }
    // A retired authority can never publish a prepared address for a later Twin.
    native_origin_root(reference, origin, roots)?;
    Ok(
        AssetPath::from_path_buf(std::path::Path::new(&authority).join(relative))
            .with_source(crate::TWIN_SCHEME),
    )
}

/// Resolve a load reference without filesystem access. Native file URIs require
/// a worker-prepared result for the exact origin and its still-live Twin mount.
/// Labels must be attached separately to the returned typed path.
pub fn load_asset_path(
    reference: &str,
    origin: Option<&AssetPath<'_>>,
    roots: Option<&crate::TwinRoots>,
    prepared: Option<&PreparedAssetPaths>,
) -> Result<AssetPath<'static>, crate::TwinRootsError> {
    if lunco_storage::file_uri_to_path(reference)
        .map_err(|error| invalid_asset(error.to_string()))?
        .is_some()
    {
        native_origin_root(reference, origin, roots)?;
        let prepared = prepared
            .filter(|prepared| same_native_mount(prepared.origin.as_ref(), origin))
            .ok_or_else(|| {
                invalid_asset(format!(
                    "native asset `{reference}` has no preparation for its current source"
                ))
            })?;
        return prepared.entries.get(reference).cloned().ok_or_else(|| {
            invalid_asset(format!(
                "native asset `{reference}` was not prepared for this source revision"
            ))
        })?;
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
        return Err(invalid_asset(format!(
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
    fn logical_load_addresses_preserve_filename_characters_without_labels() {
        for (reference, source, path) in [
            (
                "twin://fixture/scenes/scene # % 月.usda",
                Some(crate::TWIN_SCHEME),
                "fixture/scenes/scene # % 月.usda",
            ),
            (
                "lunco://scenes/scene # % 月.usda",
                Some(crate::LUNCO_SCHEME),
                "scenes/scene # % 月.usda",
            ),
            (
                "schemas/schema # % 月.usda",
                None,
                "schemas/schema # % 月.usda",
            ),
        ] {
            let address =
                load_asset_path(reference, None, None, None).expect("logical load address");
            assert_eq!(address.path(), std::path::Path::new(path));
            assert_eq!(
                address.source(),
                &source
                    .map(|source| AssetSourceId::Name(source.into()))
                    .unwrap_or_default()
            );
            assert_eq!(address.label(), None);
        }
        let origin = load_asset_path("twin://fixture/scenes/main.usda", None, None, None).unwrap();
        let relative = load_asset_path("child # % 月.usda", Some(&origin), None, None).unwrap();
        assert_eq!(
            relative.path(),
            std::path::Path::new("fixture/scenes/child # % 月.usda")
        );
        assert_eq!(relative.source(), origin.source());
        assert_eq!(relative.label(), None);
        assert_eq!(relative.with_label("Scene0").label(), Some("Scene0"));
    }

    fn prepared_load_asset_path(
        reference: &str,
        origin: Option<&AssetPath<'_>>,
        roots: Option<&crate::TwinRoots>,
    ) -> Result<AssetPath<'static>, crate::TwinRootsError> {
        let prepared = PreparedAssetPaths::prepare_on_worker(
            [reference.to_owned()],
            origin.map(|origin| origin.clone().into_owned()),
            roots,
        );
        load_asset_path(reference, origin, roots, Some(&prepared))
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn native_preparation_canonicalizes_aliases_and_rejects_retired_mounts() {
        let folder = tempfile::tempdir().expect("mount folder");
        let root = folder.path().join("MiXeDRoot");
        lunco_storage::ensure_directory_sync(&root).expect("root folder");
        let file = root.join("payload # % 月.txt");
        lunco_storage::write_file_sync(&file, b"native payload").expect("payload");
        let outside = folder.path().join("foreign.txt");
        lunco_storage::write_file_sync(&outside, b"outside").expect("foreign payload");
        let roots = crate::TwinRoots::default();
        let authority = roots.register("fixture", &root).expect("mount");
        let origin = AssetPath::from_path_buf(std::path::Path::new(&authority).join("scene.usda"))
            .with_source(crate::TWIN_SCHEME);
        #[cfg(windows)]
        let alias = folder
            .path()
            .join("mIxEdRoOt")
            .join(file.file_name().expect("filename"));
        #[cfg(unix)]
        let alias = {
            let alias_root = folder.path().join("alias");
            lunco_storage::create_directory_symlink_sync(&root, &alias_root).expect("root alias");
            alias_root.join(file.file_name().expect("filename"))
        };
        #[cfg(not(any(windows, unix)))]
        let alias = file.clone();
        let reference = lunco_storage::file_path_to_uri(&alias).expect("native alias URI");
        let foreign = lunco_storage::file_path_to_uri(&outside).expect("outside URI");
        let missing =
            lunco_storage::file_path_to_uri(&root.join("missing.txt")).expect("missing URI");
        let prepared = PreparedAssetPaths::prepare_on_worker(
            [reference.clone(), foreign.clone(), missing.clone()],
            Some(origin.clone()),
            Some(&roots),
        );
        let admitted = load_asset_path(&reference, Some(&origin), Some(&roots), Some(&prepared))
            .expect("canonical alias must remain in the exact mount");
        assert_eq!(
            admitted.path(),
            std::path::Path::new(&authority).join(file.file_name().expect("filename"))
        );
        assert_eq!(admitted.label(), None);
        assert!(load_asset_path(&foreign, Some(&origin), Some(&roots), Some(&prepared)).is_err());
        assert!(load_asset_path(&missing, Some(&origin), Some(&roots), Some(&prepared)).is_err());
        assert!(load_asset_path(&reference, Some(&origin), Some(&roots), None).is_err());
        roots.unregister_name(&authority).expect("retire mount");
        let replacement = roots.register("fixture", &root).expect("reopen mount");
        assert_ne!(replacement, authority);
        assert_eq!(
            load_asset_path(&reference, Some(&origin), Some(&roots), Some(&prepared)),
            Err(crate::TwinRootsError::UnknownAuthority(authority))
        );
        let next_origin =
            AssetPath::from_path_buf(std::path::Path::new(&replacement).join("scene.usda"))
                .with_source(crate::TWIN_SCHEME);
        assert!(
            load_asset_path(
                &reference,
                Some(&next_origin),
                Some(&roots),
                Some(&prepared)
            )
            .is_err()
        );
    }

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
        lunco_storage::ensure_directory_sync(&root.join("textures")).expect("texture folder");
        lunco_storage::write_file_sync(&root.join(&relative), b"texture payload")
            .expect("native fixture");
        lunco_storage::ensure_directory_sync(&root.join("buffers")).expect("buffer folder");
        lunco_storage::write_file_sync(&root.join("buffers/mesh.bin"), b"mesh payload")
            .expect("sibling fixture");
        let uri = lunco_storage::file_path_to_uri(&root.join(&relative)).expect("native URI");
        let path = prepared_load_asset_path(&uri, Some(&origin), Some(&roots))
            .expect("admitted native asset");
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
        let sibling = prepared_load_asset_path(&sibling, Some(&origin), Some(&roots))
            .expect("admitted sibling");
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
        assert!(prepared_load_asset_path(&uri, None, Some(&roots)).is_err());
        assert!(
            prepared_load_asset_path(
                &uri,
                Some(&AssetPath::parse("lunco://scene.usda")),
                Some(&roots)
            )
            .is_err()
        );
        assert_eq!(
            prepared_load_asset_path(&uri, Some(&origin), None),
            Err(crate::TwinRootsError::RegistryUnavailable)
        );
        assert!(prepared_load_asset_path(&foreign, Some(&origin), Some(&roots)).is_err());
        let unknown = AssetPath::parse("twin://unknown/scene.usda");
        assert_eq!(
            prepared_load_asset_path(&uri, Some(&unknown), Some(&roots)),
            Err(crate::TwinRootsError::UnknownAuthority("unknown".into()))
        );
        roots.unregister_name(&authority).expect("unmount");
        roots.register("fixture", folder.path()).expect("reopen");
        assert_eq!(
            prepared_load_asset_path(&uri, Some(&origin), Some(&roots)),
            Err(crate::TwinRootsError::UnknownAuthority(authority))
        );
    }

    #[test]
    fn source_load_paths_preserve_literal_filename_characters() {
        let origin =
            AssetPath::from_path_buf(std::path::PathBuf::from("fixture/# 100% 月/main.usda"))
                .with_source(crate::TWIN_SCHEME);
        let path = prepared_load_asset_path("textures/# %.png", Some(&origin), None)
            .expect("relative source path");
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
        let path = prepared_load_asset_path("textures/albedo.png", Some(&origin), None)
            .expect("library asset");
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
        lunco_storage::ensure_directory_sync(&root.join("textures")).expect("texture folder");
        lunco_storage::write_file_sync(&root.join("textures/# %.png"), b"texture payload")
            .expect("native fixture");
        let uri =
            lunco_storage::file_path_to_uri(&root.join("textures/# %.png")).expect("Windows URI");
        let origin = AssetPath::from_path_buf(std::path::Path::new(&authority).join("scene.usda"))
            .with_source(crate::TWIN_SCHEME);
        let path = prepared_load_asset_path(&uri, Some(&origin), Some(&roots))
            .expect("verbatim mount admission");
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
