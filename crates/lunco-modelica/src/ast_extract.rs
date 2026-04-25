//! AST-based extraction functions for Modelica source code.
//!
//! Replaces regex-based extraction by walking the full Modelica AST produced by
//! `rumoca_phase_parse::parse_to_ast`. All functions accept raw source text so
//! they can be used as drop-in replacements for the legacy regex functions.
//!
//! ## Design Notes
//!
//! - **All types**: Unlike regex which only handled `Real`, these functions work
//!   with any component type (Real, Integer, Boolean, String, custom types).
//! - **Full class coverage**: Walks all top-level and nested classes, not just
//!   the first `model|class|block|package` declaration.
//! - **Expression-aware**: Extracts numeric values from AST expressions, not
//!   just regex-captured number literals.

use rumoca_phase_parse::parse_to_ast;
use rumoca_session::parsing::ast::{
    Causality, ClassDef, Expression, StoredDefinition, TerminalType, Variability,
};
use rumoca_session::parsing::ClassType;
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Parsing entry point
// ---------------------------------------------------------------------------

/// Parse Modelica source code into a `StoredDefinition` AST.
///
/// Returns `None` on parse failure. Use [`extract_from_source`] for the
/// high-level API that extracts all symbols in one pass.
fn parse(source: &str) -> Option<StoredDefinition> {
    parse_to_ast(source, "model.mo").ok()
}

// ---------------------------------------------------------------------------
// Public extraction functions (drop-in replacements for regex versions)
// ---------------------------------------------------------------------------

/// Extract the model name from Modelica source code.
///
/// Returns the name of the first non-package class found (model, block, class,
/// connector, function, etc.). Package-level names are only returned if no
/// other class exists.
///
/// This is a drop-in replacement for the regex-based `extract_model_name`.
pub fn extract_model_name(source: &str) -> Option<String> {
    let ast = parse(source)?;
    extract_model_name_from_ast(&ast)
}

/// AST-based variant. Callers that already have a parsed
/// `StoredDefinition` (the document registry caches one per doc)
/// MUST use this path — calling [`extract_model_name`] from the
/// main thread on a 184 KB MSL source means a fresh uncached
/// rumoca parse that runs for tens of seconds in debug builds and
/// visibly freezes the app.
///
/// Returns a fully qualified class name (e.g.
/// `"AnnotatedRocketStage.RocketStage"`) when the non-package class
/// lives nested inside a package. Returns just the short name for
/// top-level non-package classes. This matters because when the
/// user clicks Compile without drilling into a specific class and
/// the file is package-scoped (e.g. `package Foo { model Bar ... }`),
/// rumoca needs the qualified `Foo.Bar` to locate the instantiable
/// class — passing just `"Foo"` makes it compile the empty package.
pub fn extract_model_name_from_ast(ast: &StoredDefinition) -> Option<String> {
    find_first_non_package_qualified(&ast.classes, "")
}

/// Return ALL non-package classes (qualified) reachable from the
/// top-level classes, depth-first. Used by the Compile handler to
/// decide whether to auto-pick (length 0–1) or open a picker modal
/// (length ≥ 2, task #102). Cheap — walks the already-parsed AST.
pub fn collect_non_package_classes_qualified(
    ast: &StoredDefinition,
) -> Vec<String> {
    let mut out = Vec::new();
    collect_non_package_qualified(&ast.classes, "", &mut out);
    out
}

fn collect_non_package_qualified(
    classes: &indexmap::IndexMap<String, ClassDef>,
    parent: &str,
    out: &mut Vec<String>,
) {
    for (name, class) in classes {
        let qualified = if parent.is_empty() {
            name.clone()
        } else {
            format!("{parent}.{name}")
        };
        match class.class_type {
            // Descend into packages to reach nested runnable classes.
            ClassType::Package => {
                collect_non_package_qualified(&class.classes, &qualified, out);
            }
            // Only runnable classes end up on the compile picker —
            // connectors / records / types / functions have no
            // equations to simulate and would only confuse the user
            // by appearing as "Compile this" candidates.
            ClassType::Model | ClassType::Block | ClassType::Class => {
                out.push(qualified);
            }
            _ => {}
        }
    }
}

/// Depth-first walk of `classes` returning the first non-package
/// class found, qualified by its path inside the surrounding packages.
fn find_first_non_package_qualified(
    classes: &indexmap::IndexMap<String, ClassDef>,
    parent: &str,
) -> Option<String> {
    // Runnable = Model / Block / Class. Skip connectors, records,
    // types, functions — they have no equations to simulate and
    // compile would only produce `EmptySystem` / type errors.
    let is_runnable = |t: &ClassType| {
        matches!(
            t,
            ClassType::Model | ClassType::Block | ClassType::Class
        )
    };
    // First pass: prefer a runnable class AT THIS level.
    for (name, class) in classes {
        if is_runnable(&class.class_type) {
            return Some(if parent.is_empty() {
                name.clone()
            } else {
                format!("{parent}.{name}")
            });
        }
    }
    // Second pass: descend into each package.
    for (name, class) in classes {
        if class.class_type != ClassType::Package {
            continue;
        }
        let next_parent = if parent.is_empty() {
            name.clone()
        } else {
            format!("{parent}.{name}")
        };
        if let Some(found) = find_first_non_package_qualified(&class.classes, &next_parent) {
            return Some(found);
        }
    }
    // Entire subtree is packages-only (or empty). Fall back to the
    // top-level package name so earlier callers that relied on the
    // old "return the package when nothing else exists" behaviour
    // still get something non-empty; compile will likely still fail
    // but at least the error message names the file's top entity.
    classes
        .keys()
        .next()
        .map(|n| if parent.is_empty() { n.to_string() } else { format!("{parent}.{n}") })
}

/// Extract the Modelica description string (`"..."` after a
/// declaration, per MLS §A.2.5) for every component across all classes
/// in the source. Returns a `name → description` map.
///
/// Rumoca's compiled DAE drops component descriptions during
/// compile → DAE lowering (as of rumoca `main` at the time of writing),
/// so we can't read them from `Dae.states[name].description`. The
/// AST-level `Component.description: Vec<Token>` still has them,
/// which is what we walk here.
///
/// Used by the worker to feed hover tooltips in the Telemetry panel,
/// the Diagram icon block, and anywhere else a variable name appears
/// that benefits from inline docs.
pub fn extract_descriptions(source: &str) -> HashMap<String, String> {
    let ast = match parse(source) {
        Some(a) => a,
        None => return HashMap::new(),
    };
    let mut out: HashMap<String, String> = HashMap::new();
    collect_descriptions_from_classes(&ast.classes, &mut out);
    out
}

/// AST-based variant — see `extract_parameters_from_ast`.
pub fn extract_descriptions_from_ast(
    ast: &StoredDefinition,
) -> HashMap<String, String> {
    let mut out: HashMap<String, String> = HashMap::new();
    collect_descriptions_from_classes(&ast.classes, &mut out);
    out
}

fn collect_descriptions_from_classes(
    classes: &indexmap::IndexMap<String, ClassDef>,
    out: &mut HashMap<String, String>,
) {
    for class in classes.values() {
        for component in class.components.values() {
            if component.description.is_empty() {
                continue;
            }
            // Rumoca's AST stores the already-unquoted string-literal
            // text in each Token. Concatenate (Modelica allows
            // adjacent-string concatenation `"a" " b"` → `a b`) and
            // trim. We still keep a quote-strip fallback in case a
            // future rumoca revision includes the quotes verbatim.
            let joined: String = component
                .description
                .iter()
                .map(|t| t.text.as_ref())
                .collect::<Vec<_>>()
                .join(" ");
            let cleaned = if joined.contains('"') {
                dequote_description(&joined)
            } else {
                joined.trim().to_string()
            };
            if !cleaned.is_empty() {
                out.insert(component.name.clone(), cleaned);
            }
        }
        collect_descriptions_from_classes(&class.classes, out);
    }
}

/// Strip bounding `"..."` wrappers from a concatenation of description
/// tokens, e.g. `"a" " b"` → `a b`. Used as a fallback when a rumoca
/// revision surfaces raw quoted text in the description tokens; the
/// current revision pre-unquotes each literal.
fn dequote_description(raw: &str) -> String {
    let mut out = String::new();
    let mut in_str = false;
    let mut escape = false;
    for ch in raw.chars() {
        if escape {
            out.push(ch);
            escape = false;
            continue;
        }
        match ch {
            '\\' if in_str => escape = true,
            '"' => in_str = !in_str,
            c if in_str => out.push(c),
            _ => {}
        }
    }
    out.trim().to_string()
}

/// Extract parameter values from Modelica source code.
///
/// Finds all components with `parameter` variability across all classes and
/// extracts their binding values. Handles any component type, not just
/// `parameter Real`.
///
/// This is a drop-in replacement for the regex-based `extract_parameters`.
pub fn extract_parameters(source: &str) -> HashMap<String, f64> {
    let ast = match parse(source) {
        Some(a) => a,
        None => return HashMap::new(),
    };

    let mut params = HashMap::new();
    collect_parameters_from_classes(&ast.classes, &mut params);
    params
}

/// AST-based variant — call this from any hot path that already
/// holds a parsed `StoredDefinition`. The `_source` variants above
/// re-parse on every call, which is catastrophic (~minutes) on
/// 150 KB MSL package files; hot paths like `on_compile_model`
/// MUST use these.
pub fn extract_parameters_from_ast(ast: &StoredDefinition) -> HashMap<String, f64> {
    let mut params = HashMap::new();
    collect_parameters_from_classes(&ast.classes, &mut params);
    params
}

/// Extract input variables that have runtime-settable default values.
///
/// Finds all components with `input` causality that have a numeric binding
/// expression. In rumoca, inputs with default bindings (like `input Real g = 9.81`)
/// are compiled as constants in the DAE and cannot be changed at runtime via
/// `set_input()`. This function returns them separately so the UI can treat
/// them as parameters (recompile on change).
///
/// This is a drop-in replacement for the regex-based `extract_inputs_with_defaults`.
pub fn extract_inputs_with_defaults(source: &str) -> HashMap<String, f64> {
    let ast = match parse(source) {
        Some(a) => a,
        None => return HashMap::new(),
    };

    let mut inputs = HashMap::new();
    collect_inputs_with_defaults_from_classes(&ast.classes, &mut inputs);
    inputs
}

/// AST-based variant — see `extract_parameters_from_ast`.
pub fn extract_inputs_with_defaults_from_ast(
    ast: &StoredDefinition,
) -> HashMap<String, f64> {
    let mut inputs = HashMap::new();
    collect_inputs_with_defaults_from_classes(&ast.classes, &mut inputs);
    inputs
}

/// Extract names of all continuous (non-parameter, non-constant, non-input) variables.
///
/// These are the model's state and algebraic variables — everything that would
/// normally appear as a "variable" in the simulation (e.g., `volume`, `netForce`,
/// `buoyancy`). We need this because rumoca's `SimStepper::variable_names()`
/// returns only solver-state entries (often just states after DAE reduction),
/// omitting algebraics that were eliminated by substitution. Querying each
/// extracted name via `SimStepper::get()` recovers the algebraic values.
pub fn extract_variable_names(source: &str) -> Vec<String> {
    let ast = match parse(source) {
        Some(a) => a,
        None => return Vec::new(),
    };

    let mut names = Vec::new();
    collect_variable_names_from_classes(&ast.classes, &mut names);
    names
}

/// AST-based variant — see `extract_parameters_from_ast`.
pub fn extract_variable_names_from_ast(ast: &StoredDefinition) -> Vec<String> {
    let mut names = Vec::new();
    collect_variable_names_from_classes(&ast.classes, &mut names);
    names
}

/// Extract input variable names **without** default values.
///
/// These are true runtime-settable slots that can be changed via `set_input()`
/// without recompilation.
///
/// Only returns inputs **without** binding expressions. Inputs with defaults
/// like `input Real g = 9.81` are treated as parameters by the Modelica
/// compiler (they become constants in the DAE, not runtime-settable slots).
///
/// This is a drop-in replacement for the regex-based `extract_input_names`.
pub fn extract_input_names(source: &str) -> Vec<String> {
    let ast = match parse(source) {
        Some(a) => a,
        None => return Vec::new(),
    };

    let mut inputs = Vec::new();
    collect_input_names_from_classes(&ast.classes, &mut inputs);
    inputs
}

/// AST-based variant — see `extract_parameters_from_ast`.
pub fn extract_input_names_from_ast(ast: &StoredDefinition) -> Vec<String> {
    let mut inputs = Vec::new();
    collect_input_names_from_classes(&ast.classes, &mut inputs);
    inputs
}

/// Strip default values from `input` declarations in source code.
///
/// Rumoca compiles `input Real g = 9.81` as a constant (not a runtime slot).
/// By stripping the default, the input becomes a true runtime slot that can be
/// changed via `set_input()`. The original default values are returned so the UI
/// can initialize the input correctly.
///
/// Returns `(modified_source, defaults_map)` where `modified_source` has all
/// `= value` removed from input declarations and `defaults_map` contains the
/// extracted numeric defaults.
///
/// This is a drop-in replacement for the regex-based `strip_input_defaults`.
pub fn strip_input_defaults(source: &str) -> (String, HashMap<String, f64>) {
    let ast = match parse(source) {
        Some(a) => a,
        None => return (source.to_string(), HashMap::new()),
    };

    let mut defaults = HashMap::new();
    collect_inputs_with_defaults_from_classes(&ast.classes, &mut defaults);

    // Rebuild source with input defaults stripped. The regex has to
    // cope with modifications between the name and the binding —
    // `input Real throttle(min=0, max=100, unit="%") = 100;` is
    // common once users add bounds, and the old pattern skipped such
    // declarations, so rumoca baked `throttle` in as a constant and
    // `set_input()` silently no-op'd. Capture groups:
    //   1: everything up to the component name (`input Type `), with
    //      dotted types like `Modelica.Blocks.Interfaces.RealInput`
    //   2: the component name
    //   3: optional `(…mods…)` — preserved
    //   4: the `= literal` — dropped
    let re = regex::Regex::new(
        r"(?m)(^\s*input\s+[\w\.]+\s+)([A-Za-z_][A-Za-z0-9_]*)(\s*\([^)]*\))?(\s*=\s*[-+]?[0-9]*\.?[0-9]+(?:[eE][-+]?[0-9]+)?)",
    )
    .unwrap();

    let modified = re
        .replace_all(source, |caps: &regex::Captures| {
            let prefix = &caps[1];
            let name = &caps[2];
            let mods = caps.get(3).map(|m| m.as_str()).unwrap_or("");
            format!("{prefix}{name}{mods}")
        })
        .to_string();

    (modified, defaults)
}

/// Substitute parameter values into Modelica source code.
///
/// Replaces `parameter <type> <name> = <value>` lines with the given values,
/// enabling recompilation with different parameter values.
///
/// This is a drop-in replacement for the regex-based `substitute_params_in_source`.
pub fn substitute_params_in_source(source: &str, parameters: &HashMap<String, f64>) -> String {
    let mut modified = source.to_string();
    for (name, value) in parameters {
        let pattern = format!(
            r"(?m)(^\s*parameter\s+\w+\s+{}\s*=\s*)[-+]?[0-9]*\.?[0-9]+([eE][-+]?[0-9]+)?",
            regex::escape(name)
        );
        if let Ok(re) = regex::Regex::new(&pattern) {
            modified = re.replace_all(&modified, format!("${{1}}{}", value)).to_string();
        }
    }
    modified
}

// ---------------------------------------------------------------------------
// Combined extraction (single-pass, future-facing)
// ---------------------------------------------------------------------------

/// Extract all symbols from Modelica source in a single parse pass.
///
/// This is the preferred API for new code. It avoids re-parsing the source
/// multiple times (unlike calling each `extract_*` function separately).
pub fn extract_from_source(source: &str) -> ModelicaSymbols {
    let mut result = ModelicaSymbols::default();
    let ast = match parse(source) {
        Some(a) => a,
        None => return result,
    };

    extract_from_ast(&ast, &mut result);
    result
}

/// All extractable symbols from a Modelica source file.
#[derive(Debug, Default, Clone)]
pub struct ModelicaSymbols {
    /// Top-level class name (first non-package class found).
    pub model_name: Option<String>,
    /// Parameters with numeric binding values.
    pub parameters: HashMap<String, f64>,
    /// Input names without defaults (true runtime-settable slots).
    pub input_names: Vec<String>,
    /// Input names with defaults (require recompile to change).
    pub inputs_with_defaults: HashMap<String, f64>,
}

/// Extract all symbols from a parsed AST into the given result struct.
///
/// Use this when you already have a `StoredDefinition` (e.g., from a cached
/// parse) and want to avoid re-parsing.
pub fn extract_from_ast(ast: &StoredDefinition, result: &mut ModelicaSymbols) {
    // Model name: first non-package class
    for (name, class) in &ast.classes {
        if class.class_type != ClassType::Package {
            result.model_name = Some(name.as_str().to_string());
            break;
        }
    }

    // Walk all classes
    for class in ast.classes.values() {
        walk_class(class, result);
    }
}

// ---------------------------------------------------------------------------
// Internal AST walkers
// ---------------------------------------------------------------------------

fn walk_class(class: &ClassDef, result: &mut ModelicaSymbols) {
    for component in class.components.values() {
        // Parameters
        if matches!(component.variability, Variability::Parameter(_)) {
            if let Some(value) = extract_numeric_binding(&component.binding) {
                result.parameters.insert(component.name.clone(), value);
            }
        }

        // Inputs
        if matches!(component.causality, Causality::Input(_)) {
            if let Some(value) = extract_numeric_binding(&component.binding) {
                result.inputs_with_defaults.insert(component.name.clone(), value);
            } else {
                result.input_names.push(component.name.clone());
            }
        }
    }

    // Recurse into nested classes
    for nested in class.classes.values() {
        walk_class(nested, result);
    }
}

fn collect_parameters_from_classes(
    classes: &indexmap::IndexMap<String, ClassDef>,
    params: &mut HashMap<String, f64>,
) {
    for class in classes.values() {
        for component in class.components.values() {
            if matches!(component.variability, Variability::Parameter(_)) {
                if let Some(value) = extract_numeric_binding(&component.binding) {
                    params.insert(component.name.clone(), value);
                }
            }
        }
        collect_parameters_from_classes(&class.classes, params);
    }
}

fn collect_inputs_with_defaults_from_classes(
    classes: &indexmap::IndexMap<String, ClassDef>,
    inputs: &mut HashMap<String, f64>,
) {
    for class in classes.values() {
        for component in class.components.values() {
            if matches!(component.causality, Causality::Input(_)) {
                if let Some(value) = extract_numeric_binding(&component.binding) {
                    inputs.insert(component.name.clone(), value);
                }
            }
        }
        collect_inputs_with_defaults_from_classes(&class.classes, inputs);
    }
}

fn collect_variable_names_from_classes(
    classes: &indexmap::IndexMap<String, ClassDef>,
    names: &mut Vec<String>,
) {
    for class in classes.values() {
        for component in class.components.values() {
            let is_parameter = matches!(component.variability, Variability::Parameter(_));
            let is_constant = matches!(component.variability, Variability::Constant(_));
            let is_input = matches!(component.causality, Causality::Input(_));
            if !is_parameter && !is_constant && !is_input {
                names.push(component.name.clone());
            }
        }
        collect_variable_names_from_classes(&class.classes, names);
    }
}

fn collect_input_names_from_classes(
    classes: &indexmap::IndexMap<String, ClassDef>,
    inputs: &mut Vec<String>,
) {
    for class in classes.values() {
        for component in class.components.values() {
            if matches!(component.causality, Causality::Input(_)) {
                if extract_numeric_binding(&component.binding).is_none() {
                    inputs.push(component.name.clone());
                }
            }
        }
        collect_input_names_from_classes(&class.classes, inputs);
    }
}

/// Try to extract a numeric `f64` value from a binding expression.
///
/// Handles `Expression::Terminal` with Real, Integer, or unsigned numeric types.
/// Returns `None` for non-numeric bindings (strings, booleans, references, etc.).
fn extract_numeric_binding(expr: &Option<Expression>) -> Option<f64> {
    let expr = expr.as_ref()?;
    numeric_of(expr)
}

/// Parse a numeric literal expression (including a leading `-` unary
/// minus — rumoca represents `-5` as `Unary(Minus, 5)`). Used for
/// `min`/`max` modifier extraction where negative bounds are common.
fn numeric_of(expr: &Expression) -> Option<f64> {
    use rumoca_session::parsing::ast::OpUnary;
    match expr {
        Expression::Terminal { terminal_type, token } => match terminal_type {
            TerminalType::UnsignedReal | TerminalType::UnsignedInteger => {
                token.text.parse::<f64>().ok()
            }
            _ => None,
        },
        Expression::Unary { op, rhs } if matches!(op, OpUnary::Minus(_)) => {
            numeric_of(rhs).map(|v| -v)
        }
        _ => None,
    }
}

/// Parameter bounds (min, max) pulled from `parameter Real x(min=…,
/// max=…)` modifiers. Either end is `None` if not declared. Used by
/// the Telemetry panel to clamp DragValues to the authored operating
/// envelope — MLS §4.8 says the UI SHOULD respect these bounds.
pub fn extract_parameter_bounds_from_ast(
    ast: &StoredDefinition,
) -> HashMap<String, (Option<f64>, Option<f64>)> {
    let mut bounds = HashMap::new();
    collect_parameter_bounds_from_classes(&ast.classes, &mut bounds);
    bounds
}

fn collect_parameter_bounds_from_classes(
    classes: &indexmap::IndexMap<String, ClassDef>,
    out: &mut HashMap<String, (Option<f64>, Option<f64>)>,
) {
    // Extract bounds from EVERY component that declares `min`/`max`
    // modifications, regardless of variability or causality.
    //
    // The previous implementation gated on
    //   `Variability::Parameter(_) || Causality::Input(_)`
    // — which silently dropped a very common case: inputs typed via
    // a connector class, e.g. `Modelica.Blocks.Interfaces.RealInput`.
    // The `input` keyword lives inside the connector definition
    // (`connector RealInput = input Real`), so the AST shows the
    // component's own causality as `Empty` and the gate rejected it.
    // Result: `RealInput x(min=0, max=1)` had its bounds invisible
    // to Telemetry and the UI clamping silently no-op'd.
    //
    // Filtering is done on the lookup side instead: Telemetry only
    // queries this map for displayed parameter / input rows, so
    // bounds attached to algebraics or outputs (rare but legal —
    // they document runtime envelopes for assert checks) cause no
    // visible UI behaviour.
    for class in classes.values() {
        for component in class.components.values() {
            let min = component.modifications.get("min").and_then(numeric_of);
            let max = component.modifications.get("max").and_then(numeric_of);
            if min.is_some() || max.is_some() {
                out.insert(component.name.clone(), (min, max));
            }
        }
        collect_parameter_bounds_from_classes(&class.classes, out);
    }
}

// ---------------------------------------------------------------------------
// Structural extractors (spec 033 P1 follow-up — describe_model coverage)
// ---------------------------------------------------------------------------
//
// These walk a *specific* class in the AST rather than merging across all
// classes the way the simulator-tuning extractors do. The agent decides
// which class via the `class` parameter on `describe_model`; without this
// per-class scoping a multi-class doc like AnnotatedRocketStage would
// merge `RocketStage`'s components with `Tank`'s and `Engine`'s into one
// nonsensical pile.

/// Find a class by short name, walking nested classes too.
///
/// Many MSL packages and user-authored multi-class files (e.g.
/// `AnnotatedRocketStage` which wraps `RocketStage`/`Tank`/`Valve`/…
/// inside a `package AnnotatedRocketStage`) expose simulatable classes
/// only inside a wrapper package. A top-level-only lookup misses them
/// and breaks `describe_model` even when `compile_model` (which uses
/// `collect_non_package_classes_qualified`) succeeds. Recursing here
/// keeps the two views consistent.
///
/// Returns the first match in iteration order — duplicate short names
/// across nested levels are resolved by the outer-most occurrence.
pub fn find_class_by_short_name<'a>(
    ast: &'a StoredDefinition,
    short_name: &str,
) -> Option<&'a ClassDef> {
    find_in_classes(&ast.classes, short_name)
}

fn find_in_classes<'a>(
    classes: &'a indexmap::IndexMap<String, ClassDef>,
    short_name: &str,
) -> Option<&'a ClassDef> {
    if let Some((_, class)) = classes.iter().find(|(name, _)| name.as_str() == short_name) {
        return Some(class);
    }
    for class in classes.values() {
        if let Some(found) = find_in_classes(&class.classes, short_name) {
            return Some(found);
        }
    }
    None
}

/// Lower-case Modelica class kind keyword: `model`, `block`, `connector`,
/// `package`, `function`, `record`, `type`, `class`, `operator`. The same
/// taxonomy the canvas's class-kind badge surfaces, kept consistent so
/// the agent and the GUI agree.
pub fn class_kind_label(class: &ClassDef) -> &'static str {
    match class.class_type {
        ClassType::Model => "model",
        ClassType::Block => "block",
        ClassType::Connector => "connector",
        ClassType::Package => "package",
        ClassType::Function => "function",
        ClassType::Record => "record",
        ClassType::Type => "type",
        ClassType::Class => "class",
        ClassType::Operator => "operator",
    }
}

/// `extends` base type names for a class, in declaration order.
/// Resolved enough for the agent to traverse the inheritance graph by
/// re-querying `describe_model` on each base — full transitive closure
/// is the agent's responsibility, not this single call's.
pub fn extract_extends_for_class(class: &ClassDef) -> Vec<String> {
    class
        .extends
        .iter()
        .map(|e| e.base_name.to_string())
        .collect()
}

/// Sub-component declarations of a class — the diagram boxes.
/// Returns one entry per `Tank tank;`, `Valve valve;`, etc. found in
/// the class body. Excludes inherited components (those live behind
/// `extends`); the agent walks `extends` itself if it wants the full
/// flattened picture, matching MLS §5.3 semantics.
///
/// Each entry carries the component's instance name, declared type,
/// description string, and the literal modification map (`R=10`,
/// `unit="kg"`, …) projected to strings.
#[derive(Debug, Clone)]
pub struct ComponentInfo {
    pub name: String,
    pub type_name: String,
    pub description: String,
    pub modifications: HashMap<String, String>,
}

pub fn extract_components_for_class(class: &ClassDef) -> Vec<ComponentInfo> {
    class
        .components
        .values()
        .map(|c| ComponentInfo {
            name: c.name.clone(),
            type_name: c.type_name.to_string(),
            description: tokens_to_description(&c.description),
            modifications: c
                .modifications
                .iter()
                .map(|(k, v)| (k.clone(), expression_to_string(v)))
                .collect(),
        })
        .collect()
}

/// Connect-equations of a class. Returns `(from, to)` pairs as
/// dot-paths (e.g. `("tank.outlet", "valve.inlet")`). Non-connect
/// equations (algebraic, when, if, …) are intentionally not surfaced
/// here — the agent's structural picture is the wiring, not the
/// constitutive equations.
pub fn extract_connections_for_class(
    class: &ClassDef,
) -> Vec<(String, String)> {
    use rumoca_session::parsing::ast::Equation;
    class
        .equations
        .iter()
        .filter_map(|e| match e {
            Equation::Connect { lhs, rhs } => {
                Some((lhs.to_string(), rhs.to_string()))
            }
            _ => None,
        })
        .collect()
}

/// Collapse a description token sequence (Modelica string literal) to
/// a single trimmed string. Strips surrounding quotes — the AST keeps
/// them in the lexed token but the agent wants the value, not the
/// quoting.
fn tokens_to_description(tokens: &[rumoca_session::parsing::Token]) -> String {
    let raw = tokens
        .iter()
        .map(|t| t.text.as_ref())
        .collect::<Vec<_>>()
        .join(" ");
    let trimmed = raw.trim();
    trimmed
        .trim_start_matches('"')
        .trim_end_matches('"')
        .to_string()
}

/// Cheap stringification of an Expression for the modifications map.
/// Numeric and string literals round-trip exactly; complex expressions
/// fall back to a placeholder so the agent does not see a truncated
/// half-rendering. `describe_model` is best-effort surface for
/// authoring intent — for full fidelity the agent reads
/// `get_document_source`.
fn expression_to_string(expr: &Expression) -> String {
    match expr {
        Expression::Terminal { terminal_type, token } => match terminal_type {
            TerminalType::String => token.text.trim_matches('"').to_string(),
            _ => token.text.to_string(),
        },
        Expression::ComponentReference(cref) => cref.to_string(),
        _ => "<expr>".into(),
    }
}

/// Pull the `unit="..."` modification for a component, if any. Returns
/// the inner string with quotes stripped.
pub fn unit_of_component(comp: &rumoca_session::parsing::ast::Component) -> Option<String> {
    comp.modifications
        .get("unit")
        .and_then(|expr| match expr {
            Expression::Terminal { terminal_type: TerminalType::String, token } => {
                Some(token.text.trim_matches('"').to_string())
            }
            _ => None,
        })
}

/// Extract every input-typed component for a class with rich metadata
/// (name, type, unit, default if any, description). Companion to the
/// existing `extract_input_names_from_ast` which only returns names.
#[derive(Debug, Clone)]
pub struct TypedComponent {
    pub name: String,
    pub type_name: String,
    pub unit: Option<String>,
    pub default: Option<f64>,
    pub description: String,
    pub min: Option<f64>,
    pub max: Option<f64>,
}

pub fn extract_typed_inputs_for_class(class: &ClassDef) -> Vec<TypedComponent> {
    typed_components_filtered(class, |c| {
        matches!(c.causality, Causality::Input(_))
            || is_input_connector_type(&c.type_name.to_string())
    })
}

pub fn extract_typed_parameters_for_class(class: &ClassDef) -> Vec<TypedComponent> {
    typed_components_filtered(class, |c| {
        matches!(c.variability, Variability::Parameter(_))
    })
}

pub fn extract_typed_outputs_for_class(class: &ClassDef) -> Vec<TypedComponent> {
    typed_components_filtered(class, |c| {
        matches!(c.causality, Causality::Output(_))
            || is_output_connector_type(&c.type_name.to_string())
    })
}

/// Whether `type_name` looks like an MSL "RealInput / IntegerInput /
/// BooleanInput / StringInput" connector class (cf. MLS Annex E.3 +
/// `Modelica.Blocks.Interfaces`). Components declared with these
/// types behave as **inputs** at the API surface even though the
/// `input` keyword lives inside the connector definition rather than
/// on the component itself, so the bare causality check misses them.
///
/// Matches by short-name suffix (`*RealInput`, `*RealInput[N]` for
/// arrays). Returns `true` for the four primitive variants and for
/// any user type that happens to end in `Input` — false-positives
/// here are preferable to the false-negatives (silently missing
/// `valve` on AnnotatedRocketStage etc.).
fn is_input_connector_type(type_name: &str) -> bool {
    // Strip array brackets if any, then split on `.` and inspect the
    // tail. `Modelica.Blocks.Interfaces.RealInput` and bare
    // `RealInput` both resolve to the short name `RealInput`.
    let bare = type_name.split('[').next().unwrap_or(type_name);
    let short = bare.rsplit('.').next().unwrap_or(bare);
    matches!(
        short,
        "RealInput" | "IntegerInput" | "BooleanInput" | "StringInput"
    ) || short.ends_with("Input")
}

/// Symmetric counterpart of [`is_input_connector_type`] for output
/// connectors — see that doc for the rationale.
fn is_output_connector_type(type_name: &str) -> bool {
    let bare = type_name.split('[').next().unwrap_or(type_name);
    let short = bare.rsplit('.').next().unwrap_or(bare);
    matches!(
        short,
        "RealOutput" | "IntegerOutput" | "BooleanOutput" | "StringOutput"
    ) || short.ends_with("Output")
}

fn typed_components_filtered<F>(class: &ClassDef, want: F) -> Vec<TypedComponent>
where
    F: Fn(&rumoca_session::parsing::ast::Component) -> bool,
{
    class
        .components
        .values()
        .filter(|c| want(c))
        .map(|c| TypedComponent {
            name: c.name.clone(),
            type_name: c.type_name.to_string(),
            unit: unit_of_component(c),
            default: c
                .binding
                .as_ref()
                .and_then(numeric_of)
                .or_else(|| numeric_of(&c.start)),
            description: tokens_to_description(&c.description),
            min: c.modifications.get("min").and_then(numeric_of),
            max: c.modifications.get("max").and_then(numeric_of),
        })
        .collect()
}

/// Compute a simple hash of the source content for change detection.
pub fn hash_content(source: &str) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut s = DefaultHasher::new();
    source.hash(&mut s);
    s.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- extract_model_name ---

    #[test]
    fn test_extract_model_name_nested_in_package_returns_qualified() {
        // Regression: user opened assets/models/AnnotatedRocketStage.mo
        // (a package containing `model RocketStage`, `model Engine`, …)
        // and hit Compile without drilling in first. Old extractor
        // returned just `"AnnotatedRocketStage"` (the package) → rumoca
        // compiled the empty package → error. The fallback must
        // descend into packages and qualify the model name so rumoca
        // can resolve it.
        let source = r#"
package AnnotatedRocketStage
  model RocketStage
    Real x;
  end RocketStage;
  model Engine
    Real y;
  end Engine;
end AnnotatedRocketStage;
"#;
        assert_eq!(
            extract_model_name(source),
            Some("AnnotatedRocketStage.RocketStage".to_string())
        );
    }

    #[test]
    fn test_extract_model_name_nested_two_levels_deep() {
        let source = r#"
package Outer
  package Inner
    model Leaf
      Real x;
    end Leaf;
  end Inner;
end Outer;
"#;
        assert_eq!(
            extract_model_name(source),
            Some("Outer.Inner.Leaf".to_string())
        );
    }

    #[test]
    fn test_extract_model_name_simple_model() {
        let source = r#"
model Ball
  Real x;
  Real v;
equation
  der(x) = v;
  der(v) = -9.81;
end Ball;
"#;
        assert_eq!(extract_model_name(source), Some("Ball".to_string()));
    }

    #[test]
    fn test_extract_model_name_block() {
        let source = r#"
block FirstOrder
  input Real u;
  output Real y;
  parameter Real k = 1.0;
equation
  k * u = y;
end FirstOrder;
"#;
        assert_eq!(extract_model_name(source), Some("FirstOrder".to_string()));
    }

    #[test]
    fn test_extract_model_name_package_fallback() {
        let source = r#"
package MyPackage
  model Inner
    Real x;
  end Inner;
end MyPackage;
"#;
        // Used to return just `"MyPackage"` which made rumoca compile
        // the empty package and error out. New behaviour descends into
        // packages and returns the qualified path of the first model.
        assert_eq!(
            extract_model_name(source),
            Some("MyPackage.Inner".to_string())
        );
    }

    // --- extract_parameters ---

    #[test]
    fn test_extract_parameters_simple() {
        let source = r#"
model SpringMass
  parameter Real k = 100.0;
  parameter Real m = 1.0;
  Real x;
end SpringMass;
"#;
        let params = extract_parameters(source);
        assert_eq!(params.len(), 2);
        assert_eq!(params.get("k"), Some(&100.0));
        assert_eq!(params.get("m"), Some(&1.0));
    }

    #[test]
    fn test_extract_parameters_no_binding() {
        let source = r#"
model Test
  parameter Real k;
end Test;
"#;
        let params = extract_parameters(source);
        // Parameter without binding value should not appear (no numeric value)
        assert!(params.is_empty());
    }

    // --- extract_input_names ---

    #[test]
    fn test_extract_input_names_no_defaults() {
        let source = r#"
model Test
  input Real u;
  output Real y;
equation
  y = u;
end Test;
"#;
        let inputs = extract_input_names(source);
        assert_eq!(inputs, vec!["u"]);
    }

    #[test]
    fn test_extract_input_names_excludes_with_defaults() {
        let source = r#"
model Test
  input Real u = 1.0;
  output Real y;
equation
  y = u;
end Test;
"#;
        let inputs = extract_input_names(source);
        // Should NOT include `u` because it has a default
        assert!(inputs.is_empty());
    }

    // --- extract_inputs_with_defaults ---

    #[test]
    fn test_extract_inputs_with_defaults() {
        let source = r#"
model Test
  input Real g = 9.81;
  output Real y;
equation
  y = g;
end Test;
"#;
        let inputs = extract_inputs_with_defaults(source);
        assert_eq!(inputs.len(), 1);
        assert_eq!(inputs.get("g"), Some(&9.81));
    }

    // --- strip_input_defaults ---

    #[test]
    fn test_strip_input_defaults() {
        let source = r#"
model Test
  input Real g = 9.81;
  input Real u;
end Test;
"#;
        let (modified, defaults) = strip_input_defaults(source);
        assert_eq!(defaults.len(), 1);
        assert_eq!(defaults.get("g"), Some(&9.81));
        assert!(modified.contains("input Real g"));
        assert!(!modified.contains("input Real g = 9.81"));
        assert!(modified.contains("input Real u"));
    }

    // --- extract_variable_names ---

    #[test]
    fn test_extract_variable_names_balloon() {
        // A minimal balloon-like model mirroring the real one's variable declarations.
        let source = r#"
model Balloon
  parameter Real g = 9.81;
  parameter Real mass = 4.5;
  parameter Real initVolume = 4.0;
  input Real height = 0;
  input Real velocity = 0;
  Real volume(start = initVolume);
  Real temperature;
  Real airDensity;
  Real buoyancy;
  Real weight;
  Real drag;
  Real netForce;
equation
  temperature = 288.15 - 0.0065 * height;
  airDensity = 1.225;
  volume = initVolume;
  buoyancy = airDensity * volume * g;
  weight = mass * g;
  drag = 0.0;
  netForce = buoyancy - weight - drag;
end Balloon;
"#;
        let mut names = extract_variable_names(source);
        names.sort();
        // Parameters (g, mass, initVolume) and inputs (height, velocity) must NOT appear.
        // All declared Real variables should appear.
        let mut expected = vec![
            "airDensity".to_string(),
            "buoyancy".to_string(),
            "drag".to_string(),
            "netForce".to_string(),
            "temperature".to_string(),
            "volume".to_string(),
            "weight".to_string(),
        ];
        expected.sort();
        assert_eq!(names, expected, "expected all 7 continuous variables to be extracted");
    }

    // --- extract_from_source (single-pass) ---

    #[test]
    fn test_extract_from_source_rc_circuit() {
        let source = r#"
model RC_Circuit
  parameter Real R = 100.0;
  parameter Real C = 1e-3;
  input Real V = 5.0;
  output Real Vc;
  Real i;
equation
  V = R * i + Vc;
  C * der(Vc) = i;
end RC_Circuit;
"#;
        let symbols = extract_from_source(source);
        assert_eq!(symbols.model_name, Some("RC_Circuit".to_string()));
        assert_eq!(symbols.parameters.len(), 2);
        assert_eq!(symbols.parameters.get("R"), Some(&100.0));
        assert_eq!(symbols.parameters.get("C"), Some(&0.001));
        assert_eq!(symbols.inputs_with_defaults.get("V"), Some(&5.0));
        assert!(symbols.input_names.is_empty()); // all inputs have defaults
    }

    #[test]
    fn test_extract_from_source_with_runtime_input() {
        let source = r#"
model Test
  parameter Real k = 2.0;
  input Real u;
  output Real y;
equation
  y = k * u;
end Test;
"#;
        let symbols = extract_from_source(source);
        assert_eq!(symbols.model_name, Some("Test".to_string()));
        assert_eq!(symbols.parameters.get("k"), Some(&2.0));
        assert_eq!(symbols.input_names, vec!["u"]);
        assert!(symbols.inputs_with_defaults.is_empty());
    }

    // --- substitute_params_in_source (still regex-based, TODO: AST regen) ---

    #[test]
    fn test_substitute_params_in_source() {
        let source = r#"
model Test
  parameter Real k = 1.0;
  parameter Real tau = 0.5;
end Test;
"#;
        let mut params = HashMap::new();
        params.insert("k".to_string(), 10.0);
        params.insert("tau".to_string(), 2.0);
        let modified = substitute_params_in_source(source, &params);
        assert!(modified.contains("parameter Real k = 10"));
        assert!(modified.contains("parameter Real tau = 2"));
    }

    #[test]
    fn test_extract_bundled_spring_mass() {
        let source = include_str!("../../../assets/models/SpringMass.mo");
        let symbols = extract_from_source(source);
        assert_eq!(symbols.model_name, Some("SpringMass".to_string()));
        assert_eq!(symbols.parameters.len(), 3);
        assert_eq!(symbols.parameters.get("m"), Some(&1.0));
        assert_eq!(symbols.parameters.get("k"), Some(&10.0));
        assert_eq!(symbols.parameters.get("d"), Some(&0.5));
        assert!(symbols.input_names.is_empty());
        assert!(symbols.inputs_with_defaults.is_empty());
    }

    #[test]
    fn test_extract_bundled_battery() {
        let source = include_str!("../../../assets/models/Battery.mo");
        let symbols = extract_from_source(source);
        assert_eq!(symbols.model_name, Some("Battery".to_string()));
        assert_eq!(symbols.parameters.len(), 4);
        assert_eq!(symbols.parameters.get("capacity"), Some(&1.0));
        assert_eq!(symbols.parameters.get("voltage_nom"), Some(&12.0));
        assert_eq!(symbols.parameters.get("R_internal"), Some(&0.01));
        assert_eq!(symbols.parameters.get("T_filter"), Some(&0.1));
        // current_in has no default → runtime-settable slot
        assert_eq!(symbols.input_names, vec!["current_in"]);
        assert!(symbols.inputs_with_defaults.is_empty());
    }

    #[test]
    fn test_extract_bundled_rc_circuit() {
        let source = include_str!("../../../assets/models/RC_Circuit.mo");
        let symbols = extract_from_source(source);
        assert_eq!(symbols.model_name, Some("RC_Circuit".to_string()));
        // RC_Circuit is now a proper schematic with three tunable
        // parameters (V_source, R, C) feeding component modifications.
        assert_eq!(symbols.parameters.len(), 3);
        assert!(symbols.parameters.contains_key("R"));
        assert!(symbols.parameters.contains_key("C"));
        assert!(symbols.parameters.contains_key("V_source"));
    }

    #[test]
    fn test_extract_bundled_bouncy_ball() {
        let source = include_str!("../../../assets/models/BouncyBall.mo");
        let symbols = extract_from_source(source);
        assert_eq!(symbols.model_name, Some("BouncyBall".to_string()));
        // BouncyBall has parameter Real g = 9.81
        assert_eq!(symbols.parameters.get("g"), Some(&9.81));
    }

    // --- hash_content (unchanged, still needed) ---

    #[test]
    fn test_hash_content_deterministic() {
        let source = "model Test end Test;";
        let h1 = hash_content(source);
        let h2 = hash_content(source);
        assert_eq!(h1, h2);
    }
}
