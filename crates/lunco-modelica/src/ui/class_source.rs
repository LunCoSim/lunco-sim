//! Unified "qualified class name → source text" resolver.
//!
//! Single source-of-truth for *where a class's source lives*, across
//! every backend the workbench knows: the MSL / third-party index, the
//! filesystem library roots, already-open documents, and the embedded
//! bundled examples.
//!
//! Before this, each consumer that needed a class's source *by name*
//! reimplemented its own subset of those backends. The duplicate path
//! knew only the MSL index, so duplicating a bundled composite
//! (`AnnotatedRocketStage.RocketStage`) produced a "could not locate"
//! comment document that rendered as "(no classes yet)". Routing every
//! by-name source lookup through here means a new backend (or a fix
//! like the bundled fallback) lands once, for all consumers.
//!
//! Relationship to the path resolvers in [`crate::library_fs`]:
//! `resolve_class_path_indexed` / `locate_library_file` answer "which
//! *file*?" for the lazy drill-in / View flow (which never reads the
//! source eagerly — it opens a tab and streams the file in). This
//! module answers "what *source text*?" for flows that must read and
//! rewrite it (duplicate). Both sit on the same path building blocks so
//! they cannot disagree about a class's home.

use crate::ui::state::ModelicaDocumentRegistry;
use bevy::prelude::World;

/// A class's source text plus the metadata an extract/rewrite pass needs.
pub(crate) struct ResolvedClassSource {
    /// Full source of the file / document that contains the class.
    pub source: String,
    /// On-disk path the source was read from, when it came from a file
    /// backend (MSL, third-party, filesystem library). `None` for
    /// in-memory backends (open documents, bundled examples). Lets
    /// callers reuse the content-hash span cache via
    /// [`crate::document::duplicate::extract_class_spans_via_path`] and
    /// harvest enclosing-package imports with
    /// [`crate::document::duplicate::collect_parent_imports`].
    pub origin_path: Option<std::path::PathBuf>,
}

/// Find an open document whose parsed AST contains `qualified`.
///
/// Shared by [`resolve_class_source`] (to read the doc's source) and
/// [`crate::ui::panels::canvas_diagram::drill_into_class`] (to reuse the
/// doc's tab) so the "which open doc owns this class" rule lives in one
/// place instead of being copied between the two flows.
pub(crate) fn find_open_doc_with_class(
    world: &World,
    qualified: &str,
) -> Option<lunco_doc::DocumentId> {
    let registry = world.resource::<ModelicaDocumentRegistry>();
    registry.iter().find_map(|(doc_id, host)| {
        host.document().strict_ast().and_then(|ast| {
            crate::diagram::find_class_by_qualified_name(&ast, qualified).map(|_| doc_id)
        })
    })
}

/// Embedded bundled example source for a qualified name, keyed by its
/// head segment (`AnnotatedRocketStage.RocketStage` →
/// `AnnotatedRocketStage.mo`). Bundled files are named after their
/// top-level class. Pure (no `World`), so it's unit-testable.
pub(crate) fn bundled_source_for(qualified: &str) -> Option<&'static str> {
    let head = qualified.split('.').next().unwrap_or(qualified);
    crate::models::get_model(&format!("{head}.mo"))
}

/// Resolve `qualified` to its source text across all backends, in the
/// same priority order the drill-in path uses for *paths*:
/// indexed MSL/third-party → filesystem library roots → open documents
/// → bundled examples. Returns `None` only when no backend knows the
/// class.
pub(crate) fn resolve_class_source(
    world: &World,
    qualified: &str,
) -> Option<ResolvedClassSource> {
    // 1) MSL / third-party. The prebuilt palette index is the fast path;
    //    `locate_library_file` covers extra libraries not in that index.
    if let Some(path) = crate::library_fs::resolve_class_path_indexed(qualified)
        .or_else(|| crate::library_fs::locate_library_file(qualified))
    {
        if let Some(source) =
            lunco_assets::msl::msl_read(&path).and_then(|b| String::from_utf8(b).ok())
        {
            return Some(ResolvedClassSource {
                source,
                origin_path: Some(path),
            });
        }
    }

    // 2) An already-open document (user file, scratch model, prior
    //    drill-in). Covers non-library classes that live only in a
    //    workspace document.
    if let Some(doc) = find_open_doc_with_class(world, qualified) {
        if let Some(source) = world
            .resource::<ModelicaDocumentRegistry>()
            .host(doc)
            .map(|h| h.document().source_arc().to_string())
        {
            return Some(ResolvedClassSource {
                source,
                origin_path: None,
            });
        }
    }

    // 3) Embedded bundled example.
    bundled_source_for(qualified).map(|src| ResolvedClassSource {
        source: src.to_string(),
        origin_path: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_source_resolves_via_head_segment() {
        // The nested-class qualified name resolves to the file named
        // after its head segment — the path that fixes "(no classes
        // yet)" on a bundled composite duplicate.
        let src = bundled_source_for("AnnotatedRocketStage.RocketStage")
            .expect("AnnotatedRocketStage.mo must be bundled");
        assert!(src.contains("model RocketStage"), "expected nested model in bundled source");
    }

    #[test]
    fn bundled_source_resolves_flat_top_level() {
        // A flat bundled model (no dot) maps to `<name>.mo`.
        assert!(bundled_source_for("RC_Circuit").is_some());
    }

    #[test]
    fn bundled_source_unknown_is_none() {
        assert!(bundled_source_for("NoSuchBundledThing.Whatever").is_none());
    }
}
