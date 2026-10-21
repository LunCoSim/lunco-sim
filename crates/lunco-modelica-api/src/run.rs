//! Headless Modelica run commands for authored tools.

use std::collections::BTreeMap;

use bevy::prelude::*;
use lunco_command_contracts::{Ack, OpId};
use lunco_core::{Command, on_command, register_commands};
use lunco_doc::{Document, DocumentId};
use lunco_doc_bevy::DocumentRegistry;
use lunco_experiments::{ExperimentOrigin, ExperimentOrigins};
use lunco_experiments::{ExperimentRegistry, ExperimentRunner, ModelRef, RunBounds, TwinId};
use lunco_modelica_document::ModelicaDocument;
use lunco_modelica_runner::{ModelSource, ModelicaRunnerResource, PendingHandles, PendingRun};
use lunco_workspace::{PinnedDocumentRuntimeOwner, WorkspaceResource};

type ModelicaDocuments = DocumentRegistry<ModelicaDocument>;

/// Start an asynchronous Rumoca run for one explicitly selected, current
/// Modelica document snapshot. Results are read through `RunStatus` and
/// `GetExperimentResult` using the returned experiment id.
#[Command(default)]
pub struct RunModelicaSolve {
    pub doc_id: DocumentId,
    pub class: String,
    pub source_generation: u64,
    /// Constant finite Real input bindings for this experiment snapshot.
    pub inputs: BTreeMap<String, f64>,
    pub t_start: f64,
    pub t_end: f64,
    pub dt: f64,
    pub tolerance: Option<f64>,
    pub label: String,
}

#[on_command(RunModelicaSolve)]
fn on_run_modelica_solve(
    trigger: On<RunModelicaSolve>,
    registry: Res<ModelicaDocuments>,
    workspace: Option<Res<WorkspaceResource>>,
    connection: Option<Res<lunco_core_session::ClientConnection>>,
    replica: Option<Res<lunco_core_session::ReplicatedScene>>,
    runner: Option<Res<ModelicaRunnerResource>>,
    mut experiments: Option<ResMut<ExperimentRegistry>>,
    mut sources: Option<ResMut<ExperimentOrigins>>,
    mut pending: Option<ResMut<PendingHandles>>,
    journal: Option<Res<lunco_doc_bevy::JournalResource>>,
    settings: Res<lunco_experiments::ExperimentSettings>,
    mut roots: Option<ResMut<lunco_modelica_source_roots::SourceRootRegistry>>,
    channels: Option<Res<lunco_modelica_runtime::ModelicaChannels>>,
) -> Result<Ack, String> {
    let request = trigger.event();
    if request.doc_id.is_unassigned() {
        return Err("RunModelicaSolve requires an explicit document".into());
    }
    if request.class.trim().is_empty() {
        return Err("RunModelicaSolve requires an explicit Modelica class".into());
    }
    if request.label.trim().is_empty() {
        return Err("RunModelicaSolve requires a non-empty run label".into());
    }
    let mut inputs = BTreeMap::new();
    for (name, value) in &request.inputs {
        if name.trim().is_empty() || !value.is_finite() {
            return Err(
                "RunModelicaSolve inputs require non-empty names and finite Real values".into(),
            );
        }
        inputs.insert(
            lunco_experiments::ParamPath(name.clone()),
            lunco_experiments::ParamValue::Real(*value),
        );
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
    let model_name = lunco_modelica_core::sim_target::resolve_requested_class(
        &request.class,
        &candidates,
        document.index().within_path.as_deref(),
    )
    .map_err(|error| {
        format!(
            "RunModelicaSolve class `{}` {error}; candidates: [{}]",
            request.class,
            candidates.join(", ")
        )
    })?;
    let ast = document.strict_ast().ok_or_else(|| {
        "RunModelicaSolve requires a current strict Modelica source snapshot".to_owned()
    })?;
    let dependency_roots = rumoca_compile::compile::source_root_dependencies(&ast);
    let source = lunco_modelica_source_roots::compile_overlay_source(document);
    let filename = document.origin().session_uri();
    let attribution = PinnedDocumentRuntimeOwner::for_document(
        request.doc_id,
        workspace.as_deref().map(|workspace| &workspace.0),
    )?;
    let replication =
        lunco_core_session::current_replication_owner(connection.as_deref(), replica.as_deref());
    if !attribution.is_current(
        workspace.as_deref().map(|workspace| &workspace.0),
        replication.as_ref(),
    ) {
        return Err("solve source runtime has retired".into());
    }
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

    let model_ref = ModelRef(model_name.clone());
    let source_snapshot = ModelSource {
        model_name,
        source,
        filename,
        extras: Vec::new(),
        runtime: attribution.runtime.clone(),
        result_limits: settings.result_limits,
    };
    let twin_id = TwinId(match attribution.runtime.local_twin() {
        Some(twin) => format!("workspace:{}", twin.raw()),
        None => format!("loose-document:{}", request.doc_id.raw()),
    });
    let bounds = RunBounds {
        t_start: request.t_start,
        t_end: request.t_end,
        dt: Some(request.dt),
        n_intervals: None,
        tolerance: request.tolerance,
        solver: None,
        h0: None,
    };
    lunco_modelica_core::sim_target::validate_run_bounds(&bounds)
        .map_err(|error| error.to_string())?;
    if !dependency_roots.is_empty() {
        let roots = roots
            .as_deref_mut()
            .ok_or_else(|| "Modelica source-root registry is not installed".to_owned())?;
        let channels = channels
            .as_deref()
            .ok_or_else(|| "Modelica worker channel is not available".to_owned())?;
        lunco_modelica_source_roots::admit_compile_roots(roots, dependency_roots, channels)?;
    }
    let origin = ExperimentOrigin::LocalDocument(attribution);
    let experiment_id = sources.insert_new(
        &mut experiments,
        origin.clone(),
        twin_id,
        model_ref,
        Default::default(),
        inputs,
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
    experiments.set_status(experiment_id, lunco_experiments::RunStatus::Queued);
    let result_limits = source_snapshot.result_limits;
    let artifact_admission = origin.artifact_admission(
        workspace.as_deref().map(|workspace| &workspace.0),
        result_limits,
    );
    let handle = runner.0.run_fast(&experiment, source_snapshot);
    pending.0.push(PendingRun {
        artifact_admission,
        handle,
        origin,
        result_limits,
    });

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
