// ── Automated tests — how a scene test asserts and reports PASS / FAIL ─────
//
// A scenario is a tick hook on a live sim, not a process, so it has no exit
// code. `scene_test` takes its verdict from TELEMETRY instead: the last thing a
// test scenario does is `emit(channel, "PASS"|"FAIL")`, and the runner's exit
// code follows it. These helpers are that contract, in one place.
//
// WHY THIS IS PRELUDE. Every test scenario needs the same six checks and the
// same report format, and each copy of them drifts. Use these; do not paste a
// private `t_range` into a new test.
//
// WHY A FAILING CHECK RETURNS A STRING. A check yields "" on pass and a MESSAGE
// on failure, and the caller collects them into an array. That makes every
// check run — the report names all of them, not just the first — and makes the
// failure self-describing without a parallel list of labels.
//
//     fn verdict(s) {
//         let f = [];
//         f.push(t_range(s.tilt, 0.0, 5.0, "hull tilt at rest (deg)"));
//         f.push(t_bounded(s.hull_pos, 100.0, "hull"));
//         report_verdict(f, "LANDING LEGS", "LANDING_LEGS");
//     }
//
// A SILENT PASS IS NOT A PASS. A scenario fails silently in every direction
// that matters: a hook that never fires, a `find` that returned -1, a phase
// that never advanced all look exactly like a clean run. So a test must also
// prove it MEASURED something — see `t_present` and `t_moved` — and print a
// per-sample table. A run with no sample rows proves nothing.

/// Round to 2 dp, for log tables.
fn r2(x) { if x == () { return (); } (x * 100.0).round() / 100.0 }

/// Round to 4 dp, for values where millimetres matter (joint travel).
fn r4(x) { if x == () { return (); } (x * 10000.0).round() / 10000.0 }

/// `v` must sit within `[lo, hi]`.
///
/// Phrased POSITIVELY (`v >= lo && v <= hi`), never as `!(v < lo || v > hi)`.
/// The native math verbs return `()` rather than NaN, but a script that does
/// its own arithmetic can still produce one, and NaN compares false against
/// everything — the negated form admits exactly the value a test exists to
/// catch.
fn t_range(v, lo, hi, what) {
    if v == () { return what + ": MISSING"; }
    if v >= lo && v <= hi { return ""; }
    return what + ": " + r2(v) + " outside [" + lo + ", " + hi + "]";
}

/// `|v|` must not exceed `limit` — a drift/error bound.
fn t_max(v, limit, what) {
    if v == () { return what + ": MISSING"; }
    if v.abs() <= limit { return ""; }
    return what + ": " + r2(v) + " exceeds " + limit;
}

/// A plain predicate. `what` is the FAILURE message, so write it as the thing
/// that went wrong, not as the thing being checked.
fn t_true(cond, what) { if cond { "" } else { what } }

/// A value must have been measured at all.
///
/// The dead-test guard: `()` means a read returned nothing, and every
/// comparison against it would otherwise be skipped or coerced into a pass.
fn t_present(v, what) {
    if v == () { return what + ": MISSING — nothing measured, so this test proved nothing"; }
    ""
}

/// Two values must agree within `tol_pct` of the larger magnitude.
///
/// RELATIVE, because the quantity under test is a ratio between two
/// realizations of one parameter set. Two values that are both ~zero are a
/// FAILURE, not a match — that is the dead-command-surface case, where nothing
/// moved and everything "agreed".
fn t_rel(a, b, tol_pct, what) {
    if a == () || b == () { return what + ": MISSING (a=" + a + " b=" + b + ")"; }
    let m = if a.abs() > b.abs() { a.abs() } else { b.abs() };
    if m < 0.001 {
        return what + ": BOTH ~ZERO (a=" + r2(a) + " b=" + r2(b) +
               ") - nothing moved, so this is a dead test, not a match";
    }
    let d = (a - b).abs() / m * 100.0;
    if d <= tol_pct { return ""; }
    return what + ": a=" + r2(a) + " b=" + r2(b) +
           " ratio=" + r2(a / b) + "x diff=" + r2(d) + "% (tol " + tol_pct + "%)";
}

/// A body must still be at a finite position inside the scene.
///
/// The "is it still a vehicle" check. A physics divergence throws parts to
/// hundreds of metres within the first second, so any generous finite bound
/// catches it and cannot false-positive on a bouncy landing.
fn t_bounded(pos, radius_m, what) {
    if pos == () { return what + ": MISSING (no world_pos)"; }
    let l = vlen(pos);
    if l == () { return what + ": position is not a vector"; }
    if l >= 0.0 && l <= radius_m { return ""; }
    return what + ": |pos|=" + r2(l) + " m — left the scene (or is not finite)";
}

/// Something must actually have moved by at least `min_m`.
///
/// Pairs with `t_bounded`: bounded-and-stationary is what a scene looks like
/// when the command surface is dead.
fn t_moved(distance_m, min_m, what) {
    if distance_m == () { return what + ": MISSING"; }
    if distance_m >= min_m { return ""; }
    return what + ": moved " + r2(distance_m) + " m, expected at least " + min_m +
           " m — nothing drove, so a match here would be meaningless";
}

/// Print the failures, print the greppable verdict line, emit it on `channel`,
/// and raise a toast. Call ONCE, last. Returns true on pass.
///
/// `title` is the human label ("LANDING LEGS"); `channel` is the telemetry
/// channel `scene_test --verdict-channel` reads ("LANDING_LEGS"). The LAST line
/// printed is `<title>: PASS|FAIL`, which is what a log grep looks for.
fn report_verdict(fails, title, channel) {
    let real = [];
    for f in fails { if f != "" { real.push(f); } }
    if real.is_empty() {
        print("TESTS_OK " + fails.len());
        print(title + ": PASS");
        emit(channel, "PASS");
        notify_kind(title + ": PASS", "success");
        return true;
    }
    for f in real { print("  FAIL: " + f); }
    print("TESTS_FAIL " + real.len() + "/" + fails.len());
    print(title + ": FAIL");
    emit(channel, "FAIL");
    notify_kind(title + ": FAIL", "error");
    false
}

/// Fail immediately, before the run wastes its ticks.
///
/// For the setup failures that otherwise look like a clean run forever: a
/// `find` that returned -1, a wrong scene, a missing port. Call from the first
/// tick and stop the scenario.
fn fail_fast(reason, title, channel) {
    print("  FAIL: " + reason);
    print("TESTS_FAIL 1/1");
    print(title + ": FAIL");
    emit(channel, "FAIL");
    notify_kind(title + ": FAIL — " + reason, "error");
    false
}

/// Straight-line distance between two samples, 0 when either is missing.
/// Accumulate these for a path length; a start-to-end measure is not one.
fn seg(prev, cur) {
    if prev == () || cur == () { return 0.0; }
    let d = vlen(vsub(cur, prev));
    if d == () { return 0.0; }
    d
}

/// Resolve a prim path, or `()` — `find` returns -1 on a miss, which is a
/// perfectly good array index and a perfectly bad entity id.
fn find_or_none(path) {
    let id = find(path);
    if id == () || id < 0 { return (); }
    id
}
