//! # Telemetry & Monitoring Standards
//!
//! This module defines the common data structures for the simulation's 
//! monitoring fabric. It adheres to **XTCE/YAMCS** standards to ensure 
//! compatibility with real-world mission control toolchains.
//!
//! ## Domain Standards
//! 1. **Parameters**: Continuous data points (e.g., Temperature, Voltage) 
//!    sampled at a specific frequency. These are typically broadcast as 
//!    `SampledParameter` packets.
//! 2. **Events**: Discrete notifications of system state changes (e.g., 
//!    "Battery Low", "Command Ack"). These are typically broadcast as 
//!    `TelemetryEvent` packets.
//! 3. **Timekeeping**: All telemetry is timestamped using the [CelestialClock] 
//!    epoch (Julian Date) to allow for precise post-mission analysis and 
//!    correlation with ephemeris data.

use bevy::prelude::*;

/// Severity of a telemetry incident, ordered by urgency.
///
/// **Mapping**: Aligns with the 5-tier YAMCS alert hierarchy.
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

/// A polymorphic container for telemetry values.
///
/// **Why**: Ensures that the telemetry transport layer is agnostic of the 
/// internal Rust type (f32, i32, bool), allowing external subscribers to 
/// deserialize data into a unified variant type.
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

/// A discrete notification pulse.
///
/// **Example**: A "BATTERY_MINIMUM_REACHED" message with severity `Warning`.
#[derive(Event, Debug, Clone, Reflect)]
#[reflect(Debug, Default)]
pub struct TelemetryEvent {
    /// The unique mnemonic for the event.
    pub name: String,
    /// Alert level.
    pub severity: Severity,
    /// Associated data payload.
    pub data: TelemetryValue,
    /// The simulation TDB epoch of the event.
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

/// Metadata tag for a reflection-ready telemetry point.
///
/// **Usage**: Attach this to any component to expose its fields to the 
/// [lunco-telemetry] sampling engine.
#[derive(Component, Debug, Clone, Reflect, Default, PartialEq)]
#[reflect(Component, Default, Debug, PartialEq)]
pub struct Parameter {
    /// Mnemonic name (e.g., "OBC_TEMP").
    pub name: String,
    /// Engineering units (e.g., "degC").
    pub unit: String,
    /// The reflection path to the field (e.g., "PhysicalPort.value").
    pub path: String,
}

/// A captured snapshot of a [Parameter].
///
/// **Logic**: Emitted periodically by the [lunco-telemetry] crate.
#[derive(Event, Debug, Clone, Reflect)]
#[reflect(Debug)]
pub struct SampledParameter {
    /// The mnemonic name of the parameter.
    pub name: String,
    /// The engineering value at the time of sampling.
    pub value: TelemetryValue,
    /// Engineering units for scale context.
    pub unit: String,
    /// The simulation TDB epoch of the sample.
    pub timestamp: f64,
}

