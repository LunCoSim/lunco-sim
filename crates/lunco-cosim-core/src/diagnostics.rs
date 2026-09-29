//! Co-sim connection diagnostics — the machine-readable form of the wiring
//! log lines that the co-simulation propagation system emits.
//!
//! The interaction report asked for `GET /api/diagnostics` so a caller can *poll*
//! unresolved connections instead of scraping the terminal. The log line and this
//! resource are the same fact in two forms; this one is refreshed every
//! propagation tick so a poller always sees the current fabric, not a stale
//! snapshot.
//!
//! It distinguishes work which is still waiting for an endpoint contract from a
//! terminal wiring failure. A generated Modelica island deliberately exists while
//! it is compiling; its port interface is not final until that lifecycle stage
//! completes. Treating an early partial surface as a typo made load order look
//! like an authoring fault.
//!
//! * [`CosimDiagnostics::pending`] holds structural endpoints and endpoints
//!   whose [`crate::SimComponent`] is still compiling. It is an observation, not
//!   a warning or a test failure.
//! * [`CosimDiagnostics::broken`] holds only terminal failures: a ready (or
//!   failed) endpoint that still cannot accept the named input.
//!
//! Every rejected write keeps its owner-supplied reason, including range and
//! writability failures. Pending endpoints retain the latest resolution reason
//! without being promoted to terminal faults.

use bevy::prelude::*;
use lunco_core::GlobalEntityId;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// One connection target that did not accept its write on the last propagation
/// tick. Rebuilt every tick by the co-simulation propagation system.
#[derive(Debug, Clone)]
pub struct BrokenConnection {
    /// The target entity whose input port could not be written.
    pub entity: Entity,
    /// Its stable id, when assigned — the identity an API caller can address.
    pub global_id: Option<GlobalEntityId>,
    /// The input port name the wire targeted.
    pub port: Arc<str>,
    /// Whether the entity exposes any port surface. `true` = genuine fault (has
    /// ports, not this one); `false` = structural/still-loading endpoint.
    pub has_port_surface: bool,
    /// The accumulated value that was dropped (what the source(s) resolved to).
    pub dropped_value: f64,
    /// Owner-supplied rejection reason, including range and writability details.
    pub failure: Option<String>,
}

/// A force-producing feedback cycle in the current co-simulation fabric.
///
/// This is deliberately separate from [`BrokenConnection`]. Explicit causal
/// feedback that does not reach a force or torque input is valid dynamic
/// behavior and is not included here. A force-producing cycle remains visible
/// so the client-prediction safety contract cannot be mistaken for a missing
/// input port. Mixing the two makes a healthy scenario look like it has a
/// dangling wire and makes API/test consumers guess from a synthetic port
/// name.
#[derive(Debug, Clone)]
pub struct AlgebraicLoopDiagnostic {
    /// Canonical participant chosen for this loop's stable identity.
    pub entity: Entity,
    /// Stable id, when assigned.
    pub global_id: Option<GlobalEntityId>,
    /// Deterministic description of the wires in the cycle.
    pub detail: String,
    /// The cycle reaches a physics force or torque input.
    pub force_producing: bool,
    /// The cycle is rejected because it reaches a client-predicted body
    /// without the program's explicit realtime-safety promise.
    pub rejected: bool,
}

/// The live set of unresolved connection targets, refreshed every propagation
/// tick. Empty when every wire resolves. Read by the API's `GetBrokenConnections`
/// query (registered in `lunco-usd-sim`, which sees both this crate and the API).
///
/// **Two questions, two fields.** A poller asks *"what is broken right now"* and
/// wants [`broken`](Self::broken), which clears itself when a wire resolves. A
/// gate asks *"did anything ever fail to land"* and cannot use that: propagation
/// is CHANGE-DRIVEN, so a wire that dropped its write at load is not re-attempted
/// on a quiet tick and the live set reads empty a second later. A scene test that
/// sampled `broken` at verdict time therefore passed a run whose rover was never
/// actuated — the failure had happened, been reported, and been overwritten.
///
/// [`faults`](Self::faults) is the record of what happened, so the answer does
/// not depend on when it is asked.
#[derive(Resource, Debug, Default)]
pub struct CosimDiagnostics {
    /// Targets waiting for their endpoint contract. Rebuilt each propagation
    /// tick; never logged as a wiring fault.
    pub pending: Vec<BrokenConnection>,
    /// Targets that dropped their write after their endpoint contract became
    /// terminal. Rebuilt each propagation tick.
    pub broken: Vec<BrokenConnection>,
    /// Force-producing feedback cycles in the current wiring fabric. Ordinary
    /// causal cycles are valid dynamic feedback and are not included. These
    /// entries are not missing ports and therefore never enter [`Self::faults`].
    pub algebraic_loops: Vec<AlgebraicLoopDiagnostic>,
    /// Wires that have NEVER successfully written, indexed by entity then port
    /// name so a wire that drops on a thousand ticks is one entry and successful
    /// ticks can look up borrowed names without allocating tuple keys.
    ///
    /// Only terminal targets are recorded. A compiling model, or an endpoint
    /// with no runtime port surface, remains pending rather than manufacturing a
    /// failure during scene assembly.
    ///
    /// **A wire that later lands RETRACTS its entry**, and once landed it can
    /// never be re-reported (see [`landed`](Self::landed)). Dropping a write
    /// before the endpoint is ready is not an authoring error, it is load order:
    /// a joint's `angle` port exists only once avian has admitted both its bodies
    /// into the island graph, which is a documented multi-frame window every
    /// jointed mechanism passes through. Recording those permanently made every
    /// antenna in the project look broken while `rocker_bogie`'s own scenario
    /// measured the joint working.
    ///
    /// What survives is the wire that never landed at all — the Modelica drive
    /// law writing a port no rover declares, the antenna joint that never
    /// attaches. That is the authoring error, and it is what a gate must fail on.
    ///
    /// Another entry source shares this ledger: `SetPorts` writes to a name the
    /// target's port surface doesn't declare (M12) — the same `(entity, port)`
    /// key and landed-retraction rules as wires. Causal feedback cycles are not
    /// ledger entries: they are valid explicit co-simulation topology, while
    /// acausal islands require a typed backend partition before stepping.
    pub faults: HashMap<Entity, HashMap<Arc<str>, BrokenConnection>>,
    /// `(entity, port)` pairs proven wired by at least one successful write.
    ///
    /// Needed because propagation is CHANGE-DRIVEN and cannot be re-asked: a
    /// quiet tick writes nothing, so "is this wire fine *now*" has no answer.
    /// "Has this wire ever carried a value" does, and it only ever ratchets one
    /// way, which is what makes the gate order-independent.
    pub landed: HashMap<Entity, HashSet<Arc<str>>>,
    /// Runtime warning keys for the current scene. This is deliberately owned
    /// by the scene diagnostics resource, rather than a process-lifetime
    /// system-local cache, so a replacement scene can report the same port
    /// name again.
    pub(crate) reported: std::collections::HashSet<String>,
}

impl CosimDiagnostics {
    /// Record a diagnostic key and return whether it was new in this scene.
    pub fn report_once(&mut self, key: impl Into<String>) -> bool {
        self.reported.insert(key.into())
    }

    /// Whether a port has accepted a write at least once this scene.
    #[inline]
    pub fn has_landed(&self, entity: Entity, port: &str) -> bool {
        self.landed
            .get(&entity)
            .is_some_and(|ports| ports.contains(port))
    }

    /// Record a first successful write using a borrowed public port name.
    /// Repeated successful ticks do not allocate or clone the name.
    pub fn mark_landed(&mut self, entity: Entity, port: &str) -> bool {
        if self.has_landed(entity, port) {
            return false;
        }
        self.landed
            .entry(entity)
            .or_default()
            .insert(Arc::from(port));
        true
    }

    /// Record a first successful write while sharing the compiled target name.
    pub fn mark_landed_shared(&mut self, entity: Entity, port: &Arc<str>) -> bool {
        if self.has_landed(entity, port) {
            return false;
        }
        self.landed
            .entry(entity)
            .or_default()
            .insert(Arc::clone(port));
        true
    }

    /// Whether a terminal failed-write record exists for this borrowed name.
    #[inline]
    pub fn has_fault(&self, entity: Entity, port: &str) -> bool {
        self.faults
            .get(&entity)
            .is_some_and(|ports| ports.contains_key(port))
    }

    /// Insert a terminal target fault once, sharing its indexed name only when
    /// this fault first appears.
    pub fn record_fault(&mut self, fault: BrokenConnection) -> bool {
        if self.has_fault(fault.entity, &fault.port) {
            return false;
        }
        self.faults
            .entry(fault.entity)
            .or_default()
            .insert(Arc::clone(&fault.port), fault);
        true
    }

    /// Remove a terminal fault when the same endpoint later accepts a write.
    pub fn remove_fault(&mut self, entity: Entity, port: &str) -> Option<BrokenConnection> {
        let (removed, empty) = {
            let ports = self.faults.get_mut(&entity)?;
            let removed = ports.remove(port);
            (removed, ports.is_empty())
        };
        if empty {
            self.faults.remove(&entity);
        }
        removed
    }

    /// Forget all port diagnostics for an entity that left the world.
    pub fn forget_entity(&mut self, entity: Entity) {
        self.faults.remove(&entity);
        self.landed.remove(&entity);
    }

    /// Iterate terminal faults without exposing the entity-indexed storage.
    pub fn fault_entries(&self) -> impl Iterator<Item = &BrokenConnection> {
        self.faults.values().flat_map(|ports| ports.values())
    }

    /// Number of terminal faults across all entities.
    pub fn fault_count(&self) -> usize {
        self.faults.values().map(HashMap::len).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::{BrokenConnection, CosimDiagnostics};
    use bevy::prelude::*;
    use std::sync::Arc;

    #[test]
    fn borrowed_port_keys_record_and_retract_one_fault() {
        let mut world = World::new();
        let entity = world.spawn_empty().id();
        let mut diagnostics = CosimDiagnostics::default();
        let fault = BrokenConnection {
            entity,
            global_id: None,
            port: Arc::from("demand"),
            has_port_surface: true,
            dropped_value: 3.5,
            failure: Some("value must be ≤ 1".into()),
        };

        assert!(diagnostics.record_fault(fault.clone()));
        assert!(!diagnostics.record_fault(fault));
        assert!(diagnostics.has_fault(entity, "demand"));
        assert_eq!(diagnostics.fault_count(), 1);

        assert!(diagnostics.mark_landed(entity, "demand"));
        assert!(!diagnostics.mark_landed(entity, "demand"));
        assert!(diagnostics.remove_fault(entity, "demand").is_some());
        assert!(!diagnostics.has_fault(entity, "demand"));
        assert!(diagnostics.has_landed(entity, "demand"));
        assert_eq!(diagnostics.fault_count(), 0);
    }
}
