//! AST-based extraction functions for Modelica source code.
//!
//! Walks the full Modelica AST produced by `rumoca_phase_parse::parse_to_ast`.
//! All functions accept raw source text and parse internally — callers that
//! already hold an `Arc<StoredDefinition>` can use the lower-level helpers
//! instead.
//!
//! ## Design Notes
//!
//! - **All types**: Unlike regex which only handled `Real`, these functions work
//!   with any component type (Real, Integer, Boolean, String, custom types).
//! - **Full class coverage**: Walks all top-level and nested classes, not just
//!   the first `model|class|block|package` declaration.
//! - **Expression-aware**: Extracts numeric values from AST expressions, not
//!   just regex-captured number literals.

use rumoca_core::{Causality, ClassType, OpBinary, OpUnary, Variability};
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

/// Parse Modelica source code into a `StoredDefinition` AST.
///
/// Returns `None` on parse failure. Use [`extract_from_source`] for the
/// high-level API that extracts all symbols in one pass.
fn parse_recovered(source: &str, file_label: &str) -> StoredDefinition {
    // Interface metadata uses the compiler's tolerant syntax projection.
    crate::parse_to_syntax(source, file_label)
        .best_effort()
        .clone()
}

fn parse(source: &str) -> Option<StoredDefinition> {
    let syntax = crate::parse_to_syntax(source, "model.mo");
    (!syntax.has_errors()).then(|| syntax.best_effort().clone())
}

// ---------------------------------------------------------------------------
// Public extraction functions (drop-in replacements for regex versions)
// ---------------------------------------------------------------------------

/// The declared, solver-independent face of a model: what it is called, what it
/// takes, what it is tuned by.
#[derive(Debug, Default, Clone)]
pub struct ModelInterface {
    /// First non-package class, fully qualified when nested.
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
    /// `parameter` declarations with their authored values.
    pub parameters: HashMap<String, f64>,
    /// Every declared input, seeded with its authored default (`0.0` when it has
    /// none). The INTERFACE is every input; the defaults map covers only the
    /// subset that authored a numeric binding — seeding from defaults alone
    /// gives an unbound `input Real drive_left` no port at all.
    pub inputs: HashMap<String, f64>,
    /// Authored numeric bindings for input variables. Unlike [`Self::inputs`],
    /// this omits unbound inputs and preserves their distinction from the
    /// documented zero seed used by the runtime port surface.
    pub input_defaults: HashMap<String, f64>,
    /// Every causal output declared by the model, including output connector
    /// types whose causality is carried by the connector class.
    pub outputs: std::collections::BTreeSet<String>,
    /// Documentation projected from the same parsed declarations as this
    /// interface. Solver adapters use it for observable telemetry metadata.
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

/// Extract authored descriptions and units for variables in a Modelica source.
///
/// An undocumented declaration produces no entry. Consumers therefore preserve
/// the absence of documentation instead of manufacturing prose from a variable
/// identifier.
pub fn variable_metadata(
    source: &str,
    file_label: &str,
) -> HashMap<String, ModelicaVariableMetadata> {
    let ast = parse_recovered(source, file_label);
    variable_metadata_from_ast(&ast)
}

fn variable_metadata_from_ast(ast: &StoredDefinition) -> HashMap<String, ModelicaVariableMetadata> {
    fn collect(class: &ClassDef, output: &mut HashMap<String, ModelicaVariableMetadata>) {
        for component in class.components.values() {
            let description = description_from_tokens(&component.description);
            let unit = component
                .modifications
                .get("unit")
                .map(expression_to_string)
                .map(|unit| unit.trim_matches('"').to_string())
                .filter(|unit| !unit.is_empty());
            if description.is_some() || unit.is_some() {
                output.insert(
                    component.name.clone(),
                    ModelicaVariableMetadata { description, unit },
                );
            }
        }
        for nested in class.classes.values() {
            collect(nested, output);
        }
    }

    let mut output = HashMap::new();
    for class in ast.classes.values() {
        collect(class, &mut output);
    }
    output
}

/// Read a model's interface from source, in one lenient parse.
///
/// The ONE way any USD-driven path derives a `ModelicaModel` stub, whether the
/// source was fetched as an asset (`cosim::dispatch_loaded_modelica_sources`)
/// or emitted by the network projector (`lunco-usd-sim-domain`). Both used to
/// open-code the same four extracts, and the copies drifted in exactly the
/// place that matters — which inputs become ports.
///
/// Lenient (`best_effort`): a model with a semantic error still yields usable
/// name/parameter/input snapshots, the same recovery `Session::recovered_file_query`
/// gives the engine side.
pub fn parse_model_interface(source: &str, file_label: &str) -> ModelInterface {
    let ast = parse_recovered(source, file_label);
    parse_model_interface_from_ast(&ast)
}

/// Read a model's interface from an already recovered AST.
///
/// Callers that already parsed the source MUST use this variant so name,
/// parameter, input, output, and metadata projections cannot silently drift
/// through separate parses or recovery modes.
pub fn parse_model_interface_from_ast(ast: &StoredDefinition) -> ModelInterface {
    let defaults = extract_inputs_with_defaults_from_ast(ast);
    let within = within_package(ast);
    let required_source_roots = required_source_roots_from_ast(ast);
    ModelInterface {
        model_name: extract_model_name_from_ast(ast),
        within,
        required_source_roots,
        parameters: extract_parameters_from_ast(ast),
        inputs: extract_input_names_from_ast(ast)
            .into_iter()
            .map(|name| {
                let seed = defaults.get(&name).copied().unwrap_or(0.0);
                (name, seed)
            })
            .collect(),
        input_defaults: defaults,
        outputs: extract_output_names_from_ast(ast),
        variable_metadata: variable_metadata_from_ast(ast),
    }
}

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
/// main thread on a 184 KB source-library source means a fresh uncached
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

/// The package a file's classes belong to, from its `within` clause —
/// `within LunCo.Propulsion;` → `Some("LunCo.Propulsion")`.
///
/// This is what makes a `.mo` a package MEMBER rather than a standalone
/// document, and the two cannot be compiled the same way: a member's
/// fully-qualified class is already owned by its package's source root, so
/// seating the file on its own registers that class a second time and rumoca's
/// merge pass rejects the pair (`Duplicate class '…' with non-identical
/// definition`). The Modelica compiler host routes on this distinction before
/// seating source into Rumoca.
///
/// A bare `within;` names the top level and is reported as `None` — it declares
/// membership of no package, which is the same thing as having no clause.
pub fn within_package(ast: &StoredDefinition) -> Option<String> {
    let name = ast.within.as_ref()?.to_string();
    (!name.is_empty()).then_some(name)
}

/// Source-level counterpart of [`within_package`], for callers that do not
/// already hold a parsed AST. Parses; `None` on a source too broken to parse
/// (such a file has no compilable class either, so the caller's next step
/// fails on its own terms).
pub fn within_package_of_source(source: &str) -> Option<String> {
    within_package(&parse(source)?)
}

/// Fully qualified names declared by a source document, including nested
/// classes. This is document metadata, not a model-specific rule: the
/// compiler uses it to decide whether a durable source root already owns an
/// extra document before registering a second URI for the same class.
/// Extract every fully qualified class name declared by one source document.
///
/// This is the same recovered AST walk used by the Modelica source-root
/// indexer. Callers that inspect a Twin-wide namespace use it to compare the
/// names that the compiler can actually resolve, rather than comparing file
/// basenames.
pub fn declared_class_names(source: &str, file_label: &str) -> Vec<String> {
    let ast = parse_recovered(source, file_label);
    let prefix = ast
        .within
        .as_ref()
        .map(ToString::to_string)
        .unwrap_or_default();
    let mut names = Vec::new();
    collect_declared_class_names(&ast.classes, &prefix, &mut names);
    names
}

fn collect_declared_class_names(
    classes: &AstIndexMap<String, ClassDef>,
    prefix: &str,
    names: &mut Vec<String>,
) {
    for (name, class) in classes {
        let qualified = qualify(prefix, name);
        names.push(qualified.clone());
        collect_declared_class_names(&class.classes, &qualified, names);
    }
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

/// Return the last dotted segment of a qualified name — the short
/// display form (`"Modelica.Blocks.PID"` → `"PID"`). For names
/// without any `.`, returns the whole input. Empty input → empty.
///
/// Delegates to rumoca-core's `top_level_last_segment`, so it is
/// **subscript-aware**: dots inside bracketed subscripts (`a.b[c.d]`)
/// are ignored rather than split on. Single source of truth shared
/// with rumoca's own name handling.
pub fn short_name(qualified: &str) -> &str {
    rumoca_core::top_level_last_segment(qualified)
}

/// Decode Modelica string-literal escape sequences. Replaces `\"`,
/// `\\`, `\n`, `\t`, `\r`, and `\'` with the corresponding character;
/// leaves any other `\X` pair as-is.
///
/// Operates on the **already-quote-stripped** content of a Modelica
/// `STRING` terminal — the surrounding `"…"` should be removed by
/// the caller. Use [`string_literal_value`] when starting from an
/// `Expression` to do both steps in one call.
pub fn unescape_modelica_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(n) = chars.next() {
                match n {
                    '"' => out.push('"'),
                    '\\' => out.push('\\'),
                    'n' => out.push('\n'),
                    't' => out.push('\t'),
                    'r' => out.push('\r'),
                    '\'' => out.push('\''),
                    other => {
                        out.push('\\');
                        out.push(other);
                    }
                }
            } else {
                out.push('\\');
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Decode an `Expression::Terminal { terminal_type: String, .. }`
/// into the raw `String` value. Strips surrounding quotes and
/// applies the full Modelica escape table via
/// [`unescape_modelica_string`]. Returns `None` for non-string
/// terminals or non-terminal expressions.
///
/// Canonical entry point for decoding Modelica string terminals. All AST
/// mutation and projection code uses this same decoder so stripping and escape
/// handling stay identical at every call site.
pub fn string_literal_value(e: &rumoca_ir_ast::Expression) -> Option<String> {
    use rumoca_ir_ast::Expression;
    use rumoca_ir_ast::TerminalType;
    let Expression::Terminal {
        terminal_type,
        token,
        ..
    } = e
    else {
        return None;
    };
    if !matches!(terminal_type, TerminalType::String) {
        return None;
    }
    let raw: &str = &token.text;
    let trimmed = raw
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .unwrap_or(raw);
    Some(unescape_modelica_string(trimmed))
}

/// Return the qualified-name prefix *before* the last dotted segment
/// — the parent scope. `"Modelica.Blocks.PID"` → `"Modelica.Blocks"`.
/// Names without any `.` (single-segment, e.g. `"PID"`) return `""`
/// — the implicit top-level scope. Empty input → `""`.
///
/// Uses the shared Modelica name segments, so a dot inside a quoted identifier
/// or a bracketed subscript remains part of that segment.
pub fn parent_qualified(qualified: &str) -> &str {
    crate::qualified_name_segments(qualified)
        .last()
        .filter(|last| last.len() < qualified.len())
        .map_or("", |last| &qualified[..qualified.len() - last.len() - 1])
}

/// Return ALL non-package classes (qualified) reachable from the
/// top-level classes, depth-first. Used by the Compile handler to
/// decide whether to auto-pick (length 0–1) or open a picker modal
/// (length ≥ 2, task #102). Cheap — walks the already-parsed AST.
pub fn collect_non_package_classes_qualified(ast: &StoredDefinition) -> Vec<String> {
    let mut out = Vec::new();
    collect_non_package_qualified(&ast.classes, "", &mut out);
    out
}

fn collect_non_package_qualified(
    classes: &AstIndexMap<String, ClassDef>,
    parent: &str,
    out: &mut Vec<String>,
) {
    for (name, class) in classes {
        let qualified = qualify(parent, name);
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
    classes: &AstIndexMap<String, ClassDef>,
    parent: &str,
) -> Option<String> {
    // Runnable = Model / Block / Class. Skip connectors, records,
    // types, functions — they have no equations to simulate and
    // compile would only produce `EmptySystem` / type errors.
    let is_runnable =
        |t: &ClassType| matches!(t, ClassType::Model | ClassType::Block | ClassType::Class);
    // First pass: prefer a runnable class AT THIS level.
    for (name, class) in classes {
        if is_runnable(&class.class_type) {
            return Some(qualify(parent, name));
        }
    }
    // Second pass: descend into each package.
    for (name, class) in classes {
        if class.class_type != ClassType::Package {
            continue;
        }
        let next_parent = qualify(parent, name);
        if let Some(found) = find_first_non_package_qualified(&class.classes, &next_parent) {
            return Some(found);
        }
    }
    // Entire subtree is packages-only (or empty). Fall back to the
    // top-level package name so earlier callers that relied on the
    // old "return the package when nothing else exists" behaviour
    // still get something non-empty; compile will likely still fail
    // but at least the error message names the file's top entity.
    classes.keys().next().map(|n| qualify(parent, n))
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
    extract_parameters_from_ast(&ast)
}

/// AST-based variant — call this from any hot path that already
/// holds a parsed `StoredDefinition`. The `_source` variants above
/// re-parse on every call, which is catastrophic (~minutes) on
/// 150 KB source library package files; hot paths like `on_compile_model`
/// MUST use these.
///
/// This source-interface projection selects outer declarations by depth.
/// Runtime initialization is owned by the compiler at qualified DAE paths.
pub fn extract_parameters_from_ast(ast: &StoredDefinition) -> HashMap<String, f64> {
    let mut collector = DefaultCollector::default();
    collect_parameters_from_classes(&ast.classes, "", 0, &mut collector);
    collector.values
}

/// Extract numeric input bindings for source-interface presentation.
/// Runtime input initialization uses the compiled declaration, not this map.
pub fn extract_inputs_with_defaults(source: &str) -> HashMap<String, f64> {
    let ast = match parse(source) {
        Some(a) => a,
        None => return HashMap::new(),
    };
    extract_inputs_with_defaults_from_ast(&ast)
}

/// AST-based variant — see `extract_parameters_from_ast`. Callers that need the
/// runtime initialization use the compiler, which retains each declaration.
pub fn extract_inputs_with_defaults_from_ast(ast: &StoredDefinition) -> HashMap<String, f64> {
    let mut collector = DefaultCollector::default();
    collect_inputs_with_defaults_from_classes(&ast.classes, "", 0, &mut collector);
    collector.values
}

/// Every `input` this model declares, bound or not — the model's INPUT
/// INTERFACE, as opposed to [`extract_inputs_with_defaults_from_ast`]'s map of
/// the subset that authored a numeric default.
///
/// The two answer different questions, and conflating them cost a whole class of
/// wiring. A driven input is normally declared UNBOUND —
/// `input Real drive_left "Normalized left-side drive command";` — precisely
/// because a wire supplies it. The defaults map skips those (correctly: there is
/// no authored default to report), so using it as the port list published an
/// interface with no inputs at all. Every wire into such a model was then
/// dropped by `PortRegistry::write_port` with
/// `[cosim] connection targets unknown input port …`, while its OUTPUTS arrived
/// normally from the solver — so the entity looked live, reported a port
/// surface, and silently accepted nothing. `RoverMotorThermal`'s
/// `drive_left`/`drive_right` are the shipped case.
pub fn extract_input_names_from_ast(ast: &StoredDefinition) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    collect_input_names_from_classes(&ast.classes, &mut names);
    names
}

/// Every output-typed component in the parsed Modelica source. This is the
/// source-level interface used when a USD prim carries additional native
/// outputs alongside its Modelica facet: only outputs the instantiated class
/// actually owns may be emitted into a generated Modelica wrapper.
pub fn extract_output_names_from_ast(ast: &StoredDefinition) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    collect_output_names_from_classes(&ast.classes, &mut names);
    names
}

// ---------------------------------------------------------------------------
// Internal AST walkers
// ---------------------------------------------------------------------------

/// Source-interface projection for the selected outer class. Shallower
/// declarations take precedence over nested class metadata; this is not a
/// runtime initialization map.
#[derive(Default)]
struct DefaultCollector {
    /// Leaf name → carried default.
    values: HashMap<String, f64>,
    /// Leaf name → (qualified scope that owns the carried value, its depth).
    origin: HashMap<String, (String, usize)>,
}

impl DefaultCollector {
    fn offer(&mut self, name: &str, scope: &str, depth: usize, value: f64) {
        let Some((_, prev_depth)) = self.origin.get(name).cloned() else {
            self.values.insert(name.to_string(), value);
            self.origin
                .insert(name.to_string(), (scope.to_string(), depth));
            return;
        };
        if depth < prev_depth {
            self.values.insert(name.to_string(), value);
            self.origin
                .insert(name.to_string(), (scope.to_string(), depth));
        }
    }
}

/// `Outer.Inner` — the scope path a nested class sits at, used only to NAME a
/// collision (the map key stays the leaf, which is what `set_input` takes).
fn qualify_scope(scope: &str, class_name: &str) -> String {
    if scope.is_empty() {
        class_name.to_string()
    } else {
        format!("{scope}.{class_name}")
    }
}

fn collect_parameters_from_classes(
    classes: &AstIndexMap<String, ClassDef>,
    scope: &str,
    depth: usize,
    out: &mut DefaultCollector,
) {
    for (class_name, class) in classes.iter() {
        let class_scope = qualify_scope(scope, class_name);
        for component in class.components.values() {
            if matches!(component.variability, Variability::Parameter(_)) {
                if let Some(value) = extract_numeric_binding(&component.binding) {
                    out.offer(&component.name, &class_scope, depth, value);
                }
            }
        }
        collect_parameters_from_classes(&class.classes, &class_scope, depth + 1, out);
    }
}

fn collect_inputs_with_defaults_from_classes(
    classes: &AstIndexMap<String, ClassDef>,
    scope: &str,
    depth: usize,
    out: &mut DefaultCollector,
) {
    for (class_name, class) in classes.iter() {
        let class_scope = qualify_scope(scope, class_name);
        for component in class.components.values() {
            if matches!(component.causality, Causality::Input(_)) {
                // An unbound input has no authored default.  Inventing `0.0`
                // here changes Modelica's initialization semantics and later
                // makes the CLI/workbench overwrite a model's own start value.
                if let Some(value) = extract_numeric_binding(&component.binding) {
                    out.offer(&component.name, &class_scope, depth, value);
                }
            }
        }
        collect_inputs_with_defaults_from_classes(&class.classes, &class_scope, depth + 1, out);
    }
}

/// Recursive half of [`extract_input_names_from_ast`]. Unlike the defaults
/// collector, an unbound `input` is exactly what this is looking for.
fn collect_input_names_from_classes(
    classes: &AstIndexMap<String, ClassDef>,
    names: &mut BTreeSet<String>,
) {
    for class in classes.values() {
        for component in class.components.values() {
            if matches!(component.causality, Causality::Input(_)) {
                names.insert(component.name.clone());
            }
        }
        collect_input_names_from_classes(&class.classes, names);
    }
}

fn collect_output_names_from_classes(
    classes: &AstIndexMap<String, ClassDef>,
    names: &mut BTreeSet<String>,
) {
    for class in classes.values() {
        for component in class.components.values() {
            if matches!(component.causality, Causality::Output(_))
                || is_output_connector_type(&component.type_name.to_string())
            {
                names.insert(component.name.clone());
            }
        }
        collect_output_names_from_classes(&class.classes, names);
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

// Walk the component tree of a chosen root class (depth-first
// through nested instance components) and emit instance-qualified
// variable names — `tank.m`, `engine.thrust`, … — matching what the
// simulator publishes once compiled. Pre-compile, this lets the
// Variables list show "where" each value lives instead of a flat
// list of leaf identifiers that collide across components.
//
// Stops recursing when a component's declared type isn't an AST
// class in this `StoredDefinition` (i.e. resolves to a source library or
// user library that we'd need rumoca's resolver to walk). Those
// components are emitted as leaves under their qualified path —
// good enough for the common authored-domain models where Tank /
// Engine / Valve sit in the same file as RocketStage.

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

/// Find a class by short name, walking nested classes too.
///
/// Many source library packages and user-authored multi-class files (e.g.
/// `AnnotatedRocketStage` which wraps `RocketStage`/`Tank`/`Valve`/…
/// inside a `package AnnotatedRocketStage`) expose simulatable classes
/// only inside a wrapper package. A top-level-only lookup misses them
/// and breaks `describe_model` even when `compile_model` (which uses
/// `collect_non_package_classes_qualified`) succeeds. Recursing here
/// keeps the two views consistent.
///
/// Returns the first match in iteration order — duplicate short names
/// across nested levels are resolved by the outer-most occurrence.
///
/// NOTE: this is a *distinct concern* from MLS §5.3 scope resolution
/// ([`crate::scope_chain_candidates`]) — it's an intra-document
/// leaf search for navigate-to-symbol, with no enclosing-scope/import
/// context and no library lookup. It is intentionally NOT folded into
/// the scope-chain resolver.
pub fn find_class_by_short_name<'a>(
    ast: &'a StoredDefinition,
    short_name: &str,
) -> Option<&'a ClassDef> {
    find_in_classes(&ast.classes, short_name)
}

fn find_in_classes<'a>(
    classes: &'a AstIndexMap<String, ClassDef>,
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

/// Byte range of a class's FULL source text — from its leading prefix
/// keyword(s) (`package`/`model`/`partial connector`/…) through the
/// terminating `;`.
///
/// rumoca's `ClassDef.location` is misleading: despite its doc comment
/// ("spanning from class keyword to end statement") it actually covers
/// only the NAME token → the `end <Name>` token, omitting BOTH the prefix
/// keyword(s) and the closing `;`. Slicing `source` by that bare range
/// drops them and yields invalid Modelica (`FooCopy … end FooCopy` with no
/// `package` and no `;`). Any code that extracts or duplicates a class's
/// source text must use this span, never `location` directly.
pub fn class_full_text_span(class: &ClassDef, source: &str) -> (usize, usize) {
    let bytes = source.as_bytes();
    // The parser's class-kind token owns the declaration start. Only class
    // qualifiers may precede it; enclosing names on the same line are not
    // part of this declaration.
    let mut start = (class.class_type_token.location.start as usize).min(bytes.len());
    loop {
        let mut i = start;
        while i > 0 && bytes[i - 1].is_ascii_whitespace() {
            i -= 1;
        }
        let word_end = i;
        while i > 0 && bytes[i - 1].is_ascii_alphabetic() {
            i -= 1;
        }
        if i == word_end
            || !matches!(
                source.get(i..word_end),
                Some(
                    "encapsulated"
                        | "partial"
                        | "final"
                        | "inner"
                        | "outer"
                        | "replaceable"
                        | "redeclare"
                        | "expandable"
                        | "operator"
                        | "pure"
                        | "impure"
                )
            )
        {
            break;
        }
        start = i;
    }
    // Advance from the `end <Name>` token past the terminating `;`.
    let mut end = class
        .end_name_token
        .as_ref()
        .map(|t| t.location.end as usize)
        .unwrap_or(class.location.end as usize)
        .min(bytes.len());
    while end < bytes.len() && bytes[end].is_ascii_whitespace() {
        end += 1;
    }
    if end < bytes.len() && bytes[end] == b';' {
        end += 1;
    }
    (start, end)
}

/// Visit every type-name reference reachable from `class`, recursing
/// into nested classes. Emits each `extends` base name and each
/// component `type_name` raw — **no filtering**. Callers apply their
/// own predicate (built-in vs not, qualified-only, etc.).
///
/// Centralised here so the icon warmer's "what to prefetch" and the
/// source-roots scanner's "which libraries to load" share one
/// traversal. The previous local `walk_class` / `walk_class_qualified_types`
/// pair was identical traversal + different filter, which is exactly
/// how the canonical `find_class_by_qualified_name` and the buggy
/// local `walk_qualified` diverged. Filter at the call site, not in
/// the walker.
pub fn walk_class_type_names<F: FnMut(&str)>(class: &ClassDef, visit: &mut F) {
    for ext in &class.extends {
        let name = ext.base_name.to_string();
        visit(&name);
    }
    for (_, comp) in class.iter_components() {
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
        .filter_map(|name| name.split('.').next().map(str::to_owned))
        .filter(|root| !root.is_empty() && !is_builtin_type_name(root))
        .collect()
}

/// Return every source root needed to compile one definition: qualified
/// references plus its `within` package root when that root is not declared in
/// the same source set.
pub fn required_source_roots_from_ast(ast: &StoredDefinition) -> BTreeSet<String> {
    let mut roots = source_root_dependencies_from_ast(ast);
    if let Some(package) = within_package(ast)
        && let Some(root) = package.split('.').next()
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
/// description string, and the literal modification map (`R=10`,
/// `unit="kg"`, …) projected to strings.
#[derive(Debug, Clone)]
pub struct ComponentInfo {
    /// Instance name as authored in the class.
    pub name: String,
    /// Declared Modelica type name.
    pub type_name: String,
    /// Description string with surrounding Modelica quotes removed.
    pub description: String,
    /// Literal component modifications keyed by their authored names.
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

/// Collapse Modelica description string tokens into the value shown to users.
/// The AST stores each quoted token separately, so quote removal must happen
/// before joining; otherwise a multi-token description retains embedded quote
/// characters and each consumer renders it differently.
pub fn description_from_tokens(tokens: &[rumoca_core::Token]) -> Option<String> {
    let mut description = String::new();
    for token in tokens {
        let text = token.text.trim();
        let text = text.strip_prefix('"').unwrap_or(text);
        let text = text.strip_suffix('"').unwrap_or(text);
        if !text.is_empty() {
            if !description.is_empty() {
                description.push(' ');
            }
            description.push_str(text);
        }
    }
    (!description.is_empty()).then_some(description)
}

/// Cheap stringification of an Expression for the modifications map.
/// Numeric and string literals round-trip exactly; complex expressions
/// fall back to a placeholder so the agent does not see a truncated
/// half-rendering. `describe_model` is best-effort surface for
/// authoring intent — for full fidelity the agent reads
/// `get_document_source`.
fn expression_to_string(expr: &Expression) -> String {
    match expr {
        Expression::Terminal {
            terminal_type,
            token,
            ..
        } => match terminal_type {
            TerminalType::String => token.text.trim_matches('"').to_string(),
            _ => token.text.to_string(),
        },
        Expression::ComponentReference(cref) => cref.to_string(),
        _ => "<expr>".into(),
    }
}

/// Format the small expression subset used by Modelica parameter/icon labels.
///
/// This is deliberately a display projection, not a source serializer: values
/// that cannot be rendered without misleading truncation return an empty
/// string. Keeping it here makes the source library indexer and diagram projection use the
/// same representation for literals, references, arithmetic, and arrays.
pub fn format_expression_for_display(expr: &Expression) -> String {
    match expr {
        Expression::Terminal {
            terminal_type,
            token,
            ..
        } => match terminal_type {
            TerminalType::String => token.text.trim_matches('"').to_string(),
            _ => token.text.to_string(),
        },
        Expression::ComponentReference(cref) => cref
            .parts
            .last()
            .map(|part| part.ident.text.as_ref().to_string())
            .unwrap_or_default(),
        Expression::Unary { op, rhs, .. } => match (op, rhs.as_ref()) {
            (OpUnary::Minus, inner) => {
                let inner = format_expression_for_display(inner);
                if inner.is_empty() {
                    String::new()
                } else {
                    format!("-{inner}")
                }
            }
            (OpUnary::Plus, inner) => {
                let inner = format_expression_for_display(inner);
                if inner.is_empty() {
                    String::new()
                } else {
                    format!("+{inner}")
                }
            }
            _ => String::new(),
        },
        Expression::Parenthesized { inner, .. } => {
            let inner = format_expression_for_display(inner);
            if inner.is_empty() {
                String::new()
            } else {
                format!("({inner})")
            }
        }
        Expression::Binary { op, lhs, rhs, .. } => {
            let lhs = format_expression_for_display(lhs);
            let rhs = format_expression_for_display(rhs);
            let symbol = match op {
                OpBinary::Add => "+",
                OpBinary::Sub => "-",
                OpBinary::Mul => "*",
                OpBinary::Div => "/",
                OpBinary::Exp => "^",
                _ => return String::new(),
            };
            if lhs.is_empty() || rhs.is_empty() {
                String::new()
            } else {
                format!("{lhs}{symbol}{rhs}")
            }
        }
        Expression::Array { elements, .. } => {
            let elements: Vec<String> =
                elements.iter().map(format_expression_for_display).collect();
            if elements.iter().any(String::is_empty) {
                String::new()
            } else {
                format!("{{{}}}", elements.join(","))
            }
        }
        _ => String::new(),
    }
}

/// Extract every input-typed component for a class with rich metadata
/// (name, type, unit, default if any, description). Companion to the
/// existing `extract_input_names_from_ast` which only returns names.
#[derive(Debug, Clone)]
pub struct TypedComponent {
    /// Component name as authored in the class.
    pub name: String,
    /// Declared Modelica type name.
    pub type_name: String,
    /// Optional `unit` modification.
    pub unit: Option<String>,
    /// Numeric binding or start value, when one is available.
    pub default: Option<f64>,
    /// Description string with surrounding Modelica quotes removed.
    pub description: String,
    /// Optional lower bound from the declaration.
    pub min: Option<f64>,
    /// Optional upper bound from the declaration.
    pub max: Option<f64>,
}

/// Extract direct input components with their authoring metadata.
pub fn extract_typed_inputs_for_class(class: &ClassDef) -> Vec<TypedComponent> {
    typed_components_filtered(class, |c| {
        matches!(c.causality, Causality::Input(_))
            || is_input_connector_type(&c.type_name.to_string())
    })
}

/// Extract direct parameter components with their authoring metadata.
pub fn extract_typed_parameters_for_class(class: &ClassDef) -> Vec<TypedComponent> {
    typed_components_filtered(class, |c| {
        matches!(c.variability, Variability::Parameter(_))
    })
}

/// Extract direct output components with their authoring metadata.
pub fn extract_typed_outputs_for_class(class: &ClassDef) -> Vec<TypedComponent> {
    typed_components_filtered(class, |c| {
        matches!(c.causality, Causality::Output(_))
            || is_output_connector_type(&c.type_name.to_string())
    })
}

/// Whether `type_name` looks like a source-library "RealInput / IntegerInput /
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
    let short = short_name(bare);
    matches!(
        short,
        "RealInput" | "IntegerInput" | "BooleanInput" | "StringInput"
    ) || short.ends_with("Input")
}

/// Symmetric counterpart of [`is_input_connector_type`] for output
/// connectors — see that doc for the rationale.
fn is_output_connector_type(type_name: &str) -> bool {
    let bare = type_name.split('[').next().unwrap_or(type_name);
    let short = short_name(bare);
    matches!(
        short,
        "RealOutput" | "IntegerOutput" | "BooleanOutput" | "StringOutput"
    ) || short.ends_with("Output")
}

/// Pull the `unit="..."` modification for a component, if any. Returns
/// the inner string with quotes stripped.
fn unit_of_component(comp: &rumoca_ir_ast::Component) -> Option<String> {
    comp.modifications.get("unit").and_then(|expr| match expr {
        Expression::Terminal {
            terminal_type: TerminalType::String,
            token,
            ..
        } => Some(token.text.trim_matches('"').to_string()),
        _ => None,
    })
}

fn typed_components_filtered<F>(class: &ClassDef, want: F) -> Vec<TypedComponent>
where
    F: Fn(&rumoca_ir_ast::Component) -> bool,
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
            description: description_from_tokens(&c.description).unwrap_or_default(),
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

    #[test]
    fn class_full_text_span_confines_same_line_nested_declarations() {
        for (source, expected) in [
            (
                "package Root package B model Part Real value; end Part; end B; end Root;",
                "model Part Real value; end Part;",
            ),
            (
                "package Root package B partial connector Part Real value; end Part; end B; end Root;",
                "partial connector Part Real value; end Part;",
            ),
            (
                "package Root package B encapsulated\npartial model Part Real value; end Part\n; end B; end Root;",
                "encapsulated\npartial model Part Real value; end Part\n;",
            ),
        ] {
            let syntax = crate::parse_to_syntax(source, "class-span.mo");
            assert!(!syntax.has_errors());
            let ast = syntax.parsed().expect("valid inline source");
            let class = &ast.classes["Root"].classes["B"].classes["Part"];
            let (start, end) = class_full_text_span(class, source);
            assert_eq!(&source[start..end], expected);
            assert!(!crate::parse_to_syntax(expected, "extracted.mo").has_errors());
        }
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

        let names = extract_input_names_from_ast(&ast);
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
        let defaults = extract_inputs_with_defaults_from_ast(&ast);
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
