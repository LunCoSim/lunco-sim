// ── Assertion harness for rhai tests ────────────────────────────────────────
//
// TEST-ONLY. This lives under `assets/scripting/tests/`, NOT
// `assets/scripting/prelude/` — `lunco_assets::scripting::prelude_files()` scans
// only the prelude directory, so nothing here is ever auto-loaded into a running
// scenario. Test code must not ride along in production scripts; a harness
// concatenates these libs ahead of a test file explicitly — either
// `cargo test -p lunco-scripting --test rhai_test_harness` for the pure suites,
// or `scripts/api/rhai_eval.py -e "$(cat lib/*.rhai)" -f <test>` for the ones
// needing a live stage (the shape `scripts/scenario_sync_test.sh` uses).
//
// ## Why every helper returns a string
//
// Rhai functions are PURE: a function body cannot see or mutate script-level
// variables, so the usual "assert throws / bumps a global counter" shape is not
// available. Instead each check RETURNS "" on pass or a failure message, and the
// test file collects them:
//
//     let fails = [];
//     fails.push(t_near(a, b, 1e-6, "wall thickness"));
//     t_report(fails);
//
// `t_report` filters the empty strings, so a passing check costs one "" in an
// array and nothing else.
//
// ## Verdict contract
//
// The LAST line of output is `TESTS_OK <n>` or `TESTS_FAIL <n>/<m>`, which the
// runner greps to set its exit code. Keep that contract — the harness's own
// output is the only channel a shell/CI has.

/// Absolute-tolerance float compare. `tol` is absolute, not relative: these
/// check authored dimensions in metres, where "within 1 mm" is the meaningful
/// question and a relative epsilon would be looser on big numbers than on small
/// ones for no reason.
///
/// A `()` (missing attribute) is a FAILURE, never a silent pass — reading an
/// attribute that does not exist is exactly the drift these tests hunt.
fn t_near(actual, expected, tol, what) {
    if type_of(actual) == "()" || type_of(expected) == "()" {
        return what + ": MISSING (got " + actual + ", expected " + expected + ")";
    }
    let d = actual - expected;
    if d < 0.0 { d = -d; }
    if d > tol {
        return what + ": " + actual + " != " + expected + " (delta " + d + ", tol " + tol + ")";
    }
    return "";
}

/// Exact compare, for values that are genuinely discrete — counts, orders,
/// vertex totals, type names, booleans. Use `t_near` for anything measured.
fn t_eq(actual, expected, what) {
    if type_of(actual) == "()" {
        return what + ": MISSING (expected " + expected + ")";
    }
    if actual != expected {
        return what + ": " + actual + " != " + expected;
    }
    return "";
}

/// Bare predicate, for conditions that do not reduce to a comparison.
fn t_true(cond, what) {
    if cond { return ""; }
    return what + ": expected true";
}

/// Assert a value is ABSENT. The mirror of the others, and load-bearing here:
/// several invariants are about an attribute that must NOT be authored (a
/// per-face transform, an override that should live on the parent).
fn t_absent(actual, what) {
    if type_of(actual) == "()" { return ""; }
    return what + ": expected absent, got " + actual;
}

/// Print the verdict and the failures. Call ONCE, last, in a test file.
fn t_report(fails) {
    let real = [];
    for f in fails {
        if f != "" { real.push(f); }
    }
    let total = fails.len();
    if real.is_empty() {
        print("TESTS_OK " + total);
    } else {
        for f in real { print("  FAIL " + f); }
        print("TESTS_FAIL " + real.len() + "/" + total);
    }
}
