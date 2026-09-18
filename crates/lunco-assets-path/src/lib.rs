//! Platform-neutral URI and relative-path algebra for LunCoSim assets.
//!
//! This crate owns the canonical spelling and traversal rules shared by USD
//! composition, document authoring, Twin resolution, and script imports. It
//! has no filesystem, Bevy, storage, or application dependency, so headless
//! document packages can use the same rules without inheriting an asset source.

use std::path::{Component, Path, PathBuf};

/// Whether a reference contains an explicit asset scheme.
pub fn has_scheme(reference: impl AsRef<str>) -> bool {
    split_scheme(reference.as_ref()).is_some()
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

/// A path's string form with `\` normalized to `/`.
///
/// Asset identities and URIs are forward-slash strings on every peer; a
/// Windows-built `Path` renders with `\`, so any path that becomes an identity
/// must pass through here.
pub fn slashed(p: impl AsRef<Path>) -> String {
    p.as_ref().to_string_lossy().replace('\\', "/")
}

/// Turn a URI-relative path into a native relative path, or reject it.
///
/// Asset identities use `/` on every platform. Normalize separator-neutral
/// authored input before creating a native [`Path`], so the same asset identity
/// names the same file on Windows, macOS, and Linux. URI paths never name a
/// volume or an absolute path; refusing those forms here also prevents a
/// source-relative asset from escaping its registered root.
pub fn relative_path(reference: &str) -> Option<PathBuf> {
    let reference = slashed(reference);
    if reference.is_empty()
        || reference.starts_with('/')
        || reference
            .as_bytes()
            .get(1)
            .is_some_and(|colon| *colon == b':')
    {
        return None;
    }

    let mut path = PathBuf::new();
    for segment in reference.split('/') {
        match segment {
            "" | "." => {}
            ".." => return None,
            // `:` is a volume separator on Windows. Refusing it in a logical
            // URI component keeps the identity portable to every supported OS.
            segment if segment.contains(':') || segment.contains('\0') => return None,
            segment => path.push(segment),
        }
    }
    (!path.as_os_str().is_empty()).then_some(path)
}

/// Whether `rel` is safe to join to an owned root directory.
///
/// Asset references that cross a root are scheme-qualified. A root-relative
/// path therefore has to be strictly relative and must not contain a segment
/// whose meaning changes when it is joined on another platform. Keep this
/// check at the asset boundary so Twin readers, downloads, and tutorial
/// sources cannot disagree about traversal.
pub fn is_safe_relative_path(rel: &str) -> bool {
    if rel.is_empty() || rel.contains('\\') {
        return false;
    }
    // Inspect the spelling directly: Path::components can hide the `..` that
    // this boundary must reject, and URI identities use `/` on every platform.
    if rel
        .split('/')
        .any(|segment| segment.is_empty() || segment == "." || segment == "..")
    {
        return false;
    }
    // Reject absolute paths on the host and Windows drive paths even when the
    // current host is Unix, where `C:/...` is not considered absolute.
    if Path::new(rel).is_absolute() {
        return false;
    }
    let bytes = rel.as_bytes();
    !(bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':')
}

/// Whether a path contains only ordinary relative components.
///
/// This is the `Path` counterpart to [`is_safe_relative_path`]. It is used at
/// async reader boundaries, after a URI has already become a platform path,
/// where the platform's own component parser is the authority for roots,
/// prefixes, and parent traversal.
pub fn is_safe_relative_components(path: &Path) -> bool {
    path.components()
        .all(|component| matches!(component, Component::Normal(_)))
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
    let asset_path = slashed(asset_path);
    if is_anchored(&asset_path) {
        return canonicalize_root(&asset_path);
    }
    let (scheme, anchor_path) = match split_scheme(anchor) {
        Some((s, rest)) => (Some(s), rest),
        None => (None, anchor),
    };
    let base = Path::new(&slashed(anchor_path))
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    let resolved = normalize(&base.join(&asset_path))
        .to_string_lossy()
        .into_owned();
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
    let reference = slashed(reference);
    if has_scheme(&reference) {
        return reference;
    }
    let rel = reference.strip_prefix('/').unwrap_or(&reference);
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
    has_scheme(reference) || reference.starts_with('/')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scheme_qualified_passes_through() {
        assert_eq!(canonicalize_root("lunco://a/b.usda"), "lunco://a/b.usda");
        assert_eq!(
            canonicalize_root(r"twin://fixture\sim\scenes\traverse.usda"),
            "twin://fixture/sim/scenes/traverse.usda"
        );
        assert_eq!(
            canonicalize("twin://ep1/lib.rhai", "lunco://scenes/x.usda"),
            "twin://ep1/lib.rhai"
        );
    }

    #[test]
    fn relative_keeps_the_anchors_scheme() {
        assert_eq!(
            canonicalize(
                "../../components/wheel.usda",
                "lunco://vessels/rovers/skid.usda"
            ),
            "lunco://components/wheel.usda"
        );
        assert_eq!(
            canonicalize(
                r"..\\components\\wheel.usda",
                r"twin://vessels\\rovers\\skid.usda"
            ),
            "twin://vessels/components/wheel.usda"
        );
        assert_eq!(
            canonicalize("lib.rhai", "twin://ep1/main.rhai"),
            "twin://ep1/lib.rhai"
        );
    }

    #[test]
    fn unmatched_parent_dir_is_preserved() {
        assert_eq!(
            normalize(Path::new("../../a/b")),
            PathBuf::from("../../a/b")
        );
        assert_eq!(normalize(Path::new("a/./b/../c")), PathBuf::from("a/c"));
    }

    #[test]
    fn uri_relative_paths_are_separator_neutral_and_cannot_escape_a_root() {
        assert_eq!(
            relative_path(r"sim\\scenes\\traverse.usda"),
            Some(PathBuf::from("sim").join("scenes").join("traverse.usda"))
        );
        for invalid in [
            "../scene.usda",
            "/scene.usda",
            r"C:\\scene.usda",
            "a/../../b",
        ] {
            assert_eq!(relative_path(invalid), None, "{invalid} must be rejected");
        }
    }

    #[test]
    fn anchored_matches_canonicalize_branches() {
        for reference in ["lunco://a.usda", "twin://e/a.usda", "/a.usda"] {
            assert!(is_anchored(reference));
            assert_eq!(
                canonicalize(reference, "lunco://other/x.usda"),
                canonicalize_root(reference)
            );
        }
        assert!(!is_anchored("a.usda"));
    }

    #[test]
    fn root_relative_paths_reject_traversal_and_absolute_spellings() {
        for path in [
            "tutorials/basic/lesson.rhai",
            "terrain/apollo15/.cache/dtm.tif",
        ] {
            assert!(is_safe_relative_path(path));
        }
        for path in [
            "",
            ".",
            "..",
            "../outside.rhai",
            "tutorials/../../outside.rhai",
            "/etc/passwd",
            "C:/Users/user/secret.rhai",
            r"tutorials\\..\\outside.rhai",
            "tutorials//lesson.rhai",
        ] {
            assert!(!is_safe_relative_path(path));
        }
    }
}
