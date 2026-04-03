use bevy::prelude::*;

/// Severity of a telemetry event (aligned with YAMCS/XTCE standards)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Reflect, Default)]
#[reflect(Default, Debug, PartialEq)]
pub enum Severity {
    #[default]
    Debug,
    Info,
    Warning,
    Error,
    Critical,
}

/// A polymorphic value for telemetry parameters and attributes
#[derive(Debug, Clone, PartialEq, Reflect)]
#[reflect(Debug, PartialEq, Default)]
pub enum TelemetryValue {
    F64(f64),
    I64(i64),
    Bool(bool),
    String(String),
}

impl Default for TelemetryValue {
    fn default() -> Self {
        Self::F64(0.0)
    }
}

/// A discrete pulse of telemetry data (e.g., "Battery Low" event)
#[derive(Event, Debug, Clone, Reflect)]
#[reflect(Debug, Default)]
pub struct TelemetryEvent {
    /// The name of the event (e.g., "BATTERY_DRAINED")
    pub name: String,
    /// The severity of the event
    pub severity: Severity,
    /// Associated data for the event
    pub data: TelemetryValue,
    /// Simulation epoch at which the event occurred
    pub timestamp: f64,
}

impl Default for TelemetryEvent {
    fn default() -> Self {
        Self {
            name: "UNKNOWN".to_string(),
            severity: Severity::Info,
            data: TelemetryValue::F64(0.0),
            timestamp: 0.0,
        }
    }
}

/// A live monitoring channel (TM Parameter)
#[derive(Component, Debug, Clone, Reflect, Default, PartialEq)]
#[reflect(Component, Default, Debug, PartialEq)]
pub struct Parameter {
    pub name: String,
    pub unit: String,
    /// Path to the field to sample (e.g., "PhysicalPort.value")
    pub path: String,
}

/// A periodic pulse of sampled telemetry
#[derive(Event, Debug, Clone, Reflect)]
#[reflect(Debug)]
pub struct SampledParameter {
    pub name: String,
    pub value: TelemetryValue,
    pub unit: String,
    pub timestamp: f64,
}

pub struct TelemetryBroadcasterPlugin;

impl Plugin for TelemetryBroadcasterPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, telemetry_broadcaster_system);
    }
}

/// System that samples all entities with a `Parameter` component and emits `SampledParameter` events.
pub fn telemetry_broadcaster_system(
    q_parameters: Query<(Entity, &Parameter)>,
    world: &World,
    mut commands: Commands,
    clock: Res<crate::CelestialClock>,
    time: Res<Time>,
    mut timer: Local<f32>,
) {
    *timer += time.delta_secs();
    if *timer < 1.0 { return; }
    *timer = 0.0;

    let type_registry = world.resource::<AppTypeRegistry>().read();
    
    for (entity, param) in q_parameters.iter() {
        // Parse the path: "Component.field"
        let parts: Vec<&str> = param.path.split('.').collect();
        if parts.is_empty() { continue; }
        
        let component_name = parts[0];
        let field_path = if parts.len() > 1 { parts[1] } else { "" };

        // Find the entity
        let Ok(entity_ref) = world.get_entity(entity) else { continue; };

        // Find component reflection data
        let Some(reg) = type_registry.get_with_short_type_path(component_name) else { continue; };
        let Some(reflect_comp) = reg.data::<ReflectComponent>() else { continue; };
        
        // Get the component as &dyn Reflect
        let Some(reflect_data) = reflect_comp.reflect(entity_ref) else { continue; };

        // Drill down to the field
        let target_field = if field_path.is_empty() {
            Some(reflect_data.as_partial_reflect())
        } else {
            reflect_data.reflect_path(field_path).ok()
        };

        if let Some(field) = target_field {
            let value = if let Some(v) = field.try_downcast_ref::<f32>() {
                TelemetryValue::F64(*v as f64)
            } else if let Some(v) = field.try_downcast_ref::<f64>() {
                TelemetryValue::F64(*v)
            } else if let Some(v) = field.try_downcast_ref::<i16>() {
                TelemetryValue::I64(*v as i64)
            } else if let Some(v) = field.try_downcast_ref::<i32>() {
                TelemetryValue::I64(*v as i64)
            } else if let Some(v) = field.try_downcast_ref::<i64>() {
                TelemetryValue::I64(*v)
            } else if let Some(v) = field.try_downcast_ref::<bool>() {
                TelemetryValue::Bool(*v)
            } else if let Some(v) = field.try_downcast_ref::<String>() {
                TelemetryValue::String(v.clone())
            } else {
                continue;
            };

            commands.trigger(SampledParameter {
                name: param.name.clone(),
                value,
                unit: param.unit.clone(),
                timestamp: clock.epoch,
            });
        }
    }
}
