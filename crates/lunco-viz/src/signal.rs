//! Signal model: what data looks like *before* any viz touches it.
//!
//! A [`Signal`] is a typed, time-varying datum identified by a
//! [`SignalRef`]. Producers (the Modelica worker, the Avian bridge,
//! script-defined derived signals) push samples into [`SignalRegistry`];
//! viz kinds read from the same registry.
//!
//! This layer knows nothing about Modelica or Avian. A "signal" is just
//! (`ref`, `type`, `history`) plus optional metadata (description, unit,
//! provenance). Extending to new signal shapes (Vec3, Pose, Event, …) is
//! a variant on [`SignalType`] and a new `push_*` method on the registry.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};

/// Stable identity for a signal across frames / save-load cycles.
///
/// Current encoding: the Bevy `Entity` that owns the signal plus a
/// dotted path. An entity of [`Entity::PLACEHOLDER`] means "global / no
/// entity" (e.g. the simulation clock, top-level events).
///
/// `path` is a free-form string so different domains can structure it
/// however makes sense: Modelica uses variable names (`m_prop`,
/// `inputs.throttle`); Avian will use `body.<component>.<field>`
/// (`body.linear_velocity`); scripts can define `derived.<name>`.
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

    /// Global signal not tied to a specific entity (e.g. simulation
    /// clock, workspace-level events).
    pub fn global(path: impl Into<String>) -> Self {
        Self { entity: Entity::PLACEHOLDER, path: path.into() }
    }
}

// Minimal serde glue for `Entity`. Bevy's reflect serializer handles
// this elsewhere, but we want plain TOML / JSON friendliness for
// workspace files. Round-trips through u64 bit-pattern.
mod entity_as_u64 {
    use bevy::prelude::Entity;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(e: &Entity, s: S) -> Result<S::Ok, S::Error> {
        e.to_bits().serialize(s)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Entity, D::Error> {
        let bits = u64::deserialize(d)?;
        Ok(Entity::from_bits(bits))
    }
}

/// What shape the samples take. Viz kinds declare which of these they
/// accept per role; the registry stores them accordingly.
///
/// Extending: add a variant here, add a matching `push_*` / `history_*`
/// pair on [`SignalRegistry`], and teach relevant viz kinds to consume
/// it. No other place needs to change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SignalType {
    /// Continuous `f64` time-series. Covers the majority of Modelica
    /// variables (states, parameters, algebraics, inputs).
    Scalar,
    // Reserved for future work — shape locked in so viz-kind trait
    // compatibility checks can reference them today.
    Vec3,
    Pose,
    Event,
}

/// Descriptive metadata attached to a signal. Optional and non-load-
/// bearing — viz kinds render without it, but tooltips, legends, and
/// axis labels get better when it's populated.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SignalMeta {
    /// Human-readable description, typically the Modelica `"..."`
    /// description string (MLS §A.2.5).
    pub description: Option<String>,
    /// Physical unit, e.g. `"kg"`, `"N"`, `"m/s"`. Reserved — Modelica
    /// doesn't currently expose units through the compile pipeline, but
    /// the field is here so Avian / user-declared signals can provide
    /// them.
    pub unit: Option<String>,
    /// Free-form tag naming who created the signal: `"modelica"`,
    /// `"avian"`, `"script"`. Lets the inspector group signals by
    /// provenance.
    pub provenance: Option<String>,
}

/// One (time, value) pair for a [`SignalType::Scalar`] signal. Other
/// signal types will get their own `Sample` variants as they land.
#[derive(Debug, Clone, Copy)]
pub struct ScalarSample {
    pub time: f64,
    pub value: f64,
}

/// Ring-buffer-backed history for one scalar signal.
///
/// The default capacity mirrors `WorkbenchState.max_history` (2000
/// samples) so the Modelica Graphs panel sees the same horizon when it
/// reads from here instead of `WorkbenchState.history`.
#[derive(Debug, Clone)]
pub struct ScalarHistory {
    pub samples: VecDeque<ScalarSample>,
    pub capacity: usize,
}

impl ScalarHistory {
    pub fn new(capacity: usize) -> Self {
        Self {
            samples: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    pub fn push(&mut self, sample: ScalarSample) {
        if self.samples.len() >= self.capacity {
            self.samples.pop_front();
        }
        self.samples.push_back(sample);
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
///
/// Producers call [`push_scalar`](Self::push_scalar) (or the equivalent
/// for other types as they land). Viz kinds call
/// [`scalar_history`](Self::scalar_history) to read.
///
/// `meta` is keyed the same way as `scalar_history` — one metadata
/// entry per `SignalRef`. Producers should call
/// [`update_meta`](Self::update_meta) once per signal-lifecycle event
/// (model compile, body spawn) rather than every frame.
#[derive(Resource, Debug, Default)]
pub struct SignalRegistry {
    scalar_history: HashMap<SignalRef, ScalarHistory>,
    types: HashMap<SignalRef, SignalType>,
    meta: HashMap<SignalRef, SignalMeta>,
    default_capacity: usize,
}

impl SignalRegistry {
    pub fn with_default_capacity(capacity: usize) -> Self {
        Self {
            default_capacity: capacity,
            ..Default::default()
        }
    }

    /// Push a scalar (time, value) sample. Allocates the history
    /// buffer and records the type on first sample.
    pub fn push_scalar(&mut self, sig: SignalRef, time: f64, value: f64) {
        if !value.is_finite() {
            return;
        }
        let cap = if self.default_capacity == 0 { 2000 } else { self.default_capacity };
        let history = self
            .scalar_history
            .entry(sig.clone())
            .or_insert_with(|| ScalarHistory::new(cap));
        history.push(ScalarSample { time, value });
        self.types.entry(sig).or_insert(SignalType::Scalar);
    }

    /// Replace / insert the metadata for a signal. Producers typically
    /// call this once per compile / spawn event.
    pub fn update_meta(&mut self, sig: SignalRef, meta: SignalMeta) {
        self.meta.insert(sig, meta);
    }

    pub fn scalar_history(&self, sig: &SignalRef) -> Option<&ScalarHistory> {
        self.scalar_history.get(sig)
    }

    /// Iterate every (`SignalRef`, `ScalarHistory`) pair the registry
    /// holds. Used by per-frame snapshot builders that need to copy
    /// data into a `&dyn Any` carrier (e.g. canvas plot nodes whose
    /// `NodeVisual::draw` has no `World` access).
    pub fn iter_scalar(&self) -> impl Iterator<Item = (&SignalRef, &ScalarHistory)> {
        self.scalar_history.iter()
    }

    pub fn signal_type(&self, sig: &SignalRef) -> Option<SignalType> {
        self.types.get(sig).copied()
    }

    pub fn meta(&self, sig: &SignalRef) -> Option<&SignalMeta> {
        self.meta.get(sig)
    }

    /// All known signals (of any type). Used by the inspector UI to
    /// populate pick-lists.
    pub fn iter_signals(&self) -> impl Iterator<Item = (&SignalRef, SignalType)> {
        self.types.iter().map(|(r, t)| (r, *t))
    }

    /// Drop every signal owned by `entity`. Called when an entity
    /// despawns so stale references don't linger.
    pub fn drop_entity(&mut self, entity: Entity) {
        self.scalar_history.retain(|r, _| r.entity != entity);
        self.types.retain(|r, _| r.entity != entity);
        self.meta.retain(|r, _| r.entity != entity);
    }

    /// Clear the history of a specific signal without dropping its
    /// type / meta entry. Used on simulation reset.
    pub fn clear_history(&mut self, sig: &SignalRef) {
        if let Some(h) = self.scalar_history.get_mut(sig) {
            h.clear();
        }
    }
}

/// Build a CSV table from the given signals' histories.
///
/// Output is `time,<path1>,<path2>,...` with one row per distinct
/// timestamp across all included signals, ascending. Each column is
/// forward-filled (last known value) when that signal has no sample
/// at that exact timestamp; cells before a signal's first sample are
/// empty. Signals with no history are skipped.
pub fn export_signals_to_csv(
    registry: &SignalRegistry,
    signals: &[SignalRef],
    column_labels: &[String],
) -> String {
    let cols: Vec<(&str, &ScalarHistory)> = signals
        .iter()
        .zip(column_labels.iter())
        .filter_map(|(s, l)| registry.scalar_history(s).map(|h| (l.as_str(), h)))
        .collect();

    if cols.is_empty() {
        return String::from("time\n");
    }

    // Union all timestamps.
    let mut times: Vec<f64> = cols
        .iter()
        .flat_map(|(_, h)| h.samples.iter().map(|s| s.time))
        .collect();
    times.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    times.dedup_by(|a, b| (*a - *b).abs() < f64::EPSILON);

    // Header — quote labels containing commas or quotes.
    let mut out = String::from("time");
    for (label, _) in &cols {
        out.push(',');
        out.push_str(&csv_escape(label));
    }
    out.push('\n');

    // Per-column cursor (index of next unread sample).
    let mut cursors = vec![0usize; cols.len()];
    let mut last_val = vec![Option::<f64>::None; cols.len()];

    for &t in &times {
        out.push_str(&format!("{t}"));
        for (i, (_, hist)) in cols.iter().enumerate() {
            while cursors[i] < hist.samples.len() && hist.samples[cursors[i]].time <= t {
                last_val[i] = Some(hist.samples[cursors[i]].value);
                cursors[i] += 1;
            }
            out.push(',');
            if let Some(v) = last_val[i] {
                out.push_str(&format!("{v}"));
            }
        }
        out.push('\n');
    }
    out
}

fn csv_escape(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') {
        let escaped = s.replace('"', "\"\"");
        format!("\"{escaped}\"")
    } else {
        s.to_string()
    }
}

/// Deterministic colour for a signal path, shared across every plot
/// surface (panel `Graphs`, `VizPanel`, in-canvas `PlotNodeVisual`,
/// future inspector). Same `path` ⇒ same colour everywhere; stable
/// across sessions so a saved layout reopens with consistent legend
/// colours.
///
/// 12-entry Tab10/D3-derived palette via FNV-1a hash. Tweak only by
/// **appending** — keep existing indices stable so saved layouts
/// don't change colour after a palette edit.
pub fn color_for_signal(path: &str) -> bevy_egui::egui::Color32 {
    use bevy_egui::egui::Color32;
    const PALETTE: &[Color32] = &[
        Color32::from_rgb(0x1f, 0x77, 0xb4),
        Color32::from_rgb(0xff, 0x7f, 0x0e),
        Color32::from_rgb(0x2c, 0xa0, 0x2c),
        Color32::from_rgb(0xd6, 0x27, 0x28),
        Color32::from_rgb(0x94, 0x67, 0xbd),
        Color32::from_rgb(0x8c, 0x56, 0x4b),
        Color32::from_rgb(0xe3, 0x77, 0xc2),
        Color32::from_rgb(0x7f, 0x7f, 0x7f),
        Color32::from_rgb(0xbc, 0xbd, 0x22),
        Color32::from_rgb(0x17, 0xbe, 0xcf),
        Color32::from_rgb(0xae, 0xc7, 0xe8),
        Color32::from_rgb(0xff, 0xbb, 0x78),
    ];
    let mut h: u32 = 0x811c_9dc5;
    for b in path.as_bytes() {
        h ^= *b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    PALETTE[(h as usize) % PALETTE.len()]
}
