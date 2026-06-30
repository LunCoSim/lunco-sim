//! Physics-backed spatial-query providers exposed to the API / scripting.
//!
//! Registered into [`ApiQueryRegistry`] so any caller — HTTP API, MCP, or a
//! rhai scenario via `query("Raycast", #{...})` — can sense geometry WITHOUT
//! taking an avian dependency. avian stays contained behind this string-keyed
//! provider seam (the read-side twin of the `#[Command]` bus).

use avian3d::prelude::*;
use bevy::ecs::system::SystemState;
use bevy::math::DVec3;
use bevy::prelude::*;
use lunco_api::queries::{ApiQueryProvider, ApiQueryRegistry};
use lunco_api::registry::ApiEntityRegistry;
use lunco_api::schema::{ApiErrorCode, ApiResponse};
use lunco_core::{Severity, TelemetryEvent, TelemetryValue};

/// Parse a `[x, y, z]` JSON array under `key`.
fn parse_vec3(params: &serde_json::Value, key: &str) -> Option<DVec3> {
    let a = params.get(key)?.as_array()?;
    if a.len() < 3 {
        return None;
    }
    Some(DVec3::new(a[0].as_f64()?, a[1].as_f64()?, a[2].as_f64()?))
}

/// `Raycast` — cast a world-space ray, return the first collider hit.
/// params: `{ origin:[x,y,z], dir:[x,y,z], max?:f64 }` ·
/// returns: `{ hit, entity:gid|null, distance, point:[x,y,z], normal:[x,y,z] }`.
pub struct RaycastProvider;
impl ApiQueryProvider for RaycastProvider {
    fn name(&self) -> &'static str {
        "Raycast"
    }
    fn execute(&self, world: &mut World, params: &serde_json::Value) -> ApiResponse {
        let (Some(origin), Some(dir_v)) =
            (parse_vec3(params, "origin"), parse_vec3(params, "dir"))
        else {
            return ApiResponse::error(
                ApiErrorCode::DeserializationError,
                "Raycast: `origin` and `dir` [x,y,z] required".to_string(),
            );
        };
        let max = params
            .get("max")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(1.0e6);
        let Ok(dir) = Dir3::new(dir_v.as_vec3()) else {
            return ApiResponse::error(
                ApiErrorCode::DeserializationError,
                "Raycast: `dir` must be non-zero".to_string(),
            );
        };
        cast_ray_response(world, origin, dir, max)
    }
}

/// `GroundHeight` — collider/terrain height under a world `(x, z)` by casting
/// straight down. params: `{ x:f64, z:f64, from?:f64, max?:f64 }` ·
/// returns: `{ hit, height, entity:gid|null, normal:[x,y,z] }`.
pub struct GroundHeightProvider;
impl ApiQueryProvider for GroundHeightProvider {
    fn name(&self) -> &'static str {
        "GroundHeight"
    }
    fn execute(&self, world: &mut World, params: &serde_json::Value) -> ApiResponse {
        let (Some(x), Some(z)) = (
            params.get("x").and_then(serde_json::Value::as_f64),
            params.get("z").and_then(serde_json::Value::as_f64),
        ) else {
            return ApiResponse::error(
                ApiErrorCode::DeserializationError,
                "GroundHeight: `x` and `z` required".to_string(),
            );
        };
        let from = params
            .get("from")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(1.0e5);
        let max = params
            .get("max")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(2.0e5);
        let origin = DVec3::new(x, from, z);
        match cast_ray_response(world, origin, Dir3::NEG_Y, max) {
            ApiResponse::Ok {
                data: Some(d), ..
            } => {
                let hit = d.get("hit").and_then(serde_json::Value::as_bool).unwrap_or(false);
                let height = d
                    .get("point")
                    .and_then(|p| p.get(1))
                    .and_then(serde_json::Value::as_f64);
                ApiResponse::ok(serde_json::json!({
                    "hit": hit,
                    "height": height,
                    "entity": d.get("entity").cloned().unwrap_or(serde_json::Value::Null),
                    "normal": d.get("normal").cloned().unwrap_or(serde_json::Value::Null),
                }))
            }
            other => other,
        }
    }
}

/// Shared cast → JSON. Maps the hit collider back to its `GlobalEntityId` (null
/// when the collider has no registered id, e.g. unregistered terrain).
fn cast_ray_response(world: &mut World, origin: DVec3, dir: Dir3, max: f64) -> ApiResponse {
    let mut state: SystemState<(SpatialQuery, Res<ApiEntityRegistry>)> = SystemState::new(world);
    let (spatial, registry) = state.get(world);
    match spatial.cast_ray(origin, dir, max, true, &SpatialQueryFilter::default()) {
        Some(hit) => {
            let point = origin + (*dir).as_dvec3() * hit.distance;
            let entity = registry.api_id_for(hit.entity).map(|g| g.get());
            ApiResponse::ok(serde_json::json!({
                "hit": true,
                "entity": entity,
                "distance": hit.distance,
                "point": [point.x, point.y, point.z],
                "normal": [hit.normal.x, hit.normal.y, hit.normal.z],
            }))
        }
        None => ApiResponse::ok(serde_json::json!({ "hit": false })),
    }
}

/// Register the physics-backed spatial providers. Idempotent re: the registry
/// resource (init-if-absent), so plugin ordering vs. `LunCoApiPlugin` doesn't
/// matter.
pub fn register_physics_queries(app: &mut App) {
    app.init_resource::<ApiQueryRegistry>();
    let mut reg = app.world_mut().resource_mut::<ApiQueryRegistry>();
    reg.register(RaycastProvider);
    reg.register(GroundHeightProvider);
}

// ── Collision / trigger-volume events → the telemetry bus ───────────────────
//
// avian's `CollisionStart` / `CollisionEnd` are the physics-truth for "two
// colliders started/stopped touching". We mirror them onto the engine-wide
// `TelemetryEvent` bus so a rhai scenario's `on_event` hook can REACT to
// contact (entered a trigger zone, hit an obstacle) instead of polling
// `distance()` every tick. Scripting never sees avian — it just receives a
// `COLLISION_START` / `COLLISION_END` telemetry pulse like any other event.
//
// avian only emits these for entities carrying `CollisionEventsEnabled`, so
// there is NO firehose by default: a wheel resting on terrain is silent. Sensor
// (trigger-volume) entities get the marker auto-added below for zero-config
// zone triggers; solid bodies that want collision callbacks must carry it
// explicitly (authored in USD / spawn code).

/// The `GlobalEntityId` (as `u64`) for a contact side: the collider's own id if
/// it's registered, else its rigid body's id. `None` when neither is registered
/// (nothing a script could name).
fn contact_gid(reg: &ApiEntityRegistry, collider: Entity, body: Option<Entity>) -> Option<u64> {
    reg.api_id_for(collider)
        .or_else(|| body.and_then(|b| reg.api_id_for(b)))
        .map(|g| g.get())
}

/// `"a:b"` gid pair for a contact, or `None` if neither side is a registered
/// entity. An unregistered side is reported as `0` (e.g. rover hits unregistered
/// terrain → `"42:0"`), so a script still learns it touched *something*.
fn contact_pair(
    reg: &ApiEntityRegistry,
    c1: Entity,
    b1: Option<Entity>,
    c2: Entity,
    b2: Option<Entity>,
) -> Option<String> {
    let a = contact_gid(reg, c1, b1).unwrap_or(0);
    let b = contact_gid(reg, c2, b2).unwrap_or(0);
    if a == 0 && b == 0 {
        return None;
    }
    Some(format!("{a}:{b}"))
}

/// Bridge `CollisionStart`/`CollisionEnd` → `TelemetryEvent` (`COLLISION_START`
/// / `COLLISION_END`, payload `"gidA:gidB"`). Triggered events reach the script
/// inbox observer and are delivered to `on_event` next tick (the same
/// frame-delayed actor model `emit` uses), so a one-tick latency is expected.
fn bridge_collision_events(
    mut starts: MessageReader<CollisionStart>,
    mut ends: MessageReader<CollisionEnd>,
    registry: Res<ApiEntityRegistry>,
    world: Option<Res<lunco_time::WorldTime>>,
    mut commands: Commands,
) {
    let timestamp = world.map(|w| w.epoch_jd).unwrap_or(0.0);
    let mut fire = |name: &str, pair: String, commands: &mut Commands| {
        commands.trigger(TelemetryEvent {
            name: name.to_string(),
            severity: Severity::Info,
            data: TelemetryValue::String(pair),
            timestamp,
        });
    };
    for ev in starts.read() {
        if let Some(p) = contact_pair(&registry, ev.collider1, ev.body1, ev.collider2, ev.body2) {
            fire("COLLISION_START", p, &mut commands);
        }
    }
    for ev in ends.read() {
        if let Some(p) = contact_pair(&registry, ev.collider1, ev.body1, ev.collider2, ev.body2) {
            fire("COLLISION_END", p, &mut commands);
        }
    }
}

/// Auto-arm collision events on trigger volumes: any `Sensor` without the
/// `CollisionEventsEnabled` marker gets it, so authoring a sensor zone in USD is
/// enough to receive `COLLISION_START`/`COLLISION_END` in a script — no extra
/// component plumbing. The `Without` filter makes this a no-op once armed.
fn arm_sensor_collision_events(
    q: Query<Entity, (With<Sensor>, Without<CollisionEventsEnabled>)>,
    mut commands: Commands,
) {
    for e in &q {
        commands.entity(e).insert(CollisionEventsEnabled);
    }
}

/// Wire the collision → telemetry bridge + sensor auto-arming. Kept separate
/// from [`register_physics_queries`] (the read-side provider seam) since this is
/// the event-side bridge.
pub fn register_collision_event_bridge(app: &mut App) {
    app.add_systems(Update, arm_sensor_collision_events);
    app.add_systems(FixedUpdate, bridge_collision_events);
}

#[cfg(test)]
mod tests {
    use super::*;
    use lunco_core::GlobalEntityId;

    #[test]
    fn contact_pair_uses_body_fallback_and_marks_unregistered_zero() {
        let mut world = World::new();
        let rover = world.spawn_empty().id();
        let wheel = world.spawn_empty().id(); // a collider child, not itself registered
        let obstacle = world.spawn_empty().id();
        let unreg = world.spawn_empty().id();

        let mut reg = ApiEntityRegistry::default();
        reg.assign(rover, GlobalEntityId::from_raw(42));
        reg.assign(obstacle, GlobalEntityId::from_raw(7));

        // collider1 is the unregistered wheel, but its body is the registered
        // rover → resolves to 42 via the body fallback; collider2 = obstacle(7).
        assert_eq!(
            contact_pair(&reg, wheel, Some(rover), obstacle, None),
            Some("42:7".to_string())
        );
        // Registered rover vs. unregistered terrain → "42:0" (still reported).
        assert_eq!(
            contact_pair(&reg, rover, None, unreg, None),
            Some("42:0".to_string())
        );
        // Neither side registered → None (nothing a script could name).
        assert_eq!(contact_pair(&reg, wheel, None, unreg, None), None);
    }
}
