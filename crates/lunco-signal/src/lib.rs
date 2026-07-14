//! Signal model: what data looks like *before* any viz touches it.
//!
//! A signal is a typed, time-varying datum identified by a [`SignalRef`]. Producers
//! (the Modelica worker, the telemetry sampler, the Avian bridge, script-defined
//! derived signals) push samples into [`SignalRegistry`]; viz kinds read from the same
//! registry.
//!
//! This layer knows nothing about Modelica, Avian, or plotting. A signal is just
//! (`ref`, `type`, `history`) plus optional metadata.
//!
//! # Why this is its own crate
//!
//! It used to live in `lunco-viz`, which links `bevy_egui → bevy_render → wgpu`. But a
//! ring buffer of `f64`s is **data**, not rendering, and a headless `--no-ui` run needs
//! retention exactly as much as a plot does — the telemetry sampler must be able to push
//! here without linking a GPU stack. So the data core lives here and `lunco-viz`
//! re-exports it; `color_for_signal` (the one genuinely render-bound item, returning an
//! `egui::Color32`) stayed behind. Same split as `lunco-render` / `lunco-render-bevy`.
//!
//! See `docs/architecture/render-decoupling.md` and
//! `docs/architecture/telemetry-subsystem.md`.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};

/// Fallback ring-buffer depth when neither the signal nor the registry names one.
/// Mirrors `WorkbenchState.max_history`.
pub const DEFAULT_CAPACITY: usize = 2000;

/// Stable identity for a signal across frames / save-load cycles.
///
/// The `entity` half is not decoration: signal *names collide*. Two rovers both report
/// `"motor_current"`, and only the owning entity tells them apart. [`Entity::PLACEHOLDER`]
/// means "global / no entity" (the simulation clock, top-level events).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SignalRef {
    #[serde(with = "entity_as_u64")]
    pub entity: Entity,
    pub path: String,
}

impl SignalRef {
    pub fn new(entity: Entity, path: impl Into<String>) -> Self {
        Self { entity, path: path.into() }
    }

    /// Global signal not tied to a specific entity.
    pub fn global(path: impl Into<String>) -> Self {
        Self { entity: Entity::PLACEHOLDER, path: path.into() }
    }
}

// Minimal serde glue for `Entity` — plain JSON/TOML friendliness for workspace files.
mod entity_as_u64 {
    use bevy::prelude::Entity;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub(super) fn serialize<S: Serializer>(e: &Entity, s: S) -> Result<S::Ok, S::Error> {
        e.to_bits().serialize(s)
    }
    pub(super) fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Entity, D::Error> {
        let bits = u64::deserialize(d)?;
        Ok(Entity::from_bits(bits))
    }
}

/// What shape the samples take.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SignalType {
    /// Continuous `f64` time-series.
    Scalar,
    // Reserved — shape locked in so viz-kind compatibility checks can name them today.
    Vec3,
    Pose,
    Event,
}

/// Descriptive metadata. Optional and non-load-bearing — viz kinds render without it,
/// but tooltips, legends, and axis labels get better when it's populated.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SignalMeta {
    pub description: Option<String>,
    /// Physical unit, e.g. `"kg"`, `"N"`, `"m/s"`.
    pub unit: Option<String>,
    /// Free-form tag naming who created the signal: `"modelica"`, `"avian"`, `"script"`,
    /// `"telemetry"`. Lets the inspector group signals by provenance.
    pub provenance: Option<String>,
}

/// One (time, value) pair for a [`SignalType::Scalar`] signal.
#[derive(Debug, Clone, Copy)]
pub struct ScalarSample {
    pub time: f64,
    pub value: f64,
}

/// Ring-buffer-backed history for one scalar signal.
#[derive(Debug, Clone)]
pub struct ScalarHistory {
    pub samples: VecDeque<ScalarSample>,
    pub capacity: usize,
}

impl ScalarHistory {
    pub fn new(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        Self { samples: VecDeque::with_capacity(capacity), capacity }
    }

    pub fn push(&mut self, sample: ScalarSample) {
        while self.samples.len() >= self.capacity {
            self.samples.pop_front();
        }
        self.samples.push_back(sample);
    }

    /// Change the retention depth, dropping the oldest samples if it shrank.
    ///
    /// A capacity of 0 would make `push` spin forever popping an empty deque, so it is
    /// clamped to 1 — "keep nothing" is spelled by disabling the channel, not by a
    /// zero-length buffer.
    pub fn set_capacity(&mut self, capacity: usize) {
        self.capacity = capacity.max(1);
        while self.samples.len() > self.capacity {
            self.samples.pop_front();
        }
    }

    pub fn clear(&mut self) {
        self.samples.clear();
    }

    pub fn iter(&self) -> impl Iterator<Item = &ScalarSample> {
        self.samples.iter()
    }

    pub fn len(&self) -> usize {
        self.samples.len()
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }
}

/// Global registry of every known signal.
#[derive(Resource, Debug, Default)]
pub struct SignalRegistry {
    scalar_history: HashMap<SignalRef, ScalarHistory>,
    types: HashMap<SignalRef, SignalType>,
    meta: HashMap<SignalRef, SignalMeta>,
    default_capacity: usize,
}

impl SignalRegistry {
    pub fn with_default_capacity(capacity: usize) -> Self {
        Self { default_capacity: capacity, ..Default::default() }
    }

    fn capacity_default(&self) -> usize {
        if self.default_capacity == 0 { DEFAULT_CAPACITY } else { self.default_capacity }
    }

    /// Push a scalar (time, value) sample. Allocates the history buffer and records the
    /// type on first sample.
    pub fn push_scalar(&mut self, sig: SignalRef, time: f64, value: f64) {
        if !value.is_finite() {
            return;
        }
        let cap = self.capacity_default();
        let history =
            self.scalar_history.entry(sig.clone()).or_insert_with(|| ScalarHistory::new(cap));
        history.push(ScalarSample { time, value });
        self.types.entry(sig).or_insert(SignalType::Scalar);
    }

    /// Push a sample into a signal with an explicit retention depth — **this is how a
    /// per-channel `retention` reaches the ring buffer.** Applies the capacity on first
    /// sight and whenever it changes, so re-authoring a channel's retention resizes its
    /// buffer in place rather than silently keeping the old depth.
    pub fn push_scalar_with_capacity(
        &mut self,
        sig: SignalRef,
        time: f64,
        value: f64,
        capacity: usize,
    ) {
        if !value.is_finite() {
            return;
        }
        let history = self
            .scalar_history
            .entry(sig.clone())
            .or_insert_with(|| ScalarHistory::new(capacity));
        if history.capacity != capacity.max(1) {
            history.set_capacity(capacity);
        }
        history.push(ScalarSample { time, value });
        self.types.entry(sig).or_insert(SignalType::Scalar);
    }

    pub fn update_meta(&mut self, sig: SignalRef, meta: SignalMeta) {
        self.meta.insert(sig, meta);
    }

    pub fn scalar_history(&self, sig: &SignalRef) -> Option<&ScalarHistory> {
        self.scalar_history.get(sig)
    }

    pub fn iter_scalar(&self) -> impl Iterator<Item = (&SignalRef, &ScalarHistory)> {
        self.scalar_history.iter()
    }

    pub fn signal_type(&self, sig: &SignalRef) -> Option<SignalType> {
        self.types.get(sig).copied()
    }

    pub fn meta(&self, sig: &SignalRef) -> Option<&SignalMeta> {
        self.meta.get(sig)
    }

    pub fn iter_signals(&self) -> impl Iterator<Item = (&SignalRef, SignalType)> {
        self.types.iter().map(|(r, t)| (r, *t))
    }

    /// Drop every signal owned by `entity`. Called when an entity despawns so stale
    /// references don't linger.
    pub fn drop_entity(&mut self, entity: Entity) {
        self.scalar_history.retain(|r, _| r.entity != entity);
        self.types.retain(|r, _| r.entity != entity);
        self.meta.retain(|r, _| r.entity != entity);
    }

    /// Forget a signal entirely — history, type, and metadata.
    ///
    /// Distinct from [`drop_entity`](Self::drop_entity): a channel that measures a rover is
    /// its own entity, so when the CHANNEL dies the rover does not, and only that one signal
    /// should go.
    pub fn remove_signal(&mut self, sig: &SignalRef) {
        self.scalar_history.remove(sig);
        self.types.remove(sig);
        self.meta.remove(sig);
    }

    /// Clear one signal's history without dropping its type / meta entry.
    pub fn clear_history(&mut self, sig: &SignalRef) {
        if let Some(h) = self.scalar_history.get_mut(sig) {
            h.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_full_history_drops_the_oldest_sample() {
        let mut h = ScalarHistory::new(3);
        for i in 0..5 {
            h.push(ScalarSample { time: i as f64, value: i as f64 });
        }
        assert_eq!(h.len(), 3);
        assert_eq!(h.iter().next().unwrap().time, 2.0, "the two oldest must have been evicted");
    }

    /// Retention is per-signal and re-authorable: shrinking must drop the oldest samples
    /// immediately, not lazily on the next push.
    #[test]
    fn shrinking_capacity_evicts_immediately() {
        let mut h = ScalarHistory::new(10);
        for i in 0..10 {
            h.push(ScalarSample { time: i as f64, value: 0.0 });
        }
        h.set_capacity(4);
        assert_eq!(h.len(), 4);
        assert_eq!(h.iter().next().unwrap().time, 6.0);
    }

    /// A zero capacity would make `push` spin forever popping an empty deque.
    #[test]
    fn a_zero_capacity_is_clamped_not_fatal() {
        let mut h = ScalarHistory::new(0);
        h.push(ScalarSample { time: 0.0, value: 1.0 });
        assert_eq!(h.len(), 1);
        h.set_capacity(0);
        h.push(ScalarSample { time: 1.0, value: 2.0 });
        assert_eq!(h.len(), 1);
    }

    /// Names collide across entities — the registry must keep them apart.
    #[test]
    fn the_same_path_on_two_entities_is_two_signals() {
        let mut reg = SignalRegistry::default();
        let a = SignalRef::new(Entity::from_raw_u32(1).unwrap(), "motor_current");
        let b = SignalRef::new(Entity::from_raw_u32(2).unwrap(), "motor_current");
        reg.push_scalar(a.clone(), 0.0, 1.0);
        reg.push_scalar(b.clone(), 0.0, 2.0);
        assert_eq!(reg.scalar_history(&a).unwrap().len(), 1);
        assert_eq!(reg.scalar_history(&b).unwrap().len(), 1);
        assert_eq!(reg.scalar_history(&b).unwrap().iter().next().unwrap().value, 2.0);
    }

    #[test]
    fn a_non_finite_sample_is_dropped() {
        let mut reg = SignalRegistry::default();
        let s = SignalRef::global("nan");
        reg.push_scalar(s.clone(), 0.0, f64::NAN);
        reg.push_scalar(s.clone(), 1.0, f64::INFINITY);
        assert!(reg.scalar_history(&s).is_none(), "NaN/inf must never enter a plot buffer");
    }

    #[test]
    fn per_channel_capacity_is_applied_and_resized() {
        let mut reg = SignalRegistry::default();
        let s = SignalRef::global("chan");
        for i in 0..10 {
            reg.push_scalar_with_capacity(s.clone(), i as f64, i as f64, 5);
        }
        assert_eq!(reg.scalar_history(&s).unwrap().len(), 5);

        // Re-author the retention downward — the buffer must resize in place.
        reg.push_scalar_with_capacity(s.clone(), 10.0, 10.0, 2);
        assert_eq!(reg.scalar_history(&s).unwrap().len(), 2);
        assert_eq!(reg.scalar_history(&s).unwrap().capacity, 2);
    }
}
