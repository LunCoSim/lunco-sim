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

use rumoca_compile::parsing::ast::AstIndexMap;
use rumoca_compile::parsing::{
    Causality, ClassDef, ClassType, Expression, StoredDefinition, TerminalType, Variability,
};
use std::collections::{BTreeSet, HashMap};

// ---------------------------------------------------------------------------
// Parsing entry point
// ---------------------------------------------------------------------------

/// Parse Modelica source code into a `StoredDefinition` AST.
///
/// Returns `None` on parse failure. Use [`extract_from_source`] for the
/// high-level API that extracts all symbols in one pass.
fn parse_recovered(source: &str, file_label: &str) -> StoredDefinition {
    // Keep this prepass on the same tolerant syntax path as the production
    // compiler. The strict semantic AST rejects valid library members that
    // reference package imports or use recoverable Modelica constructs; that
    // made the input-default strip warn and silently demote every bound input
    // in those files even though Rumoca could compile them successfully.
    let source = crate::source_asset::normalize_modelica_source(source);
    rumoca_phase_parse::parse_to_syntax(&source, file_label)
        .best_effort()
        .clone()
}

fn parse(source: &str) -> Option<StoredDefinition> {
    let source = crate::source_asset::normalize_modelica_source(source);
    let syntax = rumoca_phase_parse::parse_to_syntax(&source, "model.mo");
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
    /// `parameter` declarations with their authored values.
    pub parameters: HashMap<String, f64>,
    /// Every declared input, seeded with its authored default (`0.0` when it has
    /// none). The INTERFACE is every input; the defaults map covers only the
    /// subset that authored a numeric binding — seeding from defaults alone
    /// gives an unbound `input Real drive_left` no port at all.
    pub inputs: HashMap<String, f64>,
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
    pub description: Option<String>,
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
    let mut index = crate::index::ModelicaIndex::new();
    index.rebuild_from_ast(&ast, source);
    variable_metadata_from_index(index)
}

fn variable_metadata_from_index(
    index: crate::index::ModelicaIndex,
) -> HashMap<String, ModelicaVariableMetadata> {
    index
        .components
        .into_iter()
        .filter_map(|component| {
            let description = (!component.description.is_empty()).then_some(component.description);
            let unit = component
                .modifications
                .get("unit")
                .map(|unit| unit.trim_matches('"').to_string())
                .filter(|unit| !unit.is_empty());
            (description.is_some() || unit.is_some()).then_some((
                component.name,
                ModelicaVariableMetadata { description, unit },
            ))
        })
        .collect()
}

/// Read a model's interface from source, in one lenient parse.
///
/// The ONE way any USD-driven path derives a `ModelicaModel` stub, whether the
/// source was fetched as an asset (`cosim::dispatch_loaded_modelica_sources`)
/// or emitted by the network projector (`domain_projection`). Both used to
/// open-code the same four extracts, and the copies drifted in exactly the
/// place that matters — which inputs become ports.
///
/// Lenient (`best_effort`): a model with a semantic error still yields usable
/// name/parameter/input snapshots, the same recovery `Session::recovered_file_query`
/// gives the engine side.
pub fn parse_model_interface(source: &str, file_label: &str) -> ModelInterface {
    let ast = parse_recovered(source, file_label);
    let defaults = extract_inputs_with_defaults_from_ast(&ast);
    let mut index = crate::index::ModelicaIndex::new();
    index.rebuild_from_ast(&ast, source);
    ModelInterface {
        model_name: extract_model_name_from_ast(&ast),
        within: within_package(&ast),
        parameters: extract_parameters_from_ast(&ast),
        inputs: extract_input_names_from_ast(&ast)
            .into_iter()
            .map(|name| {
                let seed = defaults.get(&name).copied().unwrap_or(0.0);
                (name, seed)
            })
            .collect(),
        outputs: extract_output_names_from_ast(&ast),
        variable_metadata: variable_metadata_from_index(index),
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

/// The package a file's classes belong to, from its `within` clause —
/// `within LunCo.Propulsion;` → `Some("LunCo.Propulsion")`.
///
/// This is what makes a `.mo` a package MEMBER rather than a standalone
/// document, and the two cannot be compiled the same way: a member's
/// fully-qualified class is already owned by its package's source root, so
/// seating the file on its own registers that class a second time and rumoca's
/// merge pass rejects the pair (`Duplicate class '…' with non-identical
/// definition`). [`crate::ModelicaCompiler::compile_str`] routes on this.
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
pub(crate) fn declared_class_names(source: &str, file_label: &str) -> Vec<String> {
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
    rumoca_compile::parsing::ir_core::top_level_last_segment(qualified)
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
pub fn string_literal_value(e: &rumoca_compile::parsing::ast::Expression) -> Option<String> {
    use rumoca_compile::parsing::ast::Expression;
    use rumoca_compile::parsing::TerminalType;
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
/// Centralised so callers stop reinventing it inline. The codebase
/// previously had two competing idioms (`rsplit_once('.').map(...)`
/// and `rsplitn(2, '.').nth(1).unwrap_or("")`) at ~12 sites; the
/// latter is one typo away from "first segment" instead of "all but
/// last". Delegates to rumoca-core's `parent_scope` (subscript-aware,
/// shared with rumoca); its `None` for single-segment names maps to
/// the empty top-level scope `""`.
pub fn parent_qualified(qualified: &str) -> &str {
    rumoca_compile::parsing::ir_core::parent_scope(qualified).unwrap_or("")
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
/// 150 KB MSL package files; hot paths like `on_compile_model`
/// MUST use these.
///
/// Leaf-name collisions between nested classes are resolved by depth (see
/// `DefaultCollector`) rather than last-write-wins, but this signature has no
/// report channel, so a collision here is deterministic yet unreported —
/// unlike the `input` side, which reports through
/// [`strip_input_defaults_with_report`].
pub fn extract_parameters_from_ast(ast: &StoredDefinition) -> HashMap<String, f64> {
    let mut collector = DefaultCollector::default();
    collect_parameters_from_classes(&ast.classes, "", 0, &mut collector);
    collector.values
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
    extract_inputs_with_defaults_from_ast(&ast)
}

/// AST-based variant — see `extract_parameters_from_ast`. Callers that need the
/// collision / unresolvable report must use
/// [`strip_input_defaults_with_report`], which is the same walk plus the strip.
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
///
/// Discards the [`InputDefaultIssue`] report. Any caller in a position to show
/// or log a diagnostic MUST use [`strip_input_defaults_with_report`] instead —
/// an issue dropped here is a slot silently sitting at 0.0.
pub fn strip_input_defaults(source: &str) -> (String, HashMap<String, f64>) {
    let (modified, defaults, _issues) = strip_input_defaults_with_report(source);
    (modified, defaults)
}

/// Everything that can go wrong while carrying a bound `input`'s default
/// across the strip, on ONE report channel.
///
/// The strip itself always succeeds where it runs (it is length-preserving
/// blanking), so none of these are about the text — they are all the same
/// failure seen from three sides: a runtime input slot that ends up at 0.0 with
/// no trace of the value the author wrote. Callers MUST surface them (the
/// worker turns each into a compile-result diagnostic); a model silently
/// running at 0.0 is the expensive failure this module exists to prevent.
#[derive(Debug, Clone)]
pub enum InputDefaultIssue {
    /// The binding was blanked (so the input stays a runtime slot) but its
    /// default could not be captured: the binding is an expression
    /// (`input Real w = 2*3.14/T`), not a numeric literal.
    Unresolvable {
        /// Component name of the `input`.
        name: String,
        /// Verbatim binding expression text from the original source.
        binding: String,
        /// Byte offset of the binding expression in the original source
        /// (length-preserving blanking keeps it valid for the stripped text
        /// too).
        byte_offset: usize,
    },
    /// Two classes in the same file declare a component with the same LEAF
    /// name and different defaults. The defaults map is leaf-keyed — that is
    /// what `SimulationSession::set_input` addresses — so only one value can
    /// be carried. The shallower scope wins and this names the one dropped,
    /// which used to vanish under a last-write-wins `HashMap::insert`.
    Collision {
        /// The leaf component name both scopes declare.
        name: String,
        /// Qualified scope (`Outer.Inner`) whose value is carried.
        kept_scope: String,
        /// The carried value.
        kept: f64,
        /// Qualified scope whose value is dropped.
        dropped_scope: String,
        /// The dropped value.
        dropped: f64,
    },
    /// The strip pre-pass could NOT parse the source, so nothing was stripped
    /// and no default was captured. Every bound `input` in this file is then
    /// left for rumoca to demote to an algebraic and the model loses those
    /// runtime slots entirely. rumoca may still compile the file (it drives its
    /// own parse), so this is often the only warning there is — it must never
    /// be swallowed.
    ParseFailed,
}

/// [`strip_input_defaults`] plus a report of everything that stopped a bound
/// `input`'s default from being carried — see [`InputDefaultIssue`].
pub fn strip_input_defaults_with_report(
    source: &str,
) -> (String, HashMap<String, f64>, Vec<InputDefaultIssue>) {
    let ast = match parse(source) {
        Some(a) => a,
        None => {
            // Returning the source UNSTRIPPED with an empty report is exactly
            // the silent fold this function exists to prevent: rumoca demotes
            // every bound input to an algebraic and nobody is told. The source
            // still goes back unstripped (there is no AST to locate bindings
            // with), but the caller now learns.
            return (
                source.to_string(),
                HashMap::new(),
                vec![InputDefaultIssue::ParseFailed],
            );
        }
    };

    let mut collector = DefaultCollector::default();
    collect_inputs_with_defaults_from_classes(&ast.classes, "", 0, &mut collector);
    let defaults = collector.values;
    let mut issues = collector.issues;

    // Walk the AST for every `input` component with an explicit binding
    // and collect the source byte range covering `= <expr>` (the
    // declaration equation), derived from the binding `Expression`'s span.
    //
    // WHY this exists: rumoca *demotes* an `input` with a binding to an
    // algebraic variable (rumoca-phase-dae, MLS §4.4.1), so `input Real g =
    // 9.81` would NOT appear in `SimulationSession::input_names()` and
    // `set_input("g", …)` would fail. By neutralising the binding we keep it a
    // true runtime slot; the original default is returned in `defaults` so the
    // UI can seed it via `set_input`. rumoca still exposes no compile-time
    // "runtime override" API to do this for us.
    //
    // STILL REQUIRED as of rumoca 0.9.20 — re-verified at that bump by
    // compiling `input Real g = 9.81` unstripped: `input_names()` came back
    // EMPTY. Delete this only when that probe lists `g`.
    //
    // CRUCIAL: we BLANK the range in place with spaces (newlines kept)
    // rather than DELETING bytes. The worker compiles this stripped
    // source and every compile/sim diagnostic's line/col is computed
    // against it; length-preserving blanking keeps byte offsets — and
    // thus click-to-source — identical to the editor's original buffer.
    // Deleting would shift every downstream offset. (Was a no-op from
    // the rumoca bump until 2026-06-14, silently breaking defaulted
    // inputs — see [[project_rumoca_input_default_strip]].)
    let mut ranges: Vec<InputBindingRange> = Vec::new();
    collect_input_binding_ranges(&ast.classes, source, &mut ranges);
    let mut bytes = source.as_bytes().to_vec();
    // The parser saw a BOM-free, same-length view above. Keep that invariant
    // in the source sent to Rumoca as well, including files with no input
    // bindings to strip.
    if source.starts_with('\u{feff}') {
        bytes[..3].copy_from_slice(b"   ");
    }
    for range in ranges {
        let (start, end) = (range.blank_start, range.expr_end);
        // Only blank ASCII ranges so we never split a multi-byte UTF-8
        // char (a string default like `= "café"`); such a binding is
        // left intact (degraded but safe — String isn't a numeric slot).
        if end <= bytes.len() && start < end && source[start..end].is_ascii() {
            for b in &mut bytes[start..end] {
                if *b != b'\n' && *b != b'\r' {
                    *b = b' ';
                }
            }
            // The binding is gone but no numeric default was captured for
            // it: without a report the runtime slot would start at 0.0
            // with no trace of the authored expression.
            if !range.numeric {
                issues.push(InputDefaultIssue::Unresolvable {
                    name: range.name,
                    binding: source[range.expr_start..range.expr_end].trim().to_string(),
                    byte_offset: range.expr_start,
                });
            }
        }
    }
    let modified = String::from_utf8(bytes).unwrap_or_else(|_| source.to_string());

    (modified, defaults, issues)
}

/// One `input` declaration binding located in the source — see
/// [`collect_input_binding_ranges`].
struct InputBindingRange {
    /// Component name of the `input`.
    name: String,
    /// Start of the range to blank (the introducing `=`).
    blank_start: usize,
    /// Byte range of the binding expression itself.
    expr_start: usize,
    expr_end: usize,
    /// Whether the binding is a numeric literal (i.e. its default lands in
    /// the captured defaults map).
    numeric: bool,
}

/// Collect the byte range covering `= <binding>` for every `input`
/// component that has an explicit declaration binding, so the binding can
/// be neutralised (see [`strip_input_defaults`]).
///
/// The range runs from the introducing `=` through the end of the binding
/// expression. We take the expression's end from `Expression::span()` and
/// walk backwards over whitespace to the `=` (declaration bindings use `=`,
/// never `:=`). If no literal `=` precedes the expression — e.g. a binding
/// synthesised from a modification rather than a `name = expr` clause — the
/// component is skipped (conservative: we only blank what we can see).
fn collect_input_binding_ranges(
    classes: &AstIndexMap<String, ClassDef>,
    source: &str,
    out: &mut Vec<InputBindingRange>,
) {
    let bytes = source.as_bytes();
    for class in classes.values() {
        for component in class.components.values() {
            if !matches!(component.causality, Causality::Input(_)) {
                continue;
            }
            let Some(binding) = component.binding.as_ref() else {
                continue;
            };
            let span = binding.span();
            let (expr_start, expr_end) = (span.start.0, span.end.0);
            // Guard against dummy/synthesised spans not indexing `source`.
            if expr_start >= expr_end || expr_end > source.len() {
                continue;
            }
            let mut i = expr_start;
            while i > 0 && matches!(bytes[i - 1], b' ' | b'\t' | b'\r' | b'\n') {
                i -= 1;
            }
            if i > 0 && bytes[i - 1] == b'=' {
                out.push(InputBindingRange {
                    name: component.name.clone(),
                    blank_start: i - 1,
                    expr_start,
                    expr_end,
                    numeric: extract_numeric_binding(&component.binding).is_some(),
                });
            }
        }
        collect_input_binding_ranges(&class.classes, source, out);
    }
}

// ---------------------------------------------------------------------------
// Internal AST walkers
// ---------------------------------------------------------------------------

/// Accumulates leaf-keyed defaults while a walk descends nested classes, and
/// REPORTS a same-leaf-name clash instead of losing one silently.
///
/// The map has to stay keyed by the leaf component name — that is the name
/// `SimulationSession::set_input` addresses — so two classes in one file that
/// both declare `input Real k` cannot both be represented. Precedence is by
/// DEPTH first (a top-level class's own component outranks a nested class's,
/// because the top-level class is the compile target and its slot is the
/// unqualified one), then by declaration order. Every discarded value becomes
/// an [`InputDefaultIssue::Collision`]; the previous `insert` made the LAST
/// nested class silently win.
#[derive(Default)]
struct DefaultCollector {
    /// Leaf name → carried default.
    values: HashMap<String, f64>,
    /// Leaf name → (qualified scope that owns the carried value, its depth).
    origin: HashMap<String, (String, usize)>,
    issues: Vec<InputDefaultIssue>,
}

impl DefaultCollector {
    fn offer(&mut self, name: &str, scope: &str, depth: usize, value: f64) {
        let Some((prev_scope, prev_depth)) = self.origin.get(name).cloned() else {
            self.values.insert(name.to_string(), value);
            self.origin
                .insert(name.to_string(), (scope.to_string(), depth));
            return;
        };
        let prev_value = self.values.get(name).copied().unwrap_or(value);
        if prev_value == value {
            // The same default authored twice — nothing is lost, so nothing to
            // report; whichever scope is recorded carries the same number.
            return;
        }
        let take_new = depth < prev_depth;
        let (kept_scope, kept, dropped_scope, dropped) = if take_new {
            (scope.to_string(), value, prev_scope, prev_value)
        } else {
            (prev_scope, prev_value, scope.to_string(), value)
        };
        self.issues.push(InputDefaultIssue::Collision {
            name: name.to_string(),
            kept_scope: kept_scope.clone(),
            kept,
            dropped_scope,
            dropped,
        });
        if take_new {
            self.values.insert(name.to_string(), value);
            self.origin.insert(name.to_string(), (kept_scope, depth));
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
// class in this `StoredDefinition` (i.e. resolves to an MSL or
// user library that we'd need rumoca's resolver to walk). Those
// components are emitted as leaves under their qualified path —
// good enough for the common authored-domain models where Tank /
// Engine / Valve sit in the same file as RocketStage.

/// Parse a numeric literal expression (including a leading `-` unary
/// minus — rumoca represents `-5` as `Unary(Minus, 5)`). Used for
/// `min`/`max` modifier extraction where negative bounds are common,
/// and shared with the annotation parser (`annotations::parsing`).
pub(crate) fn numeric_of(expr: &Expression) -> Option<f64> {
    use rumoca_compile::parsing::ir_core::OpUnary;
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
///
/// NOTE: this is a *distinct concern* from MLS §5.3 scope resolution
/// ([`crate::diagram::scope_chain_candidates`]) — it's an intra-document
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
    // Rewind from the name over the space/tab-separated prefix keyword(s)
    // (`model`/`package` plus qualifiers like `partial`/`final`/
    // `encapsulated`/`replaceable`). Stops at a newline or non-alphabetic
    // byte, so it can't cross into a previous declaration.
    let mut start = (class.name.location.start as usize).min(bytes.len());
    loop {
        let mut i = start;
        while i > 0 && matches!(bytes[i - 1], b' ' | b'\t') {
            i -= 1;
        }
        let word_end = i;
        while i > 0 && bytes[i - 1].is_ascii_alphabetic() {
            i -= 1;
        }
        if i == word_end {
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
    while end < bytes.len() && matches!(bytes[end], b' ' | b'\t') {
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
pub fn extract_connections_for_class(class: &ClassDef) -> Vec<(String, String)> {
    use rumoca_compile::parsing::ast::Equation;
    class
        .equations
        .iter()
        .filter_map(|e| match e {
            Equation::Connect { lhs, rhs, .. } => Some((lhs.to_string(), rhs.to_string())),
            _ => None,
        })
        .collect()
}

/// Collapse a description token sequence (Modelica string literal) to
/// a single trimmed string. Strips surrounding quotes — the AST keeps
/// them in the lexed token but the agent wants the value, not the
/// quoting.
fn tokens_to_description(tokens: &[rumoca_compile::parsing::Token]) -> String {
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
fn unit_of_component(comp: &rumoca_compile::parsing::ast::Component) -> Option<String> {
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
    F: Fn(&rumoca_compile::parsing::ast::Component) -> bool,
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

    // --- strip_input_defaults (rumoca bound-input demotion) ---

    #[test]
    fn strip_input_defaults_blanks_binding_length_preserving() {
        // rumoca demotes a bound `input` to an algebraic variable,
        // so the `= 9.81` must be neutralised to keep `g` a runtime slot.
        // The blanking MUST be length-preserving so diagnostic offsets
        // computed against this stripped source still map onto the editor.
        let source = "model M\n  input Real g = 9.81;\n  Real x;\nequation\n  x = g;\nend M;\n";
        let (modified, defaults) = strip_input_defaults(source);

        // Offset preservation: identical byte length and identical newlines.
        assert_eq!(modified.len(), source.len(), "strip must preserve length");
        assert_eq!(
            modified.matches('\n').count(),
            source.matches('\n').count(),
            "strip must preserve newlines"
        );

        // Default captured for UI seeding.
        assert_eq!(defaults.get("g"), Some(&9.81));

        // The binding text is gone but the declaration head survives.
        assert!(modified.contains("input Real g"));
        assert!(!modified.contains("9.81"));
        assert!(!modified.contains("= 9.81"));
        // Other lines untouched (offset of `Real x;` line unchanged).
        assert!(modified.contains("  Real x;\n"));
        assert!(modified.contains("  x = g;\n"));
        // Still parses after blanking.
        assert!(
            parse(&modified).is_some(),
            "blanked source must still parse"
        );
    }

    #[test]
    fn strip_input_defaults_accepts_windows_bom_and_crlf() {
        let source = "\u{feff}model M\r\n  input Real g = 9.81;\r\nend M;\r\n";
        let (modified, defaults, issues) = strip_input_defaults_with_report(source);

        assert_eq!(modified.len(), source.len(), "strip must preserve offsets");
        assert_eq!(&modified.as_bytes()[..3], b"   ");
        assert_eq!(
            modified.matches("\r\n").count(),
            source.matches("\r\n").count()
        );
        assert_eq!(defaults.get("g"), Some(&9.81));
        assert!(issues.is_empty(), "BOM/CRLF must not be a parse failure");
        assert!(!modified.contains("= 9.81"));
        assert!(parse(&modified).is_some(), "normalized source must parse");
    }

    #[test]
    fn strip_input_defaults_reports_non_literal_binding() {
        // `2*3.14/T` is not a numeric literal: the strip still blanks it
        // (so `w` stays a runtime slot) but can't capture a default. That
        // MUST come back as an unresolved report — the slot starts at 0.0
        // and silence here is silent wrong numbers.
        let source = "model M\n  parameter Real T = 2.0;\n  input Real w = 2*3.14/T;\nend M;\n";
        let (modified, defaults, issues) = strip_input_defaults_with_report(source);
        assert_eq!(modified.len(), source.len(), "strip must preserve length");
        assert!(!modified.contains("2*3.14/T"), "binding must be blanked");
        assert!(
            !defaults.contains_key("w"),
            "an expression binding has no capturable numeric default"
        );
        assert_eq!(issues.len(), 1);
        match &issues[0] {
            InputDefaultIssue::Unresolvable {
                name,
                binding,
                byte_offset,
            } => {
                assert_eq!(name, "w");
                assert_eq!(binding, "2*3.14/T");
                assert_eq!(&source[*byte_offset..][..1], "2");
            }
            other => panic!("expected Unresolvable, got {other:?}"),
        }
    }

    #[test]
    fn strip_input_defaults_literal_binding_is_not_reported() {
        let source = "model M\n  input Real g = 9.81;\nend M;\n";
        let (_, defaults, issues) = strip_input_defaults_with_report(source);
        assert_eq!(defaults.get("g"), Some(&9.81));
        assert!(issues.is_empty());
    }

    #[test]
    fn strip_input_defaults_reports_a_parse_failure_instead_of_silently_folding() {
        // A source the strip pre-pass cannot parse comes back UNSTRIPPED, so
        // rumoca will demote every bound input to an algebraic. That has to
        // arrive as a report — an empty report here is the silent fold.
        let source = "model M\n  input Real g = ;;;\nthis is not modelica\n";
        let (modified, defaults, issues) = strip_input_defaults_with_report(source);
        assert_eq!(modified, source, "unparseable source is returned verbatim");
        assert!(defaults.is_empty());
        assert!(
            issues
                .iter()
                .any(|i| matches!(i, InputDefaultIssue::ParseFailed)),
            "a parse failure must be reported, got {issues:?}"
        );
    }

    #[test]
    fn same_leaf_name_in_two_nested_classes_reports_a_collision() {
        // Both nested classes declare `input Real k` with DIFFERENT defaults.
        // The map is leaf-keyed, so one value cannot be carried — but it must
        // not vanish silently the way `HashMap::insert` made it.
        let source = "package P\n  model A\n    input Real k = 1.0;\n  end A;\n  \
                      model B\n    input Real k = 2.0;\n  end B;\nend P;\n";
        let (_, defaults, issues) = strip_input_defaults_with_report(source);
        assert_eq!(defaults.len(), 1, "one leaf key, one value");
        let collision = issues.iter().find_map(|i| match i {
            InputDefaultIssue::Collision {
                name,
                kept,
                dropped,
                ..
            } => Some((name.clone(), *kept, *dropped)),
            _ => None,
        });
        let (name, kept, dropped) = collision.expect("collision must be reported");
        assert_eq!(name, "k");
        assert_ne!(kept, dropped);
        assert_eq!(defaults.get("k"), Some(&kept));
    }

    #[test]
    fn strip_input_defaults_leaves_unbound_input_and_params_alone() {
        // Unbound input has nothing to strip; a parameter must NOT be
        // touched (only `input` causality is neutralised).
        let source = "model M\n  input Real u;\n  parameter Real p = 2.0;\nend M;\n";
        let (modified, defaults) = strip_input_defaults(source);
        assert_eq!(modified, source, "no input binding → source unchanged");
        // `p` is a parameter, not an input default.
        assert!(defaults.is_empty());
    }

    // --- extract_input_names (the INTERFACE, vs the defaults map) ---

    /// An UNBOUND input is the normal shape of a wired input, and it must still
    /// appear in the interface. Publishing the port surface from the defaults map
    /// instead gave `RoverMotorThermal` no inputs at all, so every wire into it
    /// was dropped as an "unknown input port" while its outputs solved normally.
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

    // --- hash_content (unchanged, still needed) ---

    #[test]
    fn test_hash_content_deterministic() {
        let source = "model Test end Test;";
        let h1 = hash_content(source);
        let h2 = hash_content(source);
        assert_eq!(h1, h2);
    }
}
