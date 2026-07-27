//! Runtime projection of composed USD component networks into Modelica wrappers.
//!
//! A reusable part applies `LunCoProgramAPI` for its model facet. Modelica remains the
//! authority for equations and member types; USD supplies instances, constant
//! input opinions, and ordinary property connections between public members.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use bevy::prelude::*;
use lunco_modelica::{parse_model_interface, ModelicaChannels, ModelicaCommand, ModelicaModel,
    ModelicaNotice, NoticeLevel};
use lunco_usd_bevy::{CanonicalStages, UsdPrimPath, UsdRead, UsdStageAsset};
use openusd::sdf::Path as SdfPath;

// The USD side of a Modelica program facet — the class an asset names, the
// lexical rules for member/instance identifiers — is ONE reader, shared with the
// lint fact producer. See `lunco_usd_bevy::program`.
pub use lunco_usd_bevy::program::is_domain_network_root;
use lunco_usd_bevy::program::{
    is_modelica_identifier, modelica_identifier, modelica_member_class, modelica_path_identifier,
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
    /// Exact transient Modelica source sent to the compiler.
    pub source: String,
    /// Included composed USD component paths.
    pub component_paths: Vec<String>,
}

/// One public Modelica component facet authored in USD.
#[derive(Clone, Debug, PartialEq)]
pub struct DomainComponent {
    /// Composed USD path of the `LunCoProgramAPI` facet.
    pub path: String,
    /// Fully-qualified class derived from `info:sourceAsset`.
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
}

/// One authoring error that prevents a safe runtime projection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DomainProjectionError {
    /// USD prim or property carrying the invalid opinion.
    pub path: String,
    /// Actionable explanation suitable for the simulator console.
    pub message: String,
}

// A `Scope` is ONE compilation unit: one generated model on one entity carrying
// one `ModelicaModel`. There is deliberately no runtime island partition — a
// collection holding several electrically independent islands is an authoring
// error, reported by the `network-scope-multiple-islands` lint rule, which is
// where that policy lives. A partitioner here would have to invent extra
// entities to host the extra models, and the two definitions of "island" would
// then have to agree forever.

/// Emit one deterministic Modelica wrapper for a composed network scope.
pub fn emit_modelica(network: &DomainNetwork, model_name: &str) -> String {
    let model_name = modelica_identifier(model_name);
    let mut source = format!("model {model_name}\n");
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
    let boundary_by_source: BTreeMap<_, _> = network
        .input_sources
        .iter()
        .map(|(boundary, source)| (source.as_str(), boundary.as_str()))
        .collect();

    for input in &network.inputs {
        source.push_str(&format!("  input Real {};\n", modelica_identifier(input)));
    }
    for output in network.outputs.keys() {
        source.push_str(&format!("  output Real {};\n", modelica_identifier(output)));
    }
    for component in &network.components {
        source.push_str(&format!("  // USD: {}\n", component.path));
        source.push_str(&format!(
            "  {} {}",
            component.model_class,
            names[component.path.as_str()]
        ));
        if !component.constants.is_empty() {
            source.push('(');
            for (index, (name, value)) in component.constants.iter().enumerate() {
                if index > 0 {
                    source.push_str(", ");
                }
                source.push_str(name);
                source.push_str(" = ");
                source.push_str(&value.to_string());
            }
            source.push(')');
        }
        source.push_str(";\n");
    }

    source.push_str("equation\n");
    let mut emitted_edges = BTreeSet::new();
    for component in &network.components {
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
                let left = format!("{local_instance}.{connector}");
                let right = format!("{target_instance}.{target_connector}");
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
            let boundary_prefix = format!("{}.inputs:", network.root);
            if let Some(boundary) = target.strip_prefix(&boundary_prefix) {
                source.push_str(&format!(
                    "  {local_instance}.{input} = {};\n",
                    modelica_identifier(boundary)
                ));
            } else if let Some(boundary) = boundary_by_source.get(target.as_str()) {
                // OpenUSD may resolve a connection through the Scope input and
                // return its ultimate source. Preserve the authored wrapper
                // boundary instead of bypassing it.
                source.push_str(&format!(
                    "  {local_instance}.{input} = {};\n",
                    modelica_identifier(boundary)
                ));
            } else if let Some((target_prim, output)) = target.split_once(".outputs:") {
                if let Some(target_instance) = names.get(target_prim) {
                    source.push_str(&format!(
                        "  {local_instance}.{input} = {target_instance}.{output};\n"
                    ));
                }
            }
        }
    }
    for (output, target) in &network.outputs {
        if let Some((target_prim, member)) = target.split_once(".outputs:") {
            if let Some(instance) = names.get(target_prim) {
                source.push_str(&format!(
                    "  {} = {instance}.{member};\n",
                    modelica_identifier(output)
                ));
            }
        }
    }
    source.push_str(&format!("end {model_name};\n"));
    source
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
    channels: Option<Res<ModelicaChannels>>,
    mut notices: MessageWriter<ModelicaNotice>,
) {
    if added.is_empty() && identity_added.is_empty() && !dirty.0 {
        return;
    }
    let Some(channels) = channels else { return };

    for (entity, prim, previous, installed_model) in &prims {
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
        let instance_id =
            lunco_usd_bevy::instance_key(entity, &q_provenance, &q_gid, &q_instance_root);
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
        let network = match read_network(&view, &root_path) {
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
                let model_name = network_model_name(&prim.path, instance_id);
                notices.write(ModelicaNotice {
                    level: NoticeLevel::Error,
                    text: format!("[{model_name}] Projection error: {message}"),
                });
                error!("[domain-projection] `{}` rejected: {message}", prim.path);
                retire_sim_interface(&mut commands, entity);
                // A rejected projection has no interface to hold anyone to; the
                // rejection itself is the error the user must act on.
                commands
                    .entity(entity)
                    .remove::<UsdModelicaPortContract>();
                commands.entity(entity).try_insert((
                    ModelicaModel {
                        model_path: PathBuf::from(format!("generated://{model_name}.mo")),
                        model_name,
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
                        source: String::new(),
                        component_paths: Vec::new(),
                    },
                ));
                continue;
            }
        };
        let Some(network) = network else {
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
        let model_name = network_model_name(&network.root, instance_id);
        let source = emit_modelica(&network, &model_name);
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
            model_path: PathBuf::from(&doc_uri),
            model_name: compiled_name.clone(),
            parameters: interface.parameters,
            inputs: interface.inputs,
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
            doc_uri,
            extra_sources: Vec::new(),
            stream: None,
            // A projected domain island is a NETWORK of components — a battery
            // bus, a thermal loop — not a program driving a client-predicted
            // body. It is `NotPredictable` by construction, so it takes the
            // replicated class's adaptive implicit solver. Handing it the
            // realtime class's explicit stepper is what silently killed every
            // solar rover: the island compiled, then failed every step with
            // `algebraic refresh row 2 cannot be solved` and published nothing.
            realtime_safe: false,
        });
        info!(
            "[domain-projection] compiling `{}` from {} component(s) as generated://{}.mo",
            network.root,
            network.components.len(),
            model_name
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
        // A changed wrapper may expose a different port interface. Rebuild the
        // derived co-sim projection instead of retaining values and port names
        // from the previous compiled topology.
        retire_sim_interface(&mut commands, entity);
        commands.entity(entity).try_insert((
            model,
            UsdSourcedCosim,
            // The same USD-declares / compiler-confirms contract the per-prim
            // program path has carried all along: the network's boundary is an
            // authored promise, and `validate_usd_modelica_port_contracts` is
            // what turns a promise the DAE does not keep into one durable,
            // actionable error instead of an island that steps and publishes
            // nothing.
            UsdModelicaPortContract::new(
                network.inputs.iter().cloned(),
                network.outputs.keys().cloned(),
            ),
            DomainProjectionState { fingerprint },
            GeneratedModelicaSource {
                network_root: network.root.clone(),
                source: source_for_diagnostics,
                component_paths: network
                    .components
                    .iter()
                    .map(|component| component.path.clone())
                    .collect(),
            },
        ));
    }
}

/// `GeneratedModelicaSource` — read back the exact Modelica text a projected
/// network was compiled from.
///
/// `curl … {"command":"GeneratedModelicaSource","params":{}}` lists every
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
                    "doc_uri": model
                        .map(|model| model.model_path.display().to_string())
                        .unwrap_or_default(),
                    "error": model.and_then(|model| model.last_error.clone()),
                    "components": generated.component_paths,
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
        let model_class = match modelica_member_class(view, &path) {
            Ok(class) => class,
            Err(issue) => {
                extraction_errors.push(DomainProjectionError {
                    path: issue.property,
                    message: issue.message,
                });
                continue;
            }
        };
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

#[cfg(test)]
mod tests {
    use super::*;

    fn component(path: &str, target: Option<&str>) -> DomainComponent {
        DomainComponent {
            path: path.into(),
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
        };
        let source = emit_modelica(&network, "Electrical System");
        assert!(source.contains("input Real drive_left;"));
        assert!(source.contains("Left_x2f_Motor_x2f_Model.demand = drive_left;"));
        assert!(source.contains("connect(Battery_x2f_Model.p, Left_x2f_Motor_x2f_Model.p);"));
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
        };
        let source = emit_modelica(&network, "Electrical");
        assert!(source.contains("connect(Bus_x2f_Model.p, LoadA_x2f_Model.p);"));
        assert!(source.contains("connect(Bus_x2f_Model.p, LoadB_x2f_Model.p);"));
    }

    #[test]
    fn rejects_external_connector_targets_without_rejecting_independent_islands() {
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
        };
        let errors = validate_network(&network);
        assert!(errors
            .iter()
            .any(|error| error.message.contains("outside collection")));
        assert!(errors
            .iter()
            .all(|error| !error.message.contains("multiple disconnected")));
    }

    #[test]
    fn unconnected_acausal_component_is_omitted_from_generated_network() {
        let panel = component("/Electrical/SolarPanel", None);
        let mut battery = component("/Electrical/Battery", None);
        battery.connectors.insert(
            "p".into(),
            vec!["/Electrical/Motor.connectors:p".into()],
        );
        let motor = component("/Electrical/Motor", None);
        let mut components = vec![panel, battery, motor];
        let omitted = retain_connected_acausal_components(&mut components);
        assert_eq!(
            components.iter().map(|component| component.path.as_str()).collect::<Vec<_>>(),
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
                    source: "model Electrical end Electrical;".into(),
                    component_paths: vec!["/Battery".into()],
                },
                lunco_cosim::SimComponent {
                    outputs: std::collections::HashMap::from([("soc".into(), 0.75)]),
                    ..default()
                },
            ))
            .id();

        app.update();

        assert!(
            app.world().get::<lunco_cosim::SimComponent>(entity).is_none(),
            "a changed or rejected projection must not retain solved values from its previous topology"
        );
    }
}
