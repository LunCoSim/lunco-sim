//! Verifies the rumoca APIs that the current Modelica runtime depends on.
//!
//! Pure compilation and AST/token-location contracts used by the runtime.

/// Bound external declarations remain writable runtime inputs.
#[test]
fn compile_str_keeps_bound_input_as_runtime_slot() {
    let src = "model M\n  input Real g = 9.81;\n  Real x;\nequation\n  der(x) = g;\nend M;\n";
    let mut compiler = lunco_modelica_compiler::ModelicaCompiler::new();
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
        "
    );
}

/// AST-driven span splicing produces the expected renamed source.
#[test]
fn ast_class_rename_via_token_spans() {
    use lunco_modelica_ast::parse_to_ast;
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
