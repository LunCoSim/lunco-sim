//! Automated telemetry sampling and monitoring systems.
//!
//! This crate implements a generic telemetry engine that uses Bevy's reflection
//! system to sample any component field at runtime without requiring 
//! specialized sampling code for every device.

use bevy::prelude::*;
use lunco_core::telemetry::{Parameter, TelemetryEvent, Severity, TelemetryValue};

/// Plugin for managing the automated telemetry sampling cycle.
pub struct LunCoTelemetryPlugin;

impl Plugin for LunCoTelemetryPlugin {
    fn build(&self, app: &mut App) {
        // Run sampling every Update frame to ensure live monitoring in the UI.
        app.add_systems(Update, sample_parameters_system);
    }
}

/// System wrapper for the world-based parameters sampling.
fn sample_parameters_system(world: &mut World) {
    sample_parameters(world);
}

/// The core sampling logic using reflection.
///
/// Iterates over all entities with [Parameter] components and drills down 
/// into their target component fields to extract current simulation values.
pub fn sample_parameters(world: &mut World) {
    let type_registry = world.resource::<AppTypeRegistry>().clone();
    let registry_read = type_registry.read();
    
    // We collect samples first to avoid multiple mutable borrows of the World.
    let mut samples = Vec::new();
    
    let mut query = world.query::<(Entity, &Parameter)>();
    for (entity, param) in query.iter(world) {
        if param.path.is_empty() { continue; }
        
        // Split path (e.g., "PhysicalPort.value") to identify the component and field.
        let mut parts = param.path.split('.');
        let component_name = parts.next().unwrap_or("");
        let field_path = parts.collect::<Vec<&str>>().join(".");
        
        if let Some(reg) = registry_read.get_with_short_type_path(component_name) {
            if let Some(reflect_component) = reg.data::<ReflectComponent>() {
                if let Ok(entity_ref) = world.get_entity(entity) {
                    if let Some(reflect_data) = reflect_component.reflect(entity_ref) {
                        // Dynamically navigate the reflection tree to the target field.
                        let target: Option<&dyn PartialReflect> = if field_path.is_empty() {
                            Some(reflect_data.as_partial_reflect())
                        } else {
                            reflect_data.reflect_path(field_path.as_str()).ok()
                        };
                        
                        if let Some(value_reflect) = target {
                            // Convert various reflected types into unified TelemetryValue types.
                            let val = if let Some(v) = value_reflect.try_downcast_ref::<f32>() {
                                TelemetryValue::F64(*v as f64)
                            } else if let Some(v) = value_reflect.try_downcast_ref::<f64>() {
                                TelemetryValue::F64(*v)
                            } else if let Some(v) = value_reflect.try_downcast_ref::<i32>() {
                                TelemetryValue::I64(*v as i64)
                            } else if let Some(v) = value_reflect.try_downcast_ref::<bool>() {
                                TelemetryValue::Bool(*v)
                            } else {
                                continue;
                            };
                            
                            samples.push(TelemetryEvent {
                                name: param.name.clone(),
                                severity: Severity::Info,
                                data: val,
                                timestamp: 0.0, // TODO: Sync with simulation clock
                            });
                        }
                    }
                }
            }
        }
    }
    
    // Trigger [TelemetryEvent] pulses for each sample captured.
    for sample in samples {
        world.trigger(sample);
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use lunco_core::architecture::PhysicalPort;
    use std::sync::{Arc, Mutex};

    #[test]
    fn test_telemetry_sampling_cycle() {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins, 
            lunco_core::LunCoCorePlugin,
            LunCoTelemetryPlugin
        ));

        let captured_name = Arc::new(Mutex::new(String::new()));
        let captured_val = Arc::new(Mutex::new(TelemetryValue::F64(0.0)));

        let c_name = captured_name.clone();
        let c_val = captured_val.clone();

        // Add observer to capture the trigger
        app.add_observer(move |trigger: On<TelemetryEvent>| {
            let mut name = c_name.lock().unwrap();
            let mut val = c_val.lock().unwrap();
            *name = trigger.event().name.clone();
            *val = trigger.event().data.clone();
        });

        // Spawn a device with a measurable property
        app.world_mut().spawn((
            PhysicalPort { value: 42.0 },
            Parameter {
                name: "motor_current".to_string(),
                unit: "Amps".to_string(),
                path: "PhysicalPort.value".to_string(),
            }
        ));

        app.update();

        assert_eq!(*captured_name.lock().unwrap(), "motor_current");
        assert_eq!(*captured_val.lock().unwrap(), TelemetryValue::F64(42.0));
    }
}
