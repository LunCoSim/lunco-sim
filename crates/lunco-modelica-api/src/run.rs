//! Headless Modelica run commands for authored tools.

use bevy::prelude::*;
use lunco_command_contracts::{Ack, OpId};
use lunco_core::{on_command, register_commands, Command};
use lunco_doc::{Document, DocumentId};
use lunco_doc_bevy::DocumentRegistry;
use lunco_experiments::{ExperimentRegistry, ExperimentRunner, ModelRef, RunBounds, TwinId};
use lunco_modelica_document::ModelicaDocument;
use lunco_modelica_runner::{
    ExperimentSources, ModelSource, ModelicaRunnerResource, PendingHandles,
};
use lunco_workspace::WorkspaceResource;

type ModelicaDocuments = DocumentRegistry<ModelicaDocument>;

/// Start an asynchronous Rumoca run for one explicitly selected, current
/// Modelica document snapshot. Results are read through `RunStatus` and
/// `GetExperimentResult` using the returned experiment id.
#[Command(default)]
pub struct RunModelicaSolve {
    pub doc_id: DocumentId,
    pub class: String,
    pub source_generation: u64,
    pub t_start: f64,
    pub t_end: f64,
    pub dt: f64,
    pub label: String,
}

#[on_command(RunModelicaSolve)]
fn on_run_modelica_solve(
    trigger: On<RunModelicaSolve>,
    registry: Res<ModelicaDocuments>,
    workspace: Option<Res<WorkspaceResource>>,
    runner: Option<Res<ModelicaRunnerResource>>,
    mut experiments: Option<ResMut<ExperimentRegistry>>,
    mut sources: Option<ResMut<ExperimentSources>>,
    mut pending: Option<ResMut<PendingHandles>>,
    journal: Option<Res<lunco_doc_bevy::JournalResource>>,
) -> Result<Ack, String> {
    let request = trigger.event();
    if request.doc_id.is_unassigned() {
        return Err("RunModelicaSolve requires an explicit document".into());
    }
    if request.class.trim().is_empty() {
        return Err("RunModelicaSolve requires an explicit Modelica class".into());
    }
    if !request.t_start.is_finite()
        || !request.t_end.is_finite()
        || request.t_end <= request.t_start
        || !request.dt.is_finite()
        || request.dt <= 0.0
    {
        return Err(
            "RunModelicaSolve requires finite increasing bounds and a positive output interval"
                .into(),
        );
    }
    if request.label.trim().is_empty() {
        return Err("RunModelicaSolve requires a non-empty run label".into());
    }

    let host = registry.host(request.doc_id).ok_or_else(|| {
        format!(
            "RunModelicaSolve: unknown Modelica document {}",
            request.doc_id
        )
    })?;
    let document = host.document();
    if document.generation() != request.source_generation {
        return Err(format!(
            "RunModelicaSolve: stale source generation for document {}: expected {}, current {}",
            request.doc_id,
            request.source_generation,
            document.generation()
        ));
    }
    if document.syntax_is_stale() || document.ast_is_stale() {
        return Err("RunModelicaSolve requires a current parsed Modelica source snapshot".into());
    }
    let candidates = document.index().simulation_candidates();
    let model_name =
        lunco_modelica_core::sim_target::resolve_requested_class(&request.class, &candidates)
            .map_err(|error| {
                format!(
                    "RunModelicaSolve class `{}` {error}; candidates: [{}]",
                    request.class,
                    candidates.join(", ")
                )
            })?;
    let source = document.source().to_owned();
    let model_name = lunco_modelica_ast::ast_extract::within_package_of_source(&source)
        .filter(|package| !model_name.starts_with(&format!("{package}.")))
        .map(|package| format!("{package}.{model_name}"))
        .unwrap_or(model_name);
    let filename = document.origin().session_uri();
    let runner = runner.ok_or_else(|| "Modelica runner is not installed".to_owned())?;
    let mut experiments = experiments
        .take()
        .ok_or_else(|| "experiment registry is not installed".to_owned())?;
    let mut sources = sources
        .take()
        .ok_or_else(|| "Modelica experiment source registry is not installed".to_owned())?;
    let mut pending = pending
        .take()
        .ok_or_else(|| "Modelica run-handle queue is not installed".to_owned())?;

    let model_ref = ModelRef(format!("{}#document:{}", model_name, request.doc_id.raw()));
    runner.0.set_model_source(
        model_ref.clone(),
        ModelSource {
            model_name,
            source,
            filename,
            extras: Vec::new(),
        },
    );
    let workspace_twin = workspace.as_ref().and_then(|workspace| {
        let state = &workspace.0;
        state
            .document(request.doc_id)
            .and_then(|entry| entry.context_twin)
            .or_else(|| {
                (state.active_document == Some(request.doc_id))
                    .then_some(state.active_twin)
                    .flatten()
            })
    });
    let twin_id = TwinId(match workspace_twin {
        Some(twin) => format!("workspace:{}", twin.raw()),
        None => format!("loose-document:{}", request.doc_id.raw()),
    });
    let bounds = RunBounds {
        t_start: request.t_start,
        t_end: request.t_end,
        dt: Some(request.dt),
        n_intervals: None,
        tolerance: None,
        solver: None,
        h0: None,
        runtime: Default::default(),
    };
    let experiment_id = experiments.insert_new(
        twin_id,
        model_ref,
        Default::default(),
        Default::default(),
        bounds,
    );
    let Some(experiment) = experiments.get_mut(experiment_id) else {
        return Err("RunModelicaSolve could not read back its new experiment".into());
    };
    experiment.name = request.label.clone();
    let experiment = experiment.clone();

    if let Some(journal) = journal.as_ref() {
        lunco_modelica_core::experiment_journal::record_create(journal, &experiment);
    }
    let handle = runner.0.run_fast(&experiment);
    sources.0.insert(experiment_id, request.doc_id);
    pending.0.push(handle);
    experiments.set_status(experiment_id, lunco_experiments::RunStatus::Queued);

    Ok(Ack::with_data(
        OpId::new(),
        lunco_api_core::api_value!({
            "experiment_id": experiment_id.0.to_string(),
            "state": "dispatched"
        }),
    ))
}

register_commands!(on_run_modelica_solve);

pub struct ModelicaRunApiPlugin;

impl Plugin for ModelicaRunApiPlugin {
    fn build(&self, app: &mut App) {
        register_all_commands(app);
    }
}
