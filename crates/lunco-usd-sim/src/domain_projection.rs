//! Runtime projection of composed USD component networks into Modelica wrappers.
//!
//! A reusable part applies `LunCoProgramAPI` for its model facet. Modelica remains the
//! authority for equations and member types; USD supplies instances, constant
//! input opinions, and ordinary property connections between public members.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::sync::Arc;

use bevy::asset::AssetId;
use bevy::prelude::*;
use bevy::tasks::{block_on, futures_lite::future, AsyncComputeTaskPool, Task};
use lunco_modelica::{
    ast_extract::{parse_model_interface, ModelicaVariableMetadata},
    ModelicaChannels, ModelicaCommand, ModelicaModel, ModelicaNotice, ModelicaSignalLayout,
    ModelicaSignalProvenance, NoticeLevel,
};
use lunco_usd_bevy::program::ProgramGraph;
use lunco_usd_bevy::read::UsdReadObject as ComposedReader;
use lunco_usd_bevy::{CanonicalStages, UsdPrimPath, UsdStageAsset};
use openusd::sdf::Path as SdfPath;
use rumoca_compile::parsing::Causality;

// The USD side of a Modelica program facet — the class an asset names, the
// lexical rules for member/instance identifiers — is ONE reader, shared with the
// lint fact producer. See `lunco_usd_bevy::program`.
pub use lunco_usd_bevy::program::is_domain_network_root;
use lunco_usd_bevy::program::{
    is_modelica_identifier, modelica_identifier, modelica_path_identifier, modelica_source_ref,
    ACTUATOR_WRENCH_DOMAIN_SYNTHESIZER, DEFAULT_DOMAIN_SYNTHESIZER,
};

use crate::cosim::{UsdModelicaPortContract, UsdSourcedCosim, WiringDirty};

fn retire_sim_interface(commands: &mut Commands, entity: Entity) {
    commands
        .entity(entity)
        .remove::<(lunco_cosim::SimComponent, crate::cosim::UsdModelicaSchedule)>();
}

/// Generated documents are runtime projections, unlike authored documents
/// whose source must outlive a scene entity. Retire only the generated origin;
/// this guard keeps ordinary document lifecycle semantics untouched.
fn retire_generated_document(
    document: lunco_doc::DocumentId,
    documents: &mut lunco_modelica::state::ModelicaDocumentRegistry,
) {
    if document.is_unassigned() {
        return;
    }
    let generated = documents
        .host(document)
        .is_some_and(|host| lunco_modelica::state::is_generated_document(host.document()));
    if generated {
        documents.remove_document(document);
    }
}

fn queue_retire_generated_document(commands: &mut Commands, document: lunco_doc::DocumentId) {
    commands.queue(move |world: &mut World| {
        if let Some(mut documents) =
            world.get_resource_mut::<lunco_modelica::state::ModelicaDocumentRegistry>()
        {
            retire_generated_document(document, &mut documents);
        }
    });
}

/// Fingerprint of the generated wrapper currently installed on a network root.
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
    /// Composed USD network root that owns this compilation unit.
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
    /// Bundled Modelica source roots required by the emitted classes. This is
    /// returned by the policy so the UI can load real dependencies without
    /// parsing generated source or hardcoding a library name in Rust.
    pub source_roots: Vec<String>,
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
    /// Public causal inputs on the generated root. Kept separate from
    /// promoted member telemetry so the workbench can explain the interface.
    pub boundary_inputs: Vec<String>,
    /// Public causal outputs authored on the generated root.
    pub boundary_outputs: Vec<String>,
    /// Unit and member positions selected by the synthesizer policy.
    pub layout: SynthesisLayout,
    /// Error produced while this USD network was being projected. Runtime
    /// solver errors belong to `ModelicaModel` and are not source-projection
    /// metadata.
    pub projection_error: Option<String>,
}

/// One public Modelica component facet authored in USD.
#[derive(Clone, Debug, PartialEq)]
pub struct DomainComponent {
    /// Composed USD path of the `LunCoProgramAPI` facet.
    pub path: String,
    /// The `info:sourceAsset` this facet names — the file whose `within` + class
    /// decides what [`MemberClasses`] exposes to the selected synthesis policy.
    pub source_asset: String,
    /// Fully-qualified class declared by the loaded `info:sourceAsset` source.
    pub model_class: String,
    /// Constant public inputs, supplied to the selected synthesis policy as
    /// component modifications.
    pub constants: BTreeMap<String, f64>,
    /// Acausal member name to the connected `connectors:*` property path.
    pub connectors: BTreeMap<String, Vec<String>>,
    /// All declared acausal members, including currently unconnected pins.
    pub declared_connectors: BTreeSet<String>,
    /// Causal input name to its connected source property path.
    pub inputs: BTreeMap<String, String>,
    /// Public causal outputs declared by the reusable model facet.
    pub declared_outputs: BTreeSet<String>,
    /// Optional presentation role for a generated Modelica topology. This is
    /// USD-authored metadata, not a solver direction: acausal Modelica flow
    /// remains reversible and runtime sign still controls animated direction.
    pub topology_role: String,
}

/// One network root and its public causal boundary.
#[derive(Clone, Debug, PartialEq)]
pub struct DomainNetwork {
    /// Composed path of the USD prim carrying `CollectionAPI:components`.
    pub root: String,
    /// Modelica component facets in the root's explicit collection.
    pub components: Vec<DomainComponent>,
    /// Public wrapper inputs authored on the network root.
    pub inputs: BTreeSet<String>,
    /// Public wrapper input name to its composed external source.
    pub input_sources: BTreeMap<String, String>,
    /// Public wrapper output name to component output property.
    pub outputs: BTreeMap<String, String>,
    /// One master-clock communication period for the generated Modelica
    /// wrapper. Every composed member must resolve to the same lattice point;
    /// a single solver cannot honor conflicting member schedules.
    pub communication_period_secs: f64,
    /// At least one member's `.mo` has not loaded and therefore its declared
    /// class is not available. The projector waits instead of compiling a
    /// partial network. See [`MemberClasses`].
    pub pending_sources: bool,
}

/// One deterministic Modelica composite unit inside a network root.
///
/// A unit is a connected component of the composed program graph. It is not a
/// second ECS participant: the selected synthesizer emits the units below one
/// generated root model, so the network root keeps one public boundary and one
/// runtime lifecycle while independent acausal subgraphs remain explicit in
/// the generated Modelica. This is the same composite-model shape used by
/// SSP/FMI toolchains.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SynthesisUnit {
    /// Stable generated Modelica class name for the composite unit.
    pub name: String,
    /// Modelica instance name chosen by the synthesis policy. Runtime signal
    /// mapping follows this exact name; the policy result must provide it.
    pub instance: String,
    /// Composed USD members absorbed into the unit.
    pub component_paths: Vec<String>,
    /// Root boundary inputs consumed by this unit.
    pub inputs: BTreeSet<String>,
    /// Root boundary outputs produced by this unit.
    pub outputs: BTreeSet<String>,
}

/// Visual placement selected alongside a generated Modelica plan.
///
/// Positions are presentation facts, not simulation inputs. The built-in
/// synthesizer supplies a deterministic topology layout; a hook-backed
/// synthesizer may replace it by returning a `layout` map. Keeping the
/// positions in the plan makes the policy result inspectable through the
/// generated-source API instead of leaving the visual decision implicit in a
/// separate Rust pass.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SynthesisLayout {
    /// Generated child-unit class name to Modelica diagram position.
    pub unit_positions: BTreeMap<String, (i32, i32)>,
    /// Composed USD member path to Modelica diagram position, local to the
    /// generated unit that owns the member. Unit diagrams are independent
    /// coordinate systems; root diagrams use `unit_positions` instead.
    pub member_positions: BTreeMap<String, (i32, i32)>,
}

/// One authoring error that prevents a safe runtime projection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DomainProjectionError {
    /// USD prim or property carrying the invalid opinion.
    pub path: String,
    /// Actionable explanation suitable for the simulator console.
    pub message: String,
}

// A network root is ONE runtime compilation unit: one generated root model on
// one entity carrying one `ModelicaModel`. The synthesizer may partition the
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
    /// Bundled Modelica source roots required by the policy-emitted source.
    pub source_roots: BTreeSet<String>,
    /// `(prim, source asset, class)` per member — attribution + class audit.
    pub members: Vec<(String, String, String)>,
    /// Causal output aliases emitted for composed members. This is collected
    /// from the USD/Modelica facts once and carried with the policy result so
    /// the runtime projection does not reread the stage or infer aliases from
    /// generated strings.
    pub member_output_aliases: Vec<(String, String, String)>,
    /// Connected composite units emitted below the root model.
    pub units: Vec<SynthesisUnit>,
    /// Presentation placements selected by the synthesizer.
    pub layout: SynthesisLayout,
    /// Communication period inherited from the composed Modelica members.
    pub communication_period_secs: f64,
}

/// One way of turning a composed USD network root into ONE Modelica compilation unit.
///
/// The seam doc 37 §8 asks for. What ships is the acausal-network synthesizer
/// below; a `thermal`, `harness` or `comms-link` synthesizer is a registration,
/// not an edit to [`project_domain_islands`]. A generic network root may select one
/// through `LunCoDomainSynthesisAPI`; otherwise the projector derives the
/// built-in owner from the typed member role schemas.
///
/// Hook-backed entries use the existing `lunco_hooks` substrate: Rust supplies
/// the composed facts and Rhai returns the Modelica source, merge units, and
/// diagram placements. The registry keeps that policy selection independent of
/// `project_domain_islands`, so changing dynamic building behaviour does not
/// require a Rust branch.
pub trait DomainSynthesizer: Send + Sync + 'static {
    /// Registry key, and the token a network root names.
    fn name(&self) -> &str;
    /// Turn one composed network root into a compilation unit.
    fn synthesize(
        &self,
        view: &dyn ComposedReader,
        root: &SdfPath,
        model_name: &str,
        ctx: &SynthContext<'_>,
    ) -> Result<SynthOutcome, Vec<DomainProjectionError>>;
}

/// What a synthesizer concluded about a network root.
#[derive(Debug)]
pub enum SynthOutcome {
    /// Not a scope this synthesizer compiles (or nothing solvable is in it).
    NotMine,
    /// Cannot be decided yet — a member's source has not loaded, so the class it
    /// declares is not knowable. The projection simply waits and is re-triggered
    /// when the source lands.
    Pending,
    Ready(Box<SynthesisPlan>),
}

/// Read-only facts a synthesizer may need beyond the stage itself.
pub struct SynthContext<'a> {
    /// Class-per-source-asset, as declared BY THE FILE. See [`MemberClasses`].
    pub classes: &'a MemberClasses,
}

/// The default for a collection of `LunCoProgramAPI` members.
pub const DEFAULT_SYNTHESIZER: &str = DEFAULT_DOMAIN_SYNTHESIZER;
/// The generic force-actuator allocator used by the shipped lander.
pub const ACTUATOR_WRENCH_SYNTHESIZER: &str = ACTUATOR_WRENCH_DOMAIN_SYNTHESIZER;

/// Select the domain owner for a composed network root.
///
/// An authored `LunCoDomainSynthesisAPI` is an explicit contract. Without that
/// API, ownership is derived from the composed member role schemas. Keeping
/// this selection in one function makes the runtime projector and the linter
/// agree on both the explicit-selector default and the role-derived owner.
pub(crate) fn select_synthesizer_name(
    view: &dyn ComposedReader,
    root: &SdfPath,
) -> Result<String, String> {
    if view.has_api_schema(root, "LunCoDomainSynthesisAPI") {
        return Ok(view
            .text(root, "lunco:synthesizer")
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| DEFAULT_SYNTHESIZER.to_string()));
    }
    derive_synthesizer_name(view, root)
}

/// Derive the domain owner from the composed USD role schemas.
///
/// A physical actuator collection is not a Modelica network: its members carry
/// `LunCoForceActuatorAPI`, while a Modelica network's members carry
/// `LunCoProgramAPI`. This is a structural classification, not a name or
/// filename heuristic. A mixed collection is rejected because choosing either
/// owner would hide an authoring error.
pub fn derive_synthesizer_name(
    view: &dyn ComposedReader,
    root: &SdfPath,
) -> Result<String, String> {
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
        return Ok(ACTUATOR_WRENCH_SYNTHESIZER.to_string());
    }
    if modelica_programs > 0 && force_actuators == 0 && unclassified.is_empty() {
        return Ok(DEFAULT_SYNTHESIZER.to_string());
    }
    if force_actuators > 0 || modelica_programs > 0 || !unclassified.is_empty() {
        return Err(format!(
            "component collection has incompatible member roles: force_actuators={force_actuators}, \
             modelica_programs={modelica_programs}, unclassified={unclassified:?}"
        ));
    }
    Ok(DEFAULT_SYNTHESIZER.to_string())
}

/// Open registry of synthesizers, by name. No enum: a new domain is a
/// registration from any plugin.
#[derive(Resource)]
pub struct SynthesizerRegistry(
    std::collections::BTreeMap<String, std::sync::Arc<dyn DomainSynthesizer>>,
);

impl Default for SynthesizerRegistry {
    fn default() -> Self {
        let mut registry = Self(Default::default());
        // The shipped network schema is an authored policy. Rust owns the
        // composed-USD facts and validates the policy result, but it must not
        // silently become the owner of source/layout generation when the Rhai
        // policy is absent.
        registry.register(HookSynthesizer {
            hook_id: format!("synth.{DEFAULT_SYNTHESIZER}"),
            name: DEFAULT_SYNTHESIZER.to_string(),
        });
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
/// required `source`, `units`, `layout`, `source_roots`, and
/// `member_output_aliases` keys. `layout` must contain both `units` and
/// `members`, even when a policy has no entries in one section. Unit positions
/// are root-diagram coordinates; member positions are local to their owning
/// unit diagram. Rust validates the policy-owned result but never fills an
/// omitted synthesis decision from a second emitter.
///
/// Registered through [`register_hook_synthesizer`]; the hook id is by convention
/// `synth.<name>`, reached exactly like `lint.usd`.
pub struct HookSynthesizer {
    name: String,
    hook_id: String,
}

impl DomainSynthesizer for HookSynthesizer {
    fn name(&self) -> &str {
        &self.name
    }
    fn synthesize(
        &self,
        view: &dyn ComposedReader,
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
        let facts = network_facts(&network, model_name, Some(ctx.classes)).map_err(|message| {
            vec![DomainProjectionError {
                path: network_root.clone(),
                message: format!(
                    "synthesizer `{}` could not build policy facts: {message}",
                    self.name
                ),
            }]
        })?;
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
        let units = parse_policy_units(
            hook_map_value(map, "units"),
            &network,
            &network_root,
            &self.name,
        )
        .map_err(|message| {
            vec![DomainProjectionError {
                path: network_root.clone(),
                message,
            }]
        })?;
        let layout = parse_policy_layout(
            hook_map_value(map, "layout"),
            &network,
            &units,
            &network_root,
            &self.name,
        )
        .map_err(|message| {
            vec![DomainProjectionError {
                path: network_root.clone(),
                message,
            }]
        })?;
        let source_roots =
            parse_policy_source_roots(hook_map_value(map, "source_roots"), &self.name).map_err(
                |message| {
                    vec![DomainProjectionError {
                        path: network_root.clone(),
                        message,
                    }]
                },
            )?;
        let member_output_aliases = parse_policy_member_output_aliases(
            hook_map_value(map, "member_output_aliases"),
            &network,
            Some(ctx.classes),
            &network_root,
            &self.name,
        )
        .map_err(|message| {
            vec![DomainProjectionError {
                path: network_root.clone(),
                message,
            }]
        })?;
        validate_generated_source(source, model_name, &network, &units, &member_output_aliases)
            .map_err(|message| {
                vec![DomainProjectionError {
                    path: network_root.clone(),
                    message: format!(
                        "synthesizer `{}` returned invalid Modelica: {message}",
                        self.name
                    ),
                }]
            })?;
        Ok(SynthOutcome::Ready(Box::new(SynthesisPlan {
            source: source.to_string(),
            // The BOUNDARY remains Rust's composed-USD answer. The policy owns
            // the emitted source, merge partition, and visual placement, but
            // cannot invent a runtime port surface or a member outside the
            // composed network.
            inputs: network.inputs.clone(),
            outputs: network.outputs.keys().cloned().collect(),
            component_paths: network
                .components
                .iter()
                .map(|component| component.path.clone())
                .collect(),
            source_roots,
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
            member_output_aliases,
            units,
            layout,
            communication_period_secs: network.communication_period_secs,
        })))
    }
}

/// Register an authored synthesizer under `name`, backed by hook `synth.<name>`.
///
/// The hook itself is registered by whatever compiled it — `lunco_hooks_rhai::register_rhai_hook`
/// for a rhai policy — so this crate needs no scripting dependency and any
/// language that implements [`lunco_hooks::ScriptHook`] can author one.
pub fn register_hook_synthesizer(registry: &mut SynthesizerRegistry, name: impl Into<String>) {
    let name = name.into();
    registry.register(HookSynthesizer {
        hook_id: format!("synth.{name}"),
        name,
    });
}

/// Remove a policy-owned hook synthesizer. A removed selector has no runtime
/// owner; selecting it therefore reports the missing registration instead of
/// silently restoring a compiled policy.
pub fn unregister_hook_synthesizer(registry: &mut SynthesizerRegistry, name: &str) {
    registry.0.remove(name);
}

fn hook_map_value<'a>(
    map: &'a [(String, lunco_hooks::HookValue)],
    key: &str,
) -> Option<&'a lunco_hooks::HookValue> {
    map.iter()
        .find_map(|(candidate, value)| (candidate == key).then_some(value))
}

fn hook_map_string(
    map: &[(String, lunco_hooks::HookValue)],
    key: &str,
    context: &str,
) -> Result<String, String> {
    hook_map_value(map, key)
        .and_then(lunco_hooks::HookValue::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| format!("{context} must contain a non-empty string `{key}`"))
}

fn hook_map_string_array(
    map: &[(String, lunco_hooks::HookValue)],
    key: &str,
    context: &str,
) -> Result<Vec<String>, String> {
    let Some(value) = hook_map_value(map, key) else {
        return Ok(Vec::new());
    };
    let lunco_hooks::HookValue::Array(values) = value else {
        return Err(format!("{context}.{key} must be an array of strings"));
    };
    values
        .iter()
        .enumerate()
        .map(|(index, value)| {
            value
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| format!("{context}.{key}[{index}] must be a string"))
        })
        .collect()
}

fn parse_policy_source_roots(
    value: Option<&lunco_hooks::HookValue>,
    policy_name: &str,
) -> Result<BTreeSet<String>, String> {
    let Some(value) = value else {
        return Err(format!(
            "synthesizer `{policy_name}` must return `source_roots`"
        ));
    };
    let lunco_hooks::HookValue::Array(values) = value else {
        return Err(format!(
            "synthesizer `{policy_name}` returned `source_roots`, which must be an array of strings"
        ));
    };
    let roots: Vec<String> = values
        .iter()
        .enumerate()
        .map(|(index, value)| {
            value.as_str().map(str::to_string).ok_or_else(|| {
                format!("synthesizer `{policy_name}` source_roots[{index}] must be a string")
            })
        })
        .collect::<Result<_, _>>()?;
    roots
        .into_iter()
        .map(|root| {
            if is_modelica_identifier(&root) {
                Ok(root)
            } else {
                Err(format!(
                    "synthesizer `{policy_name}` returned invalid source root `{root}`"
                ))
            }
        })
        .collect()
}

/// Parse the policy-owned telemetry promotion table. The policy must return the
/// table explicitly, even when it is empty; Rust only validates its references.
fn parse_policy_member_output_aliases(
    value: Option<&lunco_hooks::HookValue>,
    network: &DomainNetwork,
    classes: Option<&MemberClasses>,
    root: &str,
    policy_name: &str,
) -> Result<Vec<(String, String, String)>, String> {
    let Some(value) = value else {
        return Err(format!(
            "synthesizer `{policy_name}` must return `member_output_aliases`"
        ));
    };
    let lunco_hooks::HookValue::Array(entries) = value else {
        return Err(format!(
            "synthesizer `{policy_name}` returned `member_output_aliases`, which must be an array"
        ));
    };
    let known: BTreeSet<(String, String)> = generated_member_outputs(network, classes)?
        .into_iter()
        .map(|(member, output, _)| (member, output))
        .collect();
    let mut aliases = Vec::with_capacity(entries.len());
    let mut seen = BTreeSet::new();
    for (index, entry) in entries.iter().enumerate() {
        let context = format!("synthesizer `{policy_name}` member_output_aliases[{index}]");
        let lunco_hooks::HookValue::Map(map) = entry else {
            return Err(format!("{context} must be a map"));
        };
        let member = hook_map_string(map, "member_path", &context)?;
        let output = hook_map_string(map, "output", &context)?;
        let alias = hook_map_string(map, "alias", &context)?;
        if !known.contains(&(member.clone(), output.clone())) {
            return Err(format!(
                "{context} refers to `{member}.outputs:{output}`, which is not a declared member output in `{root}`"
            ));
        }
        if !is_modelica_identifier(&alias) {
            return Err(format!(
                "{context}.alias `{alias}` is not a valid Modelica identifier"
            ));
        }
        if network.inputs.contains(&alias) || network.outputs.contains_key(&alias) {
            return Err(format!(
                "{context}.alias `{alias}` collides with a root boundary port"
            ));
        }
        if !seen.insert(alias.clone()) {
            return Err(format!("{context}.alias `{alias}` is duplicated"));
        }
        aliases.push((member, output, alias));
    }
    Ok(aliases)
}

/// Parse a policy result and validate the root interface shared by every
/// generated Modelica synthesizer. Returning the AST keeps callers from
/// parsing the same generated source twice before they inspect policy-specific
/// structure.
fn parse_validated_root_interface(
    source: &str,
    model_name: &str,
    inputs: &BTreeSet<String>,
    outputs: &BTreeSet<String>,
    aliases: &[(String, String, String)],
) -> Result<rumoca_compile::parsing::ast::StoredDefinition, String> {
    let ast = rumoca_phase_parse::parse_to_ast(source, "generated-policy.mo")
        .map_err(|error| format!("strict Modelica parse failed: {error:?}"))?;
    let root = lunco_modelica::diagram::find_class_by_qualified_name(&ast, model_name)
        .ok_or_else(|| format!("root class `{model_name}` is missing"))?;

    let mut expected_root_outputs = outputs.clone();
    expected_root_outputs.extend(aliases.iter().map(|(_, _, alias)| alias.clone()));
    for component in root.components.values() {
        match &component.causality {
            Causality::Input(_) if !inputs.contains(component.name.as_str()) => {
                return Err(format!(
                    "root declares undeclared boundary input `{}`",
                    component.name
                ));
            }
            Causality::Output(_) if !expected_root_outputs.contains(component.name.as_str()) => {
                return Err(format!(
                    "root declares undeclared boundary output `{}`",
                    component.name
                ));
            }
            _ => {}
        }
    }

    for input in inputs {
        let Some(component) = root.components.get(input) else {
            return Err(format!("root boundary input `{input}` is missing"));
        };
        if !matches!(component.causality, Causality::Input(_)) {
            return Err(format!("root boundary `{input}` is not declared as input"));
        }
    }
    for output in outputs {
        let Some(component) = root.components.get(output) else {
            return Err(format!("root boundary output `{output}` is missing"));
        };
        if !matches!(component.causality, Causality::Output(_)) {
            return Err(format!(
                "root boundary `{output}` is not declared as output"
            ));
        }
    }
    for (_, _, alias) in aliases {
        let Some(component) = root.components.get(alias) else {
            return Err(format!(
                "promoted output `{alias}` is missing from the root"
            ));
        };
        if !matches!(component.causality, Causality::Output(_)) {
            return Err(format!(
                "promoted output `{alias}` is not declared as output"
            ));
        }
    }
    Ok(ast)
}

/// Validate the policy's actual Modelica source against the Rust-owned graph
/// facts. Parsing only is insufficient: a policy can return a syntactically
/// valid empty model while the runtime later falls back to an invented class
/// name or silently loses every member. This validator is intentionally an AST
/// mechanism, not a knowledge of the shipped emitter, so future Rhai policies
/// can change layout, equations, and partition without Rust changes.
fn validate_generated_source(
    source: &str,
    model_name: &str,
    network: &DomainNetwork,
    units: &[SynthesisUnit],
    aliases: &[(String, String, String)],
) -> Result<(), String> {
    let outputs: BTreeSet<String> = network.outputs.keys().cloned().collect();
    let ast =
        parse_validated_root_interface(source, model_name, &network.inputs, &outputs, aliases)?;
    let root = lunco_modelica::diagram::find_class_by_qualified_name(&ast, model_name)
        .ok_or_else(|| format!("root class `{model_name}` is missing"))?;

    let expected_members: BTreeMap<String, String> = network
        .components
        .iter()
        .map(|component| {
            instance_identifier(&network.root, &component.path)
                .map(|instance| (instance, component.model_class.clone()))
        })
        .collect::<Result<_, _>>()?;
    let expected_unit_instances: BTreeSet<String> =
        units.iter().map(|unit| unit.instance.clone()).collect();
    if expected_unit_instances.len() != units.len() {
        return Err("generated unit instances must be unique".into());
    }
    for unit in units {
        let instance = &unit.instance;
        if network.inputs.contains(instance)
            || outputs.contains(instance)
            || aliases.iter().any(|(_, _, alias)| alias == instance)
        {
            return Err(format!(
                "generated unit instance `{instance}` collides with a root interface name"
            ));
        }
        let Some(component) = root.components.get(instance) else {
            return Err(format!("root unit instance `{instance}` is missing"));
        };
        if component.type_name.to_string() != unit.name {
            return Err(format!(
                "root unit `{instance}` has type `{}`, expected `{}`",
                component.type_name, unit.name
            ));
        }
    }
    for component in root.components.values() {
        let name = component.name.as_str();
        if expected_members.contains_key(name)
            || expected_members
                .values()
                .any(|class| class == &component.type_name.to_string())
        {
            return Err(format!(
                "root directly declares native member `{name}`; members must live inside generated units"
            ));
        }
        if component.type_name.to_string().starts_with("Unit_")
            && !expected_unit_instances.contains(name)
        {
            return Err(format!("root contains undeclared generated unit `{name}`"));
        }
    }

    for unit in units {
        let class = lunco_modelica::diagram::find_class_by_qualified_name(&ast, &unit.name)
            .ok_or_else(|| format!("generated unit class `{}` is missing", unit.name))?;
        let mut expected_unit_outputs = unit.outputs.clone();
        expected_unit_outputs.extend(
            aliases
                .iter()
                .filter(|(member, _, _)| unit.component_paths.iter().any(|path| path == member))
                .map(|(_, _, alias)| alias.clone()),
        );
        for component in class.components.values() {
            match &component.causality {
                Causality::Input(_) if !unit.inputs.contains(component.name.as_str()) => {
                    return Err(format!(
                        "unit `{}` declares undeclared boundary input `{}`",
                        unit.name, component.name
                    ));
                }
                Causality::Output(_)
                    if !expected_unit_outputs.contains(component.name.as_str()) =>
                {
                    return Err(format!(
                        "unit `{}` declares undeclared boundary output `{}`",
                        unit.name, component.name
                    ));
                }
                _ => {}
            }
        }
        for input in &unit.inputs {
            let Some(component) = class.components.get(input) else {
                return Err(format!(
                    "unit `{}` is missing boundary input `{input}`",
                    unit.name
                ));
            };
            if !matches!(component.causality, Causality::Input(_)) {
                return Err(format!(
                    "unit `{}` boundary `{input}` is not declared as input",
                    unit.name
                ));
            }
        }
        for output in &unit.outputs {
            let Some(component) = class.components.get(output) else {
                return Err(format!(
                    "unit `{}` is missing boundary output `{output}`",
                    unit.name
                ));
            };
            if !matches!(component.causality, Causality::Output(_)) {
                return Err(format!(
                    "unit `{}` boundary `{output}` is not declared as output",
                    unit.name
                ));
            }
        }
        for member_path in &unit.component_paths {
            let instance = instance_identifier(&network.root, member_path)?;
            let expected_type = network
                .components
                .iter()
                .find(|component| component.path == *member_path)
                .map(|component| component.model_class.as_str())
                .ok_or_else(|| {
                    format!(
                        "unit `{}` references unknown member `{member_path}`",
                        unit.name
                    )
                })?;
            let Some(component) = class.components.get(&instance) else {
                return Err(format!(
                    "unit `{}` is missing member instance `{instance}`",
                    unit.name
                ));
            };
            if component.type_name.to_string() != expected_type {
                return Err(format!(
                    "unit `{}` member `{instance}` has type `{}`, expected `{expected_type}`",
                    unit.name, component.type_name
                ));
            }
        }
        let expected_instances: BTreeSet<String> = unit
            .component_paths
            .iter()
            .map(|path| instance_identifier(&network.root, path))
            .collect::<Result<_, _>>()?;
        let native_types: BTreeSet<String> = network
            .components
            .iter()
            .map(|component| component.model_class.clone())
            .collect();
        for component in class.components.values() {
            let is_native = native_types.contains(&component.type_name.to_string());
            if is_native && !expected_instances.contains(&component.name) {
                return Err(format!(
                    "unit `{}` contains unassigned native member `{}`",
                    unit.name, component.name
                ));
            }
        }
        for (member, _, alias) in aliases {
            if !unit.component_paths.iter().any(|path| path == member) {
                continue;
            }
            let Some(component) = class.components.get(alias) else {
                return Err(format!(
                    "unit `{}` is missing promoted output `{alias}` for member `{member}`",
                    unit.name
                ));
            };
            if !matches!(component.causality, Causality::Output(_)) {
                return Err(format!(
                    "unit `{}` promoted output `{alias}` is not declared as output",
                    unit.name
                ));
            }
        }
    }
    Ok(())
}

/// Read a policy-owned unit partition and prove that it is only rearranging
/// the composed graph. USD facts stay authoritative for membership and public
/// boundaries; Rhai chooses how those members are merged into Modelica units.
fn parse_policy_units(
    value: Option<&lunco_hooks::HookValue>,
    network: &DomainNetwork,
    root: &str,
    policy_name: &str,
) -> Result<Vec<SynthesisUnit>, String> {
    let Some(value) = value else {
        return Err(format!("synthesizer `{policy_name}` must return `units`"));
    };
    let lunco_hooks::HookValue::Array(raw_units) = value else {
        return Err(format!(
            "synthesizer `{policy_name}` returned `units`, which must be an array"
        ));
    };
    let known_components: BTreeSet<String> = network
        .components
        .iter()
        .map(|component| component.path.clone())
        .collect();
    let known_inputs = &network.inputs;
    let known_outputs: BTreeSet<_> = network.outputs.keys().map(String::as_str).collect();
    let mut seen_components = BTreeSet::new();
    let mut seen_names = BTreeSet::new();
    let mut seen_instances = BTreeSet::new();
    let mut units = Vec::with_capacity(raw_units.len());

    for (index, raw_unit) in raw_units.iter().enumerate() {
        let context = format!("synthesizer `{policy_name}` units[{index}]");
        let lunco_hooks::HookValue::Map(map) = raw_unit else {
            return Err(format!("{context} must be a map"));
        };
        let name = hook_map_string(map, "name", &context)?;
        if !is_modelica_identifier(&name) {
            return Err(format!(
                "{context}.name `{name}` is not a valid Modelica identifier"
            ));
        }
        if !seen_names.insert(name.clone()) {
            return Err(format!("{context}.name `{name}` is duplicated"));
        }
        let instance = hook_map_string(map, "instance", &context)?;
        if !is_modelica_identifier(&instance) {
            return Err(format!(
                "{context}.instance `{instance}` is not a valid Modelica identifier"
            ));
        }
        if !seen_instances.insert(instance.clone()) {
            return Err(format!("{context}.instance `{instance}` is duplicated"));
        }
        if known_inputs.contains(&instance) || known_outputs.contains(instance.as_str()) {
            return Err(format!(
                "{context}.instance `{instance}` collides with a network boundary name"
            ));
        }
        let component_paths = hook_map_string_array(map, "components", &context)?;
        if component_paths.is_empty() {
            return Err(format!("{context}.components must not be empty"));
        }
        for path in &component_paths {
            if !known_components.contains(path) {
                return Err(format!(
                    "{context}.components contains `{path}`, which is not in `{root}`"
                ));
            }
            if !seen_components.insert(path.clone()) {
                return Err(format!(
                    "component `{path}` occurs in more than one policy unit"
                ));
            }
        }
        let inputs = hook_map_string_array(map, "inputs", &context)?
            .into_iter()
            .collect::<BTreeSet<_>>();
        for input in &inputs {
            if !known_inputs.contains(input) {
                return Err(format!(
                    "{context}.inputs contains `{input}`, which is not a network boundary input"
                ));
            }
        }
        let outputs = hook_map_string_array(map, "outputs", &context)?
            .into_iter()
            .collect::<BTreeSet<_>>();
        for output in &outputs {
            if !known_outputs.contains(output.as_str()) {
                return Err(format!(
                    "{context}.outputs contains `{output}`, which is not a network boundary output"
                ));
            }
        }
        units.push(SynthesisUnit {
            name,
            instance,
            component_paths,
            inputs,
            outputs,
        });
    }

    if seen_components != known_components {
        let missing = known_components
            .difference(&seen_components)
            .map(String::as_str)
            .collect::<Vec<_>>();
        return Err(format!(
            "synthesizer `{policy_name}` units do not cover every composed component; missing: {}",
            missing.join(", ")
        ));
    }
    if units.is_empty() {
        return Err(format!(
            "synthesizer `{policy_name}` returned an empty unit partition for `{root}`"
        ));
    }
    Ok(units)
}

fn parse_policy_coordinate(
    map: &[(String, lunco_hooks::HookValue)],
    key: &str,
    context: &str,
) -> Result<i32, String> {
    let value = hook_map_value(map, key)
        .and_then(lunco_hooks::HookValue::as_i64)
        .ok_or_else(|| format!("{context} must contain integer `{key}`"))?;
    i32::try_from(value)
        .map_err(|_| format!("{context}.{key} is outside Modelica coordinate range"))
}

/// Read the policy-owned unit/member diagram placements. Both sections and
/// every placement are required; Rust validates the result but never fills in
/// omitted coordinates from a second presentation policy.
fn parse_policy_layout(
    value: Option<&lunco_hooks::HookValue>,
    network: &DomainNetwork,
    units: &[SynthesisUnit],
    root: &str,
    policy_name: &str,
) -> Result<SynthesisLayout, String> {
    let Some(value) = value else {
        return Err(format!("synthesizer `{policy_name}` must return `layout`"));
    };
    let lunco_hooks::HookValue::Map(map) = value else {
        return Err(format!(
            "synthesizer `{policy_name}` returned `layout`, which must be a map"
        ));
    };
    let known_units: BTreeSet<_> = units.iter().map(|unit| unit.name.as_str()).collect();
    let known_members: BTreeSet<_> = network
        .components
        .iter()
        .map(|component| component.path.as_str())
        .collect();

    let mut layout = SynthesisLayout::default();
    for (section, key, known, target) in [
        ("unit", "units", known_units, &mut layout.unit_positions),
        (
            "member",
            "members",
            known_members,
            &mut layout.member_positions,
        ),
    ] {
        let Some(value) = hook_map_value(map, key) else {
            return Err(format!(
                "synthesizer `{policy_name}` layout must contain `{key}`"
            ));
        };
        let lunco_hooks::HookValue::Array(placements) = value else {
            return Err(format!(
                "synthesizer `{policy_name}` layout.{key} must be an array"
            ));
        };
        let mut provided = BTreeSet::new();
        for (index, placement) in placements.iter().enumerate() {
            let context = format!("synthesizer `{policy_name}` layout.{key}[{index}]");
            let lunco_hooks::HookValue::Map(placement) = placement else {
                return Err(format!("{context} must be a map"));
            };
            let identity_key = if section == "unit" { "name" } else { "path" };
            let identity = hook_map_string(placement, identity_key, &context)?;
            if !known.contains(identity.as_str()) {
                return Err(format!(
                    "{context}.{identity_key} `{identity}` is not part of `{root}`"
                ));
            }
            if !provided.insert(identity.clone()) {
                return Err(format!(
                    "{context}.{identity_key} `{identity}` is duplicated"
                ));
            }
            let x = parse_policy_coordinate(placement, "x", &context)?;
            let y = parse_policy_coordinate(placement, "y", &context)?;
            target.insert(identity, (x, y));
        }
        let provided: BTreeSet<_> = target.keys().map(String::as_str).collect();
        if provided != known {
            let missing = known.difference(&provided).copied().collect::<Vec<_>>();
            return Err(format!(
                "synthesizer `{policy_name}` layout.{key} is missing: {}",
                missing.join(", ")
            ));
        }
    }
    let mut occupied_units = BTreeMap::<(i32, i32), &str>::new();
    for (identity, position) in &layout.unit_positions {
        if let Some(previous) = occupied_units.insert(*position, identity.as_str()) {
            return Err(format!(
                "synthesizer `{policy_name}` layout.units places `{identity}` on top of `{previous}` at ({}, {})",
                position.0, position.1
            ));
        }
    }
    for unit in units {
        let mut occupied_members = BTreeMap::<(i32, i32), &str>::new();
        for identity in &unit.component_paths {
            let Some(position) = layout.member_positions.get(identity) else {
                return Err(format!(
                    "synthesizer `{policy_name}` layout.members is missing `{identity}` in unit `{}`",
                    unit.name
                ));
            };
            if let Some(previous) = occupied_members.insert(*position, identity.as_str()) {
                return Err(format!(
                    "synthesizer `{policy_name}` layout.members places `{identity}` on top of `{previous}` in unit `{}` at ({}, {})",
                    unit.name, position.0, position.1
                ));
            }
        }
    }
    Ok(layout)
}

/// The composed network, as a map an authored policy can read.
///
/// Deliberately the WHOLE graph, flat and self-describing: instance name, class,
/// constants, acausal edges, causal edges, and the wrapper boundary. A policy
/// that needs something not in here is a reason to extend this function — not a
/// reason for the policy to go read USD itself.
pub fn network_facts(
    network: &DomainNetwork,
    model_name: &str,
    classes: Option<&MemberClasses>,
) -> Result<lunco_hooks::HookValue, String> {
    use lunco_hooks::HookValue as H;
    let units = partition_network(network);
    let layout = default_synthesis_layout(network, &units);
    let member_outputs = generated_member_outputs(network, classes)?;
    let source_roots: BTreeSet<String> = network
        .components
        .iter()
        .filter_map(|component| component.model_class.split('.').next())
        .map(str::to_string)
        .collect();
    let component_paths: BTreeSet<_> = network
        .components
        .iter()
        .map(|component| component.path.as_str())
        .collect();
    let boundary_prefix = format!("{}.inputs:", network.root);
    let mut connections = BTreeSet::new();
    let mut causal_links = BTreeSet::new();
    let mut boundary_links = BTreeSet::new();
    for component in &network.components {
        let target_instance = instance_identifier(&network.root, &component.path)?;
        for (connector, targets) in &component.connectors {
            for target in targets {
                let Some((target_path, target_connector)) = target.split_once(".connectors:")
                else {
                    continue;
                };
                if !component_paths.contains(target_path) {
                    continue;
                }
                let left = (
                    component.path.clone(),
                    connector.clone(),
                    target_path.to_string(),
                    target_connector.to_string(),
                );
                let right = (
                    target_path.to_string(),
                    target_connector.to_string(),
                    component.path.clone(),
                    connector.clone(),
                );
                connections.insert(left.min(right));
            }
        }
        for (input, target) in &component.inputs {
            if let Some((source_path, output)) = target.split_once(".outputs:") {
                if component_paths.contains(source_path) {
                    causal_links.insert((
                        source_path.to_string(),
                        output.to_string(),
                        component.path.clone(),
                        target_instance.clone(),
                        input.clone(),
                    ));
                    continue;
                }
            }
            if let Some(boundary) = target.strip_prefix(&boundary_prefix).or_else(|| {
                network
                    .input_sources
                    .iter()
                    .find_map(|(name, source)| (source == target).then_some(name.as_str()))
            }) {
                boundary_links.insert((
                    boundary.to_string(),
                    component.path.clone(),
                    target_instance.clone(),
                    input.clone(),
                ));
            }
        }
    }
    let connection_facts = connections
        .into_iter()
        .map(|(left_path, left_connector, right_path, right_connector)| {
            Ok(H::map([
                ("left_path", H::str(left_path.clone())),
                (
                    "left_instance",
                    H::str(instance_identifier(&network.root, &left_path)?),
                ),
                ("left_connector", H::str(left_connector)),
                ("right_path", H::str(right_path.clone())),
                (
                    "right_instance",
                    H::str(instance_identifier(&network.root, &right_path)?),
                ),
                ("right_connector", H::str(right_connector)),
            ]))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let causal_link_facts = causal_links
        .into_iter()
        .map(
            |(source_path, source_output, target_path, target_instance, target_input)| {
                Ok(H::map([
                    ("source_path", H::str(source_path.clone())),
                    (
                        "source_instance",
                        H::str(instance_identifier(&network.root, &source_path)?),
                    ),
                    ("source_output", H::str(source_output)),
                    ("target_path", H::str(target_path)),
                    ("target_instance", H::str(target_instance)),
                    ("target_input", H::str(target_input)),
                ]))
            },
        )
        .collect::<Result<Vec<_>, String>>()?;
    let boundary_link_facts = boundary_links
        .into_iter()
        .map(|(input, target_path, target_instance, target_input)| {
            H::map([
                ("input", H::str(input)),
                ("target_path", H::str(target_path)),
                ("target_instance", H::str(target_instance)),
                ("target_input", H::str(target_input)),
            ])
        })
        .collect::<Vec<_>>();
    let boundary_output_facts = network
        .outputs
        .iter()
        .map(|(name, target)| {
            let (source_path, source_output) = target.split_once(".outputs:").ok_or_else(|| {
                format!(
                    "network output `{name}` points to malformed target `{target}`; expected `.outputs:`"
                )
            })?;
            Ok(H::map([
                ("name", H::str(name.clone())),
                ("source_path", H::str(source_path)),
                (
                    "source_instance",
                    H::str(instance_identifier(&network.root, source_path)?),
                ),
                ("source_output", H::str(source_output)),
            ]))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let member_output_facts = member_outputs
        .iter()
        .map(|(member_path, output, alias)| {
            Ok(H::map([
                ("member_path", H::str(member_path.clone())),
                (
                    "member_instance",
                    H::str(instance_identifier(&network.root, member_path)?),
                ),
                ("output", H::str(output.clone())),
                ("alias", H::str(alias.clone())),
            ]))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let components: Vec<H> = network
        .components
        .iter()
        .map(|component| {
            Ok(H::map([
                ("path", H::str(component.path.clone())),
                (
                    "instance",
                    H::str(instance_identifier(&network.root, &component.path)?),
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
                    "constant_modifications",
                    H::Array(
                        component
                            .constants
                            .iter()
                            .map(|(name, value)| {
                                H::map([
                                    ("name", H::str(name.clone())),
                                    ("value", H::Float(*value)),
                                ])
                            })
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
                ("topology_role", H::str(component.topology_role.clone())),
            ]))
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(H::map([
        ("model_name", H::str(model_name.to_string())),
        ("root", H::str(network.root.clone())),
        (
            "source_roots",
            H::Array(source_roots.into_iter().map(H::str).collect()),
        ),
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
        ("connections", H::Array(connection_facts)),
        ("causal_links", H::Array(causal_link_facts)),
        ("boundary_links", H::Array(boundary_link_facts)),
        ("boundary_outputs", H::Array(boundary_output_facts)),
        ("member_outputs", H::Array(member_output_facts)),
        (
            "units",
            H::Array(
                units
                    .into_iter()
                    .map(|unit| {
                        let unit_name = unit.name.clone();
                        H::map([
                            ("name", H::str(unit_name.clone())),
                            ("instance", H::str(unit.instance.clone())),
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
        (
            "layout",
            H::map([
                (
                    "units",
                    H::Array(
                        layout
                            .unit_positions
                            .into_iter()
                            .map(|(name, (x, y))| {
                                H::map([
                                    ("name", H::str(name)),
                                    ("x", H::Int(i64::from(x))),
                                    ("y", H::Int(i64::from(y))),
                                ])
                            })
                            .collect(),
                    ),
                ),
                (
                    "members",
                    H::Array(
                        layout
                            .member_positions
                            .into_iter()
                            .map(|(path, (x, y))| {
                                H::map([
                                    ("path", H::str(path)),
                                    ("x", H::Int(i64::from(x))),
                                    ("y", H::Int(i64::from(y))),
                                ])
                            })
                            .collect(),
                    ),
                ),
            ]),
        ),
    ]))
}

/// Partition a composed network once, at the synthesizer boundary.
///
/// Acausal connector edges and internal causal output-to-input edges both keep
/// components in one composite unit. Boundary connections do not: they are
/// the public FMI/SSP-style interface of the containing network root. The
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
        .enumerate()
        .map(|(unit_index, component_paths)| {
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
                instance: unit_instance_identifier(&name, unit_index),
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

/// Synthesize a normalized actuator command map from composed USD geometry.
///
/// This is deliberately a separate synthesizer from `acausal-network`: force
/// actuator prims are physical USD members, not Modelica component facets. The
/// authored geometry supplies each actuator's moment contribution and an
/// explicit relationship to the generated network's command output. Modelica
/// owns the runtime clamp and matrix operation. A rank-deficient actuator
/// arrangement is an authoring error, not a reason to silently select a
/// different allocation policy.
pub struct ActuatorWrenchSynthesizer;

impl DomainSynthesizer for ActuatorWrenchSynthesizer {
    fn name(&self) -> &str {
        ACTUATOR_WRENCH_SYNTHESIZER
    }

    fn synthesize(
        &self,
        view: &dyn ComposedReader,
        root: &SdfPath,
        model_name: &str,
        _ctx: &SynthContext<'_>,
    ) -> Result<SynthOutcome, Vec<DomainProjectionError>> {
        if !is_domain_network_root(view, root) {
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
            let command_targets = view.rel_targets(&path, "lunco:forceActuator:commandSource");
            if command_targets.len() != 1 {
                return Err(vec![DomainProjectionError {
                    path: path.to_string(),
                    message: "actuator-wrench collection members must target exactly one \
                              scalar output with lunco:forceActuator:commandSource"
                        .into(),
                }]);
            }
            let command_target = &command_targets[0];
            let Some((command_root, command_property)) = command_target.split_property() else {
                return Err(vec![DomainProjectionError {
                    path: format!("{path}.lunco:forceActuator:commandSource"),
                    message: format!(
                        "command source `{command_target}` must target a scalar output property"
                    ),
                }]);
            };
            let Some(command) = command_property.strip_prefix("outputs:") else {
                return Err(vec![DomainProjectionError {
                    path: format!("{path}.lunco:forceActuator:commandSource"),
                    message: format!(
                        "command source `{command_target}` must target an `outputs:` property"
                    ),
                }]);
            };
            if command_root != *root || command.is_empty() || !is_modelica_identifier(command) {
                return Err(vec![DomainProjectionError {
                    path: format!("{path}.lunco:forceActuator:commandSource"),
                    message: format!(
                        "command source `{command_target}` must target a valid Modelica output \
                         on network root `{root}`"
                    ),
                }]);
            }
            let command = command.to_string();
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
                    path: format!("{root}.lunco:forceActuator:commandSource"),
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
        let (wrench_matrix, allocation_step) =
            actuator_wrench_matrix(&columns).map_err(|message| {
                vec![DomainProjectionError {
                    path: root_string.clone(),
                    message,
                }]
            })?;
        let component_paths = actuators
            .values()
            .map(|(path, _)| path.to_string())
            .collect::<Vec<_>>();
        let facts = lunco_hooks::HookValue::Map(vec![
            (
                "model_name".to_string(),
                lunco_hooks::HookValue::str(model_name),
            ),
            (
                "root".to_string(),
                lunco_hooks::HookValue::str(root_string.clone()),
            ),
            (
                "inputs".to_string(),
                lunco_hooks::HookValue::Array(
                    inputs
                        .iter()
                        .cloned()
                        .map(lunco_hooks::HookValue::str)
                        .collect(),
                ),
            ),
            (
                "outputs".to_string(),
                lunco_hooks::HookValue::Array(
                    outputs
                        .iter()
                        .cloned()
                        .map(lunco_hooks::HookValue::str)
                        .collect(),
                ),
            ),
            (
                "actuator_paths".to_string(),
                lunco_hooks::HookValue::Array(
                    component_paths
                        .iter()
                        .cloned()
                        .map(lunco_hooks::HookValue::str)
                        .collect(),
                ),
            ),
            (
                "wrench_matrix".to_string(),
                lunco_hooks::HookValue::Array(
                    (0..6)
                        .map(|row| {
                            lunco_hooks::HookValue::Array(
                                wrench_matrix
                                    .iter()
                                    .map(|column| lunco_hooks::HookValue::Float(column[row]))
                                    .collect(),
                            )
                        })
                        .collect(),
                ),
            ),
            (
                "allocation_step".to_string(),
                lunco_hooks::HookValue::Float(allocation_step),
            ),
            (
                "actuator_count".to_string(),
                lunco_hooks::HookValue::Int(wrench_matrix.len() as i64),
            ),
        ]);
        let value = lunco_hooks::invoke("synth.actuator-wrench", &[facts]).ok_or_else(|| {
            vec![DomainProjectionError {
                path: root_string.clone(),
                message: "actuator-wrench is selected but its Rhai synthesis policy is not registered".into(),
            }]
        })?.map_err(|error| {
            vec![DomainProjectionError {
                path: root_string.clone(),
                message: format!("actuator-wrench synthesis policy failed: {}", error.0),
            }]
        })?;
        let lunco_hooks::HookValue::Map(map) = value else {
            return Err(vec![DomainProjectionError {
                path: root_string,
                message: "actuator-wrench synthesis policy must return a map with a Modelica `source` key".into(),
            }]);
        };
        let Some(source) = map
            .iter()
            .find_map(|(key, value)| (key == "source").then(|| value.as_str()))
            .flatten()
        else {
            return Err(vec![DomainProjectionError {
                path: root_string,
                message: "actuator-wrench synthesis policy returned no string `source` key".into(),
            }]);
        };
        parse_validated_root_interface(source, model_name, &inputs, &outputs, &[]).map_err(
            |message| {
                vec![DomainProjectionError {
                    path: root_string.clone(),
                    message: format!(
                        "actuator-wrench synthesis policy returned invalid Modelica: {message}"
                    ),
                }]
            },
        )?;
        let source_roots = parse_policy_source_roots(
            hook_map_value(&map, "source_roots"),
            ACTUATOR_WRENCH_SYNTHESIZER,
        )
        .map_err(|message| {
            vec![DomainProjectionError {
                path: root_string.clone(),
                message,
            }]
        })?;
        // Force actuators are Avian/USD members, not Modelica component
        // members. Do not invent a generated unit class for them: the policy
        // emits one ordinary root model whose real Modelica child is the
        // allocator, and the source UI must not promise a drill-down class
        // that does not exist.
        Ok(SynthOutcome::Ready(Box::new(SynthesisPlan {
            source: source.to_string(),
            inputs,
            outputs,
            component_paths,
            source_roots,
            members: Vec::new(),
            member_output_aliases: Vec::new(),
            units: Vec::new(),
            layout: SynthesisLayout::default(),
            communication_period_secs: lunco_modelica::DEFAULT_COMMUNICATION_PERIOD_SECS,
        })))
    }
}

fn strip_connection_suffix(name: &str) -> String {
    name.strip_suffix(".connect").unwrap_or(name).to_string()
}

/// Return the authored six-component wrench matrix and a stable projected-
/// gradient step for the bounded actuator solve. Each column is one actuator's
/// maximum body force and torque, so the Modelica allocator solves the actual
/// one-sided least-squares problem instead of clamping a signed pseudo-inverse.
fn actuator_wrench_matrix(
    actuators: &[lunco_cosim::ForceActuator],
) -> Result<(Vec<[f64; 6]>, f64), String> {
    let columns: Vec<[f64; 6]> = actuators
        .iter()
        .map(|actuator| {
            let direction = actuator.direction_local.normalize_or_zero().as_dvec3();
            let force = direction * actuator.max_force_n;
            let torque = actuator.local_position.as_dvec3().cross(direction) * actuator.max_force_n;
            [force.x, force.y, force.z, torque.x, torque.y, torque.z]
        })
        .collect();
    if columns.iter().flatten().any(|value| !value.is_finite()) {
        return Err("actuator-wrench geometry produced a non-finite torque column".into());
    }

    let gram_trace = columns
        .iter()
        .flatten()
        .map(|value| value * value)
        .sum::<f64>();
    if !gram_trace.is_finite() || gram_trace <= f64::EPSILON {
        return Err("actuator-wrench geometry has no finite physical wrench authority".into());
    }
    // `||B||_F²` bounds the largest eigenvalue of BᵀB. A 0.9 margin keeps the
    // fixed projected-gradient solve stable for every authored arrangement.
    Ok((columns, 0.9 / gram_trace))
}

/// Deterministic layout facts supplied to the synthesis policy.
///
/// The policy owns the generated Modelica and diagram; Rust only supplies a
/// stable topology-derived starting arrangement in the facts map.
const GENERATED_UNIT_COLUMN_SPACING: i32 = 150;
const GENERATED_UNIT_ROW_SPACING: i32 = 100;
const NETWORK_LAYOUT_ORIGIN_X: i32 = -100;
const NETWORK_LAYOUT_LAYER_SPACING: i32 = 55;
const NETWORK_LAYOUT_ROW_SPACING: i32 = 22;
const NETWORK_LAYOUT_ROW_CENTER_STEP: i32 = 2;

/// Deterministic default Modelica name for a synthesized unit instance. A
/// policy may replace this name in its returned unit table; this helper is not
/// a visual emitter or a second policy.
fn unit_instance_identifier(unit_name: &str, unit_index: usize) -> String {
    let encoded_path = unit_name.strip_prefix("Unit_").unwrap_or(unit_name);
    let path_parts = encoded_path.split("_x2f_").collect::<Vec<_>>();
    let readable = path_parts
        .iter()
        .rev()
        .take(2)
        .copied()
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join("_");
    modelica_identifier(&format!("unit_{}_{}", unit_index + 1, readable))
}

/// The deterministic presentation supplied in the facts map. The Rhai policy
/// may replace it, but the graph reader remains the only source of membership.
fn generated_unit_layout(units: &[SynthesisUnit]) -> BTreeMap<String, (i32, i32)> {
    let columns = (units.len() as f64).sqrt().ceil().max(1.0) as i32;
    let rows = ((units.len() as i32 + columns - 1) / columns).max(1);
    let origin_x = -((columns - 1) * GENERATED_UNIT_COLUMN_SPACING) / 2;
    let origin_y = ((rows - 1) * GENERATED_UNIT_ROW_SPACING) / 2;

    units
        .iter()
        .enumerate()
        .map(|(index, unit)| {
            let index = index as i32;
            (
                unit.name.clone(),
                (
                    origin_x + (index % columns) * GENERATED_UNIT_COLUMN_SPACING,
                    origin_y - (index / columns) * GENERATED_UNIT_ROW_SPACING,
                ),
            )
        })
        .collect()
}

fn default_synthesis_layout(network: &DomainNetwork, units: &[SynthesisUnit]) -> SynthesisLayout {
    SynthesisLayout {
        unit_positions: generated_unit_layout(units),
        member_positions: network_layout(network),
    }
}

/// Place generated components by topology, not source-file order. These are
/// facts for the policy and telemetry mapping; Rust does not emit their schema.
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
                    if let Some(component_neighbours) = neighbours.get_mut(&component.path) {
                        component_neighbours.insert(target.to_string());
                    }
                    if let Some(target_neighbours) = neighbours.get_mut(target) {
                        target_neighbours.insert(component.path.clone());
                    }
                }
            }
        }
        for target in component.inputs.values() {
            if let Some((source, _)) = target.split_once(".outputs:") {
                if paths.contains(source) {
                    if let Some(source_neighbours) = neighbours.get_mut(source) {
                        source_neighbours.insert(component.path.clone());
                    }
                    if let Some(component_incoming) = incoming.get_mut(&component.path) {
                        *component_incoming += 1;
                    }
                }
            }
        }
    }
    let mut roots: Vec<_> = network
        .components
        .iter()
        .filter(|component| incoming.get(&component.path).copied() == Some(0))
        .map(|component| component.path.clone())
        .collect();
    roots.sort();
    let mut rank = BTreeMap::new();
    let mut queue = VecDeque::new();
    for root in roots {
        if !rank.contains_key(&root) {
            queue.push_back((root, 0usize));
        }
        while let Some((path, layer)) = queue.pop_front() {
            if rank.contains_key(&path) {
                continue;
            }
            rank.insert(path.clone(), layer);
            for neighbour in neighbours.get(&path).into_iter().flatten() {
                if !rank.contains_key(neighbour) {
                    queue.push_back((neighbour.clone(), layer + 1));
                }
            }
        }
    }
    for path in &paths {
        if rank.contains_key(path) {
            continue;
        }
        queue.push_back((path.clone(), rank.values().copied().max().unwrap_or(0) + 1));
        while let Some((path, layer)) = queue.pop_front() {
            if rank.contains_key(&path) {
                continue;
            }
            rank.insert(path.clone(), layer);
            for neighbour in neighbours.get(&path).into_iter().flatten() {
                if !rank.contains_key(neighbour) {
                    queue.push_back((neighbour.clone(), layer + 1));
                }
            }
        }
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
                (
                    NETWORK_LAYOUT_ORIGIN_X + layer as i32 * NETWORK_LAYOUT_LAYER_SPACING,
                    (count - 1 - row as i32 * NETWORK_LAYOUT_ROW_CENTER_STEP)
                        * NETWORK_LAYOUT_ROW_SPACING,
                ),
            );
        }
    }
    placements
}
/// Stable Modelica name for a causal output promoted from a generated member.
///
/// The wrapper is the only runtime solver participant, so member outputs that
/// remain visible in USD need a first-class boundary name. The prefix keeps
/// these derived names separate from authored network outputs; the escaped
/// instance identifier keeps the mapping injective for arbitrary USD paths.
pub(crate) fn generated_member_output_name(
    root: &str,
    member: &str,
    output: &str,
) -> Result<String, String> {
    Ok(format!(
        "__member_{}_{}",
        instance_identifier(root, member)?,
        modelica_identifier(output)
    ))
}

fn generated_member_outputs(
    network: &DomainNetwork,
    classes: Option<&MemberClasses>,
) -> Result<Vec<(String, String, String)>, String> {
    let mut member_outputs = Vec::new();
    for component in &network.components {
        let modelica_outputs =
            classes.and_then(|classes| classes.output_names(&component.source_asset));
        for output in component
            .declared_outputs
            .iter()
            .filter(|output| modelica_outputs.is_none_or(|outputs| outputs.contains(*output)))
        {
            member_outputs.push((
                component.path.clone(),
                output.clone(),
                generated_member_output_name(&network.root, &component.path, output)?,
            ));
        }
    }
    Ok(member_outputs)
}

/// A domain synthesis request that owns no OpenUSD handles.
///
/// The initial asset projection plan is prepared by the asset loader and is
/// shared by every network task. The task owns the policy call and source
/// validation; the main thread only commits the resulting ECS/Modelica state.
struct PendingDomainProjection {
    entity: Entity,
    stage_id: AssetId<UsdStageAsset>,
    stage_generation: u64,
    root_path: String,
    model_name: String,
    requested: String,
    plan: Arc<lunco_usd_bevy::UsdStageProjectionPlan>,
    task: Task<Result<SynthOutcome, Vec<DomainProjectionError>>>,
}

/// In-flight domain synthesis owned by the scene projection lifecycle.
///
/// One network has one synthesis owner, and completion is fenced by the USD
/// entity and canonical-stage generation before it can publish a result.
#[derive(Resource, Default)]
pub struct PendingDomainProjections {
    tasks: Vec<PendingDomainProjection>,
}

fn queue_domain_projection(
    pending: &mut PendingDomainProjections,
    entity: Entity,
    stage_id: AssetId<UsdStageAsset>,
    stage_generation: u64,
    root_path: &SdfPath,
    model_name: String,
    requested: String,
    synthesizer: Arc<dyn DomainSynthesizer>,
    plan: Arc<lunco_usd_bevy::UsdStageProjectionPlan>,
    classes: MemberClasses,
) {
    let root_path_string = root_path.to_string();
    let task_root = root_path.clone();
    let task_model_name = model_name.clone();
    let task_plan = plan.clone();
    let task = AsyncComputeTaskPool::get().spawn(async move {
        let view: &dyn ComposedReader = task_plan.as_ref();
        let context = SynthContext { classes: &classes };
        synthesizer.synthesize(view, &task_root, &task_model_name, &context)
    });
    pending.tasks.push(PendingDomainProjection {
        entity,
        stage_id,
        stage_generation,
        root_path: root_path_string,
        model_name,
        requested,
        plan,
        task,
    });
}

fn resolve_domain_synthesizer(
    view: &dyn ComposedReader,
    root_path: &SdfPath,
    prim_path: &str,
    registry: &SynthesizerRegistry,
) -> Option<(String, Arc<dyn DomainSynthesizer>)> {
    let requested = match select_synthesizer_name(view, root_path) {
        Ok(name) => name,
        Err(message) => {
            error!("[domain-projection] `{prim_path}` rejected: {message}");
            return None;
        }
    };
    let Some(synthesizer) = registry.get(&requested).cloned() else {
        let known = registry.names().join(", ");
        error!(
            "[domain-projection] `{prim_path}` names synthesizer `{requested}`, which is not \
             registered (known: {known}) — the scope is not projected."
        );
        return None;
    };
    Some((requested, synthesizer))
}

/// Commit one completed synthesis result on the main thread.
///
/// Rhai execution, graph extraction, and generated-source validation happen in
/// the task above. This function is the single publication path for both the
/// prepared startup plan and the canonical live-edit reader, so the Modelica
/// lifecycle, signal layout, and generated-source diagnostics cannot diverge.
fn commit_domain_projection(
    commands: &mut Commands,
    entity: Entity,
    prim: &UsdPrimPath,
    previous: Option<&DomainProjectionState>,
    installed_model: Option<&ModelicaModel>,
    root_path: &SdfPath,
    view: &dyn ComposedReader,
    classes: &MemberClasses,
    channels: &ModelicaChannels,
    requested: &str,
    model_name: &str,
    synthesized: Result<SynthOutcome, Vec<DomainProjectionError>>,
    notices: &mut MessageWriter<ModelicaNotice>,
) -> bool {
    let synthesized = match synthesized {
        Ok(synthesized) => synthesized,
        Err(errors) => {
            let message = errors
                .iter()
                .map(|error| format!("{}: {}", error.path, error.message))
                .collect::<Vec<_>>()
                .join("; ");
            let fingerprint = source_fingerprint(&format!("projection-error:{message}"));
            if previous.is_some_and(|state| state.fingerprint == fingerprint) {
                return false;
            }
            notices.write(ModelicaNotice {
                level: NoticeLevel::Error,
                text: format!("[{model_name}] Projection error: {message}"),
            });
            error!("[domain-projection] `{}` rejected: {message}", prim.path);
            retire_sim_interface(commands, entity);
            if let Some(model) = installed_model {
                queue_retire_generated_document(commands, model.document);
            }
            commands
                .entity(entity)
                .remove::<(UsdModelicaPortContract, ModelicaSignalLayout)>();
            commands.entity(entity).try_insert((
                ModelicaModel {
                    model_name: model_name.to_string(),
                    source_uri: format!("generated://{model_name}.mo"),
                    session_id: installed_model.map_or(1, |model| model.session_id + 1),
                    is_stepping: false,
                    is_compiling: false,
                    last_error: Some(message.clone()),
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
                    source_roots: Vec::new(),
                    member_output_aliases: Vec::new(),
                    units: Vec::new(),
                    boundary_inputs: Vec::new(),
                    boundary_outputs: Vec::new(),
                    layout: SynthesisLayout::default(),
                    projection_error: Some(message),
                },
            ));
            return false;
        }
    };

    if matches!(synthesized, SynthOutcome::Pending) {
        return false;
    }
    let SynthOutcome::Ready(synthesized) = synthesized else {
        if previous.is_some() {
            retire_sim_interface(commands, entity);
            if let Some(model) = installed_model {
                queue_retire_generated_document(commands, model.document);
            }
            commands.entity(entity).remove::<(
                ModelicaModel,
                ModelicaSignalLayout,
                UsdSourcedCosim,
                UsdModelicaPortContract,
                DomainProjectionState,
                GeneratedModelicaSource,
            )>();
        }
        return false;
    };

    let component_count = synthesized.component_paths.len();
    let source = synthesized.source;
    let source_for_diagnostics = source.clone();
    let fingerprint = source_fingerprint(&source);
    if previous.is_some_and(|state| state.fingerprint == fingerprint) {
        return false;
    }

    // ONE parse-and-extract, shared with `cosim::dispatch_loaded_modelica_sources`.
    let interface = parse_model_interface(&source, "usd-network-projection.mo");
    let compiled_name = interface
        .model_name
        .unwrap_or_else(|| model_name.to_string());
    let declared_output_ports = interface.outputs.clone();
    let session_id = installed_model.map_or(1, |model| model.session_id + 1);
    let doc_uri = format!("generated://{model_name}.mo");
    let mut model = ModelicaModel {
        model_name: compiled_name.clone(),
        source_uri: doc_uri.clone(),
        parameters: interface.parameters,
        inputs: interface.inputs,
        communication_period_secs: synthesized.communication_period_secs,
        session_id,
        is_stepping: true,
        is_compiling: true,
        resume_after_compile: true,
        ..default()
    };
    let member_output_aliases = synthesized
        .member_output_aliases
        .iter()
        .filter(|(_, _, alias)| interface.outputs.contains(alias))
        .cloned()
        .collect::<Vec<_>>();
    let signal_layout = match generated_signal_layout(
        view,
        root_path,
        &prim.path,
        &synthesized.outputs,
        &synthesized.members,
        &member_output_aliases,
        &synthesized.units,
        classes,
    ) {
        Ok(layout) => layout,
        Err(message) => {
            let message = format!("generated signal layout failed: {message}");
            model.is_stepping = false;
            model.is_compiling = false;
            model.last_error = Some(message.clone());
            notices.write(ModelicaNotice {
                level: NoticeLevel::Error,
                text: format!("[{}] Projection error: {message}", model.model_name),
            });
            error!("[domain-projection] {} rejected: {message}", prim.path);
            retire_sim_interface(commands, entity);
            commands.entity(entity).try_insert(model);
            return false;
        }
    };
    let projection_error = match channels.tx.send(ModelicaCommand::Compile {
        entity,
        session_id,
        model_name: compiled_name,
        source,
        doc_uri: doc_uri.clone(),
        extra_sources: Vec::new(),
        parameter_overrides: Vec::new(),
        stream: None,
        // The worker, not this projector, owns backend selection and DAE
        // lowering for generated domain networks.
        realtime_safe: false,
    }) {
        Ok(()) => {
            info!(
                "[domain-projection] compiling `{}` from {} component(s) via `{requested}` as \
                 generated://{}.mo",
                prim.path, component_count, model_name
            );
            None
        }
        Err(error) => {
            let message = format!("could not dispatch generated model compile: {error}");
            model.is_stepping = false;
            model.is_compiling = false;
            model.last_error = Some(message.clone());
            notices.write(ModelicaNotice {
                level: NoticeLevel::Error,
                text: format!("[{}] Compile error: {message}", model.model_name),
            });
            Some(message)
        }
    };
    let generated_source = GeneratedModelicaSource {
        network_root: prim.path.clone(),
        doc_uri: doc_uri.clone(),
        source: source_for_diagnostics,
        component_paths: synthesized.component_paths,
        members: synthesized.members,
        source_roots: synthesized.source_roots.into_iter().collect(),
        member_output_aliases,
        units: synthesized.units,
        boundary_inputs: synthesized.inputs.iter().cloned().collect(),
        boundary_outputs: synthesized.outputs.iter().cloned().collect(),
        layout: synthesized.layout,
        projection_error,
    };
    retire_sim_interface(commands, entity);
    commands.entity(entity).try_insert((
        model,
        signal_layout,
        UsdSourcedCosim,
        lunco_core::PortSurfacePending,
        UsdModelicaPortContract::new(synthesized.inputs.iter().cloned(), declared_output_ports),
        crate::cosim::UsdModelicaSchedule {
            communication_period_secs: synthesized.communication_period_secs,
        },
        DomainProjectionState { fingerprint },
        generated_source,
    ));
    true
}

/// Reactively compile every prim containing a standard component collection of
/// Modelica program facets. The generated source is runtime projection only.
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
    canonical: NonSend<CanonicalStages>,
    dirty: Res<WiringDirty>,
    // A member class landing is the projector's third trigger: the networks that
    // returned `Pending` have to be re-asked, and no prim spawned or changed.
    mut projection: ParamSet<(ResMut<ProjectionDirty>, ResMut<PendingDomainProjections>)>,
    classes: Res<MemberClasses>,
    registry: Res<SynthesizerRegistry>,
    channels: Option<Res<ModelicaChannels>>,
    mut notices: MessageWriter<ModelicaNotice>,
) {
    let started = web_time::Instant::now();
    let mut projected = 0usize;
    let mut projection_dirty = projection.p0();
    if !projection_is_due_from_flags(
        !added.is_empty(),
        !identity_added.is_empty(),
        dirty.0,
        projection_dirty.0,
    ) {
        return;
    }
    // Identity assignment is per prim during a runtime-instance spawn. Do
    // not turn one descendant's identity transition into a re-synthesis of
    // every existing prim: only a wiring/source invalidation needs the full
    // stage projection. The query iteration remains cheap, while the stage
    // and policy reads below are reserved for the changed entities.
    let full_reprojection = dirty.0 || projection_dirty.0;
    projection_dirty.0 = false;
    drop(projection_dirty);
    let mut pending = projection.p1();
    if full_reprojection {
        // A pending task captured the previous class/source and wiring view.
        // Drop it before queuing the new transaction; otherwise a late
        // `Pending` result can consume the invalidation and leave the network
        // without another trigger. The task owns no mutable world state, so
        // cancellation is the complete invalidation operation.
        pending.tasks.clear();
    }
    let Some(channels) = channels else { return };
    for (entity, prim, previous, installed_model) in &prims {
        if !full_reprojection && !added.contains(entity) && !identity_added.contains(entity) {
            continue;
        }
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
        let Some(stage_asset) = stages.get(&prim.stage_handle) else {
            continue;
        };
        let (reader, stage_generation) = canonical.reader_for(id, stage_asset);
        let Ok(root_path) = SdfPath::new(&prim.path) else {
            continue;
        };
        if stage_generation == 0 {
            if pending.tasks.iter().any(|task| task.entity == entity) {
                continue;
            }
            let plan = stages
                .get(&prim.stage_handle)
                .map(|asset| asset.projection_plan.clone())
                .expect("loaded USD asset always carries a prepared projection plan");
            let plan_view: &dyn ComposedReader = &reader;
            if !is_domain_network_root(plan_view, &root_path) {
                continue;
            }
            let Some((requested, synthesizer)) =
                resolve_domain_synthesizer(plan_view, &root_path, &prim.path, &registry)
            else {
                continue;
            };
            let model_name = network_model_name(&prim.path, instance_id);
            queue_domain_projection(
                &mut pending,
                entity,
                id,
                stage_generation,
                &root_path,
                model_name,
                requested,
                synthesizer,
                plan,
                classes.clone(),
            );
            continue;
        }
        // Domain projection owns only prims with the standard component
        // collection.  Keep this structural gate ahead of synthesizer
        // selection: deriving ownership for an ordinary prim would walk its
        // collection metadata even though it cannot be a network root.
        if !is_domain_network_root(&reader, &root_path) {
            continue;
        }
        // Domain ownership is derived from the typed member role schemas. A
        // domain API may still explicitly select a registered non-default
        // policy for a generic Modelica collection; physical actuator
        // collections have no exposed selector and are classified from their
        // `LunCoForceActuatorAPI` members.
        let Some((requested, synthesizer)) =
            resolve_domain_synthesizer(&reader, &root_path, &prim.path, &registry)
        else {
            continue;
        };
        let model_name = network_model_name(&prim.path, instance_id);
        let synthesized = synthesizer.synthesize(
            &reader,
            &root_path,
            &model_name,
            &SynthContext { classes: &classes },
        );
        if commit_domain_projection(
            &mut commands,
            entity,
            prim,
            previous,
            installed_model,
            &root_path,
            &reader,
            &classes,
            &channels,
            &requested,
            &model_name,
            synthesized,
            &mut notices,
        ) {
            projected += 1;
        }
        continue;
    }
    if projected > 0 {
        bevy::log::debug!(
            "[domain-projection] prepared {projected} network(s) in {:.2} ms",
            started.elapsed().as_secs_f64() * 1_000.0
        );
    }
}

/// Publish completed startup synthesis tasks without making the UI schedule
/// wait for Rhai, network extraction, or generated-source validation.
pub fn poll_domain_projection_tasks(
    mut commands: Commands,
    mut pending: ResMut<PendingDomainProjections>,
    prims: Query<(
        &UsdPrimPath,
        Option<&DomainProjectionState>,
        Option<&ModelicaModel>,
    )>,
    stages: Res<Assets<UsdStageAsset>>,
    canonical: NonSend<CanonicalStages>,
    classes: Res<MemberClasses>,
    channels: Option<Res<ModelicaChannels>>,
    mut notices: MessageWriter<ModelicaNotice>,
) {
    let Some(channels) = channels else { return };
    let mut index = 0;
    while index < pending.tasks.len() {
        let ready = block_on(future::poll_once(&mut pending.tasks[index].task));
        let Some(synthesized) = ready else {
            index += 1;
            continue;
        };
        let task = pending.tasks.swap_remove(index);
        let Ok((prim, previous, installed_model)) = prims.get(task.entity) else {
            continue;
        };
        if prim.stage_handle.id() != task.stage_id {
            continue;
        }
        let Some(_stage_asset) = stages.get(&prim.stage_handle) else {
            continue;
        };
        if canonical.generation_for(task.stage_id) != task.stage_generation {
            continue;
        }
        let Ok(root_path) = SdfPath::new(&task.root_path) else {
            continue;
        };
        let view: &dyn ComposedReader = task.plan.as_ref();
        commit_domain_projection(
            &mut commands,
            task.entity,
            prim,
            previous,
            installed_model,
            &root_path,
            view,
            &classes,
            &channels,
            &task.requested,
            &task.model_name,
            synthesized,
            &mut notices,
        );
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
    mut generated_metadata: ResMut<lunco_modelica::state::GeneratedModelicaSources>,
) {
    for (entity, source, mut model) in &mut generated {
        // Projection errors are represented by an empty diagnostic source and
        // must not create a misleading editable-looking blank document.
        if source.source.is_empty() {
            continue;
        }
        // Generated documents use the same source-aware class resolver as
        // authored Modelica documents. Request every referenced bundled root
        // asynchronously; the canvas shows an explicit loading state until
        // the shared engine publishes the generic completion notification.
        if let Some(handle) = lunco_modelica::engine_resource::global_engine_handle() {
            let mut roots: BTreeSet<String> = source.source_roots.iter().cloned().collect();
            roots.extend(
                source
                    .members
                    .iter()
                    .filter_map(|(_, _, class)| class.split('.').next())
                    .map(str::to_string),
            );
            for root in roots {
                let _ = handle.ensure_library_root_async(&root);
            }
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
        generated_metadata.dirty = true;
    }
}

/// Remove the ephemeral source/document metadata when a generated component is
/// removed for any reason, including scene despawn. The normal Modelica
/// cleanup intentionally keeps authored documents, so generated lifecycle has
/// its own narrowly classified observer.
pub fn on_remove_generated_source(
    trigger: On<Remove, GeneratedModelicaSource>,
    source_query: Query<(&GeneratedModelicaSource, Option<&ModelicaModel>)>,
    mut documents: Option<ResMut<lunco_modelica::state::ModelicaDocumentRegistry>>,
    mut generated: Option<ResMut<lunco_modelica::state::GeneratedModelicaSources>>,
) {
    let (network_root, doc_uri, model_document) = source_query
        .get(trigger.entity)
        .map(|(source, model)| {
            (
                Some(source.network_root.clone()),
                Some(source.doc_uri.clone()),
                model.map(|m| m.document),
            )
        })
        .unwrap_or((None, None, None));
    let document = documents.as_deref_mut().and_then(|registry| {
        let document = model_document
            .filter(|document| !document.is_unassigned())
            .or_else(|| {
                let model_name = doc_uri
                    .as_deref()?
                    .strip_prefix("generated://")?
                    .strip_suffix(".mo")?;
                registry.find_bundled(&format!("generated/{model_name}.mo"))
            })?;
        let is_generated = registry
            .host(document)
            .is_some_and(|host| lunco_modelica::state::is_generated_document(host.document()));
        if is_generated {
            registry.remove_document(document);
            Some(document)
        } else {
            None
        }
    });
    if let Some(metadata) = generated.as_deref_mut() {
        metadata.entries.retain(|entry| {
            network_root
                .as_deref()
                .is_none_or(|root| entry.network_root != root)
                && document.is_none_or(|doc| entry.document != doc)
        });
        metadata.dirty = true;
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
                model_name: model
                    .map(|m| m.model_name.clone())
                    .unwrap_or_else(|| source.network_root.trim_matches('/').replace('/', "_")),
                source: source.source.clone(),
                component_paths: source.component_paths.clone(),
                units: source
                    .units
                    .iter()
                    .map(|unit| lunco_modelica::state::GeneratedModelicaUnit {
                        name: unit.name.clone(),
                        instance: unit.instance.clone(),
                        members: unit.component_paths.clone(),
                        inputs: unit.inputs.iter().cloned().collect(),
                        outputs: unit.outputs.iter().cloned().collect(),
                    })
                    .collect(),
                members: source.members.clone(),
                source_roots: source.source_roots.clone(),
                boundary_inputs: source.boundary_inputs.clone(),
                boundary_outputs: source.boundary_outputs.clone(),
                member_output_aliases: source.member_output_aliases.clone(),
                projection_error: source.projection_error.clone(),
            },
        )
        .collect();
    generated.dirty = false;
}

/// Change gate for the generated metadata publisher. The resource flag covers
/// document-link and removal changes, while ECS change detection covers the
/// generated source projection itself. Runtime solver output is deliberately
/// outside this metadata contract.
pub fn generated_sources_need_publish(
    changed: Query<(), Changed<GeneratedModelicaSource>>,
    generated: Res<lunco_modelica::state::GeneratedModelicaSources>,
) -> bool {
    generated.dirty || !changed.is_empty()
}

/// `GeneratedModelicaSource` — read back the exact Modelica text a projected
/// network was compiled from.
///
/// `curl … {"type":"ExecuteCommand","command":"GeneratedModelicaSource","params":{}}` lists every
/// projected network; `{"network_root":"/Rover"}` returns one. This
/// is the read path for the `generated://…` documents the compiler reports
/// errors against, and the only way to see what USD actually emitted.
pub struct GeneratedSourceProvider;

impl lunco_api::ApiQueryProvider for GeneratedSourceProvider {
    fn name(&self) -> &'static str {
        "GeneratedModelicaSource"
    }

    fn execute(&self, world: &World, params: &serde_json::Value) -> lunco_api::ApiResponse {
        let wanted = params
            .get("network_root")
            .and_then(|value| value.as_str())
            .map(str::to_string);
        let Some(mut q) = bevy::ecs::query::QueryState::<(
            &GeneratedModelicaSource,
            Option<&ModelicaModel>,
        )>::try_new(world) else {
            return lunco_api::ApiResponse::error(
                lunco_api::ApiErrorCode::InternalError,
                "GeneratedModelicaSource: ECS query is unavailable",
            );
        };
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
                    "projection_error": generated.projection_error,
                    "boundary_inputs": generated.boundary_inputs,
                    "boundary_outputs": generated.boundary_outputs,
                    "member_output_aliases": generated.member_output_aliases,
                    "components": generated.component_paths,
                    "members": generated
                        .members
                        .iter()
                        .map(|(prim, asset, class)| serde_json::json!({
                            "prim": prim, "source_asset": asset, "class": class,
                        }))
                        .collect::<Vec<_>>(),
                    "source_roots": generated.source_roots,
                    "units": generated
                        .units
                        .iter()
                        .map(|unit| serde_json::json!({
                            "name": unit.name,
                            "instance": unit.instance,
                            "components": unit.component_paths,
                            "inputs": unit.inputs,
                            "outputs": unit.outputs,
                        }))
                        .collect::<Vec<_>>(),
                    "layout": {
                        "units": generated
                            .layout
                            .unit_positions
                            .iter()
                            .map(|(name, (x, y))| serde_json::json!({
                                "name": name, "x": x, "y": y,
                            }))
                            .collect::<Vec<_>>(),
                        "members": generated
                            .layout
                            .member_positions
                            .iter()
                            .map(|(path, (x, y))| serde_json::json!({
                                "path": path, "x": x, "y": y,
                            }))
                            .collect::<Vec<_>>(),
                    },
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
/// scopes with the same leaf name. Including the composed prim path also keeps
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

const COMMUNICATION_PERIOD_ATTR: &str = "lunco:program:communicationPeriod";

/// Aggregate member communication periods for one generated Modelica solver.
///
/// Periods are compared by their validated master-tick lattice index rather
/// than raw floating-point spelling, so `0.1` and an authored six-tick value
/// describe the same schedule. A generated wrapper has one scheduler; silently
/// selecting one member's period would make the other member's authored policy
/// false, so mixed periods are a terminal projection error.
fn aggregate_communication_periods<I>(periods: I) -> Result<f64, Vec<DomainProjectionError>>
where
    I: IntoIterator<Item = Result<(String, f64), DomainProjectionError>>,
{
    let mut errors = Vec::new();
    let mut selected: Option<(String, u64, f64)> = None;
    for period in periods {
        let (path, period) = match period {
            Ok(period) => period,
            Err(error) => {
                errors.push(error);
                continue;
            }
        };
        let ticks = (period / lunco_core::SECS_PER_TICK).round() as u64;
        if let Some((selected_path, selected_ticks, selected_period)) = &selected {
            if *selected_ticks != ticks {
                errors.push(DomainProjectionError {
                    path: format!("{path}.{COMMUNICATION_PERIOD_ATTR}"),
                    message: format!(
                        "communication period {period:.9}s conflicts with {selected_period:.9}s authored at {selected_path}.{COMMUNICATION_PERIOD_ATTR}; one generated Modelica solver cannot honor mixed member schedules"
                    ),
                });
            }
        } else {
            selected = Some((path, ticks, period));
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    Ok(selected
        .map(|(_, _, period)| period)
        .unwrap_or(lunco_modelica::DEFAULT_COMMUNICATION_PERIOD_SECS))
}

fn network_communication_period(
    view: &dyn ComposedReader,
    components: &[DomainComponent],
) -> Result<f64, Vec<DomainProjectionError>> {
    aggregate_communication_periods(components.iter().map(|component| {
        let path = SdfPath::new(&component.path).map_err(|error| DomainProjectionError {
            path: component.path.clone(),
            message: format!("invalid Modelica component path: {error}"),
        })?;
        let authored = view
            .attr_names(&path)
            .iter()
            .any(|name| name == COMMUNICATION_PERIOD_ATTR);
        let period = lunco_modelica::resolve_communication_period_secs(
            authored,
            view.real(&path, COMMUNICATION_PERIOD_ATTR),
        )
        .map_err(|reason| DomainProjectionError {
            path: format!("{}.{COMMUNICATION_PERIOD_ATTR}", component.path),
            message: format!("invalid Modelica communication period: {reason}"),
        })?;
        Ok((component.path.clone(), period))
    }))
}

/// Read one composed network root as a network, or say why it cannot be one.
///
/// `Ok(None)` = not a network root (or nothing solvable is left in it);
/// `Err` = authored opinions that would produce a model the compiler could only
/// reject, reported against the property that carries them.
///
/// Public because this is the layer worth testing against REAL composed USD:
/// every unit test below builds a `DomainNetwork` by hand, and the composition
/// arcs are exactly where this has broken in the field.
pub fn read_network(
    view: &dyn ComposedReader,
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
        let topology_role = if view.has_api_schema(&path, "LunCoModelicaTopologyAPI") {
            match view.text(&path, "lunco:modelica:topologyRole").as_deref() {
                Some(role @ ("source" | "storage" | "load" | "neutral")) => role.to_string(),
                Some(role) => {
                    extraction_errors.push(DomainProjectionError {
                        path: format!("{path}.lunco:modelica:topologyRole"),
                        message: format!(
                            "unsupported Modelica topology role `{role}`; expected source, storage, load, or neutral"
                        ),
                    });
                    "neutral".to_string()
                }
                None => {
                    extraction_errors.push(DomainProjectionError {
                        path: format!("{path}.lunco:modelica:topologyRole"),
                        message: "LunCoModelicaTopologyAPI is applied but its topology role is unauthored".into(),
                    });
                    "neutral".to_string()
                }
            }
        } else {
            "neutral".to_string()
        };
        for attr in attrs {
            if let Some(name) = attr.strip_prefix("connectors:") {
                let name = name.strip_suffix(".connect").unwrap_or(name);
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
                let name = name.strip_suffix(".connect").unwrap_or(name);
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
                let name = name.strip_suffix(".connect").unwrap_or(name);
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
            topology_role,
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
            communication_period_secs: lunco_modelica::DEFAULT_COMMUNICATION_PERIOD_SECS,
            pending_sources: true,
        }));
    }
    // A component with an acausal port but no authored edge is a legitimate
    // installed-but-unwired part may be valid in the authored assembly but has
    // no well-posed acausal network by itself. Omit it from this generated
    // island rather than rejecting unrelated connected equipment.
    // Causal-only components remain: they may be complete models without an
    // acausal connector at all.
    let omitted = retain_connected_acausal_components(&mut components);
    if components.is_empty() {
        return Ok(None);
    }
    let communication_period_secs = network_communication_period(view, &components)?;
    let attrs = view.attr_names(root);
    let authored_inputs: BTreeSet<_> = attrs
        .iter()
        .filter_map(|attr| {
            attr.strip_prefix("inputs:")
                .map(|name| name.strip_suffix(".connect").unwrap_or(name).to_string())
        })
        .collect();
    let mut authored_input_sources = BTreeMap::new();
    for attr in &attrs {
        let Some(name) = attr.strip_prefix("inputs:") else {
            continue;
        };
        let name = name.strip_suffix(".connect").unwrap_or(name);
        let targets = view.connections(root, attr);
        if targets.len() > 1 {
            extraction_errors.push(DomainProjectionError {
                path: format!("{root}.{attr}"),
                message: "a scalar network input must have at most one connection source".into(),
            });
        } else if let Some(target) = targets.first() {
            authored_input_sources.insert(name.to_string(), target.to_string());
        }
    }
    let internal_inputs: BTreeMap<String, String> = authored_inputs
        .iter()
        .filter_map(|name| {
            lunco_usd_bevy::program::internal_network_input_source(view, root, name)
                .map(|source| (name.clone(), source))
        })
        .collect();
    let internal_outputs: BTreeMap<String, String> = attrs
        .iter()
        .filter_map(|attr| {
            let name = attr
                .strip_prefix("outputs:")
                .map(|name| name.strip_suffix(".connect").unwrap_or(name))?;
            lunco_usd_bevy::program::network_member_output_source(view, root, name)
                .map(|source| (name.to_string(), source))
        })
        .collect();
    // Collapse a root input whose source is a member output into a direct
    // causal member edge. This keeps an authored drive-law forward internal to
    // the generated network instead of declaring the same identifier as both
    // a Modelica input and output.
    let root_input_prefix = format!("{root_string}.inputs:");
    for component in &mut components {
        for target in component.inputs.values_mut() {
            if let Some(name) = target.strip_prefix(&root_input_prefix) {
                if let Some(source) = internal_inputs.get(name) {
                    *target = source.clone();
                }
            } else if let Some(name) = target.strip_prefix(&format!("{root_string}.outputs:")) {
                if let Some(source) = internal_outputs.get(name) {
                    *target = source.clone();
                }
            }
        }
    }
    let inputs: BTreeSet<_> = authored_inputs
        .into_iter()
        .filter(|name| !internal_inputs.contains_key(name))
        .collect();
    let input_sources: BTreeMap<_, _> = authored_input_sources
        .into_iter()
        .filter(|(name, _)| !internal_inputs.contains_key(name))
        .collect();
    let mut outputs = BTreeMap::new();
    for attr in &attrs {
        let Some(name) = attr.strip_prefix("outputs:") else {
            continue;
        };
        // A USD prim may expose the same spelling in both namespaces. The
        // generated Modelica root cannot declare one identifier as input and
        // output, so a colliding output stays on the runtime actuator surface;
        // its promoted member alias is wired to that surface by the generic
        // USD connection pass.
        let name = name.strip_suffix(".connect").unwrap_or(name);
        if inputs.contains(name) {
            continue;
        }
        // A network root can also expose ordinary vehicle outputs such as
        // `drive_left`. Only an output sourced from a member in this root's
        // component collection is part of the generated Modelica interface.
        // The other outputs remain available to physics and control wiring.
        if !lunco_usd_bevy::program::is_network_boundary_output(view, root, attr) {
            continue;
        }
        let targets = view.connections(root, attr);
        if targets.len() != 1 {
            extraction_errors.push(DomainProjectionError {
                path: format!("{root}.{attr}"),
                message: "a network output must have exactly one component source".into(),
            });
            continue;
        }
        // A boundary output whose source was OMITTED above (an installed but
        // unwired part) drops with it. Rejecting the whole network would make
        // one incomplete reference arc hide every otherwise valid member. The
        // two policies have to agree — omitting a part means omitting what it
        // published.
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
        communication_period_secs,
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
        let generated = match instance_identifier(&network.root, &component.path) {
            Ok(generated) => generated,
            Err(message) => {
                errors.push(DomainProjectionError {
                    path: component.path.clone(),
                    message,
                });
                continue;
            }
        };
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

/// Drop only unconnected acausal component facets before emitting a DAE.
///
/// USD connections are directional authoring opinions whereas Modelica
/// `connect()` is symmetric, so retaining only the source side would be wrong:
/// every endpoint of an authored edge belongs to the generated island. A part
/// with an acausal connector and no edge at all has no solvable network context;
/// it remains a perfectly valid physical component, just not a member of this
/// runtime Modelica model.
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

/// Stable, readable Modelica instance name for a composed USD member.
///
/// The full prim path remains the authoritative identity in members, signal
/// provenance, and USD. It is a poor display name, though: emitting the
/// assembly name made a six-member diagram read SolarRover__Motor__FL.
/// Prefer the member leaf (Motor_FL) and add its immediate parent only for
/// nested members (YawHead__SolarPanel). The network root's parent is used
/// as the common assembly scope because composed members may sit beside the
/// component collection rather than below it. validate_network still
/// rejects a same-name collision; it must be fixed in USD rather than hidden
/// by a numeric fallback.
fn instance_identifier(root: &str, path: &str) -> Result<String, String> {
    let root_scope = root
        .trim_matches('/')
        .rsplit_once('/')
        .map(|(parent, _)| format!("/{}", parent))
        .unwrap_or_else(|| root.trim_matches('/').to_string());
    let relative = path
        .strip_prefix(root)
        .or_else(|| path.strip_prefix(root_scope.as_str()))
        .unwrap_or(path)
        .trim_matches('/');
    let mut segments = relative.split('/').filter(|segment| !segment.is_empty());
    let Some(last) = segments.next_back() else {
        return Err(format!(
            "generated Modelica member path `{path}` has no name relative to network root `{root}`"
        ));
    };
    let Some(parent) = segments.next_back() else {
        return Ok(modelica_identifier(last));
    };
    Ok(format!(
        "{}__{}",
        modelica_identifier(parent),
        modelica_identifier(last)
    ))
}

/// Whether a generated member output already has an authored operator-facing
/// telemetry declaration.  Such a declaration owns the public channel name;
/// the generated wrapper alias remains available as implementation state but
/// must not be classified as a second public channel for the same value.
fn has_authored_telemetry_for_output(
    view: &dyn ComposedReader,
    member: &str,
    output: &str,
) -> bool {
    let Ok(path) = SdfPath::new(member) else {
        return false;
    };
    if view.boolean(&path, "lunco:telemetry") == Some(true)
        && view
            .text(&path, "lunco:telemetry:port")
            .is_some_and(|port| port == output)
    {
        return true;
    }

    // One prim can carry one LunCoTelemetryAPI declaration. Additional
    // operator channels are authored as declaration prims that target the
    // measured member through the same API's relationship, so they still
    // suppress a duplicate generated public alias.
    view.prim_paths().into_iter().any(|candidate| {
        view.boolean(&candidate, "lunco:telemetry") == Some(true)
            && view
                .text(&candidate, "lunco:telemetry:port")
                .is_some_and(|port| port == output)
            && view
                .rel_target(&candidate, "lunco:telemetry:target")
                .is_some_and(|target| target == member)
    })
}

/// Build the runtime address map that reconnects one generated solver to the
/// composed USD ownership tree.
///
/// A generated network intentionally has one `ModelicaModel` entity.  Its
/// solver variables therefore cannot be grouped by ECS parentage: that parent
/// is the network root, not the battery, motor, or panel that owns a value.
/// The composed network already contains the authoritative mapping in two
/// forms: boundary output connections and generated unit/member instance
/// names.  Materialize those facts once at projection time so every telemetry
/// consumer sees the same USD structure without parsing generated names in a
/// UI or retaining a second solver.
fn generated_signal_layout(
    view: &dyn ComposedReader,
    root_path: &SdfPath,
    root: &str,
    outputs: &BTreeSet<String>,
    members: &[(String, String, String)],
    member_output_aliases: &[(String, String, String)],
    units: &[SynthesisUnit],
    classes: &MemberClasses,
) -> Result<ModelicaSignalLayout, String> {
    let mut layout = ModelicaSignalLayout {
        root_path: root.to_string(),
        ..default()
    };
    let mut public_member_outputs = BTreeMap::new();

    // Public network outputs retain the authored connection's physical owner.
    for output in outputs {
        let attr = format!("outputs:{output}");
        let connections = view.connections(root_path, &attr);
        let Some(target) = connections.first() else {
            continue;
        };
        let Some((target_prim, target_output)) = target.rsplit_once(".outputs:") else {
            continue;
        };
        public_member_outputs.insert(
            format!("{target_prim}.outputs:{target_output}"),
            output.clone(),
        );
        layout
            .exact_paths
            .insert(output.clone(), target_prim.to_string());
        // If the member already authored the operator-facing channel, that
        // declaration owns the public identity and this wrapper boundary is
        // only its generated implementation address.  Keep it retained, but
        // classify it internal so the same physical value is not presented
        // twice in the canonical catalog.
        if !has_authored_telemetry_for_output(view, target_prim, target_output) {
            layout.public_exact_paths.insert(output.clone());
        }
        if let Some((_, asset, class)) = members.iter().find(|(path, _, _)| path == target_prim) {
            layout.exact_provenance.insert(
                output.clone(),
                ModelicaSignalProvenance {
                    source_asset: Some(asset.clone()),
                    model_class: Some(class.clone()),
                    model_variable: Some(target_output.to_string()),
                    canonical_name: Some(output.clone()),
                },
            );
            if let Some(metadata) = classes.output_metadata(asset, target_output) {
                layout.metadata.insert(output.clone(), metadata.clone());
            }
        }
    }

    // Boundary inputs are also solver variables.  Their authored source is
    // the ownership fact for the command/environment value, so use it instead
    // of leaving every generated input under the implementation scope.
    for attr in view.attr_names(root_path) {
        let Some(name) = attr
            .strip_prefix("inputs:")
            .map(|name| name.strip_suffix(".connect").unwrap_or(name))
        else {
            continue;
        };
        let connections = view.connections(root_path, &attr);
        let Some(target) = connections.first() else {
            continue;
        };
        let Some((target_prim, _)) = target.rsplit_once(".outputs:") else {
            continue;
        };
        layout
            .exact_paths
            .entry(name.to_string())
            .or_insert_with(|| target_prim.to_string());
    }

    for (member, output, alias) in member_output_aliases {
        layout.exact_paths.insert(alias.clone(), member.clone());
        if let Some((_, asset, class)) = members.iter().find(|(path, _, _)| path == member) {
            let canonical_name = public_member_outputs
                .get(&format!("{member}.outputs:{output}"))
                .cloned()
                .unwrap_or_else(|| alias.clone());
            layout.exact_provenance.insert(
                alias.clone(),
                ModelicaSignalProvenance {
                    source_asset: Some(asset.clone()),
                    model_class: Some(class.clone()),
                    model_variable: Some(output.clone()),
                    canonical_name: Some(canonical_name),
                },
            );
            if let Some(metadata) = classes.output_metadata(asset, output) {
                layout.metadata.insert(alias.clone(), metadata.clone());
            }
        }
        // A generated member alias is the canonical public projection for an
        // authored component output unless the network already exposes that
        // same USD port under a public boundary name. This is topology-derived
        // and therefore applies equally to motors, batteries, panels, and
        // future Modelica facets without a component-name classifier.
        if !public_member_outputs.contains_key(&format!("{member}.outputs:{output}"))
            && !has_authored_telemetry_for_output(view, member, output)
        {
            layout.public_exact_paths.insert(alias.clone());
        }
    }

    // The synthesizer emits every component under its policy-selected unit
    // instance. A longest-prefix lookup assigns all public and internal
    // variables of that member—including variables introduced by a later
    // Modelica revision—to the authored member without an output annotation.
    for unit in units {
        let unit_prefix = unit.instance.clone();
        for (output, owner) in layout.exact_paths.clone() {
            let qualified = format!("{unit_prefix}.{output}");
            layout.exact_paths.insert(qualified.clone(), owner);
            // The unit instance is a generated implementation boundary, not a
            // new physical value. Preserve the public classification of the
            // authored boundary/member alias when copying it into the unit;
            // otherwise the runtime retains the value but the operator tree
            // hides the only representation of an unpromoted member output.
            if layout.public_exact_paths.contains(&output) {
                layout.public_exact_paths.insert(qualified);
            }
        }
        for (variable, identity) in layout.exact_provenance.clone() {
            layout
                .exact_provenance
                .insert(format!("{unit_prefix}.{variable}"), identity);
        }
        for (member, _, alias) in member_output_aliases
            .iter()
            .filter(|(member, _, _)| unit.component_paths.iter().any(|path| path == member))
        {
            layout
                .exact_paths
                .insert(format!("{unit_prefix}.{alias}"), member.clone());
        }
        for member in &unit.component_paths {
            let member_prefix = instance_identifier(root, member)?;
            let prefix = format!("{unit_prefix}.{member_prefix}.");
            layout.prefixes.push((prefix.clone(), member.clone()));
            if let Some((_, asset, class)) = members.iter().find(|(path, _, _)| path == member) {
                if let Some(metadata) = classes.variable_metadata(asset) {
                    for (variable, metadata) in metadata {
                        layout
                            .metadata
                            .entry(format!("{prefix}{variable}"))
                            .or_insert_with(|| metadata.clone());
                    }
                }
                layout.provenance_prefixes.push((
                    prefix,
                    ModelicaSignalProvenance {
                        source_asset: Some(asset.clone()),
                        model_class: Some(class.clone()),
                        ..default()
                    },
                ));
            }
        }
    }
    // Deterministic ordering keeps the component inspectable and makes the
    // metadata stable for tests and API snapshots. Resolution itself chooses
    // the longest prefix, so nested instance names remain unambiguous.
    layout
        .prefixes
        .sort_by(|(left, _), (right, _)| right.len().cmp(&left.len()).then(left.cmp(right)));
    layout
        .provenance_prefixes
        .sort_by(|(left, _), (right, _)| right.len().cmp(&left.len()).then(left.cmp(right)));

    // A generated member alias may be emitted in more than one synthesized
    // unit only if the authored topology is invalid. The projection validator
    // owns that error; this map remains a direct data projection and never
    // invents another owner.
    debug_assert!(members.iter().all(|(member, _, _)| {
        units
            .iter()
            .any(|unit| unit.component_paths.contains(member))
    }));
    Ok(layout)
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
#[derive(Resource, Clone, Default)]
pub struct MemberClasses {
    known: HashMap<String, MemberClass>,
    outputs: HashMap<String, BTreeSet<String>>,
    metadata: HashMap<String, HashMap<String, ModelicaVariableMetadata>>,
    /// Resident handles let source modification events invalidate the exact
    /// declaration they changed without rescanning every pending source.
    handles: HashMap<String, Handle<lunco_modelica::source_asset::ModelicaSource>>,
    pending: HashMap<String, Handle<lunco_modelica::source_asset::ModelicaSource>>,
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

    /// Units and descriptions for the output declarations that the generated
    /// wrapper promotes from this member.  The metadata is read from the same
    /// Modelica source as the class and output names; it is not reconstructed
    /// from generated solver identifiers or component names.
    pub fn output_metadata(&self, asset: &str, output: &str) -> Option<&ModelicaVariableMetadata> {
        self.metadata
            .get(asset)
            .and_then(|metadata| metadata.get(output))
    }

    /// Units and descriptions for every declared variable in a member source.
    /// Generated solver members use this map for internal inspection rows as
    /// well as promoted outputs, so the browser and API do not lose authored
    /// metadata at the generated-document boundary.
    pub fn variable_metadata(
        &self,
        asset: &str,
    ) -> Option<&HashMap<String, ModelicaVariableMetadata>> {
        self.metadata.get(asset)
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
/// re-runs. Prim spawn and live edits are the projector's other triggers; the
/// asset event is the resolution trigger for a pending source.
#[derive(Resource, Default)]
pub struct ProjectionDirty(pub bool);

fn projection_is_due_from_flags(
    has_added_prim: bool,
    has_added_identity: bool,
    wiring_dirty: bool,
    projection_dirty: bool,
) -> bool {
    has_added_prim || has_added_identity || wiring_dirty || projection_dirty
}

/// Run condition for the generated-domain projector.
///
/// Projection is an authoring/lifecycle transaction, not a frame service. Keep
/// the trigger set beside [`project_domain_islands`] so the scheduler can avoid
/// constructing its stage, identity, and synthesizer queries on stable frames.
pub(crate) fn domain_projection_due(
    added: Query<(), Added<UsdPrimPath>>,
    identity_added: Query<(), Added<lunco_core::GlobalEntityId>>,
    dirty: Res<WiringDirty>,
    projection_dirty: Res<ProjectionDirty>,
) -> bool {
    projection_is_due_from_flags(
        !added.is_empty(),
        !identity_added.is_empty(),
        dirty.0,
        projection_dirty.0,
    )
}

/// Resolve every member source's DECLARED class before synthesis.
///
/// Scans the stage for component collections, loads each member's
/// `info:sourceAsset` once, and reads `within` + the class the file declares.
/// Until a member has a verdict its network does not project at all
/// ([`SynthOutcome::Pending`]) — synthesizing before the source settles would
/// produce a generated model with an unknown member class and an unattributed
/// compiler failure.
///
/// A source that fails to load or does not expose a class settles as
/// [`MemberClass::Invalid`]. The projection reports that terminal source error
/// and does not compile an incomplete model. Completion and failure are driven
/// by the Modelica asset events; there is no time-based give-up path.
pub fn resolve_member_classes(
    prims: Query<&UsdPrimPath>,
    added: Query<(), Added<UsdPrimPath>>,
    mut classes: ResMut<MemberClasses>,
    mut projection_dirty: ResMut<ProjectionDirty>,
    dirty: Res<WiringDirty>,
    stages: Res<Assets<UsdStageAsset>>,
    canonical: NonSend<CanonicalStages>,
    asset_server: Res<AssetServer>,
    sources: Res<Assets<lunco_modelica::source_asset::ModelicaSource>>,
    mut source_events: MessageReader<AssetEvent<lunco_modelica::source_asset::ModelicaSource>>,
    mut source_failures: MessageReader<
        bevy::asset::AssetLoadFailedEvent<lunco_modelica::source_asset::ModelicaSource>,
    >,
) {
    let mut loaded = HashSet::new();
    let mut modified = HashSet::new();
    for event in source_events.read() {
        match event {
            AssetEvent::Added { id } | AssetEvent::LoadedWithDependencies { id } => {
                loaded.insert(*id);
            }
            AssetEvent::Modified { id } => {
                modified.insert(*id);
            }
            _ => {}
        }
    }
    let failed: HashMap<AssetId<lunco_modelica::source_asset::ModelicaSource>, String> =
        source_failures
            .read()
            .map(|event| (event.id, event.error.to_string()))
            .collect();
    let discover = !added.is_empty() || dirty.0;
    if !discover && loaded.is_empty() && modified.is_empty() && failed.is_empty() {
        return;
    }

    // Discovery runs on the same triggers as the projector — plus never at all
    // once every member is known, which is the steady state.
    let mut discovered = HashSet::new();
    let modified_assets: Vec<_> = classes
        .handles
        .iter()
        .filter(|(_, handle)| modified.contains(&handle.id()))
        .map(|(asset, handle)| (asset.clone(), handle.clone()))
        .collect();
    for (asset, handle) in modified_assets {
        classes.known.remove(&asset);
        classes.outputs.remove(&asset);
        classes.metadata.remove(&asset);
        classes.pending.insert(asset, handle);
    }
    if discover {
        for prim in &prims {
            let id = prim.stage_handle.id();
            let Some(stage_asset) = stages.get(&prim.stage_handle) else {
                continue;
            };
            let (reader, _generation) = canonical.reader_for(id, stage_asset);
            let view: &dyn ComposedReader = &reader;
            let Ok(root) = SdfPath::new(&prim.path) else {
                continue;
            };
            if !is_domain_network_root(view, &root) {
                continue;
            }
            let Ok(members) = view.collection_members(&root, "components") else {
                continue;
            };
            for member in members {
                if !view.has_api_schema(&member, "LunCoProgramAPI") {
                    continue;
                }
                let source_ref = match modelica_source_ref(view, &member) {
                    Ok(source_ref) => source_ref,
                    Err(issue) => {
                        warn!(
                            "[domain-projection] member {} has unresolved Modelica source at {}: {}",
                            member, issue.property, issue.message
                        );
                        continue;
                    }
                };
                let asset = source_ref.asset;
                if classes.known.contains_key(&asset) || classes.pending.contains_key(&asset) {
                    continue;
                }
                let handle: Handle<lunco_modelica::source_asset::ModelicaSource> =
                    asset_server.load(asset.clone());
                discovered.insert(handle.id());
                classes.handles.insert(asset.clone(), handle.clone());
                classes.pending.insert(asset, handle);
            }
        }
    }

    if classes.pending.is_empty() {
        return;
    }
    let settled: Vec<(
        String,
        Result<
            (
                String,
                BTreeSet<String>,
                HashMap<String, ModelicaVariableMetadata>,
            ),
            String,
        >,
    )> = classes
        .pending
        .iter()
        .filter_map(|(asset, handle)| {
            let id = handle.id();
            if !discovered.contains(&id) && !loaded.contains(&id) && !failed.contains_key(&id) {
                return None;
            }
            if let Some(source) = sources.get(handle) {
                let interface = parse_model_interface(&source.text, "member-class.mo");
                let Some(declared) = interface.model_name else {
                    return Some((
                        asset.clone(),
                        Err("the Modelica source did not expose a declared class".into()),
                    ));
                };
                let class = match interface.within {
                    Some(within) => format!("{within}.{declared}"),
                    None => declared,
                };
                return Some((
                    asset.clone(),
                    Ok((class, interface.outputs, interface.variable_metadata)),
                ));
            }
            failed.get(&id).map(|error| {
                (
                    asset.clone(),
                    Err(format!("failed to load Modelica source asset: {error}")),
                )
            })
        })
        .collect();
    for (asset, result) in settled {
        classes.pending.remove(&asset);
        match result {
            Ok((class, outputs, metadata)) => {
                classes.outputs.insert(asset.clone(), outputs);
                classes.metadata.insert(asset.clone(), metadata);
                classes.known.insert(asset, MemberClass::Declared(class));
            }
            Err(error) => {
                warn!("[domain-projection] {asset}: {error}");
                classes.known.insert(asset, MemberClass::Invalid(error));
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

    #[test]
    fn domain_projection_schedule_requires_an_authoring_trigger() {
        assert!(!projection_is_due_from_flags(false, false, false, false));
        assert!(projection_is_due_from_flags(true, false, false, false));
        assert!(projection_is_due_from_flags(false, true, false, false));
        assert!(projection_is_due_from_flags(false, false, true, false));
        assert!(projection_is_due_from_flags(false, false, false, true));
    }

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
            topology_role: "neutral".into(),
        }
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
            communication_period_secs: lunco_modelica::DEFAULT_COMMUNICATION_PERIOD_SECS,
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
    }

    #[test]
    fn member_layout_coordinates_are_scoped_to_their_owning_unit() {
        let network = DomainNetwork {
            root: "/Rig".into(),
            components: vec![
                component("/Rig/Source_A", Some("/Rig/Load_A")),
                component("/Rig/Load_A", Some("/Rig/Source_A")),
                component("/Rig/Source_B", Some("/Rig/Load_B")),
                component("/Rig/Load_B", Some("/Rig/Source_B")),
            ],
            inputs: BTreeSet::new(),
            input_sources: BTreeMap::new(),
            outputs: BTreeMap::new(),
            communication_period_secs: lunco_modelica::DEFAULT_COMMUNICATION_PERIOD_SECS,
            pending_sources: false,
        };
        let units = vec![
            SynthesisUnit {
                name: "NetworkUnit_1".into(),
                instance: "network_unit_1".into(),
                component_paths: vec!["/Rig/Source_A".into(), "/Rig/Load_A".into()],
                ..Default::default()
            },
            SynthesisUnit {
                name: "NetworkUnit_2".into(),
                instance: "network_unit_2".into(),
                component_paths: vec!["/Rig/Source_B".into(), "/Rig/Load_B".into()],
                ..Default::default()
            },
        ];
        let layout = lunco_hooks::HookValue::map([
            (
                "units",
                lunco_hooks::HookValue::Array(vec![
                    lunco_hooks::HookValue::map([
                        ("name", lunco_hooks::HookValue::str("NetworkUnit_1")),
                        ("x", lunco_hooks::HookValue::Int(-200)),
                        ("y", lunco_hooks::HookValue::Int(0)),
                    ]),
                    lunco_hooks::HookValue::map([
                        ("name", lunco_hooks::HookValue::str("NetworkUnit_2")),
                        ("x", lunco_hooks::HookValue::Int(200)),
                        ("y", lunco_hooks::HookValue::Int(0)),
                    ]),
                ]),
            ),
            (
                "members",
                lunco_hooks::HookValue::Array(vec![
                    lunco_hooks::HookValue::map([
                        ("path", lunco_hooks::HookValue::str("/Rig/Source_A")),
                        ("x", lunco_hooks::HookValue::Int(-170)),
                        ("y", lunco_hooks::HookValue::Int(0)),
                    ]),
                    lunco_hooks::HookValue::map([
                        ("path", lunco_hooks::HookValue::str("/Rig/Load_A")),
                        ("x", lunco_hooks::HookValue::Int(0)),
                        ("y", lunco_hooks::HookValue::Int(80)),
                    ]),
                    lunco_hooks::HookValue::map([
                        ("path", lunco_hooks::HookValue::str("/Rig/Source_B")),
                        ("x", lunco_hooks::HookValue::Int(-170)),
                        ("y", lunco_hooks::HookValue::Int(0)),
                    ]),
                    lunco_hooks::HookValue::map([
                        ("path", lunco_hooks::HookValue::str("/Rig/Load_B")),
                        ("x", lunco_hooks::HookValue::Int(0)),
                        ("y", lunco_hooks::HookValue::Int(80)),
                    ]),
                ]),
            ),
        ]);

        let parsed = parse_policy_layout(Some(&layout), &network, &units, "/Rig", "local-layout")
            .expect("independent unit diagrams may reuse local coordinates");
        assert_eq!(parsed.member_positions["/Rig/Source_A"], (-170, 0));
        assert_eq!(parsed.member_positions["/Rig/Source_B"], (-170, 0));
    }

    #[test]
    fn generated_member_instance_names_are_readable_without_a_collision_fallback() {
        assert_eq!(
            instance_identifier(
                "/SolarRoverTest/SolarRover",
                "/SolarRoverTest/SolarRover/Motor_FL"
            )
            .unwrap(),
            "Motor_FL"
        );
        assert_eq!(
            instance_identifier("/Rig", "/Rig/Battery").unwrap(),
            "Battery"
        );
        assert_ne!(
            instance_identifier("/Rig", "/Rig/Motor-A").unwrap(),
            instance_identifier("/Rig", "/Rig/Motor_A").unwrap()
        );
    }

    #[test]
    fn read_network_admits_the_composed_modelica_drive_law() {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/scenes/tests/modelica_drive_law.usda");
        let composed =
            lunco_usd_bevy::compose_file_to_stage(&path).expect("compose drive-law scene");
        let stage = lunco_usd_bevy::CanonicalStage::from_stage(
            composed,
            path.to_string_lossy().to_string(),
        );
        let view = stage.view();
        let root = SdfPath::new("/ModelicaDriveLaw/RoverModelica").unwrap();
        assert_eq!(
            lunco_usd_bevy::program::internal_network_input_source(&view, &root, "drive_left"),
            Some("/ModelicaDriveLaw/RoverModelica/Drivetrain.outputs:drive_left".into())
        );
        assert_eq!(
            lunco_usd_bevy::program::network_member_output_source(&view, &root, "drive_left"),
            Some("/ModelicaDriveLaw/RoverModelica/Drivetrain.outputs:drive_left".into())
        );
        let mut classes = MemberClasses::default();
        for member in view.collection_members(&root, "components").unwrap() {
            if !lunco_usd_bevy::UsdRead::has_api_schema(&view, &member, "LunCoProgramAPI") {
                continue;
            }
            let asset = lunco_usd_bevy::UsdRead::asset(&view, &member, "info:sourceAsset").unwrap();
            let source = std::fs::read_to_string(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../assets")
                    .join(lunco_assets::engine_asset_rel(&asset)),
            )
            .unwrap();
            let interface = parse_model_interface(&source, "drive-law-member.mo");
            let class = match interface.within {
                Some(within) => format!("{within}.{}", interface.model_name.unwrap()),
                None => interface.model_name.unwrap(),
            };
            classes.declare(asset, class);
        }
        let network = read_network(&view, &root, &classes)
            .expect("the composed drive-law network must be structurally valid")
            .expect("drive-law scene must expose a Modelica network");
        lunco_hooks_rhai::register_rhai_hook(
            "synth.acausal-network",
            "synthesize",
            lunco_assets::scripting::policy("synth_acausal_network").unwrap(),
            true,
        )
        .unwrap();
        let synthesizer = HookSynthesizer {
            name: DEFAULT_DOMAIN_SYNTHESIZER.into(),
            hook_id: "synth.acausal-network".into(),
        };
        assert!(matches!(
            synthesizer
                .synthesize(
                    &view,
                    &root,
                    "ModelicaDriveLaw_x2f_RoverModelica_System",
                    &SynthContext { classes: &classes },
                )
                .expect("synthesize composed drive-law network"),
            SynthOutcome::Ready(_)
        ));
        assert!(network
            .components
            .iter()
            .any(|component| component.path.ends_with("/Drivetrain")));
        assert!(
            !network.inputs.contains("drive_left") && !network.inputs.contains("drive_right"),
            "a drive-law-produced actuator must not remain an external network input"
        );
        assert!(
            network.input_sources.get("drive_left").is_none()
                && network.input_sources.get("drive_right").is_none(),
            "the internal actuator path must not be rebound as a runtime input"
        );
        assert!(
            network.components.iter().any(|component| component
                .inputs
                .values()
                .any(|target| target.ends_with("/Drivetrain.outputs:drive_left"))),
            "the authored drive-left path must become a direct causal member edge"
        );
        assert!(
            !network.outputs.contains_key("drive_left"),
            "a USD input/output name collision must not become duplicate Modelica declarations"
        );
    }

    #[test]
    fn member_path_without_a_name_is_reported_instead_of_panicking() {
        let error = instance_identifier("/Rig", "/Rig")
            .expect_err("the network root is not a member instance");
        assert!(error.contains("has no name"), "{error}");
    }

    #[test]
    fn generated_signal_layout_keeps_member_class_and_canonical_output_identity() {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/electrical_network.usda");
        let composed = lunco_usd_bevy::compose_file_to_stage(&path).expect("compose fixture");
        let stage = lunco_usd_bevy::CanonicalStage::from_stage(
            composed,
            path.to_string_lossy().to_string(),
        );
        let view = stage.view();
        let root_path = SdfPath::new("/Rig").unwrap();
        let mut classes = MemberClasses::default();
        classes.declare(
            "lunco://models/LunCo/Electrical/Battery.mo",
            "LunCo.Electrical.Battery",
        );
        classes.metadata.insert(
            "lunco://models/LunCo/Electrical/Battery.mo".into(),
            HashMap::from([(
                "terminal_voltage_v".into(),
                ModelicaVariableMetadata {
                    description: Some("Battery terminal voltage on the electrical bus".into()),
                    unit: Some("V".into()),
                },
            )]),
        );
        classes.declare(
            "lunco://models/LunCo/Electrical/DCMotor.mo",
            "LunCo.Electrical.DCMotor",
        );
        classes.declare(
            "lunco://models/LunCo/Electrical/SolarPanel.mo",
            "LunCo.Electrical.SolarPanel",
        );
        lunco_hooks_rhai::register_rhai_hook(
            "synth.acausal-network",
            "synthesize",
            lunco_assets::scripting::policy("synth_acausal_network")
                .expect("shipped synthesis policy"),
            true,
        )
        .expect("shipped synthesis policy compiles");
        let synthesizer = SynthesizerRegistry::default()
            .get(DEFAULT_SYNTHESIZER)
            .expect("default synthesizer is policy-backed")
            .clone();
        let SynthOutcome::Ready(plan) = synthesizer
            .synthesize(
                &view,
                &root_path,
                "Rig_System",
                &SynthContext { classes: &classes },
            )
            .expect("fixture synthesis")
        else {
            panic!("fixture must be a ready acausal network");
        };
        let aliases = plan.member_output_aliases.clone();
        let layout = generated_signal_layout(
            &view,
            &root_path,
            "/Rig",
            &plan.outputs,
            &plan.members,
            &aliases,
            &plan.units,
            &classes,
        )
        .expect("validated generated member paths");

        let soc = layout.provenance("soc").expect("boundary output identity");
        assert_eq!(soc.model_class.as_deref(), Some("LunCo.Electrical.Battery"));
        assert_eq!(soc.model_variable.as_deref(), Some("soc_out"));
        assert_eq!(soc.canonical_name.as_deref(), Some("soc"));
        assert_eq!(
            soc.source_asset.as_deref(),
            Some("lunco://models/LunCo/Electrical/Battery.mo")
        );

        let battery_unit = plan
            .units
            .iter()
            .find(|unit| {
                unit.component_paths
                    .iter()
                    .any(|path| path == "/Rig/Battery")
            })
            .expect("battery synthesis unit");
        let solver_name = format!(
            "{}.{}.soc_out",
            battery_unit.instance,
            instance_identifier("/Rig", "/Rig/Battery").unwrap(),
        );
        let internal = layout
            .provenance(&solver_name)
            .expect("member solver identity");
        assert_eq!(
            internal.model_class.as_deref(),
            Some("LunCo.Electrical.Battery")
        );
        assert_eq!(internal.model_variable.as_deref(), Some("soc_out"));

        let internal_terminal = format!(
            "{}.{}.terminal_voltage_v",
            battery_unit.instance,
            instance_identifier("/Rig", "/Rig/Battery").unwrap(),
        );
        let terminal_metadata = layout
            .metadata
            .get(&internal_terminal)
            .expect("member metadata follows the generated solver prefix");
        assert_eq!(terminal_metadata.unit.as_deref(), Some("V"));
        assert_eq!(
            terminal_metadata.description.as_deref(),
            Some("Battery terminal voltage on the electrical bus")
        );

        let motor_alias = aliases
            .iter()
            .find(|(member, output, _)| member == "/Rig/Motor" && output == "electrical_power")
            .map(|(_, _, alias)| alias)
            .expect("the unpromoted motor output is in the generated interface");
        assert_eq!(
            layout.exposure(motor_alias),
            lunco_signal::SignalExposure::Public
        );
        let motor_unit = plan
            .units
            .iter()
            .find(|unit| unit.component_paths.iter().any(|path| path == "/Rig/Motor"))
            .expect("motor synthesis unit");
        let motor_solver_name = format!("{}.{}", motor_unit.instance, motor_alias);
        assert_eq!(
            layout.exposure(&motor_solver_name),
            lunco_signal::SignalExposure::Public,
            "unit-qualified member aliases retain their authored public exposure"
        );
    }

    #[test]
    fn authored_member_telemetry_owns_the_public_output_identity() {
        let stage =
            lunco_usd_bevy::CanonicalStage::from_recipe(&lunco_usd_bevy::StageRecipe::from_source(
                "telemetry-owner.usda",
                r#"#usda 1.0
def Scope "Rig"
{
    def Xform "Battery"
    {
        bool lunco:telemetry = true
        token lunco:telemetry:port = "soc_out"
        token outputs:soc_out
    }
}
"#,
            ))
            .expect("telemetry owner stage");
        let view = stage.view();
        assert!(has_authored_telemetry_for_output(
            &view,
            "/Rig/Battery",
            "soc_out"
        ));
        assert!(!has_authored_telemetry_for_output(
            &view,
            "/Rig/Battery",
            "terminal_voltage_v"
        ));
    }

    #[test]
    fn rejects_external_connector_targets_and_keeps_unit_partition_deterministic() {
        let mut external = component("/Rig/Load/Model", None);
        external
            .connectors
            .insert("p".into(), vec!["/Other/Battery/Model.connectors:p".into()]);
        let network = DomainNetwork {
            root: "/Rig".into(),
            components: vec![component("/Rig/Battery/Model", None), external],
            inputs: BTreeSet::new(),
            input_sources: BTreeMap::new(),
            outputs: BTreeMap::new(),
            communication_period_secs: lunco_modelica::DEFAULT_COMMUNICATION_PERIOD_SECS,
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
        let panel = component("/Rig/SolarPanel", None);
        let mut battery = component("/Rig/Battery", None);
        battery
            .connectors
            .insert("p".into(), vec!["/Rig/Motor.connectors:p".into()]);
        let motor = component("/Rig/Motor", None);
        let mut components = vec![panel, battery, motor];
        let omitted = retain_connected_acausal_components(&mut components);
        assert_eq!(
            components
                .iter()
                .map(|component| component.path.as_str())
                .collect::<Vec<_>>(),
            ["/Rig/Battery", "/Rig/Motor"],
            "only explicitly wired program facets enter a generated acausal island"
        );
        assert!(
            omitted.contains("/Rig/SolarPanel"),
            "what the island omits has to be nameable — a boundary output published \
             through an omitted part drops with it instead of rejecting the network"
        );
    }

    #[test]
    fn generated_model_identity_is_qualified_by_network_path() {
        assert_ne!(
            network_model_name("/Rover", Some(10)),
            network_model_name("/Payload", Some(20))
        );
        assert_eq!(network_model_name("/Rover", Some(42)), "Rover_G42_System");
        assert_ne!(
            network_model_name("/Rover", Some(10)),
            network_model_name("/Rover", Some(20))
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
    fn aggregates_one_schedule_and_rejects_conflicting_member_periods() {
        assert_eq!(
            aggregate_communication_periods(std::iter::empty()).unwrap(),
            lunco_modelica::DEFAULT_COMMUNICATION_PERIOD_SECS
        );
        let six_ticks = 6.0 * lunco_core::SECS_PER_TICK;
        assert_eq!(
            aggregate_communication_periods([
                Ok(("/Rig/A".into(), six_ticks)),
                Ok(("/Rig/B".into(), 0.1)),
            ])
            .unwrap(),
            six_ticks
        );
        let errors = aggregate_communication_periods([
            Ok(("/Rig/A".into(), six_ticks)),
            Ok(("/Rig/B".into(), 12.0 * lunco_core::SECS_PER_TICK)),
        ])
        .unwrap_err();
        assert_eq!(errors.len(), 1);
        assert!(errors[0].message.contains("mixed member schedules"));
    }

    #[test]
    fn rejects_ambiguous_forwarded_boundary_sources() {
        let network = DomainNetwork {
            root: "/Rig".into(),
            components: vec![component("/Rig/Battery", None)],
            inputs: BTreeSet::from(["left".into(), "right".into()]),
            input_sources: BTreeMap::from([
                ("left".into(), "/Controls.outputs:throttle".into()),
                ("right".into(), "/Controls.outputs:throttle".into()),
            ]),
            outputs: BTreeMap::new(),
            communication_period_secs: lunco_modelica::DEFAULT_COMMUNICATION_PERIOD_SECS,
            pending_sources: false,
        };
        assert!(validate_network(&network)
            .iter()
            .any(|error| error.message.contains("boundary identity is ambiguous")));
    }

    #[test]
    fn rejects_modelica_keywords_as_public_members() {
        let mut bad = component("/Rig/Load", None);
        bad.inputs
            .insert("equation".into(), "/Rig.inputs:demand".into());
        let network = DomainNetwork {
            root: "/Rig".into(),
            components: vec![bad],
            inputs: BTreeSet::from(["demand".into()]),
            input_sources: BTreeMap::new(),
            outputs: BTreeMap::new(),
            communication_period_secs: lunco_modelica::DEFAULT_COMMUNICATION_PERIOD_SECS,
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
                    network_root: "/Rig".into(),
                    doc_uri: "generated://Rig.mo".into(),
                    source: "model Rig end Rig;".into(),
                    component_paths: vec!["/Battery".into()],
                    members: Vec::new(),
                    source_roots: Vec::new(),
                    member_output_aliases: Vec::new(),
                    units: Vec::new(),
                    boundary_inputs: Vec::new(),
                    boundary_outputs: Vec::new(),
                    layout: SynthesisLayout::default(),
                    projection_error: None,
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
    fn removing_generated_source_retires_only_its_ephemeral_document() {
        let mut app = App::new();
        app.init_resource::<lunco_modelica::state::ModelicaDocumentRegistry>()
            .init_resource::<lunco_modelica::state::GeneratedModelicaSources>()
            .add_observer(on_remove_generated_source);
        let document = app
            .world_mut()
            .resource_mut::<lunco_modelica::state::ModelicaDocumentRegistry>()
            .allocate_with_origin(
                "model Generated end Generated;".into(),
                lunco_doc::DocumentOrigin::Bundled {
                    filename: "generated/Generated.mo".into(),
                },
            );
        let entity = app
            .world_mut()
            .spawn((
                ModelicaModel {
                    document,
                    ..default()
                },
                GeneratedModelicaSource {
                    network_root: "/Rig".into(),
                    doc_uri: "generated://Generated.mo".into(),
                    source: "model Generated end Generated;".into(),
                    component_paths: Vec::new(),
                    members: Vec::new(),
                    source_roots: Vec::new(),
                    member_output_aliases: Vec::new(),
                    units: Vec::new(),
                    boundary_inputs: Vec::new(),
                    boundary_outputs: Vec::new(),
                    layout: SynthesisLayout::default(),
                    projection_error: None,
                },
            ))
            .id();
        app.world_mut()
            .resource_mut::<lunco_modelica::state::ModelicaDocumentRegistry>()
            .link(entity, document);
        app.world_mut()
            .resource_mut::<lunco_modelica::state::GeneratedModelicaSources>()
            .entries
            .push(lunco_modelica::state::GeneratedModelicaSourceEntry {
                document,
                uri: "generated://Generated.mo".into(),
                network_root: "/Rig".into(),
                model_name: "Generated".into(),
                source: "model Generated end Generated;".into(),
                component_paths: Vec::new(),
                units: Vec::new(),
                members: Vec::new(),
                source_roots: Vec::new(),
                boundary_inputs: Vec::new(),
                boundary_outputs: Vec::new(),
                member_output_aliases: Vec::new(),
                projection_error: None,
            });

        app.world_mut()
            .entity_mut(entity)
            .remove::<GeneratedModelicaSource>();
        app.update();

        assert!(app
            .world()
            .resource::<lunco_modelica::state::ModelicaDocumentRegistry>()
            .host(document)
            .is_none());
        assert!(app
            .world()
            .resource::<lunco_modelica::state::GeneratedModelicaSources>()
            .entries
            .is_empty());
    }

    #[test]
    fn generated_source_publication_ignores_runtime_model_output_changes() {
        #[derive(Resource, Default)]
        struct PublicationCount(usize);

        fn count_publications(mut count: ResMut<PublicationCount>) {
            count.0 += 1;
        }

        let mut app = App::new();
        app.init_resource::<lunco_modelica::state::GeneratedModelicaSources>()
            .init_resource::<PublicationCount>()
            .add_systems(
                Update,
                count_publications.run_if(generated_sources_need_publish),
            );
        let entity = app
            .world_mut()
            .spawn((
                GeneratedModelicaSource {
                    network_root: "/Rig".into(),
                    doc_uri: "generated://Rig.mo".into(),
                    source: "model Rig end Rig;".into(),
                    component_paths: Vec::new(),
                    members: Vec::new(),
                    source_roots: Vec::new(),
                    member_output_aliases: Vec::new(),
                    units: Vec::new(),
                    boundary_inputs: Vec::new(),
                    boundary_outputs: Vec::new(),
                    layout: SynthesisLayout::default(),
                    projection_error: None,
                },
                ModelicaModel::default(),
            ))
            .id();

        app.update();
        assert_eq!(app.world().resource::<PublicationCount>().0, 1);

        app.world_mut()
            .get_mut::<ModelicaModel>(entity)
            .unwrap()
            .current_time = 1.0;
        app.update();
        assert_eq!(
            app.world().resource::<PublicationCount>().0,
            1,
            "solver output changes are not generated-source metadata changes"
        );

        app.world_mut()
            .get_mut::<GeneratedModelicaSource>(entity)
            .unwrap()
            .source
            .push(' ');
        app.update();
        assert_eq!(app.world().resource::<PublicationCount>().0, 2);

        app.world_mut()
            .resource_mut::<lunco_modelica::state::GeneratedModelicaSources>()
            .dirty = true;
        app.update();
        assert_eq!(app.world().resource::<PublicationCount>().0, 3);
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

        let (matrix, step) = actuator_wrench_matrix(&actuators).unwrap();
        assert_eq!(matrix.len(), 3);
        assert!(step > 0.0 && step.is_finite());
        assert_eq!(matrix[0], [0.0, 0.0, 1.0, 1.0, 0.0, 0.0]);
        assert_eq!(matrix[1], [1.0, 0.0, 0.0, 0.0, 1.0, 0.0]);
        assert_eq!(matrix[2], [0.0, 1.0, 0.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn actuator_wrench_rejects_zero_authority_geometry() {
        let actuators = [lunco_cosim::ForceActuator {
            local_position: Vec3::ZERO,
            direction_local: Vec3::ZERO,
            max_force_n: 1.0,
        }];

        let error = actuator_wrench_matrix(&actuators).unwrap_err();
        assert!(error.contains("no finite physical wrench authority"));
    }

    #[test]
    fn actuator_wrench_policy_emits_source_and_visual_schema() {
        lunco_hooks_rhai::register_rhai_hook(
            "synth.actuator-wrench",
            "synthesize",
            lunco_assets::scripting::policy("synth_actuator_wrench")
                .expect("shipped actuator policy"),
            true,
        )
        .expect("actuator policy compiles");
        let facts = lunco_hooks::HookValue::Map(vec![
            (
                "model_name".into(),
                lunco_hooks::HookValue::str("AttitudeActuation"),
            ),
            (
                "root".into(),
                lunco_hooks::HookValue::str("/Lander/Actuation"),
            ),
            (
                "inputs".into(),
                lunco_hooks::HookValue::Array(vec![lunco_hooks::HookValue::str(
                    "desired_torque_z",
                )]),
            ),
            (
                "outputs".into(),
                lunco_hooks::HookValue::Array(vec![lunco_hooks::HookValue::str("valve")]),
            ),
            (
                "actuator_paths".into(),
                lunco_hooks::HookValue::Array(vec![lunco_hooks::HookValue::str(
                    "/Lander/Thruster",
                )]),
            ),
            (
                "wrench_matrix".into(),
                lunco_hooks::HookValue::Array(
                    (0..6)
                        .map(|row| {
                            lunco_hooks::HookValue::Array(vec![lunco_hooks::HookValue::Float(
                                if row == 5 { 1.0 } else { 0.0 },
                            )])
                        })
                        .collect(),
                ),
            ),
            ("allocation_step".into(), lunco_hooks::HookValue::Float(0.1)),
            ("actuator_count".into(), lunco_hooks::HookValue::Int(1)),
        ]);
        let value = lunco_hooks::invoke("synth.actuator-wrench", &[facts])
            .expect("actuator hook registered")
            .expect("actuator policy succeeds");
        let lunco_hooks::HookValue::Map(map) = value else {
            panic!("actuator policy must return a map");
        };
        let source = map
            .into_iter()
            .find_map(|(key, value)| (key == "source").then(|| value.as_str().map(str::to_owned)))
            .flatten()
            .expect("actuator policy source");
        assert!(source.contains("LunCo.Actuation.WrenchAllocator"));
        assert!(source.contains("wrench_matrix = ["));
        assert!(source.contains("allocation_step = 0.1"));
        assert!(source.contains("allocator.desired_torque_z = desired_torque_z;"));
        assert!(source.contains("allocator.desired_force_x = 0.0;"));
        assert!(source.contains("valve = allocator.command[1];"));
        assert!(source.contains("Force allocation | 1 actuator(s)"));
        let ast = rumoca_phase_parse::parse_to_ast(&source, "wrench.mo")
            .expect("generated actuator visual schema must remain valid Modelica");
        let class =
            lunco_modelica::diagram::find_class_by_qualified_name(&ast, "AttitudeActuation")
                .expect("generated actuator model");
        assert!(lunco_modelica::annotations::extract_icon(&class.annotation).is_some());
        assert!(lunco_modelica::annotations::extract_diagram(&class.annotation).is_some());
    }
}
