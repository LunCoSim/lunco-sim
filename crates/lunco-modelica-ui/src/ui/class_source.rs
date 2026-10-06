//! Immutable admission and worker resolution of qualified class source.
//!
//! The caller snapshots resident source and its exact lifetime without I/O.
//! The duplicate task resolves library paths and reads library/bundled bytes.
//! A selected source failure is terminal; another same-name source cannot replace it.

use crate::ui::document_context::ModelicaDocuments;
use bevy::prelude::World;
use lunco_workspace::{
    DocumentRuntimeOwner, PinnedDocumentRuntimeOwner, ReplicationOwner, Workspace,
};
use std::path::PathBuf;
use std::sync::Arc;

pub(crate) struct ResolvedClassSource {
    pub source: Arc<str>,
    pub origin_path: Option<PathBuf>,
    pub runtime: DocumentRuntimeOwner,
    pub resident: Option<PinnedDocumentRuntimeOwner>,
}

impl ResolvedClassSource {
    pub(crate) fn from_resident(source: Arc<str>, pin: PinnedDocumentRuntimeOwner) -> Self {
        Self {
            source,
            origin_path: None,
            runtime: pin.runtime.clone(),
            resident: Some(pin),
        }
    }
}

/// Only resident selection needs application state. Edits after admission do
/// not change the captured source Arc; its source lifetime is checked at install.
pub(crate) struct ClassSourcePlan {
    qualified: String,
    resident: Option<ResolvedClassSource>,
    #[cfg(not(target_arch = "wasm32"))]
    file_admission: lunco_workspace::FileDocumentAdmission,
}

impl ClassSourcePlan {
    pub(crate) fn capture(
        qualified: &str,
        registry: &ModelicaDocuments,
        workspace: Option<&Workspace>,
        replication: Option<&ReplicationOwner>,
    ) -> Self {
        let resident =
            find_resident(registry, workspace, replication, qualified).and_then(|document| {
                let host = registry.host(document)?;
                let pin = PinnedDocumentRuntimeOwner::for_document(document, workspace).ok()?;
                Some(ResolvedClassSource::from_resident(
                    host.document().source_arc(),
                    pin,
                ))
            });
        Self {
            qualified: qualified.to_owned(),
            resident,
            #[cfg(not(target_arch = "wasm32"))]
            file_admission: lunco_workspace::FileDocumentAdmission::capture(workspace, replication),
        }
    }

    /// Preserve indexed-library, resident, bundled priority. A selected library
    /// path's read/decode failure must not resolve another source with that name.
    pub(crate) async fn resolve(self) -> Result<ResolvedClassSource, String> {
        if let Some(path) = crate::library_fs::resolve_class_path_indexed(&self.qualified)
            .or_else(|| crate::library_fs::locate_library_file(&self.qualified))
        {
            let provider = lunco_assets_runtime::library::global_library_sources()
                .iter()
                .find(|provider| provider.contains(&path))
                .ok_or_else(|| {
                    format!(
                        "selected library source is no longer available: {}",
                        path.display()
                    )
                })?;
            let (runtime, source, origin_path) = match provider {
                lunco_assets_runtime::library::LibrarySource::InMemory(_) => {
                    let bytes = provider.read(&path).ok_or_else(|| {
                        format!("selected library source is unavailable: {}", path.display())
                    })?;
                    let source = String::from_utf8(bytes).map_err(|error| {
                        format!(
                            "selected library source {} is not UTF-8: {error}",
                            path.display()
                        )
                    })?;
                    (DocumentRuntimeOwner::Application, source, None)
                }
                lunco_assets_runtime::library::LibrarySource::Filesystem(_) => {
                    #[cfg(not(target_arch = "wasm32"))]
                    {
                        let (resolved, source) =
                            lunco_modelica_runtime::source_asset::read_admitted_file(
                                &path,
                                self.file_admission,
                            )
                            .await?;
                        (resolved.runtime, source, Some(resolved.path))
                    }
                    #[cfg(target_arch = "wasm32")]
                    {
                        return Err(format!(
                            "selected filesystem library source is unavailable in browser: {}",
                            path.display()
                        ));
                    }
                }
            };
            return Ok(ResolvedClassSource {
                source: source.into(),
                origin_path,
                runtime,
                resident: None,
            });
        }
        if let Some(resident) = self.resident {
            return Ok(resident);
        }
        let head = lunco_modelica_ast::qualified_name_segments(&self.qualified)
            .next()
            .unwrap_or(&self.qualified);
        let source = crate::models::get_model(&format!("{head}.mo"))?
            .ok_or_else(|| format!("class source was not found: {}", self.qualified))?;
        Ok(ResolvedClassSource {
            source: source.into(),
            origin_path: None,
            runtime: DocumentRuntimeOwner::Application,
            resident: None,
        })
    }
}

fn find_resident(
    registry: &ModelicaDocuments,
    workspace: Option<&Workspace>,
    replication: Option<&ReplicationOwner>,
    qualified: &str,
) -> Option<lunco_doc::DocumentId> {
    registry.iter().find_map(|(document, host)| {
        if !PinnedDocumentRuntimeOwner::for_document(document, workspace)
            .is_ok_and(|pin| pin.is_in_active_scope(workspace, replication))
        {
            return None;
        }
        host.document().strict_ast().and_then(|ast| {
            lunco_modelica_index::class_lookup::find_class_by_qualified_name(&ast, qualified)
                .map(|_| document)
        })
    })
}

/// Shared resident selection for drill-in and immutable duplicate capture.
pub(crate) fn find_open_doc_with_class(
    world: &World,
    qualified: &str,
) -> Option<lunco_doc::DocumentId> {
    let workspace = world
        .get_resource::<lunco_workspace::WorkspaceResource>()
        .map(|workspace| &workspace.0);
    let replication = lunco_core_session::current_replication_owner_in(world);
    find_resident(
        world.resource::<ModelicaDocuments>(),
        workspace,
        replication.as_ref(),
        qualified,
    )
}

/// Optional bundled-source lookup for canvas and compile enrichment.
pub(crate) fn bundled_source_for(qualified: &str) -> Option<String> {
    let head = lunco_modelica_ast::qualified_name_segments(qualified)
        .next()
        .unwrap_or(qualified);
    crate::models::get_model(&format!("{head}.mo"))
        .ok()
        .flatten()
}
