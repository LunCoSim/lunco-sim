//! Pure simulation-target & run-configuration resolution.
//!
//! This module holds the *decision logic* for two questions every
//! "run this model" surface must answer:
//!
//!   1. **Which class** do we simulate? (`default_class`)
//!   2. **What bounds** do we run it with? (`resolve_bounds`)
//!
//! These rules used to be inlined, and drifted, across the Fast Run popup,
//! the Experiments Setup form, and the `FastRunActiveModel`/`RunExperiment`
//! command handlers — N copies of the same precedence and the same
//! `Interval=0` sentinel handling. They now live here, once.
//!
//! Everything in this module is **pure**: no `World`, no Bevy resources, no
//! UI types. The `ui/` layer is responsible for *gathering* the inputs
//! (drill-in pin, draft override, runner cache, AST annotation) from live
//! ECS state and calling down into these functions. That keeps the
//! dependency arrow pointing the right way — UI depends on this, never the
//! reverse — and makes the resolution rules unit-testable without a `World`.

use lunco_experiments::RunBounds;

/// The fallback simulation horizon when nothing else supplies one (no draft,
/// no runner cache, no `experiment(...)` annotation). `1.0` is the Modelica
/// spec default for `experiment(StopTime=...)`. The single canonical value —
/// surfaces that display the default and the run that actually executes must
/// agree, so both read this.
pub const DEFAULT_STOP_TIME: f64 = 1.0;

/// Map a Modelica `experiment(Interval=...)` value to an output step (`dt`).
/// `Interval=0` is the spec's "unspecified" sentinel → `None`, so the run
/// loop derives the spec default (numberOfIntervals) instead of treating 0
/// as a real step. Shared by every annotation→bounds path so the sentinel
/// rule can't drift.
pub fn interval_to_dt(interval: Option<f64>) -> Option<f64> {
    interval.filter(|&i| i > 0.0)
}

/// Default `numberOfIntervals` when the `experiment` annotation supplies no
/// positive `Interval` — the Modelica spec's output-sampling default.
pub const NUM_INTERVALS: f64 = 500.0;

/// Non-spec safety backstop: a run never emits more than this many output
/// samples, regardless of the (derived or explicit) interval. On wasm the
/// heap is ~4 GB, so unbounded `Vec<f64>` sample accumulation OOM-traps the
/// worker — this clamps `dt` up so that can't happen. Well-formed models
/// never reach it.
pub const SAMPLE_CAP: f64 = 200_000.0;

/// Resolve the output sample spacing (`step_dt`) a stepping loop advances by,
/// from the resolved horizon and `Interval`. This is the SINGLE source of
/// truth shared by every run loop — native (`experiments_runner`) and the
/// wasm worker (`lunica_worker`) — so the spec rule and the memory backstop
/// can't drift between platforms. (They did: the worker kept a pathological
/// `unwrap_or(0.01)` long after native was fixed, which emitted ~10M samples
/// over a multi-day horizon and OOM-trapped the browser.)
///
///   * explicit positive `n_intervals` (Modelica `NumberOfIntervals`, the
///     "give me exactly N+1 points" option) → `span / n_intervals`. Wins over
///     `dt` when both are set.
///   * explicit positive `dt` (the `Interval` annotation) → honoured as given
///   * both missing / `<= 0` (the spec's 0 sentinel)      → `span / NUM_INTERVALS`
///   * degenerate zero-length span                         → `0.01`
///
/// then clamped up so `span / step_dt <= SAMPLE_CAP`.
pub fn resolve_step_dt(
    t_start: f64,
    t_end: f64,
    dt: Option<f64>,
    n_intervals: Option<u32>,
) -> f64 {
    let span = (t_end - t_start).max(0.0);
    let mut step_dt = match (n_intervals.filter(|&n| n > 0), dt) {
        // Count wins: N intervals over the span → N+1 evenly-spaced points.
        (Some(n), _) if span > 0.0 => span / n as f64,
        (_, Some(dt)) if dt > 0.0 => dt,
        _ if span > 0.0 => span / NUM_INTERVALS,
        _ => 0.01, // degenerate zero-length span; emit a couple of points
    };
    if span > 0.0 && step_dt > 0.0 && span / step_dt > SAMPLE_CAP {
        let capped = span / SAMPLE_CAP;
        bevy::log::warn!(
            "[sim] Interval={step_dt}s over span={span}s would emit {:.0} \
             samples (>{SAMPLE_CAP:.0}); clamping to Interval={capped}s",
            span / step_dt
        );
        step_dt = capped;
    }
    step_dt
}

/// The bounds used when no source supplies any: `[0, DEFAULT_STOP_TIME]`,
/// adaptive solver, no fixed output interval.
pub fn default_bounds() -> RunBounds {
    RunBounds {
        t_start: 0.0,
        t_end: DEFAULT_STOP_TIME,
        dt: None,
        n_intervals: None,
        tolerance: None,
        solver: None,
        h0: None,
        runtime: lunco_experiments::RuntimeMode::Batch,
    }
}

/// The class a simulation surface defaults to, in precedence order:
///   1. `drilled_in` — the UI drill-in pin; the user is looking at a leaf
///      model and expects *that* to run, not the enclosing package.
///   2. the first `candidate` — caller supplies the tier-ranked
///      [`simulation_candidates`](crate::index::ModelicaIndex::simulation_candidates)
///      list, where an `experiment(...)`-annotated, non-partial class sorts
///      first (NOT arbitrary `HashMap` order).
///
/// Returns `None` when there is no pin and no candidate. This deliberately
/// does *not* encode disambiguation-by-picker: a caller that wants to prompt
/// the user on multiple candidates inspects the candidate list itself and
/// layers that on top (see `dispatch_experiment`).
pub fn default_class(drilled_in: Option<&str>, candidates: &[String]) -> Option<String> {
    drilled_in
        .map(str::to_string)
        .or_else(|| candidates.first().cloned())
}

/// Why [`resolve_requested_class`] could not turn a caller-supplied name into
/// a single canonical class.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClassResolveError {
    /// No candidate matches by qualified name or by leaf name.
    Unknown,
    /// The leaf name matches several candidates in different packages; the
    /// caller must qualify (or disambiguate via the picker). Carries the
    /// matches so the message / picker can list them.
    Ambiguous(Vec<String>),
}

impl std::fmt::Display for ClassResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unknown => write!(f, "is not a simulatable class in this document"),
            Self::Ambiguous(m) => write!(f, "is ambiguous — matches {}", m.join(", ")),
        }
    }
}

/// Resolve a caller-supplied simulation target — fully qualified OR a bare
/// leaf name — to the canonical fully-qualified class, matched against the
/// document's simulatable `candidates` (themselves always qualified, from
/// [`simulation_candidates`](crate::index::ModelicaIndex::simulation_candidates)).
///
/// THE single source of class-name resolution shared by every "run this
/// class" surface (`CompileModel`, `FastRunActiveModel`, `RunExperiment`), so
/// they can't drift on how a name maps to a model. Precedence:
///   1. exact match — the request is already a valid qualified name.
///   2. unique leaf match — exactly one candidate whose last `.`-segment
///      equals the request. This is what lets a caller pass
///      `"RoverThermalSystem"` and reach `"LunarRover.RoverThermalSystem"`
///      instead of handing the compiler a name it rejects with the opaque
///      "model not found" at instantiate time.
///   3. otherwise [`ClassResolveError`] (unknown / ambiguous), so the caller
///      surfaces a clear, candidate-listing error rather than a deep compiler
///      failure.
pub fn resolve_requested_class(
    requested: &str,
    candidates: &[String],
) -> Result<String, ClassResolveError> {
    let req = requested.trim();
    // 1. Exact (already-qualified, or a dot-free top-level name) match.
    if let Some(hit) = candidates.iter().find(|c| c.as_str() == req) {
        return Ok(hit.clone());
    }
    // 2. Leaf-name match: the bare request equals a candidate's last
    //    `.`-segment (e.g. `"RoverThermalSystem"` → `"LunarRover.RoverThermalSystem"`).
    let leaf_hits: Vec<String> = candidates
        .iter()
        .filter(|c| c.rsplit('.').next() == Some(req))
        .cloned()
        .collect();
    match leaf_hits.len() {
        1 => return Ok(leaf_hits.into_iter().next().unwrap()),
        n if n > 1 => return Err(ClassResolveError::Ambiguous(leaf_hits)),
        _ => {}
    }
    // 3. Fully-qualified request that is a segment-aligned SUPERSET of an
    //    under-qualified candidate — i.e. a candidate is a trailing dotted
    //    suffix of the request. Return the request: it carries the full
    //    prefix the compiler needs. This is the drilled-MSL-class case: the
    //    pin is the true FQN `Modelica.Blocks.Examples.PID_Controller`, while
    //    the in-doc candidate is the `within`-relative
    //    `Blocks.Examples.PID_Controller` (the doc's `within Modelica.Blocks.
    //    Examples;` prefix isn't folded into the index's qualified names).
    //    Compiling the under-qualified candidate would fail "model not found".
    if req.contains('.') {
        let suffix_hits: Vec<&String> = candidates
            .iter()
            .filter(|c| {
                c.rsplit('.').next() == req.rsplit('.').next()
                    && req.ends_with(&format!(".{c}"))
            })
            .collect();
        if suffix_hits.len() == 1 {
            return Ok(req.to_string());
        }
    }
    Err(ClassResolveError::Unknown)
}

/// Map a model's `experiment(...)` annotation to [`RunBounds`]. `None` when
/// the annotation has no `StopTime` — a `StopTime` is what makes the
/// annotation usable as a run horizon.
pub fn bounds_from_experiment(exp: &crate::annotations::Experiment) -> Option<RunBounds> {
    let t_end = exp.stop_time?;
    let dt = interval_to_dt(exp.interval);
    Some(RunBounds {
        t_start: exp.start_time.unwrap_or(0.0),
        t_end,
        dt,
        // Modelica: `Interval` wins over `NumberOfIntervals` when both appear,
        // so only carry the count when no explicit interval was given.
        n_intervals: number_of_intervals_to_n(exp.number_of_intervals, dt),
        tolerance: exp.tolerance,
        solver: None,
        h0: None,
        runtime: lunco_experiments::RuntimeMode::Batch,
    })
}

/// Map a Modelica `NumberOfIntervals` value to the typed
/// [`RunBounds::n_intervals`](lunco_experiments::RunBounds::n_intervals).
/// Returns `None` when an explicit `Interval` (`dt`) is already present
/// (Interval wins) or the count is absent / non-positive.
pub fn number_of_intervals_to_n(number_of_intervals: Option<f64>, dt: Option<f64>) -> Option<u32> {
    if dt.is_some() {
        return None;
    }
    number_of_intervals
        .filter(|&n| n >= 1.0 && n.is_finite())
        .map(|n| n as u32)
}

/// Resolve the run bounds from the four precedence tiers, highest first:
///   1. `draft_override` — a value the user edited in a Setup form.
///   2. `annotation_bounds` — bounds derived from the AST `experiment(...)`
///      (see [`bounds_from_experiment`]). This is the deterministic,
///      always-fresh source and MUST outrank the async cache below.
///   3. `runner_cached` — the runner's `default_bounds` annotation cache,
///      populated *asynchronously* by the worker after a compile completes.
///      A pure fallback for paths with no live AST (e.g. headless / no doc
///      registry). It must never shadow a fresh annotation: the bounds a run
///      is frozen with at creation would otherwise depend on whether the
///      worker's `set_model_defaults` callback had landed yet — the source of
///      the "flaky terminator" race (same experiment, different tolerance/dt
///      depending only on wall-clock dispatch timing).
///   4. [`default_bounds`] — the `DEFAULT_STOP_TIME` fallback.
///
/// The caller gathers tiers 1–3 from live state and passes them in; this
/// function owns only the precedence and the fallback.
pub fn resolve_bounds(
    draft_override: Option<RunBounds>,
    annotation_bounds: Option<RunBounds>,
    runner_cached: Option<RunBounds>,
) -> RunBounds {
    draft_override
        .or(annotation_bounds)
        .or(runner_cached)
        .unwrap_or_else(default_bounds)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rb(t_end: f64) -> RunBounds {
        RunBounds { t_start: 0.0, t_end, dt: None, n_intervals: None, tolerance: None, solver: None, h0: None, runtime: lunco_experiments::RuntimeMode::Batch }
    }

    #[test]
    fn default_class_prefers_drill_pin_then_first_candidate() {
        let cands = vec!["Pkg.Env".to_string(), "Pkg.System".to_string()];
        assert_eq!(default_class(Some("Pkg.System"), &cands).as_deref(), Some("Pkg.System"));
        assert_eq!(default_class(None, &cands).as_deref(), Some("Pkg.Env"));
        assert_eq!(default_class(None, &[]), None);
    }

    #[test]
    fn resolve_bounds_prefers_draft_then_annotation_then_cache() {
        // Args, highest precedence first: (draft, annotation, runner_cached).
        // Draft wins over everything.
        assert_eq!(resolve_bounds(Some(rb(1.0)), Some(rb(2.0)), Some(rb(3.0))).t_end, 1.0);
        // Race fix: the fresh AST annotation beats the async runner cache, so a
        // run's frozen bounds don't depend on whether the worker callback landed.
        assert_eq!(resolve_bounds(None, Some(rb(2.0)), Some(rb(3.0))).t_end, 2.0);
        // The cache is only a fallback when there is no annotation.
        assert_eq!(resolve_bounds(None, None, Some(rb(3.0))).t_end, 3.0);
        assert_eq!(resolve_bounds(None, None, None).t_end, DEFAULT_STOP_TIME);
        assert_eq!(DEFAULT_STOP_TIME, 1.0); // Modelica spec default StopTime
    }

    #[test]
    fn resolve_requested_class_handles_qualified_leaf_unknown_and_ambiguous() {
        let cands = vec![
            "LunarRover.RoverThermalSystem".to_string(),
            "LunarRover.LunarEnvironment".to_string(),
            "OtherPkg.RoverThermalSystem".to_string(), // same leaf, different pkg
            "TopLevel".to_string(),                    // dot-free
        ];
        // Exact qualified passes through.
        assert_eq!(
            resolve_requested_class("LunarRover.LunarEnvironment", &cands).unwrap(),
            "LunarRover.LunarEnvironment"
        );
        // Dot-free top-level matches exactly.
        assert_eq!(resolve_requested_class("TopLevel", &cands).unwrap(), "TopLevel");
        // Unique leaf resolves to its qualified form.
        assert_eq!(
            resolve_requested_class("LunarEnvironment", &cands).unwrap(),
            "LunarRover.LunarEnvironment"
        );
        // Whitespace is trimmed.
        assert_eq!(
            resolve_requested_class("  LunarEnvironment ", &cands).unwrap(),
            "LunarRover.LunarEnvironment"
        );
        // Unknown name → Unknown.
        assert_eq!(resolve_requested_class("Nope", &cands), Err(ClassResolveError::Unknown));
        // Leaf shared across packages → Ambiguous with both matches.
        match resolve_requested_class("RoverThermalSystem", &cands) {
            Err(ClassResolveError::Ambiguous(m)) => {
                assert_eq!(m.len(), 2);
                assert!(m.contains(&"LunarRover.RoverThermalSystem".to_string()));
                assert!(m.contains(&"OtherPkg.RoverThermalSystem".to_string()));
            }
            other => panic!("expected Ambiguous, got {other:?}"),
        }
    }

    #[test]
    fn interval_zero_is_unspecified_sentinel() {
        assert_eq!(interval_to_dt(Some(0.0)), None);
        assert_eq!(interval_to_dt(Some(3600.0)), Some(3600.0));
        assert_eq!(interval_to_dt(None), None);
    }

    #[test]
    fn bounds_from_experiment_needs_stop_time_and_drops_zero_interval() {
        let mut exp = crate::annotations::Experiment::default();
        assert!(bounds_from_experiment(&exp).is_none()); // no stop_time
        exp.stop_time = Some(5.0);
        exp.interval = Some(0.0); // sentinel → dt None
        let b = bounds_from_experiment(&exp).unwrap();
        assert_eq!(b.t_end, 5.0);
        assert_eq!(b.dt, None);
    }
}
