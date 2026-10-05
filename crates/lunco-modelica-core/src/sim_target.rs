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
//! (drill-in pin, draft override, current AST annotation) from live
//! ECS state and calling down into these functions. That keeps the
//! dependency arrow pointing the right way — UI depends on this, never the
//! reverse — and makes the resolution rules unit-testable without a `World`.

use lunco_experiments::RunBounds;

/// The fallback simulation horizon when nothing else supplies one (no draft,
/// no `experiment(...)` annotation). `1.0` is the Modelica
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
    interval.filter(|&i| i != 0.0)
}

/// Default `numberOfIntervals` when the `experiment` annotation supplies no
/// positive `Interval` — the Modelica spec's output-sampling default.
pub const NUM_INTERVALS: f64 = 500.0;

/// Maximum admitted output interval count. The inclusive start sample adds
/// one point. Shared by native and wasm admission before solver allocation.
pub const SAMPLE_CAP: f64 = 200_000.0;

/// Invalid caller-supplied bounds or output grid.
#[derive(Debug, Clone, PartialEq)]
pub struct RunBoundsError {
    pub field: &'static str,
    pub reason: String,
}

impl std::fmt::Display for RunBoundsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "invalid run {}: {}", self.field, self.reason)
    }
}

impl std::error::Error for RunBoundsError {}

fn invalid(field: &'static str, reason: impl Into<String>) -> RunBoundsError {
    RunBoundsError {
        field,
        reason: reason.into(),
    }
}

/// Resolve and validate the output sample spacing before any solver allocates
/// its grid. Explicit inputs are finite and positive; an explicit count wins
/// over an explicit interval, but neither may be invalid. Omitted spacing uses
/// the Modelica default of 500 intervals. Over-limit requests are rejected;
/// admission never changes the requested grid.
pub fn resolve_step_dt(
    t_start: f64,
    t_end: f64,
    dt: Option<f64>,
    n_intervals: Option<u32>,
) -> Result<f64, RunBoundsError> {
    if !t_start.is_finite() {
        return Err(invalid("t_start", "must be finite"));
    }
    if !t_end.is_finite() {
        return Err(invalid("t_end", "must be finite"));
    }
    let span = t_end - t_start;
    if !span.is_finite() || span <= 0.0 {
        return Err(invalid("horizon", "must be finite and strictly increasing"));
    }
    if dt.is_some_and(|dt| !dt.is_finite() || dt <= 0.0) {
        return Err(invalid("dt", "must be finite and positive"));
    }
    if let Some(n) = n_intervals {
        if n == 0 {
            return Err(invalid("n_intervals", "must be positive"));
        }
        if f64::from(n) > SAMPLE_CAP {
            return Err(invalid(
                "n_intervals",
                format!("{n} exceeds the {SAMPLE_CAP:.0}-interval limit"),
            ));
        }
    }
    let step_dt = match (n_intervals, dt) {
        (Some(n), _) => span / f64::from(n),
        (_, Some(dt)) => dt,
        _ => span / NUM_INTERVALS,
    };
    if !step_dt.is_finite() || step_dt <= 0.0 {
        return Err(invalid(
            "output interval",
            "cannot be represented as a positive finite value",
        ));
    }
    // Compare the interval against the canonical limit directly. Re-dividing
    // a derived span/N can round just above N and reject an exact-cap count.
    if n_intervals.is_none() && step_dt < span / SAMPLE_CAP {
        let intervals = (span / step_dt).ceil();
        return Err(invalid(
            "output grid",
            format!("requires {intervals} intervals, exceeding the {SAMPLE_CAP:.0}-interval limit"),
        ));
    }
    if step_dt < span && (t_start + step_dt <= t_start || t_end - step_dt >= t_end) {
        return Err(invalid(
            "output interval",
            "does not advance time across the requested horizon",
        ));
    }
    Ok(step_dt)
}

/// Shared admission guard for UI, API, native threads and wasm workers.
/// Returns the validated output spacing; omitted solver hints keep their
/// documented defaults, while invalid explicit hints fail at admission.
pub fn validate_run_bounds(bounds: &RunBounds) -> Result<f64, RunBoundsError> {
    let dt = resolve_step_dt(bounds.t_start, bounds.t_end, bounds.dt, bounds.n_intervals)?;
    for (field, value) in [("tolerance", bounds.tolerance), ("h0", bounds.h0)] {
        if value.is_some_and(|value| !value.is_finite() || value <= 0.0) {
            return Err(invalid(field, "must be finite and positive"));
        }
    }
    Ok(dt)
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
///      [`simulation_candidates`](lunco_modelica_index::index::ModelicaIndex::simulation_candidates)
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
/// [`simulation_candidates`](lunco_modelica_index::index::ModelicaIndex::simulation_candidates)).
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
    //    prefix the compiler needs. This is the drilled-source library-class case: the
    //    pin is the true FQN `Modelica.Blocks.Examples.PID_Controller`, while
    //    the in-doc candidate is the `within`-relative
    //    `Blocks.Examples.PID_Controller` (the doc's `within Modelica.Blocks.
    //    Examples;` prefix isn't folded into the index's qualified names).
    //    Compiling the under-qualified candidate would fail "model not found".
    if req.contains('.') {
        let mut suffix_hits = candidates.iter().filter(|c| {
            c.rsplit('.').next() == req.rsplit('.').next() && req.ends_with(&format!(".{c}"))
        });
        if suffix_hits.next().is_some() && suffix_hits.next().is_none() {
            return Ok(req.to_string());
        }
    }
    Err(ClassResolveError::Unknown)
}

/// Map and validate a model's `experiment(...)` annotation to [`RunBounds`]. `None` when
/// the annotation has no `StopTime` — a `StopTime` is what makes the
/// annotation usable as a run horizon.
pub fn bounds_from_experiment(
    exp: &lunco_modelica_ast::annotations::Experiment,
) -> Result<Option<RunBounds>, RunBoundsError> {
    let Some(t_end) = exp.stop_time else {
        return Ok(None);
    };
    let dt = interval_to_dt(exp.interval);
    let bounds = RunBounds {
        t_start: exp.start_time.unwrap_or(0.0),
        t_end,
        dt,
        // Modelica: `Interval` wins over `NumberOfIntervals` when both appear,
        // so only carry the count when no explicit interval was given.
        n_intervals: number_of_intervals_to_n(exp.number_of_intervals, dt)?,
        tolerance: exp.tolerance,
        solver: None,
        h0: None,
        runtime: lunco_experiments::RuntimeMode::Batch,
    };
    validate_run_bounds(&bounds)?;
    Ok(Some(bounds))
}

/// Map a Modelica `NumberOfIntervals` value to the typed
/// [`RunBounds::n_intervals`](lunco_experiments::RunBounds::n_intervals).
/// Explicit `Interval` takes precedence, so an unused count is not validated.
/// Otherwise a supplied count must be a finite positive integer representable
/// as `u32` and within the shared output limit; only absence means default.
pub fn number_of_intervals_to_n(
    number_of_intervals: Option<f64>,
    dt: Option<f64>,
) -> Result<Option<u32>, RunBoundsError> {
    if dt.is_some() {
        return Ok(None);
    }
    let Some(n) = number_of_intervals else {
        return Ok(None);
    };
    if !n.is_finite() || n < 1.0 || n.fract() != 0.0 || n > SAMPLE_CAP {
        return Err(invalid(
            "NumberOfIntervals",
            format!("must be a finite positive integer no greater than {SAMPLE_CAP:.0}"),
        ));
    }
    Ok(Some(n as u32))
}

/// Resolve document run bounds: explicit draft, current AST annotation,
/// then the documented one-second default when both are omitted.
pub fn resolve_bounds(
    draft_override: Option<RunBounds>,
    annotation_bounds: Option<RunBounds>,
) -> RunBounds {
    draft_override
        .or(annotation_bounds)
        .unwrap_or_else(default_bounds)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rb(t_end: f64) -> RunBounds {
        RunBounds {
            t_start: 0.0,
            t_end,
            dt: None,
            n_intervals: None,
            tolerance: None,
            solver: None,
            h0: None,
            runtime: lunco_experiments::RuntimeMode::Batch,
        }
    }

    #[test]
    fn default_class_prefers_drill_pin_then_first_candidate() {
        let cands = vec!["Pkg.Env".to_string(), "Pkg.System".to_string()];
        assert_eq!(
            default_class(Some("Pkg.System"), &cands).as_deref(),
            Some("Pkg.System")
        );
        assert_eq!(default_class(None, &cands).as_deref(), Some("Pkg.Env"));
        assert_eq!(default_class(None, &[]), None);
    }

    #[test]
    fn resolve_bounds_prefers_draft_then_current_annotation() {
        assert_eq!(resolve_bounds(Some(rb(1.0)), Some(rb(2.0))).t_end, 1.0);
        assert_eq!(resolve_bounds(None, Some(rb(2.0))).t_end, 2.0);
        assert_eq!(resolve_bounds(None, None).t_end, DEFAULT_STOP_TIME);
        assert_eq!(DEFAULT_STOP_TIME, 1.0);
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
        assert_eq!(
            resolve_requested_class("TopLevel", &cands).unwrap(),
            "TopLevel"
        );
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
        assert_eq!(
            resolve_requested_class("Nope", &cands),
            Err(ClassResolveError::Unknown)
        );
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
    fn run_bounds_reject_invalid_inputs_and_preserve_the_exact_output_limit() {
        let baseline = default_bounds();
        assert_eq!(validate_run_bounds(&baseline).unwrap(), 1.0 / NUM_INTERVALS);
        let exact = RunBounds {
            t_end: 0.1,
            n_intervals: Some(SAMPLE_CAP as u32),
            ..baseline.clone()
        };
        let dt = validate_run_bounds(&exact).unwrap();
        assert_eq!(dt, 0.1 / SAMPLE_CAP);
        assert_eq!(resolve_step_dt(0.0, 0.1, Some(dt), None).unwrap(), dt);
        // A step can advance below an exponent boundary but stall above it.
        let start = 1.0 - f64::EPSILON;
        let end = 1.0 + f64::EPSILON;
        let step = f64::EPSILON * 0.375;
        assert!(start + step > start);
        assert_eq!(end - step, end);
        assert!(resolve_step_dt(start, end, Some(step), None).is_err());
        // A coarse output interval can be valid while the browser's derived
        // internal cadence cannot advance an extreme absolute time.
        let start = 1.0e20;
        let end = start + 1.0e9;
        let output = resolve_step_dt(start, end, Some(end - start), None).unwrap();
        let internal = output.min(25.0).max((end - start) / SAMPLE_CAP);
        assert_eq!(start + internal, start);
        assert!(resolve_step_dt(start, end, Some(internal), None).is_err());
        for invalid in [
            RunBounds {
                t_start: f64::NAN,
                ..baseline.clone()
            },
            RunBounds {
                t_end: f64::INFINITY,
                ..baseline.clone()
            },
            RunBounds {
                t_start: -f64::MAX,
                t_end: f64::MAX,
                ..baseline.clone()
            },
            RunBounds {
                t_end: 0.0,
                ..baseline.clone()
            },
            RunBounds {
                t_end: -1.0,
                ..baseline.clone()
            },
            RunBounds {
                dt: Some(0.0),
                ..baseline.clone()
            },
            RunBounds {
                dt: Some(-1.0),
                ..baseline.clone()
            },
            RunBounds {
                dt: Some(f64::NAN),
                ..baseline.clone()
            },
            RunBounds {
                dt: Some(1.0e-300),
                ..baseline.clone()
            },
            RunBounds {
                n_intervals: Some(0),
                ..baseline.clone()
            },
            RunBounds {
                n_intervals: Some(u32::MAX),
                ..baseline.clone()
            },
            RunBounds {
                tolerance: Some(0.0),
                ..baseline.clone()
            },
            RunBounds {
                tolerance: Some(f64::INFINITY),
                ..baseline.clone()
            },
            RunBounds {
                h0: Some(-1.0),
                ..baseline.clone()
            },
            RunBounds {
                h0: Some(f64::NAN),
                ..baseline.clone()
            },
            RunBounds {
                t_start: 1.0e20,
                t_end: 1.0e20 + 32768.0,
                dt: Some(1.0),
                ..baseline.clone()
            },
        ] {
            let error =
                validate_run_bounds(&invalid).expect_err("explicit invalid bounds must reject");
            assert!(error.to_string().starts_with("invalid run "));
        }
        for n in [0.0, -1.0, 1.5, f64::NAN, f64::INFINITY, f64::from(u32::MAX)] {
            assert!(number_of_intervals_to_n(Some(n), None).is_err());
        }
        // An explicit interval makes the annotation count unused.
        assert_eq!(
            number_of_intervals_to_n(Some(f64::NAN), Some(0.1)).unwrap(),
            None
        );
    }

    #[test]
    fn interval_zero_is_unspecified_sentinel() {
        assert_eq!(interval_to_dt(Some(0.0)), None);
        assert_eq!(interval_to_dt(Some(3600.0)), Some(3600.0));
        assert_eq!(interval_to_dt(Some(-1.0)), Some(-1.0));
        assert_eq!(interval_to_dt(None), None);
    }

    #[test]
    fn bounds_from_experiment_needs_stop_time_and_drops_zero_interval() {
        let mut exp = lunco_modelica_ast::annotations::Experiment::default();
        assert!(bounds_from_experiment(&exp).unwrap().is_none()); // no stop_time
        exp.stop_time = Some(5.0);
        exp.interval = Some(0.0); // sentinel → dt None
        let b = bounds_from_experiment(&exp).unwrap().unwrap();
        assert_eq!(b.t_end, 5.0);
        assert_eq!(b.dt, None);
    }
}
