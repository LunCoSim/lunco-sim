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
///   * `/…` (absolute-from-assets-root) → strip the leading slash.
///   * relative → resolved against the anchor document's directory, KEEPING the
///     anchor's scheme.
///
/// The scheme is split off before any `Path` work and reattached after, because
/// `Path` normalization turns `scheme://a/b` into `scheme:/a/b` — which names a
/// different (nonexistent) source. That is the subtle failure this function
/// exists to prevent.
///
/// The anchor is NOT optional. It used to be, defaulting to `""`, which meant a
/// relative reference with no anchor resolved against the *default* source rather
/// than the caller's root — silently, and differently from every subsystem that
/// did pass one. That is the precise "loads here, 404s there" split this module
/// exists to close, so a caller that has no anchoring document now has to say so
/// by calling [`canonicalize_root`] instead of passing `None`.
///
/// MUST stay identical between any pre-fetch pass and the resolver that consumes
/// its results — a pre-fetch keyed on one spelling and a lookup keyed on another
/// is a guaranteed cache miss (R-canon).
pub fn canonicalize(asset_path: &str, anchor: &str) -> String {
    if is_anchored(asset_path) {
        return canonicalize_root(asset_path);
    }
    let (scheme, anchor_path) = match split_scheme(anchor) {
        Some((s, rest)) => (Some(s), rest),
        None => (None, anchor),
    };
    let base = Path::new(anchor_path)
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    let resolved = normalize(&base.join(asset_path)).to_string_lossy().into_owned();
    match scheme {
        Some(s) => uri(s, &resolved),
        None => resolved,
    }
}

/// Canonicalize a reference that NAMES a root — no document anchors it: the scene
/// layer a stage is opened from, or a filesystem path handed in from outside.
///
/// This is the honest spelling of what passing `None` used to mean. A relative
/// reference here is assets-root-relative by definition rather than by accident,
/// which is what makes the distinction worth a second entry point: the two cases
/// are genuinely different questions, and collapsing them into one nullable
/// argument is what let callers ask the wrong one without noticing.
pub fn canonicalize_root(reference: &str) -> String {
    if crate::has_scheme(reference) {
        return reference.to_string();
    }
    let rel = reference.strip_prefix('/').unwrap_or(reference);
    normalize(Path::new(rel)).to_string_lossy().into_owned()
}

/// Split `scheme://rest` into its two halves, or `None` for a bare reference.
///
/// The ONE place `://` is spelled when taking a reference APART, as [`uri`] is
/// the one place it is spelled when putting one together. Every scheme used to
/// also carry a hand-written `"<name>://"` prefix constant beside its name — two
/// literals per scheme that had to agree, checked by nobody.
pub fn split_scheme(reference: &str) -> Option<(&str, &str)> {
    reference.split_once("://")
}

/// Build `scheme://rel`. The inverse of [`split_scheme`].
pub fn uri(scheme: &str, rel: &str) -> String {
    format!("{scheme}://{rel}")
}

/// Whether a reference resolves WITHOUT an anchor — either already addressable
/// (`scheme://…`) or already rooted at the assets root (`/…`).
///
/// These are exactly the two [`canonicalize`] branches that ignore their anchor,
/// so a caller deciding "does this one need anchoring?" must agree with them.
/// Named here so that agreement is structural rather than two `starts_with`
/// chains that have to be kept in step by hand.
pub fn is_anchored(reference: &str) -> bool {
    crate::has_scheme(reference) || reference.starts_with('/')
}

/// The same-origin URL a reference is fetched from on the web.
///
/// The engine library is staged next to the wasm bundle under
/// [`ASSETS_DIR_NAME`](crate::ASSETS_DIR_NAME), so a library-relative reference
/// has to be rooted there — and one that is ALREADY addressable must not be.
///
/// This is a *resolution* rule, so it lives here rather than in the subsystems
/// that fetch. Terrain open-coded it twice as
/// `starts_with("assets/") || starts_with("http") || starts_with('/')`, which
/// recognised no scheme but `http` — a `lunco://` DEM silently became
/// `assets/lunco://…` and 404'd, web-only, invisible from the native path.
pub fn web_url(reference: &str) -> String {
    let raw = reference.replace('\\', "/");
    // Absolute — server-rooted or a full URL. Nothing to anchor.
    if raw.starts_with('/') || raw.starts_with("http://") || raw.starts_with("https://") {
        return raw;
    }
    // `lunco://x` names the shipped library, which IS this directory: reduce it to
    // its library-relative form rather than rooting a URI. Any OTHER scheme has a
    // root this same-origin mount cannot serve, so it passes through untouched.
    let rel = crate::engine_asset_rel(&raw);
    if crate::has_scheme(rel) {
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
    fn scheme_qualified_passes_through() {
        assert_eq!(canonicalize_root("lunco://a/b.usda"), "lunco://a/b.usda");
        // Even with an anchor — an absolute reference is absolute.
        assert_eq!(
            canonicalize("twin://ep1/lib.rhai", "lunco://scenes/x.usda"),
            "twin://ep1/lib.rhai"
        );
    }

    #[test]
    fn leading_slash_is_assets_root_relative() {
        assert_eq!(canonicalize_root("/scenes/x.usda"), "scenes/x.usda");
    }

    /// The case the scheme-splitting exists for: a relative reference inside a
    /// scheme-sourced document must stay under that source.
    #[test]
    fn relative_keeps_the_anchors_scheme() {
        assert_eq!(
            canonicalize("../../components/wheel.usda", "lunco://vessels/rovers/skid.usda"),
            "lunco://components/wheel.usda"
        );
        assert_eq!(
            canonicalize("lib.rhai", "twin://ep1/main.rhai"),
            "twin://ep1/lib.rhai"
        );
    }

    #[test]
    fn unmatched_parent_dir_is_preserved() {
        assert_eq!(normalize(Path::new("../../a/b")), PathBuf::from("../../a/b"));
        assert_eq!(normalize(Path::new("a/./b/../c")), PathBuf::from("a/c"));
    }

    /// The regression the terrain copies had: only `http` was recognised, so every
    /// other scheme got the library root prepended to a URI.
    #[test]
    fn web_url_roots_library_paths_and_passes_addressable_ones_through() {
        assert_eq!(web_url("dem/site.tif"), "assets/dem/site.tif");
        // Already rooted — must not be doubled.
        assert_eq!(web_url("assets/dem/site.tif"), "assets/dem/site.tif");
        // `lunco://` IS the library mount: reduced, not rooted.
        assert_eq!(web_url("lunco://dem/site.tif"), "assets/dem/site.tif");
        assert_eq!(web_url("https://h/x.tif"), "https://h/x.tif");
        assert_eq!(web_url("/abs/x.tif"), "/abs/x.tif");
        // Another scheme's root — this mount cannot serve it, so it is untouched
        // rather than turned into `assets/twin://…`.
        assert_eq!(web_url("twin://ep1/x.tif"), "twin://ep1/x.tif");
        assert_eq!(web_url("dem\\site.tif"), "assets/dem/site.tif");
    }

    #[test]
    fn anchored_matches_the_branches_canonicalize_ignores_its_anchor_on() {
        for r in ["lunco://a.usda", "twin://e/a.usda", "/a.usda"] {
            assert!(is_anchored(r), "{r} should be anchored");
            // An anchored reference means the same thing however it is asked.
            assert_eq!(canonicalize(r, "lunco://other/x.usda"), canonicalize_root(r));
        }
        assert!(!is_anchored("a.usda"));
    }

    /// A root reference is assets-root-relative BY DECLARATION. Previously this
    /// was what `canonicalize(_, None)` did by accident, so a caller that simply
    /// had no anchor to hand got this silently — now it has to say so.
    #[test]
    fn a_relative_root_reference_is_assets_root_relative() {
        assert_eq!(canonicalize_root("scenes/x.usda"), "scenes/x.usda");
        assert_eq!(canonicalize_root("a/./b/../c.usda"), "a/c.usda");
    }
}
