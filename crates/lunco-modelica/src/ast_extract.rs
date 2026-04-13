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

    // Prefer non-package classes first (models, blocks, functions, etc.)
    let mut package_name: Option<String> = None;
    for (name, class) in &ast.classes {
        if class.class_type != ClassType::Package {
            return Some(name.clone());
        }
        if package_name.is_none() {
            package_name = Some(name.as_str().to_string());
        }
    }
    package_name
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
    collect_parameters_from_classes(ast.classes, &mut params);
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
    collect_inputs_with_defaults_from_classes(ast.classes, &mut inputs);
    inputs
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
    collect_input_names_from_classes(ast.classes, &mut inputs);
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
    collect_inputs_with_defaults_from_classes(ast.classes, &mut defaults);

    // Rebuild source with input defaults stripped using regex replacement.
    // TODO: Replace with AST-based source regeneration once we have a Modelica
    // source printer. For now, regex is the pragmatic choice for text mutation.
    let re = regex::Regex::new(
        r"(?m)(^\s*(?:input)\s+\w+\s+)([a-zA-Z0-9_]+)(\s*=\s*[-+]?[0-9]*\.?[0-9]+([eE][-+]?[0-9]+)?)",
    )
    .unwrap();

    let modified = re
        .replace_all(source, |caps: &regex::Captures| {
            format!("{}{}", &caps[1], &caps[2])
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

fn collect_parameters_from_classes(classes: impl IntoIterator<Item = (String, ClassDef)>, params: &mut HashMap<String, f64>) {
    for (_, class) in classes {
        for component in class.components.values() {
            if matches!(component.variability, Variability::Parameter(_)) {
                if let Some(value) = extract_numeric_binding(&component.binding) {
                    params.insert(component.name.clone(), value);
                }
            }
        }
        collect_parameters_from_classes(class.classes, params);
    }
}

fn collect_inputs_with_defaults_from_classes(classes: impl IntoIterator<Item = (String, ClassDef)>, inputs: &mut HashMap<String, f64>) {
    for (_, class) in classes {
        for component in class.components.values() {
            if matches!(component.causality, Causality::Input(_)) {
                if let Some(value) = extract_numeric_binding(&component.binding) {
                    inputs.insert(component.name.clone(), value);
                }
            }
        }
        collect_inputs_with_defaults_from_classes(class.classes, inputs);
    }
}

fn collect_input_names_from_classes(classes: impl IntoIterator<Item = (String, ClassDef)>, inputs: &mut Vec<String>) {
    for (_, class) in classes {
        for component in class.components.values() {
            if matches!(component.causality, Causality::Input(_)) {
                if extract_numeric_binding(&component.binding).is_none() {
                    inputs.push(component.name.clone());
                }
            }
        }
        collect_input_names_from_classes(class.classes, inputs);
    }
}

/// Try to extract a numeric `f64` value from a binding expression.
///
/// Handles `Expression::Terminal` with Real, Integer, or unsigned numeric types.
/// Returns `None` for non-numeric bindings (strings, booleans, references, etc.).
fn extract_numeric_binding(expr: &Option<Expression>) -> Option<f64> {
    let expr = expr.as_ref()?;
    match expr {
        Expression::Terminal { terminal_type, token } => match terminal_type {
            TerminalType::UnsignedReal | TerminalType::UnsignedInteger => {
                token.text.parse::<f64>().ok()
            }
            _ => None,
        },
        _ => None,
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

    // --- extract_model_name ---

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
        // Top-level class is the package; Inner is nested.
        // AST returns the first top-level class (the package),
        // which matches the old regex behavior (first match in source order).
        assert_eq!(extract_model_name(source), Some("MyPackage".to_string()));
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
        assert_eq!(symbols.parameters.len(), 2);
        assert!(symbols.parameters.contains_key("R"));
        assert!(symbols.parameters.contains_key("C"));
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
