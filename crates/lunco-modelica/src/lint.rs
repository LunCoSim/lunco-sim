//! FACTS for the `modelica` lint domain.
//!
//! The split is `lunco-lint`'s: Rust extracts what is true about a model, and
//! `assets/scripting/policy/lint_modelica.rhai` decides what is worth saying
//! about it. Only this crate can read a Modelica AST, so extraction is code and
//! is tested as code; a rule is a line in a script that can be retuned against a
//! running sim.
//!
//! These are PARSE-phase facts — names, parameters, declared inputs, and the
//! SHAPE of the equation section. A model's variables and its solver do not
//! exist until a compile, so no rule reached from `ValidateAsset` can ask about
//! their values; but the equations themselves are in the AST at parse, and the
//! worst defect this domain has is a shape, not a value:
//!
//! rumoca's solver path is BRANCH-FREE. An algebraic observable defined behind a
//! conditional — `x = if cond then e else 0` — parses, compiles, and then reads
//! literal **0** at runtime because the elimination reconstructor only
//! substitutes continuous expressions. Nothing fails; the observable just lies.
//! That is only lintable if a rule can see which variable a conditional
//! equation defines, so the equation facts below exist for exactly that rule.

use lunco_hooks::HookValue as H;
use rumoca_compile::parsing::ast::Equation;
use rumoca_compile::parsing::{Causality, ClassDef, Expression, StoredDefinition, Variability};
use std::collections::BTreeMap;

/// The lint domain name, and with it the hook (`lint.modelica`) and the policy
/// file (`assets/scripting/policy/lint_modelica.rhai`).
pub const MODELICA_LINT_DOMAIN: &str = "modelica";

/// Parse-phase facts about one model, for the authored rules.
///
/// Shape (merged at TOP LEVEL into the validator's facts, never nested — a
/// nested fact map is one every rule silently fails to match):
///
/// ```text
/// model:      "RocketStage"
/// params:     [ #{ name: "m_dry", value: 120.0 }, … ]
/// inputs:     [ #{ name: "throttle", default: 0.0 }, … ]
/// param_names / input_names:  [ "m_dry", … ]   // for cheap `in` tests
/// shadowed:   [ "x", … ]   // declared BOTH input and parameter
/// conditional_equations: [ #{ name: "m_dot", kind: "algebraic",
///                            form: "if-expression", line: 42 }, … ]
/// conditional_algebraic_names: [ "m_dot", … ]   // the silent-zero set
/// ```
///
/// `shadowed` and `conditional_algebraic_names` are computed here rather than
/// left to the policy because set intersection and nested filtering in rhai run
/// into the expression-complexity cap that makes a whole policy fail to
/// compile — the same trap the drivetrain rules hit.
///
/// `ast` is the best-effort parse the caller already holds (the recovering
/// parser's `best_effort()`); an empty `StoredDefinition` is legitimate input and
/// simply yields no equation facts.
pub fn modelica_facts(
    model: &str,
    params: &BTreeMap<String, f64>,
    inputs: &BTreeMap<String, f64>,
    ast: &StoredDefinition,
) -> H {
    let param_entries: Vec<H> = params
        .iter()
        .map(|(name, value)| H::map([("name", H::Str(name.clone())), ("value", H::Float(*value))]))
        .collect();

    let input_entries: Vec<H> = inputs
        .iter()
        .map(|(name, default)| {
            H::map([
                ("name", H::Str(name.clone())),
                ("default", H::Float(*default)),
            ])
        })
        .collect();

    let shadowed: Vec<H> = inputs
        .keys()
        .filter(|n| params.contains_key(*n))
        .map(|n| H::Str(n.clone()))
        .collect();

    let conditionals = conditional_equations(ast);
    let silent_zero: Vec<H> = conditionals
        .iter()
        .filter(|c| c.kind == VarKind::Algebraic && c.form == CondForm::IfExpression)
        .map(|c| H::Str(c.name.clone()))
        .collect();
    let conditional_entries: Vec<H> = conditionals
        .iter()
        .map(ConditionalEquation::to_fact)
        .collect();

    H::map([
        ("model", H::Str(model.to_string())),
        ("params", H::Array(param_entries)),
        ("inputs", H::Array(input_entries)),
        (
            "param_names",
            H::Array(params.keys().map(|n| H::Str(n.clone())).collect()),
        ),
        (
            "input_names",
            H::Array(inputs.keys().map(|n| H::Str(n.clone())).collect()),
        ),
        ("shadowed", H::Array(shadowed)),
        ("conditional_equations", H::Array(conditional_entries)),
        ("conditional_algebraic_names", H::Array(silent_zero)),
    ])
}

// ---------------------------------------------------------------------------
// Equation-shape extraction
// ---------------------------------------------------------------------------

/// What the conditional is, syntactically. The three are NOT the same defect,
/// and a rule that conflated them would either miss the silent zero or shout at
/// every event-driven model — so the distinction is drawn here, where the AST
/// still has it, and never left to the policy to guess from a name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CondForm {
    /// `x = if cond then e else 0` — a conditional EXPRESSION on the RHS. This
    /// is the silent zero: it survives compilation and reads 0 at runtime.
    IfExpression,
    /// `if cond then x = a; else x = b; end if;` — a structural if-EQUATION.
    IfEquation,
    /// `when cond then x = a; end when;` — a discrete event, a different thing
    /// entirely (and one rumoca's branch-free path also cannot take).
    WhenEquation,
}

impl CondForm {
    fn as_str(self) -> &'static str {
        match self {
            CondForm::IfExpression => "if-expression",
            CondForm::IfEquation => "if-equation",
            CondForm::WhenEquation => "when-equation",
        }
    }
}

/// What the equation defines, from the component's own declaration. Only
/// `Algebraic` is the silent-zero case: a state is reconstructed by the
/// integrator from its derivative, a parameter never enters the DAE, and a
/// `discrete` is an event variable whose author already knows it is not
/// continuous.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VarKind {
    Algebraic,
    State,
    Parameter,
    Discrete,
    Input,
    /// Not declared in this class — inherited through `extends`, or a member of
    /// a sub-component (`tank.level`), or the recovering parser lost the
    /// declaration. The rule must not claim certainty about these.
    Unknown,
}

impl VarKind {
    fn as_str(self) -> &'static str {
        match self {
            VarKind::Algebraic => "algebraic",
            VarKind::State => "state",
            VarKind::Parameter => "parameter",
            VarKind::Discrete => "discrete",
            VarKind::Input => "input",
            VarKind::Unknown => "unknown",
        }
    }
}

struct ConditionalEquation {
    name: String,
    kind: VarKind,
    form: CondForm,
    line: i64,
}

impl ConditionalEquation {
    fn to_fact(&self) -> H {
        H::map([
            ("name", H::Str(self.name.clone())),
            ("kind", H::Str(self.kind.as_str().to_string())),
            ("form", H::Str(self.form.as_str().to_string())),
            ("line", H::Int(self.line)),
        ])
    }
}

/// Every equation in `ast` whose value depends on a branch, with the variable it
/// defines. Walks the `equation` sections of every class, nested classes
/// included; `initial equation` is left out deliberately — it runs once, before
/// the solver, so a branch there is not the runtime lie this exists to catch.
fn conditional_equations(ast: &StoredDefinition) -> Vec<ConditionalEquation> {
    let mut out = Vec::new();
    for class in ast.classes.values() {
        collect_from_class(class, &mut out);
    }
    out
}

fn collect_from_class(class: &ClassDef, out: &mut Vec<ConditionalEquation>) {
    for eq in &class.equations {
        collect_from_equation(eq, class, None, out);
    }
    for nested in class.classes.values() {
        collect_from_class(nested, out);
    }
}

/// `inherited_form` is `Some(..)` while walking the body of an `if`/`when`
/// equation: an equation inside one is conditional whatever its own RHS looks
/// like.
fn collect_from_equation(
    eq: &Equation,
    class: &ClassDef,
    inherited_form: Option<CondForm>,
    out: &mut Vec<ConditionalEquation>,
) {
    let line = eq
        .get_location()
        .map(|l| i64::from(l.start_line))
        .unwrap_or(0);
    match eq {
        Equation::Simple { lhs, rhs } => {
            let form = match inherited_form {
                Some(f) => Some(f),
                None if expression_is_conditional(rhs) => Some(CondForm::IfExpression),
                None => None,
            };
            let Some(form) = form else { return };
            let (name, is_derivative) = defined_variable(lhs);
            let kind = if is_derivative {
                VarKind::State
            } else {
                declared_kind(class, &name)
            };
            out.push(ConditionalEquation {
                name,
                kind,
                form,
                line,
            });
        }
        Equation::If {
            cond_blocks,
            else_block,
        } => {
            for block in cond_blocks {
                for inner in &block.eqs {
                    collect_from_equation(inner, class, Some(CondForm::IfEquation), out);
                }
            }
            for inner in else_block.iter().flatten() {
                collect_from_equation(inner, class, Some(CondForm::IfEquation), out);
            }
        }
        Equation::When(blocks) => {
            for block in blocks {
                for inner in &block.eqs {
                    collect_from_equation(inner, class, Some(CondForm::WhenEquation), out);
                }
            }
        }
        Equation::For { equations, .. } => {
            for inner in equations {
                collect_from_equation(inner, class, inherited_form, out);
            }
        }
        // Connect / FunctionCall / Assert / Empty define nothing an observable
        // is read from.
        _ => {}
    }
}

/// Does this expression's value depend on a branch, at any depth? `x = 1 + (if c
/// then a else 0)` is the same defect as the bare form, so the walk is
/// recursive rather than a top-level `matches!`.
fn expression_is_conditional(expr: &Expression) -> bool {
    match expr {
        Expression::If { .. } => true,
        Expression::Unary { rhs, .. } => expression_is_conditional(rhs),
        Expression::Binary { lhs, rhs, .. } => {
            expression_is_conditional(lhs) || expression_is_conditional(rhs)
        }
        Expression::Parenthesized { inner, .. } => expression_is_conditional(inner),
        Expression::FunctionCall { args, .. } => args.iter().any(expression_is_conditional),
        Expression::Array { elements, .. } | Expression::Tuple { elements, .. } => {
            elements.iter().any(expression_is_conditional)
        }
        Expression::Range {
            start, step, end, ..
        } => {
            expression_is_conditional(start)
                || step.as_deref().is_some_and(expression_is_conditional)
                || expression_is_conditional(end)
        }
        Expression::ArrayIndex { base, .. } | Expression::FieldAccess { base, .. } => {
            expression_is_conditional(base)
        }
        _ => false,
    }
}

/// The variable an equation's left-hand side defines, and whether it was written
/// as `der(x)`. Returns an empty name for a left-hand side that is not a
/// component reference (a tuple output, say) — a fact a rule can test rather
/// than an absence it has to infer.
fn defined_variable(lhs: &Expression) -> (String, bool) {
    match lhs {
        Expression::ComponentReference(cr) => (cr.to_string(), false),
        Expression::FunctionCall { comp, args, .. } if comp.to_string() == "der" => {
            match args.first() {
                Some(Expression::ComponentReference(cr)) => (cr.to_string(), true),
                _ => (String::new(), true),
            }
        }
        _ => (String::new(), false),
    }
}

/// Classify a defined name against the class's own declarations.
fn declared_kind(class: &ClassDef, name: &str) -> VarKind {
    // `a.b` and `x[i]` are declared under their base identifier.
    let base = name
        .split('.')
        .next()
        .unwrap_or(name)
        .split('[')
        .next()
        .unwrap_or(name);
    if base.is_empty() || base != name {
        // A dotted or subscripted reference: the declaration in hand describes
        // the container, not the thing the equation defines.
        return VarKind::Unknown;
    }
    let Some(component) = class.components.get(base) else {
        return VarKind::Unknown;
    };
    match component.variability {
        Variability::Parameter(_) | Variability::Constant(_) => VarKind::Parameter,
        Variability::Discrete(_) => VarKind::Discrete,
        _ => match component.causality {
            Causality::Input(_) => VarKind::Input,
            _ => VarKind::Algebraic,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts_of(params: &[(&str, f64)], inputs: &[(&str, f64)]) -> H {
        let p: BTreeMap<String, f64> =
            params.iter().map(|(n, v)| (n.to_string(), *v)).collect();
        let i: BTreeMap<String, f64> =
            inputs.iter().map(|(n, v)| (n.to_string(), *v)).collect();
        modelica_facts("M", &p, &i, &StoredDefinition::default())
    }

    /// Facts from real source, through the same recovering parse `validate_asset`
    /// uses — the equation facts only exist if the AST really carries them.
    fn facts_of_source(src: &str) -> H {
        let syntax = rumoca_phase_parse::parse_to_syntax(src, "M.mo");
        let ast = syntax.best_effort();
        modelica_facts("M", &BTreeMap::new(), &BTreeMap::new(), ast)
    }

    /// `#{ name, kind, form }` triples, for asserting on shape without pinning
    /// line numbers into every test.
    fn conditionals(facts: &H) -> Vec<(String, String, String)> {
        let H::Array(entries) = key(facts, "conditional_equations") else {
            panic!("conditional_equations is an array")
        };
        entries
            .iter()
            .map(|e| {
                let s = |k: &str| match key(e, k) {
                    H::Str(v) => v.clone(),
                    other => panic!("{k} is a string, got {other:?}"),
                };
                (s("name"), s("kind"), s("form"))
            })
            .collect()
    }

    fn key<'a>(facts: &'a H, k: &str) -> &'a H {
        let H::Map(entries) = facts else {
            panic!("facts are a map")
        };
        &entries.iter().find(|(name, _)| name == k).expect(k).1
    }

    /// The one fact a rule cannot cheaply compute for itself: a name declared as
    /// BOTH an input and a parameter. The cosim would wire it while the
    /// parameter override also writes it, and neither surface says so.
    #[test]
    fn a_name_declared_input_and_parameter_is_reported_as_shadowed() {
        let facts = facts_of(&[("x", 1.0), ("m", 2.0)], &[("x", 0.0), ("throttle", 0.0)]);
        let H::Array(shadowed) = key(&facts, "shadowed") else {
            panic!("shadowed is an array")
        };
        assert_eq!(shadowed.len(), 1, "expected exactly `x`: {shadowed:?}");
        assert_eq!(shadowed[0], H::Str("x".to_string()));
    }

    /// A model whose inputs and parameters are disjoint — the normal case —
    /// must report nothing, or every shipped asset trips the rule.
    #[test]
    fn disjoint_inputs_and_parameters_shadow_nothing() {
        let facts = facts_of(&[("m", 2.0)], &[("throttle", 0.0)]);
        assert_eq!(key(&facts, "shadowed"), &H::Array(Vec::new()));
    }

    /// Names are carried alongside the full entries so a rule can test
    /// membership without walking maps — the expensive shape in rhai.
    #[test]
    fn names_are_exposed_flat_for_membership_tests() {
        let facts = facts_of(&[("m", 2.0)], &[("throttle", 0.0)]);
        assert_eq!(
            key(&facts, "param_names"),
            &H::Array(vec![H::Str("m".to_string())])
        );
        assert_eq!(
            key(&facts, "input_names"),
            &H::Array(vec![H::Str("throttle".to_string())])
        );
    }

    /// THE DEFECT: an algebraic observable defined by a conditional expression.
    /// This is the RocketEngine `m_dot` shape, and it reads 0 at runtime.
    #[test]
    fn an_if_guarded_algebraic_is_named_with_its_variable() {
        let facts = facts_of_source(
            "model M\n  Real m_dot;\n  Real throttle;\nequation\n  m_dot = if throttle > 0.01 then 2.0 * throttle else 0.0;\n  throttle = 1.0;\nend M;\n",
        );
        assert_eq!(
            conditionals(&facts),
            vec![(
                "m_dot".to_string(),
                "algebraic".to_string(),
                "if-expression".to_string()
            )]
        );
        assert_eq!(
            key(&facts, "conditional_algebraic_names"),
            &H::Array(vec![H::Str("m_dot".to_string())])
        );
    }

    /// A conditional nested inside arithmetic is the same runtime lie, so the
    /// walk must not stop at the top of the right-hand side.
    #[test]
    fn a_conditional_nested_in_arithmetic_is_still_found() {
        let facts = facts_of_source(
            "model M\n  Real f;\n  Real x;\nequation\n  f = 1.0 + (if x > 0.0 then x else 0.0);\n  x = 2.0;\nend M;\n",
        );
        assert_eq!(
            key(&facts, "conditional_algebraic_names"),
            &H::Array(vec![H::Str("f".to_string())])
        );
    }

    /// A `when` clause is a discrete event, NOT the silent-zero defect. It must
    /// be reported as its own form and must stay OUT of the algebraic set, or
    /// the rule shouts at authors who did nothing wrong for this reason.
    #[test]
    fn a_when_clause_is_reported_as_a_distinct_form() {
        let facts = facts_of_source(
            "model M\n  Real x;\n  discrete Real y;\nequation\n  der(x) = 1.0;\n  when x > 1.0 then\n    y = x;\n  end when;\nend M;\n",
        );
        let forms: Vec<String> = conditionals(&facts).into_iter().map(|c| c.2).collect();
        assert_eq!(forms, vec!["when-equation".to_string()], "{facts:?}");
        assert_eq!(
            key(&facts, "conditional_algebraic_names"),
            &H::Array(Vec::new())
        );
    }

    /// A branch-free model — the form every shipped `.mo` is expected to be in —
    /// reports nothing at all.
    #[test]
    fn a_branch_free_model_reports_no_conditionals() {
        let facts = facts_of_source(
            "model M\n  Real f;\n  Real x;\nequation\n  f = max(0.0, x);\n  der(x) = -x;\nend M;\n",
        );
        assert_eq!(key(&facts, "conditional_equations"), &H::Array(Vec::new()));
        assert_eq!(
            key(&facts, "conditional_algebraic_names"),
            &H::Array(Vec::new())
        );
    }

    /// A model that never parsed still produces the keys, empty. A missing key is
    /// an error inside every rule that reads it; an empty array is a fact.
    #[test]
    fn the_equation_keys_exist_even_with_no_ast() {
        let facts = facts_of(&[], &[]);
        assert_eq!(key(&facts, "conditional_equations"), &H::Array(Vec::new()));
        assert_eq!(
            key(&facts, "conditional_algebraic_names"),
            &H::Array(Vec::new())
        );
    }
}
