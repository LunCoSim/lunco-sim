//! FACTS for the `modelica` lint domain.
//!
//! The split is `lunco-lint`'s: Rust extracts what is true about a model, and
//! `assets/scripting/policy/lint_modelica.rhai` decides what is worth saying
//! about it. Only this crate can read a Modelica AST, so extraction is code and
//! is tested as code; a rule is a line in a script that can be retuned against a
//! running sim.
//!
//! These are PARSE-phase facts — names, parameters and declared inputs. A
//! model's variables and its solver do not exist until a compile, so no rule
//! reached from `ValidateAsset` can ask about them; that is why the vocabulary
//! here is deliberately narrow rather than aspirational.

use lunco_hooks::HookValue as H;
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
/// ```
///
/// `shadowed` is computed here rather than left to the policy because set
/// intersection in rhai runs into the expression-complexity cap that makes a
/// whole policy fail to compile — the same trap the drivetrain rules hit.
pub fn modelica_facts(
    model: &str,
    params: &BTreeMap<String, f64>,
    inputs: &BTreeMap<String, f64>,
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
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts_of(params: &[(&str, f64)], inputs: &[(&str, f64)]) -> H {
        let p: BTreeMap<String, f64> =
            params.iter().map(|(n, v)| (n.to_string(), *v)).collect();
        let i: BTreeMap<String, f64> =
            inputs.iter().map(|(n, v)| (n.to_string(), *v)).collect();
        modelica_facts("M", &p, &i)
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
}
