//! Balloon co-simulation setup — compiles balloon.mo and wires it to AvianSim.
//!
//! When a balloon entity spawns (via SpawnCatalog), it gets a `BalloonModelMarker`.
//! This module picks it up, compiles the Modelica model, and creates wires
//! connecting balloon outputs to AvianSim inputs.

use bevy::prelude::*;
use lunco_assets::assets_dir;
use lunco_cosim::{SimComponent, SimConnection, SimStatus};
use lunco_doc::DocumentOrigin;
use lunco_modelica::{
    ModelicaModel,
    ui::{CompileModel, ModelicaDocumentRegistry},
};
use lunco_sandbox_edit::catalog::BalloonModelMarker;

/// System that triggers Modelica compilation for new balloons.
///
/// Routes through the canonical compile pipeline (`CompileModel`
/// Reflect event → `on_compile_model` observer) rather than driving
/// the worker channel directly. The observer:
///
/// - Refreshes the document AST and runs the AST-based extractors
///   (params **with min/max bounds**, inputs with defaults + runtime
///   input names, descriptions). The previous regex-based extractors
///   (`extract_parameters`, `extract_inputs_with_defaults`) silently
///   dropped `parameter Real x(min=…, max=…)` bounds and connector-
///   typed inputs — both are visible now.
/// - Updates the existing `ModelicaModel` in place (we pre-link the
///   entity below so it hits the update-in-place branch, not the
///   spawn-new-entity branch).
/// - Manages `CompileStates` and `SimStreamRegistry` itself (Phase A
///   lock-free streaming — the balloon now publishes through the same
///   `SimStream` mechanism the workbench panels use).
/// - Writes the source to `modelica_dir()/<entity>_<gen>/model.mo`
///   inside the worker thread (the manual `fs::write` shim that used
///   to sit here was redundant).
pub fn compile_balloon_model(
    mut commands: Commands,
    q_new: Query<(Entity, &Name), Added<BalloonModelMarker>>,
    mut doc_registry: ResMut<ModelicaDocumentRegistry>,
) {
    for (entity, name) in &q_new {
        let model_path = assets_dir().join("models/balloon.mo");
        let source = match std::fs::read_to_string(&model_path) {
            Ok(s) => s,
            Err(e) => {
                warn!("Failed to read balloon.mo: {e}");
                continue;
            }
        };

        // Allocate doc + link entity in one shot. `readonly_file`
        // origin matches the bundled-asset semantics (edits land
        // in-memory only; Save-As to commit).
        let doc_id = doc_registry.open_for_with_origin(
            entity,
            source,
            DocumentOrigin::readonly_file(model_path.clone()),
        );

        // Stub `ModelicaModel` so the canonical compile observer's
        // update-in-place branch finds it. Every meaningful field is
        // overwritten by `on_compile_model` from AST-extracted data;
        // we only seed the path + doc id since those don't come from
        // the AST.
        commands.entity(entity).insert(ModelicaModel {
            model_path: model_path.clone(),
            document: doc_id,
            ..Default::default()
        });

        // Fire the canonical compile. `class: None` means "use the
        // detected name" — balloon.mo defines a single class so the
        // picker logic stays out of the way.
        commands.trigger(CompileModel { doc: doc_id, class: None });

        info!("Balloon '{name}' — compile dispatched (doc {})", doc_id.raw());
    }
}

/// System that creates SimComponent and wires after Modelica compilation succeeds.
///
/// Watches for balloon entities with a compiled ModelicaModel (non-empty variables)
/// that haven't been wired yet (no SimComponent).
pub fn setup_balloon_wires(
    mut commands: Commands,
    q_new: Query<(Entity, &Name, &ModelicaModel), (With<BalloonModelMarker>, Without<SimComponent>)>,
) {
    for (entity, name, model) in &q_new {
        // Skip if compile hasn't produced outputs yet
        if model.variables.is_empty() {
            continue;
        }

        info!("Balloon '{name}' compiled — variables: {:?}", model.variables.keys().collect::<Vec<_>>());
        info!("Balloon '{name}' compiled — setting up SimComponent and wires");

        // Create SimComponent wrapping the ModelicaModel
        let comp = SimComponent {
            model_name: model.model_name.clone(),
            parameters: model.parameters.clone(),
            inputs: model.inputs.clone(),
            outputs: model.variables.clone(),
            status: if model.paused { SimStatus::Paused } else { SimStatus::Running },
            is_stepping: model.is_stepping,
        };
        commands.entity(entity).insert(comp);

        // Wire: balloon.netForce → avian.force_y (aerodynamic force; gravity applied by Avian)
        commands.spawn(SimConnection {
            start_element: entity, start_connector: "netForce".into(),
            end_element: entity, end_connector: "force_y".into(), scale: 1.0,
        });
        // Wire: balloon.volume → self.collider
        commands.spawn(SimConnection {
            start_element: entity, start_connector: "volume".into(),
            end_element: entity, end_connector: "collider".into(), scale: 1.0,
        });
        // Wire: avian.height → balloon.height
        commands.spawn(SimConnection {
            start_element: entity, start_connector: "height".into(),
            end_element: entity, end_connector: "height".into(), scale: 1.0,
        });
        // Wire: avian.velocity_y → balloon.velocity
        commands.spawn(SimConnection {
            start_element: entity, start_connector: "velocity_y".into(),
            end_element: entity, end_connector: "velocity".into(), scale: 1.0,
        });

        info!("Balloon '{name}' — wires created");
        commands.entity(entity).remove::<BalloonModelMarker>();
    }
}

/// Syncs ModelicaModel.variables → SimComponent.outputs every frame.
///
/// Also copies inputs to outputs so that propagate_connections can read them
/// for self-loop wires (e.g., avian.height → balloon.height where the
/// wire reads from SimComponent.outputs to get the height value that was
/// written to SimComponent.inputs by the same propagate_connections system).
pub fn sync_modelica_outputs(
    mut q_models: Query<(&ModelicaModel, &mut SimComponent), Without<BalloonModelMarker>>,
) {
    for (model, mut comp) in &mut q_models {
        // Copy computed variables (outputs) from the Modelica worker: netForce,
        // volume, temperature, buoyancy, etc.
        // Do NOT copy model.inputs here — if input names (height, velocity) were
        // in comp.outputs, propagate_connections would read them instead of the actual
        // AvianSim.outputs, creating a self-loop disconnected from physics.
        for (name, val) in &model.variables {
            comp.outputs.insert(name.clone(), *val);
        }
        comp.status = if model.paused { SimStatus::Paused } else { SimStatus::Running };
    }
}

/// Syncs wire inputs → ModelicaModel.inputs (Avian position → height/velocity).
pub fn sync_inputs_to_modelica(
    mut q_models: Query<(&SimComponent, &mut ModelicaModel), Without<BalloonModelMarker>>,
) {
    for (comp, mut model) in &mut q_models {
        for (name, val) in &comp.inputs {
            model.inputs.insert(name.clone(), *val);
        }
    }
}
