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
// Plus the SHAPE of the equation section — the equations are in the AST at parse
// even though their values are not:
//
//   facts.conditional_equations[]        #{ name, kind, form, line }
//        name  the variable the equation defines ("" if the LHS is not a
//              component reference), `x` for `der(x) = …`
//        kind  "algebraic" | "state" | "parameter" | "discrete" | "input" |
//              "unknown" (inherited through `extends`, a sub-component member
//              like `tank.level`, or the parse lost the declaration)
//        form  "if-expression"  `x = if c then e else 0`   ← the silent zero
//              "if-equation"    `if c then x = a; end if;`
//              "when-equation"  a discrete event — a DIFFERENT defect
//        line  1-based source line, 0 when the AST carried no location
//   facts.conditional_algebraic_names[]  the `algebraic` × `if-expression`
//                                        subset, precomputed
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

/// **An algebraic observable is defined behind a branch.**
///
/// rumoca's solver path is BRANCH-FREE, and its elimination reconstructor
/// substitutes continuous expressions only. `x = if cond then e else 0` therefore
/// parses, compiles, and then evaluates as literal **0** at every step — the
/// variable reports a plausible-looking zero instead of failing, and every
/// observable computed from it goes to zero with it. This is the defect that made
/// `RocketEngine`'s thrust read 0 at full throttle.
///
/// The fix is always to rewrite the equation branch-free: a one-sided quantity is
/// a `max(0, …)`, a two-sided clamp is `min(hi, max(lo, …))`, and a gate is a
/// product of such terms rather than a test. Keep the arithmetic; drop the
/// branch.
fn conditional_algebraic_observable(facts) {
    let out = [];
    for name in facts.conditional_algebraic_names {
        let msg = "`";
        msg += name;
        msg += "` is an algebraic variable defined behind a conditional. rumoca's";
        msg += " solver path is branch-free and reconstructs continuous";
        msg += " expressions only, so this equation evaluates as literal 0 at";
        msg += " every step — silently, with no compile or run error. Rewrite it";
        msg += " branch-free: a one-sided quantity (a contact force, a";
        msg += " non-negative flow) becomes `max(0.0, ...)`, a bounded one";
        msg += " `min(hi, max(lo, ...))`, and a gate becomes a PRODUCT of such";
        msg += " terms instead of a test. Keep the arithmetic, drop the `if`.";
        out.push(finding("conditional-algebraic-observable", "error", name, msg));
    }
    out
}

/// The same branch, on a variable this file does not declare — inherited through
/// `extends`, a member of a sub-component (`tank.level`), or a declaration the
/// recovering parser lost. It is reported SEPARATELY and as a warning because the
/// facts cannot prove the variable is a continuous algebraic: acting on it needs
/// the author to look, and an error here would be a guess wearing certainty.
fn conditional_equation_of_unknown_variable(facts) {
    let out = [];
    for eq in facts.conditional_equations {
        if eq.form != "if-expression" { continue; }
        if eq.kind != "unknown" { continue; }
        let subject = eq.name;
        if subject == "" { subject = "equation"; }
        let msg = "The equation at line ";
        msg += eq.line;
        msg += " defines `";
        msg += subject;
        msg += "` from a conditional expression. rumoca's solver path is";
        msg += " branch-free, so a conditional here reads as literal 0 if the";
        msg += " variable is continuous. This file does not declare `";
        msg += subject;
        msg += "` (it is inherited, a sub-component member, or the parse lost";
        msg += " the declaration), so the check cannot prove that — check it,";
        msg += " and if it is continuous rewrite the branch as `max`/`min`";
        msg += " arithmetic.";
        out.push(finding("conditional-equation-unresolved", "warning", subject, msg));
    }
    out
}

fn lint_modelica(facts) {
    let out = [];
    for f in input_shadows_parameter(facts) { out.push(f); }
    for f in conditional_algebraic_observable(facts) { out.push(f); }
    for f in conditional_equation_of_unknown_variable(facts) { out.push(f); }
    out
}
