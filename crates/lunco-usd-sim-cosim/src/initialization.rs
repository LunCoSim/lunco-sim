//! Coupled t=0 admission for connected Modelica participants.
//!
//! An initialization-only worker job solves the initial equations at one
//! physical coordinate. Its result admits live construction; the coordinator
//! certifies the resulting participants against that source and wiring snapshot.

use super::*;
use lunco_cosim_core::initialization::{
    ConsistentInitialInputs, CoupledInitializationPending, PendingInputConnections,
};
use lunco_hooks::HookValue;
use std::collections::BTreeMap;

lunco_hooks::declare_hook! {
    id: "synth.coupled-initialization",
    owner: "lunco-usd-sim-cosim",
    description: "Emit a Modelica initial-equation composite from an admitted causal graph.",
    signature: [plan: Map],
    output: Map,
    deterministic: true,
    required: true,
    installable: true,
}

#[derive(Clone, Debug, PartialEq)]
struct Member {
    entity: Entity,
    path: String,
    session: u64,
    uri: String,
    source: String,
    class: String,
    interface_revision: u64,
    outputs: BTreeSet<String>,
    bindings: Vec<InputBinding>,
    parameters: BTreeMap<String, f64>,
    handle: Option<Handle<ModelicaSource>>,
    authored: BTreeMap<String, u64>,
    resume: bool,
}

/// One complete checked input aggregate. Policy owns array-literal syntax.
#[derive(Clone, Debug, PartialEq)]
struct InputBinding {
    name: String,
    dimensions: Vec<u32>,
    values: Vec<String>,
}

fn input_bindings(
    path: &str,
    interface: &lunco_modelica_ast::resolved_interface::ResolvedModelicaInterface,
    mut values: BTreeMap<String, String>,
) -> Result<Vec<InputBinding>, String> {
    use lunco_modelica_ast::resolved_interface::{ModelicaInputDefaults, ModelicaScalarValue};
    let mut bindings = Vec::new();
    for group in &interface.input_groups {
        if !group
            .scalar_names
            .iter()
            .any(|name| values.contains_key(name))
        {
            continue;
        }
        if group.aggregate_name_is_indexed {
            return Err(format!(
                "{path}.{} requires an indexed aggregate modifier, which coupled initialization cannot represent",
                group.name
            ));
        }
        let count = group
            .dimensions
            .iter()
            .try_fold(1usize, |count, size| count.checked_mul(*size as usize))
            .ok_or_else(|| format!("{path}.{} input shape overflows", group.name))?;
        if count != group.scalar_names.len() {
            return Err(format!(
                "{path}.{} checked input shape and scalar order disagree",
                group.name
            ));
        }
        let mut elements = Vec::with_capacity(count);
        for (index, name) in group.scalar_names.iter().enumerate() {
            if let Some(value) = values.remove(name) {
                elements.push(value);
                continue;
            }
            let Some(ModelicaInputDefaults::Known(defaults)) = &group.default_values else {
                return Err(format!(
                    "{path}.{name} has no admitted scalar binding or checked static declaration default; a partial array binding cannot replace its aggregate default"
                ));
            };
            if defaults.len() != count {
                return Err(format!(
                    "{path}.{} checked default and scalar order disagree",
                    group.name
                ));
            }
            let kind = group
                .scalar_type
                .ok_or_else(|| format!("{path}.{name} has no checked primitive type"))?;
            elements.push(
                ModelicaScalarValue::from_numeric(kind, defaults[index])
                    .map_err(|error| format!("{path}.{name}: {error}"))?
                    .source(),
            );
        }
        bindings.push(InputBinding {
            name: group.name.clone(),
            dimensions: group.dimensions.clone(),
            values: elements,
        });
    }
    if let Some((name, _)) = values.first_key_value() {
        return Err(format!(
            "{path}.{name} has no checked input aggregate owner"
        ));
    }
    Ok(bindings)
}

// A unit definition can change without rewriting the edge. Certification must
// retain the resolved coefficients as well as the authored connection fields.
type WireSnapshot = (
    Entity,
    String,
    Entity,
    String,
    bool,
    u64,
    u64,
    Result<(u64, u64), String>,
);

#[derive(Clone, Debug, PartialEq)]
struct Plan {
    generation: u64,
    members: Vec<Member>,
    probes: Vec<(Entity, String, String)>,
    wires: Vec<WireSnapshot>,
}

struct Job {
    request_id: lunco_modelica_runtime::ModelicaPreparationId,
    plan: Plan,
}

struct Certificate {
    plan: Plan,
    values: BTreeMap<Entity, Vec<(String, f64)>>,
    states: BTreeMap<Entity, Vec<(String, f64)>>,
    receipt: Option<CoupledInitializationReceipt>,
}

/// Worker-issued diagnostics from the current scene's last certified initialization.
pub struct CoupledInitializationReceipt {
    pub scene_generation: u64,
    pub solver: std::sync::Arc<lunco_modelica_runtime::ModelicaLiveSolverSnapshot>,
    pub construction_profile: lunco_modelica_runtime::ModelicaConstructionProfile,
}

/// Read the scene-scoped diagnostic receipt; it does not authorize admission.
pub fn last_coupled_initialization(world: &World) -> Option<&CoupledInitializationReceipt> {
    world
        .get_resource::<CoupledInitialization>()?
        .last_completed
        .as_ref()
}

#[derive(Resource, Default)]
pub(super) struct CoupledInitialization {
    failed: bool,
    job: Option<Job>,
    certificate: Option<Certificate>,
    last_completed: Option<CoupledInitializationReceipt>,
}

fn wire_snapshot(world: &mut World) -> Vec<WireSnapshot> {
    let registry = world
        .resource::<lunco_port_core::ports::PortRegistry>()
        .clone();
    let mut query = world.query::<&SimConnection>();
    let mut values: Vec<_> = query
        .iter(world)
        .map(|wire| {
            (
                wire.end_element,
                wire.end_connector.clone(),
                wire.start_element,
                wire.start_connector.clone(),
                wire.start_is_input,
                wire.scale.to_bits(),
                wire.offset.to_bits(),
                wire.resolved_transform(&registry, world)
                    .map(|(scale, offset)| (scale.to_bits(), offset.to_bits())),
            )
        })
        .collect();
    values.sort();
    values
}

fn string(value: impl Into<String>) -> HookValue {
    HookValue::Str(value.into())
}
fn map(values: impl IntoIterator<Item = (&'static str, HookValue)>) -> HookValue {
    HookValue::Map(
        values
            .into_iter()
            .map(|(name, value)| (name.into(), value))
            .collect(),
    )
}

impl Plan {
    fn facts(&self) -> HookValue {
        map([
            ("model_name", string("LunCoCoupledInitialization")),
            (
                "members",
                HookValue::Array(
                    self.members
                        .iter()
                        .enumerate()
                        .map(|(index, member)| {
                            map([
                                ("class", string(&member.class)),
                                ("instance", string(format!("p{index}"))),
                                (
                                    "bindings",
                                    HookValue::Array(
                                        member
                                            .bindings
                                            .iter()
                                            .map(|binding| {
                                                map([
                                                    ("name", string(&binding.name)),
                                                    (
                                                        "dimensions",
                                                        HookValue::Array(
                                                            binding
                                                                .dimensions
                                                                .iter()
                                                                .map(|size| {
                                                                    HookValue::Int(i64::from(*size))
                                                                })
                                                                .collect(),
                                                        ),
                                                    ),
                                                    (
                                                        "values",
                                                        HookValue::Array(
                                                            binding
                                                                .values
                                                                .iter()
                                                                .map(string)
                                                                .collect(),
                                                        ),
                                                    ),
                                                ])
                                            })
                                            .collect(),
                                    ),
                                ),
                            ])
                        })
                        .collect(),
                ),
            ),
            (
                "probes",
                HookValue::Array(
                    self.probes
                        .iter()
                        .enumerate()
                        .map(|(index, (_, _, value))| {
                            map([
                                ("name", string(format!("initial_{index}"))),
                                ("value", string(value)),
                            ])
                        })
                        .collect(),
                ),
            ),
        ])
    }
}

struct InitialExpressions<'a> {
    world: &'a World,
    registry: &'a lunco_port_core::ports::PortRegistry,
    members: &'a [Member],
    indices: &'a BTreeMap<Entity, usize>,
    input_names: &'a BTreeMap<Entity, BTreeSet<String>>,
    wires: &'a [SimConnection],
}

enum InitialExpression {
    Constant(f64),
    Port(usize, String),
    Sum(Vec<Self>),
    Affine(Box<Self>, f64, f64),
}

impl InitialExpression {
    fn references_participant(&self) -> bool {
        match self {
            Self::Constant(_) => false,
            Self::Port(_, _) => true,
            Self::Sum(terms) => terms.iter().any(Self::references_participant),
            Self::Affine(source, _, _) => source.references_participant(),
        }
    }
    fn source(&self) -> String {
        match self {
            Self::Constant(value) => value.to_string(),
            Self::Port(index, name) => format!("p{index}.{name}"),
            Self::Sum(terms) => format!(
                "({})",
                terms
                    .iter()
                    .map(Self::source)
                    .collect::<Vec<_>>()
                    .join(" + ")
            ),
            Self::Affine(source, scale, offset) => {
                format!("({} * {scale} + {offset})", source.source())
            }
        }
    }
    fn value(&self) -> Result<f64, String> {
        let value = match self {
            Self::Constant(value) => *value,
            Self::Port(_, _) => {
                return Err(
                    "initialization observation still has an unsolved participant reference".into(),
                );
            }
            Self::Sum(terms) => terms
                .iter()
                .try_fold(0.0, |sum, term| term.value().map(|value| sum + value))?,
            Self::Affine(source, scale, offset) => source.value()? * scale + offset,
        };
        if value.is_finite() {
            Ok(value)
        } else {
            Err("initialization observation is not finite".into())
        }
    }
}

impl InitialExpressions<'_> {
    fn resolve(
        &self,
        entity: Entity,
        name: &str,
        input: bool,
        visiting: &mut BTreeSet<(Entity, String, bool)>,
    ) -> Result<Option<InitialExpression>, String> {
        if let Some(index) = self.indices.get(&entity).filter(|index| {
            if input {
                self.input_names[&entity].contains(name)
            } else {
                self.members[**index].outputs.contains(name)
            }
        }) {
            return Ok(Some(InitialExpression::Port(*index, name.into())));
        }
        let key = (entity, name.to_owned(), input);
        if !visiting.insert(key.clone()) {
            return Err(format!("native initialization equation cycles at {name}"));
        }
        let result = self.resolve_native(entity, name, input, visiting);
        visiting.remove(&key);
        result
    }

    fn resolve_native(
        &self,
        entity: Entity,
        name: &str,
        input: bool,
        visiting: &mut BTreeSet<(Entity, String, bool)>,
    ) -> Result<Option<InitialExpression>, String> {
        if input {
            if let Some(value) = self
                .world
                .resource::<lunco_cosim_core::PortHolds>()
                .get(entity, name)
            {
                return value
                    .is_finite()
                    .then(|| Some(InitialExpression::Constant(value)))
                    .ok_or_else(|| format!("initialization hold {name} is not finite"));
            }
            let incoming: Vec<_> = self
                .wires
                .iter()
                .filter(|wire| wire.end_element == entity && wire.end_connector == name)
                .collect();
            if !incoming.is_empty() {
                let mut terms = Vec::new();
                for wire in incoming {
                    let Some(source) = self.resolve(
                        wire.start_element,
                        &wire.start_connector,
                        wire.start_is_input,
                        visiting,
                    )?
                    else {
                        return Ok(None);
                    };
                    let (scale, offset) = wire.resolved_transform(self.registry, self.world)?;
                    terms.push(InitialExpression::Affine(Box::new(source), scale, offset));
                }
                return Ok(Some(InitialExpression::Sum(terms)));
            }
        }
        let direction = if input {
            lunco_port_core::ports::PortDirection::In
        } else {
            lunco_port_core::ports::PortDirection::Out
        };
        use lunco_port_core::ports::PortInitialSource;
        let expression = match self
            .registry
            .initial_expression(self.world, entity, name, direction)?
        {
            PortInitialSource::Pending => {
                debug!(
                    ?entity,
                    name,
                    input,
                    symbolic = !self.members.is_empty(),
                    "initialization port owner is pending"
                );
                return Ok(None);
            }
            PortInitialSource::Sample => None,
            PortInitialSource::Expression(expression) => Some(expression),
        };
        if let Some(expression) = expression {
            if !expression.constant.is_finite() {
                return Err("initialization equation constant is not finite".into());
            }
            let mut terms = vec![InitialExpression::Constant(expression.constant)];
            for term in expression.terms {
                let input = match term.direction {
                    lunco_port_core::ports::PortDirection::In => true,
                    lunco_port_core::ports::PortDirection::Out => false,
                    _ => return Err("initialization equation has ambiguous causality".into()),
                };
                if !term.coefficient.is_finite() {
                    return Err("initialization equation coefficient is not finite".into());
                }
                let Some(source) = self.resolve(term.entity, &term.name, input, visiting)? else {
                    return Ok(None);
                };
                terms.push(InitialExpression::Affine(
                    Box::new(source),
                    term.coefficient,
                    0.0,
                ));
            }
            return Ok(Some(InitialExpression::Sum(terms)));
        }
        let Some(value) = lunco_cosim_core::initialization::read_causal_sample(
            self.registry,
            self.world,
            entity,
            name,
            input,
        ) else {
            debug!(
                ?entity,
                name,
                input,
                symbolic = !self.members.is_empty(),
                "initialization port sample is pending"
            );
            return Ok(None);
        };
        if !value.is_finite() {
            return Err(format!("initialization source {name} is not finite"));
        }
        Ok(Some(InitialExpression::Constant(value)))
    }
}

/// Capture only stalled Modelica participants; admitted producers supply their
/// real current samples through the registered port owner. Stable prim paths
/// determine instance identity and floating-point sum order.
fn capture(world: &mut World) -> Result<Option<Plan>, String> {
    let Some(generation) = world
        .get_resource::<lunco_core::SceneTransitionCoordinator>()
        .and_then(lunco_core::SceneTransitionCoordinator::lifecycle_generation)
    else {
        return Ok(None);
    };
    // The certificate covers every wire, including independently initialized
    // networks. Their declarations must exist before taking that snapshot.
    // Domain discovery holds admission before a network has an entity surface.
    if world
        .resource::<lunco_core_runtime::SimulationProgress>()
        .contains(lunco_core_runtime::SimulationProgressKey::usd_domain_discovery())
        || world
            .query_filtered::<Entity, With<lunco_port_core::PortSurfacePending>>()
            .iter(world)
            .next()
            .is_some()
    {
        return Ok(None);
    }
    let mut query = world.query_filtered::<(
        Entity,
        &UsdPrimPath,
        &SimComponent,
        Option<&PendingModelicaSource>,
        Option<&GeneratedModelicaSource>,
        Option<&ModelicaModel>,
    ), With<UsdSourcedCosim>>();
    let candidates: Vec<_> = query
        .iter(world)
        .filter(|(_, _, _, pending, generated, model)| {
            pending.is_some()
                || generated.is_some()
                    && model.is_some_and(|model| !model.is_compiled && model.last_error.is_none())
        })
        .map(|(entity, path, component, pending, generated, model)| {
            (
                entity,
                path.path.clone(),
                component
                    .inputs
                    .keys()
                    .map(str::to_owned)
                    .collect::<BTreeSet<_>>(),
                pending.map(|pending| {
                    (
                        pending.handle.clone(),
                        pending.asset_path.clone(),
                        pending.session_id,
                        pending.resume_after_compile,
                    )
                }),
                generated.cloned(),
                model.map(|model| {
                    (
                        model.model_name.clone(),
                        model.session_id,
                        model.resume_after_compile || !model.paused,
                    )
                }),
            )
        })
        .collect();
    if candidates.is_empty() {
        return Ok(None);
    }
    let mut candidates = candidates;
    candidates.sort_by(|a, b| a.1.cmp(&b.1));
    let mut members = Vec::new();
    let mut scalar_bindings = Vec::new();
    let mut input_names = BTreeMap::new();
    for (entity, path, _, pending, generated, model) in candidates {
        let resolved = world
            .get::<ResolvedUsdModelicaInterface>(entity)
            .ok_or_else(|| format!("{path} has no admitted resolved Modelica interface"))?
            .clone();
        if !resolved.matches_authored(world.get::<UsdInputDefaults>(entity)) {
            return Err(format!(
                "{path} authored bindings changed after interface admission"
            ));
        }
        let inputs = resolved.inputs.clone();
        if world.get::<ConsistentInitialInputs>(entity).is_some()
            || world
                .get::<lunco_port_core::PortSurfacePending>(entity)
                .is_some()
            || world
                .get::<PendingInputConnections>(entity)
                .is_some_and(|pending| pending.0.iter().any(|name| inputs.contains(name)))
        {
            return Ok(None);
        }
        // Ready acyclic participants use their own admission. Dependencies on
        // their outputs still wait in InitialExpressions until samples exist.
        if lunco_cosim_core::initialization::initialization_inputs(world, entity)?.is_some() {
            continue;
        }
        let (handle, resume) = if let Some((handle, _, _, resume)) = &pending {
            (Some(handle.clone()), *resume)
        } else {
            (None, model.as_ref().is_some_and(|(_, _, resume)| *resume))
        };
        let (uri, source, class, session) = if let Some((handle, uri, session, _)) = pending {
            let Some(source) = world.resource::<Assets<ModelicaSource>>().get(&handle) else {
                return Ok(None);
            };
            if !resolved.matches(session, &uri, &source.text) {
                return Err(format!(
                    "{path} source changed after resolved interface admission"
                ));
            }
            (
                uri,
                source.text.clone(),
                resolved.outcome.interface.model_name.clone(),
                session,
            )
        } else if let (Some(generated), Some((class, session, _))) = (generated, model.as_ref()) {
            if let Some(error) = generated.projection_error {
                return Err(error);
            }
            if !resolved.matches(*session, &resolved.outcome.doc_uri, &generated.source)
                || resolved.outcome.interface.model_name != *class
            {
                return Err(format!(
                    "{path} generated source changed after resolved interface admission"
                ));
            }
            (
                resolved.outcome.doc_uri.clone(),
                generated.source,
                class.clone(),
                *session,
            )
        } else {
            return Ok(None);
        };
        let outputs = resolved.outputs.clone();
        let parameters = &resolved.parameters;
        let mut bindings = BTreeMap::new();
        let mut parameter_values = BTreeMap::new();
        if let Some(defaults) = world.get::<UsdInputDefaults>(entity) {
            for (name, value) in &defaults.0 {
                if inputs.contains(name) || parameters.contains(name) {
                    let declaration = resolved
                        .outcome
                        .interface
                        .declarations
                        .get(name)
                        .ok_or_else(|| format!("{path}.{name} lost its resolved declaration"))?;
                    let binding = declaration
                        .numeric_binding_source(*value)
                        .map_err(|error| format!("{path}.{name}: {error}"))?;
                    if parameters.contains(name) {
                        parameter_values.insert(name.clone(), *value);
                    } else {
                        bindings.insert(name.clone(), binding);
                    }
                } else if !resolved.native_inputs.contains(name) {
                    return Err(format!(
                        "{path}.{name} has no Modelica or registered native input owner"
                    ));
                }
            }
        }
        input_names.insert(entity, inputs);
        let authored = world
            .get::<UsdInputDefaults>(entity)
            .map(|defaults| {
                defaults
                    .0
                    .iter()
                    .map(|(name, value)| (name.clone(), value.to_bits()))
                    .collect()
            })
            .unwrap_or_default();
        scalar_bindings.push(bindings);
        members.push(Member {
            entity,
            path,
            session,
            uri,
            source,
            class,
            interface_revision: resolved.outcome.library_revision,
            outputs,
            bindings: Vec::new(),
            parameters: parameter_values,
            handle,
            authored,
            resume,
        });
    }
    let indices: BTreeMap<_, _> = members
        .iter()
        .enumerate()
        .map(|(index, member)| (member.entity, index))
        .collect();
    let mut wires_query = world.query::<&SimConnection>();
    let mut all_wires: Vec<_> = wires_query.iter(world).cloned().collect();
    all_wires.sort_by(|a, b| a.compare_authoritative(b, world));
    let wires: Vec<_> = all_wires
        .iter()
        .filter(|wire| {
            input_names
                .get(&wire.end_element)
                .is_some_and(|names| names.contains(&wire.end_connector))
        })
        .cloned()
        .collect();
    let registry = world.resource::<lunco_port_core::ports::PortRegistry>();
    let expressions = InitialExpressions {
        world,
        registry,
        members: &members,
        indices: &indices,
        input_names: &input_names,
        wires: &all_wires,
    };
    let mut sums: BTreeMap<(usize, String), Vec<String>> = BTreeMap::new();
    let mut internal = false;
    for wire in wires {
        let destination = indices[&wire.end_element];
        if let Some(value) = world
            .resource::<lunco_cosim_core::PortHolds>()
            .get(wire.end_element, &wire.end_connector)
        {
            if !value.is_finite() {
                return Err("coupled initialization hold is not finite".into());
            }
            sums.insert((destination, wire.end_connector), vec![value.to_string()]);
            continue;
        }
        let Some(source) = expressions.resolve(
            wire.start_element,
            &wire.start_connector,
            wire.start_is_input,
            &mut BTreeSet::new(),
        )?
        else {
            return Ok(None);
        };
        internal |= source.references_participant();
        let (scale, offset) = wire.resolved_transform(registry, world)?;
        sums.entry((destination, wire.end_connector))
            .or_default()
            .push(InitialExpression::Affine(Box::new(source), scale, offset).source());
    }
    if !internal {
        return Ok(None);
    }
    let mut probes = Vec::new();
    for ((index, name), terms) in sums {
        scalar_bindings[index].insert(name.clone(), terms.join(" + "));
        probes.push((
            members[index].entity,
            name.clone(),
            format!("p{index}.{name}"),
        ));
    }
    for (member, values) in members.iter_mut().zip(scalar_bindings) {
        let interface = &world
            .get::<ResolvedUsdModelicaInterface>(member.entity)
            .ok_or_else(|| format!("{} lost its checked interface", member.path))?
            .outcome
            .interface;
        member.bindings = input_bindings(&member.path, interface, values)?;
    }
    let wires = wire_snapshot(world);
    Ok(Some(Plan {
        generation,
        members,
        probes,
        wires,
    }))
}

fn certify(world: &mut World, certificate: &Certificate) -> Result<bool, String> {
    if !world.contains_resource::<ModelicaChannels>() {
        return Err("Modelica construction channels retired during certification".into());
    }
    if world
        .get_resource::<lunco_core::SceneTransitionCoordinator>()
        .and_then(lunco_core::SceneTransitionCoordinator::lifecycle_generation)
        != Some(certificate.plan.generation)
        || wire_snapshot(world) != certificate.plan.wires
    {
        return Err("scene or wiring changed during coupled initialization".into());
    }
    let mut waiting = false;
    for member in &certificate.plan.members {
        if world
            .get::<ResolvedUsdModelicaInterface>(member.entity)
            .is_none_or(|interface| interface.outcome.library_revision != member.interface_revision)
        {
            return Err(format!(
                "{} prepared interface retired during initialization",
                member.path
            ));
        }
        let authored: BTreeMap<_, _> = world
            .get::<UsdInputDefaults>(member.entity)
            .map(|defaults| {
                defaults
                    .0
                    .iter()
                    .map(|(name, value)| (name.clone(), value.to_bits()))
                    .collect()
            })
            .unwrap_or_default();
        let source = if let Some(handle) = &member.handle {
            world
                .resource::<Assets<ModelicaSource>>()
                .get(handle)
                .map(|source| source.text.as_str())
        } else {
            world
                .get::<GeneratedModelicaSource>(member.entity)
                .map(|source| source.source.as_str())
        };
        if source != Some(member.source.as_str()) || authored != member.authored {
            return Err(format!(
                "{} source or authored values changed during initialization",
                member.path
            ));
        }
        let Some(model) = world.get::<ModelicaModel>(member.entity) else {
            if world
                .get::<PendingModelicaSource>(member.entity)
                .is_some_and(|pending| {
                    member.handle.as_ref() == Some(&pending.handle)
                        && member.uri == pending.asset_path
                        && member.session == pending.session_id
                })
            {
                waiting = true;
                continue;
            }
            return Err(format!(
                "{} initialization participant retired before construction completed",
                member.path
            ));
        };
        if let Some(error) = &model.last_error {
            return Err(error.clone());
        }
        if !model.is_compiled {
            waiting = true;
            continue;
        }
        let expected_session = world
            .get::<CoupledInitializationPending>(member.entity)
            .and_then(|pending| pending.expected_session);
        if expected_session != Some(model.session_id)
            || member.handle.is_none() && model.model_name != member.class
        {
            return Err(format!(
                "{} initialization participant was replaced before certification",
                member.path
            ));
        }
        if model.current_time != 0.0 {
            return Err("participant advanced before initialization certification".into());
        }
        let snapshot = model
            .live_solver_snapshot
            .as_ref()
            .ok_or_else(|| format!("{} initialized without a construction receipt", member.path))?;
        if let Some(receipt) = &certificate.receipt {
            if snapshot.admitted_library_revision != receipt.solver.admitted_library_revision {
                return Err(format!(
                    "{} source-root revision changed during initialization",
                    member.path
                ));
            }
        }
        for (name, expected) in certificate.values.get(&member.entity).into_iter().flatten() {
            if !snapshot
                .initial_inputs
                .iter()
                .any(|(input, value)| input == name && value.to_bits() == expected.to_bits())
            {
                return Err(format!(
                    "{}.{} constructor did not consume its admitted initial input",
                    member.path, name
                ));
            }
        }
    }
    if waiting {
        return Ok(false);
    }
    let mut query = world.query::<&SimConnection>();
    let mut wires: Vec<_> = query.iter(world).cloned().collect();
    wires.sort_by(|a, b| a.compare_authoritative(b, world));
    let registry = world.resource::<lunco_port_core::ports::PortRegistry>();
    let indices = BTreeMap::new();
    let names = BTreeMap::new();
    let expressions = InitialExpressions {
        world,
        registry,
        members: &[],
        indices: &indices,
        input_names: &names,
        wires: &wires,
    };
    for member in &certificate.plan.members {
        let model = world
            .get::<ModelicaModel>(member.entity)
            .ok_or("initialization participant retired")?;
        let parameters = model
            .live_solver_snapshot
            .as_ref()
            .ok_or("initialized participant has no solver settings")?
            .parameters;
        let states = certificate.states.get(&member.entity);
        let snapshot = model
            .live_solver_snapshot
            .as_ref()
            .ok_or("initialized participant has no construction receipt")?;
        for (name, value) in &snapshot.initial_state_constraints {
            if !states
                .into_iter()
                .flatten()
                .any(|(state, expected)| state == name && expected.to_bits() == value.to_bits())
            {
                return Err(format!(
                    "{}.{} constructor used an unadmitted state constraint",
                    member.path, name
                ));
            }
        }
        let variables = world
            .get::<ModelicaVariables>(member.entity)
            .ok_or("initialized participant has no required state observation")?;
        for (name, expected) in states.into_iter().flatten() {
            let value = variables
                .values()
                .get(name)
                .copied()
                .filter(|value| value.is_finite())
                .ok_or_else(|| {
                    format!("{}.{} has no finite reconstructed state", member.path, name)
                })?;
            let tolerance = parameters.atol + parameters.rtol * expected.abs().max(value.abs());
            if !tolerance.is_finite() || tolerance < 0.0 || (value - expected).abs() > tolerance {
                return Err(format!(
                    "{}.{} initialized state differs: solved {expected}, observed {value}",
                    member.path, name
                ));
            }
        }
        for (name, expected) in certificate.values.get(&member.entity).into_iter().flatten() {
            // Resolve the actual wired sources through the same initial-equation
            // contract, including native derived outputs, at the settled t=0.
            let Some(expression) =
                expressions.resolve(member.entity, name, true, &mut BTreeSet::new())?
            else {
                return Ok(false);
            };
            let value = expression.value()?;
            let tolerance = parameters.atol + parameters.rtol * expected.abs().max(value.abs());
            if !tolerance.is_finite() || tolerance < 0.0 {
                return Err("initialization certification tolerance is invalid".into());
            }
            if (value - expected).abs() > tolerance {
                return Err(format!(
                    "{}.{} initialized inconsistently: solved {expected}, observed {value}",
                    member.path, name
                ));
            }
        }
    }
    Ok(true)
}

fn fail(world: &mut World, plan: &Plan, error: String) {
    error!("[usd-cosim] coupled initialization failed: {error}");
    world.resource_mut::<CoupledInitialization>().failed = true;
    for member in &plan.members {
        if let Some(channels) = world.get_resource::<ModelicaChannels>() {
            let _ = channels.tx.send(ModelicaCommand::Despawn {
                entity: member.entity,
            });
        }
        if let Some(mut model) = world.get_mut::<ModelicaModel>(member.entity) {
            model.last_error = Some(error.clone());
            model.is_compiling = false;
            model.is_stepping = false;
            model.pending_step_inputs = None;
            model.resume_after_compile = false;
            model.paused = true;
        }
        if let Some(mut component) = world.get_mut::<SimComponent>(member.entity) {
            component.status = SimStatus::Error(error.clone());
        }
        if let Ok(mut entity) = world.get_entity_mut(member.entity) {
            entity.remove::<PendingModelicaSource>();
            entity.remove::<ConsistentInitialInputs>();
            entity.remove::<CoupledInitializationPending>();
        }
    }
    if let Some(mut faults) = world.get_resource_mut::<lunco_core::RuntimeFaults>() {
        faults.raise(
            "modelica-initialization-failed",
            None,
            "Coupled Modelica initialization",
            error,
        );
    }
}

/// Admit a solved composite atomically or retain the physics/readiness holds.
/// Compiler work runs on the existing worker, never on the live World thread.
pub(super) fn coordinate(world: &mut World) {
    world.run_schedule(lunco_cosim_core::schedule::InitializationSampling);
    world.init_resource::<CoupledInitialization>();
    let certificate = world
        .resource_mut::<CoupledInitialization>()
        .certificate
        .take();
    if let Some(certificate) = certificate {
        match certify(world, &certificate) {
            Ok(false) => {
                world.resource_mut::<CoupledInitialization>().certificate = Some(certificate);
            }
            Ok(true) => {
                for member in &certificate.plan.members {
                    world
                        .entity_mut(member.entity)
                        .remove::<CoupledInitializationPending>();
                    if let Some(mut model) = world.get_mut::<ModelicaModel>(member.entity) {
                        model.paused = !member.resume;
                    }
                }
                info!(
                    "[usd-cosim] certified coupled initialization for {} participant(s)",
                    certificate.plan.members.len()
                );
                world.resource_mut::<CoupledInitialization>().last_completed = certificate.receipt;
            }
            Err(error) => fail(world, &certificate.plan, error),
        }
        return;
    }
    let job = world.resource_mut::<CoupledInitialization>().job.take();
    if let Some(job) = job {
        let Some(mut preparations) =
            world.get_resource_mut::<super::preparations::ModelicaPreparations>()
        else {
            fail(
                world,
                &job.plan,
                "Modelica preparation owner retired".into(),
            );
            return;
        };
        let Some(outcome) = preparations.take(job.request_id) else {
            world.resource_mut::<CoupledInitialization>().job = Some(job);
            return;
        };
        let outcome = outcome.and_then(|outcome| match outcome {
            lunco_modelica_runtime::ModelicaPreparationOutcome::Initialization(result) => {
                Ok(result)
            }
            _ => Err("initialization request received a different preparation purpose".into()),
        });
        // Source identity, parameters, current physical samples and every wire
        // are compared before accepting a worker result, including its order.
        match capture(world) {
            Ok(Some(current)) if current == job.plan => {
                let outcome = outcome.and_then(|result| {
                    let expected_revision = job
                        .plan
                        .members
                        .first()
                        .map(|member| member.interface_revision);
                    if expected_revision.is_none()
                        || result.solver.admitted_library_revision != expected_revision
                    {
                        return Err(
                            "initialization result belongs to a different source-root revision"
                                .into(),
                        );
                    }
                    if result.values.len() != job.plan.probes.len() {
                        return Err(
                            "initialization result probe count differs from its request".into()
                        );
                    }
                    let receipt = Some(CoupledInitializationReceipt {
                        scene_generation: job.plan.generation,
                        solver: result.solver,
                        construction_profile: result.construction_profile,
                    });
                    let mut states: BTreeMap<Entity, Vec<(String, f64)>> = BTreeMap::new();
                    for (name, value) in result.state_values {
                        if !value.is_finite() {
                            return Err("initialization result contains a non-finite state".into());
                        }
                        let owner = job
                            .plan
                            .members
                            .iter()
                            .enumerate()
                            .find_map(|(index, member)| {
                                name.strip_prefix(&format!("p{index}."))
                                    .filter(|relative| !relative.is_empty())
                                    .map(|relative| (member.entity, relative.to_owned()))
                            })
                            .ok_or_else(|| {
                                format!("initial state `{name}` has no admitted member owner")
                            })?;
                        states.entry(owner.0).or_default().push((owner.1, value));
                    }
                    let values = result.values;
                    let mut inputs: BTreeMap<Entity, Vec<(String, f64)>> = BTreeMap::new();
                    for (index, (entity, name, _)) in job.plan.probes.iter().enumerate() {
                        let value = values
                            .get(index)
                            .copied()
                            .filter(|value| value.is_finite())
                            .ok_or("initialization result has no finite input observation")?;
                        inputs
                            .entry(*entity)
                            .or_default()
                            .push((name.clone(), value));
                    }
                    for member in &job.plan.members {
                        world
                            .entity_mut(member.entity)
                            .insert(CoupledInitializationPending {
                                expected_session: member.handle.as_ref().map(|_| member.session),
                            });
                        world
                            .entity_mut(member.entity)
                            .insert(ConsistentInitialInputs(
                                lunco_cosim_core::initialization::InitialInputAdmission {
                                    inputs: inputs.get(&member.entity).cloned().unwrap_or_default(),
                                    state_values: Some(
                                        states.get(&member.entity).cloned().unwrap_or_default(),
                                    ),
                                },
                            ));
                        if let Some(mut model) = world.get_mut::<ModelicaModel>(member.entity) {
                            model.paused = true;
                        }
                    }
                    world.resource_mut::<CoupledInitialization>().certificate = Some(Certificate {
                        plan: job.plan.clone(),
                        values: inputs,
                        states,
                        receipt,
                    });
                    Ok(())
                });
                if let Err(error) = outcome {
                    fail(world, &job.plan, error);
                }
            }
            Err(error) => fail(world, &job.plan, error),
            _ => {
                debug!("[usd-cosim] discarded superseded coupled initialization");
            }
        }
        return;
    }
    let plan = match capture(world) {
        Ok(Some(plan)) => plan,
        Ok(None) => return,
        Err(error) => {
            world.resource_mut::<CoupledInitialization>().failed = true;
            if let Some(mut faults) = world.get_resource_mut::<lunco_core::RuntimeFaults>() {
                faults.raise(
                    "modelica-initialization-failed",
                    None,
                    "Coupled Modelica initialization",
                    error,
                );
            }
            return;
        }
    };
    let context = lunco_core::RuntimeExecutionContext {
        target_twin_id: None,
        route: Some(lunco_core::RuntimeRoute::twin(
            lunco_core::RuntimeCycle::Lifecycle,
            plan.generation,
        )),
        phase: lunco_core::RuntimePhase::Preparation,
        clock: lunco_core::RuntimeClock::None,
        time_seconds: None,
        delta_seconds: None,
        sequence: None,
        producer: None,
    };
    let source =
        lunco_hooks::invoke_with_context("synth.coupled-initialization", &[plan.facts()], context)
            .ok_or("coupled initialization synthesis policy is missing".to_owned())
            .and_then(|result| result.map_err(|error| error.0))
            .and_then(|value| match value {
                HookValue::Map(values) => values
                    .into_iter()
                    .find_map(|(name, value)| {
                        if name == "source" {
                            value.as_str().map(str::to_owned)
                        } else {
                            None
                        }
                    })
                    .ok_or("coupled initialization policy has no source".to_owned()),
                _ => Err("coupled initialization policy must return a map".into()),
            });
    let source = match source {
        Ok(source) => source,
        Err(error) => {
            fail(world, &plan, error);
            return;
        }
    };
    let sources: BTreeMap<_, _> = plan
        .members
        .iter()
        .map(|member| (member.uri.clone(), member.source.clone()))
        .collect();
    let Some(first) = plan.members.first() else {
        fail(world, &plan, "initialization plan has no members".into());
        return;
    };
    let library_revision = first.interface_revision;
    if plan
        .members
        .iter()
        .any(|member| member.interface_revision != library_revision)
    {
        fail(
            world,
            &plan,
            "initialization interfaces belong to different source-root revisions".into(),
        );
        return;
    }
    let request_id = match world
        .resource_mut::<super::preparations::ModelicaPreparations>()
        .allocate(plan.generation)
    {
        Ok(id) => id,
        Err(error) => {
            fail(world, &plan, error);
            return;
        }
    };
    let sent = world
        .resource::<ModelicaChannels>()
        .tx
        .send(ModelicaCommand::Initialize {
            request_id,
            expected_library_revision: Some(library_revision),
            model_name: "LunCoCoupledInitialization".into(),
            source,
            doc_uri: "generated://coupled-initialization.mo".into(),
            extra_sources: sources.into_iter().collect(),
            authored_bindings: Default::default(),
            parameter_overrides: plan
                .members
                .iter()
                .enumerate()
                .flat_map(|(index, member)| {
                    member
                        .parameters
                        .iter()
                        .map(move |(name, value)| (format!("p{index}.{name}"), *value))
                })
                .collect(),
            initial_inputs: Vec::new(),
            probes: (0..plan.probes.len())
                .map(|index| format!("initial_{index}"))
                .collect(),
        });
    if sent.is_err() {
        world.resource_scope(
            |world, mut preparations: Mut<super::preparations::ModelicaPreparations>| {
                preparations.cancel(request_id, world.get_resource::<ModelicaChannels>());
            },
        );
        fail(world, &plan, "Modelica worker channel closed".into());
        return;
    }
    info!(
        "[usd-cosim] solving coupled initialization for {} participant(s)",
        plan.members.len()
    );
    world.resource_mut::<CoupledInitialization>().job = Some(Job { request_id, plan });
}

/// Admit initial capture, ready worker outcomes and pending certification.
pub(super) fn needed(
    pending: Query<(), With<PendingModelicaSource>>,
    models: Query<&ModelicaModel, (With<UsdSourcedCosim>, With<GeneratedModelicaSource>)>,
    owner: Option<Res<CoupledInitialization>>,
    actors: Query<&ModelicaModel>,
    preparations: Option<Res<super::preparations::ModelicaPreparations>>,
    holds: Query<&CoupledInitializationPending>,
) -> bool {
    if owner.as_ref().is_some_and(|owner| owner.failed) {
        return false;
    }
    if let Some(job) = owner.as_ref().and_then(|owner| owner.job.as_ref()) {
        return preparations.is_none_or(|preparations| preparations.ready(job.request_id));
    }
    if let Some(certificate) = owner.as_ref().and_then(|owner| owner.certificate.as_ref()) {
        // Certification samples and revalidates once every participant has an
        // outcome. Pending constructors cannot yet supply a t=0 observation.
        let mut waiting = false;
        for member in &certificate.plan.members {
            let Ok(model) = actors.get(member.entity) else {
                return true;
            };
            if model.last_error.is_some() {
                return true;
            }
            if holds.get(member.entity).is_ok_and(|hold| {
                hold.expected_session
                    .is_some_and(|session| session != model.session_id)
            }) {
                return true;
            }
            waiting |= !model.is_compiled;
        }
        return !waiting;
    }
    !pending.is_empty()
        || models
            .iter()
            .any(|model| !model.is_compiled && model.last_error.is_none())
}

/// Cancel preparation with its owning scene before another scene can publish it.
pub(super) fn retire(world: &mut World) {
    if let Some(mut owner) = world.remove_resource::<CoupledInitialization>() {
        if let Some(certificate) = owner.certificate.take() {
            for member in certificate.plan.members {
                if let Ok(mut entity) = world.get_entity_mut(member.entity) {
                    entity.remove::<ConsistentInitialInputs>();
                    entity.remove::<CoupledInitializationPending>();
                }
            }
        }
        if let Some(job) = owner.job.take() {
            if world.contains_resource::<super::preparations::ModelicaPreparations>() {
                world.resource_scope(
                    |world, mut preparations: Mut<super::preparations::ModelicaPreparations>| {
                        preparations
                            .cancel(job.request_id, world.get_resource::<ModelicaChannels>());
                    },
                );
            } else if let Some(channels) = world.get_resource::<ModelicaChannels>() {
                let _ = channels.tx.send(ModelicaCommand::CancelPreparation {
                    request_id: job.request_id,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

    #[test]
    fn checked_input_aggregate_bindings_preserve_order_and_default_authority() {
        use lunco_modelica_ast::resolved_interface::{
            ModelicaInputDefaults, ModelicaInputGroup, ModelicaScalarType,
            ResolvedModelicaInterface,
        };
        let mut interface = ResolvedModelicaInterface {
            model_name: "ArrayInput".into(),
            declarations: BTreeMap::new(),
            input_groups: vec![ModelicaInputGroup {
                name: "u".into(),
                dimensions: vec![2],
                scalar_names: vec!["u[1]".into(), "u[2]".into()],
                scalar_type: Some(ModelicaScalarType::Real),
                aggregate_name_is_indexed: false,
                default_values: Some(ModelicaInputDefaults::Known(vec![4.0, 8.0])),
            }],
        };
        let partial = BTreeMap::from([("u[1]".into(), "p1.sample".into())]);
        let binding = input_bindings("/Owner", &interface, partial.clone()).unwrap();
        assert_eq!(
            binding,
            vec![InputBinding {
                name: "u".into(),
                dimensions: vec![2],
                values: vec!["p1.sample".into(), "8.0".into()],
            }]
        );
        interface.input_groups[0].default_values =
            Some(ModelicaInputDefaults::InitializationDependent);
        assert!(input_bindings("/Owner", &interface, partial.clone()).unwrap_err()
            .contains("/Owner.u[2] has no admitted scalar binding or checked static declaration default"));
        interface.input_groups[0].default_values = None;
        assert!(input_bindings("/Owner", &interface, partial.clone()).is_err());
        let full = BTreeMap::from([
            ("u[2]".into(), "p1.second".into()),
            ("u[1]".into(), "p1.first".into()),
        ]);
        assert_eq!(
            input_bindings("/Owner", &interface, full.clone()).unwrap()[0].values,
            vec!["p1.first", "p1.second"]
        );
        interface.input_groups[0].aggregate_name_is_indexed = true;
        assert!(
            input_bindings("/Owner", &interface, full.clone())
                .unwrap_err()
                .contains("indexed aggregate modifier")
        );
        interface.input_groups[0].aggregate_name_is_indexed = false;
        interface.input_groups[0].dimensions = vec![2, 2];
        assert!(
            input_bindings("/Owner", &interface, full)
                .unwrap_err()
                .contains("checked input shape and scalar order disagree")
        );
    }

    #[test]
    fn certification_waits_for_participant_outcomes() {
        let mut world = World::new();
        let first = world.spawn(ModelicaModel::default()).id();
        let second = world.spawn(ModelicaModel::default()).id();
        let members = [first, second]
            .into_iter()
            .map(|entity| Member {
                entity,
                path: String::new(),
                session: 1,
                uri: String::new(),
                source: String::new(),
                class: String::new(),
                interface_revision: 0,
                outputs: BTreeSet::new(),
                bindings: Vec::new(),
                parameters: BTreeMap::new(),
                handle: None,
                authored: BTreeMap::new(),
                resume: false,
            })
            .collect();
        world.insert_resource(CoupledInitialization {
            certificate: Some(Certificate {
                plan: Plan {
                    generation: 1,
                    members,
                    probes: Vec::new(),
                    wires: Vec::new(),
                },
                values: BTreeMap::new(),
                states: BTreeMap::new(),
                receipt: None,
            }),
            ..default()
        });
        assert!(!world.run_system_once(needed).unwrap());
        world.get_mut::<ModelicaModel>(first).unwrap().is_compiled = true;
        assert!(!world.run_system_once(needed).unwrap());
        world.get_mut::<ModelicaModel>(second).unwrap().is_compiled = true;
        assert!(world.run_system_once(needed).unwrap());

        world.get_mut::<ModelicaModel>(first).unwrap().is_compiled = false;
        world.get_mut::<ModelicaModel>(second).unwrap().last_error =
            Some("construction failed".into());
        assert!(world.run_system_once(needed).unwrap());
        world.entity_mut(second).remove::<ModelicaModel>();
        assert!(world.run_system_once(needed).unwrap());
    }
}
