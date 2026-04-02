use bevy::prelude::*;
use std::collections::HashMap;
use lunco_core::architecture::{PhysicalPort, DigitalPort};

pub struct LunCoAttributesPlugin;

impl Plugin for LunCoAttributesPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AttributeRegistry>();
        app.add_observer(process_attribute_writes);
    }
}

/// A centralized dictionary mapping SysML/XTCE string paths 
/// directly to live ECS memory references for real-time manipulation.
#[derive(Resource, Default)]
pub struct AttributeRegistry {
    pub map: HashMap<String, Entity>,
}

/// An abstract request from an external system (MCP, CLI) to mutate simulation state.
#[derive(Event)]
pub struct SetAttribute {
    pub path: String,
    pub value: f32, // Simplified for Phase 2: mapping to abstract floats
}

/// System that executes external requests safely on the main thread, 
/// leveraging the Registry to find the true memory location.
fn process_attribute_writes(
    trigger: On<SetAttribute>,
    registry: Res<AttributeRegistry>,
    mut q_physical: Query<&mut PhysicalPort>,
    mut q_digital: Query<&mut DigitalPort>,
) {
    let evt = trigger.event();
    
    // Look up the string path in the registry
    if let Some(&entity) = registry.map.get(&evt.path) {
        // Here we'd use full `bevy_reflect` to dynamically drill into nested structs.
        // For our test boundaries, PhysicalPort and DigitalPort cover 100% of our hardware limits.
        if let Ok(mut phys) = q_physical.get_mut(entity) {
            phys.value = evt.value;
        } else if let Ok(mut digi) = q_digital.get_mut(entity) {
            digi.raw_value = evt.value as i16;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_real_time_attribute_modification() {
        let mut app = App::new();
        app.add_plugins(LunCoAttributesPlugin);

        let motor_port = app.world_mut().spawn(PhysicalPort { value: 0.0 }).id();
        
        // Register the friendly SysML path to the raw Entity
        app.world_mut()
            .resource_mut::<AttributeRegistry>()
            .map
            .insert("vessel.rover1.motor_left.max_torque".to_string(), motor_port);

        // Simulate an external MCP Tool injecting a dynamic command via the XTCE Attribute API
        app.world_mut().trigger(SetAttribute {
            path: "vessel.rover1.motor_left.max_torque".to_string(),
            value: 95.5,
        });

        app.update();

        // Verify the logic found the entity and securely edited the live ECS state!
        assert_eq!(app.world().get::<PhysicalPort>(motor_port).unwrap().value, 95.5);
    }
}
