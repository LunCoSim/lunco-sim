// Shared runtime numerical policy.
//
// This module intentionally has no numerical defaults. A value that changes
// the meaning of a requirement belongs to the requirement (normally SysML);
// a value that controls a runtime algorithm belongs to the active Twin's
// generic settings table. The two must not be silently substituted for one
// another.
//
// Settings are deliberately separate f64 fields. Do not collapse length,
// angle, time, solver, and dimensionless comparison margins into one magic
// epsilon. Call `active()` once per report/solve, then pass the selected f64
// value into the hot relation loop.

fn _f64_setting(value) {
    f64_only(value)
}

fn _nonnegative(value) {
    let result = _f64_setting(value);
    if result == () || result < 0.0 { return (); }
    result
}

/// Read one explicitly floating-point Twin setting. Integer settings are not
/// silently promoted: callers that need an engineering tolerance must author
/// a decimal/number value, so the persisted type is visible and intentional.
fn f64_setting(key) {
    _f64_setting(get_twin_setting(key))
}

fn nonnegative_f64_setting(key) {
    _nonnegative(get_twin_setting(key))
}

/// Resolve the common app-wide numerical policy once. Missing fields remain
/// `()` and are an explicit configuration error for the consumer that needs
/// them. `require` is a small convenience for that validation path.
fn active() {
    #{
        scalar_abs: nonnegative_f64_setting("numerics.comparison.scalar_abs"),
        length_abs_m: nonnegative_f64_setting("numerics.comparison.length_abs_m"),
        angle_abs_rad: nonnegative_f64_setting("numerics.comparison.angle_abs_rad"),
        time_abs_s: nonnegative_f64_setting("numerics.comparison.time_abs_s"),
        solver_abs: nonnegative_f64_setting("numerics.solver.residual_abs"),
    }
}

fn require(profile, field) {
    if profile == () || field == () || profile[field] == () { return (); }
    _nonnegative(profile[field])
}
