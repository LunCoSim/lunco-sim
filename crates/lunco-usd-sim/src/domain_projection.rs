//! Runtime projection of composed USD component networks into Modelica wrappers.
//!
//! A reusable part applies `LunCoProgramAPI` for its model facet. Modelica remains the
//! authority for equations and member types; USD supplies instances, constant
//! input opinions, and ordinary property connections between public members.

use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, BTreeSet};
use std::hash::{Hash, Hasher};
use std::path::PathBuf;

use bevy::prelude::*;
use lunco_modelica::{
    extract_inputs_with_defaults_from_ast, extract_model_name_from_ast,
    extract_parameters_from_ast, ModelicaChannels, ModelicaCommand, ModelicaModel, ModelicaNotice,
    NoticeLevel,
};
use lunco_usd_bevy::{CanonicalStages, UsdPrimPath, UsdRead, UsdStageAsset};
use openusd::sdf::Path as SdfPath;

use crate::cosim::{UsdSourcedCosim, WiringDirty};

/// Fingerprint of the generated wrapper currently installed on a network scope.
#[derive(Component)]
pub struct DomainProjectionState {
    fingerprint: u64,
}

/// Inspectable runtime artifact for diagnostics and API/UI projection.
///
/// This is derived state, never persisted back into USD. Keeping the exact
/// compiler input beside the run entity makes a compiler line actionable.
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

/// Partition components by acausal connector connectivity.
///
/// The serialized USD edge has an authoring direction, but Modelica `connect()`
/// does not: both endpoint owners are unioned.
pub fn partition_islands(mut components: Vec<DomainComponent>) -> Vec<Vec<DomainComponent>> {
    components.sort_by(|a, b| a.path.cmp(&b.path));
    let mut parent: Vec<usize> = (0..components.len()).collect();
    let owners: BTreeMap<_, _> = components
        .iter()
        .enumerate()
        .map(|(index, component)| (component.path.as_str(), index))
        .collect();

    for (index, component) in components.iter().enumerate() {
        for targets in component.connectors.values() {
            for target in targets {
                if let Some((target_prim, _)) = target.split_once(".connectors:") {
                    if let Some(&other) = owners.get(target_prim) {
                        union(&mut parent, index, other);
                    }
                }
            }
        }
    }

    let mut groups = BTreeMap::<usize, Vec<DomainComponent>>::new();
    for (index, component) in components.into_iter().enumerate() {
        let root = find(&mut parent, index);
        groups.entry(root).or_default().push(component);
    }
    groups.into_values().collect()
}

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
                commands
                    .entity(entity)
                    .remove::<lunco_cosim::SimComponent>();
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
                commands.entity(entity).remove::<(
                    ModelicaModel,
                    UsdSourcedCosim,
                    DomainProjectionState,
                    GeneratedModelicaSource,
                    lunco_cosim::SimComponent,
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

        let ast = rumoca_phase_parse::parse_to_syntax(&source, "usd-network-projection.mo")
            .best_effort()
            .clone();
        let compiled_name = extract_model_name_from_ast(&ast).unwrap_or_else(|| model_name.clone());
        let session_id = installed_model.map_or(1, |model| model.session_id + 1);
        let doc_uri = format!("generated://{model_name}.mo");
        let mut model = ModelicaModel {
            model_path: PathBuf::from(&doc_uri),
            model_name: compiled_name.clone(),
            parameters: extract_parameters_from_ast(&ast),
            inputs: extract_inputs_with_defaults_from_ast(&ast)
                .into_iter()
                .collect(),
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
        commands
            .entity(entity)
            .remove::<lunco_cosim::SimComponent>();
        commands.entity(entity).try_insert((
            model,
            UsdSourcedCosim,
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

fn source_fingerprint(source: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    source.hash(&mut hasher);
    hasher.finish()
}

fn read_network(
    view: &lunco_usd_bevy::StageView<'_>,
    root: &SdfPath,
) -> Result<Option<DomainNetwork>, Vec<DomainProjectionError>> {
    let root_string = root.to_string();
    // Codeless multiple-apply schemas are not consistently surfaced by every
    // OpenUSD binding through `HasAPI`; their standard authored properties are
    // authoritative and round-trip in all runtimes.
    if !view
        .attr_names(root)
        .iter()
        .any(|name| name.starts_with("collection:components:"))
    {
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
            continue;
        }
        let implementation = view
            .value_str(&path, "info:implementationSource")
            .unwrap_or_default();
        if implementation != "sourceAsset" {
            extraction_errors.push(DomainProjectionError {
                path: format!("{path}.info:implementationSource"),
                message:
                    "a Modelica network member must use info:implementationSource = sourceAsset"
                        .into(),
            });
            continue;
        }
        let Some(source) = view.asset(&path, "info:sourceAsset") else {
            extraction_errors.push(DomainProjectionError {
                path: format!("{path}.info:sourceAsset"),
                message:
                    "a Modelica network member must use info:implementationSource = sourceAsset"
                        .into(),
            });
            continue;
        };
        let sub_identifier = view
            .value_str(&path, "info:sourceAsset:subIdentifier")
            .filter(|value| !value.is_empty());
        let Some(model_class) = model_class_from_asset(&source, sub_identifier.as_deref()) else {
            extraction_errors.push(DomainProjectionError {
                path: format!("{path}.info:sourceAsset"),
                message: format!(
                    "`{source}` is not a Modelica source asset; use a .mo asset and optional info:sourceAsset:subIdentifier"
                ),
            });
            continue;
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
    if components.is_empty() {
        return Err(vec![DomainProjectionError {
            path: root_string,
            message: "component collection contains no Modelica program facets".into(),
        }]);
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
        } else {
            outputs.insert(name.to_string(), targets[0].to_string());
        }
    }
    if !extraction_errors.is_empty() {
        return Err(extraction_errors);
    }
    let network = DomainNetwork {
        root: root_string,
        components,
        inputs,
        input_sources,
        outputs,
    };
    let errors = validate_network(&network);
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

    let acausal: Vec<_> = network
        .components
        .iter()
        .filter(|component| !component.declared_connectors.is_empty())
        .cloned()
        .collect();
    if partition_islands(acausal).len() > 1 {
        errors.push(DomainProjectionError {
            path: network.root.clone(),
            message: "component collection contains multiple disconnected acausal networks; author one Scope and CollectionAPI:components per independently solved network".into(),
        });
    }
    errors
}

fn model_class_from_asset(asset: &str, sub_identifier: Option<&str>) -> Option<String> {
    if let Some(class) = sub_identifier {
        return is_modelica_class_name(class).then(|| class.to_string());
    }
    let path = asset
        .strip_prefix("lunco://")
        .or_else(|| asset.strip_prefix("twin://"))
        .unwrap_or(asset);
    let model_path = path.split("models/").nth(1)?;
    let class = model_path.strip_suffix(".mo")?;
    Some(class.replace('/', "."))
}

fn is_modelica_class_name(class: &str) -> bool {
    !class.is_empty() && class.split('.').all(is_modelica_identifier)
}

fn instance_identifier(root: &str, path: &str) -> String {
    modelica_path_identifier(path.strip_prefix(root).unwrap_or(path).trim_matches('/'))
}

fn find(parent: &mut [usize], node: usize) -> usize {
    if parent[node] != node {
        parent[node] = find(parent, parent[node]);
    }
    parent[node]
}

fn union(parent: &mut [usize], left: usize, right: usize) {
    let left = find(parent, left);
    let right = find(parent, right);
    if left != right {
        parent[right] = left;
    }
}

fn is_modelica_identifier(raw: &str) -> bool {
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
fn modelica_identifier(raw: &str) -> String {
    if is_modelica_identifier(raw) {
        return raw.to_string();
    }
    let mut result = modelica_path_identifier(raw);
    if !result.starts_with("usd_") {
        result.insert_str(0, "usd_");
    }
    result
}

fn modelica_path_identifier(raw: &str) -> String {
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
    fn partitions_direct_pin_connections_into_independent_islands() {
        let islands = partition_islands(vec![
            component("/Power/Battery/Model", None),
            component(
                "/Power/MotorA/Model",
                Some("/Power/Battery/Model.connectors:p"),
            ),
            component("/Payload/Battery/Model", None),
            component(
                "/Payload/Camera/Model",
                Some("/Payload/Battery/Model.connectors:p"),
            ),
        ]);
        assert_eq!(islands.len(), 2);
        assert!(islands.iter().all(|island| island.len() == 2));
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
    fn rejects_disconnected_acausal_islands_and_external_targets() {
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
            .any(|error| error.message.contains("multiple disconnected")));
    }

    #[test]
    fn derives_qualified_class_from_model_asset_path() {
        assert_eq!(
            model_class_from_asset("lunco://models/LunCo/Electrical/Battery.mo", None),
            Some("LunCo.Electrical.Battery".into())
        );
        assert_eq!(
            model_class_from_asset(
                "lunco://models/vendor/package.mo",
                Some("Vendor.Power.CustomBattery")
            ),
            Some("Vendor.Power.CustomBattery".into())
        );
        assert_eq!(
            model_class_from_asset("lunco://models/vendor/package.mo", Some("bad-class")),
            None
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
}
