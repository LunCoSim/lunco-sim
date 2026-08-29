//! # Simulation Control & Communication Fabric
//!
//! This module defines the "Nervous System" of the LunCoSim architecture.
//! It implements a multi-tier hierarchy that separates high-level user
//! intent from low-level physical actuation.
//!
//! ## Why this lives in `lunco-core` (substrate justification)
//!
//! The port/command fabric is consumed by every domain that exchanges a
//! signal: `lunco-cosim` (SimConnection endpoints ARE [`Port`]s),
//! `lunco-mobility` (wheel drive/steer ports), `lunco-hardware` (actuators),
//! `lunco-telemetry` (sampled channels), `lunco-usd-sim`
//! (port authoring from USD). No domain crate can own it without inverting
//! the dependency graph — the same argument recorded for `mobility.rs`'s
//! avian-free classifier. Domain LOGIC does not belong here; only the shared
//! currency types and the command registry those domains meet on.
//!
//! ## The "Why": Fidelity-Driven Emulation
//! Signals move between subsystems through **[Port]**: one `f64` value — a
//! command, an actuator setpoint, a sensor reading, or a value exchanged with a
//! Modelica co-simulation. A directed link between two ports is a
//! `lunco_cosim::SimConnection` (the SSP connection: element + named connector,
//! with factor and offset), which is where a unit conversion belongs when two
//! ports are authored in different units.
//!
//! ## Typed Commands
//!
//! All simulation commands are **typed structs** that derive `#[derive(Command)]`.
//! This replaces the old string-based `CommandMessage` system.
//!
//! ```ignore
//! #[derive(Command)]
//! pub struct SetPorts {
//!     pub target: Entity,
//!     pub writes: Vec<(String, f64)>,
//!     pub seq: u32,
//!     pub tick: u64,
//! }
//! ```
//!
//! Domain crates define their own commands and register them with one line:
//! ```ignore
//! app.register_command::<SetPorts>(on_set_ports);
//! ```
//!
//! The API layer discovers all registered commands via `AppTypeRegistry`
//! reflection — zero hardcoding.

use bevy::prelude::*;
use leafwing_input_manager::prelude::*;

// ── User Intent (Input Abstraction) ───────────────────────────────────────────

/// High-level semantic actions intended by the user.
///
/// These actions are mapped from raw input (keyboard, controller) to
/// abstract simulation intents. This allows the simulation logic to remain
/// agnostic of the input hardware.
#[derive(Actionlike, PartialEq, Eq, Hash, Clone, Copy, Debug, Reflect)]
pub enum UserIntent {
    /// Forward longitudinal movement.
    MoveForward,
    /// Backward longitudinal movement.
    MoveBackward,
    /// Lateral movement to the left.
    MoveLeft,
    /// Lateral movement to the right.
    MoveRight,
    /// Upward vertical movement.
    MoveUp,
    /// Downward vertical movement.
    MoveDown,

    /// Multiplies free-flight translation speed while held.
    ///
    /// This is a presentation-control intent, not a vessel port. It remains
    /// in the shared input vocabulary so the avatar reads the same configured
    /// binding as the help overlay and input simulation tools.
    SpeedBoost,

    /// Camera look/orientation adjustment.
    #[actionlike(DualAxis)]
    Look,
    /// Camera focal length or distance adjustment.
    #[actionlike(Axis)]
    Zoom,

    /// Context-sensitive primary interaction.
    Action,
    /// Normalized propulsion command for a powered vehicle.
    ///
    /// This is deliberately separate from [`UserIntent::Action`]: Action is
    /// the autopilot/editor shortcut, while a powered vehicle may consume
    /// Thrust as its engine command.
    Thrust,
    /// Vehicle-specific braking or hold command.
    ///
    /// This is deliberately separate from [`UserIntent::Action`] and
    /// [`UserIntent::Thrust`]: the former is the autopilot shortcut, while the
    /// latter is a powered-vehicle engine command.
    Brake,
    /// Release/detach a dock or coupling (e.g. a lander→rover fixed joint). Routed
    /// through the normal intent→port machinery to a `release` command port.
    Release,
    /// Toggles between different control or view modes.
    SwitchMode,
    /// Pauses or unpauses the simulation state.
    Pause,
    /// Cancel / back out: release possession or plain follow, back to free flight.
    /// A discrete key intent (default `Backspace`) — see `avatar_escape_possession`.
    /// While an egui field is focused egui consumes the key, so the guard suppresses
    /// this intent that frame and it acts only once the field is defocused.
    Cancel,
    /// Place a waypoint on the scene surface when paired with the primary
    /// pointer action. This is an editor intent, not a rover control port; it
    /// lives in the same input map so the binding is data-driven and rebinding
    /// does not leave the waypoint tool with a private raw-key path.
    PlaceWaypoint,
    /// Delete the current editor selection. This is an editor intent so the
    /// shortcut is rebindable and panels do not inspect raw keyboard state.
    DeleteSelection,
}

impl std::fmt::Display for UserIntent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let label = match self {
            Self::MoveForward => "Move forward",
            Self::MoveBackward => "Move backward",
            Self::MoveLeft => "Move left",
            Self::MoveRight => "Move right",
            Self::MoveUp => "Move up",
            Self::MoveDown => "Move down",
            Self::SpeedBoost => "Speed boost",
            Self::Look => "Look",
            Self::Zoom => "Zoom",
            Self::Action => "Primary action",
            Self::Thrust => "Vehicle thrust",
            Self::Brake => "Vehicle brake",
            Self::Release => "Release coupling",
            Self::SwitchMode => "Switch camera mode",
            Self::Pause => "Pause simulation",
            Self::Cancel => "Cancel current tool",
            Self::PlaceWaypoint => "Place waypoint",
            Self::DeleteSelection => "Delete selection",
        };
        f.write_str(label)
    }
}

/// Alias for the leafwing ActionState using our [UserIntent] enum.
pub type IntentState = ActionState<UserIntent>;

/// A component that stores the current high-resolution analog values of user intents.
///
/// **Why**: While [UserIntent] tracks 'binary' state for mapping, complex
/// systems (like throttle control or gimbal steering) require the raw
/// floating-point deflection of the input device.
#[derive(Component, EntityEvent, Debug, Clone, Reflect)]
#[reflect(Component, Default)]
pub struct IntentAnalogState {
    /// The entity this intent state belongs to.
    pub entity: Entity,
    /// Normalized forward/backward value (-1.0 to 1.0).
    pub forward: f32,
    /// Normalized left/right value (-1.0 to 1.0).
    pub side: f32,
    /// Normalized up/down value (-1.0 to 1.0).
    pub elevation: f32,
    /// Pointer look delta in **screen space** — never radians.
    ///
    /// `+x` is pointer-right, `+y` is pointer-down (the raw device convention),
    /// in device units scaled by the capture gain: the producer
    /// (`lunco_avatar::capture_avatar_intent`) writes
    /// `ActionState::axis_pair(Look) * 10.0`, i.e. mouse motion, not an angle.
    ///
    /// Consumers turn it into an angle themselves — the only one today is
    /// `lunco_avatar::avatar_behavior_input_system`, which applies
    /// `-look_delta * sensitivity * 0.01` to get yaw/pitch radians (note the sign
    /// flip: screen-down must become pitch-up). Steering does **not** read this
    /// field; vessel control flows through the port path
    /// (`ControlBinding` → `SetPorts`), so there is exactly one interpretation.
    ///
    /// Anything new that consumes it owns the same screen-space → radians
    /// conversion; do not write pre-converted angles here.
    pub look_delta: Vec2,
    /// Simulation time when this state was captured.
    pub timestamp: f64,
}

impl Default for IntentAnalogState {
    fn default() -> Self {
        Self {
            entity: Entity::PLACEHOLDER,
            forward: 0.0,
            side: 0.0,
            elevation: 0.0,
            look_delta: Vec2::ZERO,
            timestamp: 0.0,
        }
    }
}

/// Parse a canonical control-intent name (case-insensitive) into a
/// [`UserIntent`]. Used by USD authoring ([`ControlBinding::from_intent_entries`])
/// and the API; authored bindings use one vocabulary everywhere.
pub fn parse_user_intent(name: &str) -> Option<UserIntent> {
    match name.trim().to_ascii_lowercase().as_str() {
        "forward" => Some(UserIntent::MoveForward),
        "backward" => Some(UserIntent::MoveBackward),
        "left" => Some(UserIntent::MoveLeft),
        "right" => Some(UserIntent::MoveRight),
        "yaw_right" => Some(UserIntent::MoveUp),
        "yaw_left" => Some(UserIntent::MoveDown),
        "speed_boost" => Some(UserIntent::SpeedBoost),
        "action" => Some(UserIntent::Action),
        "thrust" => Some(UserIntent::Thrust),
        "brake" => Some(UserIntent::Brake),
        "release" => Some(UserIntent::Release),
        "switch_mode" => Some(UserIntent::SwitchMode),
        "pause" => Some(UserIntent::Pause),
        "cancel" => Some(UserIntent::Cancel),
        "place_waypoint" => Some(UserIntent::PlaceWaypoint),
        "delete_selection" => Some(UserIntent::DeleteSelection),
        _ => None,
    }
}

/// How the possession/follow camera treats a vessel's **attitude** — the
/// authored answer to "should the camera rotate with the body?". It is a
/// property of how the vehicle MOVES, so it is authored on the vessel's control
/// profile (its `Controls` scope, `uniform token lunco:cameraFollow`) right
/// beside the intent→port binding, and read into this component during USD
/// projection.
///
/// The distinction matters because "follow the heading" is right for a surface
/// vehicle — a stable up and a meaningful forward — but wrong for a 6-DOF flyer:
/// extracting a yaw-heading from a body that is pitching and rolling swings the
/// camera wildly (it chases the tumble). A flyer wants a STABLE external frame
/// it rotates INSIDE of (`Orbit`), or — for a pilot who wants the body frame —
/// the FULL orientation (`Chase`). Absent an authored value a vessel defaults to
/// `Heading`, the historical surface-vehicle behavior.
#[derive(Component, Reflect, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[reflect(Component)]
pub enum CameraFollow {
    /// Track the body's position; follow its YAW heading only, up = surface
    /// normal. Ground vehicles (rovers): the camera turns as the vehicle steers.
    #[default]
    Heading,
    /// Track the body's position with a STABLE world/gravity up; do NOT rotate
    /// with the body. A 6-DOF flyer (lander) tumbles inside a steady view.
    Orbit,
    /// Follow the body's FULL orientation (yaw+pitch+roll) — a cockpit/chase
    /// frame that rolls with the craft. Opt-in for pilots who want it.
    Chase,
}

/// Parse a `lunco:cameraFollow` token into a [`CameraFollow`]. Unknown/empty →
/// `None`, so the caller keeps the default (`Heading`).
pub fn parse_camera_follow(s: &str) -> Option<CameraFollow> {
    match s.trim().to_ascii_lowercase().as_str() {
        "heading" => Some(CameraFollow::Heading),
        "orbit" => Some(CameraFollow::Orbit),
        "chase" => Some(CameraFollow::Chase),
        _ => None,
    }
}

/// Per-vessel **intent → port** binding: while a [`UserIntent`] is active it
/// contributes `scale` to the named input port. Multiple entries may share an
/// intent, or a port (e.g. `MoveForward`/`MoveBackward` summing into `throttle`
/// with +1/-1).
///
/// This is the SECOND, per-vessel stage of control. The first (key → intent) is
/// the shared leafwing [`UserIntent`] input map; this component decides only what
/// each intent *actuates* on this vessel, so a rover and a lander share the
/// intent vocabulary while binding different ports. It is authored purely from
/// USD as a `Controls` child scope (intent-named `def` prims with
/// `lunco:port`+`lunco:factor`, built via
/// [`from_intent_entries`](ControlBinding::from_intent_entries)) — there is NO
/// hardcoded Rust default. It is delivered as a child `references` arc to a
/// shared profile in `control_profiles.usda` (the same arc kind wheels use), so
/// it composes through a spawn/reference. The consuming system
/// (`lunco_controller::drive_from_bindings`) reads it off the controlled endpoint
/// via the controller link.
///
/// This is an **adapter**, not the input surface or authority predicate. An
/// endpoint exposes and accepts commands through [`InputPorts`]; it can be
/// remotely controlled with `SetPorts` without an avatar-keyboard binding.
/// Adding a `Controls` scope only makes shared `UserIntent`s (keyboard, gamepad,
/// simulated intent) translate into those exposed ports.
#[derive(Component, Debug, Clone)]
pub struct ControlBinding {
    /// `(intent, port_name, scale)` — each active intent adds its scale to the
    /// port; contributions to one port are summed then clamped to [-1, 1].
    pub binds: Vec<(UserIntent, String, f64)>,
}

impl ControlBinding {
    /// Build from `(intent_name, port, scale)` triples the USD reader collects by
    /// walking a vessel's `Controls` scope — each child prim's NAME is the intent
    /// (`parse_user_intent`), with `string lunco:port` + `double lunco:factor`.
    /// Unknown intents are skipped with a warning; returns `None` when nothing
    /// valid parsed, so the endpoint has no keyboard adapter. There is no
    /// topology fallback: an omitted or invalid authored binding remains
    /// unavailable rather than silently inventing a control surface.
    pub fn from_intent_entries(entries: &[(String, String, f64)]) -> Option<ControlBinding> {
        let mut binds = Vec::new();
        for (intent, port, scale) in entries {
            let Some(i) = parse_user_intent(intent) else {
                warn!("[ControlBinding] unknown control intent '{intent}' (skipped)");
                continue;
            };
            if port.trim().is_empty() {
                warn!("[ControlBinding] empty input port for intent '{intent}' (skipped)");
                continue;
            }
            if !scale.is_finite() {
                warn!(
                    "[ControlBinding] non-finite factor for intent '{intent}' and port '{port}' (skipped)"
                );
                continue;
            }
            binds.push((i, port.clone(), *scale));
        }
        (!binds.is_empty()).then_some(ControlBinding { binds })
    }

    /// The distinct port names this binding targets — i.e. the vessel's declared
    /// input surface (from USD). An endpoint seeds exactly these into its FSW
    /// `inputs` so the strict command backend accepts writes to them and no others.
    pub fn ports(&self) -> impl Iterator<Item = &str> {
        // `binds` is small (a handful of intents); a linear "seen" scan beats a
        // HashSet here and keeps the return borrow-clean.
        let mut seen: Vec<&str> = Vec::new();
        for (_i, port, _s) in &self.binds {
            if !seen.contains(&port.as_str()) {
                seen.push(port.as_str());
            }
        }
        seen.into_iter()
    }

    /// Whether this binding routes the given semantic intent to any port.
    /// Consumers use this to install intent-specific domain actuators only when
    /// the authored control profile actually exposes that intent.
    pub fn has_intent(&self, intent: UserIntent) -> bool {
        self.binds.iter().any(|(bound, _, _)| *bound == intent)
    }

    /// Resolve active intents into summed, clamped port writes. Every port named
    /// by the binding is present (0.0 when its intents are idle) so a released
    /// input writes 0 and clears the setpoint. `active(intent)` is the sole input
    /// — shared by the keyboard path and any internal (rhai/mission/AI) driver.
    pub fn resolve(&self, active: impl Fn(UserIntent) -> bool) -> Vec<(String, f64)> {
        // Keep the first authored occurrence order. HashMap iteration would make
        // the serialized SetPorts order vary between processes.
        let mut values: Vec<(String, f64)> = Vec::new();
        for (_intent, port, _s) in &self.binds {
            if !values.iter().any(|(name, _)| name == port) {
                values.push((port.clone(), 0.0));
            }
        }
        for (intent, port, s) in &self.binds {
            if active(*intent) {
                values
                    .iter_mut()
                    .find(|(name, _)| name == port)
                    .expect("resolve seeds every binding port")
                    .1 += *s;
            }
        }
        values
            .into_iter()
            .map(|(name, value)| (name, value.clamp(-1.0, 1.0)))
            .collect()
    }
}

// ── Ports ─────────────────────────────────────────────────────────────────────

/// A named signal value exchanged between subsystems.
///
/// One port type carries every signal in the simulation — commands from the
/// control surface, actuator setpoints consumed by the physics solvers, sensor
/// readings, and the values a Modelica co-simulation exchanges. Values are `f64`
/// in whatever unit the signal is authored in; a `lunco_cosim::SimConnection`
/// applies factor/offset when two ports are expressed in different units.
#[derive(Component, Debug, Clone, Copy, PartialEq, Default, Reflect)]
#[reflect(Component)]
pub struct Port {
    /// The signal value.
    pub value: f64,
}

/// Marks an endpoint whose input changes authoritative simulated state.
///
/// Engine-owned backends add this marker to their actual state-writing
/// endpoint (a rigid body, joint, force actuator, or a wheel command port).
/// The co-simulation master uses it as a capability when deriving shared-clock
/// causal participants; it does not infer coupling from connector names or
/// solver types.
#[derive(Component, Debug, Clone, Copy, Default, Reflect)]
#[reflect(Component)]
pub struct CausalStateSink;

/// Marks an entity whose dynamic scene-property port surface is now present.
///
/// Some engine-owned backends are installed after the USD entity itself is
/// projected.  A `SphereLight`, for example, is first represented by the USD
/// prim and only then receives its Bevy `PointLight`/`SpotLight` component.
/// Co-simulation binding must be notified at the moment that component-backed
/// surface exists; otherwise a wire can be checked once, classified as
/// missing, and never reconsidered.  This marker is the dependency-neutral
/// lifecycle contract: the producer of a port surface adds it, while the wire
/// engine observes it.  It is intentionally not light-specific so the same
/// contract works for any deferred scene-property backend.
#[derive(Component, Debug, Clone, Copy, Default, Reflect)]
#[reflect(Component)]
pub struct PortSurfaceReady;

/// Marks an entity while a deferred port backend is still being installed.
///
/// A scene projection may author a wire in the same epoch in which its target
/// component is spawned.  The binder must keep that edge pending across an
/// epoch seal while the producer finishes installing its surface; otherwise a
/// valid wire becomes a terminal missing-port fault merely because component
/// insertion and wire projection were observed in different schedules.
#[derive(Component, Debug, Clone, Copy, Default, Reflect)]
#[reflect(Component)]
pub struct PortSurfacePending;

/// Marks a dynamic physics participant while its authored initial state is
/// still being admitted into the live solver.
///
/// This is the inverse lifecycle state of [`PhysicsStateReady`]. Consumers
/// must not sample or record a dynamic body until the USD projection has
/// published its authored pose and velocity and the body has been promoted
/// from its admission state.
#[derive(Component, Debug, Clone, Copy, Default, Reflect)]
#[reflect(Component)]
pub struct PhysicsStatePending;

/// Marks the boundary at which a physics participant has published its
/// authored initial state to the co-simulation fabric.
///
/// This is distinct from [`PortSurfaceReady`]: a rigid body can expose its
/// velocity and attitude ports while it is still being held kinematic during
/// articulated-scene admission. Sensors use this fact to acquire their first
/// live sample without treating the loader's zero-valued placeholder as a
/// physical measurement.
#[derive(Component, Debug, Clone, Copy, Default, Reflect)]
#[reflect(Component)]
pub struct PhysicsStateReady;

// ── Control surface ───────────────────────────────────────────────────────────

/// An entity's declared **`inputs:*` port surface**, with current values.
///
/// The input *vocabulary is data* — the keys present here declare exactly which
/// input ports this entity accepts, so the port backend stays strict (an
/// undeclared name is rejected → still reported as a dangling wire). A rover may
/// expose `throttle`/`steer`/`brake`, an avatar `forward`/`side`/`up`, and a
/// factory `start_cycle`/`target_rate`. Inputs are scalar `f64`s: an intent such
/// as `Action` or `Thrust` normally produces a binary `0.0`/`1.0` input, while analog control
/// can supply any normalized or physical value. The keys are seeded from the vessel's
/// [`ControlBinding`] (i.e. from its authored `Controls` scope) for USD vessels;
/// runtime-built endpoints may declare the same surface directly with
/// [`InputPorts::new`]. The surface, not the optional binding, is the command
/// endpoint. The avatar domain uses a non-`Avatar` surface as the vessel
/// possession boundary; an avatar's own surface is reserved for free flight.
///
/// Written through the shared port substrate (`SetPorts` → `PortRegistry`)
/// and consumed by the vehicle's actuator (`apply_drive_mix`, `apply_fly`, a
/// Modelica bridge, …).
///
/// NOTE: the command port named `"brake"` here is NOT the output port named
/// `"brake"` in [`OutputPorts`]. They carry different values — an analog command
/// in `[-1,1]` here, a discretized `1.0`/`0.0` gate there — and are deliberately
/// kept in two components so the two `"brake"`s can never be conflated.
#[derive(Component, Debug, Clone, Default)]
pub struct InputPorts {
    /// Current value per accepted input-port name. Only seeded keys are writable;
    /// see the type docs.
    pub values: std::collections::HashMap<String, f64>,
    /// Derived brake state, cached from `values["brake"] > 0.5` by the actuator so
    /// the per-tick physics systems read a bool without a map lookup.
    pub brake_active: bool,
}

impl InputPorts {
    /// Build with a seeded command vocabulary: the input-port names this vehicle
    /// accepts, each initialised to `0.0`. The seeded keys ARE the input surface.
    pub fn new(command_ports: &[&str]) -> Self {
        Self {
            values: command_ports.iter().map(|n| (n.to_string(), 0.0)).collect(),
            brake_active: false,
        }
    }

    /// Build a command surface from authored scalar defaults. The keys still
    /// define the accepted vocabulary; this preserves USD's initial value on
    /// each declared port instead of replacing an explicit value with zero.
    pub fn with_defaults(defaults: impl IntoIterator<Item = (String, f64)>) -> Self {
        Self {
            values: defaults.into_iter().collect(),
            brake_active: false,
        }
    }

    /// Current value of command input `name` (`0.0` if this vehicle doesn't accept
    /// it). The read side of the input surface for actuators.
    #[inline]
    pub fn cmd(&self, name: &str) -> f64 {
        self.values.get(name).copied().unwrap_or(0.0)
    }

    /// Move the logical command surface to its safe state without inventing
    /// undeclared ports. This is the input half of [`safe_stop_control_surface`].
    pub fn safe_stop(&mut self) {
        if let Some(throttle) = self.values.get_mut("throttle") {
            *throttle = 0.0;
        }
        if let Some(steer) = self.values.get_mut("steer") {
            *steer = 0.0;
        }
        if let Some(brake) = self.values.get_mut("brake") {
            *brake = 1.0;
            self.brake_active = true;
        }
    }
}

/// The [`InputPorts`] governing `entity` — its own, or the nearest ancestor's.
///
/// A command surface belongs to the VESSEL, and a part is not always a child of
/// it. On an articulated rover a wheel hangs off a rocker link
/// (`rocker_bogie.usda` hinges its wheels to `/RockerBogie/RockerL|R`), so the
/// wheel's carrier body is a suspension member with no command surface of its
/// own. Anything asking "is my vehicle braking?" by looking at its immediate
/// parent gets `None` there and, if it treats that as "not braking", silently
/// loses the brake on exactly the rovers with the most suspension.
///
/// Walking up terminates at the vessel because only vessels carry `InputPorts`.
pub fn owning_input_ports<'w>(
    entity: Entity,
    q_child_of: &Query<&ChildOf>,
    q_inputs: &'w Query<&InputPorts>,
) -> Option<&'w InputPorts> {
    let mut cur = entity;
    loop {
        if let Ok(inputs) = q_inputs.get(cur) {
            return Some(inputs);
        }
        cur = q_child_of.get(cur).ok()?.parent();
    }
}

/// A runtime index from **output** name to the [`Port`] entity carrying that
/// output's current value.
///
/// This is the produced-value half of a control surface, and is a different
/// thing from [`InputPorts`]: those are the logical input values a human or
/// script issues, while these are runtime registers written by an imperative
/// producer such as a drive kernel. The names and topology still come from
/// authored USD `outputs:*` attributes; this component only stores the runtime
/// endpoint for each one.
///
/// The port entities are owned by their producer so the recursive scene-clear
/// reclaims them with it. Generated Modelica outputs stay on `SimComponent` and
/// are never duplicated here.
#[derive(Component, Debug, Clone, Default)]
pub struct OutputPorts {
    /// Maps authored output names (e.g. `"drive_left"`) to their `Port` entity.
    pub ports: std::collections::HashMap<String, Entity>,
}

impl OutputPorts {
    /// Build from a prebuilt output-name → `Port` entity index.
    pub fn new(ports: std::collections::HashMap<String, Entity>) -> Self {
        Self { ports }
    }

    /// The `Port` entity for output `name`, if this producer has one.
    #[inline]
    pub fn get(&self, name: &str) -> Option<Entity> {
        self.ports.get(name).copied()
    }
}

/// A runtime surface for a USD-authored component's physical ports.
///
/// The names and endpoint entities are published by the component projection
/// from authored `inputs:*`/`outputs:*` declarations.  Consumers resolve the
/// authored connection through this surface; they do not discover a wheel,
/// motor, hydraulic valve, or thermal boundary by Rust type or entity name.
#[derive(Component, Debug, Clone, Default)]
pub struct PortSurface {
    /// Authored port name to the runtime [`Port`] entity that carries it.
    pub ports: std::collections::HashMap<String, Entity>,
}

impl PortSurface {
    /// Build a surface from the endpoints projected for one authored component.
    pub fn new(ports: std::collections::HashMap<String, Entity>) -> Self {
        Self { ports }
    }

    /// Resolve one authored port name to its runtime endpoint.
    #[inline]
    pub fn get(&self, name: &str) -> Option<Entity> {
        self.ports.get(name).copied()
    }
}

/// Apply the control lifecycle's safe-stop boundary immediately.
///
/// `InputPorts` are the command request, while the wired Modelica/hardware path
/// reads the derived output [`Port`]s. Waiting for a later producer tick to copy
/// one into the other leaves an actor's final drive demand live after its
/// lease has ended. This operation clears every actuator output now and closes the
/// discrete brake gate when present, so the next co-simulation propagation sees a
/// neutral vehicle regardless of schedule phase.
pub fn safe_stop_control_surface(
    inputs: Option<&mut InputPorts>,
    outputs: Option<&OutputPorts>,
    ports: &mut Query<&mut Port>,
) {
    if let Some(inputs) = inputs {
        inputs.safe_stop();
    }
    let Some(outputs) = outputs else {
        return;
    };
    safe_stop_outputs(outputs, |entity, value| {
        if let Ok(mut port) = ports.get_mut(entity) {
            port.value = value;
        }
    });
}

/// Neutralize all declared control outputs while engaging the discrete brake gate.
///
/// Kept apart from its caller so every lifecycle boundary uses the identical
/// actuator mapping.
fn safe_stop_outputs(outputs: &OutputPorts, mut write: impl FnMut(Entity, f64)) {
    for (name, entity) in &outputs.ports {
        write(*entity, if name == "brake" { 1.0 } else { 0.0 });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_port_defaults() {
        assert_eq!(
            Port::default().value,
            0.0,
            "A port should initialize to zero"
        );
    }

    #[test]
    fn authored_input_defaults_are_preserved_on_the_command_surface() {
        let inputs =
            InputPorts::with_defaults([("brake".to_string(), 1.0), ("throttle".to_string(), 0.0)]);
        assert_eq!(inputs.cmd("brake"), 1.0);
        assert_eq!(inputs.cmd("throttle"), 0.0);
        assert_eq!(inputs.cmd("undeclared"), 0.0);
        assert!(
            !inputs.brake_active,
            "the actuator derives this gate each tick"
        );
    }

    #[test]
    fn safe_stop_neutralizes_inputs_and_derived_actuators() {
        use bevy::ecs::system::RunSystemOnce;

        #[derive(Component)]
        struct StopTarget;

        fn stop_target(
            mut target: Query<(&mut InputPorts, &OutputPorts), With<StopTarget>>,
            mut ports: Query<&mut Port>,
        ) {
            for (mut inputs, actuators) in &mut target {
                safe_stop_control_surface(Some(&mut inputs), Some(actuators), &mut ports);
            }
        }

        let mut world = World::new();
        let left = world.spawn(Port { value: 0.8 }).id();
        let right = world.spawn(Port { value: -0.4 }).id();
        let brake = world.spawn(Port { value: 0.0 }).id();
        let mut inputs = InputPorts::new(&["throttle", "steer", "brake"]);
        inputs.values.insert("throttle".into(), 0.9);
        inputs.values.insert("steer".into(), -0.5);
        let outputs = OutputPorts::new(std::collections::HashMap::from([
            ("drive_left".into(), left),
            ("drive_right".into(), right),
            ("brake".into(), brake),
        ]));

        let target = world.spawn((inputs, outputs, StopTarget)).id();
        world.run_system_once(stop_target).unwrap();
        let inputs = world.get::<InputPorts>(target).unwrap();

        assert_eq!(inputs.cmd("throttle"), 0.0);
        assert_eq!(inputs.cmd("steer"), 0.0);
        assert_eq!(inputs.cmd("brake"), 1.0);
        assert!(inputs.brake_active);
        assert_eq!(world.get::<Port>(left).unwrap().value, 0.0);
        assert_eq!(world.get::<Port>(right).unwrap().value, 0.0);
        assert_eq!(world.get::<Port>(brake).unwrap().value, 1.0);
    }

    /// Intent parsing accepts exactly the authored control vocabulary and rejects
    /// former aliases instead of silently changing their meaning.
    #[test]
    fn parse_user_intent_accepts_only_canonical_names() {
        for (name, expected) in [
            ("forward", UserIntent::MoveForward),
            ("backward", UserIntent::MoveBackward),
            ("left", UserIntent::MoveLeft),
            ("right", UserIntent::MoveRight),
            ("yaw_right", UserIntent::MoveUp),
            ("yaw_left", UserIntent::MoveDown),
            ("speed_boost", UserIntent::SpeedBoost),
            ("action", UserIntent::Action),
            ("thrust", UserIntent::Thrust),
            ("brake", UserIntent::Brake),
            ("release", UserIntent::Release),
            ("switch_mode", UserIntent::SwitchMode),
            ("pause", UserIntent::Pause),
            ("cancel", UserIntent::Cancel),
            ("place_waypoint", UserIntent::PlaceWaypoint),
            ("delete_selection", UserIntent::DeleteSelection),
        ] {
            assert_eq!(parse_user_intent(name), Some(expected), "{name}");
        }
        for old_spelling in [
            "moveforward",
            "movebackward",
            "moveleft",
            "moveright",
            "moveup",
            "movedown",
            "up",
            "down",
            "pitch_up",
            "pitch_down",
            "roll_left",
            "roll_right",
            "arm",
            "fire",
            "detach",
            "eject",
            "decouple",
            "switchmode",
            "back",
            "unpossess",
        ] {
            assert_eq!(parse_user_intent(old_spelling), None, "{old_spelling}");
        }
    }

    #[test]
    fn camera_follow_accepts_only_canonical_tokens() {
        assert_eq!(parse_camera_follow("heading"), Some(CameraFollow::Heading));
        assert_eq!(parse_camera_follow("orbit"), Some(CameraFollow::Orbit));
        assert_eq!(parse_camera_follow("CHASE"), Some(CameraFollow::Chase));
        for old_spelling in [
            "springarm",
            "yaw",
            "stable",
            "external",
            "cockpit",
            "attitude",
            "full",
        ] {
            assert_eq!(parse_camera_follow(old_spelling), None, "{old_spelling}");
        }
    }

    #[test]
    fn control_binding_reports_only_authored_intents() {
        let rover = ControlBinding::from_intent_entries(&[
            ("forward".into(), "throttle".into(), 1.0),
            ("backward".into(), "throttle".into(), -1.0),
            ("left".into(), "steer".into(), -1.0),
        ])
        .expect("rover profile has authored controls");
        assert!(!rover.has_intent(UserIntent::Release));
        assert!(rover.has_intent(UserIntent::MoveForward));

        let lander =
            ControlBinding::from_intent_entries(&[("release".into(), "release".into(), 1.0)])
                .expect("lander profile has an authored release control");
        assert!(lander.has_intent(UserIntent::Release));
    }

    #[test]
    fn control_binding_resolve_preserves_authored_port_order_and_sums() {
        let binding = ControlBinding::from_intent_entries(&[
            ("right".into(), "steer".into(), 0.25),
            ("forward".into(), "throttle".into(), 2.0),
            ("backward".into(), "throttle".into(), -1.0),
            ("left".into(), "steer".into(), 0.5),
        ])
        .expect("valid authored controls");

        assert_eq!(
            binding.resolve(|_| true),
            vec![("steer".into(), 0.75), ("throttle".into(), 1.0)]
        );
    }

    #[test]
    fn control_binding_rejects_malformed_authored_entries() {
        assert!(ControlBinding::from_intent_entries(&[
            ("forward".into(), " ".into(), 1.0),
            ("backward".into(), "throttle".into(), f64::NAN),
            ("not_an_intent".into(), "throttle".into(), 1.0),
        ])
        .is_none());
    }
}
