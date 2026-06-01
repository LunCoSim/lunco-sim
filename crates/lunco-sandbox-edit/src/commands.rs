//! Command handlers for sandbox-edit world manipulation.
//!
//! - `SpawnEntity` — spawn from the catalog at a world position.
//! - `MoveEntity` — teleport an entity to an absolute world position.
//!   Mirrors what the gizmo does on drag: swap to Kinematic, update
//!   Transform/Position/LinearVelocity, so joint constraints
//!   propagate the move to coupled bodies. Lets API clients
//!   (MCP tools, automated tests) drive entity motion exactly the
//!   way a human would with the gizmo.

use bevy::prelude::*;
use bevy::math::DVec3;
use avian3d::prelude::{LinearVelocity, RigidBody};
use avian3d::physics_transform::Position;
use big_space::prelude::Grid;
use lunco_core::Command;
use crate::catalog::{SpawnCatalog, spawn_procedural, spawn_usd_entry};

/// Spawn an entity from the catalog at a given world position.
#[Command]
pub struct SpawnEntity {
    /// The grid entity to spawn under.
    pub target: Entity,
    /// The catalog entry ID (e.g. "ball_dynamic", "skid_rover").
    pub entry_id: String,
    /// World-space position (x, y, z).
    pub position: Vec3,
}

/// Observer that handles SpawnEntity commands.
pub fn on_spawn_entity_command(
    trigger: On<SpawnEntity>,
    mut commands: Commands,
    catalog: Res<SpawnCatalog>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    asset_server: Res<AssetServer>,
    q_grids: Query<Entity, With<Grid>>,
) {
    let cmd = trigger.event();

    let entry = match catalog.get(&cmd.entry_id) {
        Some(e) => e,
        None => {
            warn!("SPAWN_ENTITY: unknown entry '{}'", cmd.entry_id);
            return;
        }
    };

    let grid = match q_grids.get(cmd.target) {
        Ok(g) => g,
        Err(_) => {
            warn!("SPAWN_ENTITY: target entity is not a Grid");
            return;
        }
    };

    info!("SPAWN_ENTITY: {} at {:?}", cmd.entry_id, cmd.position);

    match entry.source {
        crate::catalog::SpawnSource::Procedural(_) => {
            spawn_procedural(&mut commands, &mut meshes, &mut materials, entry, cmd.position, grid);
        }
        crate::catalog::SpawnSource::UsdFile(_) => {
            spawn_usd_entry(&mut commands, &asset_server, entry, cmd.position, grid);
        }
    }
}

/// Move an existing entity to an absolute world-space position.
///
/// Programmatic equivalent of grabbing the entity with the gizmo and
/// dragging it. The handler:
/// 1. Switches the body to `RigidBody::Kinematic` (if it has a
///    `RigidBody`) so Avian treats the new pose as authoritative
///    rather than fighting back via integration.
/// 2. Writes `Transform.translation` for renderer + scene-graph.
/// 3. Writes Avian's `Position` for the joint/contact solver.
/// 4. Sets a one-tick `LinearVelocity` consistent with the move so
///    any joint coupled to a dynamic body propagates the motion.
///
/// Designed for automated tests / MCP tool clients that need to
/// drive the world without a mouse. Single-shot — body type stays
/// Kinematic until another command (or a gizmo drag-end) restores it.
#[Command(default)]
pub struct MoveEntity {
    /// API-stable global entity ID (the `api_id` from `ListEntities`).
    /// Resolved to a Bevy `Entity` inside the observer via
    /// `ApiEntityRegistry`. Using `u64` rather than `Entity` here is
    /// deliberate — the API's typed-command resolver only forwards
    /// the entity index, dropping the generation, which makes a
    /// `target: Entity` field lookup fail for any entity whose
    /// generation is non-zero.
    pub entity_id: u64,
    /// Target world-space translation.
    pub translation: Vec3,
}

/// Observer for `MoveEntity`.
pub fn on_move_entity_command(
    trigger: On<MoveEntity>,
    time: Res<Time>,
    registry: Res<lunco_api::registry::ApiEntityRegistry>,
    mut commands: Commands,
    mut q: Query<(
        &mut Transform,
        Option<&mut Position>,
        Option<&mut LinearVelocity>,
    )>,
    q_has_rb: Query<(), With<RigidBody>>,
) {
    let cmd = trigger.event();
    let global_id = lunco_core::GlobalEntityId::from_raw(cmd.entity_id);
    let Some(target) = registry.resolve(&global_id) else {
        warn!("MOVE_ENTITY: no api_id={} in registry", cmd.entity_id);
        return;
    };
    let Ok((mut tf, pos_opt, lin_vel_opt)) = q.get_mut(target) else {
        warn!("MOVE_ENTITY: entity {:?} (api_id={}) has no Transform", target, cmd.entity_id);
        return;
    };

    let prev = tf.translation;
    tf.translation = cmd.translation;

    // Force the body to Kinematic for the duration of the move so
    // Avian treats the new pose as authoritative. RigidBody is an
    // immutable Avian component (no `&mut` access) — `insert`
    // replaces it.
    if q_has_rb.get(target).is_ok() {
        commands.entity(target).insert(RigidBody::Kinematic);
    }

    if let Some(mut pos) = pos_opt {
        pos.0 = DVec3::new(
            cmd.translation.x as f64,
            cmd.translation.y as f64,
            cmd.translation.z as f64,
        );
    }

    // **Joint-propagation pulse**: set `LinearVelocity` to a one-tick
    // velocity equal to (delta / dt). Avian's joint constraint solver
    // operates on velocities — without this, kinematic teleports
    // don't drag joint-coupled dynamic bodies along. Position is
    // still set above so the body lands exactly where requested;
    // the velocity is purely a signal to the solver.
    //
    // The `JustMovedKinematic` marker (below) tells
    // `clear_kinematic_pulse_velocity` to zero the velocity after
    // exactly one physics tick. Without that follow-up, the body
    // would keep drifting at this velocity each tick.
    let dt = time.delta_secs().max(1.0 / 240.0) as f64;
    let delta = cmd.translation - prev;
    if let Some(mut lin_vel) = lin_vel_opt {
        lin_vel.0 = DVec3::new(
            delta.x as f64 / dt,
            delta.y as f64 / dt,
            delta.z as f64 / dt,
        );
    }
    commands.entity(target).insert(JustMovedKinematic);

    info!(
        "MOVE_ENTITY: {:?} → ({:.3}, {:.3}, {:.3})",
        cmd.entity_id, cmd.translation.x, cmd.translation.y, cmd.translation.z
    );
}

/// Marker inserted on a kinematic body that just received a
/// `MoveEntity` (or analogous teleport) with a one-tick velocity
/// pulse. [`clear_kinematic_pulse_velocity`] zeros that velocity
/// the frame after the pulse so the body doesn't drift.
#[derive(Component)]
pub struct JustMovedKinematic;

/// Zeros the `LinearVelocity` of bodies marked with
/// [`JustMovedKinematic`], **after one physics tick has consumed
/// the velocity** for joint propagation.
///
/// Schedule: `FixedPostUpdate`. Bevy's main schedule order is
/// `RunFixedMainLoop` (FixedUpdate cycle) → `Update`. So when a
/// `MoveEntity` observer fires in Frame N's `Update` and sets
/// LinearVelocity + marker, the velocity must persist through the
/// *next* fixed-tick physics step (Frame N+1 `FixedUpdate`) before
/// being zeroed. Running this in `FixedPostUpdate` (which fires
/// after every `FixedUpdate` step) does exactly that:
///
/// - Frame N `Update`: `MoveEntity` sets velocity + inserts marker.
/// - Frame N+1 `FixedUpdate`: physics runs WITH the velocity;
///   Avian's joint solver sees the kinematic body moving and
///   propagates the motion through joints to coupled dynamic bodies.
/// - Frame N+1 `FixedPostUpdate`: this system runs, zeros velocity,
///   removes marker.
/// - Frame N+2 `FixedUpdate`: physics with velocity = 0; body
///   settled at its new position, no drift.
pub fn clear_kinematic_pulse_velocity(
    mut commands: Commands,
    mut q: Query<(Entity, &mut LinearVelocity), With<JustMovedKinematic>>,
) {
    for (e, mut vel) in q.iter_mut() {
        vel.0 = DVec3::ZERO;
        commands.entity(e).remove::<JustMovedKinematic>();
    }
}

// ─────────────────────────────────────────────────────────────────────
// SetObjectProperty — ONE general verb to set any property on an object
// ─────────────────────────────────────────────────────────────────────

/// Set a property on a scene object at runtime (live override — not persisted
/// to USD). One general command instead of many narrow ones; new properties
/// just add a `match` arm. Drive it from curl after a screenshot to iterate:
///
/// ```jsonc
/// {"command":"SetObjectProperty",
///  "params":{"entity_id":42,"property":"shader","value":"shaders/spin_reveal.wgsl"}}
/// {"command":"SetObjectProperty",
///  "params":{"entity_id":42,"property":"param0","value":"12"}}   // wedge count
/// {"command":"SetObjectProperty",
///  "params":{"entity_id":42,"property":"colorA","value":"0.1,0.8,0.2"}}
/// ```
///
/// Recognised `property` values:
/// - `shader` → load that `.wgsl` (asset path) and bind it via `UsdShaderMaterial`.
/// - `param0`..`param7`, `colorA`/`color`/`colorB`/`colorC` → update the object's
///   shader uniforms in place (requires `shader` to have been set first, or a
///   USD `usd_shader` material).
/// - `visible` → `true`/`false` toggles `Visibility`.
/// - Per-wheel tire-spin dynamics (target a single wheel entity by its `api_id`):
///   `drive_torque`, `brake_torque`, `slip_stiffness`, `bearing_damping`,
///   `friction_mu`, `mass`, `moi`, `wheel_radius`, `rest_length`, `spring_k`,
///   `damping_c` → set that `f64` field on the wheel's `WheelRaycast` live.
///   Each wheel is its own entity, so this gives independent per-wheel control.
#[Command(default)]
pub struct SetObjectProperty {
    /// API-stable global entity ID (the `api_id` from `ListEntities`), same
    /// resolution path as [`MoveEntity`].
    pub entity_id: u64,
    /// Property name (see struct docs).
    pub property: String,
    /// Value; comma-separated `r,g,b` for colors, a single float for params,
    /// an asset path for `shader`, `true`/`false` for `visible`.
    pub value: String,
}

/// Maps a `SetObjectProperty` property name to a setter on `WheelRaycast`, or
/// `None` if the name isn't a wheel field. Non-capturing closures coerce to
/// `fn` pointers, so this stays a cheap lookup table. Accepts both the Rust
/// field names and the USD-style aliases (`radius`, `spring_stiffness`, …).
fn wheel_param_setter(name: &str) -> Option<fn(&mut lunco_mobility::WheelRaycast, f64)> {
    use lunco_mobility::WheelRaycast as W;
    Some(match name {
        "drive_torque" | "drive_torque_max" => |w: &mut W, v| w.drive_torque_max = v,
        "brake_torque" | "brake_torque_max" => |w: &mut W, v| w.brake_torque_max = v,
        "slip_stiffness" => |w: &mut W, v| w.slip_stiffness = v,
        "bearing_damping" | "damping_rate" => |w: &mut W, v| w.bearing_damping = v,
        "friction_mu" | "friction" => |w: &mut W, v| w.friction_mu = v,
        "mass" => |w: &mut W, v| w.mass = v,
        "moi" | "moment_of_inertia" => |w: &mut W, v| w.moment_of_inertia = v,
        "wheel_radius" | "radius" => |w: &mut W, v| w.wheel_radius = v,
        "rest_length" => |w: &mut W, v| w.rest_length = v,
        "spring_k" | "spring_stiffness" => |w: &mut W, v| w.spring_k = v,
        "damping_c" | "spring_damping" => |w: &mut W, v| w.damping_c = v,
        _ => return None,
    })
}

/// Observer for [`SetObjectProperty`].
pub fn on_set_object_property(
    trigger: On<SetObjectProperty>,
    registry: Res<lunco_api::registry::ApiEntityRegistry>,
    asset_server: Res<AssetServer>,
    mut materials: ResMut<Assets<lunco_materials::ShaderMaterial>>,
    q_mat: Query<&MeshMaterial3d<lunco_materials::ShaderMaterial>>,
    mut q_vis: Query<&mut Visibility>,
    mut q_wheel: Query<&mut lunco_mobility::WheelRaycast>,
    mut commands: Commands,
) {
    let cmd = trigger.event();
    let global_id = lunco_core::GlobalEntityId::from_raw(cmd.entity_id);
    let Some(target) = registry.resolve(&global_id) else {
        warn!("SET_PROPERTY: no api_id={} in registry", cmd.entity_id);
        return;
    };

    // Per-wheel tire-spin dynamics. Each wheel is its own entity, so addressing
    // a single `api_id` sets the field on just that wheel — independent control.
    if let Some(setter) = wheel_param_setter(&cmd.property) {
        let Ok(value) = cmd.value.trim().parse::<f64>() else {
            warn!("SET_PROPERTY: '{}' expects a number, got '{}'", cmd.property, cmd.value);
            return;
        };
        let Ok(mut wheel) = q_wheel.get_mut(target) else {
            warn!("SET_PROPERTY: entity {} has no WheelRaycast", cmd.entity_id);
            return;
        };
        setter(&mut wheel, value);
        info!("SET_PROPERTY: wheel {} {} = {}", cmd.entity_id, cmd.property, value);
        return;
    }

    match cmd.property.as_str() {
        "shader" => {
            // Preserve existing uniforms if the object already has a
            // ShaderMaterial, so swapping the .wgsl keeps tuned params.
            let template = q_mat
                .get(target)
                .ok()
                .and_then(|m| materials.get(&m.0))
                .cloned()
                .unwrap_or_default();
            let shader = asset_server.load(&cmd.value);
            let handle = materials.add(lunco_materials::build_shader_material(shader, template));
            commands
                .entity(target)
                .remove::<MeshMaterial3d<StandardMaterial>>()
                .insert(MeshMaterial3d(handle));
            info!("SET_PROPERTY: {} shader = {}", cmd.entity_id, cmd.value);
        }
        "visible" => {
            let Ok(mut vis) = q_vis.get_mut(target) else {
                warn!("SET_PROPERTY: entity {} has no Visibility", cmd.entity_id);
                return;
            };
            let v = cmd.value.trim();
            *vis = if matches!(v, "false" | "0" | "hidden") {
                Visibility::Hidden
            } else {
                Visibility::Visible
            };
        }
        key => {
            // param/color → mutate the live shader material's uniforms in place.
            let Ok(m) = q_mat.get(target) else {
                warn!("SET_PROPERTY: entity {} has no usd_shader material — set 'shader' first", cmd.entity_id);
                return;
            };
            let Some(mat) = materials.get_mut(&m.0) else { return };
            if !lunco_materials::apply_param(mat, key, &cmd.value) {
                warn!("SET_PROPERTY: unknown property '{}'", key);
            }
        }
    }
}

/// Point the free-flight avatar camera at an entity (by API id), from a fixed
/// side-on-and-above angle at `distance` metres. Lets API clients (MCP tools,
/// automated screenshots) frame a subject — e.g. a wheel — without hand-driving
/// the camera. `entity_id` is the API id from `ListEntities` (a `u64`), same as
/// [`MoveEntity`]/[`SetObjectProperty`].
#[Command(default)]
pub struct FocusEntityById {
    pub entity_id: u64,
    /// Camera distance from the target, metres. `<= 0` → default 6.
    pub distance: f32,
}

/// Observer that aims the avatar at the requested entity.
pub fn on_focus_entity_by_id(
    trigger: On<FocusEntityById>,
    registry: Res<lunco_api::registry::ApiEntityRegistry>,
    q_target: Query<&GlobalTransform>,
    mut q_avatar: Query<(&mut Transform, &mut lunco_avatar::FreeFlightCamera), With<lunco_core::Avatar>>,
) {
    let cmd = trigger.event();
    let global_id = lunco_core::GlobalEntityId::from_raw(cmd.entity_id);
    let Some(target) = registry.resolve(&global_id) else {
        warn!("FOCUS_ENTITY: no api_id={} in registry", cmd.entity_id);
        return;
    };
    let Ok(target_gt) = q_target.get(target) else {
        warn!("FOCUS_ENTITY: target {:?} has no GlobalTransform", target);
        return;
    };
    let Ok((mut tf, mut ff)) = q_avatar.single_mut() else {
        warn!("FOCUS_ENTITY: no Avatar with FreeFlightCamera in the scene");
        return;
    };
    // In big_space, `GlobalTransform` is expressed relative to the floating
    // origin — which is the avatar — so this IS the avatar→target vector in
    // render space (grid-aligned, unit scale).
    // `GlobalTransform.translation()` is the target's absolute world position in
    // this sandbox (the avatar's CellCoord is 0, so local == world).
    let target_pos = target_gt.translation();
    let dist = if cmd.distance > 0.1 { cmd.distance } else { 6.0 };
    // Camera sits mostly to the SIDE (+X, the wheel axle direction → we see the
    // spoke face) plus a little up and forward.
    let offset = Vec3::new(1.0, 0.4, 0.25).normalize() * dist;
    // Set the avatar to target + offset (absolute, like MoveEntity).
    tf.translation = target_pos + offset;
    // Camera forward = (camera → target) = -offset. The freeflight system rebuilds
    // rotation from yaw/pitch every frame (YXZ euler), so we must set those.
    let d = (-offset).normalize();
    ff.yaw = (-d.x).atan2(-d.z);
    ff.pitch = d.y.clamp(-1.0, 1.0).asin();
    info!("FOCUS_ENTITY: framed api_id={} at {:.1} m", cmd.entity_id, dist);
}

/// Aim the free-flight avatar camera: place it at `eye` and look at `target`
/// (both absolute world-space). The flexible primitive — the client computes the
/// angle (e.g. approach a wheel from its outboard side) and distance. Sets the
/// `FreeFlightCamera` yaw/pitch (the camera system rebuilds rotation from those
/// each frame), so the aim sticks.
#[Command(default)]
pub struct SetCameraLookAt {
    pub eye: Vec3,
    pub target: Vec3,
}

/// Observer for [`SetCameraLookAt`].
pub fn on_set_camera_look_at(
    trigger: On<SetCameraLookAt>,
    mut q_avatar: Query<(&mut Transform, &mut lunco_avatar::FreeFlightCamera), With<lunco_core::Avatar>>,
) {
    let cmd = trigger.event();
    let Ok((mut tf, mut ff)) = q_avatar.single_mut() else {
        warn!("SET_CAMERA: no Avatar with FreeFlightCamera in the scene");
        return;
    };
    tf.translation = cmd.eye;
    let look = cmd.target - cmd.eye;
    if look.length() > 1e-4 {
        let d = look.normalize();
        ff.yaw = (-d.x).atan2(-d.z);
        ff.pitch = d.y.clamp(-1.0, 1.0).asin();
    }
    info!(
        "SET_CAMERA: eye=({:.2},{:.2},{:.2}) target=({:.2},{:.2},{:.2})",
        cmd.eye.x, cmd.eye.y, cmd.eye.z, cmd.target.x, cmd.target.y, cmd.target.z
    );
}

/// Force-reload shader assets from disk so live WGSL edits apply without
/// restarting the app. Bypasses the file watcher (unreliable in this build):
/// calls [`AssetServer::reload`], which re-runs the loader and triggers
/// dependent material pipelines to rebuild. Empty `path` → reload the standard
/// `assets/shaders/*` set; otherwise reload just that path (e.g.
/// `"shaders/wheel.wgsl"`).
#[Command(default)]
pub struct ReloadShader {
    pub path: String,
}

/// Observer for [`ReloadShader`].
pub fn on_reload_shader(trigger: On<ReloadShader>, asset_server: Res<AssetServer>) {
    let p = trigger.event().path.trim().to_string();
    let paths: Vec<String> = if p.is_empty() {
        ["shaders/wheel.wgsl", "shaders/balloon.wgsl", "shaders/solar_panel.wgsl"]
            .iter()
            .map(|s| s.to_string())
            .collect()
    } else {
        vec![p]
    };
    for path in paths {
        // Owned `String` → `AssetPath<'static>`, so the queued reload doesn't
        // borrow the (short-lived) trigger.
        asset_server.reload(path.clone());
        info!("RELOAD_SHADER: {}", path);
    }
}

/// Replace a shader asset's WGSL **source in place** from text sent over the
/// API, recompiling it live without touching disk or restarting. Overwrites the
/// `Shader` asset currently at `path` (e.g. `"shaders/wheel.wgsl"`), so every
/// material using it re-specializes its pipeline next frame. Compile/validation
/// outcome surfaces in the render log (naga errors on a bad shader). Pairs with
/// [`ReloadShader`] (disk) — this one is for pushing edits directly.
#[Command(default)]
pub struct SetShaderSource {
    /// Asset path of the shader to overwrite, e.g. `"shaders/wheel.wgsl"`.
    pub path: String,
    /// New WGSL source text.
    pub source: String,
}

/// Observer for [`SetShaderSource`].
pub fn on_set_shader_source(
    trigger: On<SetShaderSource>,
    asset_server: Res<AssetServer>,
    mut shaders: ResMut<Assets<bevy::shader::Shader>>,
) {
    let ev = trigger.event();
    if ev.path.is_empty() || ev.source.is_empty() {
        warn!("SET_SHADER_SOURCE: empty path or source");
        return;
    }
    // `load` returns the handle the materials are already using (the asset is
    // loaded), so overwriting that asset id propagates to them.
    let handle = asset_server.load::<bevy::shader::Shader>(ev.path.clone());
    let shader = bevy::shader::Shader::from_wgsl(ev.source.clone(), ev.path.clone());
    shaders.insert(handle.id(), shader);
    info!(
        "SET_SHADER_SOURCE: recompiled {} from {} bytes of WGSL",
        ev.path,
        ev.source.len()
    );
}

/// Plugin that registers SPAWN_ENTITY / MOVE_ENTITY / SET_OBJECT_PROPERTY /
/// FOCUS_ENTITY_BY_ID / SET_CAMERA_LOOK_AT / RELOAD_SHADER / SET_SHADER_SOURCE
/// command observers and the kinematic-pulse cleanup system.
pub struct SpawnCommandPlugin;

impl Plugin for SpawnCommandPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_spawn_entity_command);
        app.add_observer(on_move_entity_command);
        app.add_observer(on_set_object_property);
        app.add_observer(on_focus_entity_by_id);
        app.add_observer(on_set_camera_look_at);
        app.add_observer(on_reload_shader);
        app.add_observer(on_set_shader_source);
        // Register with AppTypeRegistry so the reflection-based HTTP executor
        // (`get_with_short_type_path`) can construct it from `{"command":"SetObjectProperty",...}`.
        app.register_type::<SetObjectProperty>();
        app.register_type::<FocusEntityById>();
        app.register_type::<SetCameraLookAt>();
        app.register_type::<ReloadShader>();
        app.register_type::<SetShaderSource>();
        app.add_systems(FixedPostUpdate, clear_kinematic_pulse_velocity);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_spawn_entity_struct_exists() {
        // Verify the struct can be constructed
        let cmd = super::SpawnEntity {
            target: bevy::prelude::Entity::PLACEHOLDER,
            entry_id: "test".to_string(),
            position: bevy::math::Vec3::ZERO,
        };
        assert_eq!(cmd.entry_id, "test");
    }
}
