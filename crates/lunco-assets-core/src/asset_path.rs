//! Bevy asset-source adapters over the platform-neutral asset path algebra.

use bevy::asset::{io::AssetSourceId, AssetPath};

pub(crate) use lunco_assets_path::{
    canonicalize, canonicalize_root, has_scheme, is_anchored, is_safe_relative_components,
    is_safe_relative_path, normalize, relative_path, slashed, split_scheme, uri,
};

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
