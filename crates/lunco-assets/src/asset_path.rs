//! Canonical asset-path resolution — the ONE place a reference becomes a path.
//!
//! Every subsystem that follows a reference from inside a loaded document hits the
//! same problem: the reference may be scheme-qualified (`lunco://…`, `twin://…`,
//! absolute-from-assets-root (`/…`), or relative to the document
//! that named it. Resolving it correctly is not obvious — a relative reference
//! inside a document that itself came from a `scheme://` source must STAY under
//! that source, and `Path` normalization silently collapses `scheme://` to
//! `scheme:/`, losing it.
//!
//! This logic was written five times (USD layer composition, texture resolution,
//! terrain, the sandbox's scene loader, the rhai module resolver). Divergence
//! between any two of them is a reference that loads in one subsystem and 404s in
//! another — the exact class of bug behind scripts that sync to a peer and then
//! fail to load there. So it lives here, in the crate that owns asset access, and
//! every caller delegates.
//!
//! Nothing here touches the filesystem or the `AssetServer`: it is pure string and
//! path algebra, which is what lets it be shared by an async loader and a
//! synchronous resolver alike.

use bevy::asset::{io::AssetSourceId, AssetPath};
use std::path::{Component, Path, PathBuf};

/// The anchor string for a document the `AssetServer` already knows about.
///
/// Bevy splits a loaded asset's identity into a source (`AssetSourceId`) and a
/// path; [`canonicalize`] wants the single `scheme://path` spelling those two
/// denote. Converting here — rather than at each call site — is what stops
/// subsystems from inventing their own scheme reassembly and disagreeing about
/// the `Default` source, which has no scheme prefix at all.
pub fn anchor_of(path: &AssetPath) -> String {
    let p = path.path().to_string_lossy();
    match path.source() {
        AssetSourceId::Name(name) => format!("{name}://{p}"),
        AssetSourceId::Default => p.into_owned(),
    }
}

/// Collapse `.` and `..` segments without touching the filesystem.
///
/// A leading `..` with nothing to pop is PRESERVED. `std::fs::canonicalize` cannot
/// be used here (the path need not exist, and may live behind a non-filesystem
/// asset source), and dropping an unmatched `..` would silently resolve a relative
/// anchor to the wrong directory rather than failing.
pub fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::ParentDir => {
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

/// Resolve `asset_path`, as named inside the document at `anchor`, to a stable
/// asset-source-relative identifier.
///
/// Three forms:
///   * `scheme://…` → passthrough; the Bevy `AssetServer` source handles it.
///   * `/…` (legacy absolute-from-assets-root) → strip the leading slash.
///   * relative → resolved against the anchor document's directory, KEEPING the
///     anchor's scheme.
///
/// The scheme is split off before any `Path` work and reattached after, because
/// `Path` normalization turns `scheme://a/b` into `scheme:/a/b` — which names a
/// different (nonexistent) source. That is the subtle failure this function
/// exists to prevent.
///
/// MUST stay identical between any pre-fetch pass and the resolver that consumes
/// its results — a pre-fetch keyed on one spelling and a lookup keyed on another
/// is a guaranteed cache miss (R-canon).
pub fn canonicalize(asset_path: &str, anchor: Option<&str>) -> String {
    if crate::has_scheme(asset_path) {
        return asset_path.to_string();
    }
    if let Some(rest) = asset_path.strip_prefix('/') {
        return normalize(Path::new(rest)).to_string_lossy().into_owned();
    }
    let anchor_str = anchor.unwrap_or_default();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scheme_qualified_passes_through() {
        assert_eq!(canonicalize("lunco://a/b.usda", None), "lunco://a/b.usda");
        // Even with an anchor — an absolute reference is absolute.
        assert_eq!(
            canonicalize("twin://ep1/lib.rhai", Some("lunco://scenes/x.usda")),
            "twin://ep1/lib.rhai"
        );
    }

    #[test]
    fn leading_slash_is_assets_root_relative() {
        assert_eq!(canonicalize("/scenes/x.usda", None), "scenes/x.usda");
    }

    /// The case the scheme-splitting exists for: a relative reference inside a
    /// scheme-sourced document must stay under that source.
    #[test]
    fn relative_keeps_the_anchors_scheme() {
        assert_eq!(
            canonicalize("../../components/wheel.usda", Some("lunco://vessels/rovers/skid.usda")),
            "lunco://components/wheel.usda"
        );
        assert_eq!(
            canonicalize("lib.rhai", Some("twin://ep1/main.rhai")),
            "twin://ep1/lib.rhai"
        );
    }

    #[test]
    fn unmatched_parent_dir_is_preserved() {
        assert_eq!(normalize(Path::new("../../a/b")), PathBuf::from("../../a/b"));
        assert_eq!(normalize(Path::new("a/./b/../c")), PathBuf::from("a/c"));
    }

    #[test]
    fn relative_without_anchor_is_bare() {
        assert_eq!(canonicalize("scenes/x.usda", None), "scenes/x.usda");
    }
}
