//! Verifies the rumoca APIs that the current Modelica runtime depends on.
//!
//! These are narrow contract pins for input lowering, simulation horizons, and
//! AST/token locations. They deliberately do not duplicate the retired regex
//! component scanner or compare a replacement against historical output.

/// **Chokepoint pin: `compile_str` strips bound-`input` defaults itself.**
///
/// rumoca demotes a bound `input Real g = 9.81` to an algebraic, so it never
/// reaches `input_names()` (`docs/architecture/29-rumoca-workarounds.md` §2).
/// The strip lives INSIDE `ModelicaCompiler::compile_str`, so no compile path
/// can bypass it by forgetting to call `strip_input_defaults` first — which is
/// exactly what `modelica_tester` used to do.
///
/// This test deliberately passes RAW, unstripped source. If someone moves the
/// strip back out to the callers, `g` disappears from `input_names()` and this
/// fails.
#[test]
fn compile_str_keeps_bound_input_as_runtime_slot() {
    let src = "model M\n  input Real g = 9.81;\n  Real x;\nequation\n  der(x) = g;\nend M;\n";
    let mut compiler = lunco_modelica::ModelicaCompiler::new();
    let dae = compiler.compile_str("M", src, "m.mo").expect("M compiles");

    let opts = rumoca_sim::SimOptions {
        t_start: 0.0,
        t_end: 10.0,
        ..Default::default()
    };
    let session = rumoca_sim::SimulationSession::new(&dae.dae, opts).expect("session builds");
    let inputs = session.input_names().to_vec();

    assert!(
        inputs.iter().any(|n| n == "g"),
        "a bound `input` must survive compile_str as a runtime slot; got {inputs:?} \
         — the strip was bypassed"
    );
}

/// **Contract pin (rumoca ≥0.9.20): `SimulationSession` clamps at `t_end`.**
///
/// `step`/`advance_to` refuse to advance the model past `SimOptions::t_end`, and
/// they do it *silently* — the call returns `Ok`, the clock just stops. Every
/// interactive caller therefore has to declare its real horizon up front
/// (`experiments_runner::stepper_options_from_bounds` is the one place that
/// does), because with the `SimOptions::default()` horizon of 1.0 a long run
/// parks at t=1s and reports a frozen model rather than an error.
///
/// If this test starts failing, the clamp is gone: the horizon plumbing in
/// `stepper_options_from_bounds` can be revisited, and the live path's
/// `t_end = u32::MAX` sentinel in `worker::live_stepper_options` with it.
#[test]
fn simulation_session_clamps_advance_at_t_end() {
    let source = lunco_modelica::models::get_model("Balloon.mo").expect("bundled Balloon.mo");
    let (stripped, _) = lunco_modelica::ast_extract::strip_input_defaults(source);
    let mut compiler = lunco_modelica::ModelicaCompiler::new();
    let dae = compiler
        .compile_str("Balloon", &stripped, "balloon.mo")
        .expect("Balloon compiles");

    let opts = rumoca_sim::SimOptions {
        atol: 1e-3,
        rtol: 1e-3,
        t_start: 0.0,
        t_end: 0.5,
        ..Default::default()
    };
    let mut session = rumoca_sim::SimulationSession::new(&dae.dae, opts).expect("session builds");

    // Ask for 2 s of model time against a 0.5 s horizon.
    for _ in 0..20 {
        session.step(0.1).expect("step stays Ok even once clamped");
    }

    assert!(
        (session.time() - 0.5).abs() < 1e-9,
        "session should clamp at t_end=0.5, got t={}",
        session.time()
    );
}

/// AST-driven span splicing produces the expected renamed source.
#[test]
fn ast_class_rename_via_token_spans() {
    use rumoca_phase_parse::parse_to_ast;
    let src = "within Foo.Bar;\n\nmodel OldName \"a class\"\n  Real x;\nend OldName;\n";
    let ast = parse_to_ast(src, "t.mo").expect("parses");
    let class = ast
        .classes
        .values()
        .find(|c| c.name.text.as_ref() == "OldName")
        .expect("class found");
    let header = &class.name.location;
    let end = class
        .end_name_token
        .as_ref()
        .expect("end token present")
        .location
        .clone();

    // Splice: end first, then header (preserves earlier offsets).
    let mut out = String::new();
    out.push_str(&src[..header.start as usize]);
    out.push_str("NewName");
    out.push_str(&src[header.end as usize..end.start as usize]);
    out.push_str("NewName");
    out.push_str(&src[end.end as usize..]);

    assert!(out.contains("model NewName"), "header rename: {out}");
    assert!(out.contains("end NewName;"), "end-token rename: {out}");
    assert!(!out.contains("OldName"), "no occurrence left: {out}");
    assert!(out.contains("\"a class\""), "description preserved: {out}");
}

/// `Session::navigation_rename_locations_query` returns both class-name
/// occurrences needed by the source-preserving rename path.
#[test]
fn rumoca_rename_covers_header_and_end_token() {
    use rumoca_compile::Session;

    let mut session = Session::default();
    let source = "model Foo\n  Real x;\nend Foo;\n";
    session
        .add_document("test.mo", source)
        .expect("source parses");

    // Position cursor on the `Foo` in `model Foo` — line 0, column 6
    // (rumoca uses 0-based line indexing per its existing tests).
    let locations = session
        .navigation_rename_locations_query("test.mo", 0, 6)
        .expect("rename locations resolve for class name");

    // Each tuple is `(file_uri, span)`. We expect at least the header
    // (`model Foo`) and the end token (`end Foo;`) — two distinct
    // line numbers, both inside `test.mo`.
    assert!(
        locations.len() >= 2,
        "expected at least 2 rename locations (header + end token), got {}: {:?}",
        locations.len(),
        locations
    );
    for (uri, _loc) in &locations {
        assert_eq!(uri, "test.mo", "every location should be in test.mo");
    }
    // Distinct line numbers prove header and footer are both covered.
    let mut lines: Vec<_> = locations.iter().map(|(_, l)| l.start_line).collect();
    lines.sort();
    lines.dedup();
    assert!(
        lines.len() >= 2,
        "expected locations on at least 2 distinct lines, got {:?}",
        lines
    );
}
