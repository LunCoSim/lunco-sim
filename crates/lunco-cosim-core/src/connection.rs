//! Simulation connections and ports.
//!
//! Follows the FMI/SSP ontology: [`crate::SimConnection`] is a link between two
//! ports (SSP: Connection). The PORTS themselves are not declared by a component
//! here — every participant's port surface is answered by
//! [`lunco_port_core::ports::PortRegistry`], live, from whatever backend owns the
//! value. (A `SimPort`/`SimPorts` metadata pair used to declare them alongside;
//! nothing attached it and nothing read it once the registry landed.)
//!
//! `startElement.startConnector → endElement.endConnector`

use bevy::prelude::*;

/// A connection between two simulation ports.
///
/// Copies the output value of `start_element.start_connector` to
/// the input of `end_element.end_connector` every simulation step.
///
/// ## Port Resolution
///
/// Connector names are resolved by the backend-specific propagation system:
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

/// Persistent input setpoints that outrank the wiring fabric until released.
///
/// # Why a hold, and not just a write
///
/// Writing an input port directly works only while nothing else drives it. The
/// moment that port is a wire's target, the propagation system overwrites it on
/// the next tick: a raw port write reported success, the value
/// lasted under 16 ms, and from the caller's side that is indistinguishable from
/// a port that does not exist. Every "I set the throttle and nothing happened"
/// report has this shape.
///
/// Control and authored-program writes are held separately. A live control hold
/// takes precedence over a program hold, which takes precedence over wiring. The
/// propagation master applies the selected value in place of the accumulated
/// value; the accumulator itself is untouched.
///
/// A control intent is a level, not a pulse. It remains the latest value until
/// the owner explicitly releases it or possession is released. Authored program
/// holds survive possession changes so a running autopilot keeps its setpoints.
#[derive(Resource, Debug, Default)]
pub struct PortHolds {
    /// `(entity, port) → control and authored-program setpoints`.
    /// Names are indexed inside their entity so fixed-step readers can look up
    /// a borrowed `&str` without constructing an owned tuple key.
    holds: std::collections::HashMap<Entity, std::collections::HashMap<String, PortHold>>,
    /// Changes only when a live intent is added, changed, or removed. The
    /// propagation cache uses this to rebuild held target indices off the
    /// steady fixed-tick path.
    revision: u64,
}

#[derive(Clone, Copy, Debug, Default)]
struct PortHold {
    control: Option<f64>,
    program: Option<f64>,
}

impl PortHold {
    fn resolved(self) -> Option<f64> {
        self.control.or(self.program)
    }

    fn is_empty(self) -> bool {
        self.control.is_none() && self.program.is_none()
    }
}

impl PortHolds {
    /// Set a persistent control intent for `port` on `entity`.
    pub fn hold(&mut self, entity: Entity, port: impl Into<String>, value: f64) {
        self.set(entity, port.into(), value, false);
    }

    /// Set an authored program setpoint for `port` on `entity`.
    pub fn hold_program(&mut self, entity: Entity, port: impl Into<String>, value: f64) {
        self.set(entity, port.into(), value, true);
    }

    fn set(&mut self, entity: Entity, port: String, value: f64, program: bool) {
        let ports = self.holds.entry(entity).or_default();
        let hold = ports.entry(port).or_default();
        let current = if program {
            &mut hold.program
        } else {
            &mut hold.control
        };
        if current.is_some_and(|current| current.to_bits() == value.to_bits()) {
            return;
        }
        *current = Some(value);
        self.bump_revision();
    }

    /// Read one live intent by borrowed port name.
    #[inline]
    pub fn get(&self, entity: Entity, port: &str) -> Option<f64> {
        self.holds
            .get(&entity)
            .and_then(|ports| ports.get(port))
            .and_then(|hold| hold.resolved())
    }

    /// Iterate active intents without cloning names. The propagation engine
    /// uses this to apply holds only to matching compiled targets instead of
    /// hashing every target against the hold table on every physics tick.
    pub fn iter(&self) -> impl Iterator<Item = (Entity, &str, f64)> + '_ {
        self.holds.iter().flat_map(|(&entity, ports)| {
            ports.iter().filter_map(move |(name, hold)| {
                hold.resolved().map(|value| (entity, name.as_str(), value))
            })
        })
    }

    /// End a hold early. `true` if one was live.
    pub fn release(&mut self, entity: Entity, port: &str) -> bool {
        let Some(ports) = self.holds.get_mut(&entity) else {
            return false;
        };
        let released = ports.remove(port).is_some();
        if ports.is_empty() {
            self.holds.remove(&entity);
        }
        if released {
            self.bump_revision();
        }
        released
    }

    /// Release every persisted intent addressed to `entity`.
    pub fn clear_entity(&mut self, entity: Entity) {
        if self.holds.remove(&entity).is_some() {
            self.bump_revision();
        }
    }

    /// Clear only operator/controller inputs for an endpoint. Authored program
    /// setpoints remain active through an authority handoff.
    pub fn clear_control_entity(&mut self, entity: Entity) {
        let Some(ports) = self.holds.get_mut(&entity) else {
            return;
        };
        let mut released = false;
        for hold in ports.values_mut() {
            if hold.control.take().is_some() {
                released = true;
            }
        }
        if !released {
            return;
        }
        ports.retain(|_, hold| !hold.is_empty());
        if ports.is_empty() {
            self.holds.remove(&entity);
        }
        self.bump_revision();
    }

    /// Clear every intent at a scene boundary while preserving the revision
    /// sequence observed by the propagation cache.
    pub fn clear_all(&mut self) {
        if !self.holds.is_empty() {
            self.holds.clear();
            self.bump_revision();
        }
    }

    /// Current invalidation generation for cached hold-to-target indices.
    #[inline]
    pub fn revision(&self) -> u64 {
        self.revision
    }

    #[inline]
    fn bump_revision(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }

    /// Return the names of every persisted intent addressed to `entity`.
    pub fn entity_port_names(&self, entity: Entity) -> Vec<String> {
        self.holds
            .get(&entity)
            .into_iter()
            .flat_map(|ports| ports.keys().cloned())
            .collect()
    }

    /// Return every entity with at least one persisted control intent.
    /// The result has no stable order; owners admitting ordered actions must
    /// sort by the entities' authoritative identities.
    pub fn held_entities(&self) -> Vec<Entity> {
        self.holds.keys().copied().collect()
    }

    /// Copy live holds into the flat view consumed by presentation code.
    pub fn snapshot(&self) -> std::collections::HashMap<(Entity, String), f64> {
        self.holds
            .iter()
            .flat_map(|(entity, ports)| {
                ports.iter().filter_map(move |(name, hold)| {
                    hold.resolved()
                        .map(|value| ((*entity, name.clone()), value))
                })
            })
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.holds.is_empty()
    }
}

#[cfg(test)]
mod port_hold_tests {
    use super::PortHolds;
    use bevy::prelude::World;

    #[test]
    fn hold_revision_tracks_only_effective_intent_changes() {
        let mut world = World::new();
        let entity = world.spawn_empty().id();
        let mut holds = PortHolds::default();

        holds.hold(entity, "throttle", 0.25);
        let first = holds.revision();
        assert_eq!(first, 1);
        holds.hold(entity, "throttle", 0.25);
        assert_eq!(
            holds.revision(),
            first,
            "same intent keeps cached slots valid"
        );

        holds.hold(entity, "throttle", 0.5);
        assert_ne!(holds.revision(), first);
        assert!(holds.release(entity, "throttle"));
        let released = holds.revision();
        assert!(!holds.release(entity, "throttle"));
        assert_eq!(holds.revision(), released);

        holds.hold(entity, "throttle", 0.75);
        holds.clear_all();
        assert!(holds.is_empty());
        assert!(holds.revision() > released);
    }

    #[test]
    fn control_release_reveals_authored_program_setpoint() {
        let mut world = World::new();
        let entity = world.spawn_empty().id();
        let mut holds = PortHolds::default();

        holds.hold_program(entity, "throttle", 0.4);
        holds.hold(entity, "throttle", 0.9);
        assert_eq!(holds.get(entity, "throttle"), Some(0.9));

        holds.clear_control_entity(entity);

        assert_eq!(holds.get(entity, "throttle"), Some(0.4));
        assert_eq!(
            holds.iter().collect::<Vec<_>>(),
            vec![(entity, "throttle", 0.4)]
        );
        assert!(!holds.is_empty());
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
