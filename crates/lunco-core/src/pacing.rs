//! Frame-pacing intent, shared across crates.
//!
//! Winit's `unfocused_mode` is a single global knob that several subsystems have
//! an opinion about, and the last writer each frame wins. `lunco-modelica`'s
//! `sim_focus_pace` re-pegs it every frame (Continuous while a Modelica sim runs,
//! the binary's idle policy otherwise), so any other crate that merely *sets*
//! `WinitSettings` has its choice silently reverted on the next frame.
//!
//! [`KeepAwake`] is how a subsystem states the intent instead of fighting over the
//! knob: whoever paces winit ORs these requests in. It is a counter, not a bool, so
//! overlapping requesters cannot clobber one another — each takes a token and drops
//! it when done.
//!
//! It lives in `lunco-core` because both the requester (`lunco-workbench`'s offline
//! recorder) and the pacer (`lunco-modelica`) depend on core, and neither depends on
//! the other.

use bevy::ecs::entity::EntityHashSet;
use bevy::prelude::*;

/// Outstanding requests to keep the app updating continuously, ignoring the
/// unfocused power-saving throttle.
///
/// The canonical requester is offline frame recording: an unattended capture run
/// has no focused window, and under `reactive_low_power` the app sleeps between
/// redraws, stretching a frame from ~50 ms to whole seconds.
#[derive(Resource, Default, Debug, Clone, Copy)]
pub struct KeepAwake(pub u32);

impl KeepAwake {
    /// Take a token — the app should update continuously until it is released.
    pub fn acquire(&mut self) {
        self.0 = self.0.saturating_add(1);
    }

    /// Release a previously taken token. Saturating, so an unbalanced release
    /// cannot wrap into "everyone wants to stay awake forever".
    pub fn release(&mut self) {
        self.0 = self.0.saturating_sub(1);
    }

    /// Whether anything currently wants continuous updates.
    pub fn wanted(&self) -> bool {
        self.0 > 0
    }
}

/// Fixed-step barrier between live simulation participants.
///
/// A participant may execute its solver off-thread, but the deterministic
/// shared simulation must not advance while the result for its next
/// communication point is in flight. The participant bridge raises `held`
/// before dispatching a step and clears it when the result lands. The time
/// spine projects this state onto `Time<Virtual>`, so SimTick, Rhai,
/// controllers, co-simulation propagation, and Avian share one barrier.
///
/// Barrier membership is supplied by the composed simulation topology. The
/// resource starts unresolved, which is deliberately fail-closed while a scene
/// is still being projected. Once the wiring projection has sealed a topology,
/// only participants in the reverse causal closure of a stateful engine sink
/// hold this barrier. A model that has no such path is still stepped and its
/// outputs are held at communication points, but it cannot stall the shared
/// physics clock.
#[derive(Resource, Default, Debug, Clone, Copy)]
pub struct SimulationBarrier {
    /// Whether the next shared simulation step must wait for a participant result.
    pub held: bool,
    /// Number of live compiled participants measured on the last fixed tick.
    pub active_participants: usize,
    /// Number of live participants whose causal path requires this barrier.
    pub shared_clock_participants: usize,
    /// Largest target/current clock gap on the last fixed tick.
    pub worst_lag_secs: f64,
    /// Participant responsible for `worst_lag_secs`.
    pub worst_entity: Option<Entity>,
}

/// The authoritative set of participants that must synchronize with the
/// shared fixed-step world.
///
/// This is a projection of the resolved simulation graph, not a property of a
/// solver implementation. The USD/co-simulation projection computes the
/// reverse causal closure from stateful sinks (Avian forces, wheel actuators,
/// and joint drives) to their upstream producers. Modelica uses this resource
/// only to decide whether a pending worker result is a shared-clock barrier.
///
/// `topology_ready == false` means the graph is not trustworthy yet. Consumers
/// must then treat every live Modelica participant as coupled. This avoids
/// releasing the world during scene loading merely because the graph has not
/// been projected yet.
#[derive(Resource, Debug, Clone, Default)]
pub struct SimulationBarrierParticipants {
    pub topology_ready: bool,
    pub entities: EntityHashSet,
}

impl SimulationBarrierParticipants {
    #[inline]
    pub fn requires_barrier(&self, entity: Entity) -> bool {
        !self.topology_ready || self.entities.contains(&entity)
    }

    pub fn replace(&mut self, entities: impl IntoIterator<Item = Entity>) {
        self.entities.clear();
        self.entities.extend(entities);
        self.topology_ready = true;
    }
}
