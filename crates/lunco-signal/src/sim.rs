//! Backend-neutral simulation identity and lock-free snapshot publication.
//!
//! A simulation backend publishes immutable snapshots through [`SimStream`].
//! [`SimRegistry`] owns the process-local identity and lifecycle index, while
//! the primary stream storage is keyed by [`SimId`].  Modelica, FMU, and remote
//! replicas can therefore publish through the same substrate without making
//! the signal crate depend on any one solver or runtime.

use std::collections::HashMap;
use std::sync::Arc;

use arc_swap::ArcSwap;
use bevy::prelude::{Entity, Resource};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

/// Maximum samples retained per variable.
pub const DEFAULT_HISTORY_CAPACITY: usize = 2000;

/// Stable identity for one live simulation stream.
///
/// The registry allocates this identity.  It is deliberately independent of a
/// Bevy [`Entity`], because a simulation may outlive its presentation entity or
/// be supplied by a backend that has no entity at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SimId(u64);

impl SimId {
    /// The raw process-local identity, useful for diagnostics and API payloads.
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// One `(time, value)` pair.
#[derive(Debug, Clone, Copy)]
pub struct SimSample {
    pub time: f64,
    pub value: f64,
}

/// Per-variable ring buffer of recent samples.
#[derive(Debug, Clone, Default)]
pub struct VarHistory {
    /// Samples in append order. The oldest entry is dropped at the capacity.
    pub samples: Arc<Vec<SimSample>>,
}

impl VarHistory {
    /// Append `sample`, retaining at most [`DEFAULT_HISTORY_CAPACITY`] entries.
    ///
    /// This copy-on-write operation is intentionally on the producer thread;
    /// readers only load immutable snapshots and never contend with it.
    pub fn append(&self, sample: SimSample) -> VarHistory {
        let mut next: Vec<SimSample> = Vec::with_capacity(self.samples.len().saturating_add(1));
        let overflow = (self.samples.len() + 1).saturating_sub(DEFAULT_HISTORY_CAPACITY);
        next.extend_from_slice(&self.samples[overflow..]);
        next.push(sample);
        VarHistory {
            samples: Arc::new(next),
        }
    }
}

/// Immutable snapshot of one simulation's observable state.
#[derive(Debug, Clone, Default)]
pub struct SimSnapshot {
    /// Simulation time of the most recent sample.
    pub time: f64,
    /// Monotonic publication counter. Resetting a stream creates a new zero
    /// generation at the owner boundary.
    pub generation: u64,
    /// Variable histories in stable declaration/publication order.
    pub vars: IndexMap<String, VarHistory>,
}

impl SimSnapshot {
    /// Build a snapshot by appending one frame of observed values.
    pub fn advance(prev: &SimSnapshot, new_time: f64, outputs: &[(String, f64)]) -> SimSnapshot {
        let mut next_vars: IndexMap<String, VarHistory> = IndexMap::with_capacity(outputs.len());
        for (name, value) in outputs {
            if !value.is_finite() {
                continue;
            }
            let base = prev.vars.get(name).cloned().unwrap_or_default();
            let appended = base.append(SimSample {
                time: new_time,
                value: *value,
            });
            next_vars.insert(name.clone(), appended);
        }
        SimSnapshot {
            time: new_time,
            generation: prev.generation.saturating_add(1),
            vars: next_vars,
        }
    }

    /// Empty initial snapshot used on registration and reset.
    pub fn empty_at_zero() -> SimSnapshot {
        SimSnapshot::default()
    }
}

/// Lock-free handle to one simulation's latest snapshot.
pub type SimStream = Arc<ArcSwap<SimSnapshot>>;

/// Construct an empty stream with a zero-time snapshot.
fn new_sim_stream() -> SimStream {
    Arc::new(ArcSwap::from_pointee(SimSnapshot::empty_at_zero()))
}

/// Registry of live simulation identities and publication streams.
///
/// `SimId` is the authoritative key. An optional entity owner is only lifecycle
/// metadata for the Bevy presentation; it is not the identity exposed to other
/// backends.
#[derive(Resource, Debug)]
pub struct SimRegistry {
    next_id: u64,
    streams: HashMap<SimId, SimEntry>,
}

#[derive(Debug)]
struct SimEntry {
    owner: Option<Entity>,
    stream: SimStream,
}

impl Default for SimRegistry {
    fn default() -> Self {
        Self {
            next_id: 1,
            streams: HashMap::default(),
        }
    }
}

impl SimRegistry {
    /// Return the existing stream for `entity`, or register a new simulation.
    pub fn get_or_insert(&mut self, entity: Entity) -> SimStream {
        if let Some(entry) = self
            .streams
            .values()
            .find(|entry| entry.owner == Some(entity))
        {
            return entry.stream.clone();
        }

        self.register_with_owner(Some(entity)).1
    }

    /// Register a backend-owned simulation that has no Bevy presentation
    /// entity. The returned identity is the key used for later publication and
    /// removal.
    pub fn register(&mut self) -> (SimId, SimStream) {
        self.register_with_owner(None)
    }

    fn register_with_owner(&mut self, owner: Option<Entity>) -> (SimId, SimStream) {
        let id = SimId(self.next_id);
        self.next_id = self
            .next_id
            .checked_add(1)
            .expect("simulation id space exhausted");
        let stream = new_sim_stream();
        self.streams.insert(
            id,
            SimEntry {
                owner,
                stream: stream.clone(),
            },
        );
        (id, stream)
    }

    /// Return the simulation identity associated with a Bevy entity.
    pub fn id_for(&self, entity: Entity) -> Option<SimId> {
        self.streams
            .iter()
            .find_map(|(id, entry)| (entry.owner == Some(entity)).then_some(*id))
    }

    /// Return a stream by its backend-neutral simulation identity.
    pub fn get(&self, id: SimId) -> Option<&SimStream> {
        self.streams.get(&id).map(|entry| &entry.stream)
    }

    /// Remove the simulation associated with a Bevy entity.
    pub fn remove_entity(&mut self, entity: Entity) -> Option<SimId> {
        let id = self.id_for(entity)?;
        self.streams.remove(&id);
        Some(id)
    }

    /// Remove a simulation by its identity.
    pub fn remove(&mut self, id: SimId) -> Option<SimStream> {
        self.streams.remove(&id).map(|entry| entry.stream)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_respects_capacity() {
        let mut history = VarHistory::default();
        for i in 0..(DEFAULT_HISTORY_CAPACITY + 5) {
            history = history.append(SimSample {
                time: i as f64,
                value: i as f64,
            });
        }
        assert_eq!(history.samples.len(), DEFAULT_HISTORY_CAPACITY);
        assert_eq!(history.samples.first().unwrap().time, 5.0);
        assert_eq!(
            history.samples.last().unwrap().time,
            (DEFAULT_HISTORY_CAPACITY + 4) as f64
        );
    }

    #[test]
    fn advance_drops_missing_and_nonfinite_variables() {
        let first = SimSnapshot::advance(
            &SimSnapshot::empty_at_zero(),
            0.1,
            &[
                ("good".into(), 1.0),
                ("nan".into(), f64::NAN),
                ("inf".into(), f64::INFINITY),
            ],
        );
        assert!(first.vars.contains_key("good"));
        assert!(!first.vars.contains_key("nan"));
        assert!(!first.vars.contains_key("inf"));

        let second = SimSnapshot::advance(&first, 0.2, &[("good".into(), 2.0)]);
        assert!(!second.vars.contains_key("nan"));
        assert_eq!(second.vars["good"].samples.len(), 2);
        assert_eq!(second.generation, 2);
    }

    #[test]
    fn registry_uses_sim_id_as_primary_key() {
        let entity = Entity::from_raw_u32(7).unwrap();
        let mut registry = SimRegistry::default();
        let first = registry.get_or_insert(entity);
        let id = registry.id_for(entity).expect("entity is registered");

        assert_eq!(id.get(), 1);
        assert!(std::ptr::eq(
            Arc::as_ptr(&first),
            Arc::as_ptr(registry.get(id).expect("id has a stream"))
        ));
        assert!(registry.remove_entity(entity).is_some());
        assert!(registry.get(id).is_none());
        assert!(registry.id_for(entity).is_none());

        let (backend_id, backend_stream) = registry.register();
        assert_ne!(backend_id, id);
        assert!(std::ptr::eq(
            Arc::as_ptr(&backend_stream),
            Arc::as_ptr(registry.get(backend_id).expect("backend id has a stream"))
        ));
        assert!(registry.remove(backend_id).is_some());
    }

    #[test]
    fn stream_is_lock_free_readable() {
        let stream = new_sim_stream();
        let first = stream.load();
        assert_eq!(first.generation, 0);
        stream.store(Arc::new(SimSnapshot::advance(
            &first,
            1.0,
            &[("v".into(), 42.0)],
        )));
        let second = stream.load();
        assert_eq!(second.generation, 1);
        assert_eq!(second.time, 1.0);
    }
}
