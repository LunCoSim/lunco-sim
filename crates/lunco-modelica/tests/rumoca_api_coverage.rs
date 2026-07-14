//! Verifies rumoca's public APIs cover the regex sites we plan to delete.
//!
//! Two specific replacements (per `docs/architecture/REFACTOR_PLAN.md`
//! Commit 1):
//!
//! 1. `ClassDef::iter_components()` replaces the regex in
//!    `ui/panels/canvas_projection.rs` (`scan_component_declarations`).
//! 2. `Session::navigation_rename_locations_query()` replaces the
//!    header/footer/end-name-token regex chain in `ui/commands.rs`.
//!
//! These tests pin the API contracts. If rumoca changes one and these
//! still pass, we're fine. If a test breaks, the corresponding refactor
//! commit is blocked until rumoca is fixed or the replacement strategy
//! is adjusted.

use std::collections::HashSet;

const RC_SOURCE: &str = "model RC_Circuit\n  Real R = 100;\n  Real C = 0.001;\n  Modelica.Electrical.Analog.Basic.Resistor resistor;\n  Modelica.Electrical.Analog.Basic.Capacitor capacitor;\n  Modelica.Electrical.Analog.Basic.Ground ground;\nend RC_Circuit;\n";

/// Same regex used in
/// `lunco_modelica::ui::panels::canvas_projection::scan_component_declarations`.
/// Duplicated here to lock the comparison: when we delete that function,
/// this test continues to lock rumoca's behavior against the historical
/// regex output.
fn legacy_regex_scan(source: &str) -> HashSet<(String, String)> {
    let re = regex::Regex::new(
        r"(?m)^\s*(?:(?:redeclare|flow|stream|input|output|parameter|constant|discrete|inner|outer|replaceable|final)\s+)*((?:[A-Za-z_]\w*\.)*[A-Za-z_]\w*)\s+([A-Za-z_]\w*)\b"
    ).expect("regex compiles");
    const KEYWORDS: &[&str] = &[
        "model", "block", "connector", "package", "function", "record", "class", "type",
        "extends", "import", "equation", "algorithm", "initial", "protected", "public",
        "annotation", "connect", "if", "for", "when", "end", "within", "and", "or", "not",
        "true", "false", "else", "elseif", "elsewhen", "while", "loop", "break", "return",
        "then", "external", "encapsulated", "partial", "expandable", "operator", "pure",
        "impure", "redeclare",
    ];
    let mut out = HashSet::new();
    for cap in re.captures_iter(source) {
        let ty = cap[1].to_string();
        let inst = cap[2].to_string();
        let first_segment = ty.split('.').next().unwrap_or(&ty).to_string();
        if KEYWORDS.contains(&first_segment.as_str()) {
            continue;
        }
        out.insert((ty, inst));
    }
    out
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
    let mut session =
        rumoca_sim::SimulationSession::new(&dae.dae, opts).expect("session builds");

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

/// Build the same `(type_name, instance)` set from rumoca's typed AST.
fn rumoca_ast_scan(source: &str, file_name: &str) -> HashSet<(String, String)> {
    let ast = rumoca_phase_parse::parse_to_ast(source, file_name).expect("parses");
    let mut out = HashSet::new();
    for (_class_name, class_def) in &ast.classes {
        for (name, comp) in class_def.iter_components() {
            out.insert((format!("{}", comp.type_name), name.to_string()));
        }
    }
    out
}

/// **Commit 2 gate**: rumoca AST iteration agrees with the regex scan
/// for a representative `.mo`. If this passes we can delete
/// `scan_component_declarations` and pull from `ClassDef::iter_components`
/// instead.
#[test]
fn rumoca_components_match_regex_scan() {
    let ast_pairs = rumoca_ast_scan(RC_SOURCE, "RC_Circuit.mo");
    let regex_pairs = legacy_regex_scan(RC_SOURCE);

    // Regex picks up the parameter declarations (`Real R = 100;`) too,
    // but so does the AST — both produce the same set.
    let only_in_ast: Vec<_> = ast_pairs.difference(&regex_pairs).collect();
    let only_in_regex: Vec<_> = regex_pairs.difference(&ast_pairs).collect();

    assert!(
        only_in_ast.is_empty() && only_in_regex.is_empty(),
        "AST and regex disagree.\n  only in AST: {:?}\n  only in regex: {:?}",
        only_in_ast,
        only_in_regex
    );
    assert!(!ast_pairs.is_empty(), "expected non-empty component set");
}

// NOTE: the `rumoca_full_span_includes_leading_comments` lock test was removed.
// It asserted `ClassDef::full_span_with_leading_comments`, an API rumoca dropped
// in main. Production (document/core.rs, duplicate.rs) now falls back to
// `class_def.location` (class span without leading comments), so the locked
// behavior no longer exists to test.

/// **Commit 3 lock**: AST-driven span splicing produces the expected
/// renamed source. Same input as the old regex-driven version.
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

/// **Commit 3 gate**: `Session::navigation_rename_locations_query`
/// returns at least the class name's two occurrences (header `model X`
/// and `end X;`). If this passes we can drop the regex header/footer
/// rewrites in `commands.rs` for class rename and route through rumoca.
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
