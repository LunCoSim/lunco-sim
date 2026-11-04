//! Revision leases for the complete editor source unit and admitted libraries.

use bevy::prelude::*;
use lunco_doc_bevy::DocumentRegistry;
use lunco_modelica_document::ModelicaDocument;
use lunco_modelica_runtime::{ModelicaModel, ModelicaSourceSnapshot};
use lunco_modelica_source_roots::SourceRootRegistry;
use lunco_workspace::{PinnedDocumentRuntimeOwner, ReplicationOwner, Workspace};

/// Capture the source candidates supplied by document compilation. Recording
/// the candidate set also detects a newly opened dependency or a changed
/// first-wins declaration, before the compiler chooses its reachable subset.
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
