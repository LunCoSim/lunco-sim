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
use lunco_core::coords::GridPos;
use lunco_core::{Severity, TelemetryEvent, TelemetryValue, TriggerZone};

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
pub(crate) struct RaycastProvider;
impl ApiQueryProvider for RaycastProvider {
    fn name(&self) -> &'static str {
        "Raycast"
    }

    fn execute(&self, world: &mut World, params: &serde_json::Value) -> ApiResponse {
        let (Some(origin), Some(dir_v)) = (parse_vec3(params, "origin"), parse_vec3(params, "dir"))
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
        cast_ray_response(world, GridPos(origin), dir, max)
    }
}

/// `GroundHeight` — collider/terrain height under a world `(x, z)` by casting
/// straight down. params: `{ x:f64, z:f64, from?:f64, max?:f64 }` ·
/// returns: `{ hit, height, entity:gid|null, normal:[x,y,z] }`.
pub(crate) struct GroundHeightProvider;
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
        let origin = GridPos(DVec3::new(x, from, z));
        match cast_ray_response(world, origin, Dir3::NEG_Y, max) {
            ApiResponse::Ok { data: Some(d), .. } => {
                let hit = d
                    .get("hit")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false);
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
///
/// `origin` is a [`GridPos`] in the explicit
/// [`lunco_core::ActivePhysicsFrame`] — the same f64 frame used by Avian,
/// `TerrainHeight`, spawn commands, position ports, and site-local navigation.
/// API callers never provide a camera-relative render point: that frame moves
/// whenever BigSpace recentres and is therefore not stable user or simulation
/// state. The returned point stays in the same active frame.
fn cast_ray_response(world: &mut World, origin: GridPos, dir: Dir3, max: f64) -> ApiResponse {
    let mut state: SystemState<(lunco_physics::GridSpatialQuery, Res<ApiEntityRegistry>)> =
        SystemState::new(world);
    let (spatial, registry) = state
        .get(world)
        .expect("SpatialQuery + registry always validate");
    match spatial.cast_ray_grid(origin, dir, max, true, &SpatialQueryFilter::default()) {
        Some(hit) => {
            let point = origin.0 + (*dir).as_dvec3() * hit.distance;
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
pub(crate) fn register_physics_queries(app: &mut App) {
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
        // `0` is the wire sentinel used by contact_pair for an unregistered
        // side. Never turn an invalid/reserved zero identity into a named
        // arrival that a scenario cannot resolve back to an entity.
        .filter(|id| *id != 0)
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

/// The named trigger-zone events a contact produces: for each side that is a
/// *named* sensor (a trigger volume carrying a `Name`), an
/// `("<verb>:<zone>", entrant_gid)` pair, where the entrant is the OTHER side
/// and has a registered stable id. The entrant must be a moving rigid body;
/// static terrain, scenery, and unidentified contacts are not arrivals. Empty
/// when neither side is a named zone — those contacts still surface as the
/// generic `COLLISION_START`/`COLLISION_END`.
///
/// `zone_name` maps a collider entity to its zone name (the sensor's `Name`);
/// abstracted as a closure so this stays a pure, unit-testable function.
fn zone_events(
    verb: &str,
    c1: Entity,
    b1: Option<Entity>,
    c2: Entity,
    b2: Option<Entity>,
    zone_name: impl Fn(Entity) -> Option<String>,
    reg: &ApiEntityRegistry,
    is_moving_body: impl Fn(Option<Entity>) -> bool,
) -> Vec<(String, i64, u64)> {
    // (event name, entrant gid → payload, ZONE gid → event source)
    let mut out = Vec::new();
    if is_moving_body(b2) && contact_gid(reg, c2, b2).is_some() {
        if let Some(name) = zone_name(c1) {
            let entrant = contact_gid(reg, c2, b2).unwrap_or(0) as i64;
            let zone = contact_gid(reg, c1, b1).unwrap_or(0);
            out.push((format!("{verb}:{name}"), entrant, zone));
        }
    }
    if is_moving_body(b1) && contact_gid(reg, c1, b1).is_some() {
        if let Some(name) = zone_name(c2) {
            let entrant = contact_gid(reg, c1, b1).unwrap_or(0) as i64;
            let zone = contact_gid(reg, c2, b2).unwrap_or(0);
            out.push((format!("{verb}:{name}"), entrant, zone));
        }
    }
    out
}

/// Bridge `CollisionStart`/`CollisionEnd` → `TelemetryEvent`. Every contact fires
/// the generic `COLLISION_START`/`COLLISION_END` (payload `"gidA:gidB"`). A
/// contact involving a *named* sensor additionally fires `enter:<zone>` /
/// `exit:<zone>` whose payload is the entrant's gid — so a scenario reacts to a
/// trigger volume by name (`if evt.name == "enter:pad_2"`) without reverse-mapping
/// gids. Triggered events reach the script inbox observer and are delivered to
/// `on_event` next tick (the frame-delayed actor model `emit` uses).
fn bridge_collision_events(
    mut starts: MessageReader<CollisionStart>,
    mut ends: MessageReader<CollisionEnd>,
    registry: Res<ApiEntityRegistry>,
    zones: Query<(Option<&TriggerZone>, &Name), With<Sensor>>,
    bodies: Query<&RigidBody>,
    world: Option<Res<lunco_time::WorldTime>>,
    mut commands: Commands,
) {
    let timestamp = world.map(|w| w.epoch_jd).unwrap_or(0.0);
    let fire = |name: String, data: TelemetryValue, source: u64, commands: &mut Commands| {
        commands.trigger(TelemetryEvent {
            name,
            source,
            severity: Severity::Info,
            data,
            timestamp,
        });
    };
    // Prefer the explicit `TriggerZone` name (short, stable), falling back to the
    // entity's `Name` (its USD path) for an unnamed sensor.
    let zone_name = |e: Entity| {
        zones.get(e).ok().map(|(tz, name)| {
            tz.map(|z| z.0.clone())
                .unwrap_or_else(|| name.as_str().to_string())
        })
    };
    let is_moving_body = |body: Option<Entity>| {
        body.is_some_and(|entity| {
            matches!(
                bodies.get(entity),
                Ok(RigidBody::Dynamic | RigidBody::Kinematic)
            )
        })
    };

    for ev in starts.read() {
        if let Some(p) = contact_pair(&registry, ev.collider1, ev.body1, ev.collider2, ev.body2) {
            fire(
                "COLLISION_START".to_string(),
                TelemetryValue::String(p),
                0,
                &mut commands,
            );
        }
        for (name, entrant, zone) in zone_events(
            "enter",
            ev.collider1,
            ev.body1,
            ev.collider2,
            ev.body2,
            zone_name,
            &registry,
            is_moving_body,
        ) {
            fire(name, TelemetryValue::I64(entrant), zone, &mut commands);
        }
    }
    for ev in ends.read() {
        if let Some(p) = contact_pair(&registry, ev.collider1, ev.body1, ev.collider2, ev.body2) {
            fire(
                "COLLISION_END".to_string(),
                TelemetryValue::String(p),
                0,
                &mut commands,
            );
        }
        for (name, entrant, zone) in zone_events(
            "exit",
            ev.collider1,
            ev.body1,
            ev.collider2,
            ev.body2,
            zone_name,
            &registry,
            is_moving_body,
        ) {
            fire(name, TelemetryValue::I64(entrant), zone, &mut commands);
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
        commands.entity(e).try_insert(CollisionEventsEnabled);
    }
}

/// Wire the collision → telemetry bridge + sensor auto-arming. Kept separate
/// from [`register_physics_queries`] (the read-side provider seam) since this is
/// the event-side bridge.
pub(crate) fn register_collision_event_bridge(app: &mut App) {
    app.add_systems(Update, arm_sensor_collision_events);
    // MUST read avian's `CollisionStart`/`CollisionEnd` AFTER the physics step
    // that produces them. Avian runs its `PhysicsSchedule` in `FixedPostUpdate`;
    // reading in `FixedUpdate` (as before) ran a whole schedule-phase EARLIER, so
    // zone enter/exit lagged a tick and a fast pass-through could be missed.
    // Ordering after `PhysicsSystems::Writeback` (the last physics set) reads the
    // events generated this same tick.
    app.add_systems(
        FixedPostUpdate,
        bridge_collision_events.after(PhysicsSystems::Writeback),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::time::TimeUpdateStrategy;
    use core::time::Duration;
    use lunco_core::GlobalEntityId;

    fn response_data(response: ApiResponse) -> serde_json::Value {
        match response {
            ApiResponse::Ok {
                data: Some(data), ..
            } => data,
            other => panic!("expected query data, got {other:?}"),
        }
    }

    #[test]
    fn ground_height_uses_the_active_physics_frame_at_an_elevated_site() {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            AssetPlugin::default(),
            TransformPlugin,
            PhysicsPlugins::default(),
        ));
        app.init_asset::<Mesh>()
            .init_resource::<ApiEntityRegistry>()
            .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_micros(
                15_625,
            )));
        app.finish();
        app.cleanup();

        let frame = app.world_mut().spawn_empty().id();
        app.world_mut()
            .insert_resource(lunco_core::ActivePhysicsFrame(frame));
        app.world_mut().spawn((
            RigidBody::Static,
            Collider::cuboid(20.0, 1.0, 20.0),
            Transform::from_xyz(0.0, -1_900.0, 0.0),
        ));
        for _ in 0..4 {
            app.update();
        }

        let data = response_data(GroundHeightProvider.execute(
            app.world_mut(),
            &serde_json::json!({"x": 0.0, "z": 0.0, "from": -1_800.0, "max": 200.0}),
        ));
        assert_eq!(data["hit"], true);
        let height = data["height"].as_f64().expect("height");
        assert!(
            (height - -1_899.5).abs() < 1.0e-6,
            "active-frame height must remain at the authored -1899.5 m ground top, got {height}"
        );
    }

    #[test]
    fn contact_pair_resolves_collider_through_registered_body() {
        let mut world = World::new();
        let rover = world.spawn_empty().id();
        let wheel = world.spawn_empty().id(); // a collider child, not itself registered
        let obstacle = world.spawn_empty().id();
        let unreg = world.spawn_empty().id();

        let mut reg = ApiEntityRegistry::default();
        reg.assign(rover, GlobalEntityId::from_raw(42));
        reg.assign(obstacle, GlobalEntityId::from_raw(7));

        // collider1 is the unregistered wheel, but its authored physics owner is
        // the registered rover → the pair is identified as 42:7.
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

    #[test]
    fn zone_events_name_by_sensor_and_carry_the_entrant() {
        let mut world = World::new();
        let rover = world.spawn_empty().id();
        let pad = world.spawn_empty().id(); // the named sensor (trigger volume)
        let plain = world.spawn_empty().id(); // a sensor with no name

        let mut reg = ApiEntityRegistry::default();
        reg.assign(rover, GlobalEntityId::from_raw(42));
        reg.assign(pad, GlobalEntityId::from_raw(7));

        // Only `pad` is a named zone.
        let zone_name = |e: Entity| (e == pad).then(|| "pad_2".to_string());

        // rover (side 1) enters pad (side 2, the named sensor) → "enter:pad_2"
        // with the entrant (rover, 42) as payload, and pad (7) as zone.
        assert_eq!(
            zone_events("enter", rover, None, pad, None, zone_name, &reg, |_| true),
            vec![("enter:pad_2".to_string(), 42, 7)]
        );
        // Order-independent: the named side can be side 1.
        assert_eq!(
            zone_events("exit", pad, None, rover, None, zone_name, &reg, |_| true),
            vec![("exit:pad_2".to_string(), 42, 7)]
        );
        // A contact with no named sensor produces no zone events.
        assert!(
            zone_events("enter", rover, None, plain, None, zone_name, &reg, |_| {
                true
            })
            .is_empty()
        );
        // An unregistered entrant cannot be a script-addressable arrival.
        let ghost = world.spawn_empty().id();
        assert!(zone_events("enter", pad, None, ghost, None, zone_name, &reg, |_| true).is_empty());

        // A static terrain/body contact is not a vehicle arrival, even when the
        // zone side is named. The bridge supplies this predicate from Avian's
        // authoritative rigid-body state.
        assert!(zone_events(
            "enter",
            pad,
            None,
            ghost,
            Some(ghost),
            zone_name,
            &reg,
            |body| body == Some(rover),
        )
        .is_empty());
    }
}
