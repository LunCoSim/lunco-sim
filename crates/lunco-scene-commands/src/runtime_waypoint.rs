//! Runtime-only waypoint commands and collision-backed arrival state.
//!
//! Authored waypoints remain ordinary USD prims. A spawned rover has no owning
//! document, so its patrol is a live [`AutopilotBehaviorSpec`] and its marker is
//! an instance of the same USD waypoint asset. This module owns that one runtime
//! path so the editor and the headless production runner exercise identical code.

use avian3d::prelude::{CollisionStart, Sensor};
use bevy::math::DVec3;
use bevy::prelude::*;
use lunco_api::queries::{ApiQueryProvider, ApiQueryRegistry};
use lunco_api::registry::ApiEntityRegistry;
use lunco_api::schema::ApiResponse;
use lunco_autopilot::usd_tree::{append_waypoint_leaf, BehaviorXml, ReachedWaypoints};
use lunco_core::{
    on_command, register_commands, Command, ControlBinding, GlobalEntityId, InputPorts, Severity,
    TelemetryEvent, TelemetryValue, TriggerZone,
};
use lunco_usd::document::WAYPOINT_MARKER_ASSET;
use lunco_usd_bevy::UsdPrimPath;

use crate::catalog::{spawn_usd_entry, SpawnAnchor, SpawnCatalog, SpawnSource};

/// A runtime waypoint appended to a spawned vessel's patrol.
///
/// The target is an [`Entity`] deliberately: the API/Rhai command dispatcher
/// resolves the stable `GlobalEntityId` supplied by callers before this handler
/// runs, just like the other scene commands.
#[Command]
pub struct AddRuntimeWaypoint {
    /// Spawned rover root receiving the waypoint.
    pub target: Entity,
    /// Grid-absolute waypoint origin. The shared marker's overlap Sensor is
    /// positioned relative to this point.
    pub position: Vec3,
}

/// Binds a spawned marker to the route index that created it.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct RuntimeWaypointBinding {
    pub vessel: Entity,
    pub index: usize,
}

/// Asset and scene-root handles needed to instantiate the shared USD marker.
#[derive(bevy::ecs::system::SystemParam)]
pub struct RuntimeWaypointSpawner<'w, 's> {
    pub asset_server: Res<'w, AssetServer>,
    pub catalog: Res<'w, SpawnCatalog>,
    pub q_scene_root: Query<'w, 's, Entity, With<lunco_usd_sim::cosim::UsdSceneRoot>>,
}

/// The synthetic key used by the live route and by the arrival set.
pub fn runtime_waypoint_key(index: usize) -> String {
    format!("/__runtime_waypoint_{index}")
}

/// Extend a runtime-only patrol without changing an authored behaviour shape.
pub fn append_runtime_patrol(
    current: Option<&lunco_autopilot::AutopilotBehaviorSpec>,
    current_xml: Option<&str>,
    point: [f32; 3],
) -> Result<lunco_autopilot::BehaviorSpec, String> {
    if let Some(xml) = current_xml {
        append_waypoint_leaf(Some(xml), "/__runtime_waypoint__")
            .map_err(|e| format!("runtime rover mission cannot accept a waypoint: {e}"))?;
    }

    match current.map(|spec| &spec.0) {
        Some(lunco_autopilot::BehaviorSpec::Patrol {
            waypoints,
            speed,
            radius,
            dwell,
        }) => {
            let mut waypoints = waypoints.clone();
            waypoints.push(lunco_autopilot::PatrolWaypoint::at(point.map(f64::from)));
            Ok(lunco_autopilot::BehaviorSpec::Patrol {
                waypoints,
                speed: *speed,
                radius: *radius,
                dwell: *dwell,
            })
        }
        Some(_) => Err(
            "runtime rover has a non-patrol behaviour; edit that behaviour explicitly before adding a waypoint"
                .to_string(),
        ),
        None => Ok(lunco_autopilot::BehaviorSpec::Patrol {
            waypoints: vec![lunco_autopilot::PatrolWaypoint::at(point.map(f64::from))],
            speed: 0.6,
            radius: 3.0,
            dwell: 0.0,
        }),
    }
}

fn spawn_runtime_waypoint_marker(
    spawner: &RuntimeWaypointSpawner,
    commands: &mut Commands,
    vessel: Entity,
    index: usize,
    position: DVec3,
) -> Result<(), String> {
    let entry = spawner
        .catalog
        .entries
        .iter()
        .find(|entry| {
            matches!(
                &entry.source,
                SpawnSource::UsdFile(path)
                    if lunco_assets::engine_asset_rel(path)
                        == lunco_assets::engine_asset_rel(WAYPOINT_MARKER_ASSET)
            )
        })
        .ok_or_else(|| {
            format!(
                "runtime waypoint marker asset is not in the spawn catalog: {WAYPOINT_MARKER_ASSET}"
            )
        })?;
    let scene_root = spawner
        .q_scene_root
        .iter()
        .next()
        .ok_or_else(|| "runtime waypoint marker has no mounted scene root".to_string())?;
    let marker = spawn_usd_entry(
        commands,
        &spawner.asset_server,
        entry,
        position.as_vec3(),
        Quat::IDENTITY,
        SpawnAnchor::scene_root(scene_root),
    );
    commands
        .entity(marker.root_entity)
        .insert(RuntimeWaypointBinding { vessel, index });
    Ok(())
}

#[on_command(AddRuntimeWaypoint)]
fn on_add_runtime_waypoint(
    trigger: On<AddRuntimeWaypoint>,
    spawner: RuntimeWaypointSpawner,
    q_prim: Query<&UsdPrimPath>,
    q_xml: Query<&BehaviorXml>,
    q_inputs: Query<(), With<InputPorts>>,
    q_specs: Query<&lunco_autopilot::AutopilotBehaviorSpec>,
    q_autopilots: Query<&lunco_autopilot::Autopilot>,
    mut commands: Commands,
) {
    let cmd = trigger.event();
    if !cmd.position.is_finite() {
        warn!("[waypoint] runtime waypoint rejected: non-finite position");
        return;
    }
    if q_prim.get(cmd.target).is_err() || q_inputs.get(cmd.target).is_err() {
        warn!(
            "[waypoint] runtime waypoint target {:?} is not a rover command surface",
            cmd.target
        );
        return;
    }

    let spec = match append_runtime_patrol(
        q_specs.get(cmd.target).ok(),
        q_xml.get(cmd.target).ok().map(|xml| xml.0.as_str()),
        [cmd.position.x, cmd.position.y, cmd.position.z],
    ) {
        Ok(spec) => spec,
        Err(err) => {
            warn!("[waypoint] {err}");
            return;
        }
    };
    let waypoint_index = match &spec {
        lunco_autopilot::BehaviorSpec::Patrol { waypoints, .. } => waypoints.len() - 1,
        _ => 0,
    };
    let Ok(spec_json) = serde_json::to_string(&spec) else {
        warn!("[waypoint] runtime patrol could not be serialized");
        return;
    };
    if let Err(err) = spawn_runtime_waypoint_marker(
        &spawner,
        &mut commands,
        cmd.target,
        waypoint_index,
        cmd.position.as_dvec3(),
    ) {
        warn!("[waypoint] runtime marker could not be spawned: {err}");
        return;
    }

    if q_autopilots.iter().any(|ap| ap.vessel == cmd.target) {
        commands.trigger(lunco_autopilot::SetAutopilotBehavior {
            vessel: cmd.target,
            spec_json,
        });
    } else {
        commands.trigger(lunco_autopilot::EngageAutopilot {
            vessel: cmd.target,
            index: 0,
            throttle: 0.0,
            spec_json,
        });
    }
    info!(
        "[waypoint] runtime-only rover {:?} received patrol waypoint {}",
        cmd.target, waypoint_index
    );
}

/// Structured readback used by the production scene test and API clients.
pub struct RuntimeWaypointStatusProvider;

impl ApiQueryProvider for RuntimeWaypointStatusProvider {
    fn name(&self) -> &'static str {
        "RuntimeWaypointStatus"
    }

    fn execute(&self, world: &mut World, params: &serde_json::Value) -> ApiResponse {
        let Some(raw) = params.get("vessel").and_then(serde_json::Value::as_u64) else {
            return ApiResponse::error(
                lunco_api::schema::ApiErrorCode::DeserializationError,
                "RuntimeWaypointStatus requires a numeric vessel GID",
            );
        };
        let gid = GlobalEntityId::from_raw(raw);
        let Some(vessel) = world
            .get_resource::<ApiEntityRegistry>()
            .and_then(|registry| registry.resolve(&gid))
        else {
            return ApiResponse::error(
                lunco_api::schema::ApiErrorCode::EntityNotFound,
                "runtime waypoint vessel is not present",
            );
        };

        let marker_count = {
            let mut q = world.query::<&RuntimeWaypointBinding>();
            q.iter(world)
                .filter(|binding| binding.vessel == vessel)
                .count()
        };
        let mut reached: Vec<String> = world
            .get::<ReachedWaypoints>(vessel)
            .map(|set| set.0.iter().cloned().collect())
            .unwrap_or_default();
        reached.sort();
        ApiResponse::ok(serde_json::json!({
            "vessel": raw,
            "marker_count": marker_count,
            "reached": reached,
            "reached_count": reached.len(),
        }))
    }
}

/// Read Avian's shared Sensor overlap and turn it into live route state.
pub fn mark_reached_waypoints_on_enter(
    mut starts: MessageReader<CollisionStart>,
    q_zones: Query<(&TriggerZone, &UsdPrimPath), With<Sensor>>,
    q_runtime_bindings: Query<&RuntimeWaypointBinding>,
    // A physics body can expose generic input/output ports without being a
    // controllable vessel (the authored ground body does exactly that).  The
    // route boundary is the authored control binding; requiring it prevents a
    // waypoint trigger's normal contact with the ground from becoming a rover
    // arrival event.
    q_vessel_roots: Query<(), (With<UsdPrimPath>, With<ControlBinding>)>,
    q_parents: Query<&ChildOf>,
    q_vessels: Query<(Entity, Option<&BehaviorXml>)>,
    q_reached: Query<&ReachedWaypoints>,
    q_gids: Query<&GlobalEntityId>,
    world_time: Option<Res<lunco_time::WorldTime>>,
    mut commands: Commands,
) {
    enum Arrival {
        Authored { marker_path: String, vessel: Entity },
        Runtime { vessel: Entity, index: usize },
    }
    let mut arrivals = Vec::new();

    for ev in starts.read() {
        for (zone_ent, other_ent, other_body) in [
            (ev.collider1, ev.collider2, ev.body2),
            (ev.collider2, ev.collider1, ev.body1),
        ] {
            let Ok((zone, zone_prim)) = q_zones.get(zone_ent) else {
                continue;
            };
            if zone.0 != "waypoint" {
                continue;
            }
            let runtime_binding = {
                let mut curr = zone_ent;
                let mut binding = None;
                for _ in 0..16 {
                    if let Ok(value) = q_runtime_bindings.get(curr) {
                        binding = Some(*value);
                        break;
                    }
                    let Ok(parent) = q_parents.get(curr) else {
                        break;
                    };
                    curr = parent.parent();
                }
                binding
            };
            let Some((marker_path, _)) = zone_prim.path.rsplit_once('/') else {
                continue;
            };
            let mut resolved = false;
            for candidate in [other_ent, other_body.unwrap_or(other_ent)] {
                let mut curr = candidate;
                for _ in 0..16 {
                    if q_vessel_roots.get(curr).is_ok() {
                        if let Some(binding) = runtime_binding {
                            if binding.vessel == curr {
                                arrivals.push(Arrival::Runtime {
                                    vessel: binding.vessel,
                                    index: binding.index,
                                });
                            }
                        } else {
                            arrivals.push(Arrival::Authored {
                                marker_path: marker_path.to_string(),
                                vessel: curr,
                            });
                        }
                        resolved = true;
                        break;
                    }
                    match q_parents.get(curr) {
                        Ok(parent) => curr = parent.parent(),
                        Err(_) => break,
                    }
                }
                if resolved {
                    break;
                }
            }
        }
    }

    for arrival in arrivals {
        let (vessel, key) = match arrival {
            Arrival::Runtime { vessel, index } => (vessel, runtime_waypoint_key(index)),
            Arrival::Authored {
                marker_path,
                vessel,
            } => {
                let Ok((_, Some(xml))) = q_vessels.get(vessel) else {
                    continue;
                };
                let Ok(value) = lunco_autopilot::btcpp_xml::xml_to_value(&xml.0) else {
                    continue;
                };
                let mut targets = Vec::new();
                collect_targets(&value, &mut targets);
                if !targets.iter().any(|target| {
                    target == &marker_path
                        || target.ends_with(&marker_path)
                        || marker_path.ends_with(target)
                }) {
                    continue;
                }
                (vessel, marker_path)
            }
        };
        if q_reached
            .get(vessel)
            .map(|reached| {
                reached
                    .0
                    .iter()
                    .any(|path| path == &key || path.ends_with(&key) || key.ends_with(path))
            })
            .unwrap_or(false)
        {
            continue;
        }
        info!("[waypoint] reached {key} (sensor enter)");
        let mut set = q_reached
            .get(vessel)
            .map(|reached| reached.0.clone())
            .unwrap_or_default();
        set.insert(key.clone());
        commands.entity(vessel).insert(ReachedWaypoints(set));
        commands.trigger(TelemetryEvent {
            name: "waypoint.reached".to_string(),
            source: q_gids.get(vessel).map(GlobalEntityId::get).unwrap_or(0),
            severity: Severity::Info,
            data: TelemetryValue::String(key),
            timestamp: world_time.as_ref().map(|time| time.sim_secs).unwrap_or(0.0),
        });
    }
}

fn collect_targets(value: &serde_json::Value, out: &mut Vec<String>) {
    match value {
        serde_json::Value::Object(map) => {
            if map.get("kind").and_then(serde_json::Value::as_str) == Some("drive_to") {
                if let Some(target) = map.get("target").and_then(serde_json::Value::as_str) {
                    out.push(target.to_string());
                }
            }
            for child in map.values() {
                collect_targets(child, out);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                collect_targets(item, out);
            }
        }
        _ => {}
    }
}

register_commands!(on_add_runtime_waypoint);

pub fn register(app: &mut App) {
    register_all_commands(app);
    app.init_resource::<ApiQueryRegistry>();
    app.world_mut()
        .resource_mut::<ApiQueryRegistry>()
        .register(RuntimeWaypointStatusProvider);
    app.add_systems(
        FixedPostUpdate,
        mark_reached_waypoints_on_enter.after(avian3d::prelude::PhysicsSystems::Writeback),
    );
}

#[cfg(test)]
mod tests {
    use super::append_runtime_patrol;

    #[test]
    fn runtime_patrol_starts_with_one_waypoint() {
        let spec = append_runtime_patrol(None, None, [1.0, 2.0, 3.0]).unwrap();
        match spec {
            lunco_autopilot::BehaviorSpec::Patrol { waypoints, .. } => {
                assert_eq!(waypoints.len(), 1);
                assert_eq!(waypoints[0].pos, [1.0, 2.0, 3.0]);
            }
            _ => panic!("runtime waypoint must create a patrol"),
        }
    }
}
