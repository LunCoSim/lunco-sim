// ── Modelica lint policy — the rules, in rhai ───────────────────────────────
//
// Hook `lint.modelica`, entry `lint_modelica(facts)`. Registered at startup from
// this file (`lunco_scripting::register_builtin_policies`) and REPLACEABLE AT
// RUNTIME:
//
//     register_hook("lint.modelica", "lint_modelica", my_source)
//     unregister_hook("lint.modelica")            // back to no Modelica rules
//
// so a rule can be written, tried and tightened with no rebuild.
//
// ── WHAT A RULE MAY SEE ─────────────────────────────────────────────────────
//
// PARSE-phase facts only, built by `lunco-modelica/src/lint.rs`. A model's
// variables, its DAE and its solver do not exist until a COMPILE, so no rule
// here can ask about them — `.mo` validation is deliberately a pre-flight, not a
// simulation.
//
//   facts.model            the class name, "" when extraction failed
//   facts.params[]         #{ name, value }   declared `parameter`s
//   facts.inputs[]         #{ name, default } declared `input`s
//   facts.param_names[]    names alone, for cheap `in` tests
//   facts.input_names[]    ditto
//   facts.shadowed[]       names declared BOTH input and parameter
//
// Plus the validator's own facts, shared by every domain:
//
//   facts.path, facts.kind, facts.ok, facts.errors[], facts.source
//
// ── TRAPS ───────────────────────────────────────────────────────────────────
//
// rhai has an expression-complexity cap: build a finding field by field through
// `finding()` and grow messages with `+=`. A single large expression makes the
// WHOLE policy fail to compile, and a policy that does not compile registers
// nothing — the linter then reports a clean bill of health for a broken file.

fn finding(rule, severity, subject, message) {
    let f = #{};
    f.rule = rule;
    f.severity = severity;
    f.subject = subject;
    f.message = message;
    f
}

/// **A name is declared both `input` and `parameter`.**
///
/// The two are steered by different owners: an `input` is a cosim wire written
/// every step from another model's output, a `parameter` is a value the
/// experiments UI and `RunBounds` overrides set once. Declaring one name as both
/// means whichever writes last wins, silently, and the two surfaces disagree
/// about what the model is doing — the parameter dialog shows a value the wire
/// is overwriting every step.
fn input_shadows_parameter(facts) {
    let out = [];
    for name in facts.shadowed {
        let msg = "`";
        msg += name;
        msg += "` is declared as both an `input` and a `parameter`. A cosim wire";
        msg += " writes an input every step while a parameter override writes it";
        msg += " once, so the two fight and the value shown in the experiments";
        msg += " dialog is not the value the model integrates. Keep it an";
        msg += " `input` if another model drives it, a `parameter` if the user";
        msg += " sets it.";
        out.push(finding("input-shadows-parameter", "error", name, msg));
    }
    out
}

fn lint_modelica(facts) {
    let out = [];
    for f in input_shadows_parameter(facts) { out.push(f); }
    out
}
