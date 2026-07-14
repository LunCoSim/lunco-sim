//! Round-trip fidelity tests: source → AST → source → AST.
//!
//! Gate for the AST-canonical migration documented in
//! `docs/architecture/TAB_AST_ROADMAP.md` Section A. Section A's Path A
//! commits to regenerating source via [`rumoca_ir_ast::StoredDefinition::to_modelica`]
//! after every structural edit. Whether that regeneration is *safe*
//! depends on a single empirical question: does the AST survive the
//! round trip, and what bytes change?
//!
//! ## What we measure
//!
//! 1. **Idempotency** (`assert_idempotent`): parse → emit → parse → emit.
//!    The two emitted strings must be byte-identical. If yes, the
//!    formatter is a fixed point and AST equivalence is implied without
//!    needing to compare `StoredDefinition`s directly (which would also
//!    need to ignore `Location` / `def_id` / `scope_id` differences
//!    that come from re-parsing).
//!
//! 2. **Reparse-after-emit** (`assert_reparses`): after a full round
//!    trip, the regenerated source still parses without errors.
//!    Catches catastrophic emitter bugs (missing semicolons,
//!    unbalanced quotes) early without any AST comparison.
//!
//! ## TDD discipline
//!
//! Per `AGENTS.md` §1, tests precede implementation. The
//! `lunco-modelica/src/ast_mut/` module that lands in A.2 batch 1 will
//! depend on the conclusions drawn here:
//!
//! - **All passing** → Path A is unblocked. `ast_mut::*` helpers can
//!   mutate `ClassDef` and rely on `StoredDefinition::to_modelica()` for
//!   source regeneration.
//! - **A subset fails** → the failing fixtures classify what shape of
//!   AST input the emitter mishandles. Either fix upstream in
//!   `rumoca-ir-ast::modelica`, or fall back to per-class TextEdit
//!   splicing for ops that touch the affected shape.
//!
//! Failing tests are marked `#[ignore]` with a `TODO(A.2):` comment
//! explaining the surface they exercise — the goal is a documented gap,
//! not a hidden one.
//!
//! ## What we deliberately do *not* test here
//!
//! - **Comment preservation.** Comments inside class bodies live in
//!   token streams that may not survive `parse → to_modelica`. The
//!   AST-canonical roadmap accepts "normalize on save" as a trade-off
//!   matching Dymola/OMEdit behaviour. A separate test
//!   (`comment_fidelity.rs`) will measure exactly which classes of
//!   comments are lost — that's a UX concern, not a correctness one.
//!
//! - **Byte-for-byte source equality.** Whitespace, blank lines, and
//!   indentation are expected to normalize. We assert AST equivalence
//!   (via emitter idempotency), not byte equality.

use rumoca_phase_parse::parse_to_ast;

/// Parse `source`, emit, parse, emit. Assert the two emissions are
/// byte-identical (formatter idempotency ⇒ AST equivalence).
fn assert_idempotent(source: &str, fixture_name: &str) {
    let ast1 = parse_to_ast(source, fixture_name)
        .unwrap_or_else(|e| panic!("first parse of {fixture_name} failed: {e:?}"));
    let regen1 = ast1.to_modelica();

    let ast2 = parse_to_ast(&regen1, fixture_name).unwrap_or_else(|e| {
        panic!(
            "second parse failed for {fixture_name}: {e:?}\n\
             === regenerated source ===\n{regen1}\n=========================="
        )
    });
    let regen2 = ast2.to_modelica();

    if regen1 != regen2 {
        // Emit a unified-diff-ish failure so the gap is obvious without
        // re-running with `--nocapture`.
        panic!(
            "to_modelica() not idempotent for {fixture_name}\n\
             === pass 1 output ===\n{regen1}\n\
             === pass 2 output ===\n{regen2}\n\
             ====================="
        );
    }
}

/// Weaker check: round-trip parses cleanly. Catches emitter bugs that
/// produce syntactically invalid output. Strictly subsumed by
/// `assert_idempotent`, but useful as a tighter failure message when
/// idempotency fails because the *first* round trip already breaks.
fn assert_reparses(source: &str, fixture_name: &str) {
    let ast = parse_to_ast(source, fixture_name)
        .unwrap_or_else(|e| panic!("first parse of {fixture_name} failed: {e:?}"));
    let regen = ast.to_modelica();
    let _ = parse_to_ast(&regen, fixture_name).unwrap_or_else(|e| {
        panic!(
            "regenerated source no longer parses for {fixture_name}: {e:?}\n\
             === regenerated source ===\n{regen}\n=========================="
        )
    });
}

// ─────────────────────────────────────────────────────────────────────────────
// Tier 1 — minimal shapes.
// These should pass on day one. Regression here means the emitter is
// fundamentally broken; abandon Path A.
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn empty_model() {
    assert_idempotent("model M\nend M;\n", "empty_model.mo");
}

#[test]
fn one_real_component() {
    assert_idempotent(
        "model M\n  Real x;\nend M;\n",
        "one_real_component.mo",
    );
}

#[test]
fn one_equation() {
    assert_idempotent(
        "model M\n  Real x;\nequation\n  x = 1;\nend M;\n",
        "one_equation.mo",
    );
}

#[test]
fn parameter_with_default() {
    assert_idempotent(
        "model M\n  parameter Real k = 1.0;\nend M;\n",
        "parameter_with_default.mo",
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Tier 2 — structural shapes the canvas drives.
// These directly correspond to ModelicaOp variants in A.2 batch 1 / 2:
// AddComponent, ConnectComponents, SetPlacement, SetParameter.
// ─────────────────────────────────────────────────────────────────────────────

// TODO(rumoca): `to_modelica()` mangles a component with MULTIPLE modifiers plus
// a binding equation — it drops the `start` modifier and corrupts the binding
// value (data loss), and is not even idempotent:
//     in:     parameter Real k(start = 1.0, fixed = true) = 0.5;
//     pass 1: parameter Real k(fixed = true) = 1.0;   // dropped `start`; 0.5 → 1.0
//     pass 2: parameter Real k(fixed = true) = 0.0;   // 1.0 → 0.0
// Bug is entirely in rumoca's parse/emit (this test only calls parse_to_ast +
// to_modelica) — NOT in lunco's AST-op layer. Fixing it means editing rumoca, so
// it's parked here. Un-ignore once rumoca preserves multi-modifier decls.
#[test]
#[ignore = "rumoca to_modelica drops `start` modifier + corrupts binding value; see TODO above"]
fn component_with_modification() {
    assert_idempotent(
        "model M\n  parameter Real k(start = 1.0, fixed = true) = 0.5;\nend M;\n",
        "component_with_modification.mo",
    );
}

#[test]
fn connect_equation() {
    assert_idempotent(
        "model M\n  Real a;\n  Real b;\nequation\n  connect(a, b);\nend M;\n",
        "connect_equation.mo",
    );
}

#[test]
fn class_with_placement_annotation() {
    // Placement annotations are the canvas drag target. If the emitter
    // reorders or normalises the annotation tree non-idempotently, the
    // first canvas drag after a Format would silently rewrite every
    // component's placement.
    let src = "\
model M
  Real x annotation(Placement(transformation(extent={{-10, -10}, {10, 10}})));
end M;
";
    assert_idempotent(src, "class_with_placement_annotation.mo");
}

// ─────────────────────────────────────────────────────────────────────────────
// Tier 3 — vendor annotations + LunCo-specific shapes.
// Lunco fully owns these emissions, so any fidelity gap is a bug we
// can fix locally without upstreaming.
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn lunco_plot_node_annotation() {
    // `__LunCo(...)` is the vendor annotation namespace the canvas adds
    // plot nodes into. Vendor annotations use ordinary Modelica
    // modification syntax (named modifications + literal arrays), so
    // any well-formed payload should round-trip.
    let src = "\
model M
  Real x;
equation
  x = time;
  annotation(__LunCo(plots(p1(extent = {{0, 0}, {100, 100}}))));
end M;
";
    assert_idempotent(src, "lunco_plot_node_annotation.mo");
}

// ─────────────────────────────────────────────────────────────────────────────
// Tier 4 — Modelica idioms used heavily across MSL.
// Pass these and Path A holds for the bulk of real-world models.
// Fail any → A.2 needs a workaround for the affected op family.
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn extends_clause() {
    assert_idempotent(
        "model M\n  extends Modelica.Icons.Example;\n  Real x;\nend M;\n",
        "extends_clause.mo",
    );
}

#[test]
fn extends_with_modification() {
    // `extends X(...)` modification rewriting is the densest piece of
    // the Modification tree. SetParameter on an inherited modification
    // lands here.
    assert_idempotent(
        "model M\n  extends Base(k = 2.0);\nend M;\n",
        "extends_with_modification.mo",
    );
}

#[test]
fn nested_class() {
    assert_idempotent(
        "package P\n  model Inner\n    Real x;\n  end Inner;\nend P;\n",
        "nested_class.mo",
    );
}

#[test]
fn within_clause() {
    // Files in MSL packages start with `within Package.Path;`. If the
    // emitter drops or reorders this, every package-internal class
    // moves to the root namespace on save.
    assert_idempotent(
        "within Modelica.Mechanics;\nmodel M\n  Real x;\nend M;\n",
        "within_clause.mo",
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Tier 5 — shapes flagged as risky in the audit. All three round-trip cleanly
// as of rumoca 0.9.20 and are no longer `#[ignore]`d: they now GUARD the
// behaviour instead of documenting its absence. (The one genuinely broken shape
// is `component_with_modification` above — multi-modifier + binding — which
// still loses data.)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn redeclare_short_class() {
    let src = "\
model M
  redeclare model Engine = SimpleEngine;
end M;
";
    assert_idempotent(src, "redeclare_short_class.mo");
}

#[test]
fn conditional_component() {
    let src = "\
model M
  parameter Boolean useHeat = false;
  Real q if useHeat;
end M;
";
    assert_idempotent(src, "conditional_component.mo");
}

#[test]
fn inner_outer_prefix() {
    let src = "\
model M
  inner outer Real env;
end M;
";
    assert_idempotent(src, "inner_outer_prefix.mo");
}

// ─────────────────────────────────────────────────────────────────────────────
// Tier 6 — weak-gate sanity. If `assert_idempotent` is too strict for
// initial bring-up, these confirm at least no catastrophic emitter
// breakage. Useful triage when a Tier-2 / Tier-4 test fails.
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn reparse_minimal() {
    assert_reparses("model M\nend M;\n", "reparse_minimal.mo");
}

#[test]
fn reparse_with_components_and_equations() {
    assert_reparses(
        "model M\n  Real a;\n  Real b;\nequation\n  a = b + 1;\nend M;\n",
        "reparse_with_components_and_equations.mo",
    );
}
