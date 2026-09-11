//! FACTS for the `modelica` lint domain.
//!
//! Rust extracts only facts that require Rumoca's AST; the authored policy in
//! `assets/scripting/policy/lint_modelica.rhai` decides what is worth saying
//! about them. This keeps policy changes reloadable and leaves this package with
//! a small, reusable parse-time fact boundary.
//!
//! These are PARSE-phase facts — names, parameters, declared inputs, and the
//! SHAPE of equation and algorithm sections. A model's variables and its solver do not
//! exist until a compile, so no rule reached from `ValidateAsset` can ask about
//! their values; but the equations themselves are in the AST at parse, and the
//! worst defect this domain has is a shape, not a value:
//!
//! rumoca's solver path is BRANCH-FREE. An algebraic observable defined behind a
//! conditional — `x = if cond then e else 0` — parses, compiles, and then reads
//! literal **0** at runtime because the elimination reconstructor only
//! substitutes continuous expressions. Nothing fails; the observable just lies.
//! That is only lintable if a rule can see which variable a conditional
//! equation defines, so the source facts below preserve exactly that evidence.

use lunco_hooks::HookValue as H;
use rumoca_core::{Causality, Variability};
use rumoca_ir_ast::{ClassDef, Equation, Expression, Statement, StoredDefinition};

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
/// conditional_constructs: [ #{ name: "m_dot", kind: "algebraic",
///                             form: "if-expression", section: "equation",
///                             line: 42 }, … ]
/// ```
///
/// `ast` is the best-effort parse the caller already holds (the recovering
/// parser's `best_effort()`); an empty `StoredDefinition` is legitimate input and
/// simply yields no source facts. Declaration facts are projected from this same
/// AST, so callers cannot accidentally pass a name from one parse and equations
/// from another.
pub fn modelica_facts(ast: &StoredDefinition) -> H {
    let interface = crate::ast_extract::parse_model_interface_from_ast(ast);
    modelica_facts_from_interface(ast, &interface)
}

/// Project lint facts from an interface that the caller already extracted from
/// this AST. This avoids walking the same declarations again when a validator
/// needs both the public `info` snapshot and the policy input.
pub fn modelica_facts_from_interface(
    ast: &StoredDefinition,
    interface: &crate::ast_extract::ModelInterface,
) -> H {
    let param_entries: Vec<H> = sorted_entries(&interface.parameters)
        .into_iter()
        .map(|(name, value)| H::map([("name", H::Str(name.clone())), ("value", H::Float(*value))]))
        .collect();

    let input_entries: Vec<H> = sorted_entries(&interface.inputs)
        .into_iter()
        .map(|(name, default)| {
            H::map([
                ("name", H::Str(name.clone())),
                ("default", H::Float(*default)),
            ])
        })
        .collect();

    let conditionals = conditional_constructs(ast);
    let conditional_entries: Vec<H> = conditionals
        .iter()
        .map(ConditionalConstruct::to_fact)
        .collect();

    H::map([
        (
            "model",
            H::Str(interface.model_name.clone().unwrap_or_default()),
        ),
        ("params", H::Array(param_entries)),
        ("inputs", H::Array(input_entries)),
        ("conditional_constructs", H::Array(conditional_entries)),
    ])
}

fn sorted_entries(values: &std::collections::HashMap<String, f64>) -> Vec<(&String, &f64)> {
    let mut entries: Vec<_> = values.iter().collect();
    entries.sort_by(|(left, _), (right, _)| left.cmp(right));
    entries
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
    /// `if cond then ... end if;` in an algorithm section.
    IfStatement,
    /// `when cond then ... end when;` in an algorithm section.
    WhenStatement,
}

impl CondForm {
    fn as_str(self) -> &'static str {
        match self {
            CondForm::IfExpression => "if-expression",
            CondForm::IfEquation => "if-equation",
            CondForm::WhenEquation => "when-equation",
            CondForm::IfStatement => "if-statement",
            CondForm::WhenStatement => "when-statement",
        }
    }
}

/// What a construct defines, from the component's own declaration. Only
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

struct ConditionalConstruct {
    name: String,
    kind: VarKind,
    form: CondForm,
    section: &'static str,
    line: i64,
}

impl ConditionalConstruct {
    fn to_fact(&self) -> H {
        H::map([
            ("name", H::Str(self.name.clone())),
            ("kind", H::Str(self.kind.as_str().to_string())),
            ("form", H::Str(self.form.as_str().to_string())),
            ("section", H::Str(self.section.to_string())),
            ("line", H::Int(self.line)),
        ])
    }
}

/// Every branch construct in `ast`, with the variable it defines when there is
/// one. The walk covers regular and initial equation/algorithm sections and
/// nested classes. Rust records syntax and declaration facts; the authored
/// policy decides which construct is actionable and how it is reported.
fn conditional_constructs(ast: &StoredDefinition) -> Vec<ConditionalConstruct> {
    let mut out = Vec::new();
    for class in ast.classes.values() {
        collect_from_class(class, &mut out);
    }
    out
}

fn collect_from_class(class: &ClassDef, out: &mut Vec<ConditionalConstruct>) {
    for eq in &class.equations {
        collect_from_equation(eq, class, "equation", true, out);
    }
    for eq in &class.initial_equations {
        collect_from_equation(eq, class, "initial-equation", true, out);
    }
    for algorithm in &class.algorithms {
        for statement in algorithm {
            collect_from_statement(statement, class, "algorithm", true, out);
        }
    }
    for algorithm in &class.initial_algorithms {
        for statement in algorithm {
            collect_from_statement(statement, class, "initial-algorithm", true, out);
        }
    }
    for nested in class.classes.values() {
        collect_from_class(nested, out);
    }
}

fn collect_from_equation(
    eq: &Equation,
    class: &ClassDef,
    section: &'static str,
    allow_expression: bool,
    out: &mut Vec<ConditionalConstruct>,
) {
    let line = eq
        .get_location()
        .map(|l| i64::from(l.start_line))
        .unwrap_or(0);
    match eq {
        Equation::Simple { lhs, rhs } => {
            if !allow_expression || !expression_is_conditional(rhs) {
                return;
            }
            let (name, is_derivative) = defined_variable(lhs);
            let kind = if is_derivative {
                VarKind::State
            } else {
                declared_kind(class, &name)
            };
            out.push(ConditionalConstruct {
                name,
                kind,
                form: CondForm::IfExpression,
                section,
                line,
            });
        }
        Equation::If {
            cond_blocks,
            else_block,
        } => {
            out.push(ConditionalConstruct {
                name: String::new(),
                kind: VarKind::Unknown,
                form: CondForm::IfEquation,
                section,
                line,
            });
            for block in cond_blocks {
                for inner in &block.eqs {
                    collect_nested_equation(inner, class, section, out);
                }
            }
            for inner in else_block.iter().flatten() {
                collect_nested_equation(inner, class, section, out);
            }
        }
        Equation::When(blocks) => {
            out.push(ConditionalConstruct {
                name: String::new(),
                kind: VarKind::Unknown,
                form: CondForm::WhenEquation,
                section,
                line,
            });
            for block in blocks {
                for inner in &block.eqs {
                    collect_nested_equation(inner, class, section, out);
                }
            }
        }
        Equation::For { equations, .. } => {
            for inner in equations {
                collect_from_equation(inner, class, section, allow_expression, out);
            }
        }
        // Connect / FunctionCall / Assert / Empty define nothing an observable
        // is read from.
        _ => {}
    }
}

/// Walk nested equation blocks without reporting their ordinary assignments a
/// second time. The enclosing conditional block is already the authoritative
/// fact for the policy; nested blocks still need their own entries.
fn collect_nested_equation(
    eq: &Equation,
    class: &ClassDef,
    section: &'static str,
    out: &mut Vec<ConditionalConstruct>,
) {
    match eq {
        Equation::If { .. } | Equation::When(_) => {
            collect_from_equation(eq, class, section, false, out)
        }
        Equation::For { equations, .. } => {
            for inner in equations {
                collect_nested_equation(inner, class, section, out);
            }
        }
        _ => {}
    }
}

fn collect_from_statement(
    statement: &Statement,
    class: &ClassDef,
    section: &'static str,
    allow_expression: bool,
    out: &mut Vec<ConditionalConstruct>,
) {
    let line = statement
        .get_location()
        .map(|l| i64::from(l.start_line))
        .unwrap_or(0);
    match statement {
        Statement::Assignment { comp, value } => {
            if allow_expression && expression_is_conditional(value) {
                let name = comp.to_string();
                out.push(ConditionalConstruct {
                    kind: declared_kind(class, &name),
                    name,
                    form: CondForm::IfExpression,
                    section,
                    line,
                });
            }
        }
        Statement::If {
            cond_blocks,
            else_block,
        } => {
            out.push(ConditionalConstruct {
                name: String::new(),
                kind: VarKind::Unknown,
                form: CondForm::IfStatement,
                section,
                line,
            });
            for block in cond_blocks {
                for inner in &block.stmts {
                    collect_nested_statement(inner, class, section, out);
                }
            }
            for inner in else_block.iter().flatten() {
                collect_nested_statement(inner, class, section, out);
            }
        }
        Statement::When(blocks) => {
            out.push(ConditionalConstruct {
                name: String::new(),
                kind: VarKind::Unknown,
                form: CondForm::WhenStatement,
                section,
                line,
            });
            for block in blocks {
                for inner in &block.stmts {
                    collect_nested_statement(inner, class, section, out);
                }
            }
        }
        Statement::For { equations, .. } => {
            for inner in equations {
                collect_from_statement(inner, class, section, allow_expression, out);
            }
        }
        Statement::While(block) => {
            for inner in &block.stmts {
                collect_from_statement(inner, class, section, allow_expression, out);
            }
        }
        _ => {}
    }
}

fn collect_nested_statement(
    statement: &Statement,
    class: &ClassDef,
    section: &'static str,
    out: &mut Vec<ConditionalConstruct>,
) {
    match statement {
        Statement::If { .. } | Statement::When(_) => {
            collect_from_statement(statement, class, section, false, out)
        }
        Statement::For { equations, .. } => {
            for inner in equations {
                collect_nested_statement(inner, class, section, out);
            }
        }
        Statement::While(block) => {
            for inner in &block.stmts {
                collect_nested_statement(inner, class, section, out);
            }
        }
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

/// The variable a construct's left-hand side defines, and whether it was written
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

    /// Facts from real source, through the same recovering parse `validate_asset`
    /// uses — the conditional facts only exist if the AST really carries them.
    fn facts_of_source(src: &str) -> H {
        let syntax = crate::parse_to_syntax(src, "M.mo");
        let ast = syntax.best_effort();
        modelica_facts(ast)
    }

    /// `#{ name, kind, form }` triples, for asserting on shape without pinning
    /// line numbers into every test.
    fn conditionals(facts: &H) -> Vec<(String, String, String)> {
        let H::Array(entries) = key(facts, "conditional_constructs") else {
            panic!("conditional_constructs is an array")
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

    /// Parameters and inputs remain available as independent authored entries;
    /// policy-level comparisons are intentionally performed by Rhai.
    #[test]
    fn declaration_entries_are_exposed_for_policy_rules() {
        let facts = facts_of_source(
            "model M\n  parameter Real m = 2.0;\n  input Real throttle;\nend M;\n",
        );
        assert_eq!(
            key(&facts, "params"),
            &H::Array(vec![H::map([
                ("name", H::Str("m".to_string())),
                ("value", H::Float(2.0)),
            ])])
        );
        assert_eq!(
            key(&facts, "inputs"),
            &H::Array(vec![H::map([
                ("name", H::Str("throttle".to_string())),
                ("default", H::Float(0.0)),
            ])])
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
            key(&facts, "conditional_constructs"),
            &H::Array(vec![H::map([
                ("name", H::Str("m_dot".to_string())),
                ("kind", H::Str("algebraic".to_string())),
                ("form", H::Str("if-expression".to_string())),
                ("section", H::Str("equation".to_string())),
                ("line", H::Int(5)),
            ])])
        );
    }

    /// A conditional nested inside arithmetic is the same runtime lie, so the
    /// walk must not stop at the top of the right-hand side.
    #[test]
    fn a_conditional_nested_in_arithmetic_is_still_found() {
        let facts = facts_of_source(
            "model M\n  Real f;\n  Real x;\nequation\n  f = 1.0 + (if x > 0.0 then x else 0.0);\n  x = 2.0;\nend M;\n",
        );
        assert_eq!(conditionals(&facts)[0].0, "f");
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
        assert!(conditionals(&facts).iter().all(|c| c.2 != "if-expression"));
    }

    #[test]
    fn algorithm_branches_are_projected_without_source_scanning() {
        let facts = facts_of_source(
            "model M\n  Real x;\nalgorithm\n  if x > 0.0 then\n    x := 1.0;\n  else\n    x := 0.0;\n  end if;\nend M;\n",
        );
        let entries = conditionals(&facts);
        assert_eq!(entries, vec![(
            "".to_string(),
            "unknown".to_string(),
            "if-statement".to_string(),
        )]);
        let H::Array(raw) = key(&facts, "conditional_constructs") else {
            panic!("conditional_constructs is an array")
        };
        assert_eq!(key(&raw[0], "section"), &H::Str("algorithm".to_string()));
    }

    /// A branch-free model — the form every shipped `.mo` is expected to be in —
    /// reports nothing at all.
    #[test]
    fn a_branch_free_model_reports_no_conditionals() {
        let facts = facts_of_source(
            "model M\n  Real f;\n  Real x;\nequation\n  f = max(0.0, x);\n  der(x) = -x;\nend M;\n",
        );
        assert_eq!(key(&facts, "conditional_constructs"), &H::Array(Vec::new()));
    }

    /// A model that never parsed still produces the keys, empty. A missing key is
    /// an error inside every rule that reads it; an empty array is a fact.
    #[test]
    fn the_construct_keys_exist_even_with_no_ast() {
        let facts = modelica_facts(&StoredDefinition::default());
        assert_eq!(key(&facts, "conditional_constructs"), &H::Array(Vec::new()));
    }
}
