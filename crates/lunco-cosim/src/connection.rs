//! Simulation connections and ports.
//!
//! Follows the FMI/SSP ontology: [`crate::SimConnection`] is a link between two
//! ports (SSP: Connection). The PORTS themselves are not declared by a component
//! here — every participant's port surface is answered by
//! [`lunco_core::ports::PortRegistry`], live, from whatever backend owns the
//! value. (A `SimPort`/`SimPorts` metadata pair used to declare them alongside;
//! nothing attached it and nothing read it once the registry landed.)
//!
//! `startElement.startConnector → endElement.endConnector`

use bevy::prelude::*;

// Port causality/domain enums live in the neutral substrate so every participant
// (engine, API, scripting) shares one definition; re-exported here because this
// crate's `SimPort` and the avian backends address them as `connection::Port*`.
pub use lunco_core::ports::PortDirection;

/// A connection between two simulation ports.
///
/// Copies the output value of `start_element.start_connector` to
/// the input of `end_element.end_connector` every simulation step.
///
/// ## Port Resolution
///
/// Connector names are resolved by [`propagate_connections`](crate::systems::propagate::propagate_connections):
///
/// - `"netForce"`, `"volume"`, etc. → [`crate::SimComponent`](crate::SimComponent) outputs
/// - `"position_y"`, `"force_y"`, etc. → Avian rigid-body outputs/inputs
///
/// ## Example
///
/// ```rust,ignore
/// commands.spawn(SimConnection {
///     start_element: balloon_entity,
///     start_connector: "netForce".into(),
///     end_element: balloon_entity,
///     end_connector: "force_y".into(),
///     scale: 1.0,
///     offset: 0.0,
/// });
/// ```
///
/// ## Affine transform (SSP `LinearTransformation`)
///
/// The propagated value is `source * scale + offset`. `scale` is the SSP
/// connection *factor* and `offset` the SSP *offset* — together they express
/// unit conversions (Celsius↔Kelvin), sensor zero-points, and actuator gains
/// (e.g. a normalized command port → physical units). `offset` defaults to
/// `0.0` so pure-gain wires need not name it.
#[derive(Component, Debug, Clone, Reflect)]
#[reflect(Component)]
pub struct SimConnection {
    /// Entity owning the source port.
    pub start_element: Entity,
    /// Name of the source port.
    pub start_connector: String,
    /// The source is the endpoint's **input** (commanded) side, not its output.
    ///
    /// USD says which by namespace: `</Rover.outputs:speed>` reads what the vessel
    /// PRODUCES, `</Rover.inputs:throttle>` reads what it was COMMANDED. Both are
    /// legitimate sources — a drive law consumes the vessel's throttle command —
    /// and the two can share a name on one entity (a joint's commanded setpoint and
    /// its measured angle are both `angle`), which is exactly why
    /// `PortRegistry::read_input_port` exists alongside `read_output_port`.
    ///
    /// Default `false` keeps every existing wire reading outputs. Before this flag,
    /// the wiring pass accepted an authored `inputs:` source and propagation then
    /// read it with `read_output_port`, which input-only backends answer `None` to —
    /// so the wire silently contributed nothing, forever, with no diagnostic.
    pub start_is_input: bool,
    /// Entity owning the target port.
    pub end_element: Entity,
    /// Name of the target port (must be an input).
    pub end_connector: String,
    /// Multiplicative factor applied during propagation (SSP factor).
    pub scale: f64,
    /// Additive offset applied after scaling (SSP offset). `value = src*scale + offset`.
    pub offset: f64,
}

impl Default for SimConnection {
    fn default() -> Self {
        Self {
            start_element: Entity::PLACEHOLDER,
            start_connector: String::new(),
            start_is_input: false,
            end_element: Entity::PLACEHOLDER,
            end_connector: String::new(),
            scale: 1.0,
            offset: 0.0,
        }
    }
}

/// Manual setpoints that OUTRANK the wiring fabric, until they expire.
///
/// # Why a hold, and not just a write
///
/// Writing an input port directly works only while nothing else drives it. The
/// moment that port is a wire's target, [`crate::systems::propagate::propagate_connections`]
/// overwrites it on the next tick: a raw port write reported success, the value
/// lasted under 16 ms, and from the caller's side that is indistinguishable from
/// a port that does not exist. Every "I set the throttle and nothing happened"
/// report has this shape.
///
/// So a manual write is a HOLD: latest-wins, addressed by `(entity, port)`, and
/// applied by the propagation master in place of the accumulated value while it
/// is live. The accumulator itself is untouched — a hold suppresses a wire, it
/// does not corrupt the sum feeding other targets.
///
/// # Why it expires
///
/// An indefinite hold is a scene that silently stops responding to its own
/// wiring, and the only way back is a caller that remembers to release. A
/// deadline makes the default outcome *recovery*: a script that crashes, an API
/// client that disconnects, a test that forgets to clean up all end with the
/// vehicle back under its own control. [`DEFAULT_HOLD_SECS`] is the timeout when
/// a caller does not state one; `release` ends it early, and re-setting the same
/// port extends it (latest-wins, so a 10 Hz stream of setpoints simply keeps its
/// hold alive).
#[derive(Resource, Debug, Default)]
pub struct PortHolds {
    /// `(entity, port) → (value, expiry on the REAL clock)`.
    holds: std::collections::HashMap<(Entity, String), (f64, f64)>,
}

/// A one-fixed-tick lifecycle fence for deferred control writes.
///
/// `SetPorts` reaches its backend through a command-world closure. A control
/// owner can be retired after it emitted a trigger but before that closure runs;
/// without this fence the obsolete write can outlive the owner and re-arm its
/// hold. Lifecycle handlers block the endpoint synchronously, and
/// [`clear_control_write_fence`] opens it at the next fixed-tick boundary.
#[derive(Resource, Debug, Default)]
pub struct ControlWriteFence {
    blocked: std::collections::HashSet<Entity>,
}

impl ControlWriteFence {
    /// Reject deferred writes to `entity` until the next fixed tick.
    pub fn block(&mut self, entity: Entity) {
        self.blocked.insert(entity);
    }

    /// Whether a lifecycle boundary currently rejects writes to `entity`.
    pub fn blocks(&self, entity: Entity) -> bool {
        self.blocked.contains(&entity)
    }

    fn clear(&mut self) {
        self.blocked.clear();
    }
}

/// Open control endpoints at the start of the next fixed tick.
pub fn clear_control_write_fence(mut fence: ResMut<ControlWriteFence>) {
    fence.clear();
}

/// How long a hold lasts when the caller does not say.
///
/// Long enough to drive interactively at human rates (a slider, a keypress
/// repeat, a 1 Hz script) and short enough that an abandoned hold is a hiccup
/// rather than a stuck vehicle.
pub const DEFAULT_HOLD_SECS: f64 = 2.0;

impl PortHolds {
    /// Hold `port` on `entity` at `value` until `now + secs`.
    pub fn hold(&mut self, entity: Entity, port: impl Into<String>, value: f64, until: f64) {
        self.holds.insert((entity, port.into()), (value, until));
    }

    /// End a hold early. `true` if one was live.
    pub fn release(&mut self, entity: Entity, port: &str) -> bool {
        self.holds.remove(&(entity, port.to_string())).is_some()
    }

    /// Drop everything that has expired. Called once per propagation tick.
    pub fn expire(&mut self, now: f64) {
        self.holds.retain(|_, (_, until)| *until > now);
    }

    /// The live holds, for the propagation master's per-target lookup.
    pub fn snapshot(&self) -> std::collections::HashMap<(Entity, String), f64> {
        self.holds
            .iter()
            .map(|(key, (value, _))| (key.clone(), *value))
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.holds.is_empty()
    }
}

/// **A program's promise that it is fast enough to be trusted with a force** —
/// `docs/architecture/28-modelica-realtime-physics.md` §2.
///
/// Declared in USD as `lunco:program:realtimeSafe = true`, **never inferred**.
/// Only a program carrying it may drive an avian `force_*` / `torque_*` port on a
/// client-**predicted** `Dynamic` body: that requires a deterministic,
/// bounded-cost step — the same stop-times and the same work on every peer, every
/// tick. A model that takes 40ms to step, wired into a predicted body, diverges
/// from the server every frame it is late.
///
/// Absent is the default and means "not promised", which the wiring pass refuses a
/// force port (`lunco-usd-sim`'s `rewire_usd_connections`). Programs that never
/// touch physics — a supervisory script, a battery model — simply never declare it;
/// they are free to be stiff, adaptive, and slow, because state coupling cannot
/// desync a predicted body.
///
/// It is not a quality rating, and there is nothing below it: whether a program is
/// stepped in the live loop at all is decided by whether a live scene references it.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Reflect)]
#[reflect(Component)]
pub struct RealtimeSafe;

/// Is `port` an avian force/torque input — i.e. does writing it push a rigid
/// body around? These are the ONLY ports whose writer can desync a
/// client-predicted body, so they are what the [`RealtimeSafe`] gate guards.
///
/// The sets are declared beside the port tables that implement them — NOT
/// matched by spelling here.
pub fn is_physics_force_port(port: &str) -> bool {
    crate::avian::BODY_FORCE_PORTS.contains(&port)
        || crate::avian::ACTUATOR_FORCE_PORTS.contains(&port)
}

#[cfg(test)]
mod realtime_gate_tests {
    use super::*;

    #[test]
    fn force_ports_are_the_gated_ones() {
        assert!(is_physics_force_port("force_y"));
        assert!(is_physics_force_port("torque_z"));
        // Body-frame thrust pushes a body just as hard as world-frame thrust.
        assert!(is_physics_force_port("force_local_x"));
        assert!(!is_physics_force_port("throttle"));
        assert!(is_physics_force_port("force_command"));
        assert!(is_physics_force_port("torque_command"));
        assert!(!is_physics_force_port("angle"));
        // A gearbox's MECHANICAL shaft torque is not a body force: it drives a
        // reduction, not a rigid body, so it must not demand a realtime promise.
        assert!(!is_physics_force_port("torque"));
    }

    /// Tripwire: a body-force port added to the avian table but not declared in
    /// [`crate::avian::BODY_FORCE_PORTS`] would go UNGATED and silently. This
    /// cannot see through the write closures, so it uses the naming convention
    /// as a heuristic alarm — if you add a conventionally-named force port,
    /// declare it (or, if it genuinely does not touch a body, rename it).
    #[test]
    fn conventionally_named_force_ports_are_all_declared() {
        for group in crate::ports::AVIAN {
            for p in group.ports {
                let looks_like_force =
                    p.name.starts_with("force_") || p.name.starts_with("torque_");
                if looks_like_force {
                    assert!(
                        is_physics_force_port(p.name),
                        "avian port `{}` looks like a body-force port but is not in \
                         BODY_FORCE_PORTS — it would bypass the RealtimeSafe gate",
                        p.name
                    );
                }
            }
        }
    }
}
