//! # LunCoSim USD → Avian3D Physics Mapping
//!
//! Maps USD physics attributes to Avian3D components. This is the **second** plugin in
//! the USD processing pipeline, running after `UsdBevyPlugin` and alongside `UsdSimPlugin`.
//!
//! ## Mapped Attributes
//!
//! | USD Attribute | Avian3D Component | Notes |
//! |---|---|---|
//! | `physics:rigidBodyEnabled = true` | `RigidBody::Dynamic` | |
//! | `physics:rigidBodyEnabled = false` | `RigidBody::Static` | Only if collision enabled |
//! | `physics:mass` | `Mass` | Supports f32 and f64 |
//! | `physics:linearDamping` | `LinearDamping` | |
//! | `physics:angularDamping` | `AngularDamping` | |
//! | `physics:collisionEnabled = true` | `Collider` | Based on prim type |
//! | `physics:friction` | `Friction` | |
//!
//! ## Collider Mapping
//!
//! The collider shape is determined by the prim's `typeName`:
//! - `Cube` → `Collider::cuboid(width, height, depth)` — full dimensions
//! - `Sphere` → `Collider::sphere(radius)`
//! - `Cylinder` → `Collider::cylinder(radius, height)`
//!
//! **Important**: `Collider::cuboid()` takes **full dimensions** (same as the USD file's
//! `width`/`height`/`depth`), not half-extents. Avian3D internally halves them to produce
//! the half-extents used in collision detection.
//!
//! ## Why Deferred Processing?
//!
//! The `On<Add, UsdPrimPath>` observer fires when the entity is spawned, but the USD asset
//! may not be loaded yet (async loading). The `process_usd_avian_prims` system runs in the
//! `Update` schedule and retries every frame until the asset is available.

use bevy::prelude::*;
use bevy::math::DVec3;
use avian3d::prelude::*;
pub use lunco_usd_bevy::{UsdPrimPath, UsdStageAsset};
use openusd::sdf::{AbstractData, Path as SdfPath, Value};
use openusd::usda::TextReader;

/// Bevy plugin for USD physics mapping.
///
/// Adds an observer for USD prim spawning and a deferred processing system that maps
/// USD physics attributes to Avian3D components. The deferred system runs in the
/// `Update` schedule **after** `sync_usd_visuals` to ensure assets are loaded.
pub struct UsdAvianPlugin;

impl Plugin for UsdAvianPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_add_usd_prim)
            .add_systems(Update, (process_usd_avian_prims, build_usd_physics_joints).chain());
    }
}

/// Marker to indicate a prim has been processed by the Avian physics system.
///
/// Prevents the deferred processing system from re-processing the same entity on
/// subsequent frames.
#[derive(Component)]
struct UsdAvianProcessed;

/// Marker for USD prims awaiting joint creation.
///
/// Inserted when a `PhysicsPrismaticJoint` (or other joint type) is detected in USD
/// but the referenced body entities haven't been spawned yet. The `build_usd_physics_joints`
/// system checks for these markers and creates Avian3D joints once both bodies exist.
#[derive(Component)]
pub struct PendingUsdJoint {
    /// USD path to body0 (the anchor/chassis).
    pub body0_path: String,
    /// USD path to body1 (the driven body/wheel).
    pub body1_path: String,
    /// Joint axis in local space of body0.
    pub axis: DVec3,
    /// Lower travel limit along the axis (meters for prismatic, radians for revolute).
    pub limit_lower: f64,
    /// Upper travel limit.
    pub limit_upper: f64,
    /// The joint kind from USD (e.g., `PhysicsPrismaticJoint`).
    pub joint_type: String,
}

/// Deferred system that maps USD physics attributes to Avian3D components.
///
/// This system runs in the `Update` schedule and processes all `UsdPrimPath` entities
/// that haven't been marked with `UsdAvianProcessed` yet. It checks if the USD asset
/// is loaded and, if so, maps physics attributes.
///
/// # What It Maps
///
/// For non-wheel prims (chassis, ground, ramp, etc.):
/// - `physics:rigidBodyEnabled = true` → `RigidBody::Dynamic`
/// - `physics:rigidBodyEnabled = false` → `RigidBody::Static` (if collision enabled)
/// - `physics:mass` → `Mass`
/// - `physics:linearDamping` → `LinearDamping`
/// - `physics:angularDamping` → `AngularDamping`
/// - `physics:collisionEnabled = true` → `Collider` (based on prim type)
/// - `physics:friction` → `Friction`
///
/// For wheel prims: skipped (handled by the sim plugin which creates `WheelRaycast`).
fn process_usd_avian_prims(
    mut commands: Commands,
    query: Query<(Entity, &UsdPrimPath), Without<UsdAvianProcessed>>,
    stages: Res<Assets<UsdStageAsset>>,
) {
    for (entity, prim_path) in query.iter() {
        let Some(stage) = stages.get(&prim_path.stage_handle) else { continue; };
        let Ok(sdf_path) = SdfPath::new(&prim_path.path) else { continue; };

        let reader = (*stage.reader).clone();

        // Skip wheel prims — the sim plugin handles those (raycast wheels don't need physical bodies)
        if reader.prim_attribute_value::<f32>(&sdf_path, "physxVehicleWheel:radius").is_some() {
            commands.entity(entity).insert(UsdAvianProcessed);
            continue;
        }

        // Map RigidBody — also mark as selectable root
        if let Some(true) = reader.prim_attribute_value::<bool>(&sdf_path, "physics:rigidBodyEnabled") {
            commands.entity(entity).insert((
                RigidBody::Dynamic,
                lunco_core::SelectableRoot,
            ));
        }

        // Map Mass
        if let Some(mass) = reader.prim_attribute_value::<f32>(&sdf_path, "physics:mass") {
            commands.entity(entity).insert(Mass(mass));
        } else if let Some(mass) = reader.prim_attribute_value::<f64>(&sdf_path, "physics:mass") {
            commands.entity(entity).insert(Mass(mass as f32));
        }

        // Map Damping (matching original procedural rovers: linear=0.5, angular=2.0)
        if let Some(damping) = reader.prim_attribute_value::<f32>(&sdf_path, "physics:linearDamping") {
            commands.entity(entity).insert(LinearDamping(damping as f64));
        }
        if let Some(damping) = reader.prim_attribute_value::<f32>(&sdf_path, "physics:angularDamping") {
            commands.entity(entity).insert(AngularDamping(damping as f64));
        }

        // Map Collider
        let collision_enabled = reader.prim_attribute_value::<bool>(&sdf_path, "physics:collisionEnabled").unwrap_or(true);

        if collision_enabled {
            if let Ok(val) = reader.get(&sdf_path, "typeName") {
                if let Value::Token(ty) = &*val {
                    match ty.as_str() {
                        "Cube" => {
                            if let (Some(width), Some(height), Some(depth)) = (
                                reader.prim_attribute_value::<f64>(&sdf_path, "width"),
                                reader.prim_attribute_value::<f64>(&sdf_path, "height"),
                                reader.prim_attribute_value::<f64>(&sdf_path, "depth"),
                            ) {
                                // Collider::cuboid takes FULL dimensions (matches procedural: Collider::cuboid(2.0, 0.3, 3.5))
                                // Half-extents are computed internally: hx=1.0, hy=0.15, hz=1.75
                                commands.entity(entity).insert(Collider::cuboid(width, height, depth));
                            }
                        }
                        "Sphere" => {
                            if let Some(radius) = reader.prim_attribute_value::<f64>(&sdf_path, "radius") {
                                commands.entity(entity).insert(Collider::sphere(radius));
                            }
                        }
                        "Cylinder" => {
                            if let (Some(radius), Some(height)) = (
                                reader.prim_attribute_value::<f64>(&sdf_path, "radius"),
                                reader.prim_attribute_value::<f64>(&sdf_path, "height"),
                            ) {
                                commands.entity(entity).insert(Collider::cylinder(radius, height));
                            }
                        }
                        _ => {}
                    }
                }
            }
        }

        // Map Friction (for static and dynamic bodies)
        if let Some(friction) = reader.prim_attribute_value::<f32>(&sdf_path, "physics:friction") {
            commands.entity(entity).insert(Friction::new(friction.into()));
        }

        // Map Static bodies: when physics:rigidBodyEnabled is explicitly false but collisionEnabled is true
        if let Some(false) = reader.prim_attribute_value::<bool>(&sdf_path, "physics:rigidBodyEnabled") {
            if collision_enabled {
                commands.entity(entity).insert(RigidBody::Static);
            }
        }

        commands.entity(entity).insert(UsdAvianProcessed);
    }
}

/// Observer that fires when a USD prim entity is added.
///
/// Currently only detects physics joints (PhysicsPrismaticJoint, PhysicsRevoluteJoint,
/// etc.). Physics mapping for non-joint prims is handled by the deferred system.
fn on_add_usd_prim(
    trigger: On<Add, UsdPrimPath>,
    query: Query<&UsdPrimPath>,
    stages: Res<Assets<UsdStageAsset>>,
    mut commands: Commands,
) {
    let entity = trigger.entity;
    let Ok(prim_path) = query.get(entity) else { return; };
    let Some(stage) = stages.get(&prim_path.stage_handle) else { return; };
    let Ok(sdf_path) = SdfPath::new(&prim_path.path) else { return; };

    let reader = (*stage.reader).clone();

    // Skip wheel prims — the sim plugin handles those (raycast wheels don't need physical bodies)
    if reader.prim_attribute_value::<f32>(&sdf_path, "physxVehicleWheel:radius").is_some() {
        return;
    }

    // --- Detect Physics Joint Prims (PhysicsPrismaticJoint, PhysicsRevoluteJoint, etc.) ---
    if let Ok(val) = reader.get(&sdf_path, "typeName") {
        if let Value::Token(type_name) = &*val {
            if type_name.starts_with("Physics") && type_name.ends_with("Joint") {
                let body0 = read_rel_target(&reader, &sdf_path, "physics:body0");
                let body1 = read_rel_target(&reader, &sdf_path, "physics:body1");

                match (body0, body1) {
                    (Some(body0_path), Some(body1_path)) => {
                        let axis = read_vec3_attribute(&reader, &sdf_path, "physics:axis0")
                            .unwrap_or(DVec3::Y);
                        let limit_lower = reader.prim_attribute_value::<f64>(&sdf_path, "physics:limitLower")
                            .unwrap_or(f64::NEG_INFINITY);
                        let limit_upper = reader.prim_attribute_value::<f64>(&sdf_path, "physics:limitUpper")
                            .unwrap_or(f64::INFINITY);

                        info!("Detected USD joint {} -> {} <-> {}", type_name, body0_path, body1_path);

                        commands.entity(entity).insert(PendingUsdJoint {
                            body0_path,
                            body1_path,
                            axis,
                            limit_lower,
                            limit_upper,
                            joint_type: type_name.clone(),
                        });
                    }
                    (b0, b1) => {
                        warn!("Joint {} missing body refs: body0={:?} body1={:?}",
                            type_name, b0, b1);
                    }
                }
            }
        }
    }

    // Note: Physics mapping (RigidBody, Mass, Collider, Damping) is handled by
    // the sim plugin's process_usd_sim_prims system to ensure consistent ordering
    // and avoid duplicate processing.
}

/// Resolves pending USD joints once both body entities exist.
///
/// This system runs every frame. When a `PendingUsdJoint` entity finds that both its
/// referenced bodies have been spawned as Bevy entities with matching `UsdPrimPath`
/// components, it creates the appropriate Avian joint and removes the pending marker.
fn build_usd_physics_joints(
    mut commands: Commands,
    q_pending: Query<(Entity, &PendingUsdJoint, &UsdPrimPath)>,
    q_bodies: Query<(Entity, &UsdPrimPath)>,
) {
    for (joint_entity, pending, joint_prim_path) in q_pending.iter() {
        // Find body0 and body1 entities by matching USD paths
        let body0_ent = q_bodies.iter()
            .find(|(_, path)| path.path == pending.body0_path
                && path.stage_handle == joint_prim_path.stage_handle)
            .map(|(e, _)| e);
        let body1_ent = q_bodies.iter()
            .find(|(_, path)| path.path == pending.body1_path
                && path.stage_handle == joint_prim_path.stage_handle)
            .map(|(e, _)| e);

        let (Some(b0), Some(b1)) = (body0_ent, body1_ent) else { continue; };

        info!("Built USD joint {} -> {} <-> {}", pending.joint_type, pending.body0_path, pending.body1_path);

        match pending.joint_type.as_str() {
            "PhysicsPrismaticJoint" => {
                commands.spawn((
                    PrismaticJoint::new(b0, b1)
                        .with_slider_axis(pending.axis)
                        .with_limits(pending.limit_lower, pending.limit_upper),
                ));
            }
            "PhysicsRevoluteJoint" => {
                commands.spawn((
                    RevoluteJoint::new(b0, b1)
                        .with_hinge_axis(pending.axis)
                        .with_angle_limits(pending.limit_lower, pending.limit_upper),
                ));
            }
            "PhysicsFixedJoint" => {
                commands.spawn((
                    FixedJoint::new(b0, b1),
                ));
            }
            other => {
                warn!("Unsupported USD joint type: {}", other);
            }
        }

        commands.entity(joint_entity).remove::<PendingUsdJoint>();
    }
}

/// Reads a relationship target from a child relationship spec.
///
/// In the SDF data model, `rel physics:body0 = [</path>]` creates a property
/// spec at `<prim_path>.physics:body0` with `FieldKey::TargetPaths`.
fn read_rel_target(reader: &TextReader, prim_path: &SdfPath, rel_name: &str) -> Option<String> {
    // USD relationship specs live at <prim_path>.<rel_name> (dot-separated property path)
    let rel_path_str = format!("{}.{}", prim_path.as_str(), rel_name);
    let Ok(rel_sdf) = SdfPath::new(&rel_path_str) else { return None; };

    if let Ok(val) = reader.get(&rel_sdf, "targetPaths") {
        if let Value::PathListOp(op) = &*val {
            if let Some(target) = op.explicit_items.first()
                .or(op.prepended_items.first())
                .or(op.appended_items.first())
                .or(op.added_items.first())
            {
                return Some(target.as_str().to_string());
            }
        }
    }
    None
}

/// Reads a DVec3 attribute (e.g., double3 xformOp:translate).
///
/// Tries both `Vec<f64>` and `Vec<f32>` since USD stores vector attributes as
/// floating-point arrays. Returns `None` if the attribute doesn't exist or has
/// fewer than 3 elements.
fn read_vec3_attribute(reader: &TextReader, path: &SdfPath, attr: &str) -> Option<DVec3> {
    if let Some(v) = reader.prim_attribute_value::<Vec<f64>>(path, attr) {
        if v.len() >= 3 { return Some(DVec3::new(v[0], v[1], v[2])); }
    }
    if let Some(v) = reader.prim_attribute_value::<Vec<f32>>(path, attr) {
        if v.len() >= 3 { return Some(DVec3::new(v[0] as f64, v[1] as f64, v[2] as f64)); }
    }
    None
}
