//! Parsed Modelica source facts for runtime, validation, and authoring.
//!
//! Rumoca owns syntax and class identity. A source interface projects direct
//! declarations of its selected runnable class; source-root discovery covers
//! the complete definition. Checked DAE/FMI facts own inherited declarations,
//! alias causality, evaluated defaults, and live variable metadata.

use rumoca_core::{Causality, ClassType, Variability};
use rumoca_ir_ast::{AstIndexMap, ClassDef, Expression, StoredDefinition, TerminalType};
use std::collections::{BTreeSet, HashMap};

/// True when an expression names a `PlotNode` record in a `__LunCo` annotation.
///
/// Both qualified (`LunCoAnnotations.PlotNode`) and imported bare record names
/// are accepted by Modelica source, so the extractor intentionally checks the
/// final name segment only.
pub fn is_plot_node_record_call(expr: &Expression) -> bool {
    let parts = match expr {
        Expression::FunctionCall { comp, .. } => &comp.parts,
        Expression::ClassModification { target, .. } => &target.parts,
        _ => return false,
    };
    parts.last().map(|part| &*part.ident.text) == Some("PlotNode")
}

// ---------------------------------------------------------------------------
// Parsing entry point
// ---------------------------------------------------------------------------

/// Recover source facts through Rumoca's tolerant syntax projection.
fn parse_recovered(source: &str, file_label: &str) -> StoredDefinition {
    // Interface metadata uses the compiler's tolerant syntax projection.
    rumoca_phase_parse::parse_to_syntax(source, file_label)
        .best_effort()
        .clone()
}

fn parse(source: &str) -> Option<StoredDefinition> {
    let syntax = rumoca_phase_parse::parse_to_syntax(source, "model.mo");
    (!syntax.has_errors()).then(|| syntax.best_effort().clone())
}

// ---------------------------------------------------------------------------
// Public source and AST projections
// ---------------------------------------------------------------------------

/// The declared, solver-independent face of a model: what it is called, what it
/// takes, what it is tuned by.
#[derive(Debug, Default, Clone)]
pub struct ModelInterface {
    /// Selected class, relative to `within` and qualified by package nesting.
    pub model_name: Option<String>,
    /// The file's `within` clause — the package its classes actually live in.
    /// The only authority on what a `.mo` is CALLED from outside it, which is
    /// what a generated model instantiating it has to get right.
    pub within: Option<String>,
    /// Top-level Modelica source roots required by this source, including its
    /// `within` package root when that root is not declared in the same file.
    /// The sorted set is prepared with the interface so runtime admission does
    /// not need to parse the same source on the application schedule.
    pub required_source_roots: BTreeSet<String>,
    /// Direct numeric `parameter` bindings of the selected class.
    pub parameters: HashMap<String, f64>,
    /// Declared input names. The checked DAE owns initialization values;
    /// interface discovery never creates a numerical sample.
    pub inputs: BTreeSet<String>,
    /// Literal numeric declaration bindings projected for source/lint facts.
    /// This is not the solver's scoped default evaluation.
    pub input_defaults: HashMap<String, f64>,
    /// Direct declarations with explicit `output` causality in the selected
    /// class. Checked compiler facts resolve alias and inherited causality.
    pub outputs: std::collections::BTreeSet<String>,
    /// Authored documentation from the selected declarations. Checked FMI
    /// inventory supplies live telemetry metadata.
    pub variable_metadata: HashMap<String, ModelicaVariableMetadata>,
}

/// Documentation authored for one Modelica variable declaration.
///
/// The declaration string and `unit` modifier are Modelica source facts. This
/// projection lets solver adapters expose those facts without importing UI
/// document state into the simulation path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelicaVariableMetadata {
    /// Optional quoted-string description from the declaration.
    pub description: Option<String>,
    /// Optional `unit` modification from the declaration.
    pub unit: Option<String>,
}

/// Read direct variable documentation of the selected runnable class.
///
/// An undocumented declaration produces no entry. Consumers therefore preserve
/// the absence of documentation instead of manufacturing prose from a variable
/// identifier.
pub fn variable_metadata(
    source: &str,
    file_label: &str,
) -> HashMap<String, ModelicaVariableMetadata> {
    let ast = parse_recovered(source, file_label);
    parse_model_interface_from_ast(&ast).variable_metadata
}

fn declaration_metadata(component: &rumoca_ir_ast::Component) -> Option<ModelicaVariableMetadata> {
    let description = description_from_tokens(&component.description);
    let unit = component
        .modifications
        .get("unit")
        .and_then(string_literal_value)
        .filter(|unit| !unit.is_empty());
    (description.is_some() || unit.is_some())
        .then_some(ModelicaVariableMetadata { description, unit })
}

/// Read a model's interface from source, in one lenient parse.
///
/// File-backed assets select their default runnable declaration here.
/// Generated networks use [`model_interface_for_class`] for their validated root.
///
/// Lenient (`best_effort`): a model with a semantic error still yields usable
/// name/parameter/input snapshots, the same recovery `Session::recovered_file_query`
/// gives the engine side.
pub fn parse_model_interface(source: &str, file_label: &str) -> ModelInterface {
    let ast = parse_recovered(source, file_label);
    parse_model_interface_from_ast(&ast)
}

/// Read direct declarations of the selected runnable class from a recovered AST.
/// Library-only definitions have no model interface. Source-root requirements
/// still cover every declaration in the definition.
///
/// Callers that already parsed the source MUST use this variant so name,
/// parameter, input, output, and metadata projections cannot silently drift
/// through separate parses or recovery modes.
pub fn parse_model_interface_from_ast(ast: &StoredDefinition) -> ModelInterface {
    let (model_name, class) = match find_first_runnable_qualified(&ast.classes, "") {
        Some((name, class)) => (Some(name), Some(class)),
        None => (None, None),
    };
    project_model_interface(ast, model_name, class)
}

/// Project one explicitly selected class through Rumoca's qualified AST lookup.
/// Missing targets return no interface and never select another class.
pub fn model_interface_for_class(ast: &StoredDefinition, target: &str) -> Option<ModelInterface> {
    let class = ast.get_class_by_qualified_name(target)?;
    let within = within_package(ast);
    let name = within
        .as_deref()
        .and_then(|within| target.strip_prefix(&format!("{within}.")))
        .unwrap_or(target);
    Some(project_model_interface(
        ast,
        Some(name.to_owned()),
        Some(class),
    ))
}

fn project_model_interface(
    ast: &StoredDefinition,
    model_name: Option<String>,
    class: Option<&ClassDef>,
) -> ModelInterface {
    let mut interface = ModelInterface {
        model_name,
        within: within_package(ast),
        required_source_roots: required_source_roots_from_ast(ast),
        ..Default::default()
    };
    if let Some(class) = class {
        for component in class.components.values() {
            if matches!(component.variability, Variability::Parameter(_))
                && let Some(value) = extract_numeric_binding(&component.binding)
            {
                interface.parameters.insert(component.name.clone(), value);
            }
            match component.causality {
                Causality::Input(_) => {
                    interface.inputs.insert(component.name.clone());
                    if let Some(value) = extract_numeric_binding(&component.binding) {
                        interface
                            .input_defaults
                            .insert(component.name.clone(), value);
                    }
                }
                Causality::Output(_) => {
                    interface.outputs.insert(component.name.clone());
                }
                Causality::Empty => {}
            }
            if let Some(metadata) = declaration_metadata(component) {
                interface
                    .variable_metadata
                    .insert(component.name.clone(), metadata);
            }
        }
    }
    interface
}

/// Extract the model name from Modelica source code.
///
/// Returns the first runnable model, block, or class, qualified by its package
/// nesting. Files containing only packages, connectors, records, types, or
/// functions have no automatic simulation target.
pub fn extract_model_name(source: &str) -> Option<String> {
    let ast = parse(source)?;
    extract_model_name_from_ast(&ast)
}

/// AST-based variant. Callers that already have a parsed
/// `StoredDefinition` (the document registry caches one per doc)
/// MUST use this path — calling [`extract_model_name`] from the
/// main thread on a 184 KB source-library source means a fresh uncached
/// rumoca parse that runs for tens of seconds in debug builds and
/// visibly freezes the app.
///
/// Returns the selected runnable name relative to the document's `within`
/// clause, including its package nesting inside the document.
pub fn extract_model_name_from_ast(ast: &StoredDefinition) -> Option<String> {
    find_first_runnable_qualified(&ast.classes, "").map(|(name, _)| name)
}

/// The package a file's classes belong to, from its `within` clause —
/// `within LunCo.Propulsion;` → `Some("LunCo.Propulsion")`.
///
/// A bare `within;` names the top level and is reported as `None` — it declares
/// membership of no package, which is the same thing as having no clause.
pub fn within_package(ast: &StoredDefinition) -> Option<String> {
    let name = ast.within.as_ref()?.to_string();
    (!name.is_empty()).then_some(name)
}

/// Fully qualified names declared by a source document, including nested
/// classes. This is document metadata, not a model-specific rule: the
/// Twin lint uses it to inspect the authored namespace.
/// Extract every fully qualified class name declared by one source document.
///
/// This is the same recovered AST walk used by the Modelica source-root
/// indexer. Callers that inspect a Twin-wide namespace use it to compare the
/// names that the compiler can actually resolve, rather than comparing file
/// basenames.
pub fn declared_class_names(source: &str, file_label: &str) -> Vec<String> {
    rumoca_phase_parse::parse_to_syntax(source, file_label)
        .best_effort()
        .declared_class_names()
}

/// Join a parent qualified name with a child segment to form a new
/// qualified name. When `parent` is empty, returns `child` alone —
/// **not** `".child"`, which in Modelica (MLS §5.3.2) is a *global*
/// lookup prefix with distinct semantics. Centralised so every
/// "walk-and-emit-qualified-names" callsite handles the empty-parent
/// case the same way.
pub fn qualify(parent: &str, child: &str) -> String {
    if parent.is_empty() {
        child.to_string()
    } else {
        format!("{parent}.{child}")
    }
}

/// Read a string literal's value from Rumoca's parsed AST.
/// Rumoca has already removed source quotes and decoded Modelica escapes.
/// The retained token text is the value, including any literal quotes or
/// backslashes authored inside the string. Non-string expressions have no value.
pub fn string_literal_value(e: &Expression) -> Option<String> {
    match e {
        Expression::Terminal {
            terminal_type: TerminalType::String,
            token,
            ..
        } => Some(token.text.to_string()),
        _ => None,
    }
}

/// Select a runnable class, checking the current level before nested packages.
fn find_first_runnable_qualified<'a>(
    classes: &'a AstIndexMap<String, ClassDef>,
    parent: &str,
) -> Option<(String, &'a ClassDef)> {
    // Runnable = Model / Block / Class. Skip connectors, records,
    // types, functions — they have no equations to simulate and
    // compile would only produce `EmptySystem` / type errors.
    let is_runnable =
        |t: &ClassType| matches!(t, ClassType::Model | ClassType::Block | ClassType::Class);
    // First pass: prefer a runnable class AT THIS level.
    for (name, class) in classes {
        if is_runnable(&class.class_type) {
            return Some((qualify(parent, name), class));
        }
    }
    // Second pass: descend into each package.
    for (name, class) in classes {
        if class.class_type != ClassType::Package {
            continue;
        }
        let next_parent = qualify(parent, name);
        if let Some(found) = find_first_runnable_qualified(&class.classes, &next_parent) {
            return Some(found);
        }
    }
    None
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
/// `min`/`max` modifier extraction where negative bounds are common,
/// and shared with the annotation parser (`annotations::parsing`).
/// Extract a numeric literal or unary-signed numeric literal from an AST
/// expression. Non-numeric expressions are intentionally unresolved.
pub fn numeric_of(expr: &Expression) -> Option<f64> {
    use rumoca_core::OpUnary;
    match expr {
        Expression::Terminal {
            terminal_type: TerminalType::UnsignedReal | TerminalType::UnsignedInteger,
            token,
            ..
        } => token.text.parse::<f64>().ok(),
        Expression::Terminal { .. } => None,
        Expression::Unary {
            op: OpUnary::Minus,
            rhs,
            ..
        } => numeric_of(rhs).map(|v| -v),
        _ => None,
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

/// Visit every type-name reference reachable from `class`, recursing
/// into nested classes. Emits each `extends` base name and each
/// component `type_name` raw — **no filtering**. Callers apply their
/// own predicate (built-in vs not, qualified-only, etc.).
///
/// The icon warmer and source-root scanner share this traversal and apply
/// their type-name selection at the call site.
pub fn walk_class_type_names<F: FnMut(&str)>(class: &ClassDef, visit: &mut F) {
    for ext in &class.extends {
        let name = ext.base_name.to_string();
        visit(&name);
    }
    for (_, comp) in class.components.iter() {
        let t = format!("{}", comp.type_name);
        visit(&t);
    }
    for nested in class.classes.values() {
        walk_class_type_names(nested, visit);
    }
}

/// Return deterministic top-level roots referenced by qualified type names and
/// imports in a parsed definition. Bare names resolve in the current source
/// context, while Modelica built-ins are handled by Rumoca.
pub fn source_root_dependencies_from_ast(ast: &StoredDefinition) -> BTreeSet<String> {
    let mut qualified_names = BTreeSet::new();
    for class in ast.classes.values() {
        walk_class_type_names(class, &mut |name| {
            if name.contains('.') {
                qualified_names.insert(name.to_owned());
            }
        });
        for import in &class.imports {
            use rumoca_ir_ast::Import;
            let path = match import {
                Import::Qualified { path, .. }
                | Import::Renamed { path, .. }
                | Import::Unqualified { path, .. }
                | Import::Selective { path, .. } => path.to_string(),
            };
            if path.contains('.') {
                qualified_names.insert(path);
            }
        }
    }
    qualified_names
        .into_iter()
        .filter_map(|name| {
            rumoca_core::qualified_name_segments(&name)
                .next()
                .map(str::to_owned)
        })
        .filter(|root| !root.is_empty() && !is_builtin_type_name(root))
        .collect()
}

/// Return every source root needed to compile one definition: qualified
/// references plus its `within` package root when that root is not declared in
/// the same source set.
pub fn required_source_roots_from_ast(ast: &StoredDefinition) -> BTreeSet<String> {
    let mut roots = source_root_dependencies_from_ast(ast);
    if let Some(package) = within_package(ast)
        && let Some(root) = rumoca_core::qualified_name_segments(&package).next()
        && !ast.classes.contains_key(root)
        && !root.is_empty()
    {
        roots.insert(root.to_owned());
    }
    roots
}

/// Whether a type reference is handled by Modelica/Rumoca without an external
/// source root. Keep this filter beside the shared type-name traversal so the
/// source-root admission path and the icon warmer cannot diverge on built-ins.
pub fn is_builtin_type_name(name: &str) -> bool {
    matches!(
        name,
        "Real" | "Integer" | "Boolean" | "String" | "enumeration"
    )
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
/// description string, and the modification map (`R=10`,
/// `unit="kg"`, …) projected to strings.
#[derive(Debug, Clone)]
pub struct ComponentInfo {
    /// Instance name as authored in the class.
    pub name: String,
    /// Declared Modelica type name.
    pub type_name: String,
    /// Description value decoded by Rumoca.
    pub description: String,
    /// Component modifications rendered by Rumoca, keyed by authored names.
    pub modifications: HashMap<String, String>,
}

/// Extract direct sub-component declarations from a class.
pub fn extract_components_for_class(class: &ClassDef) -> Vec<ComponentInfo> {
    class
        .components
        .values()
        .map(|c| ComponentInfo {
            name: c.name.clone(),
            type_name: c.type_name.to_string(),
            description: description_from_tokens(&c.description).unwrap_or_default(),
            modifications: c
                .modifications
                .iter()
                .map(|(k, v)| (k.clone(), v.to_string()))
                .collect(),
        })
        .collect()
}

/// Connect-equations of a class. Returns `(from, to)` pairs as
/// dot-paths (e.g. `("tank.outlet", "valve.inlet")`). Non-connect
/// equations (algebraic, when, if, …) are intentionally not surfaced
/// here — the agent's structural picture is the wiring, not the
/// constitutive equations.
pub fn extract_connections_for_class(class: &ClassDef) -> Vec<(String, String)> {
    use rumoca_ir_ast::Equation;
    class
        .equations
        .iter()
        .filter_map(|e| match e {
            Equation::Connect { lhs, rhs, .. } => Some((lhs.to_string(), rhs.to_string())),
            _ => None,
        })
        .collect()
}

/// Concatenate Rumoca's decoded description tokens in authored order.
/// Modelica string concatenation inserts no separator and preserves whitespace.
pub fn description_from_tokens(tokens: &[rumoca_core::Token]) -> Option<String> {
    let description: String = tokens.iter().map(|token| token.text.as_ref()).collect();
    (!description.is_empty()).then_some(description)
}

/// Extract every input-typed component for a class with rich metadata
/// (name, type, unit, default if any, description). Companion to the
/// the selected source interface, which carries declaration names.
#[derive(Debug, Clone)]
pub struct TypedComponent {
    /// Component name as authored in the class.
    pub name: String,
    /// Declared Modelica type name.
    pub type_name: String,
    /// Resolved primitive type, absent for aggregate declarations.
    pub scalar_type: Option<rumoca_core::PredefinedComponentType>,
    /// Whether this component or its resolved type carries array dimensions.
    pub has_array_dimensions: bool,
    /// Optional `unit` modification.
    pub unit: Option<String>,
    /// Numeric binding or start value, when one is available.
    pub default: Option<f64>,
    /// Description value decoded by Rumoca.
    pub description: String,
    /// Optional lower bound from the declaration.
    pub min: Option<f64>,
    /// Optional upper bound from the declaration.
    pub max: Option<f64>,
}

/// Read the unit value decoded by Rumoca, when authored as a string literal.
fn unit_of_component(comp: &rumoca_ir_ast::Component) -> Option<String> {
    comp.modifications
        .get("unit")
        .and_then(string_literal_value)
}

/// Project one resolved declaration into the authoring metadata ABI.
/// The session supplies type facts; expressions and locations remain authored.
pub fn component_metadata(
    component: &rumoca_ir_ast::Component,
    scalar_type: Option<rumoca_core::PredefinedComponentType>,
    has_type_dimensions: bool,
) -> TypedComponent {
    TypedComponent {
        name: component.name.clone(),
        type_name: component.type_name.to_string(),
        scalar_type,
        has_array_dimensions: has_type_dimensions
            || !component.shape.is_empty()
            || !component.shape_expr.is_empty(),
        unit: unit_of_component(component),
        default: component
            .binding
            .as_ref()
            .map_or_else(|| numeric_of(&component.start), numeric_of),
        description: description_from_tokens(&component.description).unwrap_or_default(),
        min: component.modifications.get("min").and_then(numeric_of),
        max: component.modifications.get("max").and_then(numeric_of),
    }
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

    #[test]
    fn model_selection_requires_a_runnable_declaration() {
        for source in [
            "package Root package Empty end Empty; end Root;",
            "connector Pin Real x; end Pin;",
            "record Data Real x; end Data;",
            "type Count = Integer;",
            "function Identity input Real x; output Real y; algorithm y := x; end Identity;",
        ] {
            assert_eq!(extract_model_name(source), None);
        }
    }

    #[test]
    fn rumoca_metadata_projection_preserves_expressions_and_decoded_strings() {
        let source = r#"model Metadata
  parameter Real gain = 2;
  Real value[2](min=if gain > 0 then sin(gain) else -1, start={gain, 2});
  parameter String label = "\"quoted\"\\n";
  input Real drive = sin(gain);
  Real temperature(unit="\"K\"") "\"Case\" " + "temperature\\n";
end Metadata;"#;
        let ast = rumoca_phase_parse::parse_to_ast(source, "metadata.mo").expect("valid metadata");
        let class = &ast.classes["Metadata"];
        let components = extract_components_for_class(class);
        let value = components.iter().find(|c| c.name == "value").unwrap();
        assert_eq!(
            value.modifications["min"],
            "if gain > 0 then sin(gain) else -1"
        );
        assert_eq!(class.components["value"].start.to_string(), "{gain, 2}");
        assert_eq!(
            string_literal_value(class.components["label"].binding.as_ref().unwrap()),
            Some("\"quoted\"\\n".to_string())
        );
        let metadata = variable_metadata(source, "metadata.mo");
        assert_eq!(metadata["temperature"].unit.as_deref(), Some("\"K\""));
        assert_eq!(
            metadata["temperature"].description.as_deref(),
            Some("\"Case\" temperature\\n")
        );
        assert!(string_literal_value(class.components["gain"].binding.as_ref().unwrap()).is_none());
        assert_eq!(
            component_metadata(
                &class.components["drive"],
                Some(rumoca_core::PredefinedComponentType::Real),
                false
            )
            .default,
            None
        );
    }

    #[test]
    fn model_interface_prepares_ordered_source_root_requirements() {
        let source = concat!(
            "within LunCo.Pointing;\n",
            "model SunTracker\n",
            "  extends LunCo.Controls.Base;\n",
            "  Modelica.Blocks.Interfaces.RealInput azimuth;\n",
            "end SunTracker;\n",
        );
        let interface = parse_model_interface(source, "SunTracker.mo");
        assert_eq!(
            interface.required_source_roots,
            BTreeSet::from(["LunCo".to_owned(), "Modelica".to_owned()])
        );
    }

    #[test]
    fn input_names_include_unbound_inputs_the_defaults_map_omits() {
        let source = concat!(
            "model M\n",
            "  input Real drive_left \"wired, no default\";\n",
            "  input Real gain = 2.5;\n",
            "  parameter Real p = 1.0;\n",
            "  output Real y;\n",
            "equation\n",
            "  y = gain * drive_left;\n",
            "end M;\n",
        );
        let ast = parse(source).expect("parses");

        let interface = parse_model_interface_from_ast(&ast);
        let names = &interface.inputs;
        assert!(
            names.contains("drive_left"),
            "unbound input missing from the interface: {names:?}"
        );
        assert!(names.contains("gain"), "bound input missing: {names:?}");
        assert!(
            !names.contains("p") && !names.contains("y"),
            "only `input` causality belongs in the interface: {names:?}"
        );

        // The defaults map keeps its own meaning: only the authored binding.
        let defaults = &interface.input_defaults;
        assert_eq!(defaults.get("gain"), Some(&2.5));
        assert!(
            !defaults.contains_key("drive_left"),
            "an unbound input has no authored default to report"
        );
    }

    #[test]
    fn output_names_include_causal_outputs() {
        let source = concat!(
            "model M\n",
            "  input Real command;\n",
            "  output Real value;\n",
            "  Real internal;\n",
            "equation\n",
            "  value = command;\n",
            "  internal = value;\n",
            "end M;\n",
        );
        let interface = parse_model_interface(source, "outputs.mo");

        assert_eq!(interface.outputs, BTreeSet::from(["value".to_string()]));
        assert!(!interface.outputs.contains("internal"));
    }

    #[test]
    fn interface_is_scoped_to_selected_runnable_class() {
        let source = r#"
within Demo;
package Models
  connector UnselectedConnector
    parameter Real gain = 99;
    input Real connector_input = 88;
    output Real connector_output;
  end UnselectedConnector;
  model Selected
    parameter Real gain = 3;
    input Real command = 2;
    output Real sample(unit="m") "selected output";
    model Nested
      input Real nested_input = 77;
      output Real sample(unit="s") "nested output";
    end Nested;
  equation
    sample = gain * command;
  end Selected;
  model Sibling
    parameter Real sibling_parameter = 66;
    input Real sibling_input = 55;
    output Real sibling_output;
  equation
    sibling_output = sibling_input;
  end Sibling;
end Models;
"#;
        let interface = parse_model_interface(source, "Models.mo");
        assert_eq!(interface.model_name.as_deref(), Some("Models.Selected"));
        assert_eq!(interface.parameters, HashMap::from([("gain".into(), 3.0)]));
        assert_eq!(interface.inputs, BTreeSet::from(["command".into()]));
        assert_eq!(
            interface.input_defaults,
            HashMap::from([("command".into(), 2.0)])
        );
        assert_eq!(interface.outputs, BTreeSet::from(["sample".into()]));
        assert_eq!(
            interface.variable_metadata["sample"].unit.as_deref(),
            Some("m")
        );
        assert_eq!(
            interface.variable_metadata["sample"].description.as_deref(),
            Some("selected output")
        );
        let ast = parse_recovered(source, "Models.mo");
        let explicit = model_interface_for_class(&ast, "Demo.Models.Sibling").unwrap();
        assert_eq!(explicit.model_name.as_deref(), Some("Models.Sibling"));
        assert_eq!(
            explicit.parameters,
            HashMap::from([("sibling_parameter".into(), 66.0)])
        );
        assert_eq!(explicit.inputs, BTreeSet::from(["sibling_input".into()]));
        assert!(model_interface_for_class(&ast, "Demo.Models.Missing").is_none());
        let library = parse_model_interface(
            "connector Library input Real value=4; parameter Real gain=7; end Library;",
            "Library.mo",
        );
        assert!(library.model_name.is_none());
        assert!(
            library.parameters.is_empty()
                && library.inputs.is_empty()
                && library.outputs.is_empty()
                && library.input_defaults.is_empty()
        );
    }

    // --- extract_model_name ---

    #[test]
    fn test_extract_model_name_nested_in_package_returns_qualified() {
        // Package documents select the first qualified executable class.
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
    fn test_extract_model_name_package_selects_executable_class() {
        let source = r#"
package MyPackage
  model Inner
    Real x;
  end Inner;
end MyPackage;
"#;
        assert_eq!(
            extract_model_name(source),
            Some("MyPackage.Inner".to_string())
        );
    }

    #[test]
    fn interface_keeps_numeric_parameter_bindings() {
        let source = r#"
model SpringMass
  parameter Real k = 100.0;
  parameter Real m = 1.0;
  Real x;
end SpringMass;
"#;
        let params = parse_model_interface(source, "parameters.mo").parameters;
        assert_eq!(params.len(), 2);
        assert_eq!(params.get("k"), Some(&100.0));
        assert_eq!(params.get("m"), Some(&1.0));
    }

    #[test]
    fn interface_preserves_absent_parameter_bindings() {
        let source = r#"
model Test
  parameter Real k;
end Test;
"#;
        let params = parse_model_interface(source, "parameters.mo").parameters;
        // Parameter without binding value should not appear (no numeric value)
        assert!(params.is_empty());
    }

    #[test]
    fn interface_keeps_numeric_input_bindings() {
        let source = r#"
model Test
  input Real g = 9.81;
  output Real y;
equation
  y = g;
end Test;
"#;
        let inputs = parse_model_interface(source, "inputs.mo").input_defaults;
        assert_eq!(inputs.len(), 1);
        assert_eq!(inputs.get("g"), Some(&9.81));
    }

    #[test]
    fn variable_metadata_preserves_authored_description_and_unit() {
        let source = r#"
model Thermal
  Real case_temperature(unit="K") "Motor case temperature";
  Real undocumented;
end Thermal;
"#;
        let metadata = variable_metadata(source, "Thermal.mo");
        assert_eq!(
            metadata.get("case_temperature"),
            Some(&ModelicaVariableMetadata {
                description: Some("Motor case temperature".to_string()),
                unit: Some("K".to_string()),
            })
        );
        assert!(
            !metadata.contains_key("undocumented"),
            "missing authoring must remain missing"
        );
    }

    #[test]
    fn variable_metadata_preserves_public_diagnostic_explanations() {
        let source = r#"
model Battery
  output Real charge_remaining_ah(unit="Ah") "Charge currently available";
end Battery;
"#;
        let metadata = variable_metadata(source, "Battery.mo");
        assert_eq!(
            metadata.get("charge_remaining_ah"),
            Some(&ModelicaVariableMetadata {
                description: Some("Charge currently available".to_string()),
                unit: Some("Ah".to_string()),
            })
        );
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
