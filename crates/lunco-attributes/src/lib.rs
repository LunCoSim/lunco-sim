//! # Distributed Attribute Management
//!
//! This crate implements the simulation's "Tuning Registry"—a bridge between 
//! raw ECS memory and external System Modeling (SysML) definitions.
//!
//! ## The "Why": SysML Path Mirroring
//! Hardcoding every tunable value (mass, torque, PID gains) creates 
//! brittle code. This system provides a **String-Based Addressing** layer:
//! 1. **Friendly Names**: Allows referencing "vessel.rover1.suspension.k" 
//!    instead of hunting for specific Entity IDs.
//! 2. **Digital Twin Alignment**: Paths can be mapped 1:1 with the 
//!    vessel's architectural model in SysML v2, ensuring the simulation 
//!    exactly mirrors the design documentation.
//! 3. **Headless Tuning**: Enables external processes (Python optimization 
//!    scripts, GMAT optimizers) to mutate the live simulation state via 
//!    generic reflection commands.

use bevy::prelude::*;
use std::collections::HashMap;
use lunco_core::telemetry::TelemetryValue;
use lunco_core::PhysicalPort;

/// Plugin providing the reflection-ready tuning registry.
pub struct LunCoAttributesPlugin;

impl Plugin for LunCoAttributesPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AttributeRegistry>();
        app.add_observer(on_set_attribute);
    }
}

/// A specific simulation memory coordinate.
#[derive(Debug, Clone)]
pub struct AttributeAddress {
    /// The owning Entity.
    pub entity: Entity,
    /// The component type name.
    pub component: String,
    /// The dot-separated path to the field.
    pub field: String,
}

/// A centralized dictionary of engineering parameters.
///
/// **Theory**: Provides a mapping of "Human Paths" to "ECS Locations", 
/// acting as the primary API for external optimization and telemetry scripts.
#[derive(Resource, Default)]
pub struct AttributeRegistry {
    /// Map of canonical paths to simulation memory addresses.
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
