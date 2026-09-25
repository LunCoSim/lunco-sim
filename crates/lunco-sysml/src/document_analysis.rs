//! Asynchronous semantic analysis for open SysML source documents.

use std::collections::BTreeMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex};

use bevy::prelude::*;
use lunco_core_runtime::{AsyncWorkAdmission, AsyncWorkKey, AsyncWorkKind, AsyncWorkPriority};
use lunco_doc::{Document, DocumentId, DocumentOrigin};
use lunco_doc_bevy::{
    DocumentChanged, DocumentClosed, DocumentOpened, DocumentRegistry, DocumentSaved,
};

use crate::{SysmlDocument, document::build_analysis};

const DOCUMENT_ANALYSIS_IDENTITY_PREFIX: u128 = 1_u128 << 64;

/// Current asynchronous semantic state for one exact document revision.
#[derive(Clone)]
pub enum SysmlDocumentAnalysisState {
    /// Analysis has not committed for the current source and origin.
    Pending {
        /// Source document generation captured for analysis.
        generation: u64,
        /// Source identity captured for analysis.
        origin_uri: String,
    },
    /// Immutable analysis committed for the current source and origin.
    Ready {
        /// Source document generation captured for analysis.
        generation: u64,
        /// Source identity captured for analysis.
        origin_uri: String,
        /// Immutable semantic snapshot.
        analysis: Arc<lunco_sysml_ast::SysmlAnalysis>,
    },
    /// Preparation failed for the current source and origin.
    Failed {
        /// Source document generation captured for analysis.
        generation: u64,
        /// Source identity captured for analysis.
        origin_uri: String,
        /// Owner diagnostic describing the failure.
        error: String,
    },
}

impl SysmlDocumentAnalysisState {
    fn matches(&self, generation: u64, origin_uri: &str) -> bool {
        match self {
            Self::Pending {
                generation: stored,
                origin_uri: stored_uri,
            }
            | Self::Ready {
                generation: stored,
                origin_uri: stored_uri,
                ..
            }
            | Self::Failed {
                generation: stored,
                origin_uri: stored_uri,
                ..
            } => *stored == generation && stored_uri == origin_uri,
        }
    }
}

struct PendingDocumentAnalysis {
    doc_id: DocumentId,
    generation: u64,
    origin: DocumentOrigin,
    origin_uri: String,
    source: Arc<str>,
    operation: u64,
    submitted: bool,
    capacity_revision: Option<u64>,
}

struct DocumentAnalysisCompletion {
    doc_id: DocumentId,
    generation: u64,
    origin_uri: String,
    operation: u64,
    result: Result<Arc<lunco_sysml_ast::SysmlAnalysis>, String>,
}

/// Per-document analysis snapshots and generation-fenced worker operations.
#[derive(Resource)]
pub struct SysmlDocumentAnalyses {
    states: BTreeMap<DocumentId, SysmlDocumentAnalysisState>,
    pending: BTreeMap<DocumentId, PendingDocumentAnalysis>,
    completions: Arc<Mutex<Vec<DocumentAnalysisCompletion>>>,
    next_operation: u64,
}

impl Default for SysmlDocumentAnalyses {
    fn default() -> Self {
        Self {
            states: BTreeMap::new(),
            pending: BTreeMap::new(),
            completions: Arc::default(),
            next_operation: 1,
        }
    }
}

impl SysmlDocumentAnalyses {
    /// Read a snapshot only when it exactly matches the live document identity.
    pub fn state_for(
        &self,
        doc_id: DocumentId,
        generation: u64,
        origin_uri: &str,
    ) -> SysmlDocumentAnalysisState {
        self.states
            .get(&doc_id)
            .filter(|state| state.matches(generation, origin_uri))
            .cloned()
            .unwrap_or_else(|| SysmlDocumentAnalysisState::Pending {
                generation,
                origin_uri: origin_uri.to_owned(),
            })
    }
}

fn document_work_key(pending: &PendingDocumentAnalysis) -> AsyncWorkKey {
    AsyncWorkKey::new(
        AsyncWorkKind::SysmlAnalysis,
        pending.doc_id.raw(),
        DOCUMENT_ANALYSIS_IDENTITY_PREFIX | u128::from(pending.doc_id.raw()),
        pending.generation,
        pending.operation,
    )
}

fn retire_pending(pending: PendingDocumentAnalysis, admission: &mut AsyncWorkAdmission) {
    admission.cancel_queued(document_work_key(&pending));
}

fn request_document_analysis(
    doc_id: DocumentId,
    registry: &DocumentRegistry<SysmlDocument>,
    analyses: &mut SysmlDocumentAnalyses,
    admission: &mut AsyncWorkAdmission,
) {
    let Some(host) = registry.host(doc_id) else {
        return;
    };
    let document = host.document();
    let generation = document.generation();
    let origin = document.origin().clone();
    let origin_uri = origin.session_uri();
    if analyses
        .states
        .get(&doc_id)
        .is_some_and(|state| state.matches(generation, &origin_uri))
    {
        return;
    }
    if analyses
        .pending
        .get(&doc_id)
        .is_some_and(|pending| pending.generation == generation && pending.origin_uri == origin_uri)
    {
        return;
    }
    if let Some(previous) = analyses.pending.remove(&doc_id) {
        retire_pending(previous, admission);
    }

    let operation = analyses.next_operation;
    let Some(next_operation) = operation.checked_add(1) else {
        analyses.states.insert(
            doc_id,
            SysmlDocumentAnalysisState::Failed {
                generation,
                origin_uri,
                error: "SysML document analysis operation id exhausted".to_owned(),
            },
        );
        return;
    };
    analyses.next_operation = next_operation;
    analyses.states.insert(
        doc_id,
        SysmlDocumentAnalysisState::Pending {
            generation,
            origin_uri: origin_uri.clone(),
        },
    );
    analyses.pending.insert(
        doc_id,
        PendingDocumentAnalysis {
            doc_id,
            generation,
            origin,
            origin_uri,
            source: document.source_snapshot(),
            operation,
            submitted: false,
            capacity_revision: None,
        },
    );
}

fn on_sysml_document_opened(
    trigger: On<DocumentOpened>,
    registry: Res<DocumentRegistry<SysmlDocument>>,
    mut analyses: ResMut<SysmlDocumentAnalyses>,
    mut admission: ResMut<AsyncWorkAdmission>,
) {
    request_document_analysis(
        trigger.event().doc,
        &registry,
        &mut analyses,
        &mut admission,
    );
}

fn on_sysml_document_changed(
    trigger: On<DocumentChanged>,
    registry: Res<DocumentRegistry<SysmlDocument>>,
    mut analyses: ResMut<SysmlDocumentAnalyses>,
    mut admission: ResMut<AsyncWorkAdmission>,
) {
    request_document_analysis(
        trigger.event().doc,
        &registry,
        &mut analyses,
        &mut admission,
    );
}

fn on_sysml_document_saved(
    trigger: On<DocumentSaved>,
    registry: Res<DocumentRegistry<SysmlDocument>>,
    mut analyses: ResMut<SysmlDocumentAnalyses>,
    mut admission: ResMut<AsyncWorkAdmission>,
) {
    request_document_analysis(
        trigger.event().doc,
        &registry,
        &mut analyses,
        &mut admission,
    );
}

fn on_sysml_document_closed(
    trigger: On<DocumentClosed>,
    mut analyses: ResMut<SysmlDocumentAnalyses>,
    mut admission: ResMut<AsyncWorkAdmission>,
) {
    let doc_id = trigger.event().doc;
    analyses.states.remove(&doc_id);
    if let Some(pending) = analyses.pending.remove(&doc_id) {
        retire_pending(pending, &mut admission);
    }
}

fn prepare_document_analyses(
    registry: Res<DocumentRegistry<SysmlDocument>>,
    mut analyses: ResMut<SysmlDocumentAnalyses>,
    mut admission: ResMut<AsyncWorkAdmission>,
) {
    let capacity_revision = admission.capacity_revision();
    let doc_ids: Vec<_> = analyses.pending.keys().copied().collect();
    let mut terminal = Vec::new();
    let completion_sender = Arc::clone(&analyses.completions);
    let mut state_updates = Vec::new();
    for doc_id in doc_ids {
        let Some(host) = registry.host(doc_id) else {
            terminal.push(doc_id);
            continue;
        };
        let document = host.document();
        let generation = document.generation();
        let origin_uri = document.origin().session_uri();
        let Some(pending) = analyses.pending.get_mut(&doc_id) else {
            continue;
        };
        if pending.generation != generation || pending.origin_uri != origin_uri {
            terminal.push(doc_id);
            continue;
        }
        if pending.submitted || pending.capacity_revision == Some(capacity_revision) {
            continue;
        }

        let key = document_work_key(pending);
        let generation = pending.generation;
        let origin = pending.origin.clone();
        let origin_uri = pending.origin_uri.clone();
        let source = Arc::clone(&pending.source);
        let operation = pending.operation;
        let sender = Arc::clone(&completion_sender);
        let job = move || {
            let result = catch_unwind(AssertUnwindSafe(|| {
                Arc::new(build_analysis(&origin, &source, generation))
            }))
            .map_err(|_| "SysML document analysis worker panicked".to_owned());
            sender
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(DocumentAnalysisCompletion {
                    doc_id,
                    generation,
                    origin_uri,
                    operation,
                    result,
                });
        };
        match admission.submit(AsyncWorkPriority::Interactive, key, job) {
            Ok(()) | Err(lunco_core_runtime::AsyncWorkRejection::DuplicateKey) => {
                pending.submitted = true;
            }
            Err(lunco_core_runtime::AsyncWorkRejection::QueueFull) => {
                pending.capacity_revision = Some(capacity_revision);
            }
            Err(lunco_core_runtime::AsyncWorkRejection::NativeDispatcherUnavailable) => {
                state_updates.push((
                    doc_id,
                    pending.generation,
                    pending.origin_uri.clone(),
                    "SysML document analysis requires a native worker; this host has no Web Worker transport".to_owned(),
                ));
                terminal.push(doc_id);
            }
        }
    }
    for (doc_id, generation, origin_uri, error) in state_updates {
        analyses.states.insert(
            doc_id,
            SysmlDocumentAnalysisState::Failed {
                generation,
                origin_uri,
                error,
            },
        );
    }
    terminal.sort_unstable();
    terminal.dedup();
    for doc_id in terminal {
        if let Some(pending) = analyses.pending.remove(&doc_id) {
            retire_pending(pending, &mut admission);
        }
    }

    let completions = {
        let mut queue = analyses
            .completions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut ready = std::mem::take(&mut *queue);
        ready.sort_by_key(|completion| (completion.doc_id, completion.operation));
        ready
    };
    for completion in completions {
        let Some(pending) = analyses.pending.get(&completion.doc_id) else {
            continue;
        };
        let Some(host) = registry.host(completion.doc_id) else {
            continue;
        };
        if pending.doc_id != completion.doc_id
            || pending.generation != completion.generation
            || pending.origin_uri != completion.origin_uri
            || pending.operation != completion.operation
            || host.document().generation() != completion.generation
            || host.document().origin().session_uri() != completion.origin_uri
        {
            continue;
        }
        let state = match completion.result {
            Ok(analysis) => SysmlDocumentAnalysisState::Ready {
                generation: completion.generation,
                origin_uri: completion.origin_uri,
                analysis,
            },
            Err(error) => SysmlDocumentAnalysisState::Failed {
                generation: completion.generation,
                origin_uri: completion.origin_uri,
                error,
            },
        };
        analyses.states.insert(completion.doc_id, state);
        if let Some(pending) = analyses.pending.remove(&completion.doc_id) {
            retire_pending(pending, &mut admission);
        }
    }
}

/// Register lifecycle invalidation, shared admission, and stable result commits.
pub(crate) fn register(app: &mut App) {
    if !app.is_plugin_added::<lunco_core_runtime::AsyncWorkAdmissionPlugin>() {
        app.add_plugins(lunco_core_runtime::AsyncWorkAdmissionPlugin);
    }
    app.init_resource::<SysmlDocumentAnalyses>()
        .add_systems(Update, prepare_document_analyses)
        .add_observer(on_sysml_document_opened)
        .add_observer(on_sysml_document_changed)
        .add_observer(on_sysml_document_saved)
        .add_observer(on_sysml_document_closed);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_lookup_never_reuses_an_old_generation_or_origin() {
        let doc_id = DocumentId::new(7);
        let origin = DocumentOrigin::untitled("Untitled-7");
        let mut analyses = SysmlDocumentAnalyses::default();
        analyses.states.insert(
            doc_id,
            SysmlDocumentAnalysisState::Ready {
                generation: 3,
                origin_uri: origin.session_uri(),
                analysis: Arc::new(build_analysis(&origin, "part def Rover {}", 3)),
            },
        );

        assert!(matches!(
            analyses.state_for(doc_id, 3, &origin.session_uri()),
            SysmlDocumentAnalysisState::Ready { .. }
        ));
        assert!(matches!(
            analyses.state_for(doc_id, 4, &origin.session_uri()),
            SysmlDocumentAnalysisState::Pending { generation: 4, .. }
        ));
        assert!(matches!(
            analyses.state_for(doc_id, 3, "file:///other.sysml"),
            SysmlDocumentAnalysisState::Pending { .. }
        ));
    }

    #[test]
    fn document_work_key_is_revision_and_operation_scoped() {
        let pending = PendingDocumentAnalysis {
            doc_id: DocumentId::new(7),
            generation: 4,
            origin: DocumentOrigin::untitled("Untitled-7"),
            origin_uri: "untitled://Untitled-7".to_owned(),
            source: Arc::from("part def Rover {}"),
            operation: 12,
            submitted: false,
            capacity_revision: None,
        };
        let first = document_work_key(&pending);
        let mut edited = pending;
        edited.generation += 1;
        edited.operation += 1;
        assert_ne!(first, document_work_key(&edited));
    }

    #[test]
    fn document_analysis_keys_are_disjoint_from_twin_analysis_keys() {
        let pending = PendingDocumentAnalysis {
            doc_id: DocumentId::new(7),
            generation: 0,
            origin: DocumentOrigin::untitled("Untitled-7"),
            origin_uri: "untitled://Untitled-7".to_owned(),
            source: Arc::from("part def Rover {}"),
            operation: 1,
            submitted: false,
            capacity_revision: None,
        };
        let twin_key = AsyncWorkKey::new(AsyncWorkKind::SysmlAnalysis, 7, 7, 0, 1);
        assert_ne!(document_work_key(&pending), twin_key);
    }
}
