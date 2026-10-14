//! Coupled t=0 admission for connected Modelica participants.
//!
//! A paused, transient Modelica composite solves the initial equations at one
//! physical coordinate. The ordinary worker constructs live participants only
//! after the result is certified against the exact source and wiring snapshot.

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
    outputs: BTreeSet<String>,
    bindings: BTreeMap<String, String>,
    handle: Option<Handle<ModelicaSource>>,
    authored: BTreeMap<String, u64>,
    resume: bool,
}

#[derive(Clone, Debug, PartialEq)]
struct Plan {
    generation: u64,
    members: Vec<Member>,
    probes: Vec<(Entity, String, String)>,
    wires: Vec<(Entity, String, Entity, String, bool, u64, u64)>,
}

struct Job {
    actor: Entity,
    plan: Plan,
}

struct Certificate {
    plan: Plan,
    values: BTreeMap<Entity, Vec<(String, f64)>>,
}

#[derive(Resource, Default)]
pub(super) struct CoupledInitialization {
    job: Option<Job>,
    certificate: Option<Certificate>,
}

fn wire_snapshot(world: &mut World) -> Vec<(Entity, String, Entity, String, bool, u64, u64)> {
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
                                            .map(|(name, value)| {
                                                map([
                                                    ("name", string(name)),
                                                    ("value", string(value)),
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
                    if !wire.scale.is_finite() || !wire.offset.is_finite() {
                        return Err("initialization affine transform is not finite".into());
                    }
                    terms.push(InitialExpression::Affine(
                        Box::new(source),
                        wire.scale,
                        wire.offset,
                    ));
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
            PortInitialSource::Pending => return Ok(None),
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
    let mut input_names = BTreeMap::new();
    for (entity, path, inputs, pending, generated, model) in candidates {
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
        // Acyclic admission uses the same owner and does not need a composite.
        if lunco_cosim_core::initialization::initialization_inputs(world, entity)?.is_some() {
            return Ok(None);
        }
        let (handle, resume) = if let Some((handle, _, _, resume)) = &pending {
            (Some(handle.clone()), *resume)
        } else {
            (None, model.as_ref().is_some_and(|(_, _, resume)| *resume))
        };
        let (uri, source, class, session, outputs, parameters) =
            if let Some((handle, uri, session, _)) = pending {
                let Some(source) = world.resource::<Assets<ModelicaSource>>().get(&handle) else {
                    return Ok(None);
                };
                let Some(name) = source.interface.model_name.as_ref() else {
                    return Ok(None);
                };
                let class = match source
                    .interface
                    .within
                    .as_deref()
                    .filter(|within| !within.is_empty())
                {
                    Some(within) if !name.starts_with(&format!("{within}.")) => {
                        format!("{within}.{name}")
                    }
                    _ => name.clone(),
                };
                (
                    uri,
                    source.text.clone(),
                    class,
                    session,
                    source.interface.outputs.clone(),
                    source
                        .interface
                        .parameters
                        .keys()
                        .cloned()
                        .collect::<BTreeSet<_>>(),
                )
            } else if let (Some(generated), Some((class, session, _))) = (generated, model.as_ref())
            {
                if let Some(error) = generated.projection_error {
                    return Err(error);
                }
                let contract = world
                    .get::<UsdModelicaPortContract>(entity)
                    .ok_or_else(|| {
                        format!("{path} has no required generated Modelica port contract")
                    })?;
                (
                    generated.doc_uri,
                    generated.source,
                    class.clone(),
                    *session,
                    contract.outputs.clone(),
                    BTreeSet::new(),
                )
            } else {
                return Ok(None);
            };
        let mut bindings = BTreeMap::new();
        if let Some(defaults) = world.get::<UsdInputDefaults>(entity) {
            for (name, value) in &defaults.0 {
                if inputs.contains(name) || parameters.contains(name) {
                    if !value.is_finite() {
                        return Err(format!("{path}.{name} initial value is not finite"));
                    }
                    bindings.insert(name.clone(), value.to_string());
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
        members.push(Member {
            entity,
            path,
            session,
            uri,
            source,
            class,
            outputs,
            bindings,
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
    all_wires.sort_by(|a, b| {
        let path = |entity| {
            world
                .get::<UsdPrimPath>(entity)
                .map(|path| path.path.as_str())
        };
        path(a.end_element)
            .cmp(&path(b.end_element))
            .then_with(|| a.end_connector.cmp(&b.end_connector))
            .then_with(|| path(a.start_element).cmp(&path(b.start_element)))
            .then_with(|| a.start_connector.cmp(&b.start_connector))
    });
    let mut wires: Vec<_> = all_wires
        .iter()
        .filter(|wire| {
            input_names
                .get(&wire.end_element)
                .is_some_and(|names| names.contains(&wire.end_connector))
        })
        .cloned()
        .collect();
    wires.sort_by(|a, b| {
        let source_path = |entity| {
            world
                .get::<UsdPrimPath>(entity)
                .map(|path| path.path.as_str())
        };
        indices[&a.end_element]
            .cmp(&indices[&b.end_element])
            .then_with(|| a.end_connector.cmp(&b.end_connector))
            .then_with(|| source_path(a.start_element).cmp(&source_path(b.start_element)))
            .then_with(|| a.start_connector.cmp(&b.start_connector))
    });
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
        if !wire.scale.is_finite() || !wire.offset.is_finite() {
            return Err("coupled initialization affine transform is not finite".into());
        }
        sums.entry((destination, wire.end_connector))
            .or_default()
            .push(InitialExpression::Affine(Box::new(source), wire.scale, wire.offset).source());
    }
    if !internal {
        return Ok(None);
    }
    let mut probes = Vec::new();
    for ((index, name), terms) in sums {
        members[index]
            .bindings
            .insert(name.clone(), terms.join(" + "));
        probes.push((
            members[index].entity,
            name.clone(),
            format!("p{index}.{name}"),
        ));
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
    if world
        .get_resource::<lunco_core::SceneTransitionCoordinator>()
        .and_then(lunco_core::SceneTransitionCoordinator::lifecycle_generation)
        != Some(certificate.plan.generation)
        || wire_snapshot(world) != certificate.plan.wires
    {
        return Err("scene or wiring changed during coupled initialization".into());
    }
    for member in &certificate.plan.members {
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
            return Ok(false);
        };
        if let Some(error) = &model.last_error {
            return Err(error.clone());
        }
        if !model.is_compiled {
            return Ok(false);
        }
        if model.current_time != 0.0 {
            return Err("participant advanced before initialization certification".into());
        }
    }
    let mut query = world.query::<&SimConnection>();
    let mut wires: Vec<_> = query.iter(world).cloned().collect();
    wires.sort_by(|a, b| {
        let path = |entity| {
            world
                .get::<UsdPrimPath>(entity)
                .map(|path| path.path.as_str())
        };
        path(a.end_element)
            .cmp(&path(b.end_element))
            .then_with(|| a.end_connector.cmp(&b.end_connector))
            .then_with(|| path(a.start_element).cmp(&path(b.start_element)))
            .then_with(|| a.start_connector.cmp(&b.start_connector))
    });
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
    for member in &plan.members {
        if let Some(mut model) = world.get_mut::<ModelicaModel>(member.entity) {
            model.last_error = Some(error.clone());
            model.is_compiling = false;
            model.resume_after_compile = false;
            model.paused = true;
        }
        if let Some(mut component) = world.get_mut::<SimComponent>(member.entity) {
            component.status = SimStatus::Error(error.clone());
        }
        if let Ok(mut entity) = world.get_entity_mut(member.entity) {
            entity.remove::<PendingModelicaSource>();
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
            }
            Err(error) => fail(world, &certificate.plan, error),
        }
        return;
    }
    let job = world.resource_mut::<CoupledInitialization>().job.take();
    if let Some(job) = job {
        let model = world.get::<ModelicaModel>(job.actor);
        let outcome = model.and_then(|model| {
            model
                .last_error
                .clone()
                .map(Err)
                .or_else(|| model.is_compiled.then(|| Ok(())))
        });
        let Some(outcome) = outcome else {
            world.resource_mut::<CoupledInitialization>().job = Some(job);
            return;
        };
        // Source identity, parameters, current physical samples and every wire
        // are compared before accepting a worker result, including its order.
        match capture(world) {
            Ok(Some(current)) if current == job.plan => {
                let values = world
                    .get::<ModelicaVariables>(job.actor)
                    .map(|vars| vars.values.clone());
                let outcome = outcome.and_then(|()| {
                    let values = values.ok_or("initialization result has no observation")?;
                    let mut inputs: BTreeMap<Entity, Vec<(String, f64)>> = BTreeMap::new();
                    for (index, (entity, name, _)) in job.plan.probes.iter().enumerate() {
                        let value = values
                            .get(&format!("initial_{index}"))
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
                            .insert(CoupledInitializationPending);
                        if let Some(values) = inputs.get(&member.entity) {
                            world
                                .entity_mut(member.entity)
                                .insert(ConsistentInitialInputs(values.clone()));
                        }
                        if let Some(mut model) = world.get_mut::<ModelicaModel>(member.entity) {
                            model.paused = true;
                        }
                    }
                    world.resource_mut::<CoupledInitialization>().certificate = Some(Certificate {
                        plan: job.plan.clone(),
                        values: inputs,
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
        world.despawn(job.actor);
        return;
    }
    let plan = match capture(world) {
        Ok(Some(plan)) => plan,
        Ok(None) => return,
        Err(error) => {
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
    let actor = world
        .spawn(ModelicaModel {
            model_name: "LunCoCoupledInitialization".into(),
            source_uri: "generated://coupled-initialization.mo".into(),
            session_id: 1,
            paused: true,
            is_compiling: true,
            ..default()
        })
        .id();
    let sent = world
        .resource::<ModelicaChannels>()
        .tx
        .send(ModelicaCommand::Compile {
            entity: actor,
            session_id: 1,
            execution_domain: lunco_modelica_runtime::ModelicaExecutionDomain::Scene,
            model_name: "LunCoCoupledInitialization".into(),
            source,
            realtime_safe: false,
            doc_uri: "generated://coupled-initialization.mo".into(),
            extra_sources: sources.into_iter().collect(),
            parameter_overrides: Vec::new(),
            initial_inputs: Vec::new(),
        });
    if sent.is_err() {
        world.despawn(actor);
        fail(world, &plan, "Modelica worker channel closed".into());
        return;
    }
    info!(
        "[usd-cosim] solving coupled initialization for {} participant(s)",
        plan.members.len()
    );
    world.resource_mut::<CoupledInitialization>().job = Some(Job { actor, plan });
}

/// Startup preparation is dormant after all USD Modelica participants settle.
pub(super) fn needed(
    pending: Query<(), With<PendingModelicaSource>>,
    models: Query<&ModelicaModel, (With<UsdSourcedCosim>, With<GeneratedModelicaSource>)>,
    owner: Option<Res<CoupledInitialization>>,
) -> bool {
    !pending.is_empty()
        || owner.is_some_and(|owner| owner.job.is_some() || owner.certificate.is_some())
        || models
            .iter()
            .any(|model| !model.is_compiled && model.last_error.is_none())
}

/// Retire the transient worker participant with its owning scene.
pub(super) fn retire(world: &mut World) {
    if let Some(mut owner) = world.remove_resource::<CoupledInitialization>() {
        if let Some(job) = owner.job.take() {
            world.despawn(job.actor);
        }
    }
}
