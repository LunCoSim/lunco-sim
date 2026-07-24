//! The rhai TEST harness must parse, and must be able to fail.
//!
//! Companion to `prelude_parses.rs`, for `assets/scripting/tests/` instead of
//! `assets/scripting/prelude/`. Same blind spot, same argument: these are
//! assets, so `cargo check` never sees them and a syntax error surfaces only
//! when someone runs a suite against a live sandbox.
//!
//! ## Why this is worth a cargo test rather than only a live one
//!
//! `test_harness_selftest.rhai` is PURE — it exercises `t_near`/`t_eq`/
//! `t_true`/`t_absent` on arithmetic and strings, and calls no host function.
//! So it runs in a bare `rhai::Engine` with no App, no World, no USD stage and
//! no sandbox process, and `cargo test` can answer the question that otherwise
//! needs a full live rig: *can these assertions fail at all?*
//!
//! That question is the point. A helper that returns `""` unconditionally turns
//! every suite green and every green meaningless — the model drifts behind a
//! wall of passing tests. The moonbase invariants exist because a 3.6 m offset
//! sat beside 114 passing engine tests; a harness that cannot fail is that same
//! failure one level up, and it belongs in the default `cargo test` run rather
//! than behind a sandbox nobody starts.
//!
//! The USD-touching suites (`test_usd_query.rhai`, and the model invariants in
//! the moonbase repo) need a loaded stage, so they stay live-only — run via
//! `rhai_eval.py` from a shell harness, in the style of
//! `scripts/scenario_sync_test.sh`. This file parse-checks them so a typo there
//! still fails in CI without a sandbox.

use std::path::PathBuf;

use rhai::Engine;

/// An engine configured like the runtime's.
///
/// Mirrors `prelude_parses.rs::runtime_engine`: a bare engine can drift from the
/// runtime's bounded-resource policy and make this a false alarm.
fn runtime_engine() -> Engine {
    let mut engine = Engine::new();
    lunco_scripting::rhai_limits::apply(&mut engine);
    engine
}

fn tests_dir() -> PathBuf {
    // crates/lunco-scripting -> repo root -> assets/scripting/tests
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets/scripting/tests")
        .canonicalize()
        .expect("assets/scripting/tests must exist")
}

/// Every `.rhai` under `assets/scripting/tests/`, recursively, as (label, source).
fn test_sources() -> Vec<(String, String)> {
    fn walk(dir: &PathBuf, out: &mut Vec<(String, String)>) {
        let entries = std::fs::read_dir(dir).unwrap_or_else(|e| panic!("read {dir:?}: {e}"));
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "rhai") {
                let src =
                    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path:?}: {e}"));
                out.push((path.file_name().unwrap().to_string_lossy().into(), src));
            }
        }
    }
    let mut out = Vec::new();
    walk(&tests_dir(), &mut out);
    out.sort();
    out
}

/// The core helper libs, concatenated in the order a runner would use.
///
/// Sorted by filename so the order is deterministic: `test_assert.rhai` sorts
/// before `test_usd.rhai`, and the latter's predicates call the former's
/// `t_near`/`t_eq`/`t_absent`.
fn libs() -> String {
    let mut paths: Vec<_> = std::fs::read_dir(tests_dir().join("lib"))
        .expect("assets/scripting/tests/lib must exist")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "rhai"))
        .collect();
    paths.sort();
    assert!(!paths.is_empty(), "no test libs found");
    paths
        .iter()
        .map(|p| std::fs::read_to_string(p).unwrap())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Every test asset parses. Cheap, and the only check the USD-touching suites get
/// without a running sandbox.
#[test]
fn test_assets_all_parse() {
    let engine = runtime_engine();
    let sources = test_sources();
    assert!(!sources.is_empty(), "no test .rhai files found at all");

    for (name, src) in &sources {
        if let Err(e) = engine.compile(src.as_str()) {
            panic!("test asset '{name}' does not parse: {e}");
        }
    }
}

/// Run a pure rhai script, returning everything it `print`ed.
fn run_capturing_print(script: &str) -> Vec<String> {
    use std::sync::{Arc, Mutex};

    let out = Arc::new(Mutex::new(Vec::new()));
    let sink = out.clone();

    let mut engine = runtime_engine();
    engine.on_print(move |s| sink.lock().unwrap().push(s.to_string()));
    engine
        .run(script)
        .unwrap_or_else(|e| panic!("selftest failed to RUN (not a test failure, a crash): {e}"));

    let v = out.lock().unwrap().clone();
    v
}

/// The assertion harness passes its own self-test.
///
/// This is the one that certifies every other green in the system.
#[test]
fn assertion_harness_selftest_passes() {
    let script = format!(
        "{}\n{}",
        libs(),
        std::fs::read_to_string(tests_dir().join("test_harness_selftest.rhai"))
            .expect("test_harness_selftest.rhai must exist")
    );

    let printed = run_capturing_print(&script);
    let verdict = printed
        .last()
        .unwrap_or_else(|| panic!("selftest printed nothing — it never reached t_report"));

    assert!(
        verdict.starts_with("TESTS_OK"),
        "harness selftest did not pass.\n{}",
        printed.join("\n")
    );
}

/// `t_report` actually REPORTS — a failing check must produce `TESTS_FAIL`.
///
/// The selftest above proves the individual helpers can return a failure
/// message. This proves the reporter does not then swallow it. Both halves are
/// needed: a perfect set of helpers behind a `t_report` that always prints
/// `TESTS_OK` is indistinguishable, from the outside, from a passing suite —
/// and the verdict line is the ONLY channel the shell harnesses read.
#[test]
fn t_report_surfaces_a_failure() {
    let script = format!(
        r#"{}
        let f = [];
        f.push(t_near(1.0, 1.0, 0.001, "this one passes"));
        f.push(t_near(1.0, 2.0, 0.001, "this one must fail"));
        t_report(f);
        "#,
        libs()
    );

    let printed = run_capturing_print(&script);
    let verdict = printed.last().expect("t_report printed nothing");

    assert_eq!(
        verdict,
        "TESTS_FAIL 1/2",
        "t_report must report 1 failure of 2 checks, got:\n{}",
        printed.join("\n")
    );
    assert!(
        printed.iter().any(|l| l.contains("this one must fail")),
        "t_report must name the failing check, got:\n{}",
        printed.join("\n")
    );
}

#[test]
fn rhai_lint_rejects_production_tick_but_allows_test_tick() {
    let policy = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/scripting/policy/lint_rhai.rhai"),
    )
    .expect("rhai lint policy");
    let engine = runtime_engine();

    let production = r#"
        let facts = #{
            path: "assets/scenarios/bad.rhai",
            kind: "rhai",
            source: "fn on_tick(me) { set(me, \"x\", 1.0); }"
        };
        lint_rhai(facts)
    "#;
    let findings = engine
        .eval::<rhai::Array>(&format!("{policy}\n{production}"))
        .expect("production lint result");
    assert_eq!(findings.len(), 1);
    let finding = findings[0].clone().cast::<rhai::Map>();
    assert_eq!(
        finding["rule"].clone().into_string().unwrap(),
        "production-rhai-on-tick"
    );

    let test = r#"
        let facts = #{
            path: "assets/scenarios/tests/step.rhai",
            kind: "rhai",
            source: "fn on_tick(me) { this.t += 1; }"
        };
        lint_rhai(facts)
    "#;
    let findings = engine
        .eval::<rhai::Array>(&format!("{policy}\n{test}"))
        .expect("test lint result");
    assert!(findings.is_empty());
}
