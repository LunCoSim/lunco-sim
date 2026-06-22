//! Unified read-side view of "what does the workbench know about
//! this class?".
//!
//! Both the pre-baked MSL palette index ([`crate::index::ClassEntry`]) and
//! the live per-document index ([`ClassEntry`]) carry the same
//! conceptual fields — kind, description, documentation, icon —
//! shaped differently because one is a serialised palette payload
//! and the other a runtime AST projection. Consumers that only
//! need the read-side view (badges, docs panel, tree labels,
//! inspector title) shouldn't have to branch on which source
//! produced the data.
//!
//! [`ClassMetadata`] is the common shape, [`resolve_metadata`] is
//! the single lookup function. Dispatch is on [`Library`]: system
//! libraries (MSL, third-party, bundled) consult the pre-baked
//! palette index first; workspace docs (user files, untitled) read
//! the per-doc [`ModelicaIndex`]. The within-prefix mismatch that
//! caused the historical "(no documentation)" bug becomes
//! impossible: [`ClassRef::path`] is always within-library, the
//! per-doc index keys classes the same way, and the palette index
//! stores absolute names that match [`ClassRef::qualified`].

use bevy::prelude::World;

use crate::annotations::Icon;
use crate::class_ref::{ClassRef, Library};
use crate::index::{ClassEntry, ClassKind};

/// Read-side metadata for a class, regardless of where the source
/// of truth lives. Keep this minimal — it should *not* grow into
/// every field of both backends. The projector keeps its own
/// `crate::index::ClassEntry` lookup for ports / parameters / graphics;
/// callers that only want the display fields use this.
#[derive(Clone, Debug)]
pub struct ClassMetadata {
    /// Fully-qualified absolute name (`"Modelica.Blocks.Examples.PID_Controller"`).
    pub qualified: String,
    /// Modelica class kind.
    pub kind: ClassKind,
    /// Short description string from the class header
    /// (`model X "description"`). Empty when none was authored.
    pub description: String,
    /// `(info, revisions)` from the class's `Documentation(...)`
    /// annotation. Both `None` when no documentation was authored.
    /// Pre-baked metadata may only carry `info`.
    pub documentation: (Option<String>, Option<String>),
    /// Authored Icon annotation, if present.
    pub icon: Option<Icon>,
}

impl From<&ClassEntry> for ClassMetadata {
    fn from(e: &ClassEntry) -> Self {
        Self {
            qualified: e.name.clone(),
            kind: e.kind,
            description: e.description.clone(),
            documentation: e.documentation.clone(),
            icon: e.icon.clone(),
        }
    }
}

/// Resolve metadata for `class` from whichever backend owns it.
/// Returns `None` only when no backend has heard of the class yet
/// (e.g. an MSL drill before the indexer ran, or a workspace doc
/// whose async parse hasn't landed).
pub fn resolve_metadata(world: &World, class: &ClassRef) -> Option<ClassMetadata> {
    match &class.library {
        Library::Msl | Library::ThirdParty { .. } | Library::Bundled => {
            // 1. Pre-baked palette index — `msl_index.json` covers
            //    every indexed class with absolute qualified names.
            let qualified = class.qualified();
            if let Some(def) = crate::visual_diagram::msl_class_by_path(&qualified) {
                return Some(ClassMetadata::from(&def));
            }
            // 2. Fallback: if the user has the owning doc open
            //    (Bundled drill-in opens a Doc), consult its live
            //    index by the within-relative name.
            workspace_doc_metadata(world, class)
        }
        Library::UserFile { .. } | Library::Untitled(_) => {
            workspace_doc_metadata(world, class)
        }
    }
}

/// String-keyed convenience for call sites that don't yet build a
/// [`ClassRef`]: look up metadata for the class named `drilled`
/// inside document `doc_id`. When `drilled` is `None`, falls back
/// to the first non-package class declared in the document.
///
/// This is the lookup path the docs panel uses — its drilled string
/// may be an absolute MSL-rooted name (`"Modelica.Blocks.Examples.PID_Controller"`)
/// while the doc's index keys are within-relative
/// (`"Blocks.Examples.PID_Controller"`). The implementation tries
/// the verbatim key first, then progressively strips leading
/// segments, then suffix-matches the leaf — same handling
/// [`workspace_doc_metadata`] does, but driven from an explicit
/// document id instead of a [`ClassRef`].
pub fn resolve_metadata_for_doc(
    world: &World,
    doc_id: lunco_doc::DocumentId,
    drilled: Option<&str>,
) -> Option<ClassMetadata> {
    let registry = world
        .get_resource::<crate::state::ModelicaDocumentRegistry>()?;
    let host = registry.host(doc_id)?;
    let index = host.document().index();
    if let Some(q) = drilled {
        let mut found = index.classes.get(q);
        if found.is_none() {
            let mut remainder = q;
            while let Some((_, rest)) = remainder.split_once('.') {
                if let Some(e) = index.classes.get(rest) {
                    found = Some(e);
                    break;
                }
                remainder = rest;
            }
        }
        if found.is_none() {
            let leaf = crate::ast_extract::short_name(q);
            found = index
                .classes
                .iter()
                .find(|(k, _)| {
                    k.as_str() == leaf || k.ends_with(&format!(".{leaf}"))
                })
                .map(|(_, v)| v);
        }
        if let Some(entry) = found {
            return Some(ClassMetadata::from(entry));
        }
    }
    // Fallback: first non-package class.
    let fallback = index
        .classes
        .values()
        .find(|c| !matches!(c.kind, ClassKind::Package))
        .or_else(|| index.classes.values().next())?;
    Some(ClassMetadata::from(fallback))
}

/// Search the document registry for a class matching `class`'s
/// within-relative path. Used by [`resolve_metadata`]'s
/// non-pre-baked branch and as a fallback for system libraries
/// whose backing doc happens to be open.
fn workspace_doc_metadata(world: &World, class: &ClassRef) -> Option<ClassMetadata> {
    let registry = world
        .get_resource::<crate::state::ModelicaDocumentRegistry>()?;
    let target_doc = match &class.library {
        Library::UserFile { path } => registry.find_by_path(path),
        Library::Untitled(doc_id) => Some(*doc_id),
        _ => None,
    };
    let qualified = class.path.join(".");
    let leaf = class.path.last().cloned().unwrap_or_default();

    // Strategy: when we know the owning doc (UserFile/Untitled),
    // hit its index directly. Otherwise scan every open doc — the
    // first whose index contains the within-relative name wins.
    let metadata_from = |entry: &ClassEntry| ClassMetadata::from(entry);
    if let Some(doc_id) = target_doc {
        if let Some(host) = registry.host(doc_id) {
            let index = host.document().index();
            let hit = index
                .classes
                .get(&qualified)
                .or_else(|| index.classes.get(leaf.as_str()))
                .or_else(|| {
                    index
                        .classes
                        .iter()
                        .find(|(k, _)| {
                            k.as_str() == leaf.as_str()
                                || k.ends_with(&format!(".{}", leaf))
                        })
                        .map(|(_, v)| v)
                });
            return hit.map(metadata_from);
        }
    }
    for (_doc_id, host) in registry.iter() {
        let index = host.document().index();
        if let Some(entry) = index.classes.get(&qualified) {
            return Some(metadata_from(entry));
        }
        if let Some(entry) = index
            .classes
            .iter()
            .find(|(k, _)| {
                k.as_str() == leaf.as_str() || k.ends_with(&format!(".{}", leaf))
            })
            .map(|(_, v)| v)
        {
            return Some(metadata_from(entry));
        }
    }
    None
}
