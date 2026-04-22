//! Phase-A of the multi-sim architecture refactor — lock-free
//! publish path from the Modelica worker thread into the UI.
//!
//! # Why this exists
//!
//! Before this file was added, every successful `Step` from the
//! worker thread sent a `ModelicaResult` through a crossbeam channel
//! to the Bevy main thread. `handle_modelica_responses` then
//! iterated the per-variable outputs and called
//! `SignalRegistry::push_scalar` for each one — a `HashMap` lookup
//! + optional allocation + ring-buffer push per sample. With
//! 30 variables × 60 Hz that's ~1800 main-thread mutations per
//! second. On its own it's only a few ms/s, but combined with
//! everything else the main thread does per frame (plot re-tessellation,
//! egui paint, text-editor re-layout, canvas projection, lint
//! change-detection) the per-frame budget drifts high enough that
//! user input — specifically typing into the code editor while a
//! sim is running — visibly lags.
//!
//! # Architecture
//!
//! Each simulated entity gets its own
//! `Arc<ArcSwap<SimSnapshot>>` ("sim stream"). The worker thread,
//! after each successful Step, builds an updated `SimSnapshot`
//! (copy-on-write: previous snapshot + appended samples), wraps it
//! in `Arc::new`, and calls `arc_swap.store(new)`. Readers (plot
//! panels, telemetry, canvas overlays) call `arc_swap.load()` to
//! get the current `Arc<SimSnapshot>` and render from it. No
//! locks. The old snapshot's `Arc` is dropped once all readers
//! have moved on.
//!
//! Data producers (the worker) and consumers (UI systems) share
//! only the `Arc<ArcSwap<_>>` handle; neither blocks the other.
//!
//! # Relationship to `SignalRegistry`
//!
//! `SimStream` does *not* replace `lunco-viz::SignalRegistry`. The
//! registry still stores metadata (descriptions, units, type tags)
//! and acts as the discovery surface for inspector pickers. What
//! changes is the per-sample history storage: when an entity has a
//! live `SimStream`, the Modelica worker publishes samples *there*,
//! and the main thread skips the per-sample registry push loop.
//! Plot panels consult `SimStream` first and fall back to
//! `SignalRegistry` when none is installed (headless tests, old
//! code paths, non-Modelica signal producers).
//!
//! # Scope of Phase A
//!
//! One snapshot per Modelica entity. One ring buffer per variable.
//! Monotonic-time append. No rate shaping — the worker publishes
//! on every successful Step. Phase B lifts the snapshot out of
//! the per-entity component into a `SimRegistry` keyed by `SimId`,
//! shared with multi-sim backends (FMU, remote replica).
//!
//! TODO(arch-phase-b): expose `SimStream` via the `SimRegistry`
//!   resource so multiple workers can publish side-by-side.
//! TODO(arch-phase-c): add `CompileRequest` routing that runs in
//!   `AsyncComputeTaskPool` and swaps in the new `Dae` at the next
//!   step boundary without blocking the stepper.

use std::sync::Arc;

use arc_swap::ArcSwap;
use indexmap::IndexMap;

/// Maximum samples retained per variable. 2000 matches
/// `lunco-viz::SignalRegistry`'s default so cross-over visuals
/// don't show a horizon jump when a stream is first installed.
pub const DEFAULT_HISTORY_CAPACITY: usize = 2000;

/// One `(time, value)` pair.
#[derive(Debug, Clone, Copy)]
pub struct SimSample {
    pub time: f64,
    pub value: f64,
}

/// Per-variable ring buffer of recent samples. Stored inside
/// [`SimSnapshot`]. Cloning a `VarHistory` is an `Arc` bump over
/// the underlying `Vec<SimSample>` — snapshots share sample
/// history across generations so a new snapshot doesn't copy the
/// entire window every publish.
#[derive(Debug, Clone, Default)]
pub struct VarHistory {
    /// Samples in append order. Capped at
    /// [`DEFAULT_HISTORY_CAPACITY`]; oldest samples drop off the
    /// front when the cap is hit.
    pub samples: Arc<Vec<SimSample>>,
}

impl VarHistory {
    /// Append `sample`, returning a new `VarHistory` with the
    /// current ring plus the new tail. Keeps history under
    /// [`DEFAULT_HISTORY_CAPACITY`] by dropping the oldest entry.
    ///
    /// Used by the worker on each Step: build next snapshot =
    /// `prev.append(new_sample)` per variable. Cheap because the
    /// backing `Arc<Vec<_>>` is cloned lazily — Arcs bump a
    /// refcount; `Vec` is only cloned when we need to mutate and
    /// there's >1 reader.
    pub fn append(&self, sample: SimSample) -> VarHistory {
        let mut next: Vec<SimSample> =
            Vec::with_capacity(self.samples.len().saturating_add(1));
        let overflow = (self.samples.len() + 1).saturating_sub(DEFAULT_HISTORY_CAPACITY);
        next.extend_from_slice(&self.samples[overflow..]);
        next.push(sample);
        VarHistory {
            samples: Arc::new(next),
        }
    }
}

/// Immutable snapshot of one simulated entity's state.
///
/// The worker publishes a new `Arc<SimSnapshot>` into the entity's
/// [`SimStream`] after every Step. Readers on any thread call
/// `stream.load()` to get the latest snapshot, render from it, and
/// drop their `Arc` clone when done.
///
/// Generation bumps monotonically so UI systems that cache derived
/// state (e.g. plot tessellations) can cheaply check "did
/// anything change?" without diffing histories.
#[derive(Debug, Clone, Default)]
pub struct SimSnapshot {
    /// Sim time of the most recent sample. Monotonic while the
    /// worker is running; `Reset` rewinds to 0.
    pub time: f64,
    /// Bumped every publish. Use for skip-if-unchanged checks.
    pub generation: u64,
    /// Per-variable history. `IndexMap` keeps declaration order
    /// stable across snapshots so list-based UIs (inspector,
    /// telemetry) don't jitter their row order.
    pub vars: IndexMap<String, VarHistory>,
}

impl SimSnapshot {
    /// Build a snapshot by appending a fresh time/value frame to
    /// the given previous snapshot. `outputs` carries all
    /// variables the worker observed at `new_time` (typically the
    /// DAE's states + algebraics + inputs collected by
    /// `collect_stepper_observables`).
    ///
    /// Generation bumps by 1. Any variable present in `prev` but
    /// missing from `outputs` is dropped (keeps the snapshot
    /// aligned with the model's current schema — important after
    /// a recompile that renamed or removed a variable).
    pub fn advance(prev: &SimSnapshot, new_time: f64, outputs: &[(String, f64)]) -> SimSnapshot {
        let mut next_vars: IndexMap<String, VarHistory> =
            IndexMap::with_capacity(outputs.len());
        for (name, value) in outputs {
            if !value.is_finite() {
                // Skip NaN / ±Inf rather than poison the history —
                // matches `SignalRegistry::push_scalar`'s rule.
                continue;
            }
            let base = prev
                .vars
                .get(name)
                .cloned()
                .unwrap_or_default();
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

    /// Snapshot with no samples but `time = 0`. Used as the initial
    /// stream state on Compile and after Reset so plots have an
    /// anchor point to draw from.
    pub fn empty_at_zero() -> SimSnapshot {
        SimSnapshot::default()
    }
}

/// Lock-free handle to a sim's snapshot. Producers (the worker)
/// call `.store(new_snapshot)`; consumers (plot panels) call
/// `.load()` and read.
pub type SimStream = Arc<ArcSwap<SimSnapshot>>;

/// Construct an empty stream with an initial zero-time snapshot.
/// Worker + UI both receive `Arc` clones of the returned handle.
pub fn new_sim_stream() -> SimStream {
    Arc::new(ArcSwap::from_pointee(SimSnapshot::empty_at_zero()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_respects_capacity() {
        let mut h = VarHistory::default();
        for i in 0..(DEFAULT_HISTORY_CAPACITY + 5) {
            h = h.append(SimSample {
                time: i as f64,
                value: i as f64,
            });
        }
        assert_eq!(h.samples.len(), DEFAULT_HISTORY_CAPACITY);
        // Oldest 5 should have dropped off the front.
        assert_eq!(h.samples.first().unwrap().time, 5.0);
        assert_eq!(
            h.samples.last().unwrap().time,
            (DEFAULT_HISTORY_CAPACITY + 4) as f64
        );
    }

    #[test]
    fn advance_drops_missing_vars() {
        let prev = SimSnapshot::advance(
            &SimSnapshot::empty_at_zero(),
            0.1,
            &[
                ("a".into(), 1.0),
                ("b".into(), 2.0),
            ],
        );
        assert!(prev.vars.contains_key("a"));
        assert!(prev.vars.contains_key("b"));
        let next = SimSnapshot::advance(&prev, 0.2, &[("a".into(), 3.0)]);
        assert!(next.vars.contains_key("a"));
        assert!(!next.vars.contains_key("b"));
        // History should carry forward for surviving vars.
        assert_eq!(next.vars["a"].samples.len(), 2);
    }

    #[test]
    fn advance_skips_nonfinite() {
        let prev = SimSnapshot::advance(
            &SimSnapshot::empty_at_zero(),
            0.1,
            &[
                ("good".into(), 1.0),
                ("nan".into(), f64::NAN),
                ("inf".into(), f64::INFINITY),
            ],
        );
        assert!(prev.vars.contains_key("good"));
        assert!(!prev.vars.contains_key("nan"));
        assert!(!prev.vars.contains_key("inf"));
    }

    #[test]
    fn generation_increments() {
        let a = SimSnapshot::advance(&SimSnapshot::empty_at_zero(), 0.1, &[]);
        let b = SimSnapshot::advance(&a, 0.2, &[]);
        assert_eq!(a.generation, 1);
        assert_eq!(b.generation, 2);
    }

    #[test]
    fn stream_is_lock_free_readable() {
        // Smoke test: construct, load, store, load. Real
        // concurrent access is exercised by `ArcSwap` itself;
        // we just want to confirm the public API works.
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
