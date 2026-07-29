//! Transitive file traversal through the native asset storage boundary.
//!
//! This module owns queueing, canonical paths, and native byte access. Format
//! crates provide the two pieces of format knowledge: which files are
//! traversable documents and how a document declares its dependencies.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::asset_path;

/// Walk every file transitively named by `roots`.
///
/// `is_document` decides whether a file may contain further dependencies;
/// `dependencies` receives its UTF-8 source and returns those raw references.
/// Anchored references are intentionally skipped because this form has no
/// mapping for a scheme or an assets-root-relative path.
#[cfg(not(target_arch = "wasm32"))]
pub fn transitive_file_closure(
    roots: &[PathBuf],
    is_document: impl Fn(&Path) -> bool,
    dependencies: impl Fn(&str) -> Option<Vec<String>>,
) -> BTreeSet<PathBuf> {
    transitive_file_closure_with(roots, |_| None, is_document, dependencies)
}

/// [`transitive_file_closure`] with resolution for anchored references.
///
/// The resolver is given the unmodified reference and may return a local file
/// for it. Returning `None` leaves that dependency out of the local closure.
#[cfg(not(target_arch = "wasm32"))]
pub fn transitive_file_closure_with(
    roots: &[PathBuf],
    resolve_anchored: impl Fn(&str) -> Option<PathBuf>,
    is_document: impl Fn(&Path) -> bool,
    dependencies: impl Fn(&str) -> Option<Vec<String>>,
) -> BTreeSet<PathBuf> {
    let mut seen = BTreeSet::new();
    let mut queue: Vec<PathBuf> = roots
        .iter()
        .map(|path| asset_path::normalize(path))
        .collect();

    while let Some(path) = queue.pop() {
        if !seen.insert(path.clone()) || !is_document(&path) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Some(arcs) = dependencies(&text) else {
            continue;
        };
        let base = path.parent().map(Path::to_path_buf).unwrap_or_default();
        for arc in arcs {
            if asset_path::is_anchored(&arc) {
                if let Some(resolved) = resolve_anchored(&arc) {
                    queue.push(asset_path::normalize(&resolved));
                }
            } else {
                queue.push(asset_path::normalize(&base.join(arc)));
            }
        }
    }
    seen
}
