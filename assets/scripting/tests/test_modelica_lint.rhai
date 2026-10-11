// The Modelica lint policy's teeth test. The host only concatenates this file
// with lint_modelica.rhai; all facts, rule selection, and expected verdicts
// stay authored here.

fn require(condition, message) {
    if !condition { throw message; }
}

fn facts(params, inputs, constructs) {
    #{
        model: "M",
        params: params,
        inputs: inputs,
        conditional_constructs: constructs,
    }
}

let overlap = lint_modelica(facts(
    [#{ name: "throttle", value: 1.0 }],
    [#{ name: "throttle", "default": 0.0 }],
    []));
require(overlap.len() == 1 && overlap[0].rule == "input-shadows-parameter",
    "the Modelica policy must catch an input shadowing a parameter");

let clean = lint_modelica(facts(
    [#{ name: "dry_mass", value: 120.0 }],
    [#{ name: "throttle", "default": 0.0 }],
    []));
require(clean.is_empty(), "disjoint inputs and parameters must lint clean");
print("MODELICA_LINT_SELFTEST PASS");
