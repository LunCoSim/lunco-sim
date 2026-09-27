//! Shared Rumoca lint results for Modelica documents.
//!
//! Linting runs off the update thread and is keyed to each document's AST
//! generation. The UI and API read the same snapshot so diagnostics do not
//! depend on whether an editor panel is open.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};

use bevy::prelude::*;
use lunco_doc::DocumentId;
use lunco_doc_bevy::DocumentRegistry;
use lunco_modelica_document::ModelicaDocument;

const MAX_ACTIVE_LINTS: usize = 2;

/// Installs the shared, generation-scoped Rumoca lint service for authoring
/// surfaces that expose its diagnostics.
pub struct ModelicaLintPlugin;

impl Plugin for ModelicaLintPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ModelicaLintDiagnostics>();
        app.add_systems(Update, refresh_modelica_lint_diagnostics);
    }
}

/// One Rumoca lint finding with its stable rule id and optional fix guidance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelicaLintDiagnostic {
    pub rule: String,
    pub severity: ModelicaLintSeverity,
    pub message: String,
    pub file: String,
    pub line: u32,
    pub column: u32,
    pub suggestion: Option<String>,
}

/// Severity reported by Rumoca's Modelica linter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelicaLintSeverity {
    Hint,
    Info,
    Warning,
    Error,
}

impl ModelicaLintSeverity {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Hint => "hint",
            Self::Info => "info",
            Self::Warning => "warning",
            Self::Error => "error",
        }
    }
}

/// Current progress for Rumoca lint on one source generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelicaLintState {
    Pending,
    Ready,
    Unavailable(String),
    Failed(String),
}

/// Immutable lint snapshot for one document generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelicaLintSnapshot {
    pub generation: u64,
    pub revision: u64,
    pub state: ModelicaLintState,
    pub diagnostics: Vec<ModelicaLintDiagnostic>,
}

struct CompletedLint {
    doc: DocumentId,
    generation: u64,
    result: Result<Vec<ModelicaLintDiagnostic>, String>,
}

/// Per-document Rumoca lint snapshots shared by the API and workbench.
#[derive(Resource, Default)]
pub struct ModelicaLintDiagnostics {
    snapshots: HashMap<DocumentId, ModelicaLintSnapshot>,
    in_flight: HashSet<DocumentId>,
    completed: Arc<Mutex<VecDeque<CompletedLint>>>,
    next_revision: u64,
}

impl ModelicaLintDiagnostics {
    /// Snapshot only when it matches the requested source generation.
    pub fn for_generation(
        &self,
        doc: DocumentId,
        generation: u64,
    ) -> Option<&ModelicaLintSnapshot> {
        self.snapshots
            .get(&doc)
            .filter(|snapshot| snapshot.generation == generation)
    }

    fn set_snapshot(&mut self, doc: DocumentId, mut snapshot: ModelicaLintSnapshot) {
        self.next_revision = self.next_revision.saturating_add(1);
        snapshot.revision = self.next_revision;
        self.snapshots.insert(doc, snapshot);
    }
}

/// Start bounded background lint jobs and promote only results for the
/// generation that is still current when the job completes.
pub fn refresh_modelica_lint_diagnostics(
    registry: Res<DocumentRegistry<ModelicaDocument>>,
    mut diagnostics: ResMut<ModelicaLintDiagnostics>,
) {
    let completed = {
        let mut queue = diagnostics
            .completed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        std::mem::take(&mut *queue)
    };
    for result in completed {
        diagnostics.in_flight.remove(&result.doc);
        let current_generation = registry
            .host(result.doc)
            .map(|host| host.document().ast().generation);
        if current_generation != Some(result.generation) {
            continue;
        }
        let snapshot = match result.result {
            Ok(findings) => ModelicaLintSnapshot {
                generation: result.generation,
                revision: 0,
                state: ModelicaLintState::Ready,
                diagnostics: findings,
            },
            Err(error) => ModelicaLintSnapshot {
                generation: result.generation,
                revision: 0,
                state: ModelicaLintState::Failed(error),
                diagnostics: Vec::new(),
            },
        };
        diagnostics.set_snapshot(result.doc, snapshot);
    }

    let mut open_documents = registry
        .iter()
        .map(|(doc, host)| (doc, host.document().ast().generation))
        .collect::<Vec<_>>();
    open_documents.sort_by_key(|(doc, _)| doc.raw());
    let open_ids = open_documents
        .iter()
        .map(|(doc, _)| *doc)
        .collect::<HashSet<_>>();
    diagnostics
        .snapshots
        .retain(|doc, _| open_ids.contains(doc));

    #[cfg(target_arch = "wasm32")]
    {
        for (doc, generation) in open_documents {
            if diagnostics.for_generation(doc, generation).is_some() {
                continue;
            }
            diagnostics.set_snapshot(
                doc,
                ModelicaLintSnapshot {
                    generation,
                    revision: 0,
                    state: ModelicaLintState::Unavailable(
                        "Rumoca lint requires a background worker; browser linting is unavailable on this build.".into(),
                    ),
                    diagnostics: Vec::new(),
                },
            );
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        let mut active = diagnostics.in_flight.len();
        for (doc, generation) in open_documents {
            if active >= MAX_ACTIVE_LINTS {
                break;
            }
            if diagnostics.for_generation(doc, generation).is_some() {
                continue;
            }
            if diagnostics.in_flight.contains(&doc) {
                continue;
            }
            let Some(host) = registry.host(doc) else {
                continue;
            };
            let document = host.document();
            let source = document.source().to_owned();
            let file = document.origin().display_name();
            if !diagnostics.in_flight.insert(doc) {
                continue;
            }

            diagnostics.set_snapshot(
                doc,
                ModelicaLintSnapshot {
                    generation,
                    revision: 0,
                    state: ModelicaLintState::Pending,
                    diagnostics: Vec::new(),
                },
            );
            active += 1;
            let sender = Arc::clone(&diagnostics.completed);
            bevy::tasks::AsyncComputeTaskPool::get()
                .spawn(async move {
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        let options = rumoca_tool_lint::LintOptions {
                            min_level: rumoca_tool_lint::LintLevel::Help,
                            ..Default::default()
                        };
                        rumoca_tool_lint::lint(&source, &file, &options)
                            .into_iter()
                            .map(|finding| ModelicaLintDiagnostic {
                                rule: finding.rule.to_owned(),
                                severity: match finding.level {
                                    rumoca_tool_lint::LintLevel::Help => ModelicaLintSeverity::Hint,
                                    rumoca_tool_lint::LintLevel::Note => ModelicaLintSeverity::Info,
                                    rumoca_tool_lint::LintLevel::Warning => {
                                        ModelicaLintSeverity::Warning
                                    }
                                    rumoca_tool_lint::LintLevel::Error => {
                                        ModelicaLintSeverity::Error
                                    }
                                },
                                message: finding.message,
                                file: finding.file,
                                line: finding.line,
                                column: finding.column,
                                suggestion: finding.suggestion,
                            })
                            .collect::<Vec<_>>()
                    }))
                    .map_err(|_| "Rumoca lint failed unexpectedly".to_owned());
                    sender
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .push_back(CompletedLint {
                            doc,
                            generation,
                            result,
                        });
                })
                .detach();
        }
    }
}
