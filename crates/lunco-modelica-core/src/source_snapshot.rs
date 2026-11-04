//! Revision leases for the complete editor source unit and admitted libraries.

use bevy::prelude::*;
use lunco_doc_bevy::DocumentRegistry;
use lunco_modelica_document::ModelicaDocument;
use lunco_modelica_runtime::{ModelicaModel, ModelicaSourceSnapshot};
use lunco_modelica_source_roots::SourceRootRegistry;
use lunco_workspace::{PinnedDocumentRuntimeOwner, ReplicationOwner, Workspace};

/// Capture the source candidates supplied by document compilation. Recording
/// the candidate set also detects a newly opened dependency or a changed
/// qualified declaration, before the compiler chooses its reachable subset.
pub fn capture(
    documents: &DocumentRegistry<ModelicaDocument>,
    primary: &PinnedDocumentRuntimeOwner,
    workspace: Option<&Workspace>,
    replication: Option<&ReplicationOwner>,
    roots: Option<&SourceRootRegistry>,
) -> Result<ModelicaSourceSnapshot, String> {
    if !primary.is_current(workspace, replication) {
        return Err("Modelica source runtime has retired".into());
    }
    let document = documents
        .host(primary.document)
        .ok_or("Modelica primary source document is closed")?
        .document();
    let generated =
        lunco_modelica_runtime::generated_source::is_generated_origin(document.origin());
    let mut revisions = Vec::new();
    for (id, host) in documents.docs() {
        if id != primary.document
            && (generated
                || !primary.shares_runtime_with(id, workspace, replication)
                || lunco_modelica_source_roots::is_library_document(host.document()))
        {
            continue;
        }
        let owner = PinnedDocumentRuntimeOwner::for_document(id, workspace)?;
        if !owner.is_current(workspace, replication) {
            return Err(format!("Modelica source document {id} has retired"));
        }
        revisions.push((
            owner,
            host.document().generation_owned(),
            host.document().origin().session_uri(),
        ));
    }
    revisions.sort_unstable_by_key(|(owner, _, _)| owner.document.raw());
    let mut source_roots: Vec<_> = roots
        .into_iter()
        .flat_map(|roots| roots.roots.values())
        // Document namespace projections have operation zero until a worker
        // load is issued. Overlaid source bytes use the document leases above.
        .filter(|root| root.origin_document.is_none() || root.operation_id != 0)
        .map(|root| (root.id.clone(), root.operation_id))
        .collect();
    source_roots.sort_unstable();
    Ok(ModelicaSourceSnapshot {
        documents: revisions,
        source_roots,
    })
}

/// Validate every exact source lease without reading or parsing asset bytes.
/// Disk/library changes enter through the existing source-root operation owner.
pub fn is_current(
    snapshot: &ModelicaSourceSnapshot,
    documents: &DocumentRegistry<ModelicaDocument>,
    primary: &PinnedDocumentRuntimeOwner,
    workspace: Option<&Workspace>,
    replication: Option<&ReplicationOwner>,
    roots: Option<&SourceRootRegistry>,
) -> bool {
    capture(documents, primary, workspace, replication, roots)
        .is_ok_and(|current| current == *snapshot)
}

/// Shared freshness used by compile reuse, Run and public status queries.
pub fn model_is_current(world: &World, model: &ModelicaModel) -> bool {
    let Some(snapshot) = model.source_snapshot.as_ref() else {
        return false;
    };
    let Some(documents) = world.get_resource::<DocumentRegistry<ModelicaDocument>>() else {
        return false;
    };
    let workspace = world
        .get_resource::<lunco_workspace::WorkspaceResource>()
        .map(|w| &w.0);
    let replication = lunco_core_session::current_replication_owner_in(world);
    let Ok(primary) = PinnedDocumentRuntimeOwner::for_document(model.document, workspace) else {
        return false;
    };
    is_current(
        snapshot,
        documents,
        &primary,
        workspace,
        replication.as_ref(),
        world.get_resource::<SourceRootRegistry>(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use lunco_modelica_document::ModelicaOp;

    #[test]
    fn document_namespace_roots_are_not_library_operations() {
        for generated in [false, true] {
            let mut documents = DocumentRegistry::<ModelicaDocument>::default();
            let primary = if generated {
                documents.allocate(
                    "model Main end Main;".into(),
                    lunco_doc::PathlessOrigin::bundled("generated/Main.mo"),
                )
            } else {
                documents.allocate_untitled("model Main end Main;".into())
            };
            let library = documents.allocate_untitled("package LocalLib end LocalLib;".into());
            documents
                .host_mut(library)
                .unwrap()
                .document_mut()
                .set_origin(lunco_doc::DocumentOrigin::writable_file("local/package.mo"));
            let source = PinnedDocumentRuntimeOwner::for_document(primary, None).unwrap();
            let mut roots = SourceRootRegistry::default();
            roots.register_disk_root("Library", std::path::PathBuf::from("library"));
            let snapshot = capture(&documents, &source, None, None, Some(&roots)).unwrap();
            let namespace = roots
                .register_open_doc_root(
                    "LocalLib".into(),
                    Some(std::path::PathBuf::from("local/package.mo")),
                    library,
                    lunco_modelica_source_roots::SourceRootOwner::Application,
                )
                .unwrap();
            assert!(
                is_current(&snapshot, &documents, &source, None, None, Some(&roots)),
                "publishing namespace facts does not change admitted source bytes or library operations"
            );
            roots.roots.remove(&namespace);
            assert!(
                is_current(&snapshot, &documents, &source, None, None, Some(&roots)),
                "retiring namespace metadata does not retire its still-open source document"
            );
            documents
                .host_mut(library)
                .unwrap()
                .apply(ModelicaOp::ReplaceSource {
                    new: "package LocalLib constant Real gain=2; end LocalLib;".into(),
                })
                .unwrap();
            assert_eq!(
                is_current(&snapshot, &documents, &source, None, None, Some(&roots)),
                generated,
                "editor document libraries remain source leases; generated units have no editor sibling overlays"
            );
            let snapshot = capture(&documents, &source, None, None, Some(&roots)).unwrap();
            let namespace = roots
                .register_open_doc_root(
                    "LocalLib".into(),
                    Some(std::path::PathBuf::from("local/package.mo")),
                    library,
                    lunco_modelica_source_roots::SourceRootOwner::Application,
                )
                .unwrap();
            roots.roots.get_mut(&namespace).unwrap().operation_id = 1;
            assert!(
                !is_current(&snapshot, &documents, &source, None, None, Some(&roots)),
                "a document-backed library with a real worker operation is strictly fenced"
            );
            let snapshot = capture(&documents, &source, None, None, Some(&roots)).unwrap();
            roots.roots.get_mut(&namespace).unwrap().operation_id = 2;
            assert!(
                !is_current(&snapshot, &documents, &source, None, None, Some(&roots)),
                "a replacement document-backed worker operation invalidates the admitted snapshot"
            );
            let snapshot = capture(&documents, &source, None, None, Some(&roots)).unwrap();
            roots.roots.get_mut("Library").unwrap().operation_id = 3;
            assert!(
                !is_current(&snapshot, &documents, &source, None, None, Some(&roots)),
                "actual library operations still fence both generated and editor compilation"
            );
        }
    }

    #[test]
    fn source_leases_track_sibling_revisions_retirement_and_root_operations() {
        let mut documents = DocumentRegistry::<ModelicaDocument>::default();
        let main = documents.allocate_untitled("model Main end Main;".into());
        let helper = documents.allocate_untitled("model Helper end Helper;".into());
        let source = PinnedDocumentRuntimeOwner::for_document(main, None).unwrap();
        let mut roots = SourceRootRegistry::default();
        roots.register_disk_root("Library", std::path::PathBuf::from("library"));
        let snapshot = capture(&documents, &source, None, None, Some(&roots)).unwrap();
        assert!(is_current(
            &snapshot,
            &documents,
            &source,
            None,
            None,
            Some(&roots)
        ));
        let generation = documents.host(main).unwrap().document().generation_owned();
        documents.host_mut(main).unwrap().document_mut().set_origin(
            lunco_doc::DocumentOrigin::File {
                path: std::path::PathBuf::from("saved/main.mo"),
                writable: true,
            },
        );
        assert_eq!(
            documents.host(main).unwrap().document().generation_owned(),
            generation,
            "SaveAs changes source identity independently of source bytes"
        );
        assert!(
            !is_current(&snapshot, &documents, &source, None, None, Some(&roots)),
            "the old compiler URI cannot authorize reuse after relocation"
        );
        let snapshot = capture(&documents, &source, None, None, Some(&roots)).unwrap();
        documents
            .host_mut(helper)
            .unwrap()
            .apply(ModelicaOp::ReplaceSource {
                new: "model Helper parameter Real gain=2; end Helper;".into(),
            })
            .unwrap();
        assert!(!is_current(
            &snapshot,
            &documents,
            &source,
            None,
            None,
            Some(&roots)
        ));
        let snapshot = capture(&documents, &source, None, None, Some(&roots)).unwrap();
        roots.roots.get_mut("Library").unwrap().operation_id += 1;
        assert!(!is_current(
            &snapshot,
            &documents,
            &source,
            None,
            None,
            Some(&roots)
        ));
        let snapshot = capture(&documents, &source, None, None, Some(&roots)).unwrap();
        documents.remove_document(helper);
        assert!(!is_current(
            &snapshot,
            &documents,
            &source,
            None,
            None,
            Some(&roots)
        ));
        documents.remove_document(main);
        assert!(capture(&documents, &source, None, None, Some(&roots)).is_err());
    }
}
