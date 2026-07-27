//! Every shipped `.mo` must pass the Modelica lint rules, and the rules must
//! have teeth.
//!
//! A policy with nothing running it is a comment, and a policy that fails to
//! COMPILE registers nothing at all — which reads exactly like a clean bill of
//! health. So this pins both halves: the shipped rules load and fire on a model
//! authored wrong, and every model we ship comes back clean.
//!
//! It registers the policy itself rather than booting an app, for the same
//! reason the USD gate does: the rules are an asset, the hook registry is
//! global, and a `cargo test` that needs a window is one nobody runs.

use lunco_hooks::HookValue as H;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn assets_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")
}

/// Register `lint.modelica` from the shipped policy — the same source
/// `lunco_scripting::register_builtin_policies` loads at startup.
fn register_modelica_lint_policy() {
    let src = std::fs::read_to_string(assets_dir().join("scripting/policy/lint_modelica.rhai"))
        .expect("assets/scripting/policy/lint_modelica.rhai is missing");
    lunco_hooks_rhai::register_rhai_hook("lint.modelica", "lint_modelica", &src, false)
        .expect("the shipped Modelica lint policy must compile");
}

fn facts(params: &[(&str, f64)], inputs: &[(&str, f64)]) -> H {
    let p: BTreeMap<String, f64> = params.iter().map(|(n, v)| (n.to_string(), *v)).collect();
    let i: BTreeMap<String, f64> = inputs.iter().map(|(n, v)| (n.to_string(), *v)).collect();
    lunco_modelica::lint::modelica_facts("M", &p, &i, &Default::default())
}

/// Every `.mo` under `dir`, recursively.
fn mo_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            mo_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "mo") {
            out.push(p);
        }
    }
}

/// The rule fires on a model that declares one name as both an input and a
/// parameter. Without this, a policy that silently failed to compile would look
/// identical to a codebase with no offenders.
#[test]
fn a_name_declared_input_and_parameter_is_caught() {
    register_modelica_lint_policy();

    let findings = lunco_lint::run_lint(
        lunco_modelica::lint::MODELICA_LINT_DOMAIN,
        facts(&[("throttle", 1.0)], &[("throttle", 0.0)]),
    );

    assert!(
        findings.iter().any(|f| f.rule == "input-shadows-parameter"),
        "the shipped policy must flag a shadowed name: {findings:?}"
    );
}

/// An algebraic variable defined behind an `if` must be caught.
///
/// This one needs its own teeth test rather than relying on the shipped-models
/// sweep below: every model that carried the pattern has been rewritten
/// branch-free, so the sweep would pass whether or not the rule fires. The
/// defect it guards is silent — rumoca's branch-free solver path reconstructs
/// such an equation as literal `0`, with no compile or run error — so a rule
/// that quietly stopped working would look exactly like a clean codebase.
#[test]
fn a_conditional_algebraic_observable_is_caught() {
    register_modelica_lint_policy();

    let src = "model M\n  Real f;\n  Real x;\nequation\n  f = if x > 0.0 then x else 0.0;\n  x = 1.0;\nend M;\n";
    let syntax = rumoca_phase_parse::parse_to_syntax(src, "M.mo");
    let ast = syntax.best_effort();
    let model = lunco_modelica::extract_model_name_from_ast(ast).unwrap_or_default();

    let findings = lunco_lint::run_lint(
        lunco_modelica::lint::MODELICA_LINT_DOMAIN,
        lunco_modelica::lint::modelica_facts(
            &model,
            &BTreeMap::new(),
            &BTreeMap::new(),
            ast,
        ),
    );

    assert!(
        findings
            .iter()
            .any(|f| f.rule == "conditional-algebraic-observable"),
        "the shipped policy must flag an `if`-guarded algebraic observable: {findings:?}"
    );
}

/// A `when` clause is a discrete event, not the silent-zero defect, so the error
/// rule must NOT fire on it — otherwise the only way to keep the lint quiet is to
/// stop expressing events at all.
#[test]
fn a_when_equation_is_not_flagged_as_a_conditional_observable() {
    register_modelica_lint_policy();

    let src = "model M\n  Real x;\n  discrete Real n;\nequation\n  x = 1.0;\n  when x > 0.5 then\n    n = pre(n) + 1;\n  end when;\nend M;\n";
    let syntax = rumoca_phase_parse::parse_to_syntax(src, "M.mo");
    let ast = syntax.best_effort();
    let model = lunco_modelica::extract_model_name_from_ast(ast).unwrap_or_default();

    let findings = lunco_lint::run_lint(
        lunco_modelica::lint::MODELICA_LINT_DOMAIN,
        lunco_modelica::lint::modelica_facts(
            &model,
            &BTreeMap::new(),
            &BTreeMap::new(),
            ast,
        ),
    );

    assert!(
        !findings
            .iter()
            .any(|f| f.rule == "conditional-algebraic-observable"),
        "a `when` event must not trip the conditional-observable rule: {findings:?}"
    );
}

/// A model whose inputs and parameters are disjoint is clean. This is what keeps
/// the rule from being a tax on every ordinary model.
#[test]
fn a_model_with_disjoint_inputs_and_parameters_is_clean() {
    register_modelica_lint_policy();

    let findings = lunco_lint::run_lint(
        lunco_modelica::lint::MODELICA_LINT_DOMAIN,
        facts(&[("m_dry", 120.0)], &[("throttle", 0.0)]),
    );

    assert!(
        findings.is_empty(),
        "an ordinary model must lint clean: {findings:?}"
    );
}

/// THE GATE: every `.mo` we ship, through the real parse + fact extraction, must
/// produce no Modelica findings.
#[test]
fn every_shipped_model_lints_clean() {
    register_modelica_lint_policy();

    let models_dir = assets_dir().join("models");
    let mut files = Vec::new();
    mo_files(&models_dir, &mut files);
    assert!(
        !files.is_empty(),
        "no .mo files found under {} — the gate would pass vacuously",
        models_dir.display()
    );

    let mut offenders: Vec<String> = Vec::new();
    for path in &files {
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("shipped.mo");

        // The same parse + extraction `validate_modelica` performs.
        let syntax = rumoca_phase_parse::parse_to_syntax(&text, file_name);
        let ast = syntax.best_effort();
        let model = lunco_modelica::extract_model_name_from_ast(ast).unwrap_or_default();
        let params: BTreeMap<String, f64> = lunco_modelica::extract_parameters_from_ast(ast)
            .into_iter()
            .collect();
        let inputs: BTreeMap<String, f64> =
            lunco_modelica::extract_inputs_with_defaults_from_ast(ast)
                .into_iter()
                .collect();

        let findings = lunco_lint::run_lint(
            lunco_modelica::lint::MODELICA_LINT_DOMAIN,
            lunco_modelica::lint::modelica_facts(&model, &params, &inputs, ast),
        );
        for f in findings {
            offenders.push(format!("{}: {} — {}", path.display(), f.rule, f.message));
        }
    }

    assert!(
        offenders.is_empty(),
        "shipped models must satisfy the Modelica lint rules:\n  {}",
        offenders.join("\n  ")
    );
}
