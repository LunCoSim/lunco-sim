//! Shared ECS endpoint and control-surface contracts.
//!
//! These components are the generic runtime surface exchanged by co-simulation,
//! physics, authored USD projection, hardware, telemetry, API, and scripting.
//! The registry that discovers and accesses them lives beside them in
//! [`crate::ports`]. They are deliberately independent of the general engine
//! core so a port-bearing package does not pull the engine's unrelated scene,
//! identity, and command substrate merely to describe a scalar endpoint.

use bevy::prelude::*;

/// Register endpoint markers used by reflected commands and API schemas.
///
/// The general engine plugin registers only engine-owned types. Hosts that
/// install the port substrate call this once from their port/wiring plugin so
/// the reflection ownership follows the component ownership.
pub fn register_endpoint_types(app: &mut App) {
    app.register_type::<Port>()
        .register_type::<CausalStateSink>();
}

/// A named signal value exchanged between subsystems.
///
/// One port type carries every signal in the simulation — commands from the
/// control surface, actuator setpoints consumed by the physics solvers, sensor
/// readings, and the values a Modelica co-simulation exchanges. Values are
/// `f64` in whatever unit the signal is authored in; a
/// `lunco_cosim_core::SimConnection` applies factor/offset when two ports are
/// expressed in different units.
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
/// projected. A `SphereLight`, for example, is first represented by the USD
/// prim and only then receives its Bevy `PointLight`/`SpotLight` component.
/// Co-simulation binding must be notified at the moment that component-backed
/// surface exists; otherwise a wire can be checked once, classified as
/// missing, and never reconsidered. This marker is the dependency-neutral
/// lifecycle contract: the producer of a port surface adds it, while the wire
/// engine observes it. It is intentionally not light-specific so the same
/// contract works for any deferred scene-property backend.
#[derive(Component, Debug, Clone, Copy, Default, Reflect)]
#[reflect(Component)]
pub struct PortSurfaceReady;

/// Marks an entity while a deferred port backend is still being installed.
///
/// A scene projection may author a wire in the same epoch in which its target
/// component is spawned. The binder must keep that edge pending across an
/// epoch seal while the producer finishes installing its surface; otherwise a
/// valid wire becomes a terminal missing-port fault merely because component
/// insertion and wire projection were observed in different schedules.
#[derive(Component, Debug, Clone, Copy, Default, Reflect)]
#[reflect(Component)]
pub struct PortSurfacePending;

/// An entity's declared **`inputs:*` port surface**, with current values.
///
/// The input vocabulary is data — the keys present here declare exactly which
/// input ports this entity accepts, so the port backend stays strict (an
/// undeclared name is rejected and reported as a dangling wire). A rover may
/// expose `throttle`/`steer`/`brake`, an avatar `forward`/`side`/`up` plus the
/// normalized `speed_boost` modifier, and a factory `start_cycle`/`target_rate`.
/// Inputs are scalar `f64`s: an intent such as `Action` or `Thrust` normally
/// produces a binary `0.0`/`1.0` input, while analog control can supply any
/// normalized or physical value. The keys are seeded from an authored control
/// binding for USD vessels; runtime-built endpoints may declare the same
/// surface directly with [`InputPorts::new`]. The surface, not the optional
/// binding, is the command endpoint.
///
/// Written through the shared port substrate (`SetPorts` → `PortRegistry`) and
/// consumed by the authored mechanical controller or free-flight realization.
///
/// The command port named `"brake"` here is not the output port named `"brake"`
/// in [`OutputPorts`]. They carry different values — an analog command in
/// `[-1, 1]` here, a discretized `1.0`/`0.0` gate there — and are deliberately
/// kept in two components so the two `"brake"`s can never be conflated.
#[derive(Component, Debug, Clone, Default)]
pub struct InputPorts {
    /// Current value per accepted input-port name. Only seeded keys are
    /// writable; see the type docs.
    pub values: std::collections::HashMap<String, f64>,
    /// Derived brake state, cached from `values["brake"] > 0.5` by the actuator
    /// so per-tick physics systems read a bool without a map lookup.
    pub brake_active: bool,
}

impl InputPorts {
    /// Build with a seeded command vocabulary: the input-port names this
    /// vehicle accepts, each initialised to `0.0`.
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

    /// Current value of command input `name` (`0.0` if this vehicle does not
    /// accept it). The read side of the input surface for actuators.
    #[inline]
    pub fn cmd(&self, name: &str) -> f64 {
        self.values.get(name).copied().unwrap_or(0.0)
    }

    /// Move the logical command surface to its safe state without inventing
    /// undeclared ports. Braking is a rover-specific convention; every other
    /// declared command is neutralized so lander thrust/attitude and RCS
    /// commands cannot survive a release.
    pub fn safe_stop(&mut self) {
        for (name, value) in &mut self.values {
            *value = if name == "brake" { 1.0 } else { 0.0 };
        }
        self.brake_active = self.values.get("brake").is_some_and(|v| *v > 0.5);
    }
}

/// The [`InputPorts`] governing `entity` — its own, or the nearest ancestor's.
///
/// A command surface belongs to the vessel, and a part is not always a child
/// of it. On an articulated rover a wheel hangs off a rocker link, so the
/// wheel's carrier body is a suspension member with no command surface of its
/// own. Walking up terminates at the vessel because only vessels carry
/// `InputPorts`.
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
/// This is the produced-value half of a control surface, and is different
/// from [`InputPorts`]: those are the logical input values a human or script
/// issues, while these are runtime endpoints written by the authored
/// Modelica/Rhai controller network. The names and topology still come from
/// authored USD `outputs:*` attributes; this component only stores the runtime
/// endpoint for each one.
#[derive(Component, Debug, Clone, Default)]
pub struct OutputPorts {
    /// Maps authored output names (for example, `"drive_left"`) to their
    /// [`Port`] entity.
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
/// from authored `inputs:*`/`outputs:*` declarations. Consumers resolve the
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
/// one into the other leaves an actor's final drive demand live after its lease
/// has ended. This operation clears every actuator output now and closes the
/// discrete brake gate when present, so the next co-simulation propagation sees
/// a neutral vehicle regardless of schedule phase.
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
fn safe_stop_outputs(outputs: &OutputPorts, mut write: impl FnMut(Entity, f64)) {
    for (name, entity) in &outputs.ports {
        write(*entity, if name == "brake" { 1.0 } else { 0.0 });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn port_defaults_to_zero() {
        assert_eq!(Port::default().value, 0.0);
    }

    #[test]
    fn authored_input_defaults_are_preserved_on_the_command_surface() {
        let inputs =
            InputPorts::with_defaults([("brake".to_string(), 1.0), ("throttle".to_string(), 0.0)]);
        assert_eq!(inputs.cmd("brake"), 1.0);
        assert_eq!(inputs.cmd("throttle"), 0.0);
        assert_eq!(inputs.cmd("undeclared"), 0.0);
        assert!(!inputs.brake_active);
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
}
