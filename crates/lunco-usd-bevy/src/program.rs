//! The USD side of a **Modelica program facet** — one reader, shared by every
//! consumer of that authoring contract.
//!
//! The runtime network projector resolves the Modelica class from the loaded
//! source file. This module owns only the composed-USD contract that must hold
//! before that asynchronous source resolution can begin; the lint fact producer
//! and the runtime projector call the same validator.
//!
//! Modelica lexical rules (identifiers, keywords, the mangling used to spell a
//! USD path as an identifier) live here for the same reason: the authoring
//! check and the code emitter must use ONE definition of "valid member name",
//! and this is the crate both sides already depend on.

use std::collections::{BTreeMap, BTreeSet, HashSet};

// `StageView` rather than `impl UsdRead`: the composed reads this needs
// (`value_str`, `collection_members`) are the view's own, and every caller —
// the runtime projector, the per-prim binder, the lint facts — already holds one.
use crate::read::UsdRead;
use crate::view::StageView;
use openusd::sdf::Path as SdfPath;

/// Why a prim that claims to be a Modelica program facet cannot be used as one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProgramSourceIssue {
    /// The property carrying the unusable opinion (`<prim>.info:sourceAsset`).
    pub property: String,
    /// Actionable explanation, suitable for a console line or a lint message.
    pub message: String,
}

/// The source reference authored by a Modelica program facet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelicaSourceRef {
    /// The asset containing the Modelica source.
    pub asset: String,
    /// An optional fully-qualified definition selected inside the source file.
    pub sub_identifier: Option<String>,
}

/// Why a prim that claims to be a Modelica program facet cannot enter source
/// resolution.
///
/// The `.mo` itself is deliberately NOT parsed here: this runs inside stage
/// reads on the web too, where the file may still be unfetched. The loaded
/// source resolver is the only authority for the class name.
pub fn modelica_source_ref(
    view: &StageView<'_>,
    prim: &SdfPath,
) -> Result<ModelicaSourceRef, ProgramSourceIssue> {
    let implementation = view
        .value_str(prim, "info:implementationSource")
        .unwrap_or_default();
    if implementation != "sourceAsset" {
        return Err(ProgramSourceIssue {
            property: format!("{prim}.info:implementationSource"),
            message: "a Modelica program facet must use info:implementationSource = sourceAsset"
                .into(),
        });
    }
    let Some(asset) = view.asset(prim, "info:sourceAsset") else {
        return Err(ProgramSourceIssue {
            property: format!("{prim}.info:sourceAsset"),
            message: "a Modelica program facet must author a .mo info:sourceAsset".into(),
        });
    };
    let sub_identifier = view
        .value_str(prim, "info:sourceAsset:subIdentifier")
        .filter(|value| !value.is_empty());
    if let Some(class) = sub_identifier.as_deref() {
        if class.is_empty() || !class.split('.').all(is_modelica_identifier) {
            return Err(ProgramSourceIssue {
                property: format!("{prim}.info:sourceAsset:subIdentifier"),
                message: format!("`{class}` is not a fully-qualified Modelica class name"),
            });
        }
    }
    Ok(ModelicaSourceRef {
        asset,
        sub_identifier,
    })
}

/// Is `prim` the root of a projected domain network — i.e. does it carry the
/// component collection the runtime compiles into one generated model?
///
/// Codeless multiple-apply schemas are not consistently surfaced by every
/// OpenUSD binding through `HasAPI`; their standard authored properties are
/// authoritative and round-trip in all runtimes.
pub fn is_domain_network_root(view: &StageView<'_>, prim: &SdfPath) -> bool {
    view.any_attr_with_prefix(prim, "collection:components:")
}

/// Every prim on the stage that belongs to SOME component collection.
///
/// A member is compiled as part of its network's generated model, so no other
/// pass may give it a solver of its own. Membership — not "does it declare an
/// acausal connector" — is what makes that true: a causal-only member (a
/// controller, a PDU) is just as much part of the generated DAE, and gating on
/// connectors handed it a second, independent solver whose outputs then fed the
/// wire fabric.
pub fn network_member_paths(view: &StageView<'_>) -> HashSet<String> {
    let mut members = HashSet::new();
    for prim in view.prim_paths() {
        if !is_domain_network_root(view, &prim) {
            continue;
        }
        let Ok(paths) = view.collection_members(&prim, "components") else {
            continue;
        };
        members.extend(paths.into_iter().map(|path| path.to_string()));
    }
    members
}

/// The undirected topology used to derive synthesis units from a composed
/// program graph.
///
/// Both acausal `connectors:*` edges and internal causal output-to-input edges
/// are represented here. The graph deliberately carries no domain vocabulary:
/// electrical, thermal, harness, and future synthesizers all need the same
/// deterministic connected-component operation, while the rules for emitting
/// a component remain owned by the selected synthesizer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProgramGraph {
    nodes: BTreeSet<String>,
    edges: BTreeMap<String, BTreeSet<String>>,
}

impl ProgramGraph {
    /// Add a program facet, including an isolated facet as a one-node unit.
    pub fn add_node(&mut self, node: impl Into<String>) {
        let node = node.into();
        self.nodes.insert(node.clone());
        self.edges.entry(node).or_default();
    }

    /// Add an undirected relation between two program facets.
    pub fn connect(&mut self, left: impl Into<String>, right: impl Into<String>) {
        let left = left.into();
        let right = right.into();
        self.add_node(left.clone());
        self.add_node(right.clone());
        self.edges
            .entry(left.clone())
            .or_default()
            .insert(right.clone());
        self.edges.entry(right).or_default().insert(left);
    }

    /// Return stable connected units, sorted by their first composed path.
    pub fn connected_components(&self) -> Vec<Vec<String>> {
        let mut unseen = self.nodes.clone();
        let mut units = Vec::new();
        while let Some(seed) = unseen.iter().next().cloned() {
            let mut pending = vec![seed];
            let mut unit = Vec::new();
            while let Some(current) = pending.pop() {
                if !unseen.remove(&current) {
                    continue;
                }
                unit.push(current.clone());
                if let Some(neighbors) = self.edges.get(&current) {
                    pending.extend(neighbors.iter().cloned());
                }
            }
            unit.sort();
            units.push(unit);
        }
        units.sort_by(|left, right| left.first().cmp(&right.first()));
        units
    }
}

/// The Modelica keywords a generated or authored member name may not be.
const KEYWORDS: &[&str] = &[
    "algorithm",
    "and",
    "annotation",
    "block",
    "break",
    "class",
    "connect",
    "connector",
    "constant",
    "constrainedby",
    "der",
    "discrete",
    "each",
    "else",
    "elseif",
    "elsewhen",
    "encapsulated",
    "end",
    "enumeration",
    "equation",
    "expandable",
    "extends",
    "external",
    "false",
    "final",
    "flow",
    "for",
    "function",
    "if",
    "import",
    "impure",
    "in",
    "initial",
    "inner",
    "input",
    "loop",
    "model",
    "not",
    "operator",
    "or",
    "outer",
    "output",
    "package",
    "parameter",
    "partial",
    "protected",
    "public",
    "pure",
    "record",
    "redeclare",
    "replaceable",
    "return",
    "stream",
    "then",
    "true",
    "type",
    "when",
    "while",
    "within",
];

pub fn is_modelica_identifier(raw: &str) -> bool {
    let mut chars = raw.chars();
    chars
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && chars.all(|character| character.is_ascii_alphanumeric() || character == '_')
        && !KEYWORDS.contains(&raw)
}

/// Injective ASCII spelling for arbitrary USD path/name text.
///
/// `_` is escaped too, so punctuation replacement cannot collapse `Motor-A`
/// and `Motor_A` onto one Modelica instance.
pub fn modelica_identifier(raw: &str) -> String {
    if is_modelica_identifier(raw) {
        return raw.to_string();
    }
    let mut result = modelica_path_identifier(raw);
    if !result.starts_with("usd_") {
        result.insert_str(0, "usd_");
    }
    result
}

pub fn modelica_path_identifier(raw: &str) -> String {
    let mut result = String::with_capacity(raw.len() + 1);
    for character in raw.chars() {
        if character.is_ascii_alphanumeric() {
            result.push(character);
        } else if character == '_' {
            result.push_str("__");
        } else {
            result.push_str(&format!("_x{:x}_", character as u32));
        }
    }
    if result.is_empty() {
        result.push_str("ModelicaNetwork");
    }
    if result.as_bytes()[0].is_ascii_digit() || !is_modelica_identifier(&result) {
        result.insert_str(0, "usd_");
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_identifiers_are_injective_and_avoid_keywords() {
        assert_ne!(
            modelica_path_identifier("Motor-A"),
            modelica_path_identifier("Motor_A")
        );
        assert_eq!(modelica_identifier("model"), "usd_model");
        assert_eq!(modelica_identifier("3phase"), "usd_3phase");
        assert!(is_modelica_identifier(&modelica_identifier("left/right")));
    }

    #[test]
    fn program_graph_returns_stable_units_for_acausal_and_causal_edges() {
        let mut graph = ProgramGraph::default();
        graph.add_node("/Rover/Thermal/LeftMass");
        graph.connect("/Rover/Thermal/LeftMass", "/Rover/Thermal/LeftRadiator");
        graph.connect("/Rover/Thermal/LeftLoad", "/Rover/Thermal/LeftMass");
        graph.add_node("/Rover/Thermal/RightMass");

        assert_eq!(
            graph.connected_components(),
            vec![
                vec![
                    "/Rover/Thermal/LeftLoad".to_string(),
                    "/Rover/Thermal/LeftMass".to_string(),
                    "/Rover/Thermal/LeftRadiator".to_string(),
                ],
                vec!["/Rover/Thermal/RightMass".to_string()],
            ]
        );
    }
}
