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
use lunco_core::{AppSceneExt, GlobalEntityId, SceneResource};
use serde::{Deserialize, Deserializer, Serialize, de::Error as _};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

/// Fallback ring-buffer depth when neither the signal nor the registry names one.
/// Mirrors `WorkbenchState.max_history`.
pub const DEFAULT_CAPACITY: usize = 2000;

/// Numeric tolerance used to decide whether a telemetry sample is meaningful.
///
/// `atol` is expressed in the signal's native unit and `rtol` is a fraction of
/// the larger magnitude.  The comparison is intentionally a telemetry policy,
/// not a Modelica solver setting: solver tolerances control convergence, while
/// this controls what an operator sees and what the recorder retains.
///
/// The default is a 1e-4 native-unit floor plus 0.1% relative tolerance.  That
/// removes ordinary floating-point/physics jitter around zero while preserving
/// changes that are meaningful at the signal's current scale.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct TelemetryDeadband {
    /// Absolute tolerance in the signal's native unit.
    pub atol: f64,
    /// Relative tolerance as a fraction (0.001 = 0.1%).
    pub rtol: f64,
}

impl<'de> Deserialize<'de> for TelemetryDeadband {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct WireTelemetryDeadband {
            atol: f64,
            rtol: f64,
        }

        let wire = WireTelemetryDeadband::deserialize(deserializer)?;
        Self::try_new(wire.atol, wire.rtol).map_err(D::Error::custom)
    }
}

impl Default for TelemetryDeadband {
    fn default() -> Self {
        Self {
            atol: 1e-4,
            rtol: 1e-3,
        }
    }
}

impl TelemetryDeadband {
    /// Whether a scalar tolerance is a valid authored policy value.
    #[inline]
    pub fn is_valid_tolerance(value: f64) -> bool {
        value.is_finite() && value >= 0.0
    }

    /// Whether both tolerances form a valid policy.
    #[inline]
    pub fn is_valid(self) -> bool {
        Self::is_valid_tolerance(self.atol) && Self::is_valid_tolerance(self.rtol)
    }

    /// Construct a validated deadband policy.
    pub fn try_new(atol: f64, rtol: f64) -> Result<Self, &'static str> {
        let deadband = Self { atol, rtol };
        if deadband.is_valid() {
            Ok(deadband)
        } else {
            Err("telemetry deadband tolerances must be finite and non-negative")
        }
    }

    /// Whether `current` is far enough from `previous` to retain a sample.
    ///
    /// Non-finite values always count as changed.  This prevents a numerical
    /// fault from being hidden by a tolerance and lets the producer decide how
    /// to represent that fault.
    pub fn changed(self, previous: f64, current: f64) -> bool {
        if !self.is_valid() || !previous.is_finite() || !current.is_finite() {
            return true;
        }

        let tolerance = self.atol.max(self.rtol * previous.abs().max(current.abs()));
        (current - previous).abs() > tolerance
    }
}

/// Stable identity for a signal across frames **within one session**.
///
/// The `entity` half is not decoration: signal *names collide*. Two rovers both report
/// `"motor_current"`, and only the owning entity tells them apart. [`Entity::PLACEHOLDER`]
/// means "global / no entity" (the simulation clock, top-level events).
///
/// The serde form carries raw `Entity` bits, which are **session-local** — the
/// generation counter resets every run, so bits written in one session point at a
/// phantom entity in the next. It exists only for in-memory round-trips (viz style
/// blobs travel as `serde_json::Value`). Anything that outlives the session must go
/// through [`PersistedSignalRef`], which swaps the entity for its [`GlobalEntityId`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SignalRef {
    #[serde(with = "entity_as_u64")]
    pub entity: Entity,
    pub path: String,
}

impl SignalRef {
    pub fn new(entity: Entity, path: impl Into<String>) -> Self {
        Self {
            entity,
            path: path.into(),
        }
    }

    /// Global signal not tied to a specific entity.
    pub fn global(path: impl Into<String>) -> Self {
        Self {
            entity: Entity::PLACEHOLDER,
            path: path.into(),
        }
    }

    /// Cross-session form: swap the session-local `Entity` for its stable
    /// [`GlobalEntityId`]. `gid_of` is the caller's lookup (typically a closure over a
    /// `Query<&GlobalEntityId>`); `None` means the entity has no stable id, and a ref
    /// that cannot name its owner stably cannot be persisted.
    pub fn to_persisted(
        &self,
        gid_of: impl FnOnce(Entity) -> Option<GlobalEntityId>,
    ) -> Option<PersistedSignalRef> {
        let entity = if self.entity == Entity::PLACEHOLDER {
            None
        } else {
            Some(gid_of(self.entity)?)
        };
        Some(PersistedSignalRef {
            entity,
            path: self.path.clone(),
        })
    }
}

// TODO(backlog): the save/load boundary wiring is still pending — nothing routes
// `SignalBinding`/`LinePlotStyle` through `to_persisted`/`resolve` yet; that lands
// with plot-layout persistence. Producers must also tag [`SignalSource`] on owning
// entities so registry slots auto-clean on despawn. See the engineering-backlog doc
// in docs/architecture (signal persistence wiring).
/// What a [`SignalRef`] looks like at rest (saved plot layouts, workspace files).
///
/// The entity half is the stable [`GlobalEntityId`] — never raw `Entity` bits, which
/// are session-local and would orphan the ref on reload. `None` is the persisted
/// spelling of [`Entity::PLACEHOLDER`] (a global signal).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PersistedSignalRef {
    pub entity: Option<GlobalEntityId>,
    pub path: String,
}

impl PersistedSignalRef {
    /// Resolve back to a live [`SignalRef`]. `entity_of` is the caller's reverse
    /// lookup (gid → live entity, wherever the load path already resolves entities);
    /// `None` means the owning entity does not exist in this session.
    pub fn resolve(
        &self,
        entity_of: impl FnOnce(GlobalEntityId) -> Option<Entity>,
    ) -> Option<SignalRef> {
        let entity = match self.entity {
            None => Entity::PLACEHOLDER,
            Some(gid) => entity_of(gid)?,
        };
        Some(SignalRef {
            entity,
            path: self.path.clone(),
        })
    }
}

// Minimal serde glue for `Entity` — plain JSON friendliness for the in-memory style
// blobs. Session-local by construction; see the [`SignalRef`] docs.
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

/// Whether a signal is part of a model's operator-facing surface or is an
/// implementation value retained for runtime inspection.
///
/// This is runtime metadata, not an authoring annotation.  A producer derives
/// it from the ownership/projection it already owns.  In particular, a
/// generated Modelica network marks its authored boundary outputs and promoted
/// member outputs as [`Public`] and its copied inputs, connector values, and
/// component state as [`Internal`].  The registry still retains both kinds;
/// clients decide whether to show implementation state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum SignalExposure {
    /// A canonical value intended for the normal operator-facing catalog.
    #[default]
    Public,
    /// Complete runtime state retained for model inspection and diagnostics.
    Internal,
}

/// How one scalar channel participates in an operator-facing compound value.
///
/// The registry remains scalar and keeps each [`SignalRef`] addressable. This
/// metadata only tells presentation consumers that several addresses belong in
/// one semantic group, and whether a row is a component or a derived summary.
/// Producers own this relationship; consumers must not infer it from names.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SignalPresentation {
    /// A standalone scalar channel.
    #[default]
    Scalar,
    /// One named component of a vector or compound value.
    Component {
        /// Semantic group shared by the components and optional summary.
        group: String,
        /// Operator-facing component label, such as `x` or `quaternion w`.
        component: String,
    },
    /// A producer-supplied summary of a semantic group.
    Summary {
        /// Semantic group shared by the components and this summary.
        group: String,
        /// Operator-facing label, such as `speed` or `magnitude`.
        label: String,
        /// Human-readable formula or definition, such as `magnitude`.
        formula: String,
    },
}

/// Descriptive metadata. Optional and non-load-bearing — viz kinds render without it,
/// but tooltips, legends, and axis labels get better when it's populated.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignalMeta {
    pub description: Option<String>,
    /// Physical unit, e.g. `"kg"`, `"N"`, `"m/s"`.
    pub unit: Option<String>,
    /// Free-form tag naming who created the signal: `"modelica"`, `"avian"`, `"script"`,
    /// `"telemetry"`. Lets the inspector group signals by provenance.
    pub provenance: Option<String>,
    /// Runtime visual ownership path supplied by the producing model.
    pub group_path: Option<String>,
    /// Runtime-derived operator/implementation classification.
    pub exposure: SignalExposure,
    /// Authored Modelica class that produces this value, when the signal is a
    /// projection of a generated component network.
    ///
    /// This is deliberately separate from [`provenance`].  `provenance` says
    /// which runtime producer owns the channel; this says which authored
    /// Modelica declaration the operator is looking at.
    pub model_class: Option<String>,
    /// Variable name in the authored Modelica class (for example
    /// `power_draw_w`), rather than the generated wrapper spelling.
    pub model_variable: Option<String>,
    /// Source asset that declared [`model_class`].
    pub source_asset: Option<String>,
    /// Canonical USD-facing name for this value, when the generated solver
    /// also exposes an implementation alias.
    pub canonical_name: Option<String>,
    /// Producer-owned semantic grouping for vector and compound values.
    #[serde(default)]
    pub presentation: SignalPresentation,
}

/// One (time, value) pair for a [`SignalType::Scalar`] signal.
#[derive(Debug, Clone, Copy)]
pub struct ScalarSample {
    pub time: f64,
    pub value: f64,
}

const SCALAR_HISTORY_CHUNK_SIZE: usize = 256;

/// Immutable sample view captured for background processing.
///
/// Completed chunks are shared with the live history. Capturing a snapshot
/// copies only the bounded, still-open tail; consumers can flatten it on a
/// worker without copying the full retention window on the caller's thread.
#[derive(Debug, Clone)]
pub struct ScalarHistorySnapshot {
    chunks: Vec<Arc<[ScalarSample]>>,
    first_chunk_offset: usize,
    tail: Arc<[ScalarSample]>,
    len: usize,
}

impl ScalarHistorySnapshot {
    /// Move the retained samples into an ordered contiguous buffer.
    pub fn into_samples(self) -> Vec<ScalarSample> {
        let Self {
            chunks,
            first_chunk_offset,
            tail,
            len,
        } = self;
        let mut samples = Vec::with_capacity(len);
        for (index, chunk) in chunks.iter().enumerate() {
            let offset = if index == 0 { first_chunk_offset } else { 0 };
            samples.extend_from_slice(&chunk[offset..]);
        }
        samples.extend_from_slice(&tail);
        samples
    }
}

/// Chunked ring-buffer history for one scalar signal.
///
/// `capacity` is the logical retention limit. Completed sample chunks are
/// immutable and shared with plot workers; only the bounded tail remains
/// mutable. The live iteration order and retention semantics match a ring
/// buffer while snapshots avoid copying the full history on the UI thread.
#[derive(Debug, Clone)]
pub struct ScalarHistory {
    chunks: VecDeque<Arc<[ScalarSample]>>,
    first_chunk_offset: usize,
    tail: VecDeque<ScalarSample>,
    len: usize,
    pub capacity: usize,
}

impl ScalarHistory {
    pub fn new(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        Self {
            chunks: VecDeque::new(),
            first_chunk_offset: 0,
            tail: VecDeque::new(),
            len: 0,
            capacity,
        }
    }

    pub fn push(&mut self, sample: ScalarSample) {
        let _span = (self.tail.len() == self.tail.capacity()).then(|| {
            bevy::log::debug_span!(
                "signal_scalar_history_tail_growth",
                retained = self.len,
                tail_capacity = self.tail.capacity()
            )
            .entered()
        });
        if self.len >= self.capacity {
            self.pop_front();
        }
        self.tail.push_back(sample);
        self.len += 1;
        if self.tail.len() == SCALAR_HISTORY_CHUNK_SIZE {
            let samples: Vec<_> = self.tail.drain(..).collect();
            self.chunks.push_back(Arc::from(samples));
        }
    }

    fn pop_front(&mut self) {
        if let Some(chunk) = self.chunks.front() {
            self.first_chunk_offset += 1;
            if self.first_chunk_offset == chunk.len() {
                self.chunks.pop_front();
                self.first_chunk_offset = 0;
            }
        } else {
            self.tail.pop_front();
        }
        self.len -= 1;
    }

    /// Change the retention depth, dropping the oldest samples if it shrank.
    ///
    /// A capacity of 0 would make `push` spin forever popping an empty deque, so it is
    /// clamped to 1 — "keep nothing" is spelled by disabling the channel, not by a
    /// zero-length buffer.
    pub fn set_capacity(&mut self, capacity: usize) {
        self.capacity = capacity.max(1);
        while self.len > self.capacity {
            self.pop_front();
        }
    }

    pub fn clear(&mut self) {
        self.chunks.clear();
        self.first_chunk_offset = 0;
        self.tail.clear();
        self.len = 0;
    }

    pub fn iter(&self) -> impl Iterator<Item = &ScalarSample> {
        let first_chunk_offset = self.first_chunk_offset;
        self.chunks
            .iter()
            .enumerate()
            .flat_map(move |(index, chunk)| {
                let offset = if index == 0 { first_chunk_offset } else { 0 };
                chunk.get(offset..).unwrap_or(&[]).iter()
            })
            .chain(self.tail.iter())
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn front(&self) -> Option<&ScalarSample> {
        self.chunks
            .front()
            .and_then(|chunk| chunk.get(self.first_chunk_offset))
            .or_else(|| self.tail.front())
    }

    pub fn back(&self) -> Option<&ScalarSample> {
        self.tail
            .back()
            .or_else(|| self.chunks.back().and_then(|chunk| chunk.last()))
    }

    /// Capture shared completed chunks and a bounded copy of the mutable tail.
    pub fn snapshot(&self) -> ScalarHistorySnapshot {
        let chunks = self.chunks.iter().cloned().collect();
        let tail = Arc::from(self.tail.iter().copied().collect::<Vec<_>>());
        ScalarHistorySnapshot {
            chunks,
            first_chunk_offset: self.first_chunk_offset,
            tail,
            len: self.len,
        }
    }
}

/// Opaque recording capability for one live slot in one registry. Removal,
/// archival and scene retirement revoke it; slot reuse cannot revive it.
#[derive(Clone, Debug)]
pub struct ScalarRecordingBinding {
    index: usize,
    lease: Arc<()>,
}

#[derive(Debug)]
struct SignalEntry {
    signal: SignalRef,
    lease: Arc<()>,
    history: Option<ScalarHistory>,
    kind: Option<SignalType>,
    meta: Option<SignalMeta>,
    global_owner: Option<GlobalEntityId>,
    active: bool,
    application: bool,
}

/// Global signal owner. Text resolves at admission/inspection; numerical
/// producers retain checked recording slots into this same authoritative store.
#[derive(Resource, Debug, Default)]
pub struct SignalRegistry {
    entries: Vec<Option<SignalEntry>>,
    names: HashMap<SignalRef, usize>,
    free: Vec<usize>,
    scalar_count: usize,
    active_scalar_count: usize,
    default_capacity: usize,
    catalog_revision: u64,
    recording_revision: u64,
    descriptor_changes: std::collections::HashSet<SignalRef>,
}

impl SceneResource for SignalRegistry {
    fn clear_scene(&mut self) {
        let retired: Vec<_> = self
            .entries
            .iter()
            .flatten()
            .filter(|entry| !entry.application)
            .map(|entry| entry.signal.clone())
            .collect();
        for signal in retired {
            self.remove_signal(&signal);
        }
        // Wake surviving paused producers even when there were no scene slots.
        self.recording_revision = self.recording_revision.wrapping_add(1);
        self.catalog_revision = self.catalog_revision.wrapping_add(1);
    }
}

impl SignalRegistry {
    fn entry(&self, signal: &SignalRef) -> Option<&SignalEntry> {
        self.entries
            .get(*self.names.get(signal)?)
            .and_then(Option::as_ref)
    }

    fn ensure_entry(&mut self, signal: &SignalRef) -> usize {
        if let Some(&index) = self.names.get(signal) {
            return index;
        }
        let entry = Some(SignalEntry {
            signal: signal.clone(),
            lease: Arc::new(()),
            history: None,
            kind: None,
            meta: None,
            global_owner: None,
            active: true,
            application: false,
        });
        let index = if let Some(index) = self.free.pop() {
            self.entries[index] = entry;
            index
        } else {
            self.entries.push(entry);
            self.entries.len() - 1
        };
        self.names.insert(signal.clone(), index);
        index
    }

    /// Give a process-owned signal an explicit lifetime beyond scene replacement.
    pub fn retain_across_scenes(&mut self, signal: SignalRef) {
        let index = self.ensure_entry(&signal);
        self.entries[index]
            .as_mut()
            .expect("admitted signal")
            .application = true;
    }

    pub fn with_default_capacity(capacity: usize) -> Self {
        Self {
            default_capacity: capacity,
            ..Default::default()
        }
    }

    fn capacity_default(&self) -> usize {
        if self.default_capacity == 0 {
            DEFAULT_CAPACITY
        } else {
            self.default_capacity
        }
    }

    /// Revision of recorder membership/capacity availability. Ordinary samples,
    /// metadata and wire-owner updates never change it. Deferred producers retry
    /// admission only after this revision or their own source/settings change.
    pub fn recording_revision(&self) -> u64 {
        self.recording_revision
    }

    /// Admit a live scalar recorder subject to the caller's live-channel budget.
    /// Existing active channels remain admitted when that budget is lowered.
    /// Admission does not clear archived history; a new producer session owns
    /// that explicit operation through `clear_bound_history`.
    pub fn admit_scalar(
        &mut self,
        signal: &SignalRef,
        capacity: usize,
        max_active: usize,
    ) -> Option<ScalarRecordingBinding> {
        let live = self
            .entry(signal)
            .is_some_and(|entry| entry.active && entry.history.is_some());
        if !live && self.active_scalar_count >= max_active {
            return None;
        }
        let index = self.ensure_entry(signal);
        self.activate_scalar(index, capacity);
        let entry = self.entries[index].as_ref().expect("admitted scalar");
        Some(ScalarRecordingBinding {
            index,
            lease: entry.lease.clone(),
        })
    }

    fn activate_scalar(&mut self, index: usize, capacity: usize) {
        let entry = self.entries[index].as_mut().expect("admitted signal");
        let new_history = entry.history.is_none();
        if new_history {
            entry.history = Some(ScalarHistory::new(capacity));
            self.scalar_count += 1;
        }
        if new_history || !entry.active {
            self.active_scalar_count += 1;
            self.recording_revision = self.recording_revision.wrapping_add(1);
        }
        if entry.kind.is_none() || !entry.active {
            entry.kind = Some(SignalType::Scalar);
            self.descriptor_changes.insert(entry.signal.clone());
            self.catalog_revision = self.catalog_revision.wrapping_add(1);
        }
        entry.active = true;
    }

    /// Check the capability without resolving or hashing a signal name.
    pub fn recording_is_current(&self, binding: &ScalarRecordingBinding) -> bool {
        self.entries
            .get(binding.index)
            .and_then(Option::as_ref)
            .is_some_and(|entry| {
                entry.active && entry.history.is_some() && Arc::ptr_eq(&entry.lease, &binding.lease)
            })
    }

    /// Explicit new-session history boundary. A stale capability fails visibly.
    pub fn clear_bound_history(
        &mut self,
        binding: &ScalarRecordingBinding,
    ) -> Result<(), &'static str> {
        if !self.recording_is_current(binding) {
            return Err("scalar recording binding is stale or belongs to another registry");
        }
        self.entries[binding.index]
            .as_mut()
            .and_then(|entry| entry.history.as_mut())
            .expect("validated scalar binding")
            .clear();
        Ok(())
    }

    fn append_scalar(
        &mut self,
        index: usize,
        sample: ScalarSample,
        capacity: Option<usize>,
        rate_hz: Option<f64>,
    ) -> bool {
        // Admission and append operate on the same owner storage. Rejection
        // cannot reactivate a publisher or resize its retained history.
        if let Some(history) = self.entries[index]
            .as_mut()
            .and_then(|entry| entry.history.as_mut())
            && let (Some(rate), Some(previous)) = (rate_hz, history.back())
        {
            if sample.time < previous.time {
                history.clear();
            } else if sample.time - previous.time < 1.0 / rate {
                return false;
            }
        }
        self.activate_scalar(index, capacity.unwrap_or_else(|| self.capacity_default()));
        let history = self.entries[index]
            .as_mut()
            .and_then(|entry| entry.history.as_mut())
            .expect("active scalar recorder");
        if let Some(capacity) = capacity
            && history.capacity != capacity.max(1)
        {
            history.set_capacity(capacity);
        }
        history.push(sample);
        true
    }

    /// Append through a live recording capability. Invalid numerical input is a
    /// rejected sample; an invalid capability is an owner error, never a named
    /// lookup or implicit re-admission in the numerical loop.
    pub fn record_bound_scalar(
        &mut self,
        binding: &ScalarRecordingBinding,
        time: f64,
        value: f64,
        rate_hz: f64,
        capacity: usize,
    ) -> Result<bool, &'static str> {
        if !self.recording_is_current(binding) {
            return Err("scalar recording binding is stale or belongs to another registry");
        }
        if !time.is_finite() || !value.is_finite() || !rate_hz.is_finite() || rate_hz <= 0.0 {
            return Ok(false);
        }
        Ok(self.append_scalar(
            binding.index,
            ScalarSample { time, value },
            Some(capacity),
            Some(rate_hz),
        ))
    }

    /// Named admission for one-shot producers. Streaming producers retain a
    /// `ScalarRecordingBinding`; both paths append to the same owner storage.
    pub fn push_scalar(&mut self, sig: SignalRef, time: f64, value: f64) {
        if !value.is_finite() {
            return;
        }
        let index = self.ensure_entry(&sig);
        self.append_scalar(index, ScalarSample { time, value }, None, None);
    }

    /// Apply explicit retention on every accepted sample.
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
        let index = self.ensure_entry(&sig);
        self.append_scalar(index, ScalarSample { time, value }, Some(capacity), None);
    }

    /// Notification/deadband recording policy, using the authoritative history.
    pub fn retain_scalar_if_changed(
        &mut self,
        sig: SignalRef,
        time: f64,
        value: f64,
        rate_hz: f64,
        deadband: TelemetryDeadband,
        capacity: usize,
    ) -> bool {
        if !time.is_finite()
            || !value.is_finite()
            || !rate_hz.is_finite()
            || rate_hz <= 0.0
            || !deadband.is_valid()
        {
            return false;
        }
        let index = self.ensure_entry(&sig);
        if let Some(previous) = self.entries[index]
            .as_ref()
            .and_then(|entry| entry.history.as_ref())
            .and_then(ScalarHistory::back)
        {
            if time >= previous.time
                && (time - previous.time < 1.0 / rate_hz
                    || !deadband.changed(previous.value, value))
            {
                return false;
            }
            if time < previous.time {
                self.entries[index]
                    .as_mut()
                    .and_then(|entry| entry.history.as_mut())
                    .expect("observed history")
                    .clear();
            }
        }
        self.append_scalar(index, ScalarSample { time, value }, Some(capacity), None)
    }

    /// Record at simulation-time rate independent of notification deadband.
    /// Named dynamic admission shares the bound recorder's append mechanism.
    pub fn record_scalar_at_rate(
        &mut self,
        sig: &SignalRef,
        time: f64,
        value: f64,
        rate_hz: f64,
        capacity: usize,
    ) -> bool {
        if !time.is_finite() || !value.is_finite() || !rate_hz.is_finite() || rate_hz <= 0.0 {
            return false;
        }
        let index = self.ensure_entry(sig);
        self.append_scalar(
            index,
            ScalarSample { time, value },
            Some(capacity),
            Some(rate_hz),
        )
    }

    pub fn update_meta(&mut self, sig: SignalRef, meta: SignalMeta) {
        let index = self.ensure_entry(&sig);
        let entry = self.entries[index].as_mut().expect("admitted signal");
        if entry.meta.as_ref() != Some(&meta) {
            entry.meta = Some(meta);
            self.descriptor_changes.insert(sig);
            self.catalog_revision = self.catalog_revision.wrapping_add(1);
        }
    }

    /// Capture stable wire identity while the source is live; archives retain it.
    pub fn associate_global_owner(&mut self, sig: &SignalRef, owner: GlobalEntityId) {
        let index = self.ensure_entry(sig);
        let entry = self.entries[index].as_mut().expect("admitted signal");
        if entry.global_owner != Some(owner) {
            entry.global_owner = Some(owner);
            self.catalog_revision = self.catalog_revision.wrapping_add(1);
        }
    }

    pub fn global_owner(&self, sig: &SignalRef) -> Option<GlobalEntityId> {
        self.entry(sig)?.global_owner
    }
    pub fn catalog_revision(&self) -> u64 {
        self.catalog_revision
    }
    pub fn scalar_history(&self, sig: &SignalRef) -> Option<&ScalarHistory> {
        self.entry(sig)?.history.as_ref()
    }
    pub fn iter_scalar(&self) -> impl Iterator<Item = (&SignalRef, &ScalarHistory)> {
        self.entries
            .iter()
            .flatten()
            .filter_map(|entry| Some((&entry.signal, entry.history.as_ref()?)))
    }
    pub fn scalar_count(&self) -> usize {
        self.scalar_count
    }
    pub fn active_scalar_count(&self) -> usize {
        self.active_scalar_count
    }
    pub fn signal_type(&self, sig: &SignalRef) -> Option<SignalType> {
        self.entry(sig)?.kind
    }
    pub fn meta(&self, sig: &SignalRef) -> Option<&SignalMeta> {
        self.entry(sig)?.meta.as_ref()
    }
    pub fn is_active(&self, sig: &SignalRef) -> bool {
        self.entry(sig).is_none_or(|entry| entry.active)
    }

    pub fn deactivate_signal(&mut self, sig: &SignalRef) {
        let Some(&index) = self.names.get(sig) else {
            return;
        };
        let entry = self.entries[index].as_mut().expect("named signal");
        if entry.kind.is_some() && entry.active {
            entry.active = false;
            entry.lease = Arc::new(());
            if entry.history.is_some() {
                self.active_scalar_count -= 1;
            }
            self.recording_revision = self.recording_revision.wrapping_add(1);
            self.catalog_revision = self.catalog_revision.wrapping_add(1);
            self.descriptor_changes.insert(sig.clone());
        }
    }
    pub fn deactivate_entity(&mut self, entity: Entity) {
        let signals: Vec<_> = self
            .entries
            .iter()
            .flatten()
            .filter(|entry| entry.signal.entity == entity && entry.active)
            .map(|entry| entry.signal.clone())
            .collect();
        for signal in signals {
            self.deactivate_signal(&signal);
        }
    }
    pub fn iter_signals(&self) -> impl Iterator<Item = (&SignalRef, SignalType)> {
        self.entries
            .iter()
            .flatten()
            .filter_map(|entry| Some((&entry.signal, entry.kind?)))
    }
    pub fn drop_entity(&mut self, entity: Entity) {
        let signals: Vec<_> = self
            .entries
            .iter()
            .flatten()
            .filter(|entry| entry.signal.entity == entity)
            .map(|entry| entry.signal.clone())
            .collect();
        for signal in signals {
            self.remove_signal(&signal);
        }
    }
    pub fn remove_signal(&mut self, sig: &SignalRef) {
        let Some(index) = self.names.remove(sig) else {
            return;
        };
        let entry = self.entries[index].take().expect("named signal");
        if entry.history.is_some() {
            self.scalar_count -= 1;
            if entry.active {
                self.active_scalar_count -= 1;
            }
        }
        self.free.push(index);
        self.descriptor_changes.insert(sig.clone());
        self.catalog_revision = self.catalog_revision.wrapping_add(1);
        self.recording_revision = self.recording_revision.wrapping_add(1);
    }
    pub fn clear_history(&mut self, sig: &SignalRef) {
        if let Some(index) = self.names.get(sig).copied()
            && let Some(history) = self.entries[index]
                .as_mut()
                .and_then(|entry| entry.history.as_mut())
        {
            history.clear();
        }
    }
}

/// Coalesced channel descriptor changes. Consumers read the current descriptor
/// from the registry; repeated writes within one app update occupy one entry.
/// Samples and API wire-owner association do not change browser descriptors.
#[derive(Message, Debug)]
pub struct SignalDescriptorsChanged {
    pub signals: Vec<SignalRef>,
}

#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct SignalDescriptorPublish;

/// Owns descriptor notification fan-out in both rendered and headless hosts.
/// Message readers must consume each Update, retaining their own pending work
/// when their presentation is hidden.
pub struct SignalRegistryPlugin;

impl Plugin for SignalRegistryPlugin {
    fn build(&self, app: &mut App) {
        app.init_scene_resource::<SignalRegistry>()
            .add_message::<SignalDescriptorsChanged>()
            .add_observer(drop_signals_of_removed_source)
            .add_systems(
                Update,
                publish_descriptor_changes.in_set(SignalDescriptorPublish),
            );
    }
}

fn publish_descriptor_changes(
    mut registry: ResMut<SignalRegistry>,
    mut messages: MessageWriter<SignalDescriptorsChanged>,
) {
    if registry.descriptor_changes.is_empty() {
        return;
    }
    let mut signals: Vec<_> = registry.descriptor_changes.drain().collect();
    signals.sort_by(|left, right| {
        left.entity
            .to_bits()
            .cmp(&right.entity.to_bits())
            .then(left.path.cmp(&right.path))
    });
    messages.write(SignalDescriptorsChanged { signals });
}

/// What the user is currently looking at — the entity roots a telemetry surface
/// should narrow itself to ("the selected rover's channels", not every channel in
/// the sim).
///
/// It lives HERE, in the render-free signal core, for the same reason
/// [`SignalRegistry`] does: the surfaces that read it (`lunco-viz`'s telemetry
/// browser today, a HUD or a recorder tomorrow) must not depend on whichever app
/// crate owns the selection UX, and the app crate that WRITES it
/// (`lunco-scene-selection`'s `SelectedEntities` mirror) must not depend on a GPU
/// stack. Empty ⇒ no focus; every consumer shows everything.
///
/// Roots, not channel owners: a channel sits on the prim it measures (a motor, a
/// battery), while selection targets the vessel. Membership is therefore an
/// ANCESTOR test, which the consumer does — it has the hierarchy, this resource is
/// just the intent.
#[derive(Resource, Debug, Default, Clone, PartialEq, Eq)]
pub struct TelemetryFocus {
    /// The selected roots, in selection order (last = primary).
    pub roots: Vec<Entity>,
}

impl SceneResource for TelemetryFocus {}

impl TelemetryFocus {
    /// True while nothing is selected — "show everything".
    pub fn is_empty(&self) -> bool {
        self.roots.is_empty()
    }

    /// Order-independent fingerprint, for change-gated view-model caches that
    /// have to rebuild when the focus moves but not every frame.
    pub fn fingerprint(&self) -> u64 {
        let mut acc: u64 = 0;
        for (i, e) in self.roots.iter().enumerate() {
            acc ^= e.to_bits().rotate_left((i % 64) as u32);
        }
        acc ^ (self.roots.len() as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
    }
}

/// Marker for an entity that owns [`SignalRegistry`] entries. Producers tag the
/// owning entity when they start pushing entity-scoped signals for it;
/// [`drop_signals_of_removed_source`] then marks their retained histories inactive
/// when the source dies.
#[derive(Component, Debug, Default, Clone, Copy)]
pub struct SignalSource;

/// Observer: a [`SignalSource`] entity despawned (or lost the marker) — drop every
/// signal it owned inactive. History remains graphable within the current scene;
/// scene teardown retires it. Registered by `SignalRegistryPlugin` in every host.
pub fn drop_signals_of_removed_source(
    trigger: On<Remove<SignalSource>>,
    mut signals: ResMut<SignalRegistry>,
) {
    signals.deactivate_entity(trigger.entity);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recording_bindings_fence_owner_removal_reuse_and_scene_lifetime() {
        let mut registry = SignalRegistry::default();
        let signal = SignalRef::global("bound");
        let binding = registry.admit_scalar(&signal, 8, 1).unwrap();
        assert!(
            registry
                .record_bound_scalar(&binding, 0.0, -0.0, 10.0, 8)
                .unwrap()
        );
        assert!(
            !registry
                .record_bound_scalar(&binding, 0.05, 1.0, 10.0, 1)
                .unwrap()
        );
        assert_eq!(registry.scalar_history(&signal).unwrap().capacity, 8);
        assert_eq!(
            registry
                .scalar_history(&signal)
                .unwrap()
                .back()
                .unwrap()
                .value
                .to_bits(),
            (-0.0_f64).to_bits()
        );
        let mut other = SignalRegistry::default();
        let _other_binding = other.admit_scalar(&signal, 8, 1).unwrap();
        assert!(
            other
                .record_bound_scalar(&binding, 1.0, 1.0, 10.0, 8)
                .is_err()
        );
        registry.deactivate_signal(&signal);
        assert!(
            registry
                .record_bound_scalar(&binding, 1.0, 1.0, 10.0, 8)
                .is_err()
        );
        let replacement = registry.admit_scalar(&signal, 8, 1).unwrap();
        assert!(
            registry
                .record_bound_scalar(&replacement, -1.0, 2.0, 10.0, 3)
                .unwrap()
        );
        registry.remove_signal(&signal);
        let _reused = registry
            .admit_scalar(&SignalRef::global("reused"), 8, 1)
            .unwrap();
        assert!(
            registry
                .record_bound_scalar(&replacement, 1.0, 1.0, 10.0, 8)
                .is_err()
        );
        registry.clear_scene();
        assert!(
            registry
                .record_bound_scalar(&_reused, 1.0, 1.0, 10.0, 8)
                .is_err()
        );
    }

    #[test]
    #[ignore = "manual named versus bound recorder timing"]
    fn bound_recording_diagnostic() {
        let names: Vec<_> = (0..4096)
            .map(|index| {
                SignalRef::global(format!(
                    "participant.subsystem.continuous_state.variable_{index:05}"
                ))
            })
            .collect();
        for bound in [false, true] {
            let mut registry = SignalRegistry::default();
            let bindings: Vec<_> = names
                .iter()
                .map(|name| registry.admit_scalar(name, 16, names.len()).unwrap())
                .collect();
            let started = std::time::Instant::now();
            for tick in 0..64 {
                for (index, name) in names.iter().enumerate() {
                    let time = f64::from(tick);
                    let value = index as f64 + time;
                    let accepted = if bound {
                        registry
                            .record_bound_scalar(&bindings[index], time, value, 10.0, 16)
                            .unwrap()
                    } else {
                        registry.record_scalar_at_rate(name, time, value, 10.0, 16)
                    };
                    assert!(accepted);
                }
            }
            println!(
                "RECORDING bound={bound} ns={}",
                started.elapsed().as_nanos()
            );
            for (index, name) in names.iter().enumerate() {
                let history = registry.scalar_history(name).unwrap();
                assert_eq!(history.len(), 16);
                for (offset, sample) in history.iter().enumerate() {
                    let time = (48 + offset) as f64;
                    assert_eq!(sample.time.to_bits(), time.to_bits());
                    assert_eq!(sample.value.to_bits(), (index as f64 + time).to_bits());
                }
            }
        }
    }

    #[test]
    fn active_scalar_admission_tracks_archival_reactivation_and_removal() {
        let mut registry = SignalRegistry::default();
        let a = SignalRef::new(Entity::from_raw_u32(1).unwrap(), "a");
        let b = SignalRef::new(a.entity, "b");
        let c = SignalRef::new(Entity::from_raw_u32(2).unwrap(), "c");
        for signal in [&a, &b, &c] {
            registry.push_scalar(signal.clone(), 0.0, 1.0);
        }
        assert_eq!(registry.active_scalar_count(), 3);
        registry.descriptor_changes.clear();
        registry.deactivate_signal(&a);
        registry.deactivate_signal(&a);
        assert_eq!(registry.active_scalar_count(), 2);
        assert!(registry.descriptor_changes.contains(&a));
        assert_eq!(registry.scalar_history(&a).unwrap().len(), 1);

        // A rejected sample cannot reactivate an archived publisher.
        assert!(!registry.record_scalar_at_rate(&a, 0.0, 2.0, 5.0, 10));
        assert_eq!(registry.active_scalar_count(), 2);
        registry.descriptor_changes.clear();
        assert!(registry.record_scalar_at_rate(&a, 0.2, 2.0, 5.0, 10));
        assert_eq!(registry.active_scalar_count(), 3);
        assert!(registry.descriptor_changes.contains(&a));

        registry.deactivate_entity(a.entity);
        registry.deactivate_entity(a.entity);
        assert_eq!(registry.active_scalar_count(), 1);
        assert_eq!(registry.scalar_count(), 3);
        registry.remove_signal(&a);
        registry.remove_signal(&a);
        assert_eq!(registry.active_scalar_count(), 1);
        registry.drop_entity(b.entity);
        assert_eq!(registry.active_scalar_count(), 1);
        registry.drop_entity(c.entity);
        assert_eq!(registry.active_scalar_count(), 0);
        assert_eq!(registry.scalar_count(), 0);

        registry.push_scalar(c.clone(), 0.0, 1.0);
        registry.remove_signal(&c);
        assert_eq!(registry.active_scalar_count(), 0);
        registry.update_meta(c.clone(), SignalMeta::default());
        registry.descriptor_changes.clear();
        registry.remove_signal(&c);
        assert!(registry.descriptor_changes.contains(&c));
    }

    #[test]
    fn scene_lifecycle_retires_live_and_archived_signals_with_removal_notifications() {
        let mut app = App::new();
        app.add_plugins(SignalRegistryPlugin);
        let application = SignalRef::new(Entity::PLACEHOLDER, "application_health");
        let live = SignalRef::new(app.world_mut().spawn_empty().id(), "live");
        {
            let mut registry = app.world_mut().resource_mut::<SignalRegistry>();
            registry.retain_across_scenes(application.clone());
            registry.push_scalar(application.clone(), 0.0, 60.0);
        }
        for cycle in 1..=3 {
            let archived = SignalRef::new(app.world_mut().spawn(SignalSource).id(), "archived");
            let metadata_only = SignalRef::new(live.entity, "pending_metadata");
            {
                let mut registry = app.world_mut().resource_mut::<SignalRegistry>();
                for signal in [&live, &archived] {
                    registry.push_scalar(signal.clone(), 0.0, cycle as f64);
                    registry.associate_global_owner(signal, GlobalEntityId::from_raw(cycle));
                }
                registry.update_meta(metadata_only.clone(), SignalMeta::default());
                registry.associate_global_owner(&metadata_only, GlobalEntityId::from_raw(cycle));
                assert_eq!(registry.scalar_history(&live).unwrap().len(), 1);
            }
            app.world_mut().despawn(archived.entity);
            assert!(
                !app.world()
                    .resource::<SignalRegistry>()
                    .is_active(&archived)
            );
            let revision = app.world().resource::<SignalRegistry>().catalog_revision();
            app.world_mut()
                .resource_mut::<SignalRegistry>()
                .descriptor_changes
                .clear();
            lunco_core::run_scene_teardown(app.world_mut());
            let registry = app.world().resource::<SignalRegistry>();
            assert_eq!(registry.scalar_count(), 1);
            assert_eq!(registry.active_scalar_count(), 1);
            assert_eq!(registry.scalar_history(&application).unwrap().len(), 1);
            assert!(registry.catalog_revision() > revision);
            for signal in [&live, &archived, &metadata_only] {
                assert!(registry.scalar_history(signal).is_none());
                assert!(registry.signal_type(signal).is_none());
                assert!(registry.global_owner(signal).is_none());
                assert!(registry.meta(signal).is_none());
                assert!(registry.descriptor_changes.contains(signal));
            }
        }
    }

    #[test]
    fn incremental_catalog_notifications_coalesce_and_fan_out() {
        let mut app = App::new();
        app.add_plugins(SignalRegistryPlugin);
        let signal = SignalRef::new(Entity::from_raw_u32(1).unwrap(), "speed");
        {
            let mut registry = app.world_mut().resource_mut::<SignalRegistry>();
            registry.push_scalar(signal.clone(), 0.0, 1.0);
            registry.update_meta(
                signal.clone(),
                SignalMeta {
                    unit: Some("m/s".into()),
                    ..Default::default()
                },
            );
            registry.update_meta(
                signal.clone(),
                SignalMeta {
                    unit: Some("m/s".into()),
                    ..Default::default()
                },
            );
        }
        app.update();
        let messages = app.world().resource::<Messages<SignalDescriptorsChanged>>();
        let mut first = bevy::ecs::message::MessageCursor::<SignalDescriptorsChanged>::default();
        let mut second = bevy::ecs::message::MessageCursor::<SignalDescriptorsChanged>::default();
        assert_eq!(
            first
                .read(messages)
                .flat_map(|event| event.signals.iter())
                .collect::<Vec<_>>(),
            vec![&signal]
        );
        assert_eq!(
            second
                .read(messages)
                .flat_map(|event| event.signals.iter())
                .collect::<Vec<_>>(),
            vec![&signal]
        );
        {
            let mut registry = app.world_mut().resource_mut::<SignalRegistry>();
            registry.push_scalar(signal.clone(), 1.0, 2.0);
            registry.associate_global_owner(&signal, GlobalEntityId::from_raw(42));
            assert!(registry.descriptor_changes.is_empty());
            registry.deactivate_signal(&signal);
            assert_eq!(registry.descriptor_changes.len(), 1);
        }
        app.update();
        assert_eq!(
            first
                .read(app.world().resource::<Messages<SignalDescriptorsChanged>>())
                .count(),
            1
        );
        app.world_mut()
            .resource_mut::<SignalRegistry>()
            .remove_signal(&signal);
        app.update();
        assert_eq!(
            first
                .read(app.world().resource::<Messages<SignalDescriptorsChanged>>())
                .flat_map(|event| event.signals.iter())
                .collect::<Vec<_>>(),
            vec![&signal]
        );
        app.world_mut()
            .resource_mut::<SignalRegistry>()
            .remove_signal(&signal);
        assert!(
            app.world()
                .resource::<SignalRegistry>()
                .descriptor_changes
                .is_empty()
        );
    }

    #[test]
    fn telemetry_deadband_combines_absolute_and_relative_tolerance() {
        let deadband = TelemetryDeadband {
            atol: 1e-4,
            rtol: 1e-3,
        };

        assert!(!deadband.changed(0.0, 5e-5));
        assert!(deadband.changed(0.0, 2e-4));
        assert!(!deadband.changed(100.0, 100.05));
        assert!(deadband.changed(100.0, 100.2));
    }

    #[test]
    fn telemetry_deadband_does_not_hide_non_finite_values() {
        let deadband = TelemetryDeadband::default();
        assert!(deadband.changed(1.0, f64::NAN));
        assert!(deadband.changed(f64::INFINITY, 1.0));
    }

    #[test]
    fn telemetry_deadband_rejects_invalid_tolerances() {
        assert!(TelemetryDeadband::try_new(0.0, 0.0).is_ok());
        assert!(TelemetryDeadband::try_new(-1.0, 0.0).is_err());
        assert!(TelemetryDeadband::try_new(0.0, f64::NAN).is_err());
        assert!(
            TelemetryDeadband {
                atol: -1.0,
                rtol: 0.0
            }
            .changed(1.0, 2.0),
            "an invalid policy must never suppress a sample"
        );
    }

    #[test]
    fn a_full_history_drops_the_oldest_sample() {
        let mut h = ScalarHistory::new(3);
        for i in 0..5 {
            h.push(ScalarSample {
                time: i as f64,
                value: i as f64,
            });
        }
        assert_eq!(h.len(), 3);
        assert_eq!(
            h.iter().next().unwrap().time,
            2.0,
            "the two oldest must have been evicted"
        );
    }

    /// Retention is per-signal and re-authorable: shrinking must drop the oldest samples
    /// immediately, not lazily on the next push.
    #[test]
    fn shrinking_capacity_evicts_immediately() {
        let mut h = ScalarHistory::new(10);
        for i in 0..10 {
            h.push(ScalarSample {
                time: i as f64,
                value: 0.0,
            });
        }
        h.set_capacity(4);
        assert_eq!(h.len(), 4);
        assert_eq!(h.iter().next().unwrap().time, 6.0);
    }

    /// A zero capacity would make `push` spin forever popping an empty deque.
    #[test]
    fn a_zero_capacity_is_clamped_not_fatal() {
        let mut h = ScalarHistory::new(0);
        h.push(ScalarSample {
            time: 0.0,
            value: 1.0,
        });
        assert_eq!(h.len(), 1);
        h.set_capacity(0);
        h.push(ScalarSample {
            time: 1.0,
            value: 2.0,
        });
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
        assert_eq!(
            reg.scalar_history(&b).unwrap().iter().next().unwrap().value,
            2.0
        );
    }

    /// Persistence must carry the stable id, never `Entity` bits: resolving in a
    /// "new session" (different bits, same gid) must find the signal again.
    #[test]
    fn a_persisted_ref_survives_new_entity_bits() {
        let gid = GlobalEntityId::from_raw(42);
        let old = SignalRef::new(Entity::from_raw_u32(7).unwrap(), "motor_current");
        let persisted = old.to_persisted(|_| Some(gid)).unwrap();
        assert_eq!(persisted.entity, Some(gid));

        let reborn = Entity::from_raw_u32(99).unwrap();
        let resolved = persisted.resolve(|g| (g == gid).then_some(reborn)).unwrap();
        assert_eq!(resolved, SignalRef::new(reborn, "motor_current"));
    }

    #[test]
    fn an_archived_signal_keeps_its_stable_api_owner() {
        let entity = Entity::from_raw_u32(7).unwrap();
        let signal = SignalRef::new(entity, "motor_current");
        let owner = GlobalEntityId::from_raw(42);
        let mut registry = SignalRegistry::default();

        registry.push_scalar(signal.clone(), 0.0, 1.0);
        registry.associate_global_owner(&signal, owner);
        let revision = registry.catalog_revision();
        registry.associate_global_owner(&signal, owner);
        assert_eq!(registry.catalog_revision(), revision);
        registry.deactivate_entity(entity);

        assert_eq!(registry.global_owner(&signal), Some(owner));
        assert!(!registry.is_active(&signal));
        assert_eq!(registry.scalar_history(&signal).unwrap().len(), 1);
    }

    /// A global signal has no entity to stabilise — it round-trips as `None` and
    /// resolves without consulting the lookup.
    #[test]
    fn a_global_ref_persists_without_an_id() {
        let sig = SignalRef::global("sim_time");
        let persisted = sig.to_persisted(|_| unreachable!()).unwrap();
        assert_eq!(persisted.entity, None);
        let resolved = persisted.resolve(|_| unreachable!()).unwrap();
        assert_eq!(resolved, sig);
    }

    /// An entity with no stable id cannot be persisted — better no ref than a
    /// phantom one.
    #[test]
    fn a_ref_without_a_stable_id_refuses_to_persist() {
        let sig = SignalRef::new(Entity::from_raw_u32(3).unwrap(), "x");
        assert!(sig.to_persisted(|_| None).is_none());
    }

    #[test]
    fn a_non_finite_sample_is_dropped() {
        let mut reg = SignalRegistry::default();
        let s = SignalRef::global("nan");
        reg.push_scalar(s.clone(), 0.0, f64::NAN);
        reg.push_scalar(s.clone(), 1.0, f64::INFINITY);
        assert!(
            reg.scalar_history(&s).is_none(),
            "NaN/inf must never enter a plot buffer"
        );
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

    #[test]
    fn shared_runtime_sampler_owns_rate_deadband_and_time_reversal() {
        let mut reg = SignalRegistry::default();
        let signal = SignalRef::global("sim_time");
        let deadband = TelemetryDeadband {
            atol: 0.01,
            rtol: 0.0,
        };

        assert!(reg.retain_scalar_if_changed(signal.clone(), 0.0, 1.0, 10.0, deadband, 8,));
        assert!(!reg.retain_scalar_if_changed(signal.clone(), 0.05, 2.0, 10.0, deadband, 8,));
        assert!(!reg.retain_scalar_if_changed(signal.clone(), 0.11, 1.005, 10.0, deadband, 8,));
        assert!(reg.retain_scalar_if_changed(signal.clone(), 0.11, 2.0, 10.0, deadband, 8,));

        // A seek/reload cannot leave a history whose timestamps belong to the
        // previous simulation segment.
        assert!(reg.retain_scalar_if_changed(signal.clone(), 0.0, 3.0, 10.0, deadband, 8,));
        let history = reg.scalar_history(&signal).unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history.back().unwrap().value, 3.0);
    }

    #[test]
    fn recording_preserves_rate_history_lifecycle_and_retention() {
        let mut reg = SignalRegistry::default();
        let signal = SignalRef::global("steady");
        assert!(reg.record_scalar_at_rate(&signal, 0.0, 1.0, 10.0, 8));
        assert!(!reg.record_scalar_at_rate(&signal, 0.05, 1.0, 10.0, 8));
        assert!(reg.record_scalar_at_rate(&signal, 0.11, 1.0, 10.0, 8));
        let history = reg.scalar_history(&signal).unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(history.back().unwrap().time, 0.11);

        reg.deactivate_signal(&signal);
        let catalog = reg.catalog_revision();
        assert!(!reg.record_scalar_at_rate(&signal, 0.12, 2.0, 10.0, 1));
        assert!(!reg.is_active(&signal));
        assert_eq!(reg.catalog_revision(), catalog);
        assert_eq!(reg.scalar_history(&signal).unwrap().capacity, 8);

        assert!(reg.record_scalar_at_rate(&signal, 0.22, 2.0, 10.0, 1));
        assert!(reg.is_active(&signal));
        assert_eq!(reg.catalog_revision(), catalog.wrapping_add(1));
        assert_eq!(reg.scalar_history(&signal).unwrap().len(), 1);

        // A seek starts a new segment without losing channel identity or owner.
        reg.associate_global_owner(&signal, GlobalEntityId::from_raw(42));
        let catalog = reg.catalog_revision();
        assert!(reg.record_scalar_at_rate(&signal, 0.0, 3.0, 10.0, 4));
        let history = reg.scalar_history(&signal).unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history.back().unwrap().time, 0.0);
        assert_eq!(history.back().unwrap().value, 3.0);
        assert_eq!(history.capacity, 4);
        assert_eq!(
            reg.global_owner(&signal),
            Some(GlobalEntityId::from_raw(42))
        );
        assert_eq!(reg.catalog_revision(), catalog);

        for (time, value, rate) in [
            (f64::NAN, 1.0, 10.0),
            (0.2, f64::INFINITY, 10.0),
            (0.2, 1.0, 0.0),
        ] {
            assert!(!reg.record_scalar_at_rate(&signal, time, value, rate, 1));
        }
        assert_eq!(reg.scalar_history(&signal).unwrap().capacity, 4);
        assert_eq!(reg.scalar_history(&signal).unwrap().len(), 1);
    }
}
