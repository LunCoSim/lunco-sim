// ── Core smoke test for the QueryUsdPrim provider ───────────────────────────
//
// TEST-ONLY. Needs a LIVE sandbox with a scene loaded, so it runs the same way
// `scenario_sync_assert.rhai` does — libs and parameters injected as a prelude,
// verdict on the last line, driven from a shell harness:
//
//     python3 scripts/api/rhai_eval.py <port> \
//         -e "$(cat assets/scripting/tests/lib/*.rhai; echo 'let PROBE = "/Some/Prim";')" \
//         -f assets/scripting/tests/test_usd_query.rhai | tail -1
//
// `cargo test -p lunco-scripting --test rhai_test_harness` parse-checks this
// file, so a syntax error here still fails CI without a sandbox.
//
// This pins the MECHANISM every model suite stands on. If `QueryUsdPrim` stops
// resolving prims, a model suite fails with dozens of "MISSING attribute" lines
// that read as "the model is broken" — the asset is fine and the provider is
// down. Running this first turns that into one honest failure.
//
// ## The absent-prim contract is the load-bearing part
//
// `usd_attr` returns `()` for BOTH "no such prim" and "no such attribute", and
// `t_near`/`t_eq` treat `()` as a failure. That only works if the provider
// actually reports absence rather than, say, returning a default-constructed
// prim or an empty attrs map that indexes to something falsy. The negative
// checks below are not padding — they are what makes every green in every other
// suite meaningful.
//
// `PROBE` is injected by the runner and must be a prim that exists in the
// loaded scene. Everything else here is scene-independent.

let f = [];

// ── Absence is reported as absence ──────────────────────────────────────────
// A path no scene will ever contain.
const GHOST = "/__no_such_prim_for_tests__";

f.push(t_eq(type_of(usd_type(GHOST)), "()", "absent prim -> usd_type is ()"));
f.push(t_eq(type_of(usd_attr(GHOST, "radius")), "()", "absent prim -> usd_attr is ()"));
f.push(t_eq(type_of(usd_extent_y(GHOST)), "()", "absent prim -> usd_extent_y is ()"));
f.push(t_eq(type_of(usd_radial_min(GHOST)), "()", "absent prim -> usd_radial_min is ()"));

// And absence must PROPAGATE into a verdict, not get swallowed. If this passes
// while the checks above pass, a typo'd prim path anywhere is guaranteed to be
// caught rather than silently skipped.
f.push(t_fails_local(t_near(usd_attr(GHOST, "radius"), 1.0, 0.001, "x"),
                     "absent attr propagates to t_near failure"));
f.push(t_fails_local(t_prim_is(GHOST, "NurbsPatch", "x"),
                     "absent prim propagates to t_prim_is failure"));

/// Local copy of the inverter (see test_harness_selftest.rhai) — rhai has no
/// cross-file function sharing beyond what the runner concatenates, and this
/// file must stand alone against a scene the selftest never loads.
fn t_fails_local(result, what) {
    if result == "" { return what + ": helper returned pass, expected FAIL"; }
    return "";
}

// ── A real prim round-trips ─────────────────────────────────────────────────
// Positive control. Without this the file would pass against a provider that
// returns `()` for absolutely everything.
f.push(t_true(type_of(usd_type(PROBE)) != "()", "PROBE prim " + PROBE + " resolves"));

let p = usd_prim(PROBE);
f.push(t_true(type_of(p) != "()", "usd_prim(PROBE) returns a record"));
f.push(t_eq(p.path, PROBE, "returned path echoes the query"));
f.push(t_true(p.type_name != "", "PROBE has a non-empty type_name"));

// The frame contract: world_position is meaningless unless it says which frame
// it is in, and `MoveEntity` consumes that same frame. A provider that dropped
// the tag would leave callers guessing between grid-absolute and local.
f.push(t_eq(p.position_frame, "grid_absolute", "world_position carries its frame"));

// Named-attribute reads must agree with the read-everything form. These are two
// different code paths in the provider (selected vs. enumerated attributes), and
// only this compares them.
let names = p.attrs.keys();
if names.is_empty() {
    f.push("PROBE prim has no authored attributes — pick a geometry prim");
} else {
    let n = names[0];
    let one = usd_attrs(PROBE, [n]);
    f.push(t_eq(one.attrs[n], p.attrs[n], "attrs:[" + n + "] matches full read"));
    f.push(t_eq(one.attrs.keys().len(), 1, "attrs:[" + n + "] returns exactly one"));
}

t_report(f);
