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

/// The runtime backend selected by a program's composed source.
///
/// This is deliberately a small classification, not a registry of running
/// programs. The USD program prim remains the identity; consumers use this
/// result only to decide which executor owns the prim.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProgramBackend {
    /// A registered implementation named by `info:id`.
    Builtin,
    /// A Rhai source, inline or file-backed.
    Rhai,
    /// A BehaviorTree.CPP XML source, inline or file-backed.
    BehaviorTree,
    /// A Modelica source file.
    Modelica,
    /// A Python source file.
    Python,
}

/// The one selected implementation arm of a program.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProgramSource {
    /// A registered implementation named by `info:id`.
    Id(String),
    /// Text authored directly on the program prim.
    Code(String),
    /// A resolver-visible external asset.
    Asset(String),
}

/// The selected source form when the program is owned by the BehaviorTree.CPP
/// projection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BehaviorTreeSource {
    /// XML authored directly on the program prim.
    Code(String),
    /// XML loaded from the selected asset.
    Asset(String),
}

/// A source resolved far enough for a backend to claim it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedProgram {
    /// The backend that owns execution of this source.
    pub backend: ProgramBackend,
    /// The selected source arm and its value.
    pub source: ProgramSource,
}

fn asset_path_without_fragment(path: &str) -> &str {
    path.split(['?', '#']).next().unwrap_or(path)
}

/// Classify a source asset by its canonical extension.
fn program_asset_backend(path: &str) -> Option<ProgramBackend> {
    let path = asset_path_without_fragment(path);
    if lunco_core::programs::is_behavior_tree_asset(path) {
        Some(ProgramBackend::BehaviorTree)
    } else if path.ends_with(".rhai") {
        Some(ProgramBackend::Rhai)
    } else if path.ends_with(".mo") {
        Some(ProgramBackend::Modelica)
    } else if path.ends_with(".py") {
        Some(ProgramBackend::Python)
    } else {
        None
    }
}

fn source_issue(prim: &SdfPath, property: &str, message: impl Into<String>) -> ProgramSourceIssue {
    ProgramSourceIssue {
        property: format!("{prim}.{property}"),
        message: message.into(),
    }
}

/// Resolve the selected implementation arm for one composed
/// `LunCoProgramAPI` prim.
///
/// The selector is authoritative. A populated non-selected source arm is an
/// authoring conflict rather than an alternative to try. No host or collection
/// traversal happens here; execution ownership is resolved separately.
pub fn resolve_program(
    view: &StageView<'_>,
    prim: &SdfPath,
) -> Result<ResolvedProgram, ProgramSourceIssue> {
    let selector = view
        .value_str(prim, "info:implementationSource")
        .unwrap_or_default();
    let id = view
        .text(prim, "info:id")
        .filter(|id| !id.trim().is_empty());
    let code = view
        .scalar::<String>(prim, "info:sourceCode")
        .filter(|code| !code.trim().is_empty());
    let asset = view
        .asset(prim, "info:sourceAsset")
        .filter(|asset| !asset.trim().is_empty());

    match selector.as_str() {
        "id" => {
            let Some(id) = id else {
                return Err(source_issue(
                    prim,
                    "info:id",
                    "info:implementationSource selects id but info:id is empty",
                ));
            };
            if code.is_some() || asset.is_some() {
                return Err(source_issue(
                    prim,
                    "info:implementationSource",
                    "id is selected but sourceCode or sourceAsset is also populated",
                ));
            }
            Ok(ResolvedProgram {
                backend: ProgramBackend::Builtin,
                source: ProgramSource::Id(id),
            })
        }
        "sourceCode" => {
            let Some(code) = code else {
                return Err(source_issue(
                    prim,
                    "info:sourceCode",
                    "info:implementationSource selects sourceCode but info:sourceCode is empty",
                ));
            };
            if id.is_some() || asset.is_some() {
                return Err(source_issue(
                    prim,
                    "info:implementationSource",
                    "sourceCode is selected but info:id or sourceAsset is also populated",
                ));
            }
            Ok(ResolvedProgram {
                backend: if code.trim_start().starts_with('<') {
                    ProgramBackend::BehaviorTree
                } else {
                    ProgramBackend::Rhai
                },
                source: ProgramSource::Code(code),
            })
        }
        "sourceAsset" => {
            let Some(asset) = asset else {
                return Err(source_issue(
                    prim,
                    "info:sourceAsset",
                    "info:implementationSource selects sourceAsset but info:sourceAsset is empty",
                ));
            };
            if id.is_some() || code.is_some() {
                return Err(source_issue(
                    prim,
                    "info:implementationSource",
                    "sourceAsset is selected but info:id or sourceCode is also populated",
                ));
            }
            let Some(backend) = program_asset_backend(&asset) else {
                return Err(source_issue(
                    prim,
                    "info:sourceAsset",
                    format!("unsupported program source asset `{asset}`"),
                ));
            };
            Ok(ResolvedProgram {
                backend,
                source: ProgramSource::Asset(asset),
            })
        }
        other if other.is_empty() => Err(source_issue(
            prim,
            "info:implementationSource",
            "info:implementationSource is empty",
        )),
        other => Err(source_issue(
            prim,
            "info:implementationSource",
            format!("unsupported info:implementationSource `{other}`"),
        )),
    }
}

/// Resolve a program only when its selected source belongs to the
/// BehaviorTree.CPP projection. This keeps source selection and backend
/// classification in one place; consumers only translate the result into
/// their own runtime marker.
pub fn resolve_behavior_tree_source(
    view: &StageView<'_>,
    prim: &SdfPath,
) -> Result<Option<BehaviorTreeSource>, ProgramSourceIssue> {
    match resolve_program(view, prim)? {
        ResolvedProgram {
            backend: ProgramBackend::BehaviorTree,
            source: ProgramSource::Code(source),
        } => Ok(Some(BehaviorTreeSource::Code(source))),
        ResolvedProgram {
            backend: ProgramBackend::BehaviorTree,
            source: ProgramSource::Asset(asset),
        } => Ok(Some(BehaviorTreeSource::Asset(asset))),
        ResolvedProgram { .. } => Ok(None),
    }
}

/// Whether the source is owned by the generic script/driver projection in
/// `lunco-usd-bevy`. Modelica and BehaviorTree sources are deliberately not
/// included: their own projections own those execution paths.
pub fn is_generic_program_backend(backend: ProgramBackend) -> bool {
    matches!(backend, ProgramBackend::Builtin | ProgramBackend::Rhai)
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
    let resolved = resolve_program(view, prim)?;
    if resolved.backend != ProgramBackend::Modelica {
        return Err(source_issue(
            prim,
            "info:sourceAsset",
            "a Modelica program facet must select a .mo sourceAsset",
        ));
    }
    let ProgramSource::Asset(asset) = resolved.source else {
        return Err(source_issue(
            prim,
            "info:sourceAsset",
            "a Modelica program facet must select a .mo sourceAsset",
        ));
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

/// Whether an authored root output is a Modelica network boundary.
///
/// A vehicle root may carry ordinary output ports such as `drive_left` and
/// `steering` while also owning a `CollectionAPI:components` network. Those
/// ports are not generated-model outputs unless their authored connection
/// names a member of the collection. Keeping this distinction in the shared
/// USD contract prevents the linter and projector from treating an actuator
/// command surface as an unsourced Modelica boundary.
pub fn is_network_boundary_output(view: &StageView<'_>, root: &SdfPath, attr: &str) -> bool {
    let Some(_) = attr.strip_prefix("outputs:") else {
        return false;
    };
    let Ok(members) = view.collection_members(root, "components") else {
        return false;
    };
    let member_paths: HashSet<String> = members
        .into_iter()
        .filter(|path| !path.is_property_path())
        .map(|path| path.to_string())
        .collect();
    view.connections(root, attr).iter().any(|target| {
        target
            .rsplit_once(".outputs:")
            .is_some_and(|(source, _)| member_paths.contains(source))
    })
}

/// The default synthesizer for a collection of Modelica program facets.
pub const DEFAULT_DOMAIN_SYNTHESIZER: &str = "acausal-network";

/// The geometry-derived synthesizer for a collection of force actuators.
pub const ACTUATOR_WRENCH_DOMAIN_SYNTHESIZER: &str = "actuator-wrench";

/// Derive the owner of a component collection from its composed member roles.
///
/// A collection of LunCoProgramAPI members is a Modelica network. A
/// collection of LunCoForceActuatorAPI members is a geometry-derived wrench
/// allocator. Mixed or unclassified collections are invalid and must be
/// reported by the caller; no owner is guessed.
pub fn derive_synthesizer_name(view: &StageView<'_>, root: &SdfPath) -> Result<String, String> {
    let members = view
        .collection_members(root, "components")
        .map_err(|error| format!("could not read component collection: {error}"))?;
    let mut force_actuators = 0usize;
    let mut modelica_programs = 0usize;
    let mut unclassified = Vec::new();
    for member in members.iter().filter(|path| !path.is_property_path()) {
        let is_force = view.has_api_schema(member, "LunCoForceActuatorAPI");
        let is_program = view.has_api_schema(member, "LunCoProgramAPI");
        match (is_force, is_program) {
            (true, false) => force_actuators += 1,
            (false, true) => modelica_programs += 1,
            _ => unclassified.push(member.to_string()),
        }
    }
    if force_actuators > 0 && modelica_programs == 0 && unclassified.is_empty() {
        return Ok(ACTUATOR_WRENCH_DOMAIN_SYNTHESIZER.to_string());
    }
    if modelica_programs > 0 && force_actuators == 0 && unclassified.is_empty() {
        return Ok(DEFAULT_DOMAIN_SYNTHESIZER.to_string());
    }
    if force_actuators > 0 || modelica_programs > 0 || !unclassified.is_empty() {
        return Err(format!(
            "component collection has incompatible member roles: force_actuators={force_actuators}, modelica_programs={modelica_programs}, unclassified={unclassified:?}"
        ));
    }
    Ok(DEFAULT_DOMAIN_SYNTHESIZER.to_string())
}

/// Every Modelica program prim on the stage that belongs to SOME component
/// collection.
///
/// A Modelica program member is compiled as part of its network's generated
/// model, so no other pass may give it a solver of its own. Membership — not
/// "does it declare an acausal connector" — is what makes that true: a
/// causal-only member (a controller, a PDU) is just as much part of the
/// generated DAE, and gating on connectors handed it a second, independent
/// solver whose outputs then fed the wire fabric.
///
/// A collection may also contain physical prims consumed by a different
/// synthesizer. For example, the actuator-wrench network owns USD force
/// actuators as geometry, while those actuators still need their authored
/// scalar input wires materialised by the cosim projection. `LunCoProgramAPI`
/// is the authoritative boundary between a Modelica member and such a
/// physical participant; collection membership alone is not.
pub fn modelica_network_member_paths(view: &StageView<'_>) -> HashSet<String> {
    let mut members = HashSet::new();
    for prim in view.prim_paths() {
        if !is_domain_network_root(view, &prim) {
            continue;
        }
        let Ok(paths) = view.collection_members(&prim, "components") else {
            continue;
        };
        members.extend(
            paths
                .into_iter()
                .filter(|path| view.has_api_schema(path, "LunCoProgramAPI"))
                .map(|path| path.to_string()),
        );
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

    fn program_stage(source: &str) -> crate::CanonicalStage {
        crate::CanonicalStage::from_recipe(&crate::StageRecipe::from_source(
            "programs.usda",
            source,
        ))
        .expect("build program stage")
    }

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
    fn selected_program_arm_is_the_single_backend_resolution() {
        let stage = program_stage(
            "#usda 1.0\n\
             def Scope \"InlineTree\" (prepend apiSchemas = [\"LunCoProgramAPI\"])\n\
             {\n\
                 uniform token info:implementationSource = \"sourceCode\"\n\
                 uniform string info:sourceCode = \"<root/>\"\n\
             }\n\
             def Scope \"Rhai\" (prepend apiSchemas = [\"LunCoProgramAPI\"])\n\
             {\n\
                 uniform token info:implementationSource = \"sourceAsset\"\n\
                 uniform asset info:sourceAsset = @lunco://scenarios/test.rhai@\n\
             }\n\
             def Scope \"Modelica\" (prepend apiSchemas = [\"LunCoProgramAPI\"])\n\
             {\n\
                 uniform token info:implementationSource = \"sourceAsset\"\n\
                 uniform asset info:sourceAsset = @lunco://models/Test.mo@\n\
             }\n\
             def Scope \"Conflict\" (prepend apiSchemas = [\"LunCoProgramAPI\"])\n\
             {\n\
                 uniform token info:implementationSource = \"sourceAsset\"\n\
                 uniform asset info:sourceAsset = @lunco://scenarios/test.rhai@\n\
                 uniform string info:sourceCode = \"fn drive(ctx) { 1 }\"\n\
             }\n\
             def Scope \"MissingSelector\" (prepend apiSchemas = [\"LunCoProgramAPI\"])\n\
             {\n\
                 uniform asset info:sourceAsset = @lunco://scenarios/test.rhai@\n\
             }\n",
        );
        let view = stage.view();

        let inline = SdfPath::new("/InlineTree").unwrap();
        assert_eq!(
            resolve_behavior_tree_source(&view, &inline),
            Ok(Some(BehaviorTreeSource::Code("<root/>".into())))
        );

        let rhai = SdfPath::new("/Rhai").unwrap();
        assert_eq!(
            resolve_program(&view, &rhai),
            Ok(ResolvedProgram {
                backend: ProgramBackend::Rhai,
                source: ProgramSource::Asset("lunco://scenarios/test.rhai".into()),
            })
        );
        assert_eq!(resolve_behavior_tree_source(&view, &rhai), Ok(None));

        let modelica = SdfPath::new("/Modelica").unwrap();
        assert_eq!(
            modelica_source_ref(&view, &modelica).unwrap().asset,
            "lunco://models/Test.mo"
        );

        let conflict = SdfPath::new("/Conflict").unwrap();
        let issue = resolve_program(&view, &conflict).expect_err("conflicting arms are invalid");
        assert!(issue.property.ends_with(".info:implementationSource"));

        let missing = SdfPath::new("/MissingSelector").unwrap();
        let issue = resolve_program(&view, &missing).expect_err("selection is mandatory");
        assert!(issue.message.contains("info:implementationSource is empty"));
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
