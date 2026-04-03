//! Simulation attribute management and reflection-based mutation.
//!
//! This crate provides a centralized [AttributeRegistry] that maps human-readable 
//! paths (e.g., "vessel.rover1.motor_left.max_torque") directly to live ECS 
//! memory locations. This enables external tuning (via UI or CLI) without 
//! hardcoding every possible parameter.

use bevy::prelude::*;
use std::collections::HashMap;
use lunco_core::telemetry::{TelemetryValue};

/// Plugin for managing the simulation's dynamic attribute and tuning system.
pub struct LunCoAttributesPlugin;

impl Plugin for LunCoAttributesPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AttributeRegistry>();
        app.add_observer(on_set_attribute);
    }
}

/// A specific memory address for an attribute within the ECS.
#[derive(Debug, Clone)]
pub struct AttributeAddress {
    /// The target entity owning the component.
    pub entity: Entity,
    /// The short name of the component (e.g., "PhysicalPort").
    pub component: String,
    /// The path within the component to the specific field (e.g., "value").
    pub field: String,
}

/// A centralized dictionary mapping string paths to live ECS memory references.
///
/// This serves as the "Source of Truth" for external control systems, 
/// providing a stable API even if internal entity structures change.
#[derive(Resource, Default)]
pub struct AttributeRegistry {
    /// The map of string-based attribute paths to their ECS addresses.
    pub map: HashMap<String, AttributeAddress>,
}

/// An event representing a request to mutate a simulation attribute.
#[derive(Event, Debug, Clone)]
pub struct SetAttribute {
    /// The canonical path to the attribute in the [AttributeRegistry].
    pub path: String,
    /// The new value to apply.
    pub value: TelemetryValue,
}

/// A command that performs the heavy lifting of reflection-based mutation.
///
/// It finds the component on the entity, resolves the field path via reflection, 
/// and applies the new value with appropriate type conversion.
pub struct ApplyReflectedSet {
    /// Target address to modify.
    pub address: AttributeAddress,
    /// Value to inject.
    pub value: TelemetryValue,
}

impl Command for ApplyReflectedSet {
    fn apply(self, world: &mut World) {
        let type_registry = world.resource::<AppTypeRegistry>().clone();
        let registry_read = type_registry.read();

        // 1. Resolve the entity.
        let mut entity_mut = if let Ok(e) = world.get_entity_mut(self.address.entity) {
            e
        } else { return; };

        // 2. Locate the component's reflection data.
        let reflect_component = if let Some(reg) = registry_read.get_with_short_type_path(&self.address.component) {
            if let Some(reflect_comp) = reg.data::<ReflectComponent>() {
                reflect_comp
            } else { return; }
        } else { return; };

        // 3. Mutable access to the component via reflection.
        let mut reflect_mut = reflect_component.reflect_mut(&mut entity_mut).expect("Failed to get reflect_mut");

        // 4. Resolve the specific field within the component.
        let target_field: Option<&mut dyn PartialReflect> = if self.address.field.is_empty() {
            Some((*reflect_mut).as_partial_reflect_mut())
        } else {
            reflect_mut.reflect_path_mut(self.address.field.as_str()).ok()
        };

        // 5. Update the field with type-safe conversion from TelemetryValue.
        if let Some(field) = target_field {
            match self.value {
                TelemetryValue::F64(v) => { 
                    if let Some(f) = field.try_downcast_mut::<f32>() { *f = v as f32; } 
                    else if let Some(f) = field.try_downcast_mut::<f64>() { *f = v; } 
                },
                TelemetryValue::I64(v) => { 
                    if let Some(i) = field.try_downcast_mut::<i16>() { *i = v as i16; } 
                    else if let Some(i) = field.try_downcast_mut::<i32>() { *i = v as i32; } 
                    else if let Some(i) = field.try_downcast_mut::<i64>() { *i = v; } 
                },
                TelemetryValue::Bool(v) => { 
                    if let Some(b) = field.try_downcast_mut::<bool>() { *b = v; } 
                },
                TelemetryValue::String(v) => { 
                    if let Some(s) = field.try_downcast_mut::<String>() { *s = v; } 
                },
            }
        }
    }
}

/// Observer that handles [SetAttribute] events by queueing [ApplyReflectedSet] commands.
fn on_set_attribute(
    trigger: On<SetAttribute>,
    registry: Res<AttributeRegistry>,
    mut commands: Commands,
) {
    let evt = trigger.event();
    if let Some(address) = registry.map.get(&evt.path) {
        commands.queue(ApplyReflectedSet {
            address: address.clone(),
            value: evt.value.clone(),
        });
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_reflection_attribute_modification() {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins, 
            lunco_core::LunCoCorePlugin,
            LunCoAttributesPlugin
        ));

        let motor_port = app.world_mut().spawn(PhysicalPort { value: 0.0 }).id();
        
        // Register the friendly SysML path to the raw Entity + Component + Field
        app.world_mut()
            .resource_mut::<AttributeRegistry>()
            .map
            .insert("vessel.rover1.motor_left.max_torque".to_string(), AttributeAddress {
                entity: motor_port,
                component: "PhysicalPort".to_string(),
                field: "value".to_string(),
            });

        // Trigger reflection-based edit
        app.world_mut().trigger(SetAttribute {
            path: "vessel.rover1.motor_left.max_torque".to_string(),
            value: TelemetryValue::F64(95.5),
        });

        app.update();

        // Verify the reflection logic found the entity, component, and field!
        assert_eq!(app.world().get::<PhysicalPort>(motor_port).unwrap().value, 95.5);
    }
}
