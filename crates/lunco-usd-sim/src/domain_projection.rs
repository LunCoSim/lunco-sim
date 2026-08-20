//! Runtime projection of composed USD component networks into Modelica wrappers.
//!
//! A reusable part applies `LunCoProgramAPI` for its model facet. Modelica remains the
//! authority for equations and member types; USD supplies instances, constant
//! input opinions, and ordinary property connections between public members.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::fmt::Write as _;

use bevy::prelude::*;
use lunco_modelica::{
    ast_extract::parse_model_interface, ModelicaChannels, ModelicaCommand, ModelicaModel,
    ModelicaNotice, NoticeLevel,
};
use lunco_usd_bevy::program::ProgramGraph;
use lunco_usd_bevy::{CanonicalStages, UsdPrimPath, UsdRead, UsdStageAsset};
use openusd::sdf::Path as SdfPath;

// The USD side of a Modelica program facet — the class an asset names, the
// lexical rules for member/instance identifiers — is ONE reader, shared with the
// lint fact producer. See `lunco_usd_bevy::program`.
pub use lunco_usd_bevy::program::is_domain_network_root;
use lunco_usd_bevy::program::{
    is_modelica_identifier, modelica_identifier, modelica_path_identifier, modelica_source_ref,
};

use crate::cosim::{UsdModelicaPortContract, UsdSourcedCosim, WiringDirty};

fn retire_sim_interface(commands: &mut Commands, entity: Entity) {
    commands
        .entity(entity)
        .remove::<lunco_cosim::SimComponent>();
}

/// Fingerprint of the generated wrapper currently installed on a network scope.
#[derive(Component)]
pub struct DomainProjectionState {
    fingerprint: u64,
}

/// Inspectable runtime artifact for diagnostics and API/UI projection —
/// readable through the `GeneratedModelicaSource` query
/// ([`GeneratedSourceProvider`]).
///
/// This is derived state, never persisted back into USD. Keeping the exact
/// compiler input beside the run entity makes a compiler line actionable: the
/// worker reports errors against `generated://<model>.mo`, a document that
/// exists nowhere on disk, so without a read path those line numbers name text
/// nobody can obtain.
#[derive(Component, Clone, Debug)]
pub struct GeneratedModelicaSource {
    /// Composed USD scope that owns this compilation unit.
    pub network_root: String,
    /// Stable transient document URI used by the Modelica compiler for this unit.
    pub doc_uri: String,
    /// Exact transient Modelica source sent to the compiler.
    pub source: String,
    /// Included composed USD component paths.
    pub component_paths: Vec<String>,
    /// `(prim path, source asset, instantiated class)` per member — the
    /// attribution a `generated://` compile error needs.
    pub members: Vec<(String, String, String)>,
    /// Causal outputs of generated members promoted to the wrapper boundary.
    /// Each tuple is `(member USD path, member output, wrapper output)`.
    ///
    /// A generated network is one solver participant, but its composed USD
    /// members remain addressable presentation/topology nodes. This map is the
    /// generic address translation that lets an external USD consumer (for
    /// example a light or a telemetry adapter) read a member output without
    /// creating a second solver for that member.
    pub member_output_aliases: Vec<(String, String, String)>,
    /// Deterministic composite units selected by the synthesizer.
    pub units: Vec<SynthesisUnit>,
}

/// One public Modelica component facet authored in USD.
#[derive(Clone, Debug, PartialEq)]
pub struct DomainComponent {
    /// Composed USD path of the `LunCoProgramAPI` facet.
    pub path: String,
    /// The `info:sourceAsset` this facet names — the file whose `within` + class
    /// decides what [`MemberClasses`] lets the emitter instantiate.
    pub source_asset: String,
    /// Fully-qualified class declared by the loaded `info:sourceAsset` source.
    pub model_class: String,
    /// Constant public inputs, emitted as component modifications.
    pub constants: BTreeMap<String, f64>,
    /// Acausal member name to the connected `connectors:*` property path.
    pub connectors: BTreeMap<String, Vec<String>>,
    /// All declared acausal members, including currently unconnected pins.
    pub declared_connectors: BTreeSet<String>,
    /// Causal input name to its connected source property path.
    pub inputs: BTreeMap<String, String>,
    /// Public causal outputs declared by the reusable model facet.
    pub declared_outputs: BTreeSet<String>,
}

/// One network scope and its public causal boundary.
#[derive(Clone, Debug, PartialEq)]
pub struct DomainNetwork {
    /// Composed path of the ordinary USD `Scope`.
    pub root: String,
    /// Modelica component facets below the scope.
    pub components: Vec<DomainComponent>,
    /// Public wrapper inputs authored on the scope.
    pub inputs: BTreeSet<String>,
    /// Public wrapper input name to its composed external source.
    pub input_sources: BTreeMap<String, String>,
    /// Public wrapper output name to component output property.
    pub outputs: BTreeMap<String, String>,
    /// At least one member's `.mo` has not loaded and therefore its declared
    /// class is not available. The projector waits instead of compiling a
    /// partial network. See [`MemberClasses`].
    pub pending_sources: bool,
}

/// One deterministic Modelica composite unit inside a network Scope.
///
/// A unit is a connected component of the composed program graph. It is not a
/// second ECS participant: the selected synthesizer emits the units below one
/// generated root model, so the Scope keeps one public boundary and one
/// runtime lifecycle while independent acausal subgraphs remain explicit in
/// the generated Modelica. This is the same composite-model shape used by
/// SSP/FMI toolchains.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SynthesisUnit {
    /// Stable generated Modelica class name for the composite unit.
    pub name: String,
    /// Composed USD members absorbed into the unit.
    pub component_paths: Vec<String>,
    /// Root boundary inputs consumed by this unit.
    pub inputs: BTreeSet<String>,
    /// Root boundary outputs produced by this unit.
    pub outputs: BTreeSet<String>,
}

/// One authoring error that prevents a safe runtime projection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DomainProjectionError {
    /// USD prim or property carrying the invalid opinion.
    pub path: String,
    /// Actionable explanation suitable for the simulator console.
    pub message: String,
}

// A `Scope` is ONE runtime compilation unit: one generated root model on one
// entity carrying one `ModelicaModel`. The synthesizer may partition the
// composed graph into several Modelica composite units, but it owns that
// topology operation and emits the units under the root. The runtime therefore
// never invents entities or a second definition of graph connectivity.

/// What a synthesizer hands back: the generated root, its public contract, and
/// the explicit composite units inside it.
#[derive(Clone, Debug, Default)]
pub struct SynthesisPlan {
    /// The Modelica source to compile.
    pub source: String,
    /// Public causal inputs of the generated model.
    pub inputs: BTreeSet<String>,
    /// Public causal outputs of the generated model.
    pub outputs: BTreeSet<String>,
    /// Composed USD paths absorbed into this unit.
    pub component_paths: Vec<String>,
    /// `(prim, source asset, class)` per member — attribution + class audit.
    pub members: Vec<(String, String, String)>,
    /// Connected composite units emitted below the root model.
    pub units: Vec<SynthesisUnit>,
}

/// One way of turning a composed USD scope into ONE Modelica compilation unit.
///
/// The seam doc 37 §8 asks for. What ships is the acausal-network synthesizer
/// below; a `thermal`, `harness` or `comms-link` synthesizer is a registration,
/// not an edit to [`project_domain_islands`]. A scope selects one with
/// `uniform token lunco:synthesizer`; absent means the default.
///
/// Not yet rhai-authored: a rhai body would need an emit surface of its own
/// (`ApplyModelicaOp`-style verbs) before policy could live outside Rust. The
/// registry is what makes that a later addition rather than a rewrite.
pub trait DomainSynthesizer: Send + Sync + 'static {
    /// Registry key, and the token a scope names.
    fn name(&self) -> &'static str;
    /// Turn one composed scope into a compilation unit.
    fn synthesize(
        &self,
        view: &lunco_usd_bevy::StageView<'_>,
        root: &SdfPath,
        model_name: &str,
        ctx: &SynthContext<'_>,
    ) -> Result<SynthOutcome, Vec<DomainProjectionError>>;
}

/// What a synthesizer concluded about a scope.
#[derive(Debug)]
pub enum SynthOutcome {
    /// Not a scope this synthesizer compiles (or nothing solvable is in it).
    NotMine,
    /// Cannot be decided yet — a member's source has not loaded, so the class it
    /// declares is not knowable. The projection simply waits and is re-triggered
    /// when the source lands.
    Pending,
    Ready(SynthesisPlan),
}

/// Read-only facts a synthesizer may need beyond the stage itself.
pub struct SynthContext<'a> {
    /// Class-per-source-asset, as declared BY THE FILE. See [`MemberClasses`].
    pub classes: &'a MemberClasses,
}

/// The synthesizer a scope names with `lunco:synthesizer`, or the default.
pub const DEFAULT_SYNTHESIZER: &str = "acausal-network";
/// The generic force-actuator allocator used by the shipped lander.
pub const ACTUATOR_WRENCH_SYNTHESIZER: &str = "actuator-wrench";

/// Open registry of synthesizers, by name. No enum: a new domain is a
/// registration from any plugin.
#[derive(Resource)]
pub struct SynthesizerRegistry(
    std::collections::BTreeMap<String, std::sync::Arc<dyn DomainSynthesizer>>,
);

impl Default for SynthesizerRegistry {
    fn default() -> Self {
        let mut registry = Self(Default::default());
        registry.register(AcausalNetworkSynthesizer);
        registry.register(ActuatorWrenchSynthesizer);
        registry
    }
}

impl SynthesizerRegistry {
    pub fn register(&mut self, synthesizer: impl DomainSynthesizer) {
        self.0.insert(
            synthesizer.name().to_string(),
            std::sync::Arc::new(synthesizer),
        );
    }
    pub fn get(&self, name: &str) -> Option<&std::sync::Arc<dyn DomainSynthesizer>> {
        self.0.get(name)
    }
    pub fn names(&self) -> Vec<&str> {
        self.0.keys().map(String::as_str).collect()
    }
}

/// A synthesizer whose EMIT policy is authored, not compiled in.
///
/// The house split, applied to synthesis: **facts in Rust, rules in rhai.** The
/// composed graph is read here — membership, connectors, causal edges, the
/// boundary, the class each member's file declares — and handed to a hook as a
/// map. What that graph becomes in Modelica is the hook's business: which MSL
/// class stands in for a part, whether a fuse is inserted, whether a low-fidelity
/// variant omits parasitic resistance. None of that is a Rust concern, and none
/// of it should require a rebuild to change.
///
/// The hook receives one argument — [`network_facts`] — and returns a map with
/// a required `source` key (room for a policy to report its own diagnostics
/// later). A single result shape keeps the extension boundary typed for every
/// authored emitter.
///
/// Registered through [`register_hook_synthesizer`]; the hook id is by convention
/// `synth.<name>`, reached exactly like `lint.usd`.
pub struct HookSynthesizer {
    name: &'static str,
    hook_id: String,
}

impl DomainSynthesizer for HookSynthesizer {
    fn name(&self) -> &'static str {
        self.name
    }
    fn synthesize(
        &self,
        view: &lunco_usd_bevy::StageView<'_>,
        root: &SdfPath,
        model_name: &str,
        ctx: &SynthContext<'_>,
    ) -> Result<SynthOutcome, Vec<DomainProjectionError>> {
        // The READER is not the policy's business — a rhai body that had to
        // re-walk USD would be a second, divergent definition of what a network
        // is, which is the exact failure the one-reader rule exists to prevent.
        let Some(network) = read_network(view, root, ctx.classes)? else {
            return Ok(SynthOutcome::NotMine);
        };
        if network.pending_sources {
            return Ok(SynthOutcome::Pending);
        }
        let network_root = network.root.clone();
        let facts = network_facts(&network, model_name);
        let result = lunco_hooks::invoke(&self.hook_id, &[facts]).ok_or_else(|| {
            vec![DomainProjectionError {
                path: network_root.clone(),
                message: format!(
                    "synthesizer `{}` is selected but its hook `{}` is not registered",
                    self.name, self.hook_id
                ),
            }]
        })?;
        let value = result.map_err(|error| {
            vec![DomainProjectionError {
                path: network_root.clone(),
                message: format!("synthesizer `{}` failed: {}", self.name, error.0),
            }]
        })?;
        let lunco_hooks::HookValue::Map(map) = &value else {
            return Err(vec![DomainProjectionError {
                path: network_root,
                message: format!(
                    "synthesizer `{}` must return a map with a Modelica `source` key",
                    self.name
                ),
            }]);
        };
        let Some(source) = map
            .iter()
            .find_map(|(key, value)| (key == "source").then(|| value.as_str()))
            .flatten()
        else {
            return Err(vec![DomainProjectionError {
                path: network_root,
                message: format!(
                    "synthesizer `{}` returned a map with no string `source` key",
                    self.name
                ),
            }]);
        };
        Ok(SynthOutcome::Ready(SynthesisPlan {
            source: source.to_string(),
            // The BOUNDARY and unit partition stay Rust's answer even when the
            // emitter body is authored: they are the runtime contract and the
            // topology facts supplied to the policy.
            inputs: network.inputs.clone(),
            outputs: network.outputs.keys().cloned().collect(),
            component_paths: network
                .components
                .iter()
                .map(|component| component.path.clone())
                .collect(),
            members: network
                .components
                .iter()
                .map(|component| {
                    (
                        component.path.clone(),
                        component.source_asset.clone(),
                        component.model_class.clone(),
                    )
                })
                .collect(),
            units: partition_network(&network),
        }))
    }
}

/// Register an authored synthesizer under `name`, backed by hook `synth.<name>`.
///
/// The hook itself is registered by whatever compiled it — `lunco_hooks_rhai::register_rhai_hook`
/// for a rhai policy — so this crate needs no scripting dependency and any
/// language that implements [`lunco_hooks::ScriptHook`] can author one.
pub fn register_hook_synthesizer(registry: &mut SynthesizerRegistry, name: &'static str) {
    registry.register(HookSynthesizer {
        name,
        hook_id: format!("synth.{name}"),
    });
}

/// The composed network, as a map an authored policy can read.
///
/// Deliberately the WHOLE graph, flat and self-describing: instance name, class,
/// constants, acausal edges, causal edges, and the wrapper boundary. A policy
/// that needs something not in here is a reason to extend this function — not a
/// reason for the policy to go read USD itself.
pub fn network_facts(network: &DomainNetwork, model_name: &str) -> lunco_hooks::HookValue {
    use lunco_hooks::HookValue as H;
    let components: Vec<H> = network
        .components
        .iter()
        .map(|component| {
            H::map([
                ("path", H::str(component.path.clone())),
                (
                    "instance",
                    H::str(instance_identifier(&network.root, &component.path)),
                ),
                ("class", H::str(component.model_class.clone())),
                ("source_asset", H::str(component.source_asset.clone())),
                (
                    "constants",
                    H::Map(
                        component
                            .constants
                            .iter()
                            .map(|(name, value)| (name.clone(), H::Float(*value)))
                            .collect(),
                    ),
                ),
                (
                    "connectors",
                    H::Map(
                        component
                            .connectors
                            .iter()
                            .map(|(name, targets)| {
                                (
                                    name.clone(),
                                    H::Array(targets.iter().cloned().map(H::str).collect()),
                                )
                            })
                            .collect(),
                    ),
                ),
                (
                    "declared_connectors",
                    H::Array(
                        component
                            .declared_connectors
                            .iter()
                            .cloned()
                            .map(H::str)
                            .collect(),
                    ),
                ),
                (
                    "inputs",
                    H::Map(
                        component
                            .inputs
                            .iter()
                            .map(|(name, target)| (name.clone(), H::str(target.clone())))
                            .collect(),
                    ),
                ),
                (
                    "declared_outputs",
                    H::Array(
                        component
                            .declared_outputs
                            .iter()
                            .cloned()
                            .map(H::str)
                            .collect(),
                    ),
                ),
            ])
        })
        .collect();
    H::map([
        ("model_name", H::str(model_name.to_string())),
        ("root", H::str(network.root.clone())),
        ("components", H::Array(components)),
        (
            "inputs",
            H::Array(network.inputs.iter().cloned().map(H::str).collect()),
        ),
        (
            "input_sources",
            H::Map(
                network
                    .input_sources
                    .iter()
                    .map(|(name, source)| (name.clone(), H::str(source.clone())))
                    .collect(),
            ),
        ),
        (
            "outputs",
            H::Map(
                network
                    .outputs
                    .iter()
                    .map(|(name, target)| (name.clone(), H::str(target.clone())))
                    .collect(),
            ),
        ),
        (
            "units",
            H::Array(
                partition_network(network)
                    .into_iter()
                    .map(|unit| {
                        H::map([
                            ("name", H::str(unit.name)),
                            (
                                "components",
                                H::Array(unit.component_paths.into_iter().map(H::str).collect()),
                            ),
                            (
                                "inputs",
                                H::Array(unit.inputs.into_iter().map(H::str).collect()),
                            ),
                            (
                                "outputs",
                                H::Array(unit.outputs.into_iter().map(H::str).collect()),
                            ),
                        ])
                    })
                    .collect(),
            ),
        ),
    ])
}

/// Partition a composed network once, at the synthesizer boundary.
///
/// Acausal connector edges and internal causal output-to-input edges both keep
/// components in one composite unit. Boundary connections do not: they are
/// the public FMI/SSP-style interface of the unit's containing Scope. The
/// returned order and generated names are stable across runs and independent
/// of USD collection ordering.
pub fn partition_network(network: &DomainNetwork) -> Vec<SynthesisUnit> {
    let paths: BTreeSet<String> = network
        .components
        .iter()
        .map(|component| component.path.clone())
        .collect();
    let mut graph = ProgramGraph::default();
    for path in &paths {
        graph.add_node(path.clone());
    }
    for component in &network.components {
        for target in component.connectors.values().flatten() {
            if let Some((target_prim, _)) = target.split_once(".connectors:") {
                if paths.contains(target_prim) {
                    graph.connect(component.path.clone(), target_prim.to_string());
                }
            }
        }
        for target in component.inputs.values() {
            if let Some((target_prim, _)) = target.split_once(".outputs:") {
                if paths.contains(target_prim) {
                    graph.connect(component.path.clone(), target_prim.to_string());
                }
            }
        }
    }

    let component_by_path: BTreeMap<_, _> = network
        .components
        .iter()
        .map(|component| (component.path.as_str(), component))
        .collect();
    graph
        .connected_components()
        .into_iter()
        .map(|component_paths| {
            let members: BTreeSet<_> = component_paths.iter().map(String::as_str).collect();
            let inputs = network
                .components
                .iter()
                .filter(|component| members.contains(component.path.as_str()))
                .flat_map(|component| component.inputs.values())
                .filter_map(|target| network_boundary_for_target(network, target))
                .collect();
            let outputs = network
                .outputs
                .iter()
                .filter_map(|(name, target)| {
                    let (target_prim, _) = target.split_once(".outputs:")?;
                    members.contains(target_prim).then(|| name.clone())
                })
                .collect();
            let first = component_paths
                .first()
                .expect("connected component always has a seed");
            let relative = first
                .strip_prefix(&network.root)
                .unwrap_or(first)
                .trim_matches('/');
            let name = format!("Unit_{}", modelica_path_identifier(relative));
            debug_assert!(component_paths
                .iter()
                .all(|path| component_by_path.contains_key(path.as_str())));
            SynthesisUnit {
                name,
                component_paths,
                inputs,
                outputs,
            }
        })
        .collect()
}

fn network_boundary_for_target(network: &DomainNetwork, target: &str) -> Option<String> {
    let prefix = format!("{}.inputs:", network.root);
    target
        .strip_prefix(&prefix)
        .map(str::to_string)
        .or_else(|| {
            network
                .input_sources
                .iter()
                .find_map(|(boundary, source)| (source == target).then(|| boundary.clone()))
        })
}

/// The built-in: a `CollectionAPI:components` scope of Modelica program facets
/// wired by `connectors:*` (acausal) and `inputs:`/`outputs:` (causal) becomes
/// one DAE. This is the electrical/thermal/hydraulic shape — every physical
/// domain whose parts share a potential/flow pair.
pub struct AcausalNetworkSynthesizer;

impl DomainSynthesizer for AcausalNetworkSynthesizer {
    fn name(&self) -> &'static str {
        DEFAULT_SYNTHESIZER
    }
    fn synthesize(
        &self,
        view: &lunco_usd_bevy::StageView<'_>,
        root: &SdfPath,
        model_name: &str,
        ctx: &SynthContext<'_>,
    ) -> Result<SynthOutcome, Vec<DomainProjectionError>> {
        let Some(network) = read_network(view, root, ctx.classes)? else {
            return Ok(SynthOutcome::NotMine);
        };
        if network.pending_sources {
            return Ok(SynthOutcome::Pending);
        }
        let member_outputs = generated_member_outputs(&network, Some(ctx.classes));
        let outputs = network
            .outputs
            .keys()
            .cloned()
            .chain(member_outputs.iter().map(|(_, _, alias)| alias.clone()))
            .collect();
        Ok(SynthOutcome::Ready(SynthesisPlan {
            source: emit_modelica_with_classes(&network, model_name, Some(ctx.classes)),
            inputs: network.inputs.clone(),
            outputs,
            component_paths: network
                .components
                .iter()
                .map(|component| component.path.clone())
                .collect(),
            members: network
                .components
                .iter()
                .map(|component| {
                    (
                        component.path.clone(),
                        component.source_asset.clone(),
                        component.model_class.clone(),
                    )
                })
                .collect(),
            units: partition_network(&network),
        }))
    }
}

/// Synthesize a normalized actuator command map from composed USD geometry.
///
/// This is deliberately a separate synthesizer from `acausal-network`: force
/// actuator prims are physical USD members, not Modelica component facets. The
/// authored geometry supplies each actuator's moment contribution and command
/// name; Modelica owns the runtime clamp and matrix operation. A rank-deficient
/// actuator arrangement is an authoring error, not a reason to silently select
/// a different allocation policy.
pub struct ActuatorWrenchSynthesizer;

impl DomainSynthesizer for ActuatorWrenchSynthesizer {
    fn name(&self) -> &'static str {
        ACTUATOR_WRENCH_SYNTHESIZER
    }

    fn synthesize(
        &self,
        view: &lunco_usd_bevy::StageView<'_>,
        root: &SdfPath,
        model_name: &str,
        _ctx: &SynthContext<'_>,
    ) -> Result<SynthOutcome, Vec<DomainProjectionError>> {
        if view.type_name(root).as_deref() != Some("Scope")
            || !view.has_api_schema(root, "CollectionAPI:components")
        {
            return Ok(SynthOutcome::NotMine);
        }

        let root_string = root.to_string();
        let members = view
            .collection_members(root, "components")
            .map_err(|error| {
                vec![DomainProjectionError {
                    path: root_string.clone(),
                    message: format!("could not read actuator collection: {error}"),
                }]
            })?;

        let mut actuators = BTreeMap::new();
        for path in members {
            if path.is_property_path() || path.is_prim_variant_selection_path() {
                continue;
            }
            let Some(command) = view.text(&path, "lunco:forceActuator:commandOutput") else {
                return Err(vec![DomainProjectionError {
                    path: path.to_string(),
                    message: "actuator-wrench collection members must author a non-empty \
                              lunco:forceActuator:commandOutput"
                        .into(),
                }]);
            };
            if command.is_empty() || !is_modelica_identifier(&command) {
                return Err(vec![DomainProjectionError {
                    path: format!("{path}.lunco:forceActuator:commandOutput"),
                    message: format!("`{command}` is not a valid Modelica actuator output name"),
                }]);
            }
            let Some(actuator) = crate::force_actuator_from_usd(view, &path) else {
                return Err(vec![DomainProjectionError {
                    path: path.to_string(),
                    message: "actuator-wrench member is not a valid force actuator with a \
                              rigid-body owner, finite direction, and positive maxForce"
                        .into(),
                }]);
            };
            if actuators
                .insert(command.clone(), (path.clone(), actuator))
                .is_some()
            {
                return Err(vec![DomainProjectionError {
                    path: format!("{root}.lunco:forceActuator:commandOutput"),
                    message: format!("actuator output `{command}` is authored more than once"),
                }]);
            }
        }
        if actuators.is_empty() {
            return Err(vec![DomainProjectionError {
                path: root.to_string(),
                message: "actuator-wrench collection contains no force actuators".into(),
            }]);
        }

        let inputs: BTreeSet<String> = view
            .attr_names(root)
            .into_iter()
            .filter_map(|attr| attr.strip_prefix("inputs:").map(strip_connection_suffix))
            .filter(|name| !name.is_empty())
            .collect();
        let outputs: BTreeSet<String> = view
            .attr_names(root)
            .into_iter()
            .filter_map(|attr| attr.strip_prefix("outputs:").map(strip_connection_suffix))
            .filter(|name| !name.is_empty())
            .collect();
        for name in inputs.iter().chain(outputs.iter()) {
            if !is_modelica_identifier(name) {
                return Err(vec![DomainProjectionError {
                    path: root.to_string(),
                    message: format!("public port `{name}` is not a valid Modelica identifier"),
                }]);
            }
        }
        let actuator_outputs: BTreeSet<_> = actuators.keys().cloned().collect();
        if actuator_outputs != outputs {
            return Err(vec![DomainProjectionError {
                path: root.to_string(),
                message: format!(
                    "actuator command outputs {:?} do not match the authored network outputs {:?}",
                    actuator_outputs, outputs
                ),
            }]);
        }

        let columns: Vec<_> = actuators.values().map(|(_, actuator)| *actuator).collect();
        let allocation = actuator_torque_pseudoinverse(&columns).map_err(|message| {
            vec![DomainProjectionError {
                path: root_string.clone(),
                message,
            }]
        })?;
        let source = emit_actuator_wrench_model(model_name, &inputs, &outputs, &allocation);
        let component_paths = actuators
            .values()
            .map(|(path, _)| path.to_string())
            .collect::<Vec<_>>();
        let unit = SynthesisUnit {
            name: format!("{model_name}_ActuatorWrench"),
            component_paths: component_paths.clone(),
            inputs: inputs.clone(),
            outputs: outputs.clone(),
        };
        Ok(SynthOutcome::Ready(SynthesisPlan {
            source,
            inputs,
            outputs,
            component_paths,
            members: Vec::new(),
            units: vec![unit],
        }))
    }
}

fn strip_connection_suffix(name: &str) -> String {
    name.strip_suffix(".connect").unwrap_or(name).to_string()
}

/// Return `Bᵀ (B Bᵀ)⁻¹`, where each column of `B` is one actuator's maximum
/// body torque. The three torque axes are intentionally the only requested
/// wrench dimensions; force dimensions remain zero in the six-column
/// `WrenchAllocator` contract.
fn actuator_torque_pseudoinverse(
    actuators: &[lunco_cosim::ForceActuator],
) -> Result<Vec<[f64; 3]>, String> {
    let columns: Vec<[f64; 3]> = actuators
        .iter()
        .map(|actuator| {
            let direction = actuator.direction_local.normalize_or_zero().as_dvec3();
            let torque = actuator.local_position.as_dvec3().cross(direction) * actuator.max_force_n;
            [torque.x, torque.y, torque.z]
        })
        .collect();
    if columns.iter().flatten().any(|value| !value.is_finite()) {
        return Err("actuator-wrench geometry produced a non-finite torque column".into());
    }

    let mut gram = [[0.0; 3]; 3];
    for column in &columns {
        for row in 0..3 {
            for col in 0..3 {
                gram[row][col] += column[row] * column[col];
            }
        }
    }
    let a = gram[0][0];
    let b = gram[0][1];
    let c = gram[0][2];
    let d = gram[1][0];
    let e = gram[1][1];
    let f = gram[1][2];
    let g = gram[2][0];
    let h = gram[2][1];
    let i = gram[2][2];
    let cofactors = [
        [e * i - f * h, c * h - b * i, b * f - c * e],
        [f * g - d * i, a * i - c * g, c * d - a * f],
        [d * h - e * g, b * g - a * h, a * e - b * d],
    ];
    let determinant = a * cofactors[0][0] + b * cofactors[0][1] + c * cofactors[0][2];
    let scale = gram
        .iter()
        .flatten()
        .map(|value| value.abs())
        .fold(0.0, f64::max);
    if !determinant.is_finite() || scale == 0.0 || determinant.abs() <= scale.powi(3) * 1.0e-12 {
        return Err(
            "actuator-wrench geometry cannot produce all three body torque axes; \
             the authored force arrangement is rank deficient"
                .into(),
        );
    }
    let inverse: [[f64; 3]; 3] =
        std::array::from_fn(|row| std::array::from_fn(|col| cofactors[col][row] / determinant));
    Ok(columns
        .iter()
        .map(|column| {
            std::array::from_fn(|col| {
                (0..3)
                    .map(|row| column[row] * inverse[row][col])
                    .sum::<f64>()
            })
        })
        .collect())
}

fn emit_actuator_wrench_model(
    model_name: &str,
    inputs: &BTreeSet<String>,
    outputs: &BTreeSet<String>,
    allocation: &[[f64; 3]],
) -> String {
    let mut source = format!("model {model_name}\n");
    for input in inputs {
        writeln!(source, "  input Real {input};").expect("String write");
    }
    for output in outputs {
        writeln!(source, "  output Real {output};").expect("String write");
    }
    source.push_str("  LunCo.Actuation.WrenchAllocator allocator(\n");
    writeln!(source, "    actuator_count = {},", allocation.len()).expect("String write");
    source.push_str("    allocation_pinv = [");
    for (index, row) in allocation.iter().enumerate() {
        if index > 0 {
            source.push_str("; ");
        }
        write!(
            source,
            "0.0, 0.0, 0.0, {:.12}, {:.12}, {:.12}",
            row[0], row[1], row[2]
        )
        .expect("String write");
    }
    source.push_str("],\n    lower_command = {");
    source.push_str(&vec!["0.0"; allocation.len()].join(", "));
    source.push_str("},\n    upper_command = {");
    source.push_str(&vec!["1.0"; allocation.len()].join(", "));
    source.push_str("});\n\nequation\n");

    for name in [
        "desired_force_x",
        "desired_force_y",
        "desired_force_z",
        "desired_torque_x",
        "desired_torque_y",
        "desired_torque_z",
    ] {
        let value = if inputs.contains(name) {
            name.to_string()
        } else {
            "0.0".to_string()
        };
        writeln!(source, "  allocator.{name} = {value};").expect("String write");
    }
    for (index, output) in outputs.iter().enumerate() {
        writeln!(source, "  {output} = allocator.command[{}];", index + 1).expect("String write");
    }
    writeln!(source, "end {model_name};").expect("String write");
    source
}

/// Place generated components by network topology, not by source-file order.
///
/// Causal edges flow from an output to a consumer; acausal connector edges are
/// bidirectional. We start at sources (or graph roots), breadth-first layer the
/// graph, then sort each layer by stable composed path. The emitted Modelica
/// `Placement` annotations let every regular Modelica diagram consumer render
/// the same legible energy/thermal flow without a generated-network-only UI.
fn network_layout(network: &DomainNetwork) -> BTreeMap<String, (i32, i32)> {
    let paths: BTreeSet<_> = network.components.iter().map(|c| c.path.clone()).collect();
    let mut neighbours: BTreeMap<String, BTreeSet<String>> = paths
        .iter()
        .map(|path| (path.clone(), BTreeSet::new()))
        .collect();
    let mut incoming: BTreeMap<String, usize> =
        paths.iter().map(|path| (path.clone(), 0)).collect();
    for component in &network.components {
        for target in component.connectors.values().flatten() {
            if let Some((target, _)) = target.split_once(".connectors:") {
                if paths.contains(target) {
                    neighbours
                        .get_mut(&component.path)
                        .expect("component path indexed")
                        .insert(target.to_string());
                    neighbours
                        .get_mut(target)
                        .expect("component path indexed")
                        .insert(component.path.clone());
                }
            }
        }
        for target in component.inputs.values() {
            if let Some((source, _)) = target.split_once(".outputs:") {
                if paths.contains(source) {
                    neighbours
                        .get_mut(source)
                        .expect("component path indexed")
                        .insert(component.path.clone());
                    *incoming
                        .get_mut(&component.path)
                        .expect("component path indexed") += 1;
                }
            }
        }
    }
    let mut roots: Vec<_> = network
        .components
        .iter()
        .filter(|component| {
            incoming[&component.path] == 0
                && (component.model_class.contains("Battery")
                    || component.model_class.contains("Solar")
                    || component.model_class.contains("Source"))
        })
        .map(|component| component.path.clone())
        .collect();
    if roots.is_empty() {
        roots = network
            .components
            .iter()
            .filter(|component| incoming[&component.path] == 0)
            .map(|component| component.path.clone())
            .collect();
    }
    if roots.is_empty() {
        roots.extend(paths.iter().take(1).cloned());
    }
    roots.sort();
    let mut rank = BTreeMap::new();
    let mut queue: VecDeque<_> = roots.into_iter().map(|path| (path, 0usize)).collect();
    while let Some((path, layer)) = queue.pop_front() {
        if rank.contains_key(&path) {
            continue;
        }
        rank.insert(path.clone(), layer);
        for neighbour in &neighbours[&path] {
            if !rank.contains_key(neighbour) {
                queue.push_back((neighbour.clone(), layer + 1));
            }
        }
    }
    let fallback = rank.values().copied().max().unwrap_or(0) + 1;
    for path in paths {
        rank.entry(path).or_insert(fallback);
    }
    let mut layers: BTreeMap<usize, Vec<String>> = BTreeMap::new();
    for (path, layer) in rank {
        layers.entry(layer).or_default().push(path);
    }
    let mut placements = BTreeMap::new();
    for (layer, paths) in layers {
        let count = paths.len() as i32;
        for (row, path) in paths.into_iter().enumerate() {
            placements.insert(
                path,
                (-100 + layer as i32 * 55, (count - 1 - row as i32 * 2) * 22),
            );
        }
    }
    placements
}

/// Emit one deterministic composite Modelica wrapper for a composed network
/// Scope. The wrapper is the runtime participant; its generated child models
/// are the synthesizer-owned connected units.
pub fn emit_modelica(network: &DomainNetwork, model_name: &str) -> String {
    emit_modelica_with_classes(network, model_name, None)
}

fn emit_modelica_with_classes(
    network: &DomainNetwork,
    model_name: &str,
    classes: Option<&MemberClasses>,
) -> String {
    let root_name = modelica_identifier(model_name);
    let units = partition_network(network);
    let names: BTreeMap<_, _> = network
        .components
        .iter()
        .map(|component| {
            (
                component.path.as_str(),
                instance_identifier(&network.root, &component.path),
            )
        })
        .collect();
    let unit_instances: BTreeMap<_, _> = units
        .iter()
        .map(|unit| (unit.name.as_str(), unit_instance_identifier(&unit.name)))
        .collect();
    let unit_for_component: BTreeMap<_, _> = units
        .iter()
        .flat_map(|unit| {
            unit.component_paths
                .iter()
                .map(move |path| (path.as_str(), unit))
        })
        .collect();
    let boundary_by_source: BTreeMap<_, _> = network
        .input_sources
        .iter()
        .map(|(boundary, source)| (source.as_str(), boundary.as_str()))
        .collect();
    let placements = network_layout(network);
    let member_outputs = generated_member_outputs(network, classes);
    let mut source = format!("model {root_name}\n");

    for input in &network.inputs {
        source.push_str(&format!("  input Real {};\n", modelica_identifier(input)));
    }
    for output in network.outputs.keys() {
        source.push_str(&format!("  output Real {};\n", modelica_identifier(output)));
    }
    for (_, _, alias) in &member_outputs {
        source.push_str(&format!("  output Real {alias};\n"));
    }
    for component in &network.components {
        source.push_str(&format!("  // USD: {}\n", component.path));
    }
    for unit in &units {
        source.push_str(&format!(
            "  {} {};\n",
            unit.name,
            unit_instances[unit.name.as_str()]
        ));
    }
    source.push_str("equation\n");
    for unit in &units {
        let instance = &unit_instances[unit.name.as_str()];
        for input in &unit.inputs {
            source.push_str(&format!(
                "  {instance}.{} = {};\n",
                modelica_identifier(input),
                modelica_identifier(input)
            ));
        }
        for output in &unit.outputs {
            source.push_str(&format!(
                "  {} = {instance}.{};\n",
                modelica_identifier(output),
                modelica_identifier(output)
            ));
        }
    }
    // Member aliases are wrapper outputs, so their equations belong to the
    // wrapper's equation section. Keeping them here also keeps the complete
    // generated root model syntactically self-contained.
    for (member, _output, alias) in &member_outputs {
        if let Some(unit) = unit_for_component.get(member.as_str()) {
            let unit_instance = &unit_instances[unit.name.as_str()];
            source.push_str(&format!("  {alias} = {unit_instance}.{alias};\n"));
        }
    }
    // Generated networks are ordinary Modelica documents. Their assembly
    // annotation provides a stable canvas banner; component Icons and
    // Placement annotations remain the source of the actual network picture.
    source.push_str(&format!(
        "annotation(Icon(coordinateSystem(extent={{{{-100,-100}},{{100,100}}}}), graphics={{Rectangle(extent={{{{-82,-58}},{{82,58}}}}, lineColor={{70,95,150}}, fillColor={{125,155,215}}, fillPattern=FillPattern.Solid, radius=10), Text(extent={{{{-75,-20}},{{75,20}}}}, textString=\"USD NET\", textColor={{245,250,255}}, fontSize=18)}}), Diagram(coordinateSystem(extent={{{{-240,-180}},{{240,180}}}}), graphics={{Text(extent={{{{-220,150}},{{220,175}}}}, textString=\"USD COMPOSED NETWORK: {model_name}\", textColor={{95,125,190}}, fontSize=12)}}));\n"
    ));
    source.push_str(&format!("end {root_name};\n\n"));
    for unit in &units {
        source.push_str(&format!("model {}\n", unit.name));
        for input in &unit.inputs {
            source.push_str(&format!("  input Real {};\n", modelica_identifier(input)));
        }
        for output in &unit.outputs {
            source.push_str(&format!("  output Real {};\n", modelica_identifier(output)));
        }
        for (member, _, alias) in &member_outputs {
            if unit.component_paths.iter().any(|path| path == member) {
                source.push_str(&format!("  output Real {alias};\n"));
            }
        }
        source.push_str(&format!(
            "  // Synthesis unit members: {}\n",
            unit.component_paths.join(", ")
        ));

        for path in &unit.component_paths {
            let component = network
                .components
                .iter()
                .find(|component| component.path == *path)
                .expect("partitioned component exists in the network");
            let instance = &names[component.path.as_str()];
            source.push_str(&format!("  // USD: {}\n", component.path));
            source.push_str(&format!("  {} {}", component.model_class, instance));
            if !component.constants.is_empty() {
                source.push('(');
                for (index, (name, value)) in component.constants.iter().enumerate() {
                    if index > 0 {
                        source.push_str(", ");
                    }
                    source.push_str(&modelica_identifier(name));
                    source.push_str(" = ");
                    source.push_str(&value.to_string());
                }
                source.push(')');
            }
            let (x, y) = placements[&component.path];
            source.push_str(&format!(
                " annotation(Placement(transformation(origin = {{{x}, {y}}}, extent = {{{{-10, -10}}, {{10, 10}}}})));\n"
            ));
        }

        source.push_str("equation\n");
        let mut emitted_edges = BTreeSet::new();
        for path in &unit.component_paths {
            let component = network
                .components
                .iter()
                .find(|component| component.path == *path)
                .expect("partitioned component exists in the network");
            let local_instance = &names[component.path.as_str()];
            for (connector, targets) in &component.connectors {
                for target in targets {
                    let Some((target_prim, target_connector)) = target.split_once(".connectors:")
                    else {
                        continue;
                    };
                    let Some(target_instance) = names.get(target_prim) else {
                        continue;
                    };
                    let left = format!("{local_instance}.{}", modelica_identifier(connector));
                    let right = format!(
                        "{target_instance}.{}",
                        modelica_identifier(target_connector)
                    );
                    let edge = if left <= right {
                        (left, right)
                    } else {
                        (right, left)
                    };
                    if emitted_edges.insert(edge.clone()) {
                        source.push_str(&format!("  connect({}, {});\n", edge.0, edge.1));
                    }
                }
            }
            for (input, target) in &component.inputs {
                let boundary = network_boundary_for_target(network, target).or_else(|| {
                    boundary_by_source
                        .get(target.as_str())
                        .map(|v| v.to_string())
                });
                if let Some(boundary) = boundary {
                    source.push_str(&format!(
                        "  {local_instance}.{} = {};\n",
                        modelica_identifier(input),
                        modelica_identifier(&boundary)
                    ));
                } else if let Some((target_prim, output)) = target.split_once(".outputs:") {
                    if let Some(target_instance) = names.get(target_prim) {
                        source.push_str(&format!(
                            "  {local_instance}.{} = {target_instance}.{};\n",
                            modelica_identifier(input),
                            modelica_identifier(output)
                        ));
                    }
                }
            }
        }
        for (output, target) in &network.outputs {
            let Some((target_prim, member)) = target.split_once(".outputs:") else {
                continue;
            };
            if unit.component_paths.iter().any(|path| path == target_prim) {
                let instance = &names[target_prim];
                source.push_str(&format!(
                    "  {} = {instance}.{};\n",
                    modelica_identifier(output),
                    modelica_identifier(member)
                ));
            }
        }
        for (member, output, alias) in &member_outputs {
            if unit.component_paths.iter().any(|path| path == member) {
                let instance = &names[member.as_str()];
                source.push_str(&format!(
                    "  {alias} = {instance}.{};\n",
                    modelica_identifier(output)
                ));
            }
        }
        source.push_str(&format!("end {};\n\n", unit.name));
    }

    // The lookup is deliberately exercised for every authored boundary output:
    // a malformed cross-unit output is diagnosed by `validate_network`, not
    // silently redirected to another unit.
    for (output, target) in &network.outputs {
        if let Some((target_prim, _)) = target.split_once(".outputs:") {
            debug_assert!(unit_for_component.contains_key(target_prim));
            debug_assert!(unit_for_component
                .get(target_prim)
                .is_some_and(|unit| unit.outputs.contains(output)));
        }
    }
    source
}

/// Stable Modelica name for a causal output promoted from a generated member.
///
/// The wrapper is the only runtime solver participant, so member outputs that
/// remain visible in USD need a first-class boundary name. The prefix keeps
/// these derived names separate from authored network outputs; the escaped
/// instance identifier keeps the mapping injective for arbitrary USD paths.
pub(crate) fn generated_member_output_name(root: &str, member: &str, output: &str) -> String {
    format!(
        "__member_{}_{}",
        instance_identifier(root, member),
        modelica_identifier(output)
    )
}

fn generated_member_outputs(
    network: &DomainNetwork,
    classes: Option<&MemberClasses>,
) -> Vec<(String, String, String)> {
    network
        .components
        .iter()
        .flat_map(|component| {
            let modelica_outputs =
                classes.and_then(|classes| classes.output_names(&component.source_asset));
            component
                .declared_outputs
                .iter()
                .filter(move |output| {
                    modelica_outputs.is_none_or(|outputs| outputs.contains(*output))
                })
                .map(|output| {
                    (
                        component.path.clone(),
                        output.clone(),
                        generated_member_output_name(&network.root, &component.path, output),
                    )
                })
        })
        .collect()
}

/// Reactively compile every ordinary `Scope` containing a standard component
/// collection of Modelica program facets. The generated source is runtime projection only.
pub fn project_domain_islands(
    mut commands: Commands,
    added: Query<(), Added<UsdPrimPath>>,
    identity_added: Query<(), Added<lunco_core::GlobalEntityId>>,
    prims: Query<(
        Entity,
        &UsdPrimPath,
        Option<&DomainProjectionState>,
        Option<&ModelicaModel>,
    )>,
    q_gid: Query<&lunco_core::GlobalEntityId>,
    q_provenance: Query<&lunco_core::Provenance>,
    q_instance_root: Query<(), With<lunco_usd_bevy::UsdInstanceRoot>>,
    // A runtime-instanced descendant is PARKED as `Provenance::Local` until its
    // root's id is minted (`resolve_usd_instance_identities`), and `instance_key`
    // answers `None` for the whole window. Projecting then compiles the island
    // under an unqualified name — which the second, identity-carrying pass
    // immediately supersedes (a wasted serial compile ahead of everything on the
    // critical path), and which two spawns of one asset SHARE, so their
    // `generated://…` worker sessions clobber each other. The marker is the
    // explicit "identity still pending" signal; it is removed on upgrade.
    q_instance_member: Query<(), With<lunco_usd_bevy::UsdInstanceMember>>,
    stages: Res<Assets<UsdStageAsset>>,
    mut canonical: NonSendMut<CanonicalStages>,
    dirty: Res<WiringDirty>,
    // A member class landing is the projector's third trigger: the networks that
    // returned `Pending` have to be re-asked, and no prim spawned or changed.
    mut projection_dirty: ResMut<ProjectionDirty>,
    classes: Res<MemberClasses>,
    registry: Res<SynthesizerRegistry>,
    channels: Option<Res<ModelicaChannels>>,
    mut notices: MessageWriter<ModelicaNotice>,
) {
    if added.is_empty() && identity_added.is_empty() && !dirty.0 && !projection_dirty.0 {
        return;
    }
    projection_dirty.0 = false;
    let Some(channels) = channels else { return };
    for (entity, prim, previous, installed_model) in &prims {
        // Scope every authored path to the same USD instance as the generated
        // network. Runtime-spawned copies intentionally share stage-relative
        // paths; the instance root identity is the structural disambiguator.
        let instance_id =
            lunco_usd_bevy::instance_key(entity, &q_provenance, &q_gid, &q_instance_root);
        // Identity still pending — wait for it rather than compile under a name
        // that is neither stable nor unique. The upgrade lands a
        // `GlobalEntityId`, which re-triggers this system through
        // `identity_added`.
        if q_instance_member.contains(entity) {
            continue;
        }
        // Runtime-spawned copies may have byte-identical stage-relative paths.
        // Use the same stable instance-root identity as the USD wiring resolver;
        // scene-owned prims need no suffix because their composed paths are unique.
        let id = prim.stage_handle.id();
        if canonical.get(id).is_none() {
            if let Some(recipe) = stages
                .get(&prim.stage_handle)
                .and_then(|stage| stage.recipe.clone())
            {
                canonical.get_or_build(id, &recipe);
            }
        }
        let Some(stage) = canonical.get(id) else {
            continue;
        };
        let view = stage.view();
        let Ok(root_path) = SdfPath::new(&prim.path) else {
            continue;
        };
        if view.type_name(&root_path).as_deref() != Some("Scope") {
            continue;
        }
        // WHICH synthesizer turns this scope into a model is authored, not
        // hardcoded: `lunco:synthesizer` names one from the open registry, and
        // absent means the acausal-network default. An unknown name is an
        // authoring error, not a silent fallback to some other domain's rules.
        let requested = view
            .text(&root_path, "lunco:synthesizer")
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| DEFAULT_SYNTHESIZER.to_string());
        let Some(synthesizer) = registry.get(&requested).cloned() else {
            let known = registry.names().join(", ");
            error!(
                "[domain-projection] `{}` names synthesizer `{requested}`, which is not \
                 registered (known: {known}) — the scope is not projected.",
                prim.path
            );
            continue;
        };
        let model_name = network_model_name(&prim.path, instance_id);
        let ctx = SynthContext { classes: &classes };
        let synthesized = match synthesizer.synthesize(&view, &root_path, &model_name, &ctx) {
            Ok(network) => network,
            Err(errors) => {
                let message = errors
                    .iter()
                    .map(|error| format!("{}: {}", error.path, error.message))
                    .collect::<Vec<_>>()
                    .join("; ");
                let fingerprint = source_fingerprint(&format!("projection-error:{message}"));
                if previous.is_some_and(|state| state.fingerprint == fingerprint) {
                    continue;
                }
                notices.write(ModelicaNotice {
                    level: NoticeLevel::Error,
                    text: format!("[{model_name}] Projection error: {message}"),
                });
                error!("[domain-projection] `{}` rejected: {message}", prim.path);
                retire_sim_interface(&mut commands, entity);
                // A rejected projection has no interface to hold anyone to; the
                // rejection itself is the error the user must act on.
                commands.entity(entity).remove::<UsdModelicaPortContract>();
                commands.entity(entity).try_insert((
                    ModelicaModel {
                        model_name: model_name.clone(),
                        session_id: installed_model.map_or(1, |model| model.session_id + 1),
                        is_stepping: false,
                        is_compiling: false,
                        last_error: Some(message),
                        ..default()
                    },
                    UsdSourcedCosim,
                    DomainProjectionState { fingerprint },
                    GeneratedModelicaSource {
                        network_root: prim.path.clone(),
                        doc_uri: format!("generated://{model_name}.mo"),
                        source: String::new(),
                        component_paths: Vec::new(),
                        members: Vec::new(),
                        member_output_aliases: Vec::new(),
                        units: Vec::new(),
                    },
                ));
                continue;
            }
        };
        // Waiting on a member's declared class: no verdict, no state written, so
        // the next trigger asks again.
        if matches!(synthesized, SynthOutcome::Pending) {
            continue;
        }
        let SynthOutcome::Ready(synthesized) = synthesized else {
            if previous.is_some() {
                // The authored collection ceased to describe a compilable
                // network. Retire its runtime projection in the same update;
                // keeping the old solver would simulate stale authoring.
                retire_sim_interface(&mut commands, entity);
                commands.entity(entity).remove::<(
                    ModelicaModel,
                    UsdSourcedCosim,
                    UsdModelicaPortContract,
                    DomainProjectionState,
                    GeneratedModelicaSource,
                )>();
            }
            continue;
        };
        let component_count = synthesized.component_paths.len();
        let source = synthesized.source;
        let source_for_diagnostics = source.clone();
        let fingerprint = source_fingerprint(&source);
        if previous.is_some_and(|state| state.fingerprint == fingerprint) {
            continue;
        }

        // ONE parse-and-extract, shared with `cosim::dispatch_loaded_modelica_sources`:
        // the interface of a model is read the same way whether the source came
        // off disk or out of this emitter.
        let interface = parse_model_interface(&source, "usd-network-projection.mo");
        let compiled_name = interface.model_name.unwrap_or_else(|| model_name.clone());
        let session_id = installed_model.map_or(1, |model| model.session_id + 1);
        let doc_uri = format!("generated://{model_name}.mo");
        let mut model = ModelicaModel {
            model_name: compiled_name.clone(),
            parameters: interface.parameters,
            inputs: interface.inputs,
            communication_period_secs: lunco_modelica::DEFAULT_COMMUNICATION_PERIOD_SECS,
            session_id,
            is_stepping: true,
            is_compiling: true,
            resume_after_compile: true,
            ..default()
        };
        let dispatch = channels.tx.send(ModelicaCommand::Compile {
            entity,
            session_id,
            model_name: compiled_name,
            source,
            doc_uri: doc_uri.clone(),
            extra_sources: Vec::new(),
            parameter_overrides: Vec::new(),
            stream: None,
            // A projected domain island is a NETWORK of components — a battery
            // bus, a thermal loop — not a program driving a client-predicted
            // body. It therefore carries no prediction promise and resolves
            // through the authoritative-live capability profile. The worker,
            // not this projector, owns backend selection and DAE lowering.
            realtime_safe: false,
        });
        info!(
            "[domain-projection] compiling `{}` from {} component(s) via `{requested}` as \
             generated://{}.mo",
            prim.path, component_count, model_name
        );
        if let Err(error) = dispatch {
            let message = format!("could not dispatch generated model compile: {error}");
            model.is_stepping = false;
            model.is_compiling = false;
            model.last_error = Some(message.clone());
            notices.write(ModelicaNotice {
                level: NoticeLevel::Error,
                text: format!("[{}] Compile error: {message}", model.model_name),
            });
        }
        let member_output_aliases = generated_member_output_aliases(
            &view,
            &prim.path,
            &synthesized.members,
            &synthesized.outputs,
        );
        let generated_source = GeneratedModelicaSource {
            network_root: prim.path.clone(),
            doc_uri: doc_uri.clone(),
            source: source_for_diagnostics,
            component_paths: synthesized.component_paths,
            members: synthesized.members,
            member_output_aliases,
            units: synthesized.units,
        };
        // A changed wrapper may expose a different port interface. Rebuild the
        // derived co-sim projection instead of retaining values and port names
        // from the previous compiled topology.
        retire_sim_interface(&mut commands, entity);
        commands.entity(entity).try_insert((
            model,
            UsdSourcedCosim,
            // The generated ModelicaModel is installed before the ordinary
            // wrapper pass can publish SimComponent. This is an explicit
            // lifecycle fact, not a timing guess: consumers must wait for the
            // wrapper's port surface instead of classifying the interval as a
            // missing authored port.
            lunco_core::PortSurfacePending,
            // The same USD-declares / compiler-confirms contract the per-prim
            // program path has carried all along: the network's boundary is an
            // authored promise, and `validate_usd_modelica_port_contracts` is
            // what turns a promise the DAE does not keep into one durable,
            // actionable error instead of an island that steps and publishes
            // nothing.
            UsdModelicaPortContract::new(
                synthesized.inputs.iter().cloned(),
                synthesized.outputs.iter().cloned(),
            ),
            DomainProjectionState { fingerprint },
            generated_source,
        ));
    }
}

/// Give every successful generated network a normal, read-only Modelica
/// document. This is intentionally a separate system: the compiler projection
/// owns synthesis, while the document registry owns inspectable source and the
/// scene-to-document link used by the standard Modelica UI and API.
pub fn sync_generated_network_documents(
    mut generated: Query<
        (Entity, &GeneratedModelicaSource, &mut ModelicaModel),
        Or<(
            Added<GeneratedModelicaSource>,
            Changed<GeneratedModelicaSource>,
        )>,
    >,
    mut documents: ResMut<lunco_modelica::state::ModelicaDocumentRegistry>,
) {
    for (entity, source, mut model) in &mut generated {
        // Projection errors are represented by an empty diagnostic source and
        // must not create a misleading editable-looking blank document.
        if source.source.is_empty() {
            continue;
        }
        let document =
            if !model.document.is_unassigned() && documents.host(model.document).is_some() {
                model.document
            } else {
                documents.allocate_with_origin(
                    source.source.clone(),
                    lunco_doc::DocumentOrigin::Bundled {
                        filename: format!("generated/{}.mo", model.model_name),
                    },
                )
            };
        documents.checkpoint_source(document, source.source.clone());
        documents.link(entity, document);
        model.document = document;
    }
}

/// Publish the current generated sources to the UI-facing derived registry.
pub fn publish_generated_sources(
    q_generated: Query<(&GeneratedModelicaSource, Option<&ModelicaModel>)>,
    mut generated: ResMut<lunco_modelica::state::GeneratedModelicaSources>,
) {
    generated.entries = q_generated
        .iter()
        .map(
            |(source, model)| lunco_modelica::state::GeneratedModelicaSourceEntry {
                document: model.map(|m| m.document).unwrap_or_default(),
                uri: model
                    .map(|m| format!("generated://{}.mo", m.model_name))
                    .unwrap_or_else(|| {
                        format!(
                            "generated://{}.mo",
                            source.network_root.trim_matches('/').replace('/', "_")
                        )
                    }),
                network_root: source.network_root.clone(),
                source: source.source.clone(),
                error: model.and_then(|m| m.last_error.clone()),
            },
        )
        .collect();
}

/// `GeneratedModelicaSource` — read back the exact Modelica text a projected
/// network was compiled from.
///
/// `curl … {"type":"ExecuteCommand","command":"GeneratedModelicaSource","params":{}}` lists every
/// projected network; `{"network_root":"/Rover/Electrical"}` returns one. This
/// is the read path for the `generated://…` documents the compiler reports
/// errors against, and the only way to see what USD actually emitted.
pub struct GeneratedSourceProvider;

impl lunco_api::ApiQueryProvider for GeneratedSourceProvider {
    fn name(&self) -> &'static str {
        "GeneratedModelicaSource"
    }
    fn execute(&self, world: &mut World, params: &serde_json::Value) -> lunco_api::ApiResponse {
        let wanted = params
            .get("network_root")
            .and_then(|value| value.as_str())
            .map(str::to_string);
        let mut q = world.query::<(&GeneratedModelicaSource, Option<&ModelicaModel>)>();
        let networks: Vec<serde_json::Value> = q
            .iter(world)
            .filter(|(generated, _)| {
                wanted
                    .as_deref()
                    .is_none_or(|root| root == generated.network_root)
            })
            .map(|(generated, model)| {
                serde_json::json!({
                    "network_root": generated.network_root,
                    "model_name": model.map(|model| model.model_name.clone()).unwrap_or_default(),
                    "doc_uri": generated.doc_uri,
                    "error": model.and_then(|model| model.last_error.clone()),
                    "components": generated.component_paths,
                    "members": generated
                        .members
                        .iter()
                        .map(|(prim, asset, class)| serde_json::json!({
                            "prim": prim, "source_asset": asset, "class": class,
                        }))
                        .collect::<Vec<_>>(),
                    "units": generated
                        .units
                        .iter()
                        .map(|unit| serde_json::json!({
                            "name": unit.name,
                            "components": unit.component_paths,
                            "inputs": unit.inputs,
                            "outputs": unit.outputs,
                        }))
                        .collect::<Vec<_>>(),
                    "source": generated.source,
                })
            })
            .collect();
        lunco_api::ApiResponse::ok(serde_json::json!({ "networks": networks }))
    }
}

/// Stable, path-qualified identity for a generated network model.
///
/// The leaf name alone is not unique: a stage may contain several independent
/// scopes named `Electrical`. Including the composed prim path also keeps
/// worker sessions and diagnostics attributable to the authored network.
fn network_model_name(root: &str, global_id: Option<u64>) -> String {
    let path = modelica_path_identifier(root.trim_matches('/'));
    match global_id {
        Some(global_id) => format!("{path}_G{global_id}_System"),
        None => format!("{path}_System"),
    }
}

/// The workspace hashing substrate, not `DefaultHasher`: this value decides
/// whether a live edit recompiles, and one definition of "same source" is worth
/// more than a std default whose stability is unspecified.
fn source_fingerprint(source: &str) -> u64 {
    lunco_hash::fnv1a64(source.as_bytes())
}

/// Read one composed `Scope` as a network, or say why it cannot be one.
///
/// `Ok(None)` = not a network root (or nothing solvable is left in it);
/// `Err` = authored opinions that would produce a model the compiler could only
/// reject, reported against the property that carries them.
///
/// Public because this is the layer worth testing against REAL composed USD:
/// every unit test below builds a `DomainNetwork` by hand, and the composition
/// arcs are exactly where this has broken in the field.
pub fn read_network(
    view: &lunco_usd_bevy::StageView<'_>,
    root: &SdfPath,
    classes: &MemberClasses,
) -> Result<Option<DomainNetwork>, Vec<DomainProjectionError>> {
    let root_string = root.to_string();
    if !is_domain_network_root(view, root) {
        return Ok(None);
    }
    let member_paths = view
        .collection_members(root, "components")
        .map_err(|error| {
            vec![DomainProjectionError {
                path: root_string.clone(),
                message: format!("could not read component collection: {error}"),
            }]
        })?;
    let mut components = Vec::new();
    let mut extraction_errors = Vec::new();
    // Set when a member's class is not knowable yet — see `pending_sources`.
    let mut pending_sources = false;
    for path in member_paths {
        if path.is_property_path() || path.is_prim_variant_selection_path() {
            continue;
        }
        if !view.has_api_schema(&path, "LunCoProgramAPI") {
            // A member that lost `LunCoProgramAPI` through composition is not an
            // opinion about anything — it is almost always a reference arc that
            // failed to remap, and staying silent here is what makes the first
            // symptom a confusing boundary error about a prim the author can see
            // in their file. Name it where it happens.
            warn!(
                "[domain-projection] `{path}` is in collection `{root_string}/components` but \
                 applies no LunCoProgramAPI — it contributes nothing to the generated model. \
                 Check that the reference arc composing it survived."
            );
            continue;
        }
        // The source file's own `within` + class is the answer, once
        // `resolve_member_classes` has read it. A member with no verdict yet
        // leaves the whole network pending until the source declaration is
        // available.
        let source_ref = match modelica_source_ref(view, &path) {
            Ok(source_ref) => source_ref,
            Err(issue) => {
                extraction_errors.push(DomainProjectionError {
                    path: issue.property,
                    message: issue.message,
                });
                continue;
            }
        };
        let model_class = match classes.resolve(&source_ref.asset) {
            Ok(Some(class)) => class,
            Ok(None) => {
                pending_sources = true;
                continue;
            }
            Err(message) => {
                extraction_errors.push(DomainProjectionError {
                    path: format!("{path}.info:sourceAsset"),
                    message,
                });
                continue;
            }
        };
        let source_asset = source_ref.asset;
        let attrs = view.attr_names(&path);
        let mut constants = BTreeMap::new();
        let mut connectors = BTreeMap::new();
        let mut declared_connectors = BTreeSet::new();
        let mut inputs = BTreeMap::new();
        let mut declared_outputs = BTreeSet::new();
        for attr in attrs {
            if let Some(name) = attr.strip_prefix("connectors:") {
                declared_connectors.insert(name.to_string());
                let targets: Vec<String> = view
                    .connections(&path, &attr)
                    .iter()
                    .map(ToString::to_string)
                    .collect();
                if !targets.is_empty() {
                    connectors.insert(name.to_string(), targets);
                }
            } else if let Some(name) = attr.strip_prefix("inputs:") {
                let targets = view.connections(&path, &attr);
                if targets.len() > 1 {
                    extraction_errors.push(DomainProjectionError {
                        path: format!("{path}.{attr}"),
                        message: "a scalar Modelica input must have at most one connection source"
                            .into(),
                    });
                } else if let Some(target) = targets.first() {
                    inputs.insert(name.to_string(), target.to_string());
                } else if let Some(value) = view.real(&path, &attr) {
                    // A non-finite opinion would be emitted verbatim (`NaN`,
                    // `inf`) and come back as a compiler error against generated
                    // source, blaming the model for the authoring.
                    if !value.is_finite() {
                        extraction_errors.push(DomainProjectionError {
                            path: format!("{path}.{attr}"),
                            message: format!(
                                "`{value}` is not a finite value; a generated Modelica \
                                 modification must be a real number"
                            ),
                        });
                        continue;
                    }
                    constants.insert(name.to_string(), value);
                } else {
                    extraction_errors.push(DomainProjectionError {
                        path: format!("{path}.{attr}"),
                        message:
                            "generated Modelica inputs must be scalar real values or connections"
                                .into(),
                    });
                }
            } else if let Some(name) = attr.strip_prefix("outputs:") {
                declared_outputs.insert(name.to_string());
            }
        }
        components.push(DomainComponent {
            path: path.to_string(),
            source_asset,
            model_class,
            constants,
            connectors,
            declared_connectors,
            inputs,
            declared_outputs,
        });
    }
    if !extraction_errors.is_empty() {
        return Err(extraction_errors);
    }
    if pending_sources {
        // The member set is INCOMPLETE while a class is unknown, so every
        // conclusion below — which parts are unwired, whether a boundary output
        // has a source — would be drawn from a partial network and reported as
        // an authoring error. Say "not yet" instead.
        return Ok(Some(DomainNetwork {
            root: root_string,
            components: Vec::new(),
            inputs: BTreeSet::new(),
            input_sources: BTreeMap::new(),
            outputs: BTreeMap::new(),
            pending_sources: true,
        }));
    }
    // A component with an acausal port but no authored edge is a legitimate
    // installed-but-unwired part (for example, a solar panel before a battery
    // is selected). It has no well-posed DAE by itself, so omit it from this
    // generated island rather than rejecting unrelated connected equipment.
    // Causal-only components remain: they may be complete models without an
    // acausal connector at all.
    let omitted = retain_connected_acausal_components(&mut components);
    if components.is_empty() {
        return Ok(None);
    }

    let attrs = view.attr_names(root);
    let inputs: BTreeSet<_> = attrs
        .iter()
        .filter_map(|attr| attr.strip_prefix("inputs:").map(str::to_string))
        .collect();
    let mut input_sources = BTreeMap::new();
    let mut outputs = BTreeMap::new();
    for attr in &attrs {
        let Some(name) = attr.strip_prefix("inputs:") else {
            continue;
        };
        let targets = view.connections(root, attr);
        if targets.len() > 1 {
            extraction_errors.push(DomainProjectionError {
                path: format!("{root}.{attr}"),
                message: "a scalar network input must have at most one connection source".into(),
            });
        } else if let Some(target) = targets.first() {
            input_sources.insert(name.to_string(), target.to_string());
        }
    }
    for attr in &attrs {
        let Some(name) = attr.strip_prefix("outputs:") else {
            continue;
        };
        let targets = view.connections(root, attr);
        if targets.len() != 1 {
            extraction_errors.push(DomainProjectionError {
                path: format!("{root}.{attr}"),
                message: "a network output must have exactly one component source".into(),
            });
            continue;
        }
        // A boundary output whose source was OMITTED above (an installed but
        // unwired part) drops with it. Rejecting the network instead is what
        // took a whole rover's electrical domain offline when one reference arc
        // stopped composing the solar panel into the collection: no `soc`, no
        // `solar_power`, and an error blaming the collection. The two policies
        // have to agree — omitting a part means omitting what it published.
        let source_prim = targets[0]
            .split_once(".outputs:")
            .map(|(prim, _)| prim.to_string())
            .unwrap_or_default();
        if omitted.contains(&source_prim) {
            warn!(
                "[domain-projection] `{root}.{attr}` publishes `{}`, which is installed but has \
                 no acausal connection, so it is not part of this generated network — the output \
                 is dropped. Wire its `connectors:*` to bring both back.",
                source_prim
            );
            continue;
        }
        outputs.insert(name.to_string(), targets[0].to_string());
    }
    if !extraction_errors.is_empty() {
        return Err(extraction_errors);
    }
    // A boundary input nothing consumes is authored intent that reaches no
    // equation: the wire into it lands, the value updates every tick, and the
    // DAE never reads it. Silent, and indistinguishable from a working feed.
    for input in &inputs {
        let boundary = format!("{root_string}.inputs:{input}");
        let consumed = components.iter().any(|component| {
            component
                .inputs
                .values()
                .any(|target| *target == boundary || input_sources.get(input) == Some(target))
        });
        if !consumed {
            warn!(
                "[domain-projection] `{boundary}` is declared on the network but no member \
                 consumes it — nothing in the generated model reads this input. Connect a \
                 member's `inputs:*` to it, or remove it."
            );
        }
    }
    let network = DomainNetwork {
        root: root_string,
        components,
        inputs,
        input_sources,
        outputs,
        pending_sources: false,
    };
    let mut errors = validate_network(&network);
    // Say WHY a causal source is missing when the answer is "it was installed
    // but never wired, so the island omitted it" — otherwise the only report is
    // `outside collection`, about a prim the author can see listed in their own
    // `collection:components:includes`.
    for error in &mut errors {
        if let Some(path) = omitted
            .iter()
            .find(|path| error.message.contains(path.as_str()))
        {
            error.message.push_str(&format!(
                " — `{path}` IS in the collection, but it declares an acausal connector that \
                 nothing connects to, so it is not part of the generated network. Wire its \
                 `connectors:*`."
            ));
        }
    }
    if errors.is_empty() {
        Ok(Some(network))
    } else {
        Err(errors)
    }
}

/// Validate that projection will preserve every authored network edge.
pub fn validate_network(network: &DomainNetwork) -> Vec<DomainProjectionError> {
    let mut errors = Vec::new();
    let components: BTreeMap<_, _> = network
        .components
        .iter()
        .map(|component| (component.path.as_str(), component))
        .collect();
    let boundary_sources: BTreeSet<_> =
        network.input_sources.values().map(String::as_str).collect();

    let mut boundaries_by_source = BTreeMap::<&str, Vec<&str>>::new();
    for (boundary, source) in &network.input_sources {
        boundaries_by_source
            .entry(source)
            .or_default()
            .push(boundary);
    }
    for (source, boundaries) in boundaries_by_source {
        if boundaries.len() > 1 {
            errors.push(DomainProjectionError {
                path: network.root.clone(),
                message: format!(
                    "network inputs {} resolve to the same composed source `{source}`; their authored boundary identity is ambiguous",
                    boundaries.join(", ")
                ),
            });
        }
    }

    let mut generated_names = BTreeMap::<String, String>::new();
    for component in &network.components {
        let generated = instance_identifier(&network.root, &component.path);
        if let Some(previous) = generated_names.insert(generated.clone(), component.path.clone()) {
            errors.push(DomainProjectionError {
                path: component.path.clone(),
                message: format!(
                    "component paths `{previous}` and `{}` produce the same Modelica identifier `{generated}`",
                    component.path
                ),
            });
        }
        for member in component
            .constants
            .keys()
            .chain(component.declared_connectors.iter())
            .chain(component.inputs.keys())
            .chain(component.declared_outputs.iter())
        {
            if !is_modelica_identifier(member) {
                errors.push(DomainProjectionError {
                    path: component.path.clone(),
                    message: format!("public member `{member}` is not a valid Modelica identifier"),
                });
            }
        }
    }

    for component in &network.components {
        for (connector, targets) in &component.connectors {
            for target in targets {
                let Some((target_prim, target_connector)) = target.split_once(".connectors:")
                else {
                    errors.push(DomainProjectionError {
                        path: format!("{}.connectors:{connector}", component.path),
                        message: format!("target `{target}` is not a connectors: property"),
                    });
                    continue;
                };
                let Some(target_component) = components.get(target_prim) else {
                    errors.push(DomainProjectionError {
                        path: format!("{}.connectors:{connector}", component.path),
                        message: format!(
                            "target component `{target_prim}` is outside collection `{}`",
                            network.root
                        ),
                    });
                    continue;
                };
                if !target_component
                    .declared_connectors
                    .contains(target_connector)
                {
                    errors.push(DomainProjectionError {
                        path: format!("{}.connectors:{connector}", component.path),
                        message: format!("target connector `{target}` does not exist"),
                    });
                }
            }
        }
        for (input, target) in &component.inputs {
            let boundary_prefix = format!("{}.inputs:", network.root);
            if let Some(boundary) = target.strip_prefix(&boundary_prefix) {
                if !network.inputs.contains(boundary) {
                    errors.push(DomainProjectionError {
                        path: format!("{}.inputs:{input}", component.path),
                        message: format!("network boundary input `{target}` does not exist"),
                    });
                }
                continue;
            }
            if boundary_sources.contains(target.as_str()) {
                continue;
            }
            let Some((target_prim, output)) = target.split_once(".outputs:") else {
                errors.push(DomainProjectionError {
                    path: format!("{}.inputs:{input}", component.path),
                    message: format!(
                        "target `{target}` must be a network inputs: property or component outputs: property"
                    ),
                });
                continue;
            };
            let Some(target_component) = components.get(target_prim) else {
                errors.push(DomainProjectionError {
                    path: format!("{}.inputs:{input}", component.path),
                    message: format!(
                        "causal source component `{target_prim}` is outside collection `{}`",
                        network.root
                    ),
                });
                continue;
            };
            if !target_component.declared_outputs.contains(output) {
                errors.push(DomainProjectionError {
                    path: format!("{}.inputs:{input}", component.path),
                    message: format!("causal source output `{target}` does not exist"),
                });
            }
        }
    }
    for (output, target) in &network.outputs {
        let Some((target_prim, member)) = target.split_once(".outputs:") else {
            errors.push(DomainProjectionError {
                path: format!("{}.outputs:{output}", network.root),
                message: format!("target `{target}` is not a component outputs: property"),
            });
            continue;
        };
        let Some(component) = components.get(target_prim) else {
            errors.push(DomainProjectionError {
                path: format!("{}.outputs:{output}", network.root),
                message: format!(
                    "output source component `{target_prim}` is outside collection `{}`",
                    network.root
                ),
            });
            continue;
        };
        if !component.declared_outputs.contains(member) {
            errors.push(DomainProjectionError {
                path: format!("{}.outputs:{output}", network.root),
                message: format!("output source `{target}` does not exist"),
            });
        }
    }

    errors
}

/// Drop only electrically unconnected component facets before emitting a DAE.
///
/// USD connections are directional authoring opinions whereas Modelica
/// `connect()` is symmetric, so retaining only the source side would be wrong:
/// every endpoint of an authored edge belongs to the generated island. A part
/// with an acausal connector and no edge at all has no solvable electrical
/// context; it remains a perfectly valid physical component, just not a member
/// of this runtime Modelica model.
///
/// Returns the paths it dropped, because everything the network published
/// THROUGH those parts has to drop with them (see the boundary-output handling
/// in [`read_network`]).
fn retain_connected_acausal_components(components: &mut Vec<DomainComponent>) -> BTreeSet<String> {
    let connected: BTreeSet<String> = components
        .iter()
        .flat_map(|component| {
            component.connectors.values().flat_map(move |targets| {
                targets.iter().filter_map(move |target| {
                    target
                        .split_once(".connectors:")
                        .map(|(path, _)| [component.path.clone(), path.to_string()])
                })
            })
        })
        .flatten()
        .collect();
    let mut omitted = BTreeSet::new();
    components.retain(|component| {
        let keep = component.declared_connectors.is_empty() || connected.contains(&component.path);
        if !keep {
            omitted.insert(component.path.clone());
        }
        keep
    });
    omitted
}

fn instance_identifier(root: &str, path: &str) -> String {
    modelica_path_identifier(path.strip_prefix(root).unwrap_or(path).trim_matches('/'))
}

fn unit_instance_identifier(unit_name: &str) -> String {
    modelica_identifier(&format!("instance_{unit_name}"))
}

/// Resolve the generated wrapper names for member outputs that are actually
/// declared by the composed USD and exposed by the selected synthesizer.
///
/// The source path remains USD-authored; only the runtime solver address is
/// translated. Keeping this as data on the generated document avoids a model- or
/// renderer-specific branch in the wire resolver.
fn generated_member_output_aliases(
    view: &impl UsdRead,
    root: &str,
    members: &[(String, String, String)],
    emitted_outputs: &BTreeSet<String>,
) -> Vec<(String, String, String)> {
    members
        .iter()
        .flat_map(|(member, _, _)| {
            let Ok(path) = SdfPath::new(member) else {
                return Vec::new();
            };
            view.attr_names(&path)
                .into_iter()
                .filter_map(|attr| {
                    let output = attr
                        .strip_prefix("outputs:")
                        .map(|name| name.strip_suffix(".connect").unwrap_or(name))?;
                    let alias = generated_member_output_name(root, member, output);
                    emitted_outputs
                        .contains(&alias)
                        .then(|| (member.clone(), output.to_string(), alias))
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

/// What class a member's source asset actually declares.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MemberClass {
    /// Read from the file: `within` + the class it declares.
    Declared(String),
    /// The source settled without a usable Modelica class. This is a terminal
    /// authoring error; the projector does not substitute another class.
    Invalid(String),
}

/// How long a member source may stay unresolved before the projector reports a
/// source-resolution error.
///
/// A source-resolution transaction has its own explicit terminal state. It is
/// intentionally independent of the scene-load transaction: a missing member
/// source must not make the whole scene browser unavailable.
const CLASS_RESOLVE_MAX_SECS: f64 = 20.0;

/// The class each member source declares — the ONE authority on what a generated
/// model may instantiate.
///
/// The emitter has to name a class while it is reading the stage, where the
/// `.mo` may be an unfetched HTTP resource, so the name used to be DERIVED from
/// the asset path (`models/LunCo/Electrical/Battery.mo` → `LunCo.Electrical.Battery`).
/// That assumes the directory layout mirrors the package, which is true of the
/// shipped library and silently false the moment a directory is renamed or a
/// file's `within` says otherwise — and the symptom is "class not found" from
/// the compiler, against generated source, naming neither the prim nor the file.
///
/// So the file is loaded and read, and a network whose members are not all
/// resolved yet simply does not project until they are. Keyed by asset path, so
/// one file is fetched and parsed once per session however many networks
/// instantiate it.
#[derive(Resource, Default)]
pub struct MemberClasses {
    known: HashMap<String, MemberClass>,
    outputs: HashMap<String, BTreeSet<String>>,
    pending: HashMap<String, (Handle<lunco_modelica::source_asset::ModelicaSource>, f64)>,
}

impl MemberClasses {
    /// State a verdict directly, bypassing the loader.
    pub fn declare(&mut self, asset: impl Into<String>, class: impl Into<String>) {
        self.known
            .insert(asset.into(), MemberClass::Declared(class.into()));
    }

    /// State a terminal source-resolution error. Runtime code uses the same
    /// state after an asset load fails or its declaration cannot be parsed;
    /// tests and offline tools can seed that authoritative verdict directly.
    pub fn reject(&mut self, asset: impl Into<String>, message: impl Into<String>) {
        self.known
            .insert(asset.into(), MemberClass::Invalid(message.into()));
    }

    /// Causal outputs declared by the resolved Modelica class. `None` means
    /// the class was seeded by an offline test/tool without source interface
    /// data; callers then retain their authored contract and let compilation
    /// be the authority.
    pub fn output_names(&self, asset: &str) -> Option<&BTreeSet<String>> {
        self.outputs.get(asset)
    }

    /// Resolve the class to instantiate for `asset`. `Ok(None)` means the source
    /// is still loading; `Err` is a terminal source error.
    pub fn resolve(&self, asset: &str) -> Result<Option<String>, String> {
        match self.known.get(asset) {
            Some(MemberClass::Declared(class)) => Ok(Some(class.clone())),
            Some(MemberClass::Invalid(message)) => Err(message.clone()),
            None => Ok(None),
        }
    }
}

/// Set when a member class resolves, so the projection that was waiting on it
/// re-runs. Prim spawn and live edits are the projector's other triggers; an
/// asset finishing its load is neither.
#[derive(Resource, Default)]
pub struct ProjectionDirty(pub bool);

/// Resolve every member source's DECLARED class before synthesis.
///
/// Scans the stage for component collections, loads each member's
/// `info:sourceAsset` once, and reads `within` + the class the file declares.
/// Until a member has a verdict its network does not project at all
/// ([`SynthOutcome::Pending`]) — synthesizing before the source settles would
/// produce a generated model with an unknown member class and an unattributed
/// compiler failure.
///
/// A source that fails to load, cannot be parsed, or simply never resolves
/// within [`CLASS_RESOLVE_MAX_SECS`] settles as [`MemberClass::Invalid`]. The
/// projection reports that terminal source error and does not compile an
/// incomplete model.
pub fn resolve_member_classes(
    prims: Query<&UsdPrimPath>,
    added: Query<(), Added<UsdPrimPath>>,
    mut classes: ResMut<MemberClasses>,
    mut projection_dirty: ResMut<ProjectionDirty>,
    dirty: Res<WiringDirty>,
    stages: Res<Assets<UsdStageAsset>>,
    mut canonical: NonSendMut<CanonicalStages>,
    asset_server: Res<AssetServer>,
    sources: Res<Assets<lunco_modelica::source_asset::ModelicaSource>>,
    // REAL time, like every other give-up deadline in this crate: a paused or
    // time-warped simulation must not change when a load is declared lost.
    time: Res<Time<bevy::time::Real>>,
) {
    let now = time.elapsed_secs_f64();

    // Discovery runs on the same triggers as the projector — plus never at all
    // once every member is known, which is the steady state.
    if !added.is_empty() || dirty.0 {
        for prim in &prims {
            let id = prim.stage_handle.id();
            if canonical.get(id).is_none() {
                if let Some(recipe) = stages
                    .get(&prim.stage_handle)
                    .and_then(|stage| stage.recipe.clone())
                {
                    canonical.get_or_build(id, &recipe);
                }
            }
            let Some(stage) = canonical.get(id) else {
                continue;
            };
            let view = stage.view();
            let Ok(root) = SdfPath::new(&prim.path) else {
                continue;
            };
            if !is_domain_network_root(&view, &root) {
                continue;
            }
            let Ok(members) = view.collection_members(&root, "components") else {
                continue;
            };
            for member in members {
                if !view.has_api_schema(&member, "LunCoProgramAPI") {
                    continue;
                }
                let Some(asset) = view.asset(&member, "info:sourceAsset") else {
                    continue;
                };
                if classes.known.contains_key(&asset) || classes.pending.contains_key(&asset) {
                    continue;
                }
                let handle: Handle<lunco_modelica::source_asset::ModelicaSource> =
                    asset_server.load(asset.clone());
                classes
                    .pending
                    .insert(asset, (handle, now + CLASS_RESOLVE_MAX_SECS));
            }
        }
    }

    if classes.pending.is_empty() {
        return;
    }
    let settled: Vec<(String, Option<String>, Option<BTreeSet<String>>)> = classes
        .pending
        .iter()
        .filter_map(|(asset, (handle, expires))| {
            if let Some(source) = sources.get(handle) {
                let interface = parse_model_interface(&source.text, "member-class.mo");
                let class = interface.model_name.map(|declared| match interface.within {
                    Some(within) => format!("{within}.{declared}"),
                    None => declared,
                });
                return Some((asset.clone(), class, Some(interface.outputs)));
            }
            if asset_server.load_state(handle).is_failed() || now >= *expires {
                return Some((asset.clone(), None, None));
            }
            None
        })
        .collect();
    for (asset, class, outputs) in settled {
        classes.pending.remove(&asset);
        match class {
            Some(class) => {
                if let Some(outputs) = outputs {
                    classes.outputs.insert(asset.clone(), outputs);
                }
                classes.known.insert(asset, MemberClass::Declared(class));
            }
            None => {
                warn!(
                    "[domain-projection] could not read a declared Modelica class from `{asset}` (load \
                     failed, unparseable, or not resolved within {CLASS_RESOLVE_MAX_SECS:.0}s) — \
                     the network projection will report this source error."
                );
                classes.known.insert(
                    asset,
                    MemberClass::Invalid(
                        "the Modelica source did not expose a declared class".into(),
                    ),
                );
            }
        }
        // The projection that was waiting on this member has no other reason to
        // re-run: an asset load is neither a prim spawn nor a USD edit.
        projection_dirty.0 = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn component(path: &str, target: Option<&str>) -> DomainComponent {
        DomainComponent {
            path: path.into(),
            source_asset: "lunco://models/LunCo/Electrical/DCMotor.mo".into(),
            model_class: "LunCo.Electrical.DCMotor".into(),
            constants: BTreeMap::from([("rated_power".into(), 2000.0)]),
            connectors: target
                .map(|target| BTreeMap::from([("p".into(), vec![target.into()])]))
                .unwrap_or_default(),
            declared_connectors: BTreeSet::from(["p".into()]),
            inputs: BTreeMap::new(),
            declared_outputs: BTreeSet::new(),
        }
    }

    #[test]
    fn emits_full_path_names_connections_and_causal_boundary() {
        let mut motor = component(
            "/Electrical/Left/Motor/Model",
            Some("/Electrical/Battery/Model.connectors:p"),
        );
        motor
            .inputs
            .insert("demand".into(), "/Electrical.inputs:drive_left".into());
        let network = DomainNetwork {
            root: "/Electrical".into(),
            components: vec![component("/Electrical/Battery/Model", None), motor],
            inputs: BTreeSet::from(["drive_left".into()]),
            input_sources: BTreeMap::new(),
            outputs: BTreeMap::new(),
            pending_sources: false,
        };
        let source = emit_modelica(&network, "Electrical System");
        assert!(source.contains("input Real drive_left;"));
        assert!(source.contains("Left_x2f_Motor_x2f_Model.demand = drive_left;"));
        assert!(source.contains("connect(Battery_x2f_Model.p, Left_x2f_Motor_x2f_Model.p);"));
    }

    #[test]
    fn promotes_member_outputs_to_generated_wrapper_boundary() {
        let mut jet = component("/Propulsion/RcsJet", None);
        jet.declared_outputs.insert("light_intensity".into());
        jet.declared_outputs.insert("light_radius".into());
        let network = DomainNetwork {
            root: "/Propulsion".into(),
            components: vec![jet],
            inputs: BTreeSet::new(),
            input_sources: BTreeMap::new(),
            outputs: BTreeMap::new(),
            pending_sources: false,
        };

        let source = emit_modelica(&network, "Propulsion");
        assert!(source.contains("output Real __member_RcsJet_light_intensity;"));
        assert!(source.contains("output Real __member_RcsJet_light_radius;"));
        assert!(source.contains("__member_RcsJet_light_intensity = RcsJet.light_intensity;"));
        assert!(source.contains("__member_RcsJet_light_radius = RcsJet.light_radius;"));
        assert!(source.contains(
            "__member_RcsJet_light_intensity = instance_Unit_RcsJet.__member_RcsJet_light_intensity;"
        ));
        let (_, _, issues) = lunco_modelica::ast_extract::strip_input_defaults_with_report(&source);
        assert!(
            issues.is_empty(),
            "generated wrapper must be parseable before compilation: {issues:?}\n{source}"
        );
        assert_eq!(source.matches("end Propulsion;").count(), 1);
    }

    #[test]
    fn filters_native_only_member_outputs_from_generated_modelica() {
        let mut motor = component("/Electrical/Motor", None);
        motor.declared_outputs = BTreeSet::from(["heat".into(), "torque".into()]);
        let network = DomainNetwork {
            root: "/Electrical".into(),
            components: vec![motor],
            inputs: BTreeSet::new(),
            input_sources: BTreeMap::new(),
            outputs: BTreeMap::new(),
            pending_sources: false,
        };
        let asset = network.components[0].source_asset.clone();
        let mut classes = MemberClasses::default();
        classes
            .outputs
            .insert(asset, BTreeSet::from(["heat".into()]));

        let source = emit_modelica_with_classes(&network, "Electrical", Some(&classes));
        assert!(source.contains("__member_Motor_heat"));
        assert!(!source.contains("__member_Motor_torque"));
        let (_, _, issues) = lunco_modelica::ast_extract::strip_input_defaults_with_report(&source);
        assert!(
            issues.is_empty(),
            "generated wrapper must parse: {issues:?}"
        );
    }

    #[test]
    fn synthesizer_partitions_disconnected_graph_into_composite_units() {
        let mut left = component("/Thermal/Left/Mass", None);
        left.inputs
            .insert("heat_w".into(), "/Thermal.inputs:left_heat".into());
        let mut right = component("/Thermal/Right/Mass", None);
        right
            .inputs
            .insert("heat_w".into(), "/Thermal.inputs:right_heat".into());
        let network = DomainNetwork {
            root: "/Thermal".into(),
            components: vec![right, left],
            inputs: BTreeSet::from(["left_heat".into(), "right_heat".into()]),
            input_sources: BTreeMap::new(),
            outputs: BTreeMap::from([
                (
                    "left_temp".into(),
                    "/Thermal/Left/Mass.outputs:temp_k".into(),
                ),
                (
                    "right_temp".into(),
                    "/Thermal/Right/Mass.outputs:temp_k".into(),
                ),
            ]),
            pending_sources: false,
        };

        let units = partition_network(&network);
        assert_eq!(units.len(), 2);
        assert_eq!(
            units
                .iter()
                .map(|unit| unit.component_paths.clone())
                .collect::<Vec<_>>(),
            vec![
                vec!["/Thermal/Left/Mass".to_string()],
                vec!["/Thermal/Right/Mass".to_string()],
            ]
        );
        assert_eq!(units[0].inputs, BTreeSet::from(["left_heat".into()]));
        assert_eq!(units[1].outputs, BTreeSet::from(["right_temp".into()]));

        let source = emit_modelica(&network, "Thermal");
        assert!(source.contains("model Thermal;".trim_end_matches(';')));
        assert!(source.matches("model Unit_").count() == 2);
        assert!(source.contains("instance_Unit_Left_x2f_Mass.left_heat = left_heat;"));
        assert!(source.contains("left_temp = instance_Unit_Left_x2f_Mass.left_temp;"));
    }

    #[test]
    fn emits_every_target_of_a_multiway_connector() {
        let mut bus = component("/Electrical/Bus/Model", None);
        bus.connectors.insert(
            "p".into(),
            vec![
                "/Electrical/LoadA/Model.connectors:p".into(),
                "/Electrical/LoadB/Model.connectors:p".into(),
            ],
        );
        let network = DomainNetwork {
            root: "/Electrical".into(),
            components: vec![
                bus,
                component("/Electrical/LoadA/Model", None),
                component("/Electrical/LoadB/Model", None),
            ],
            inputs: BTreeSet::new(),
            input_sources: BTreeMap::new(),
            outputs: BTreeMap::new(),
            pending_sources: false,
        };
        let source = emit_modelica(&network, "Electrical");
        assert!(source.contains("connect(Bus_x2f_Model.p, LoadA_x2f_Model.p);"));
        assert!(source.contains("connect(Bus_x2f_Model.p, LoadB_x2f_Model.p);"));
    }

    #[test]
    fn rejects_external_connector_targets_and_keeps_unit_partition_deterministic() {
        let mut external = component("/Electrical/Load/Model", None);
        external
            .connectors
            .insert("p".into(), vec!["/Other/Battery/Model.connectors:p".into()]);
        let network = DomainNetwork {
            root: "/Electrical".into(),
            components: vec![component("/Electrical/Battery/Model", None), external],
            inputs: BTreeSet::new(),
            input_sources: BTreeMap::new(),
            outputs: BTreeMap::new(),
            pending_sources: false,
        };
        let errors = validate_network(&network);
        assert!(errors
            .iter()
            .any(|error| error.message.contains("outside collection")));
        assert_eq!(partition_network(&network).len(), 2);
    }

    #[test]
    fn unconnected_acausal_component_is_omitted_from_generated_network() {
        let panel = component("/Electrical/SolarPanel", None);
        let mut battery = component("/Electrical/Battery", None);
        battery
            .connectors
            .insert("p".into(), vec!["/Electrical/Motor.connectors:p".into()]);
        let motor = component("/Electrical/Motor", None);
        let mut components = vec![panel, battery, motor];
        let omitted = retain_connected_acausal_components(&mut components);
        assert_eq!(
            components
                .iter()
                .map(|component| component.path.as_str())
                .collect::<Vec<_>>(),
            ["/Electrical/Battery", "/Electrical/Motor"],
            "only explicitly wired program facets enter a generated acausal island"
        );
        assert!(
            omitted.contains("/Electrical/SolarPanel"),
            "what the island omits has to be nameable — a boundary output published \
             through an omitted part drops with it instead of rejecting the network"
        );
    }

    #[test]
    fn generated_model_identity_is_qualified_by_network_path() {
        assert_ne!(
            network_model_name("/Rover/Electrical", Some(10)),
            network_model_name("/Payload/Electrical", Some(20))
        );
        assert_eq!(
            network_model_name("/Rover/Electrical", Some(42)),
            "Rover_x2f_Electrical_G42_System"
        );
        assert_ne!(
            network_model_name("/Rover/Electrical", Some(10)),
            network_model_name("/Rover/Electrical", Some(20))
        );
    }

    #[test]
    fn projection_fingerprint_changes_only_with_generated_source() {
        let source = "model A\n  Real x;\nend A;\n";
        assert_eq!(source_fingerprint(source), source_fingerprint(source));
        assert_ne!(
            source_fingerprint(source),
            source_fingerprint("model A\n  Real y;\nend A;\n")
        );
    }

    #[test]
    fn rejects_ambiguous_forwarded_boundary_sources() {
        let network = DomainNetwork {
            root: "/Electrical".into(),
            components: vec![component("/Electrical/Battery", None)],
            inputs: BTreeSet::from(["left".into(), "right".into()]),
            input_sources: BTreeMap::from([
                ("left".into(), "/Controls.outputs:throttle".into()),
                ("right".into(), "/Controls.outputs:throttle".into()),
            ]),
            outputs: BTreeMap::new(),
            pending_sources: false,
        };
        assert!(validate_network(&network)
            .iter()
            .any(|error| error.message.contains("boundary identity is ambiguous")));
    }

    #[test]
    fn rejects_modelica_keywords_as_public_members() {
        let mut bad = component("/Electrical/Load", None);
        bad.inputs
            .insert("equation".into(), "/Electrical.inputs:demand".into());
        let network = DomainNetwork {
            root: "/Electrical".into(),
            components: vec![bad],
            inputs: BTreeSet::from(["demand".into()]),
            input_sources: BTreeMap::new(),
            outputs: BTreeMap::new(),
            pending_sources: false,
        };
        assert!(validate_network(&network)
            .iter()
            .any(|error| error.message.contains("not a valid Modelica identifier")));
    }

    #[test]
    fn retiring_a_changed_projection_removes_stale_outputs() {
        fn retire_once(mut commands: Commands, q: Query<Entity, With<GeneratedModelicaSource>>) {
            for entity in &q {
                retire_sim_interface(&mut commands, entity);
            }
        }

        let mut app = App::new();
        app.add_systems(Update, retire_once);
        let entity = app
            .world_mut()
            .spawn((
                GeneratedModelicaSource {
                    network_root: "/Electrical".into(),
                    doc_uri: "generated://Electrical.mo".into(),
                    source: "model Electrical end Electrical;".into(),
                    component_paths: vec!["/Battery".into()],
                    members: Vec::new(),
                    member_output_aliases: Vec::new(),
                    units: Vec::new(),
                },
                lunco_cosim::SimComponent {
                    outputs: std::collections::HashMap::from([("soc".into(), 0.75)]),
                    ..default()
                },
            ))
            .id();

        app.update();

        assert!(
            app.world()
                .get::<lunco_cosim::SimComponent>(entity)
                .is_none(),
            "a changed or rejected projection must not retain solved values from its previous topology"
        );
    }

    #[test]
    fn default_registry_contains_each_shipped_synthesizer() {
        let registry = SynthesizerRegistry::default();
        assert!(registry.get(DEFAULT_SYNTHESIZER).is_some());
        assert!(registry.get(ACTUATOR_WRENCH_SYNTHESIZER).is_some());
    }

    #[test]
    fn actuator_wrench_allocation_uses_authored_body_torque_axes() {
        let actuators = [
            lunco_cosim::ForceActuator {
                local_position: Vec3::Y,
                direction_local: Vec3::Z,
                max_force_n: 1.0,
            },
            lunco_cosim::ForceActuator {
                local_position: Vec3::Z,
                direction_local: Vec3::X,
                max_force_n: 1.0,
            },
            lunco_cosim::ForceActuator {
                local_position: Vec3::X,
                direction_local: Vec3::Y,
                max_force_n: 1.0,
            },
        ];

        let allocation = actuator_torque_pseudoinverse(&actuators).unwrap();
        assert_eq!(allocation.len(), 3);
        for (row, expected_axis) in
            allocation
                .iter()
                .zip([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]])
        {
            for (actual, expected) in row.iter().zip(expected_axis) {
                assert!((actual - expected).abs() < 1.0e-12);
            }
        }
    }

    #[test]
    fn actuator_wrench_rejects_rank_deficient_geometry() {
        let actuators = [
            lunco_cosim::ForceActuator {
                local_position: Vec3::Y,
                direction_local: Vec3::Z,
                max_force_n: 1.0,
            },
            lunco_cosim::ForceActuator {
                local_position: Vec3::Y * 2.0,
                direction_local: Vec3::Z,
                max_force_n: 1.0,
            },
        ];

        let error = actuator_torque_pseudoinverse(&actuators).unwrap_err();
        assert!(error.contains("rank deficient"));
    }

    #[test]
    fn actuator_wrench_source_binds_missing_wrench_axes_to_zero() {
        let source = emit_actuator_wrench_model(
            "AttitudeActuation",
            &BTreeSet::from(["desired_torque_z".into()]),
            &BTreeSet::from(["valve".into()]),
            &[[0.0, 0.0, 1.0]],
        );
        assert!(source.contains("LunCo.Actuation.WrenchAllocator"));
        assert!(source.contains("allocator.desired_torque_z = desired_torque_z;"));
        assert!(source.contains("allocator.desired_force_x = 0.0;"));
        assert!(source.contains("valve = allocator.command[1];"));
    }
}
