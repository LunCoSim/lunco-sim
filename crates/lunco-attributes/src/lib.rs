use bevy::prelude::*;
use std::collections::HashMap;
use lunco_core::telemetry::{TelemetryValue};

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
    pub entity: Entity,
    /// The short name of the component (e.g., "PhysicalPort")
    pub component: String,
    /// The path within the component (e.g., "value")
    pub field: String,
}

/// A centralized dictionary mapping SysML/XTCE string paths 
/// directly to live ECS memory references for real-time manipulation.
#[derive(Resource, Default)]
pub struct AttributeRegistry {
    pub map: HashMap<String, AttributeAddress>,
}

/// An abstract request from an external system (MCP, CLI) to mutate simulation state.
#[derive(Event, Debug, Clone)]
pub struct SetAttribute {
    pub path: String,
    pub value: TelemetryValue,
}

// Re-implementing with a custom command for reflection mutation
/// NOTE: This implementation performs optimized reflection access.
/// For maximum performance (60Hz+), we recommend caching the ComponentId
/// and using direct Pointer access, as hinted in the master plan.
pub struct ApplyReflectedSet {
    pub address: AttributeAddress,
    pub value: TelemetryValue,
}

impl Command for ApplyReflectedSet {
    fn apply(self, world: &mut World) {
        let type_registry = world.resource::<AppTypeRegistry>().clone();
        let registry_read = type_registry.read();

        // 1. Get the entity
        let mut entity_mut = if let Ok(e) = world.get_entity_mut(self.address.entity) {
            e
        } else { return; };

        // 2. Find the component reflection data by short name
        let reflect_component = if let Some(reg) = registry_read.get_with_short_type_path(&self.address.component) {
            if let Some(reflect_comp) = reg.data::<ReflectComponent>() {
                reflect_comp
            } else { return; }
        } else { return; };

        // 3. Get the component as &mut dyn Reflect
        // Optimization: In a real high-frequency system, we'd cache the ComponentId here.
        let mut reflect_mut = reflect_component.reflect_mut(&mut entity_mut).expect("Failed to get reflect_mut");

        // 4. Drill down to the field path
        let target_field: Option<&mut dyn PartialReflect> = if self.address.field.is_empty() {
            Some((*reflect_mut).as_partial_reflect_mut())
        } else {
            reflect_mut.reflect_path_mut(self.address.field.as_str()).ok()
        };

        // 5. Apply the value
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

/// Observer that executes external requests safely on the main thread, 
/// leveraging the Registry and Bevy Reflection to find and edit the true memory location.
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
