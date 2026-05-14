//! Backend-agnostic experiment / batch-run registry.
//!
//! See `docs/architecture/25-experiments.md` for design rationale.
//!
//! An [`Experiment`] is one batch run of a model: a set of parameter
//! overrides, a [`RunBounds`] window, and (once finished) a
//! [`RunResult`]. Experiments live in an [`ExperimentRegistry`], scoped
//! per [`TwinId`]; the registry caps each twin at 20 runs and evicts
//! the oldest finished run on overflow.
//!
//! The simulation backend is plugged in via the [`ExperimentRunner`]
//! trait. This crate has no rumoca / modelica dependency; the binding
//! lives in `lunco-modelica`. Future backends (FMU, codegen, remote)
//! plug in the same way.

use std::collections::BTreeMap;
use web_time::SystemTime;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[cfg(feature = "bevy")]
use bevy::prelude::*;

// ---------- IDs and references ----------

/// Stable id for one experiment / run record.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct ExperimentId(pub Uuid);

impl ExperimentId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for ExperimentId {
    fn default() -> Self {
        Self::new()
    }
}

/// Twin scope for experiment grouping. Stringly-typed in v1 so the
/// crate doesn't depend on `lunco-twin`. Once `lunco-twin` exposes a
/// proper `TwinId`, migrate to that.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct TwinId(pub String);

/// Opaque reference to a model. The runner crate interprets this; the
/// experiments crate does not. For lunco-modelica, this is typically a
/// fully-qualified Modelica class name plus the source/document
/// identity needed to recompile.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct ModelRef(pub String);

/// Dotted Modelica path: `rocket.engine.thrust`.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct ParamPath(pub String);

// ---------- Parameter values ----------

/// Type-tagged parameter override value.
///
/// v1 covers Modelica scalars + enumerations + 1D Real arrays. Records
/// and N-D arrays are deferred — no current call site needs them, and
/// the override path (string injection in v1) doesn't support them
/// cleanly.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ParamValue {
    Real(f64),
    Int(i64),
    Bool(bool),
    String(String),
    /// Modelica `enumeration` literal — the unqualified literal name.
    Enum(String),
    /// 1D Real array.
    RealArray(Vec<f64>),
}

// ---------- Bounds ----------

/// Run window + solver hints. Fields default from the model's
/// `experiment(...)` annotation when available; otherwise the runner
/// supplies sane defaults.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RunBounds {
    pub t_start: f64,
    pub t_end: f64,
    /// `None` means the solver chooses adaptively.
    pub dt: Option<f64>,
    pub tolerance: Option<f64>,
    /// Backend-defined solver name. UI doesn't pick one in v1.
    pub solver: Option<String>,
}

impl Default for RunBounds {
    fn default() -> Self {
        Self {
            t_start: 0.0,
            t_end: 1.0,
            dt: None,
            tolerance: None,
            solver: None,
        }
    }
}

// ---------- Status ----------

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum RunStatus {
    Pending,
    Running { t_current: f64 },
    Done { wall_time_ms: u64 },
    /// `partial` is true when a partial trajectory was salvaged before
    /// the failure (kept in `result`).
    Failed { error: String, partial: bool },
    Cancelled,
}

impl RunStatus {
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            RunStatus::Done { .. } | RunStatus::Failed { .. } | RunStatus::Cancelled
        )
    }
}

// ---------- Result ----------

/// Trajectory result. Series keyed by dotted Modelica variable path.
/// `times.len() == series[k].len()` for every k.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunResult {
    pub times: Vec<f64>,
    pub series: BTreeMap<String, Vec<f64>>,
    pub meta: RunMeta,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RunMeta {
    pub wall_time_ms: u64,
    pub sample_count: usize,
    /// Backend-specific notes (solver used, step count, etc.). Free-form.
    pub notes: Option<String>,
}

// ---------- Experiment ----------

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Experiment {
    pub id: ExperimentId,
    pub twin_id: TwinId,
    pub model_ref: ModelRef,
    /// Display name. Auto-set to `<model> — N` on creation; user-editable.
    pub name: String,
    pub overrides: BTreeMap<ParamPath, ParamValue>,
    /// Input values for Modelica `input` declarations. Stored
    /// separately from parameter overrides because the source
    /// rewrite is different (input declaration → parameter
    /// declaration with a fixed value).
    #[serde(default)]
    pub inputs: BTreeMap<ParamPath, ParamValue>,
    pub bounds: RunBounds,
    pub status: RunStatus,
    pub result: Option<RunResult>,
    pub created_at: SystemTime,
    /// Plot-color hint stable across the run's lifetime. Index into a
    /// palette chosen by the UI; allocated when the experiment is
    /// first inserted into the registry.
    pub color_hint: u8,
}

// ---------- Registry ----------

/// Per-twin cap. v1: 20 finished runs per twin, oldest evicted.
/// In-flight runs (Pending / Running) never count against the cap and
/// are never evicted.
pub const REGISTRY_CAP_PER_TWIN: usize = 20;

/// Process-wide experiment store, keyed by twin.
#[cfg_attr(feature = "bevy", derive(Resource))]
#[derive(Default, Debug)]
pub struct ExperimentRegistry {
    by_twin: BTreeMap<TwinId, Vec<Experiment>>,
    /// Monotonic counter for auto-name suffixes per (twin, model).
    name_counter: BTreeMap<(TwinId, ModelRef), u32>,
    /// Color rotation index per twin.
    color_counter: BTreeMap<TwinId, u8>,
}

impl ExperimentRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Auto-generate a name `<model> — N` and a color hint, then insert.
    /// Returns the assigned id.
    pub fn insert_new(
        &mut self,
        twin_id: TwinId,
        model_ref: ModelRef,
        overrides: BTreeMap<ParamPath, ParamValue>,
        inputs: BTreeMap<ParamPath, ParamValue>,
        bounds: RunBounds,
    ) -> ExperimentId {
        let n = self
            .name_counter
            .entry((twin_id.clone(), model_ref.clone()))
            .and_modify(|c| *c += 1)
            .or_insert(1);
        // "Run N" is short enough for plot legends; the model class
        // belongs in row metadata, not every legend entry.
        let name = format!("Run {}", n);

        let color_hint = {
            let c = self.color_counter.entry(twin_id.clone()).or_insert(0);
            let v = *c;
            *c = c.wrapping_add(1);
            v
        };

        let exp = Experiment {
            id: ExperimentId::new(),
            twin_id: twin_id.clone(),
            model_ref,
            name,
            overrides,
            inputs,
            bounds,
            status: RunStatus::Pending,
            result: None,
            created_at: SystemTime::now(),
            color_hint,
        };
        let id = exp.id;
        let bucket = self.by_twin.entry(twin_id).or_default();
        bucket.push(exp);
        Self::evict_if_needed_in(bucket);
        id
    }

    fn evict_if_needed_in(bucket: &mut Vec<Experiment>) {
        // Cap counts only terminal runs. If terminal count exceeds cap,
        // evict oldest terminal.
        let terminal_count = bucket.iter().filter(|e| e.status.is_terminal()).count();
        if terminal_count <= REGISTRY_CAP_PER_TWIN {
            return;
        }
        // Find oldest terminal by created_at and remove it.
        if let Some((idx, _)) = bucket
            .iter()
            .enumerate()
            .filter(|(_, e)| e.status.is_terminal())
            .min_by_key(|(_, e)| e.created_at)
        {
            bucket.remove(idx);
        }
    }

    pub fn get(&self, id: ExperimentId) -> Option<&Experiment> {
        self.by_twin.values().flatten().find(|e| e.id == id)
    }

    pub fn get_mut(&mut self, id: ExperimentId) -> Option<&mut Experiment> {
        self.by_twin.values_mut().flatten().find(|e| e.id == id)
    }

    pub fn list_for_twin(&self, twin: &TwinId) -> &[Experiment] {
        self.by_twin
            .get(twin)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// Rewrite every experiment under `twin` whose `model_ref`
    /// equals `old` to instead reference `new`. Returns the count
    /// of touched records. Called by the class-rename observer so a
    /// `model Foo` → `model Bar` edit in the source doesn't strand
    /// the user's run history under a class name that no longer
    /// exists.
    pub fn rename_model_ref(
        &mut self,
        twin: &TwinId,
        old: &ModelRef,
        new: &ModelRef,
    ) -> usize {
        let mut hit = 0;
        if let Some(bucket) = self.by_twin.get_mut(twin) {
            for exp in bucket.iter_mut() {
                if exp.model_ref == *old {
                    exp.model_ref = new.clone();
                    hit += 1;
                }
            }
        }
        // Migrate the auto-name counter so the next "Run N" picks
        // up where the old class left off instead of resetting to 1.
        if let Some(count) = self.name_counter.remove(&(twin.clone(), old.clone())) {
            self.name_counter
                .entry((twin.clone(), new.clone()))
                .and_modify(|c| *c = (*c).max(count))
                .or_insert(count);
        }
        hit
    }

    /// Evict every experiment under `twin`, regardless of status.
    /// Returns the number of records removed. Callers should ensure
    /// no in-flight handles still reference the cleared ids (the
    /// drain system will silently drop updates for missing rows).
    pub fn delete_for_twin(&mut self, twin: &TwinId) -> usize {
        let removed = self
            .by_twin
            .remove(twin)
            .map(|v| v.len())
            .unwrap_or(0);
        self.name_counter.retain(|(t, _), _| t != twin);
        self.color_counter.remove(twin);
        removed
    }

    pub fn delete(&mut self, id: ExperimentId) -> bool {
        for bucket in self.by_twin.values_mut() {
            if let Some(pos) = bucket.iter().position(|e| e.id == id) {
                // Don't allow deleting in-flight runs.
                if !bucket[pos].status.is_terminal() {
                    return false;
                }
                bucket.remove(pos);
                return true;
            }
        }
        false
    }

    /// Apply a status transition. Caller is responsible for emitting
    /// any matching event (see lifecycle messages). Triggers eviction
    /// if the transition is into a terminal state and pushes the
    /// terminal count over the per-twin cap.
    pub fn set_status(&mut self, id: ExperimentId, status: RunStatus) -> bool {
        let became_terminal = status.is_terminal();
        let twin_id = match self.get_mut(id) {
            Some(e) => {
                e.status = status;
                e.twin_id.clone()
            }
            None => return false,
        };
        if became_terminal {
            if let Some(bucket) = self.by_twin.get_mut(&twin_id) {
                Self::evict_if_needed_in(bucket);
            }
        }
        true
    }

    pub fn set_result(&mut self, id: ExperimentId, result: RunResult) -> bool {
        if let Some(e) = self.get_mut(id) {
            e.result = Some(result);
            true
        } else {
            false
        }
    }
}

// ---------- Runner trait ----------

/// Lifecycle update streamed from a runner back to the host.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum RunUpdate {
    Progress { t_current: f64 },
    Completed(RunResult),
    Failed { error: String, partial: Option<RunResult> },
    Cancelled,
}

/// Handle to an in-flight run. The host drains `progress_rx` to
/// observe progress / completion; `cancel` requests early stop with
/// best-effort latency (≤100 ms target).
pub struct RunHandle {
    pub run_id: ExperimentId,
    pub progress_rx: crossbeam_channel::Receiver<RunUpdate>,
    /// Trait-object cancel hook so the host can request cancellation
    /// without holding a reference to the runner.
    pub cancel: Box<dyn Fn() + Send + Sync>,
}

impl RunHandle {
    pub fn cancel(&self) {
        (self.cancel)()
    }
}

/// Backend interface. One implementation per simulation backend
/// (rumoca, FMU, codegen, …). The runner is responsible for honoring
/// `Experiment::overrides` and `Experiment::bounds` exactly.
pub trait ExperimentRunner: Send + Sync {
    /// Kick off a fast (batch) run. Returns immediately with a handle;
    /// the actual work happens off-thread (native: std::thread; wasm:
    /// Web Worker). Concurrency: at most one fast run per runner
    /// instance is in flight at any time; a second call while another
    /// is active is implementation-defined (lunco-modelica queues).
    fn run_fast(&self, exp: &Experiment) -> RunHandle;

    /// Read default bounds from the model's `experiment(...)`
    /// annotation. Returns `None` when the runner can't determine
    /// defaults (e.g., model not yet compiled). UI falls back to
    /// `RunBounds::default()`.
    fn default_bounds(&self, model: &ModelRef) -> Option<RunBounds>;
}

// ---------- Bevy events ----------

#[cfg(feature = "bevy")]
#[derive(Message, Clone, Debug)]
pub struct RunRequested {
    pub experiment_id: ExperimentId,
}

#[cfg(feature = "bevy")]
#[derive(Message, Clone, Debug)]
pub struct RunProgress {
    pub experiment_id: ExperimentId,
    pub t_current: f64,
}

#[cfg(feature = "bevy")]
#[derive(Message, Clone, Debug)]
pub struct RunCompleted {
    pub experiment_id: ExperimentId,
}

#[cfg(feature = "bevy")]
#[derive(Message, Clone, Debug)]
pub struct RunFailed {
    pub experiment_id: ExperimentId,
    pub error: String,
}

#[cfg(feature = "bevy")]
#[derive(Message, Clone, Debug)]
pub struct RunCancelled {
    pub experiment_id: ExperimentId,
}

/// Plugin that registers the registry resource + run lifecycle events.
/// Runners are NOT registered here; the binding crate
/// (`lunco-modelica`) inserts its own `ExperimentRunner` resource.
#[cfg(feature = "bevy")]
pub struct ExperimentsPlugin;

#[cfg(feature = "bevy")]
impl Plugin for ExperimentsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ExperimentRegistry>()
            .add_message::<RunRequested>()
            .add_message::<RunProgress>()
            .add_message::<RunCompleted>()
            .add_message::<RunFailed>()
            .add_message::<RunCancelled>();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_assigns_unique_names() {
        let mut reg = ExperimentRegistry::new();
        let twin = TwinId("t".into());
        let model = ModelRef("M".into());
        let id1 = reg.insert_new(twin.clone(), model.clone(), Default::default(), Default::default(), Default::default());
        let id2 = reg.insert_new(twin.clone(), model.clone(), Default::default(), Default::default(), Default::default());
        assert_ne!(id1, id2);
        let names: Vec<_> = reg.list_for_twin(&twin).iter().map(|e| e.name.clone()).collect();
        assert_eq!(names, vec!["Run 1", "Run 2"]);
    }

    #[test]
    fn registry_caps_terminal_runs() {
        let mut reg = ExperimentRegistry::new();
        let twin = TwinId("t".into());
        let model = ModelRef("M".into());
        for _ in 0..(REGISTRY_CAP_PER_TWIN + 5) {
            let id = reg.insert_new(twin.clone(), model.clone(), Default::default(), Default::default(), Default::default());
            reg.set_status(id, RunStatus::Done { wall_time_ms: 0 });
        }
        assert_eq!(reg.list_for_twin(&twin).len(), REGISTRY_CAP_PER_TWIN);
    }

    #[test]
    fn in_flight_runs_not_evicted() {
        let mut reg = ExperimentRegistry::new();
        let twin = TwinId("t".into());
        let model = ModelRef("M".into());
        // Fill with terminal first
        for _ in 0..REGISTRY_CAP_PER_TWIN {
            let id = reg.insert_new(twin.clone(), model.clone(), Default::default(), Default::default(), Default::default());
            reg.set_status(id, RunStatus::Done { wall_time_ms: 0 });
        }
        // Now add an in-flight one, which should NOT trigger eviction of itself
        let live = reg.insert_new(twin.clone(), model.clone(), Default::default(), Default::default(), Default::default());
        reg.set_status(live, RunStatus::Running { t_current: 0.0 });
        // Adding more terminal ones should evict from the terminal set
        for _ in 0..3 {
            let id = reg.insert_new(twin.clone(), model.clone(), Default::default(), Default::default(), Default::default());
            reg.set_status(id, RunStatus::Done { wall_time_ms: 0 });
        }
        // Live run still present
        assert!(reg.get(live).is_some());
    }

    #[test]
    fn delete_terminal_only() {
        let mut reg = ExperimentRegistry::new();
        let twin = TwinId("t".into());
        let model = ModelRef("M".into());
        let id = reg.insert_new(twin, model, Default::default(), Default::default(), Default::default());
        // Pending — refuse delete
        assert!(!reg.delete(id));
        reg.set_status(id, RunStatus::Done { wall_time_ms: 0 });
        assert!(reg.delete(id));
    }
}
