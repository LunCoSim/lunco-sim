//! Initial causal input admission before checked solver construction.

use crate::{PortHolds, SimComponent, SimConnection};
use bevy::prelude::*;
use lunco_port_core::ports::PortRegistry;

/// Authored causal connections whose producer surfaces are still preparing.
/// The wiring owner clears these names when it can publish the real edges.
#[derive(Component, Debug, Default, PartialEq, Eq, Reflect)]
#[reflect(Component)]
pub struct PendingInputConnections(pub std::collections::BTreeSet<String>);

/// Read a causal source through its registered backend. A missing sample is
/// pending producer admission, never a numerical zero.
pub fn read_causal_sample(
    registry: &PortRegistry,
    world: &World,
    entity: Entity,
    name: &str,
    is_input: bool,
) -> Option<f64> {
    if is_input {
        registry.read_input_port(world, entity, name)
    } else {
        registry.read_output_port(world, entity, name)
    }
}

/// Capture the connected input values required by a new participant's
/// initialization, with the same source order and affine sum as propagation.
/// `None` holds initialization until every producer has published its sample.
/// Authored holds have the same priority as in the regular exchange.
pub fn initialization_inputs(
    world: &mut World,
    target: Entity,
) -> Result<Option<Vec<(String, f64)>>, String> {
    if world.get::<SimComponent>(target).is_none()
        || world
            .get::<lunco_port_core::PortSurfacePending>(target)
            .is_some()
    {
        return Ok(None);
    }
    let mut values = std::collections::BTreeMap::new();
    if let Some(pending) = world.get::<PendingInputConnections>(target) {
        for name in &pending.0 {
            let Some(value) = world
                .get_resource::<PortHolds>()
                .and_then(|holds| holds.get(target, name))
            else {
                return Ok(None);
            };
            if !value.is_finite() {
                return Err(format!("initial input '{name}' has a non-finite hold"));
            }
            values.insert(name.clone(), value);
        }
    }
    let mut query = world.query::<&SimConnection>();
    let mut wires: Vec<_> = query
        .iter(world)
        .filter(|wire| wire.end_element == target)
        .cloned()
        .collect();
    wires.sort_by(|a, b| {
        a.end_connector
            .cmp(&b.end_connector)
            .then_with(|| {
                world
                    .get::<lunco_core::GlobalEntityId>(a.start_element)
                    .map(|id| id.get())
                    .cmp(
                        &world
                            .get::<lunco_core::GlobalEntityId>(b.start_element)
                            .map(|id| id.get()),
                    )
            })
            .then_with(|| a.start_connector.cmp(&b.start_connector))
            .then_with(|| a.start_element.to_bits().cmp(&b.start_element.to_bits()))
    });
    let registry = world
        .get_resource::<PortRegistry>()
        .ok_or("co-simulation port registry unavailable")?;
    for wire in wires {
        let held = world
            .get_resource::<PortHolds>()
            .and_then(|holds| holds.get(target, &wire.end_connector));
        if let Some(value) = held {
            if !value.is_finite() {
                return Err(format!(
                    "initial input '{}' has a non-finite hold",
                    wire.end_connector
                ));
            }
            values.insert(wire.end_connector, value);
            continue;
        }
        if let Some(component) = world.get::<SimComponent>(wire.start_element) {
            match &component.status {
                crate::SimStatus::Error(error) => {
                    return Err(format!(
                        "initial input '{}' producer failed: {error}",
                        wire.end_connector
                    ));
                }
                _ => {}
            }
        }
        let declared = if wire.start_is_input {
            registry.has_input_port(world, wire.start_element, &wire.start_connector)
        } else {
            registry.has_output_port(world, wire.start_element, &wire.start_connector)
        };
        if !declared {
            if world
                .get::<lunco_port_core::PortSurfacePending>(wire.start_element)
                .is_some()
            {
                return Ok(None);
            }
            return Err(format!(
                "initial input '{}' source port '{}' is not declared",
                wire.end_connector, wire.start_connector
            ));
        }
        let Some(source) = read_causal_sample(
            registry,
            world,
            wire.start_element,
            &wire.start_connector,
            wire.start_is_input,
        ) else {
            return Ok(None);
        };
        let contribution = source * wire.scale + wire.offset;
        let value = values.entry(wire.end_connector.clone()).or_insert(0.0);
        *value += contribution;
        if !source.is_finite() || !contribution.is_finite() || !value.is_finite() {
            return Err(format!(
                "initial input '{}' has a non-finite source or affine sum",
                wire.end_connector
            ));
        }
    }
    Ok(Some(values.into_iter().collect()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_connections_require_samples_or_finite_holds() {
        let mut world = World::new();
        world.init_resource::<PortRegistry>();
        world.init_resource::<PortHolds>();
        let entity = world
            .spawn((
                SimComponent::default(),
                PendingInputConnections(std::collections::BTreeSet::from(["initial_input".into()])),
            ))
            .id();
        assert_eq!(initialization_inputs(&mut world, entity), Ok(None));
        let value = f64::from_bits(0x3ff0_0000_0000_0001);
        world
            .resource_mut::<PortHolds>()
            .hold(entity, "initial_input", value);
        assert_eq!(
            initialization_inputs(&mut world, entity),
            Ok(Some(vec![("initial_input".into(), value)]))
        );
        world
            .resource_mut::<PortHolds>()
            .hold(entity, "initial_input", f64::NAN);
        assert_eq!(
            initialization_inputs(&mut world, entity),
            Err("initial input 'initial_input' has a non-finite hold".into())
        );
        world.entity_mut(entity).remove::<PendingInputConnections>();
        assert_eq!(
            initialization_inputs(&mut world, entity),
            Ok(Some(Vec::new()))
        );
    }
}
