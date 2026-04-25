//! Core types and plugins for the LunCo simulation.
//!
//! This crate provides the foundational components, resources, and systems used
//! across the simulation, including physical properties, celestial timing,
//! and the core plugin registration.

pub mod architecture;
pub mod mocks;
pub mod telemetry;
pub mod coords;
pub mod log;
/// Unified diagram data model — pure Rust, no Bevy dependency.
pub mod diagram;

pub use architecture::*;
pub use mocks::*;
pub use telemetry::*;
pub use log::*;

// ── Typed Command Macros ──────────────────────────────────────────────────────
//
// Import these in your crate for clean usage:
//   use lunco_core::{Command, on_command, register_commands};
//
// #[Command]
//   → struct becomes #[derive(Event, Reflect, Clone, Debug)]
//
// #[on_command(StructName)]
//   → fn wrapped with On<T>, generates __register_<fn>(app)
//
// register_commands!(fn_a, fn_b)
//   → generates pub fn register_all_commands(app) that wires everything up

pub use lunco_command_macro::{Command, on_command, register_commands};

use bevy::prelude::*;

/// The central plugin for the LunCo simulation core.
///
/// Registers all core types for reflection and initializes essential systems
/// like the physical/digital port wiring.
pub struct LunCoCorePlugin;

/// Stable identity for entities across the simulation and API.
///
/// Implements a **53-bit** time-sorted identifier, safe for use as a 
/// raw Number in JavaScript/JSON without precision loss.
/// - 32 bits: Seconds since LunCo Epoch (2025-01-01)
/// - 21 bits: Random instance ID + sequence
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Hash, Reflect, serde::Serialize, serde::Deserialize)]
#[reflect(Component)]
pub struct GlobalEntityId(pub u64);

impl GlobalEntityId {
    /// Create a new globally unique, time-sorted ID (53-bit).
    pub fn new() -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        use web_time::{SystemTime, UNIX_EPOCH};

        // LunCo Epoch: 2025-01-01 00:00:00 UTC
        const LUNCO_EPOCH_SECS: u64 = 1735689600;

        static LAST_ID: AtomicU64 = AtomicU64::new(0);

        let now_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        
        let timestamp = now_secs.saturating_sub(LUNCO_EPOCH_SECS) & 0xFFFFFFFF; // 32 bits
        
        // Base ID with timestamp shifted into the upper part of the 53 bits
        let id_base = timestamp << 21;

        // Atomic update to ensure monotonicity and uniqueness within the same second
        loop {
            let last = LAST_ID.load(Ordering::Relaxed);
            let last_ts = last >> 21;
            
            let next = if last_ts == timestamp {
                // Same second, increment sequence
                (last + 1) & 0x1FFFFFFFFFFFFF // Keep within 53 bits
            } else {
                // New second, start with random entropy in the lower 21 bits
                id_base | (rand_entropy().to_bits() as u64 & 0x1FFFFF)
            };

            if LAST_ID.compare_exchange(last, next, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
                return Self(next);
            }
        }
    }
}

/// Simple entropy helper without full 'rand' dependency
fn rand_entropy() -> f32 {
    static SEED: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(12345);
    let old = SEED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    // Simple LCG-like transformation
    ((old.wrapping_mul(1103515245).wrapping_add(12345)) & 0x7FFFFFFF) as f32
}

impl Default for GlobalEntityId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for GlobalEntityId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::str::FromStr for GlobalEntityId {
    type Err = std::num::ParseIntError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse::<u64>().map(GlobalEntityId)
    }
}

/// Marker component for the user's active avatar/entity in the simulation.
#[derive(Component)]
pub struct Avatar;

/// Defines a spacecraft entity with its ephemeris and physical constraints.
#[derive(Component, Reflect, Default)]
#[reflect(Component)]
pub struct Spacecraft {
    /// Human-readable name of the spacecraft.
    pub name: String,
    /// ID used for ephemeris lookups (e.g., SPICE ID).
    pub ephemeris_id: i32,
    /// Reference body ID (e.g., Earth, Moon).
    pub reference_id: i32,
    /// Start of valid data range in Julian Date.
    pub start_epoch_jd: Option<f64>,
    /// End of valid data range in Julian Date.
    pub end_epoch_jd: Option<f64>,
    /// Collision/interaction radius for simple math-based proximity checks.
    pub hit_radius_m: f32,
    /// Whether this spacecraft should be rendered and listed in the UI.
    pub user_visible: bool,
}

/// Marker component for generic vessels.
#[derive(Component)]
pub struct Vessel;

/// Marker component specifically for surface exploration rovers.
#[derive(Component, Clone, Copy, Reflect, Default)]
#[reflect(Component, Default)]
pub struct RoverVessel;

/// Marker component indicating an entity can be selected as a root object
/// in editing tools (e.g., rover bodies, props, ramps, solar panels).
///
/// Child entities like wheels, colliders, and visuals do NOT have this marker,
/// preventing them from being independently selected. Selection systems should
/// query for this component rather than filtering by name strings.
#[derive(Component)]
pub struct SelectableRoot;

/// Marker component for terrain/ground entities that should be excluded
/// from vessel possession and editing interactions.
#[derive(Component)]
pub struct Ground;

/// Physical properties used for gravity, collision, and mass-based calculations.
///
/// These properties use double precision (`f64`) to maintain simulation integrity
/// over astronomical scales as mandated by the project constitution.
#[derive(Component, Debug, Clone, Reflect, Default)]
#[reflect(Component)]
pub struct PhysicalProperties {
    /// Radius of the body in meters.
    pub radius_m: f64,
    /// Mass of the body in kilograms.
    pub mass_kg: f64,
}

/// Represents a major celestial body (planet, moon, asteroid) in the simulation.
#[derive(Component, Debug, Clone, Reflect, Default)]
#[reflect(Component)]
pub struct CelestialBody {
    /// Name of the celestial body.
    pub name: String,
    /// Unique identifier for ephemeris data retrieval.
    pub ephemeris_id: i32,
    /// Mean radius in meters, used for rendering and approximate physics.
    pub radius_m: f64,
}

/// Global simulation speed and physics state control.
#[derive(Resource, Default, Debug, Clone, Copy)]
pub struct TimeWarpState {
    /// Multiplier for simulation time (e.g., 2.0 = 2x speed).
    pub speed: f64,
    /// Whether the physics engine should be active (paused during warp).
    pub physics_enabled: bool,
}

/// Marker resource indicating that entity dragging is active.
///
/// Used by sandbox editing systems to signal other systems (like avatar possession)
/// to disable conflicting interactions during drag operations.
#[derive(Resource)]
pub struct DragModeActive {
    /// Whether dragging is currently active.
    pub active: bool,
}

impl Default for DragModeActive {
    fn default() -> Self {
        Self { active: false }
    }
}

/// Represents the current "wall clock" time in the simulation universe.
///
/// Uses Julian Date for astronomical precision and provides a mechanism
/// for non-linear time progression.
#[derive(Resource, Debug, Clone, Copy, Reflect)]
#[reflect(Resource)]
pub struct CelestialClock {
    /// Current Julian Date (TDB - Terrestrial Dynamic Time).
    pub epoch: f64,
    /// Multiplier relative to real-time progression.
    pub speed_multiplier: f64,
    /// Pause state for the simulation clock.
    pub paused: bool,
}

impl Default for CelestialClock {
    fn default() -> Self {
        Self {
            epoch: 2451545.0, // J2000.0
            speed_multiplier: 1.0,
            paused: false,
        }
    }
}

impl Plugin for LunCoCorePlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(LunCoLogPlugin);
        app.register_type::<Severity>()
           .register_type::<TelemetryValue>()
           .register_type::<TelemetryEvent>()
           .register_type::<Parameter>()
           .register_type::<SampledParameter>()
           .register_type::<CelestialClock>()
           .register_type::<UserIntent>()
           .register_type::<IntentAnalogState>()
           .register_type::<PhysicalPort>()
           .register_type::<DigitalPort>()
           .register_type::<Wire>()
           .register_type::<PhysicalProperties>()
           .register_type::<CelestialBody>()
           .register_type::<Spacecraft>()
           .register_type::<ActiveAction>()
           .register_type::<ActionStatus>()
           .register_type::<GlobalEntityId>()
           .add_systems(Update, wire_system)
           .add_systems(PostUpdate, assign_global_entity_ids);
    }
}

/// Automatically assigns a [GlobalEntityId] to every entity that lacks one.
fn assign_global_entity_ids(
    mut commands: Commands,
    q_new: Query<Entity, Without<GlobalEntityId>>,
) {
    for entity in q_new.iter() {
        commands.entity(entity).insert(GlobalEntityId::new());
    }
}

/// Syncs digital port values to physical actuators/sensors through wires.
///
/// This system bridges the gap between discrete digital control (i16) and
/// continuous physical forces (f32).
fn wire_system(
    q_wires: Query<&Wire>,
    q_digital: Query<&DigitalPort>,
    mut q_physical: Query<&mut PhysicalPort>,
) {
    for wire in q_wires.iter() {
        if let Ok(digital) = q_digital.get(wire.source) {
            if let Ok(mut physical) = q_physical.get_mut(wire.target) {
                // Normalize i16 (-32768..32767) to -1.0..1.0 approximately, then apply scale
                physical.value = (digital.raw_value as f32 / 32767.0) * wire.scale;
            }
        }
    }
}
