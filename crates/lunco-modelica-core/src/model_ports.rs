//! PortRegistry ownership for Modelica runtime inputs.
//!
//! Standalone workbench models own their live inputs on `ModelicaModel`. A
//! co-simulation participant owns the same contract on `SimComponent`; this
//! backend only declares models that are not co-simulation participants, so a
//! write always has exactly one authoritative storage location.

use bevy::prelude::*;
use lunco_cosim_core::SimComponent;
use lunco_modelica_runtime::ModelicaModel;
use lunco_port_core::ports::{
    PortBackend, PortDirection, PortMetadata, PortRegistry, PortTopologyRevision, PortTopologyState,
};
use std::hash::{Hash, Hasher};

/// Register the workbench Modelica input owner and its topology lifecycle.
pub(crate) fn install(app: &mut App) {
    app.init_resource::<PortRegistry>()
        .init_resource::<PortTopologyRevision>()
        .init_resource::<PortTopologyState>()
        .add_observer(lunco_port_core::ports::bump_port_topology_on_add::<ModelicaModel>)
        .add_observer(lunco_port_core::ports::bump_port_topology_on_remove::<ModelicaModel>)
        .add_observer(
            lunco_port_core::ports::bump_port_topology_on_add::<
                lunco_modelica_runtime::ModelicaSignalLayout,
            >,
        )
        .add_observer(
            lunco_port_core::ports::bump_port_topology_on_remove::<
                lunco_modelica_runtime::ModelicaSignalLayout,
            >,
        )
        .add_systems(Update, publish_modelica_input_topology);
    register_port_backend(&mut app.world_mut().resource_mut::<PortRegistry>());
}

pub(crate) fn register_port_backend(registry: &mut PortRegistry) {
    registry.register(MODELICA_INPUT_BACKEND);
}

const MODELICA_INPUT_BACKEND: PortBackend = PortBackend {
    list_entities: |world, out| {
        out.extend(
            world
                .query_filtered::<Entity, (With<ModelicaModel>, Without<SimComponent>)>()
                .iter(world)
                .filter(|entity| {
                    world
                        .get::<ModelicaModel>(*entity)
                        .is_some_and(|model| !model.inputs.is_empty())
                }),
        );
    },
    topology_key: |world, entity| {
        world.get::<ModelicaModel>(entity).map_or(0, |model| {
            modelica_input_topology_key(
                model,
                world.get::<lunco_modelica_runtime::ModelicaSignalLayout>(entity),
            )
        })
    },
    declare_ports: |world, entity, out| {
        if let Some(model) = world.get::<ModelicaModel>(entity)
            && world.get::<SimComponent>(entity).is_none()
        {
            if let Some(name) = out.requested_name() {
                if model.inputs.contains_key(name) {
                    out.declare(name, PortDirection::In);
                }
            } else {
                for name in model.inputs.keys() {
                    out.declare(name, PortDirection::In);
                }
            }
        }
    },
    metadata: |world, entity, name, direction| {
        let unit = match world.get::<lunco_modelica_runtime::ModelicaSignalLayout>(entity) {
            Some(layout) => layout
                .unit_reference(name)
                .expect("Modelica unit metadata carries a non-empty authored identity"),
            None => None,
        };
        PortMetadata::scalar(
            direction,
            unit,
            None,
            None,
            "Modelica runtime input",
            "Modelica runtime input",
            true,
            None,
        )
    },
    read_output: |_world, _entity, _name| None,
    read_input: |world, entity, name| {
        world
            .get::<ModelicaModel>(entity)?
            .inputs
            .get(name)
            .copied()
    },
    resolve_output: None,
    resolve_input: Some(|world, entity, name| {
        let model = world.get::<ModelicaModel>(entity)?;
        if world.get::<SimComponent>(entity).is_some() || !model.inputs.contains_key(name) {
            return None;
        }
        Some(
            model
                .inputs
                .keys()
                .filter(|candidate| candidate.as_str() < name)
                .count() as u64,
        )
    }),
    read_slot: None,
    read_input_slot: Some(|world, entity, slot| {
        let model = world.get::<ModelicaModel>(entity)?;
        if world.get::<SimComponent>(entity).is_some() {
            return None;
        }
        let name = modelica_input_name_at_slot(model, slot)?;
        model.inputs.get(name).copied()
    }),
    write_slot: Some(|world, entity, slot, value| {
        assert!(world.get::<SimComponent>(entity).is_none());
        let mut model = world
            .get_mut::<ModelicaModel>(entity)
            .expect("prepared Modelica input still has its ModelicaModel component");
        let changed = {
            let model_ref = model.bypass_change_detection();
            let name = modelica_input_name_at_slot(model_ref, slot)
                .expect("prepared Modelica input slot remains live through commit")
                .to_owned();
            let previous = model_ref
                .inputs
                .get_mut(&name)
                .expect("prepared Modelica input name remains live through commit");
            let changed = previous.to_bits() != value.to_bits();
            if changed {
                *previous = value;
            }
            changed
        };
        if changed {
            model.set_changed();
        }
    }),
};

fn modelica_input_name_at_slot(model: &ModelicaModel, slot: u64) -> Option<&str> {
    let slot = usize::try_from(slot).ok()?;
    let mut names = model.inputs.keys().map(String::as_str).collect::<Vec<_>>();
    names.sort_unstable();
    names.get(slot).copied()
}

fn modelica_input_topology_key(
    model: &ModelicaModel,
    signal_layout: Option<&lunco_modelica_runtime::ModelicaSignalLayout>,
) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    model.model_name.hash(&mut hasher);
    let mut names = model.inputs.keys().collect::<Vec<_>>();
    names.sort_unstable();
    names.len().hash(&mut hasher);
    for name in names {
        name.hash(&mut hasher);
    }
    signal_layout
        .map(|layout| layout.port_contract_topology_key())
        .hash(&mut hasher);
    hasher.finish()
}

fn publish_modelica_input_topology(
    models: Query<
        (
            Entity,
            &ModelicaModel,
            Option<&lunco_modelica_runtime::ModelicaSignalLayout>,
        ),
        (
            Without<SimComponent>,
            Or<(
                Changed<ModelicaModel>,
                Changed<lunco_modelica_runtime::ModelicaSignalLayout>,
            )>,
        ),
    >,
    mut state: ResMut<PortTopologyState>,
    mut revision: ResMut<PortTopologyRevision>,
) {
    for (entity, model, signal_layout) in &models {
        let model_changed = state
            .changed::<ModelicaModel>(entity, modelica_input_topology_key(model, signal_layout));
        let signal_contract_changed = signal_layout.is_some_and(|layout| {
            state.changed::<lunco_modelica_runtime::ModelicaSignalLayout>(
                entity,
                layout.port_contract_topology_key(),
            )
        });
        if model_changed || signal_contract_changed {
            revision.bump();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standalone_topology_publication_excludes_cosim_and_ignores_live_samples() {
        let mut app = App::new();
        app.init_resource::<PortTopologyState>()
            .init_resource::<PortTopologyRevision>()
            .add_systems(Update, publish_modelica_input_topology);
        let model = || ModelicaModel {
            model_name: "inline model".into(),
            inputs: [("throttle".into(), 0.25)].into(),
            ..Default::default()
        };
        let standalone = app.world_mut().spawn(model()).id();
        let cosim = app
            .world_mut()
            .spawn((model(), SimComponent::default()))
            .id();
        app.update();
        let seeded = app.world().resource::<PortTopologyRevision>().0;
        app.world_mut()
            .get_mut::<ModelicaModel>(cosim)
            .unwrap()
            .inputs
            .insert("cosim-only".into(), 0.5);
        app.world_mut()
            .get_mut::<ModelicaModel>(standalone)
            .unwrap()
            .inputs
            .insert("throttle".into(), 0.75);
        app.update();
        assert_eq!(app.world().resource::<PortTopologyRevision>().0, seeded);
        app.world_mut()
            .get_mut::<ModelicaModel>(standalone)
            .unwrap()
            .inputs
            .insert("brake".into(), 0.0);
        app.update();
        assert_eq!(app.world().resource::<PortTopologyRevision>().0, seeded + 1);
    }
}
