//! USD → cosim translator.
//!
//! Reads `lunco:modelicaModel` / `lunco:pythonModel` and `lunco:simWires`
//! attributes from USD prims after `sync_usd_visuals` has spawned the
//! entity, and drives the full cosim lifecycle end-to-end:
//!
//! - **Modelica**: opens the source file, inserts a `ModelicaModel`
//!   stub, dispatches `ModelicaCommand::Compile` directly to the
//!   worker channel, and (once `model.variables` populates) wraps the
//!   result in a `SimComponent` so the propagate / apply-forces
//!   pipeline can read it.
//! - **Python**: opens the script, registers a `ScriptDocument`,
//!   attaches `ScriptedModel`, and creates the matching `SimComponent`.
//! - **Wires**: each `lunco:simWires` entry spawns one
//!   `SimConnection` self-loop on the same entity (Modelica/Python
//!   ports ↔ AvianSim ports).
//!
//! Wire entry format inside `lunco:simWires`:
//! `"fromPort:toPort,fromPort:toPort:scale,..."` — comma-separated
//! because `string[]` arrays don't compose across `references` in the
//! current openusd parser.
//!
//! No domain-specific markers (`BalloonModelMarker`, …) are inserted
//! here. The legacy catalog/imperative spawn path in
//! `lunco-sandbox-edit` keeps using its own markers; this translator
//! is the authoritative path for USD-defined cosim entities.

use bevy::prelude::*;
use big_space::prelude::CellCoord;
use lunco_assets::assets_dir;
use lunco_cosim::{SimComponent, SimConnection, SimStatus};
use lunco_doc::DocumentId;
use lunco_modelica::{
    extract_inputs_with_defaults, extract_model_name, extract_parameters, ModelicaChannels,
    ModelicaCommand, ModelicaModel,
};
use lunco_scripting::{
    doc::{ScriptDocument, ScriptLanguage, ScriptedModel},
    ScriptRegistry,
};
use lunco_doc::DocumentHost;
use lunco_usd_bevy::{UsdPrimPath, UsdStageAsset};
use openusd::sdf::{AbstractData, Path as SdfPath, Value};
use std::collections::HashMap;

use crate::UsdSimProcessed;

/// Marker indicating a USD-driven cosim entity has been wired up by
/// `process_usd_cosim_prims`. Prevents the system from re-processing
/// the same entity on subsequent ticks.
#[derive(Component, Default)]
pub struct UsdSourcedCosim;

/// Marker for cross-entity wire prims that have been resolved into a
/// `SimConnection`. Wire prims are typeless USD nodes carrying the
/// `lunco:wireFrom` / `lunco:wireTo` rels — the translator rescans
/// each frame until both endpoints exist as ECS entities, then spawns
/// the wire and tags itself with this marker.
#[derive(Component, Default)]
pub struct UsdSourcedWire;

/// Reads cosim attributes from USD prims and dispatches model
/// compilation + wires. Runs in `Update` after `sync_usd_visuals` so
/// `Transform` / `Mesh3d` / `Material` are already present.
pub fn process_usd_cosim_prims(
    mut commands: Commands,
    query: Query<(Entity, &UsdPrimPath), Without<UsdSourcedCosim>>,
    stages: Res<Assets<UsdStageAsset>>,
    channels: Res<ModelicaChannels>,
    mut script_registry: ResMut<ScriptRegistry>,
) {
    for (entity, prim_path) in query.iter() {
        let Some(stage) = stages.get(&prim_path.stage_handle) else { continue; };
        let Ok(sdf_path) = SdfPath::new(&prim_path.path) else { continue; };
        let reader = (*stage.reader).clone();

        // Gate on `lunco:simWires` presence — that's the attribute that
        // distinguishes a *wired* cosim entity (balloons, future devices)
        // from prims that merely declare a Modelica reference for
        // documentation (wheels, motors, batteries — `lunco:modelicaModel`
        // alone is a forward-looking schema, no translator behaviour).
        let Some(wires_csv) = reader.prim_attribute_value::<String>(&sdf_path, "lunco:simWires") else { continue; };

        let modelica_path = reader.prim_attribute_value::<String>(&sdf_path, "lunco:modelicaModel");
        let python_path = reader.prim_attribute_value::<String>(&sdf_path, "lunco:pythonModel");

        if modelica_path.is_none() && python_path.is_none() {
            warn!(
                "[usd-cosim] {} declares lunco:simWires but neither lunco:modelicaModel nor lunco:pythonModel — skipping",
                prim_path.path
            );
            continue;
        }

        // big_space spatial query (used by camera follow) requires CellCoord;
        // sync_usd_visuals doesn't add it for non-rigid prims.
        commands.entity(entity).insert((
            CellCoord::default(),
            UsdSourcedCosim,
            UsdSimProcessed,
            lunco_core::SelectableRoot,
        ));

        if let Some(rel) = modelica_path.as_ref() {
            dispatch_modelica(&mut commands, entity, rel, &channels);
        }
        if let Some(rel) = python_path.as_ref() {
            dispatch_python(&mut commands, entity, rel, &mut script_registry);
        }

        for raw in wires_csv.split(',') {
            let trimmed = raw.trim();
            if trimmed.is_empty() { continue; }
            if let Some((from_port, to_port, scale)) = parse_wire(trimmed) {
                commands.spawn(SimConnection {
                    start_element: entity,
                    start_connector: from_port,
                    end_element: entity,
                    end_connector: to_port,
                    scale,
                });
            } else {
                warn!("[usd-cosim] {} — could not parse wire entry '{}'", prim_path.path, trimmed);
            }
        }

        let kind = if modelica_path.is_some() { "modelica" } else { "python" };
        info!("[usd-cosim] wired {} ({}) from USD attrs", prim_path.path, kind);
    }
}

fn dispatch_modelica(
    commands: &mut Commands,
    entity: Entity,
    rel_path: &str,
    channels: &ModelicaChannels,
) {
    let model_path = assets_dir().join(rel_path);
    let source = match std::fs::read_to_string(&model_path) {
        Ok(s) => s,
        Err(e) => {
            warn!("[usd-cosim] failed to read Modelica source `{}`: {e}", model_path.display());
            return;
        }
    };

    let model_name = extract_model_name(&source).unwrap_or_else(|| "Model".into());
    let parameters = extract_parameters(&source);
    let inputs = extract_inputs_with_defaults(&source).into_iter().collect();

    commands.entity(entity).insert(ModelicaModel {
        model_path,
        model_name: model_name.clone(),
        parameters,
        inputs,
        ..default()
    });

    let _ = channels.tx.send(ModelicaCommand::Compile {
        entity,
        session_id: 0,
        model_name,
        source,
        stream: None,
    });
}

fn dispatch_python(
    commands: &mut Commands,
    entity: Entity,
    rel_path: &str,
    registry: &mut ScriptRegistry,
) {
    let script_path = assets_dir().join(rel_path);
    let source = match std::fs::read_to_string(&script_path) {
        Ok(s) => s,
        Err(e) => {
            warn!("[usd-cosim] failed to read Python source `{}`: {e}", script_path.display());
            return;
        }
    };

    // Offset doc id away from any Modelica-allocated ids on the same
    // entity (legacy catalog Python balloon does the same).
    let doc_id = DocumentId::new(entity.index().index() as u64 + 10_000);
    registry.documents.insert(
        doc_id,
        DocumentHost::new(ScriptDocument {
            id: doc_id.raw(),
            generation: 0,
            language: ScriptLanguage::Python,
            source,
            inputs: vec!["height".to_string(), "velocity".to_string()],
            outputs: vec!["netForce".to_string()],
        }),
    );
    commands.entity(entity).insert(ScriptedModel {
        document_id: Some(doc_id.raw()),
        language: Some(ScriptLanguage::Python),
        paused: false,
        inputs: Default::default(),
        outputs: Default::default(),
    });

    // Python execution doesn't compile on a separate worker; the
    // SimComponent can be created right away (no need to wait for
    // variables to populate the way Modelica does).
    commands.entity(entity).insert(SimComponent {
        model_name: format!("Python:{}", rel_path),
        parameters: Default::default(),
        inputs: Default::default(),
        outputs: Default::default(),
        status: SimStatus::Running,
        is_stepping: false,
    });
}

/// On-Modelica-compile-complete: wraps the populated `ModelicaModel`
/// into a `SimComponent` so propagate/apply systems can pick it up.
/// Idempotent — only runs for USD-driven entities that don't already
/// have a `SimComponent` and whose Modelica variables have populated.
pub fn wrap_modelica_into_simcomponent(
    mut commands: Commands,
    q_new: Query<
        (Entity, &ModelicaModel),
        (With<UsdSourcedCosim>, Without<SimComponent>),
    >,
) {
    for (entity, model) in q_new.iter() {
        if model.variables.is_empty() {
            continue;
        }
        commands.entity(entity).insert(SimComponent {
            model_name: model.model_name.clone(),
            parameters: model.parameters.clone(),
            inputs: model.inputs.clone(),
            outputs: model.variables.clone(),
            status: if model.paused { SimStatus::Paused } else { SimStatus::Running },
            is_stepping: model.is_stepping,
        });
    }
}

/// Per-tick: ModelicaModel.variables → SimComponent.outputs.
/// Lets `propagate_connections` see fresh Modelica outputs each step.
pub fn sync_modelica_outputs(
    mut q: Query<(&ModelicaModel, &mut SimComponent), With<UsdSourcedCosim>>,
) {
    for (model, mut comp) in &mut q {
        for (name, val) in &model.variables {
            comp.outputs.insert(name.clone(), *val);
        }
        comp.status = if model.paused { SimStatus::Paused } else { SimStatus::Running };
    }
}

/// Per-tick: SimComponent.inputs → ModelicaModel.inputs.
/// Hands wire-propagated values (height, velocity, …) back to the
/// Modelica worker for the next solver step.
pub fn sync_modelica_inputs(
    mut q: Query<(&SimComponent, &mut ModelicaModel), With<UsdSourcedCosim>>,
) {
    for (comp, mut model) in &mut q {
        for (name, val) in &comp.inputs {
            model.inputs.insert(name.clone(), *val);
        }
    }
}

/// Per-tick: ScriptedModel.outputs → SimComponent.outputs.
pub fn sync_script_outputs(
    mut q: Query<(&ScriptedModel, &mut SimComponent), With<UsdSourcedCosim>>,
) {
    for (model, mut comp) in &mut q {
        for (name, val) in &model.outputs {
            comp.outputs.insert(name.clone(), *val);
        }
    }
}

/// Per-tick: SimComponent.inputs → ScriptedModel.inputs.
pub fn sync_script_inputs(
    mut q: Query<(&SimComponent, &mut ScriptedModel), With<UsdSourcedCosim>>,
) {
    for (comp, mut model) in &mut q {
        for (name, val) in &comp.inputs {
            model.inputs.insert(name.to_string(), *val);
        }
    }
}

/// Cross-entity wire translator. Reads typeless USD prims that carry
/// `rel lunco:wireFrom = </path>` + `rel lunco:wireTo = </path>` plus
/// `lunco:fromPort` / `lunco:toPort` / optional `lunco:scale`, resolves
/// the rel targets to ECS entities, and spawns one `SimConnection`.
///
/// Defers when an endpoint hasn't been spawned yet (asset loading is
/// async); reruns each frame until both sides exist or the wire is
/// dropped from the scene.
pub fn process_usd_cosim_wires(
    mut commands: Commands,
    q_unprocessed: Query<(Entity, &UsdPrimPath), Without<UsdSourcedWire>>,
    q_all: Query<(Entity, &UsdPrimPath)>,
    stages: Res<Assets<UsdStageAsset>>,
) {
    // Index every UsdPrimPath entity by its sdf path string. Built once
    // per call — wire prims and their endpoints are typically all in
    // the same stage, so this is cheap.
    let mut by_path: HashMap<String, Entity> = HashMap::new();
    for (e, p) in q_all.iter() {
        by_path.insert(p.path.clone(), e);
    }

    for (entity, prim_path) in q_unprocessed.iter() {
        let Some(stage) = stages.get(&prim_path.stage_handle) else { continue; };
        let Ok(sdf_path) = SdfPath::new(&prim_path.path) else { continue; };
        let reader = (*stage.reader).clone();

        let Some(from_path) = read_rel_target(&reader, &sdf_path, "lunco:wireFrom") else { continue; };
        let Some(to_path) = read_rel_target(&reader, &sdf_path, "lunco:wireTo") else { continue; };
        let from_port = reader.prim_attribute_value::<String>(&sdf_path, "lunco:fromPort");
        let to_port = reader.prim_attribute_value::<String>(&sdf_path, "lunco:toPort");

        let (Some(from_port), Some(to_port)) = (from_port, to_port) else {
            warn!(
                "[usd-cosim] {} declares wire rels but missing lunco:fromPort or lunco:toPort",
                prim_path.path
            );
            commands.entity(entity).insert(UsdSourcedWire);
            continue;
        };
        let scale = reader
            .prim_attribute_value::<f64>(&sdf_path, "lunco:scale")
            .unwrap_or(1.0);

        let from_str = from_path.to_string();
        let to_str = to_path.to_string();
        let (Some(&start_element), Some(&end_element)) =
            (by_path.get(&from_str), by_path.get(&to_str))
        else {
            // Endpoint(s) not spawned yet — try again next frame.
            continue;
        };

        commands.spawn(SimConnection {
            start_element,
            start_connector: from_port.clone(),
            end_element,
            end_connector: to_port.clone(),
            scale,
        });
        commands.entity(entity).insert(UsdSourcedWire);
        info!(
            "[usd-cosim] wire {}.{} → {}.{} (scale={})",
            from_str, from_port, to_str, to_port, scale,
        );
    }
}

/// Reads a single-target `rel <name> = </path>` from a USD prim. USD
/// stores rels under the `targetPaths` field (not `default`) as a
/// `PathListOp`. Returns the first contributing target.
fn read_rel_target(
    reader: &openusd::usda::TextReader,
    prim: &SdfPath,
    name: &str,
) -> Option<SdfPath> {
    let prop_path = prim.append_property(name).ok()?;
    let val = reader.get(&prop_path, "targetPaths").ok()?;
    match val.as_ref() {
        Value::PathListOp(list_op) => list_op.iter().next().cloned(),
        _ => None,
    }
}

/// Parses a `"from:to"` or `"from:to:scale"` wire entry. Empty ports are rejected.
fn parse_wire(raw: &str) -> Option<(String, String, f64)> {
    let mut parts = raw.split(':');
    let from = parts.next()?.trim();
    let to = parts.next()?.trim();
    if from.is_empty() || to.is_empty() { return None; }
    let scale = match parts.next() {
        Some(s) => s.trim().parse::<f64>().ok()?,
        None => 1.0,
    };
    Some((from.to_string(), to.to_string(), scale))
}

/// API query provider: `curl … {"command":"CosimStatus","params":{}}`
/// returns one row per USD-driven cosim entity with position, model
/// state, and propagated cosim values. Lets you probe the running
/// binary without polling logs.
pub struct CosimStatusProvider;

impl lunco_api::ApiQueryProvider for CosimStatusProvider {
    fn name(&self) -> &'static str { "CosimStatus" }
    fn execute(
        &self,
        world: &mut World,
        _params: &serde_json::Value,
    ) -> lunco_api::ApiResponse {
        let mut q = world.query_filtered::<(
            &Name,
            &Transform,
            Option<&SimComponent>,
            Option<&ModelicaModel>,
            Option<&avian3d::prelude::LinearVelocity>,
        ), With<UsdSourcedCosim>>();

        let entities: Vec<serde_json::Value> = q.iter(world).map(|(name, tf, comp, model, lv)| {
            serde_json::json!({
                "name": name.as_str(),
                "y": tf.translation.y,
                "vy": lv.map(|v| v.0.y).unwrap_or(0.0),
                "has_simcomponent": comp.is_some(),
                "modelica_var_count": model.map(|m| m.variables.len()).unwrap_or(0),
                "modelica_paused": model.map(|m| m.paused).unwrap_or(false),
                "modelica_current_time": model.map(|m| m.current_time).unwrap_or(0.0),
                "netForce": comp.and_then(|c| c.outputs.get("netForce").copied()),
                "force_y_input": comp.and_then(|c| c.inputs.get("force_y").copied()),
                "buoyancy": comp.and_then(|c| c.outputs.get("buoyancy").copied()),
            })
        }).collect();
        lunco_api::ApiResponse::ok(serde_json::json!({ "entities": entities }))
    }
}

/// Plugin install hook — registers translator systems, per-tick sync
/// systems, and the API query provider. Called from `UsdSimPlugin::build`.
///
/// Per-tick ordering inside `FixedUpdate` matches the cosim master
/// algorithm:
///   `ModelicaSet::HandleResponses → sync_*_outputs →
///    PropagateCosimSet::Propagate → ApplyForcesCosimSet::ApplyForces →
///    sync_*_inputs → ModelicaSet::SpawnRequests`.
pub(crate) fn install(app: &mut App) {
    use lunco_cosim::systems::{apply_forces::CosimSet as ApplyForcesCosimSet, propagate::CosimSet as PropagateCosimSet};
    use lunco_modelica::ModelicaSet;

    app.add_systems(
        Update,
        (
            process_usd_cosim_prims,
            // Cross-entity wires must run after participant prims are
            // processed (so their entities are addressable in the
            // path → entity index built each call).
            process_usd_cosim_wires,
            wrap_modelica_into_simcomponent,
        ).chain().after(lunco_usd_bevy::sync_usd_visuals),
    );

    app.add_systems(
        FixedUpdate,
        (
            sync_modelica_outputs.after(ModelicaSet::HandleResponses).before(PropagateCosimSet::Propagate),
            sync_script_outputs.after(ModelicaSet::HandleResponses).before(PropagateCosimSet::Propagate),
            sync_modelica_inputs.after(ApplyForcesCosimSet::ApplyForces).before(ModelicaSet::SpawnRequests),
            sync_script_inputs.after(ApplyForcesCosimSet::ApplyForces).before(ModelicaSet::SpawnRequests),
        ),
    );

    app.add_systems(Startup, |mut reg: ResMut<lunco_api::ApiQueryRegistry>| {
        reg.register(CosimStatusProvider);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_wire_default_scale() {
        assert_eq!(parse_wire("netForce:force_y"), Some(("netForce".into(), "force_y".into(), 1.0)));
    }

    #[test]
    fn parse_wire_with_scale() {
        assert_eq!(parse_wire("a:b:2.5"), Some(("a".into(), "b".into(), 2.5)));
    }

    #[test]
    fn parse_wire_rejects_empty() {
        assert_eq!(parse_wire(":b"), None);
        assert_eq!(parse_wire("a:"), None);
        assert_eq!(parse_wire(""), None);
    }
}
