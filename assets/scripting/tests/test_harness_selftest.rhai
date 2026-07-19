// ── Self-test for the rhai assertion harness ────────────────────────────────
//
// TEST-ONLY. Runs under the standard cargo workflow, no sandbox required:
//
//     cargo test -p lunco-scripting --test rhai_test_harness
//
// Needs NO scene and NO host functions — every check here is pure arithmetic and
// string work, so it executes in a bare `rhai::Engine`. That is why it is a
// cargo test rather than a live one: if this is red, every other suite's greens
// are worthless, and that must be caught in the default test run rather than
// behind a sandbox nobody starts.
//
// ## Why a test suite needs its own tests
//
// The failure mode that motivates this file is an assertion helper that returns
// "" unconditionally. Nothing else in the system would notice: every suite goes
// green, the greens mean nothing, and the model drifts underneath a wall of
// passing tests. The moonbase invariants were written specifically to catch a
// 3.6 m offset that 114 passing engine tests sat beside — a harness that cannot
// fail is that same failure one level up.
//
// So each helper is exercised on BOTH sides: a case it must pass, and a case it
// must fail. `t_fails` inverts a helper's result, so "this check correctly
// reported a failure" is itself a passing assertion.

let f = [];

/// Invert: the helper under test SHOULD have returned a failure message.
/// Takes the helper's return value, not a closure — rhai functions are pure and
/// cannot capture, so the call happens at the call site.
fn t_fails(result, what) {
    if result == "" { return what + ": helper returned pass, expected FAIL"; }
    return "";
}

// ── t_near ──────────────────────────────────────────────────────────────────
f.push(t_near(1.0, 1.0, 0.000001, "t_near: exact equality passes"));
f.push(t_near(7.3450, 7.3451, 0.001, "t_near: inside tolerance passes"));
// Tolerance is ABSOLUTE, and the boundary matters: 1.5700 vs 1.5696 is 0.4 mm,
// the real discrepancy this suite found in the wall thickness. A helper using a
// relative epsilon would swallow it on a number this size.
f.push(t_fails(t_near(1.5700, 1.5696, 0.000001, "x"), "t_near: 0.4mm outside 1e-6 tol fails"));
f.push(t_fails(t_near(7.345, 5.775, 0.001, "x"), "t_near: far apart fails"));
// Sign-independence: the delta is an absolute value, so argument order must not
// change the verdict.
f.push(t_fails(t_near(5.775, 7.345, 0.001, "x"), "t_near: fails symmetrically"));

// A MISSING attribute must FAIL, never coerce to 0.0 and quietly pass. This is
// the single most important behaviour in the harness: `usd_attr` returns `()`
// for both "no such prim" and "no such attribute", and a typo in either would
// otherwise read as green.
f.push(t_fails(t_near((), 7.345, 0.001, "x"), "t_near: () actual fails"));
f.push(t_fails(t_near(7.345, (), 0.001, "x"), "t_near: () expected fails"));

// ── t_eq ────────────────────────────────────────────────────────────────────
f.push(t_eq(6, 6, "t_eq: equal ints pass"));
f.push(t_eq("leftHanded", "leftHanded", "t_eq: equal strings pass"));
f.push(t_eq(true, true, "t_eq: equal bools pass"));
f.push(t_fails(t_eq(5, 6, "x"), "t_eq: unequal ints fail"));
f.push(t_fails(t_eq("rightHanded", "leftHanded", "x"), "t_eq: unequal strings fail"));
f.push(t_fails(t_eq(false, true, "x"), "t_eq: unequal bools fail"));
f.push(t_fails(t_eq((), "leftHanded", "x"), "t_eq: () actual fails"));

// ── t_true / t_absent ───────────────────────────────────────────────────────
f.push(t_true(true, "t_true: true passes"));
f.push(t_fails(t_true(false, "x"), "t_true: false fails"));
f.push(t_absent((), "t_absent: () passes"));
// The inverse of t_near's contract: here a PRESENT value is the defect. This is
// what pins "no local translate on a NurbsPatch", so a translate of exactly 0.0
// must still fail — the attribute being authored at all is the finding.
f.push(t_fails(t_absent(0.0, "x"), "t_absent: authored 0.0 still fails"));
f.push(t_fails(t_absent([0.0, 3.6, 0.0], "x"), "t_absent: authored vector fails"));

t_report(f);
