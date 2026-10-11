//! Unified read-side view of "what does the workbench know about
//! this class?".
//!
//! Both the pre-baked source library palette index ([`lunco_modelica_index::index::ClassEntry`]) and
//! the live per-document index ([`ClassEntry`]) carry the same
//! conceptual fields — kind, description, documentation, icon —
//! shaped differently because one is a serialised palette payload
//! and the other a runtime AST projection. Consumers that only
//! need the read-side view (badges, docs panel, tree labels,
//! inspector title) shouldn't have to branch on which source
//! produced the data.
//!
//! [`ClassMetadata`] is the common projection. Source libraries use their
//! palette index; workspace documents use their parsed declaration and index.
//! Names retain their exact authored package identity.

use bevy::prelude::World;

use crate::class_ref::{ClassRef, Library};
use crate::sim_default::ResourceRead;
use lunco_modelica_ast::annotations::Icon;
use lunco_modelica_index::index::{ClassEntry, ClassKind};

/// Read-side metadata for a class, regardless of where the source
/// of truth lives. Keep this minimal — it should *not* grow into
/// every field of both backends. The projector keeps its own
/// `lunco_modelica_index::index::ClassEntry` lookup for ports / parameters / graphics;
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
/// (e.g. a source-library drill before the indexer ran, or a workspace doc
/// whose async parse hasn't landed).
pub fn resolve_metadata(world: &World, class: &ClassRef) -> Option<ClassMetadata> {
    match &class.library {
        Library::Source { .. } | Library::Bundled => {
            // 1. Pre-baked palette index — `library_index.json` covers
            //    every indexed class with absolute qualified names.
            let qualified = class.qualified();
            if let Some(def) =
                lunco_modelica_index::visual_diagram::library_class_by_path(&qualified)
            {
                return Some(ClassMetadata::from(&def));
            }
            // An open backing document supplies current metadata before its
            // palette projection is available.
            workspace_doc_metadata(world, class)
        }
        Library::UserFile { .. } | Library::Untitled(_) => workspace_doc_metadata(world, class),
    }
}

/// Resolve an exact class inside its owning document. An omitted name uses
/// the document's shared simulation-target selection.
pub fn resolve_metadata_for_doc(
    world: &World,
    doc_id: lunco_doc::DocumentId,
    drilled: Option<&str>,
) -> Option<ClassMetadata> {
    resolve_metadata_for_doc_in(world, doc_id, drilled)
}

/// Context-generic form of [`resolve_metadata_for_doc`]. UI adapters use this
/// at their boundary instead of adding workbench dependencies to the core.
pub fn resolve_metadata_for_doc_in<R: ResourceRead>(
    ctx: &R,
    doc_id: lunco_doc::DocumentId,
    drilled: Option<&str>,
) -> Option<ClassMetadata> {
    let registry = ctx.read_resource::<lunco_doc_bevy::DocumentRegistry<lunco_modelica_document::ModelicaDocument>>()?;
    let host = registry.host(doc_id)?;
    resolve_document_metadata(host.document(), drilled)
}

/// Project only the declaration addressed by the document's authored identity.
fn resolve_document_metadata(
    document: &lunco_modelica_document::ModelicaDocument,
    requested: Option<&str>,
) -> Option<ClassMetadata> {
    let selected = match requested {
        Some(name) => name.to_owned(),
        None => crate::sim_target::default_class(None, &document.index().simulation_candidates())?,
    };
    let ast = document.syntax().ast();
    ast.get_class_by_qualified_name(&selected)?;
    let relative = ast.relative_class_name(&selected);
    let entry = document.index().classes.get(relative)?;
    let mut metadata = ClassMetadata::from(entry);
    metadata.qualified = match document
        .index()
        .within_path
        .as_deref()
        .filter(|s| !s.is_empty())
    {
        Some(within) => format!("{within}.{relative}"),
        None => relative.to_string(),
    };
    Some(metadata)
}

/// Resolve the owning workspace document, or a unique open source declaration.
fn workspace_doc_metadata(world: &World, class: &ClassRef) -> Option<ClassMetadata> {
    let registry = world.get_resource::<lunco_doc_bevy::DocumentRegistry<lunco_modelica_document::ModelicaDocument>>()?;
    let qualified = class.qualified();
    let target_doc = match &class.library {
        Library::UserFile { path } => Some(registry.find_by_path(path)?),
        Library::Untitled(doc_id) => Some(*doc_id),
        Library::Source { .. } | Library::Bundled => None,
    };
    if let Some(doc_id) = target_doc {
        let host = registry.host(doc_id)?;
        return resolve_document_metadata(host.document(), Some(&qualified));
    }
    let mut matched = None;
    for (_doc_id, host) in registry.iter() {
        if let Some(metadata) = resolve_document_metadata(host.document(), Some(&qualified)) {
            if metadata.qualified != qualified {
                continue;
            }
            if matched.is_some() {
                return None;
            }
            matched = Some(metadata);
        }
    }
    matched
}
