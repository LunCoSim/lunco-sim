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
