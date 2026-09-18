//! Optional JSON/API query providers for the USD co-simulation runtime.
//
//! The co-simulation crate owns projection, wiring, and lifecycle state. This
//! package owns only the transport-facing read projections so API serialization
//! changes do not rebuild that runtime.

use avian3d::schedule::PhysicsTime;
use bevy::ecs::query::QueryState;
use bevy::prelude::*;
use lunco_cosim_core::{
    BindingEpochDirty, BoundConnection, ConnectionBinding, SimComponent, SimConnection, SimStatus,
    UsdSourcedCosim,
};
use lunco_embodiment_core::roles::{Embodiment, LocalEmbodiment};
use lunco_modelica_runtime::ModelicaModel;
use lunco_render::SceneCamera;
use lunco_usd_bevy_camera::camera_mount::MountedCamera;
use lunco_usd_bevy_scene::{UsdPrimPath, UsdSceneAwaitingStage};
use lunco_usd_bevy_stage::UsdInstanceRoot;
use lunco_usd_sim_core::PendingDifferential;
use lunco_usd_sim_cosim::{modelica_models_terminal, BindingEpochWait};

mod broken_connections;

/// Registers all co-simulation API query providers when an API registry exists.
pub struct UsdSimCosimApiPlugin;

impl Plugin for UsdSimCosimApiPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<lunco_embodiment_core::roles::EmbodimentCorePlugin>() {
            app.add_plugins(lunco_embodiment_core::roles::EmbodimentCorePlugin);
        }
        app.add_systems(
            Startup,
            |registry: Option<ResMut<lunco_api::ApiQueryRegistry>>| {
                if let Some(mut registry) = registry {
                    registry.register(ListPortsProvider);
                    registry.register(GetPortProvider);
                    registry.register(CausalTraceProvider);
                    registry.register(CosimStatusProvider);
                    registry.register(BindingStatusProvider);
                    registry.register(SceneCameraAuditProvider);
                    registry.register(broken_connections::BrokenConnectionsProvider);
                }
            },
        );
    }
}

// ── Uniform port reads (ListPorts / GetPort) ────────────────────────────────
//
// The single API surface over the cosim **port table** (`lunco_cosim::ports`).
// Every exposed value — Modelica var, Avian force/state, joint angle, env
// signal — is read/written/listed here uniformly, regardless of which backend
// owns it. These are the canonical port verbs; they are not aliases of
// `CosimStatus` (which stays as richer per-entity cosim introspection).

/// Map a [`lunco_port_core::ports::PortDirection`] to a stable wire string.
fn port_dir_str(d: lunco_port_core::ports::PortDirection) -> &'static str {
    match d {
        lunco_port_core::ports::PortDirection::In => "in",
        lunco_port_core::ports::PortDirection::Out => "out",
        lunco_port_core::ports::PortDirection::InOut => "inout",
    }
}

fn port_to_json(p: &lunco_port_core::ports::PortInfo) -> serde_json::Value {
    let range = match (p.metadata.min, p.metadata.max) {
        (Some(min), Some(max)) => serde_json::json!({ "min": min, "max": max }),
        (Some(min), None) => serde_json::json!({ "min": min }),
        (None, Some(max)) => serde_json::json!({ "max": max }),
        (None, None) => serde_json::Value::Null,
    };
    serde_json::json!({
        "name": p.name,
        "direction": port_dir_str(p.direction),
        "value": p.value,
        "metadata": {
            "type": p.metadata.value_type,
            "unit": p.metadata.unit,
            "range": range,
            "source": p.metadata.source,
            "authority": p.metadata.authority,
            "writable": p.metadata.writable,
        },
    })
}

/// Resolve the optional `api_id` / `entity` field of a params object to an ECS
/// `Entity` via the `ApiEntityRegistry`. Returns `None` when absent (the
/// caller lists all) or when the id doesn't resolve.
fn resolve_param_entity(world: &World, params: &serde_json::Value) -> Option<Entity> {
    let raw = params
        .get("api_id")
        .or_else(|| params.get("entity"))
        .and_then(|v| v.as_u64())?;
    let reg = world.get_resource::<lunco_api::ApiEntityRegistry>()?;
    reg.resolve(&lunco_core::GlobalEntityId::from_raw(raw))
}

/// `ListPorts` — enumerate exposed ports. With `{"api_id": N}`, lists that
/// entity's ports; without, lists every registered entity that has any port.
///
/// `curl … {"type":"ExecuteCommand","command":"ListPorts","params":{"api_id":12345}}`
pub struct ListPortsProvider;

impl lunco_api::ApiQueryProvider for ListPortsProvider {
    fn name(&self) -> &'static str {
        "ListPorts"
    }
    fn execute(&self, world: &World, params: &serde_json::Value) -> lunco_api::ApiResponse {
        let ports_reg = world
            .resource::<lunco_port_core::ports::PortRegistry>()
            .clone();
        // Single-entity form.
        if let Some(e) = resolve_param_entity(world, params) {
            let ports: Vec<_> = ports_reg
                .entity_port_infos(world, e)
                .iter()
                .map(port_to_json)
                .collect();
            return lunco_api::ApiResponse::ok(serde_json::json!({ "ports": ports }));
        }
        // All-entities form: snapshot the registry list first (owned), then
        // read ports — avoids holding the resource borrow across `entity_ports`.
        let Some(reg) = world.get_resource::<lunco_api::ApiEntityRegistry>() else {
            return lunco_api::ApiResponse::ok(serde_json::json!({ "entities": [] }));
        };
        let entries = reg.entities();
        let mut rows = Vec::new();
        for (api_id, e) in entries {
            let ports = ports_reg.entity_port_infos(world, e);
            if ports.is_empty() {
                continue;
            }
            rows.push(serde_json::json!({
                "api_id": api_id.get(),
                "name": world.get::<Name>(e).map(|n| n.as_str().to_string()).unwrap_or_default(),
                "ports": ports.iter().map(port_to_json).collect::<Vec<_>>(),
            }));
        }
        lunco_api::ApiResponse::ok(serde_json::json!({ "entities": rows }))
    }
}

/// `GetPort` — read one port value.
///
/// `curl … {"type":"ExecuteCommand","command":"GetPort","params":{"api_id":N,"name":"yaw"}}`
pub struct GetPortProvider;

impl lunco_api::ApiQueryProvider for GetPortProvider {
    fn name(&self) -> &'static str {
        "GetPort"
    }
    fn execute(&self, world: &World, params: &serde_json::Value) -> lunco_api::ApiResponse {
        let Some(e) = resolve_param_entity(world, params) else {
            return lunco_api::ApiResponse::error(
                lunco_api::ApiErrorCode::EntityNotFound,
                "GetPort requires a resolvable `api_id`",
            );
        };
        let Some(name) = params.get("name").and_then(|v| v.as_str()) else {
            return lunco_api::ApiResponse::error(
                lunco_api::ApiErrorCode::DeserializationError,
                "GetPort requires a `name`",
            );
        };
        let ports_reg = world
            .resource::<lunco_port_core::ports::PortRegistry>()
            .clone();
        match ports_reg.read_port(world, e, name) {
            Some(value) => {
                lunco_api::ApiResponse::ok(serde_json::json!({ "name": name, "value": value }))
            }
            None => lunco_api::ApiResponse::error(
                lunco_api::ApiErrorCode::DeserializationError,
                format!("no port `{}` on entity", name),
            ),
        }
    }
}

fn causal_binding_status(binding: Option<&ConnectionBinding>) -> &'static str {
    match binding {
        None => "unobserved",
        Some(ConnectionBinding::Pending) => "pending",
        Some(ConnectionBinding::Bound) => "bound",
        Some(ConnectionBinding::Failed) => "failed",
    }
}

fn causal_port_owner_json(owner: &lunco_port_core::ports::PortOwnerInfo) -> serde_json::Value {
    serde_json::json!({
        "precedence": owner.precedence,
        "direction": port_dir_str(owner.direction),
        "metadata": {
            "type": owner.metadata.value_type,
            "unit": owner.metadata.unit,
            "range": {
                "min": owner.metadata.min,
                "max": owner.metadata.max,
            },
            "source": owner.metadata.source,
            "authority": owner.metadata.authority,
            "writable": owner.metadata.writable,
        },
    })
}

fn causal_endpoint_json(world: &World, entity: Entity) -> serde_json::Value {
    serde_json::json!({
        "entity": entity.to_bits(),
        "api_id": world.get::<lunco_core::GlobalEntityId>(entity).map(|gid| gid.get()),
        "name": world.get::<Name>(entity).map(|name| name.as_str()),
        "usd_path": world.get::<UsdPrimPath>(entity).map(|path| path.path.as_str()),
    })
}

/// Read-only causal explanation for one semantic action.
///
/// The semantic edge is the bounded record keyed by `target` and
/// `correlation_id`. Every downstream field is a live composition of the
/// existing binding, port, USD/Avian admission, and signal registries. A
/// missing field is therefore a real incomplete path, not a synthetic success.
///
/// `target` accepts the stable API id. `correlation_id` selects one recorded
/// edge; when omitted, the newest edge for the target is selected for a useful
/// operator default. The response includes the selected `correlation_id` so a
/// caller can repeat the exact inspection.
pub struct CausalTraceProvider;

impl lunco_api::ApiQueryProvider for CausalTraceProvider {
    fn name(&self) -> &'static str {
        "CausalTrace"
    }

    fn execute(&self, world: &World, params: &serde_json::Value) -> lunco_api::ApiResponse {
        let Some(raw_target) = params
            .get("target")
            .or_else(|| params.get("api_id"))
            .and_then(serde_json::Value::as_u64)
        else {
            return lunco_api::ApiResponse::error(
                lunco_api::ApiErrorCode::DeserializationError,
                "CausalTrace requires a numeric `target` API id",
            );
        };
        let target_gid = lunco_core::GlobalEntityId::from_raw(raw_target);
        let Some(trace) = world.get_resource::<lunco_control_core::CausalTrace>() else {
            return lunco_api::ApiResponse::error(
                lunco_api::ApiErrorCode::InternalError,
                "CausalTrace ledger is unavailable",
            );
        };
        let record = match params
            .get("correlation_id")
            .and_then(serde_json::Value::as_u64)
        {
            Some(correlation_id) => trace.find(target_gid, correlation_id),
            None => trace.latest_for_target(target_gid),
        };
        let Some(record) = record else {
            return lunco_api::ApiResponse::error(
                lunco_api::ApiErrorCode::EntityNotFound,
                format!(
                    "no semantic edge trace for target {}{}",
                    raw_target,
                    params
                        .get("correlation_id")
                        .and_then(serde_json::Value::as_u64)
                        .map(|id| format!(" and correlation_id {}", id))
                        .unwrap_or_default()
                ),
            );
        };

        let target = world
            .get::<lunco_core::GlobalEntityId>(record.target)
            .is_some()
            .then_some(record.target);
        let target_json = target
            .map(|entity| causal_endpoint_json(world, entity))
            .unwrap_or_else(|| {
                serde_json::json!({
                    "api_id": raw_target,
                    "lifecycle": "despawned",
                })
            });

        let (binding_entries, port_surface) = if let Some(entity) = target {
            let binding = world.get::<lunco_control_core::ControlBinding>(entity);
            let binding_entries = binding
                .map(|binding| {
                    binding
                        .binds
                        .iter()
                        .filter(|(intent, _, _)| *intent == record.intent)
                        .map(|(_, port, scale)| serde_json::json!({ "port": port, "scale": scale }))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let ports = world
                .resource::<lunco_port_core::ports::PortRegistry>()
                .clone();
            let owners = ports.entity_port_owners(world, entity);
            let mut names = binding_entries
                .iter()
                .filter_map(|entry| entry.get("port").and_then(serde_json::Value::as_str))
                .map(str::to_owned)
                .collect::<Vec<_>>();
            names.sort();
            names.dedup();
            let port_surface = names
                .into_iter()
                .map(|name| {
                    let candidates = owners
                        .iter()
                        .filter(|owner| owner.name == name)
                        .collect::<Vec<_>>();
                    let selected = candidates
                        .iter()
                        .find(|owner| {
                            matches!(
                                owner.direction,
                                lunco_port_core::ports::PortDirection::In
                                    | lunco_port_core::ports::PortDirection::InOut
                            )
                        })
                        .map(|owner| causal_port_owner_json(owner));
                    serde_json::json!({
                        "name": name,
                        "current_input": ports.read_input_port(world, entity, &name),
                        "selected_owner": selected,
                        "owners": candidates
                            .into_iter()
                            .map(causal_port_owner_json)
                            .collect::<Vec<_>>(),
                    })
                })
                .collect::<Vec<_>>();
            (binding_entries, port_surface)
        } else {
            (Vec::new(), Vec::new())
        };

        let connection_edges = if let Some(entity) = target {
            let Some(mut query) = QueryState::<
                (
                    Entity,
                    &SimConnection,
                    Option<&ConnectionBinding>,
                    Has<BoundConnection>,
                ),
                With<SimConnection>,
            >::try_new(world) else {
                return lunco_api::ApiResponse::error(
                    lunco_api::ApiErrorCode::InternalError,
                    "CausalTrace: connection query is unavailable",
                );
            };
            query
                .iter(world)
                .filter(|(_, connection, _, _)| {
                    connection.start_element == entity || connection.end_element == entity
                })
                .map(|(edge, connection, binding, bound)| {
                    serde_json::json!({
                        "edge": edge.to_bits(),
                        "source": causal_endpoint_json(world, connection.start_element),
                        "source_port": connection.start_connector,
                        "source_is_input": connection.start_is_input,
                        "sink": causal_endpoint_json(world, connection.end_element),
                        "sink_port": connection.end_connector,
                        "scale": connection.scale,
                        "offset": connection.offset,
                        "binding": causal_binding_status(binding),
                        "bound": bound,
                    })
                })
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };

        let joint_admission = if let Some(entity) = target {
            let Some(mut query) = QueryState::<(
                Entity,
                Option<&UsdPrimPath>,
                Option<&lunco_physics::PhysicsJointLink>,
                Option<&lunco_usd_avian_contracts::PendingJointAdmission>,
                Has<avian3d::prelude::RevoluteJoint>,
                Has<avian3d::prelude::PrismaticJoint>,
                Has<avian3d::prelude::FixedJoint>,
                Has<avian3d::prelude::SphericalJoint>,
                Has<avian3d::prelude::DistanceJoint>,
            )>::try_new(world) else {
                return lunco_api::ApiResponse::error(
                    lunco_api::ApiErrorCode::InternalError,
                    "CausalTrace: joint admission query is unavailable",
                );
            };
            query
                .iter(world)
                .filter_map(
                    |(
                        joint,
                        path,
                        link,
                        pending,
                        revolute,
                        prismatic,
                        fixed,
                        spherical,
                        distance,
                    )| {
                        let link = link?;
                        if link.body0 != entity && link.body1 != entity {
                            return None;
                        }
                        let native_type = revolute
                            .then_some("revolute")
                            .or_else(|| prismatic.then_some("prismatic"))
                            .or_else(|| fixed.then_some("fixed"))
                            .or_else(|| spherical.then_some("spherical"))
                            .or_else(|| distance.then_some("distance"));
                        Some(serde_json::json!({
                            "joint": joint.to_bits(),
                            "path": path.map(|path| path.path.as_str()),
                            "body0": link.body0.to_bits(),
                            "body1": link.body1.to_bits(),
                            "state": if pending.is_some() {
                                "pending"
                            } else if native_type.is_some() {
                                "admitted"
                            } else {
                                "not_admitted"
                            },
                            "native_type": native_type,
                        }))
                    },
                )
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };

        let measured_channels = world
            .get_resource::<lunco_signal::SignalRegistry>()
            .map(|signals| {
                signals
                    .iter_signals()
                    .filter(|(signal, _)| {
                        signal.entity == record.target
                            || signals.global_owner(signal) == Some(target_gid)
                    })
                    .filter_map(|(signal, _)| {
                        let history = signals.scalar_history(signal)?;
                        let latest = history.samples.back()?;
                        let meta = signals.meta(signal);
                        Some(serde_json::json!({
                            "channel": signal.path,
                            "owner_entity": signal.entity.to_bits(),
                            "active": signals.is_active(signal),
                            "latest": {
                                "time": latest.time,
                                "value": latest.value,
                            },
                            "metadata": meta,
                        }))
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        lunco_api::ApiResponse::ok(serde_json::json!({
            "target": target_json,
            "correlation_id": record.correlation_id,
            "intent": record.intent.canonical_name(),
            "edge": record.kind.as_str(),
            "control_binding": {
                "matched_intent": !binding_entries.is_empty(),
                "ports": binding_entries,
            },
            "port_surface": port_surface,
            "connection_edges": connection_edges,
            "joint_admission": joint_admission,
            "measured_channels": measured_channels,
        }))
    }
}

/// API query provider: `curl … {"type":"ExecuteCommand","command":"CosimStatus","params":{}}`
/// returns one row per USD-driven cosim entity with position, model
/// state, and propagated cosim values. The response also includes the
/// authoritative synchronization projection so a rate/worker diagnosis can
/// distinguish a causal barrier from an unrelated model still running on its
/// worker. Lets you probe the running binary without polling logs.
pub struct CosimStatusProvider;

impl lunco_api::ApiQueryProvider for CosimStatusProvider {
    fn name(&self) -> &'static str {
        "CosimStatus"
    }
    fn execute(&self, world: &World, _params: &serde_json::Value) -> lunco_api::ApiResponse {
        let Some(mut q) = QueryState::<
            (
                &Name,
                &Transform,
                Option<&SimComponent>,
                Option<&ModelicaModel>,
                Option<&avian3d::prelude::LinearVelocity>,
            ),
            With<UsdSourcedCosim>,
        >::try_new(world) else {
            return lunco_api::ApiResponse::error(
                lunco_api::ApiErrorCode::InternalError,
                "CosimStatus: ECS query is unavailable",
            );
        };

        let entities: Vec<serde_json::Value> = q
            .iter(world)
            .map(|(name, tf, comp, model, lv)| {
                // Full input/output maps so any cosim signal is readable
                // (the solar tracker's `yaw`/`tracking_error`, the balloon's
                // `buoyancy`, …) — not just a hardcoded set. This is the
                // general "read cosim world state" surface.
                let outputs = comp
                    .map(|c| {
                        c.outputs
                            .iter()
                            .map(|(k, v)| (k.clone(), serde_json::json!(v)))
                            .collect::<serde_json::Map<_, _>>()
                    })
                    .unwrap_or_default();
                let inputs = comp
                    .map(|c| {
                        c.inputs
                            .iter()
                            .map(|(k, v)| (k.clone(), serde_json::json!(v)))
                            .collect::<serde_json::Map<_, _>>()
                    })
                    .unwrap_or_default();
                serde_json::json!({
                    "name": name.as_str(),
                    "y": tf.translation.y,
                    "yaw": tf.rotation.to_euler(EulerRot::YXZ).0,
                    "vy": lv.map(|v| v.0.y).unwrap_or(0.0),
                    "has_simcomponent": comp.is_some(),
                    "model": comp.map(|c| c.model_name.clone()).unwrap_or_default(),
                    "status": comp.map(|c| match &c.status {
                        SimStatus::Idle => "Idle".to_string(),
                        SimStatus::Compiling => "Compiling".to_string(),
                        SimStatus::Running => "Running".to_string(),
                        SimStatus::Paused => "Paused".to_string(),
                        SimStatus::Error(reason) => format!("Error: {reason}"),
                    }).unwrap_or_else(|| "Unbound".to_string()),
                    "modelica_var_count": model.map(|m| m.variables.len()).unwrap_or(0),
                    "modelica_paused": model.map(|m| m.paused).unwrap_or(false),
                    "modelica_current_time": model.map(|m| m.current_time).unwrap_or(0.0),
                    "modelica_target_time": model.map(|m| m.target_time).unwrap_or(0.0),
                    "modelica_communication_period_secs": model.and_then(|m| {
                        m.communication_period_secs.is_finite().then_some(m.communication_period_secs)
                    }),
                    "modelica_schedule_error": model.and_then(|m| {
                        m.validated_communication_period_secs().err()
                    }),
                    "modelica_next_communication_time": model
                        .map(|m| m.next_communication_time)
                        .unwrap_or(0.0),
                    "modelica_is_stepping": model.is_some_and(|m| m.is_stepping),
                    // The Modelica worker's durable failure verdict is the
                    // reason readiness may be holding the world. Surface it
                    // here beside timing/ports so live API diagnosis does not
                    // require access to the process log.
                    "modelica_error": model.and_then(|m| m.last_error.clone()),
                    "outputs": outputs,
                    "inputs": inputs,
                })
            })
            .collect();
        let barrier = world
            .get_resource::<lunco_core_runtime::SimulationBarrier>()
            .copied()
            .unwrap_or_default();
        let (topology_ready, causal_participant_count) = world
            .get_resource::<lunco_core_runtime::SimulationBarrierParticipants>()
            .map(|participants| (participants.topology_ready, participants.entities.len()))
            .unwrap_or((false, 0));
        let causal_participants = world
            .get_resource::<lunco_core_runtime::SimulationBarrierParticipants>()
            .map(|participants| {
                participants
                    .entities
                    .iter()
                    .map(|entity| {
                        serde_json::json!({
                            "entity": entity.to_bits(),
                            "name": world.get::<Name>(*entity).map(Name::as_str),
                            "usd_path": world
                                .get::<UsdPrimPath>(*entity)
                                .map(|path| path.path.as_str()),
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let Some(mut causal_sinks) =
            QueryState::<(), With<lunco_port_core::CausalStateSink>>::try_new(world)
        else {
            return lunco_api::ApiResponse::error(
                lunco_api::ApiErrorCode::InternalError,
                "CosimStatus: causal sink query is unavailable",
            );
        };
        let causal_sink_count = causal_sinks.iter(world).count();
        lunco_api::ApiResponse::ok(serde_json::json!({
            "entities": entities,
            "synchronization": {
                "barrier_held": barrier.held,
                "active_participants": barrier.active_participants,
                "shared_clock_participants": barrier.shared_clock_participants,
                "worst_lag_secs": barrier.worst_lag_secs,
                "worst_entity": barrier.worst_entity.map(Entity::to_bits),
                "topology_ready": topology_ready,
                "causal_participant_count": causal_participant_count,
                "causal_participants": causal_participants,
                "causal_sink_count": causal_sink_count,
            }
        }))
    }
}

/// API query provider for the native binding transaction. `CosimStatus` only
/// covers solver participants; this query exposes the other admission gates
/// that can legitimately keep the world ticket open (deferred USD stages,
/// joints, wheels, and differentials).
pub struct BindingStatusProvider;

impl lunco_api::ApiQueryProvider for BindingStatusProvider {
    fn name(&self) -> &'static str {
        "BindingStatus"
    }

    fn execute(&self, world: &World, _params: &serde_json::Value) -> lunco_api::ApiResponse {
        let Some(mut awaiting_query) =
            QueryState::<&UsdPrimPath, With<UsdSceneAwaitingStage>>::try_new(world)
        else {
            return lunco_api::ApiResponse::error(
                lunco_api::ApiErrorCode::InternalError,
                "BindingStatus: awaiting-stage query is unavailable",
            );
        };
        let awaiting = awaiting_query
            .iter(world)
            .map(|path| path.path.clone())
            .collect::<Vec<_>>();
        let Some(mut pending_joints_query) = QueryState::<
            (
                Entity,
                &UsdPrimPath,
                &lunco_usd_avian_contracts::PendingUsdJoint,
                Option<&lunco_core::Provenance>,
                Option<&lunco_core::GlobalEntityId>,
                Has<UsdInstanceRoot>,
            ),
            With<lunco_usd_avian_contracts::PendingUsdJoint>,
        >::try_new(world) else {
            return lunco_api::ApiResponse::error(
                lunco_api::ApiErrorCode::InternalError,
                "BindingStatus: pending-joint query is unavailable",
            );
        };
        let pending_joints = pending_joints_query
            .iter(world)
            .map(|(entity, path, joint, provenance, gid, is_instance_root)| {
                serde_json::json!({
                    "entity": entity.to_bits(),
                    "path": path.path,
                    "stage": format!("{:?}", path.stage_handle),
                    "joint_type": joint.joint_type,
                    "body0": joint.body0_path,
                    "body1": joint.body1_path,
                    "provenance": provenance.map(|value| format!("{value:?}")),
                    "gid": gid.map(|value| value.get()),
                    "instance_root": is_instance_root,
                })
            })
            .collect::<Vec<_>>();
        let Some(mut pending_differentials_query) =
            QueryState::<&UsdPrimPath, With<PendingDifferential>>::try_new(world)
        else {
            return lunco_api::ApiResponse::error(
                lunco_api::ApiErrorCode::InternalError,
                "BindingStatus: pending-differential query is unavailable",
            );
        };
        let pending_differentials = pending_differentials_query
            .iter(world)
            .map(|path| path.path.clone())
            .collect::<Vec<_>>();

        let pending_body_paths = pending_joints
            .iter()
            .filter_map(|joint| {
                let object = joint.as_object()?;
                Some([
                    object.get("body0")?.as_str()?.to_string(),
                    object.get("body1")?.as_str()?.to_string(),
                ])
            })
            .flatten()
            .collect::<std::collections::BTreeSet<_>>();
        let Some(mut bodies_query) = QueryState::<(
            Entity,
            &UsdPrimPath,
            Option<&avian3d::prelude::RigidBody>,
            Option<&avian3d::prelude::Position>,
            Option<&avian3d::prelude::RigidBodyDisabled>,
            Option<&lunco_usd_avian_core::BridgeShadow>,
            Option<&lunco_core::Provenance>,
            Option<&lunco_core::GlobalEntityId>,
            Has<UsdInstanceRoot>,
        )>::try_new(world) else {
            return lunco_api::ApiResponse::error(
                lunco_api::ApiErrorCode::InternalError,
                "BindingStatus: body query is unavailable",
            );
        };
        let bodies = bodies_query
            .iter(world)
            .filter(|(_, path, _, _, _, _, _, _, _)| pending_body_paths.contains(&path.path))
            .map(
                |(
                    entity,
                    path,
                    body,
                    position,
                    disabled,
                    shadow,
                    provenance,
                    gid,
                    is_instance_root,
                )| {
                    serde_json::json!({
                        "entity": entity.to_bits(),
                        "path": path.path,
                        "stage": format!("{:?}", path.stage_handle),
                        "rigid_body": body.map(|body| format!("{body:?}")),
                        "has_position": position.is_some(),
                        "disabled": disabled.is_some(),
                        "shadow_seeded": shadow.map(|shadow| shadow.is_seeded()),
                        "provenance": provenance.map(|value| format!("{value:?}")),
                        "gid": gid.map(|value| value.get()),
                        "instance_root": is_instance_root,
                    })
                },
            )
            .collect::<Vec<_>>();

        let mut non_terminal_models = Vec::new();
        let Some(mut models) = QueryState::<
            (&Name, Option<&ModelicaModel>, Option<&SimComponent>),
            With<UsdSourcedCosim>,
        >::try_new(world) else {
            return lunco_api::ApiResponse::error(
                lunco_api::ApiErrorCode::InternalError,
                "BindingStatus: model query is unavailable",
            );
        };
        for (name, model, component) in models.iter(world) {
            if !modelica_models_terminal(std::iter::once((model, component))) {
                non_terminal_models.push(serde_json::json!({
                    "name": name.as_str(),
                    "has_model": model.is_some(),
                    "has_simcomponent": component.is_some(),
                    "status": component.map(|c| format!("{:?}", c.status)),
                }));
            }
        }

        let Some(mut connections_count_query) =
            QueryState::<(), With<SimConnection>>::try_new(world)
        else {
            return lunco_api::ApiResponse::error(
                lunco_api::ApiErrorCode::InternalError,
                "BindingStatus: connection query is unavailable",
            );
        };
        let connection_count = connections_count_query.iter(world).count();
        // `connection_count` alone cannot distinguish a correctly derived wire
        // from a wire that is still pending or bound to the wrong endpoint. Keep
        // the complete, generic edge inventory behind the same read-only API so
        // callers can diagnose authored USD topology without log scraping or
        // campaign-specific probes.
        let Some(mut connection_specs_query) = QueryState::<
            (
                Entity,
                &SimConnection,
                Option<&ConnectionBinding>,
                Has<BoundConnection>,
            ),
            With<SimConnection>,
        >::try_new(world) else {
            return lunco_api::ApiResponse::error(
                lunco_api::ApiErrorCode::InternalError,
                "BindingStatus: connection detail query is unavailable",
            );
        };
        let connection_specs = connection_specs_query
            .iter(world)
            .map(|(edge, spec, binding, bound)| {
                let endpoint = |entity: Entity| {
                    serde_json::json!({
                        "entity": entity.to_bits(),
                        "name": world.get::<Name>(entity).map(|name| name.as_str()),
                        "usd_path": world
                            .get::<UsdPrimPath>(entity)
                            .map(|path| path.path.as_str()),
                    })
                };
                serde_json::json!({
                    "edge": edge.to_bits(),
                    "source": endpoint(spec.start_element),
                    "source_port": spec.start_connector,
                    "source_is_input": spec.start_is_input,
                    "sink": endpoint(spec.end_element),
                    "sink_port": spec.end_connector,
                    "scale": spec.scale,
                    "offset": spec.offset,
                    "binding": binding.map(|value| format!("{value:?}")),
                    "bound": bound,
                })
            })
            .collect::<Vec<_>>();
        let Some(mut pending_revolute_query) = QueryState::<
            (),
            With<lunco_usd_avian_joints::PendingJoint<avian3d::prelude::RevoluteJoint>>,
        >::try_new(world) else {
            return lunco_api::ApiResponse::error(
                lunco_api::ApiErrorCode::InternalError,
                "BindingStatus: pending revolute-joint query is unavailable",
            );
        };
        let Some(mut pending_prismatic_query) = QueryState::<
            (),
            With<lunco_usd_avian_joints::PendingJoint<avian3d::prelude::PrismaticJoint>>,
        >::try_new(world) else {
            return lunco_api::ApiResponse::error(
                lunco_api::ApiErrorCode::InternalError,
                "BindingStatus: pending prismatic-joint query is unavailable",
            );
        };
        let Some(mut pending_fixed_query) = QueryState::<
            (),
            With<lunco_usd_avian_joints::PendingJoint<avian3d::prelude::FixedJoint>>,
        >::try_new(world) else {
            return lunco_api::ApiResponse::error(
                lunco_api::ApiErrorCode::InternalError,
                "BindingStatus: pending fixed-joint query is unavailable",
            );
        };
        let pending_avian_joints = serde_json::json!({
            "revolute": pending_revolute_query.iter(world).count(),
            "prismatic": pending_prismatic_query.iter(world).count(),
            "fixed": pending_fixed_query.iter(world).count(),
        });
        let Some(mut pending_admission_query) = QueryState::<
            (
                Entity,
                &lunco_usd_avian_contracts::PendingJointAdmission,
                Option<&UsdPrimPath>,
            ),
            With<lunco_usd_avian_contracts::PendingJointAdmission>,
        >::try_new(world) else {
            return lunco_api::ApiResponse::error(
                lunco_api::ApiErrorCode::InternalError,
                "BindingStatus: joint-admission query is unavailable",
            );
        };
        let pending_admission_details = pending_admission_query
            .iter(world)
            .map(|(joint_entity, pending, path)| {
                let body = |entity: Entity| {
                    let rb = world.get::<avian3d::prelude::RigidBody>(entity);
                    let body_path = world.get::<UsdPrimPath>(entity);
                    serde_json::json!({
                        "entity": entity.to_bits(),
                        "path": body_path.map(|value| value.path.clone()),
                        "rigid_body": rb.map(|value| format!("{value:?}")),
                        "has_solver_body": world
                            .get::<avian3d::dynamics::solver::solver_body::SolverBody>(entity)
                            .is_some(),
                        "has_island_node": world
                            .get::<avian3d::dynamics::solver::islands::BodyIslandNode>(entity)
                            .is_some(),
                        "disabled": world
                            .get::<avian3d::prelude::RigidBodyDisabled>(entity)
                            .is_some(),
                        "ecs_disabled": world
                            .get::<bevy::ecs::entity_disabling::Disabled>(entity)
                            .is_some(),
                    })
                };
                serde_json::json!({
                    "joint_entity": joint_entity.to_bits(),
                    "joint_path": path.map(|value| value.path.clone()),
                    "body0": body(pending.body0),
                    "body1": body(pending.body1),
                })
            })
            .collect::<Vec<_>>();
        let Some(mut admitted_revolute_query) =
            QueryState::<(), With<avian3d::prelude::RevoluteJoint>>::try_new(world)
        else {
            return lunco_api::ApiResponse::error(
                lunco_api::ApiErrorCode::InternalError,
                "BindingStatus: admitted revolute-joint query is unavailable",
            );
        };
        let Some(mut admitted_prismatic_query) =
            QueryState::<(), With<avian3d::prelude::PrismaticJoint>>::try_new(world)
        else {
            return lunco_api::ApiResponse::error(
                lunco_api::ApiErrorCode::InternalError,
                "BindingStatus: admitted prismatic-joint query is unavailable",
            );
        };
        let Some(mut admitted_fixed_query) =
            QueryState::<(), With<avian3d::prelude::FixedJoint>>::try_new(world)
        else {
            return lunco_api::ApiResponse::error(
                lunco_api::ApiErrorCode::InternalError,
                "BindingStatus: admitted fixed-joint query is unavailable",
            );
        };
        let admitted_avian_joints = serde_json::json!({
            "revolute": admitted_revolute_query.iter(world).count(),
            "prismatic": admitted_prismatic_query.iter(world).count(),
            "fixed": admitted_fixed_query.iter(world).count(),
        });
        let wait_open = world.get_resource::<BindingEpochWait>().is_some();
        let dirty = world
            .get_resource::<BindingEpochDirty>()
            .is_some_and(|dirty| dirty.0);
        let physics_paused = world
            .get_resource::<Time<avian3d::prelude::Physics>>()
            .is_some_and(|time| time.is_paused());
        let physics_holds = world
            .get_resource::<lunco_physics::PhysicsHolds>()
            .map(|holds| holds.reasons().collect::<Vec<_>>())
            .unwrap_or_default();
        let runtime_fault = world
            .get_resource::<lunco_core::RuntimeFaults>()
            .and_then(|faults| faults.first.as_ref())
            .map(|fault| {
                serde_json::json!({
                    "kind": fault.kind,
                    "entity": fault.entity.map(Entity::to_bits),
                    "subject": fault.subject,
                    "detail": fault.detail,
                })
            });

        lunco_api::ApiResponse::ok(serde_json::json!({
            "wait_open": wait_open,
            "dirty": dirty,
            "connection_count": connection_count,
            "connections": connection_specs,
            "pending_avian_joints": pending_avian_joints,
            "admitted_avian_joints": admitted_avian_joints,
            "physics_paused": physics_paused,
            "physics_holds": physics_holds,
            "runtime_fault": runtime_fault,
            "pending_admission_details": pending_admission_details,
            "awaiting": awaiting,
            "pending_joints": pending_joints,
            "bodies": bodies,
            "pending_differentials": pending_differentials,
            "non_terminal_models": non_terminal_models,
        }))
    }
}

/// Read-only camera/avatar inventory for diagnosing scene lifecycle failures.
///
/// [`lunco_api::ApiEntityRegistry`] is intentionally keyed by stable USD identity,
/// so two accidental ECS projections of the same prim collapse to one row in
/// `ListEntities`. This query instead enumerates the live ECS candidates by their
/// transient entity id and reports the roles that can make a duplicate visible.
/// It intentionally includes every `SceneCamera` intent, whether or not the
/// render host has attached Bevy's `Camera` component; unrelated render-only
/// cameras are not viewport candidates and are excluded.
pub struct SceneCameraAuditProvider;

impl lunco_api::ApiQueryProvider for SceneCameraAuditProvider {
    fn name(&self) -> &'static str {
        "SceneCameraAudit"
    }

    fn execute(&self, world: &World, _params: &serde_json::Value) -> lunco_api::ApiResponse {
        let Some(mut query) = QueryState::<
            (
                Entity,
                Option<&Name>,
                Option<&UsdPrimPath>,
                Option<&bevy::camera::Camera>,
                Option<&bevy::camera::RenderTarget>,
                Has<SceneCamera>,
                Has<MountedCamera>,
                Has<Embodiment>,
                Has<LocalEmbodiment>,
            ),
            With<SceneCamera>,
        >::try_new(world) else {
            return lunco_api::ApiResponse::error(
                lunco_api::ApiErrorCode::InternalError,
                "SceneCameraAudit: ECS query is unavailable",
            );
        };
        let mut candidates: Vec<_> = query
            .iter(world)
            .map(
                |(entity, name, prim, camera, target, scene_camera, mounted, avatar, local)| {
                    serde_json::json!({
                        "entity": entity.to_bits(),
                        "name": name.map(|n| n.as_str()).unwrap_or_default(),
                        "usd_path": prim.map(|p| p.path.as_str()),
                        "stage": prim.map(|p| format!("{:?}", p.stage_handle.id())),
                        "scene_camera": scene_camera,
                        "mounted_camera": mounted,
                        "avatar": avatar,
                        "local_avatar": local,
                        // Headless production runs intentionally omit Bevy's
                        // render `Camera` component, but the render-free
                        // `SceneCamera` intent is still the authoritative
                        // viewport candidate. Keep the audit useful in both
                        // worlds instead of making headless diagnostics report
                        // an empty candidate set.
                        "render_camera": camera.is_some(),
                        "camera_active": camera.is_some_and(|camera| camera.is_active),
                        "camera_output_mode": camera.map(|camera| match camera.output_mode {
                            bevy::camera::CameraOutputMode::Write { .. } => "write",
                            bevy::camera::CameraOutputMode::Skip => "skip",
                        }),
                        "render_target": target.map(|target| match target {
                            bevy::camera::RenderTarget::Window(_) => "window",
                            bevy::camera::RenderTarget::Image(_) => "image",
                            bevy::camera::RenderTarget::TextureView(_) => "texture_view",
                            bevy::camera::RenderTarget::None { .. } => "none",
                        }),
                        "physical_target_size": camera
                            .and_then(|camera| camera.physical_target_size())
                            .map(|size| [size.x, size.y]),
                        "physical_viewport_size": camera
                            .and_then(|camera| camera.physical_viewport_size())
                            .map(|size| [size.x, size.y]),
                    })
                },
            )
            .collect();
        candidates.sort_by_key(|row| row["entity"].as_u64());
        lunco_api::ApiResponse::ok(serde_json::json!({
            "count": candidates.len(),
            "candidates": candidates,
        }))
    }
}
