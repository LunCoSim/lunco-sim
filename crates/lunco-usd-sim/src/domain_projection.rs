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
use openusd::usd::{compute_included_paths, Collection, PrimPredicate};

use crate::cosim::{UsdSourcedCosim, WiringDirty};

/// Fingerprint of the generated wrapper currently installed on a network scope.
#[derive(Component)]
pub struct DomainProjectionState {
    fingerprint: u64,
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
    pub connectors: BTreeMap<String, String>,
    /// Causal input name to its connected source property path.
    pub inputs: BTreeMap<String, String>,
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
    /// Public wrapper output name to component output property.
    pub outputs: BTreeMap<String, String>,
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
        for target in component.connectors.values() {
            if let Some((target_prim, _)) = target.split_once(".connectors:") {
                if let Some(&other) = owners.get(target_prim) {
                    union(&mut parent, index, other);
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
    let model_name = sanitize_identifier(model_name);
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

    for input in &network.inputs {
        source.push_str(&format!("  input Real {};\n", sanitize_identifier(input)));
    }
    for output in network.outputs.keys() {
        source.push_str(&format!("  output Real {};\n", sanitize_identifier(output)));
    }
    for component in &network.components {
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
        for (connector, target) in &component.connectors {
            let Some((target_prim, target_connector)) = target.split_once(".connectors:") else {
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
        for (input, target) in &component.inputs {
            let boundary_prefix = format!("{}.inputs:", network.root);
            if let Some(boundary) = target.strip_prefix(&boundary_prefix) {
                source.push_str(&format!(
                    "  {local_instance}.{input} = {};\n",
                    sanitize_identifier(boundary)
                ));
            }
        }
    }
    for (output, target) in &network.outputs {
        if let Some((target_prim, member)) = target.split_once(".outputs:") {
            if let Some(instance) = names.get(target_prim) {
                source.push_str(&format!(
                    "  {} = {instance}.{member};\n",
                    sanitize_identifier(output)
                ));
            }
        }
    }
    source.push_str(&format!("end {model_name};\n"));
    source
}

/// Reactively compile every ordinary `Scope` containing connector-bearing
/// Modelica program facets. The generated source is runtime projection only.
pub fn project_domain_islands(
    mut commands: Commands,
    added: Query<(), Added<UsdPrimPath>>,
    prims: Query<(
        Entity,
        &UsdPrimPath,
        Option<&DomainProjectionState>,
        Option<&ModelicaModel>,
    )>,
    stages: Res<Assets<UsdStageAsset>>,
    mut canonical: NonSendMut<CanonicalStages>,
    dirty: Res<WiringDirty>,
    channels: Option<Res<ModelicaChannels>>,
    mut notices: MessageWriter<ModelicaNotice>,
) {
    if added.is_empty() && !dirty.0 {
        return;
    }
    let Some(channels) = channels else { return };

    for (entity, prim, previous, installed_model) in &prims {
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
        let Some(network) = read_network(&view, &root_path) else {
            if previous.is_some() {
                // The authored collection ceased to describe a compilable
                // network. Retire its runtime projection in the same update;
                // keeping the old solver would simulate stale authoring.
                commands
                    .entity(entity)
                    .remove::<(ModelicaModel, UsdSourcedCosim, DomainProjectionState)>();
            }
            continue;
        };
        let model_name = network_model_name(&network.root);
        let source = emit_modelica(&network, &model_name);
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
        commands.entity(entity).try_insert((
            model,
            UsdSourcedCosim,
            DomainProjectionState { fingerprint },
        ));
    }
}

/// Stable, path-qualified identity for a generated network model.
///
/// The leaf name alone is not unique: a stage may contain several independent
/// scopes named `Electrical`. Including the composed prim path also keeps
/// worker sessions and diagnostics attributable to the authored network.
fn network_model_name(root: &str) -> String {
    format!("{}_System", sanitize_identifier(root.trim_matches('/')))
}

fn source_fingerprint(source: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    source.hash(&mut hasher);
    hasher.finish()
}

fn read_network(view: &lunco_usd_bevy::StageView<'_>, root: &SdfPath) -> Option<DomainNetwork> {
    let root_string = root.to_string();
    if !view.has_api_schema(root, "CollectionAPI:components") {
        return None;
    }
    let member_paths = if view
        .value_str(root, "collection:components:expansionRule")
        .as_deref()
        == Some("explicitOnly")
    {
        view.rel_targets(root, "collection:components:includes")
    } else {
        let collection = Collection::new(root.clone(), "components");
        let query = collection.compute_membership_query(view.stage()).ok()?;
        compute_included_paths(view.stage(), &query, PrimPredicate::DEFAULT).ok()?
    };
    let mut components = Vec::new();
    for path in member_paths {
        if path.is_property_path() || path.is_prim_variant_selection_path() {
            continue;
        }
        if !view.has_api_schema(&path, "LunCoProgramAPI") {
            continue;
        }
        let Some(source) = view.asset(&path, "info:sourceAsset") else {
            continue;
        };
        let Some(model_class) = model_class_from_asset(&source) else {
            continue;
        };
        let attrs = view.attr_names(&path);
        if !attrs.iter().any(|attr| attr.starts_with("connectors:")) {
            continue;
        }
        let mut constants = BTreeMap::new();
        let mut connectors = BTreeMap::new();
        let mut inputs = BTreeMap::new();
        for attr in attrs {
            if let Some(name) = attr.strip_prefix("connectors:") {
                if let Some(target) = view.connections(&path, &attr).first() {
                    connectors.insert(name.to_string(), target.to_string());
                }
            } else if let Some(name) = attr.strip_prefix("inputs:") {
                if let Some(target) = view.connections(&path, &attr).first() {
                    inputs.insert(name.to_string(), target.to_string());
                } else if let Some(value) = view.real(&path, &attr) {
                    constants.insert(name.to_string(), value);
                }
            }
        }
        components.push(DomainComponent {
            path: path.to_string(),
            model_class,
            constants,
            connectors,
            inputs,
        });
    }
    if components.is_empty() {
        return None;
    }

    let attrs = view.attr_names(root);
    let inputs = attrs
        .iter()
        .filter_map(|attr| attr.strip_prefix("inputs:").map(str::to_string))
        .collect();
    let outputs = attrs
        .iter()
        .filter_map(|attr| {
            let name = attr.strip_prefix("outputs:")?;
            let target = view.connections(root, attr).first()?.to_string();
            Some((name.to_string(), target))
        })
        .collect();
    Some(DomainNetwork {
        root: root_string,
        components,
        inputs,
        outputs,
    })
}

fn model_class_from_asset(asset: &str) -> Option<String> {
    let path = asset
        .strip_prefix("lunco://")
        .or_else(|| asset.strip_prefix("twin://"))
        .unwrap_or(asset);
    let model_path = path.split("models/").nth(1)?;
    let class = model_path.strip_suffix(".mo")?;
    Some(class.replace('/', "."))
}

fn instance_identifier(root: &str, path: &str) -> String {
    sanitize_identifier(path.strip_prefix(root).unwrap_or(path).trim_matches('/'))
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

fn sanitize_identifier(raw: &str) -> String {
    let mut result = String::with_capacity(raw.len() + 1);
    for (index, character) in raw.chars().enumerate() {
        if character.is_ascii_alphanumeric() || character == '_' {
            if index == 0 && character.is_ascii_digit() {
                result.push('_');
            }
            result.push(character);
        } else {
            result.push('_');
        }
    }
    if result.is_empty() {
        result.push_str("ModelicaNetwork");
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
                .map(|target| BTreeMap::from([("p".into(), target.into())]))
                .unwrap_or_default(),
            inputs: BTreeMap::new(),
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
            outputs: BTreeMap::new(),
        };
        let source = emit_modelica(&network, "Electrical System");
        assert!(source.contains("input Real drive_left;"));
        assert!(source.contains("Left_Motor_Model.demand = drive_left;"));
        assert!(source.contains("connect(Battery_Model.p, Left_Motor_Model.p);"));
    }

    #[test]
    fn derives_qualified_class_from_model_asset_path() {
        assert_eq!(
            model_class_from_asset("lunco://models/LunCo/Electrical/Battery.mo"),
            Some("LunCo.Electrical.Battery".into())
        );
    }

    #[test]
    fn generated_model_identity_is_qualified_by_network_path() {
        assert_ne!(
            network_model_name("/Rover/Electrical"),
            network_model_name("/Payload/Electrical")
        );
        assert_eq!(
            network_model_name("/Rover/Electrical"),
            "Rover_Electrical_System"
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
}
