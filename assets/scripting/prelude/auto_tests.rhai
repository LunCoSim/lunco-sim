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

/// Round to 6 dp, for values where micrometres matter (settle traces).
fn r6(x) { if x == () { return (); } (x * 1000000.0).round() / 1000000.0 }

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

/// Declare that `port` is EXPECTED to dangle in this scene.
///
/// A fixture scene may be deliberately malformed — `lint_selftest` authors wires
/// into ports no program declares, because that malformation is what the lint
/// policy is tested against. `scene_test`'s never-landed gate cannot otherwise
/// tell a fixture from a broken vehicle.
///
/// This ASSERTS, it does not mute: a declared port that never dangles fails the
/// scene, so a fixture that quietly stops reproducing its defect is caught.
fn expect_fault(port) {
    emit("EXPECT_FAULT", port);
}

/// Declare the terminal runtime fault a negative production fixture must
/// reproduce. Unlike a dangling-wire expectation this is checked against the
/// structured `RuntimeFaults` boundary by `luncosim test`, so a fixture cannot
/// pass from a log line or from merely keeping the process alive.
fn expect_runtime_fault(kind) {
    emit("EXPECT_RUNTIME_FAULT", kind);
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

/// Emit a machine-readable requirement verdict before printing the stable
/// PASS/FAIL line.  The generic SysML evaluator returns structured result
/// maps; this adapter preserves them on the telemetry bus while keeping the
/// historical string verdict for the test runner and humans.  Any Twin can
/// use it for component-level evidence without adding a custom Rust runner.
fn report_structured_verdict(report, title, channel) {
    let failures = [];
    let raw_failures = [];
    let ok = report != () && report.ok == true;
    let source_revision = "";
    let result_count = 0;
    let check_count = 0;
    let results = [];
    let verification = "";
    let requirement_count = 0;
    let requirement_names = [];
    let requirement_summary = #{};
    let source_files = [];
    let source_revision_hex = "";
    let metrics = #{};
    if report != () {
        if report.source_revision != () { source_revision = report.source_revision; }
        if report.source_revision_hex != () {
            source_revision_hex = report.source_revision_hex;
        } else {
            source_revision_hex = source_revision;
        }
        if report.source_files != () { source_files = report.source_files; }
        if report.metrics != () { metrics = report.metrics; }
        if report.results != () {
            result_count = report.results.len();
            // Preserve the complete typed observation table.  The legacy
            // PASS/FAIL line remains the runner envelope, while this payload
            // is the durable per-check evidence consumed by UI/CI clients.
            results = report.results;
        }
        if report.check_count != () { check_count = report.check_count; }
        // Keep compatibility with older Twin observers that only supplied
        // `results`; the generic evaluator now supplies the precise count.
        if check_count == 0 && result_count > 0 { check_count = result_count; }
        if report.verification != () { verification = report.verification; }
        if report.requirement_count != () { requirement_count = report.requirement_count; }
        if report.requirement_names != () { requirement_names = report.requirement_names; }
        if report.requirement_summary != () { requirement_summary = report.requirement_summary; }
        if report.failures != () {
            for failure in report.failures {
                if type_of(failure) == "map" {
                    let line = "";
                    if failure.id != () { line += failure.id + " "; }
                    if failure.component != () { line += failure.component + " "; }
                    if failure.path != () { line += failure.path + ": "; }
                    if failure.error != () { line += failure.error; }
                    failures.push(line);
                    raw_failures.push(failure);
                } else {
                    if failure != "" {
                        failures.push(failure);
                        raw_failures.push(failure);
                    }
                }
            }
        }
    }
    if failures.is_empty() && !ok { failures.push("requirement evaluation unavailable"); }
    // Rhai values have a bounded 64 KiB string/value budget.  A large
    // component can legitimately produce hundreds of typed observations, so
    // never attempt to emit the complete result table as one event.  Small
    // reports retain the historical single-event shape; larger reports carry
    // the same typed maps as individually bounded events and expose the count
    // in the summary. Consumers can reconstruct the exact table by grouping
    // on (channel, source_revision) and ordering by result_index.
    let inline_results = result_count <= 64;
    let evidence = #{schema_version: 1, ok: ok, title: title, channel: channel,
        verdict: if ok { "pass" } else { "fail" },
        verification: verification, requirement_count: requirement_count,
        check_count: check_count, requirement_names: requirement_names,
        requirement_summary: requirement_summary,
        source_revision: source_revision, source_revision_hex: source_revision_hex,
        source_files: source_files, result_count: result_count,
        failure_count: raw_failures.len(), evidence_chunked: !inline_results,
        evidence_result_events: if inline_results { 0 } else { result_count },
        results: if inline_results { results } else { [] },
        failures: if inline_results { raw_failures } else { [] }, metrics: metrics};
    emit(channel + "_EVIDENCE", evidence);
    if !inline_results {
        for result_index in 0..result_count {
            emit(channel + "_EVIDENCE_RESULT", #{schema_version: 1,
                title: title, channel: channel, verification: verification,
                source_revision: source_revision,
                source_revision_hex: source_revision_hex,
                result_count: result_count, failure_count: raw_failures.len(),
                result_index: result_index, result: results[result_index]});
        }
    }
    print(channel + "_EVIDENCE source_revision=" + source_revision
        + " requirements=" + requirement_count + " checks=" + check_count
        + " failures=" + raw_failures.len());
    report_verdict(failures, title, channel)
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
