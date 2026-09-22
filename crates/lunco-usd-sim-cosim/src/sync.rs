//! Fixed-step participant synchronization and authored event projection.
//!
//! This module is the runtime exchange boundary after USD participant
//! discovery has published the generic [`SimComponent`] surface.  It owns the
//! per-tick copies between Modelica, Rhai/Python, and that surface, plus the
//! event edge detector that turns authored `LunCoEvent` declarations into
//! telemetry. USD discovery and connection derivation stay in the parent
//! package; scene admission and mounting belong to the USD runtime package.
//! Changes to this exchange do not need to rebuild those owners.

use bevy::prelude::*;
use lunco_cosim_core::{SimComponent, SimStatus, UsdSourcedCosim};
use lunco_modelica_runtime::ModelicaModel;
use lunco_scripting::doc::ScriptedModel;
use lunco_usd_bevy_scene::UsdPrimPath;
use lunco_usd_bevy_stage::{UsdInstanceProjection, UsdInstanceRoot};
use std::collections::HashMap;

/// The status a `ModelicaModel` projects onto its `SimComponent`.
///
/// A durable worker error wins. Compilation also is not readiness: its result
/// contains an initial algebraic snapshot, but no live inputs have reached the
/// solver yet. A model becomes `Running` only after its first successful solver
/// advance, so load readiness cannot release physics onto zero-valued,
/// compile-time ports.
/// One place keeps bind-time insertion and per-tick sync consistent.
pub(crate) fn modelica_status(model: &ModelicaModel) -> SimStatus {
    if let Some(error) = &model.last_error {
        SimStatus::Error(error.clone())
    } else if !model.is_compiled || model.current_time <= 0.0 {
        SimStatus::Compiling
    } else if model.paused {
        SimStatus::Paused
    } else {
        SimStatus::Running
    }
}

/// Copy `f64` port values into a destination map. Stable names and values do
/// not allocate, write, or invalidate the owning ECS component on each
/// `FixedUpdate`; keys are allocated only when first admitted.
#[inline]
fn upsert_value(dst: &mut HashMap<String, f64>, name: &str, value: f64) -> bool {
    match dst.get_mut(name) {
        Some(slot) if slot.to_bits() != value.to_bits() => {
            *slot = value;
            true
        }
        Some(_) => false,
        None => {
            dst.insert(name.to_owned(), value);
            true
        }
    }
}

#[inline]
fn upsert_ports<'a>(
    dst: &mut HashMap<String, f64>,
    src: impl Iterator<Item = (&'a String, &'a f64)>,
) -> bool {
    let mut changed = false;
    for (name, val) in src {
        changed |= upsert_value(dst, name, *val);
    }
    changed
}

/// Per-tick: ModelicaModel.variables → SimComponent.outputs.
/// Lets `propagate_connections` see fresh Modelica outputs each step.
pub fn sync_modelica_outputs(
    mut q: Query<(&ModelicaModel, &mut SimComponent), With<UsdSourcedCosim>>,
) {
    for (model, mut comp) in &mut q {
        let changed = {
            let comp = comp.bypass_change_detection();
            let mut changed = upsert_ports(&mut comp.outputs, model.variables.iter());
            for (k, v) in &model.inputs {
                if !comp.inputs.contains_key(k) {
                    comp.inputs.insert(k.clone(), *v);
                    changed = true;
                }
            }
            let status = modelica_status(model);
            if comp.status != status {
                comp.status = status;
                changed = true;
            }
            changed
        };
        if changed {
            comp.set_changed();
        }
    }
}

/// Copy only values accepted by the Modelica program into its solver input
/// buffer. A USD program prim can also carry physical sink attributes on the
/// same entity (`inputs:force_y` for Avian); those names belong to the shared
/// cosim surface but are not Modelica inputs. Promoting them into
/// `ModelicaModel::inputs` would hide same-named Modelica actuator outputs from
/// the worker's observable set and stop the self-loop from ever binding.
pub(crate) fn copy_modelica_input_values(
    model: &mut ModelicaModel,
    component: &SimComponent,
    command_surface: Option<&lunco_port_core::InputPorts>,
) -> bool {
    let mut changed = false;
    for (name, value) in &component.inputs {
        if model.inputs.contains_key(name) || model.compiled_input_names.contains(name) {
            changed |= upsert_value(&mut model.inputs, name, *value);
        }
    }

    // `InputPorts` is the authored command surface and is registered before the
    // Modelica backend. When a generated domain root carries both components,
    // SetPorts therefore writes the command there; mirror that authoritative
    // value into the solver buffer instead of leaving Modelica on its bind-time
    // default. This is the generic bridge for a shared endpoint, not a vehicle
    // or tutorial-specific path.
    if let Some(command_surface) = command_surface {
        for (name, value) in &command_surface.values {
            if model.inputs.contains_key(name) || model.compiled_input_names.contains(name) {
                changed |= upsert_value(&mut model.inputs, name, *value);
            }
        }
    }
    changed
}

/// Per-tick: shared command/wire inputs → ModelicaModel.inputs.
/// Hands wire-propagated values (height, velocity, …) back to the
/// Modelica worker for the next solver step. An authored `InputPorts` command
/// surface wins for names it owns because it is the public control boundary;
/// the `SimComponent` map remains the destination for propagated model inputs.
pub fn sync_modelica_inputs(
    mut q: Query<
        (
            &SimComponent,
            Option<&lunco_port_core::InputPorts>,
            &mut ModelicaModel,
        ),
        With<UsdSourcedCosim>,
    >,
) {
    for (comp, command_surface, mut model) in &mut q {
        let changed =
            copy_modelica_input_values(model.bypass_change_detection(), comp, command_surface);
        if changed {
            model.set_changed();
        }
    }
}

/// Per-tick: ScriptedModel.outputs → SimComponent.outputs.
pub fn sync_script_outputs(
    mut q: Query<(&ScriptedModel, &mut SimComponent), With<UsdSourcedCosim>>,
) {
    for (model, mut comp) in &mut q {
        let changed = upsert_ports(
            &mut comp.bypass_change_detection().outputs,
            model.outputs.iter(),
        );
        if changed {
            comp.set_changed();
        }
    }
}

/// Per-tick: SimComponent.inputs → ScriptedModel.inputs.
pub fn sync_script_inputs(
    mut q: Query<(&SimComponent, &mut ScriptedModel), With<UsdSourcedCosim>>,
) {
    for (comp, mut model) in &mut q {
        let changed = upsert_ports(
            &mut model.bypass_change_detection().inputs,
            comp.inputs.iter(),
        );
        if changed {
            model.set_changed();
        }
    }
}

// ── Connected discrete signal → event bus ───────────────────────────────────

/// Runtime projection of one authored `LunCoEvent`.
#[derive(Component)]
pub struct EventBinding {
    pub(crate) source_path: String,
    pub(crate) output: String,
    pub(crate) name: String,
    pub(crate) severity: lunco_telemetry_core::Severity,
    pub(crate) latched: bool,
    pub(crate) qualification_time_s: f64,
    pub(crate) qualified_for_s: f64,
    pub(crate) armed: bool,
}

pub(crate) fn parse_event_severity(value: &str) -> Option<lunco_telemetry_core::Severity> {
    match value {
        "debug" => Some(lunco_telemetry_core::Severity::Debug),
        "info" => Some(lunco_telemetry_core::Severity::Info),
        "warning" => Some(lunco_telemetry_core::Severity::Warning),
        "error" => Some(lunco_telemetry_core::Severity::Error),
        "critical" => Some(lunco_telemetry_core::Severity::Critical),
        _ => None,
    }
}

pub(crate) fn event_rising_edge(
    armed: &mut bool,
    qualified_for_s: &mut f64,
    qualification_time_s: f64,
    latched: bool,
    value: f64,
    delta_s: f64,
) -> bool {
    let active = value >= 0.5;
    if !active {
        *qualified_for_s = 0.0;
        if !latched {
            *armed = true;
        }
        return false;
    }
    if !*armed {
        return false;
    }
    *qualified_for_s += delta_s.max(0.0);
    if *qualified_for_s >= qualification_time_s.max(0.0) {
        *armed = false;
        true
    } else {
        false
    }
}

/// Project rising edges of connected 0/1 model outputs onto [`TelemetryEvent`].
pub fn fire_connected_events(
    mut bindings: Query<(Entity, &UsdPrimPath, &mut EventBinding)>,
    sources: Query<(
        Entity,
        &UsdPrimPath,
        &SimComponent,
        Option<&lunco_core::GlobalEntityId>,
    )>,
    q_gid: Query<&lunco_core::GlobalEntityId>,
    q_provenance: Query<&lunco_core::Provenance>,
    q_instance_root: Query<(), With<UsdInstanceRoot>>,
    q_instance_projection: Query<&UsdInstanceProjection>,
    fixed_time: Res<Time<Fixed>>,
    world_time: Option<Res<lunco_time::WorldTime>>,
    mut commands: Commands,
) {
    // Nothing is listening: don't index the scene. This runs every FixedUpdate
    // tick and the index below is a full scan of every cosim participant plus a
    // fresh allocation — paid on every scene, while `LunCoEvent` prims are rare.
    if bindings.is_empty() {
        return;
    }
    let Some(world_time) = world_time else {
        warn!("[usd-cosim] cannot publish a connected event without the authoritative WorldTime");
        return;
    };
    let instance_of = |entity| {
        lunco_usd_bevy_scene::instance_key(
            entity,
            &q_provenance,
            &q_gid,
            &q_instance_root,
            &q_instance_projection,
        )
    };
    let mut by_path = HashMap::new();
    for (entity, path, component, gid) in &sources {
        by_path.insert(
            (instance_of(entity), path.path.as_str()),
            (component, gid.map(|id| id.get()).unwrap_or(0)),
        );
    }
    for (entity, _, mut binding) in &mut bindings {
        let Some((value, source)) = by_path
            .get(&(instance_of(entity), binding.source_path.as_str()))
            .and_then(|(component, source)| {
                component
                    .outputs
                    .get(&binding.output)
                    .map(|value| (*value, *source))
            })
        else {
            continue;
        };
        let latched = binding.latched;
        let qualification_time_s = binding.qualification_time_s;
        let delta_s = fixed_time.delta_secs_f64();
        let EventBinding {
            armed,
            qualified_for_s,
            ..
        } = &mut *binding;
        if event_rising_edge(
            armed,
            qualified_for_s,
            qualification_time_s,
            latched,
            value,
            delta_s,
        ) {
            commands.trigger(lunco_telemetry_core::TelemetryEvent {
                name: binding.name.clone(),
                source,
                severity: binding.severity,
                data: lunco_telemetry_core::TelemetryValue::F64(value),
                timestamp: world_time.epoch_jd,
                sim_secs: 0.0,
                sim_tick: 0,
            });
        }
    }
}
