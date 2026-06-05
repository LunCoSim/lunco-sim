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
///   * explicit positive `dt` (the `Interval` annotation) → honoured as given
///   * `dt` missing or `<= 0` (the spec's 0 sentinel)      → `span / NUM_INTERVALS`
///   * degenerate zero-length span                         → `0.01`
///
/// then clamped up so `span / step_dt <= SAMPLE_CAP`.
pub fn resolve_step_dt(t_start: f64, t_end: f64, dt: Option<f64>) -> f64 {
    let span = (t_end - t_start).max(0.0);
    let mut step_dt = match dt {
        Some(dt) if dt > 0.0 => dt,
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
        tolerance: None,
        solver: None,
        h0: None,
    }
}

/// The class a simulation surface defaults to, in precedence order:
///   1. `drilled_in` — the UI drill-in pin; the user is looking at a leaf
///      model and expects *that* to run, not the enclosing package.
///   2. the first `candidate` — caller supplies the tier-ranked
///      [`simulation_candidates`](crate::index::ModelIndex::simulation_candidates)
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

/// Map a model's `experiment(...)` annotation to [`RunBounds`]. `None` when
/// the annotation has no `StopTime` — a `StopTime` is what makes the
/// annotation usable as a run horizon.
pub fn bounds_from_experiment(exp: &crate::annotations::Experiment) -> Option<RunBounds> {
    let t_end = exp.stop_time?;
    Some(RunBounds {
        t_start: exp.start_time.unwrap_or(0.0),
        t_end,
        dt: interval_to_dt(exp.interval),
        tolerance: exp.tolerance,
        solver: None,
        h0: None,
    })
}

/// Resolve the run bounds from the four precedence tiers, highest first:
///   1. `draft_override` — a value the user edited in a Setup form.
///   2. `runner_cached` — the runner's `default_bounds` annotation cache.
///   3. `annotation_bounds` — bounds derived from the AST `experiment(...)`
///      (see [`bounds_from_experiment`]).
///   4. [`default_bounds`] — the `DEFAULT_STOP_TIME` fallback.
///
/// The caller gathers tiers 1–3 from live state and passes them in; this
/// function owns only the precedence and the fallback.
pub fn resolve_bounds(
    draft_override: Option<RunBounds>,
    runner_cached: Option<RunBounds>,
    annotation_bounds: Option<RunBounds>,
) -> RunBounds {
    draft_override
        .or(runner_cached)
        .or(annotation_bounds)
        .unwrap_or_else(default_bounds)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rb(t_end: f64) -> RunBounds {
        RunBounds { t_start: 0.0, t_end, dt: None, tolerance: None, solver: None, h0: None }
    }

    #[test]
    fn default_class_prefers_drill_pin_then_first_candidate() {
        let cands = vec!["Pkg.Env".to_string(), "Pkg.System".to_string()];
        assert_eq!(default_class(Some("Pkg.System"), &cands).as_deref(), Some("Pkg.System"));
        assert_eq!(default_class(None, &cands).as_deref(), Some("Pkg.Env"));
        assert_eq!(default_class(None, &[]), None);
    }

    #[test]
    fn resolve_bounds_follows_precedence_and_falls_back_to_ten() {
        assert_eq!(resolve_bounds(Some(rb(1.0)), Some(rb(2.0)), Some(rb(3.0))).t_end, 1.0);
        assert_eq!(resolve_bounds(None, Some(rb(2.0)), Some(rb(3.0))).t_end, 2.0);
        assert_eq!(resolve_bounds(None, None, Some(rb(3.0))).t_end, 3.0);
        assert_eq!(resolve_bounds(None, None, None).t_end, DEFAULT_STOP_TIME);
        assert_eq!(DEFAULT_STOP_TIME, 1.0); // Modelica spec default StopTime
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
