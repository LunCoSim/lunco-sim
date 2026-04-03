//! # Telemetry Reflectance & Mirroring Subsystem
//!
//! This crate implements the simulation's "Optical Fibers"—a generic, 
//! reflection-based data extraction engine. 
//!
//! ## The "Why": Digital Twin Decoupling
//! In a complex digital twin of a spacecraft, manual telemetry definitions 
//! for every component are fragile and high-maintenance. Instead, this 
//! system leverages Bevy's [Reflect] capabilities to provide a "No-Code" 
//! telemetry bridge. 
//!
//! By simply tagging a component with a [Parameter] and a field path 
//! (e.g., `"PhysicalPort.value"`), any internal physics value can be 
//! automatically serialized and broadcast to external Mission Control 
//! systems (like YAMCS or XTCE viewers) for real-time monitoring.
//!
//! ## Headless-First Monitoring
//! This is the primary eyes-and-ears of a headless simulation. Without 
//! a GPU or visual feedback, [TelemetryEvent]s provide the raw data 
//! necessary for automated validation and Flight Software (FSW) telemetry sinks.

use bevy::prelude::*;
use lunco_core::telemetry::{Parameter, TelemetryEvent, Severity, TelemetryValue};

/// Manages the registration and execution of the automated telemetry sampling loop.
pub struct LunCoTelemetryPlugin;

impl Plugin for LunCoTelemetryPlugin {
    fn build(&self, app: &mut App) {
        // High-frequency sampling ensures that transient states (e.g., motor 
        // spikes or short-circuit triggers) are captured for ground-side analysis.
        app.add_systems(Update, sample_parameters_system);
    }
}

/// System wrapper for the world-based parameters sampling.
///
/// Wraps the complex [World] access required for dynamic reflection 
/// into a standard Bevy system order.
fn sample_parameters_system(world: &mut World) {
    sample_parameters(world);
}

/// The core reflect-and-extract engine.
///
/// This system operates in two phases to satisfy the borrow checker while 
/// maintaining high performance:
/// 1. **Discovery**: Navigates the [AppTypeRegistry] using the short name 
///    path provided in the [Parameter].
/// 2. **Extraction**: Drills down into the component's memory via 
///    [PartialReflect] paths and maps the raw memory values into a 
///    unified [TelemetryValue] transport format.
pub fn sample_parameters(world: &mut World) {
    let type_registry = world.resource::<AppTypeRegistry>().clone();
    let registry_read = type_registry.read();
    
    // Resolve simulation time for timestamping results
    let current_epoch = world.resource::<lunco_core::CelestialClock>().epoch;
    
    // Decouple sampling from event triggering to avoid parallel world mutation.
    let mut samples = Vec::new();
    
    let mut query = world.query::<(Entity, &lunco_core::telemetry::Parameter)>();
    for (entity, param) in query.iter(world) {
        if param.path.is_empty() { continue; }
        
        let mut parts = param.path.split('.');
        let component_name = parts.next().unwrap_or("");
        let field_path = parts.collect::<Vec<&str>>().join(".");
        
        if let Some(reg) = registry_read.get_with_short_type_path(component_name) {
            if let Some(reflect_component) = reg.data::<ReflectComponent>() {
                if let Ok(entity_ref) = world.get_entity(entity) {
                    if let Some(reflect_data) = reflect_component.reflect(entity_ref) {
                        let target: Option<&dyn PartialReflect> = if field_path.is_empty() {
                            Some(reflect_data.as_partial_reflect())
                        } else {
                            reflect_data.reflect_path(field_path.as_str()).ok()
                        };
                        
                        if let Some(value_reflect) = target {
                            // Map heterogeneous Rust types to transport variants
                            let val = if let Some(v) = value_reflect.try_downcast_ref::<f32>() {
                                lunco_core::telemetry::TelemetryValue::F64(*v as f64)
                            } else if let Some(v) = value_reflect.try_downcast_ref::<f64>() {
                                lunco_core::telemetry::TelemetryValue::F64(*v)
                            } else if let Some(v) = value_reflect.try_downcast_ref::<i16>() {
                                lunco_core::telemetry::TelemetryValue::I64(*v as i64)
                            } else if let Some(v) = value_reflect.try_downcast_ref::<i32>() {
                                lunco_core::telemetry::TelemetryValue::I64(*v as i64)
                            } else if let Some(v) = value_reflect.try_downcast_ref::<i64>() {
                                lunco_core::telemetry::TelemetryValue::I64(*v)
                            } else if let Some(v) = value_reflect.try_downcast_ref::<bool>() {
                                lunco_core::telemetry::TelemetryValue::Bool(*v)
                            } else if let Some(v) = value_reflect.try_downcast_ref::<String>() {
                                lunco_core::telemetry::TelemetryValue::String(v.clone())
                            } else {
                                continue;
                            };
                            
                            samples.push(lunco_core::telemetry::SampledParameter {
                                name: param.name.clone(),
                                value: val,
                                unit: param.unit.clone(),
                                timestamp: current_epoch,
                            });
                        }
                    }
                }
            }
        }
    }
    
    // Broadcast the captured frame
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
        let captured_val = Arc::new(Mutex::new(lunco_core::telemetry::TelemetryValue::F64(0.0)));

        let c_name = captured_name.clone();
        let c_val = captured_val.clone();

        app.add_observer(move |trigger: On<lunco_core::telemetry::SampledParameter>| {
            let mut name = c_name.lock().unwrap();
            let mut val = c_val.lock().unwrap();
            *name = trigger.event().name.clone();
            *val = trigger.event().value.clone();
        });

        app.world_mut().spawn((
            PhysicalPort { value: 42.0 },
            lunco_core::telemetry::Parameter {
                name: "motor_current".to_string(),
                unit: "Amps".to_string(),
                path: "PhysicalPort.value".to_string(),
            }
        ));

        app.update();

        assert_eq!(*captured_name.lock().unwrap(), "motor_current");
        assert_eq!(*captured_val.lock().unwrap(), lunco_core::telemetry::TelemetryValue::F64(42.0));
    }
}
