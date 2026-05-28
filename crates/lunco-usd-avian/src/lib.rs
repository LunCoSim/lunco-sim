//! # LunCoSim USD → Avian3D Physics Mapping
//!
//! Maps USD physics attributes to Avian3D components. This is the **second** plugin in
//! the USD processing pipeline, running after `UsdBevyPlugin` and alongside `UsdSimPlugin`.
//!
//! ## USD Standard: Compound Rigid Bodies
//!
//! Per the OpenUSD specification, a prim with `PhysicsRigidBodyAPI` aggregates all
//! descendant colliders into a **single compound rigid body**. Children with only
//! `PhysicsCollisionAPI` contribute collider shapes but are NOT independent bodies.
//!
//! Our loader follows this standard:
//! - **Parent with RigidBodyAPI** → ONE `RigidBody::Dynamic` + `SelectableRoot`
//! - **Children with CollisionAPI** → `Collider` only (no independent `RigidBody`)
//!
//! ## Mapped Attributes
//!
//! | USD Attribute | Avian3D Component | Notes |
//! |---|---|---|
//! | `PhysicsRigidBodyAPI` (parent) | `RigidBody::Dynamic` | ONE per compound assembly |
//! | `PhysicsCollisionAPI` (child) | `Collider` | Aggregated into parent compound |
//! | `physics:mass` | `Mass` | On the rigid body root |
//! | `physics:linearDamping` | `LinearDamping` | |
//! | `physics:angularDamping` | `AngularDamping` | |
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
use bevy::ecs::schedule::common_conditions::any_with_component;
use bevy::math::DVec3;
use avian3d::prelude::*;
use avian3d::physics_transform::{Position, Rotation};
use lunco_usd_bevy::UsdVisualSynced;
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
        // `on_add_usd_prim`: eager observer for joint pending-state.
        // `process_usd_avian_prims`: observer on UsdVisualSynced — fires
        //   right after `sync_usd_visuals` translates each prim, so the
        //   stage is loaded and Mesh3d/Transform exist.
        // `build_usd_physics_joints`: stays a per-frame system because
        //   it's a deferred state-machine waiting on Avian to admit
        //   both bodies into its island graph (FixedUpdate-driven).
        //   `run_if(any pending)` makes it idle when no joints await.
        app.add_observer(on_add_usd_prim)
            .add_observer(process_usd_avian_prims)
            .add_systems(
                Update,
                build_usd_physics_joints.run_if(any_with_component::<PendingUsdJoint>),
            );
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
    /// Anchor point on body0 in body0's local frame
    /// (UsdPhysics `physics:localPos0`). Defaults to origin.
    pub local_pos0: DVec3,
    /// Anchor point on body1 in body1's local frame
    /// (UsdPhysics `physics:localPos1`). Defaults to origin.
    pub local_pos1: DVec3,
    /// Lower travel limit along the axis (meters for prismatic, radians for revolute).
    pub limit_lower: f64,
    /// Upper travel limit.
    pub limit_upper: f64,
    /// The joint kind from USD (e.g., `PhysicsPrismaticJoint`).
    pub joint_type: String,
}

/// Checks if a USD prim has a specific API schema applied.
///
/// Reads the `apiSchemas` attribute. Handles all value types including
/// `TokenListOp` which stores `prepend`/`append`/`add` operations separately.
fn has_api_schema(reader: &TextReader, sdf_path: &SdfPath, schema_name: &str) -> bool {
    if let Ok(val) = reader.get(sdf_path, "apiSchemas") {
        match &*val {
            Value::Token(s) => return s.contains(schema_name),
            Value::TokenVec(ss) => return ss.iter().any(|s| s.contains(schema_name)),
            Value::String(s) => return s.contains(schema_name),
            Value::TokenListOp(list_op) => {
                for s in &list_op.explicit_items { if s.as_str() == schema_name { return true; } }
                for s in &list_op.prepended_items { if s.as_str() == schema_name { return true; } }
                for s in &list_op.appended_items { if s.as_str() == schema_name { return true; } }
                for s in &list_op.added_items { if s.as_str() == schema_name { return true; } }
            }
            _ => {}
        }
    }
    false
}

/// Collects collider shapes from all child prims of a compound body root,
/// reading directly from the USD stage.
///
/// Returns a list of `(Position, Rotation, Collider)` tuples for `Collider::compound()`.
fn collect_child_colliders_from_usd(
    reader: &TextReader,
    parent_path: &SdfPath,
) -> Vec<(Position, Rotation, Collider)> {
    let mut shapes = Vec::new();

    for child_path in reader.prim_children(parent_path) {
        // Skip wheel children — they're independent dynamics handled
        // by `lunco-usd-sim` (raycast probe or physical wheel rigid
        // body), NOT collider pieces of the chassis compound. The
        // `physxVehicleWheel:radius` attribute is the canonical marker
        // (matches the same skip in `process_usd_avian_prims`).
        if reader.prim_attribute_value::<f32>(&child_path, "physxVehicleWheel:radius").is_some() {
            continue;
        }

        // Check if child has collision enabled
        let child_collision = reader
            .prim_attribute_value::<bool>(&child_path, "physics:collisionEnabled")
            .unwrap_or(true);
        if !child_collision { continue; }

        // Read child's local transform
        let mut child_tf = read_transform_from_usd(reader, &child_path);

        // For Cylinder children, fold UsdGeomCylinder.axis into the
        // child's compound-local rotation so the Y-axis collider lines
        // up with the authored axis (mirrors what lunco-usd-bevy does
        // for the entity Transform).
        if let Ok(val) = reader.get(&child_path, "typeName") {
            if let Value::Token(ty) = &*val {
                if ty.as_str() == "Cylinder" {
                    let axis_tok = read_token_attribute(reader, &child_path, "axis")
                        .unwrap_or_else(|| "Z".to_string());
                    let axis_q = match axis_tok.as_str() {
                        "X" => Some(Quat::from_rotation_arc(Vec3::Y, Vec3::X)),
                        "Z" => Some(Quat::from_rotation_arc(Vec3::Y, Vec3::Z)),
                        _ => None,
                    };
                    if let Some(q) = axis_q {
                        child_tf.rotation = child_tf.rotation * q;
                    }
                }
            }
        }

        // Build collider from child's geometry
        if let Some(collider) = build_collider_from_usd(reader, &child_path) {
            let pos = Position(DVec3::new(
                child_tf.translation.x as f64,
                child_tf.translation.y as f64,
                child_tf.translation.z as f64,
            ));
            let rot = Rotation(child_tf.rotation.as_dquat());
            shapes.push((pos, rot, collider));
        }
    }

    shapes
}

/// Builds a Collider from a USD prim's geometry type and dimensions.
///
/// Spec-compliant shape attributes (UsdGeomCube/Sphere/Cylinder):
/// - **Cube**: `double size` (default 2.0). Non-uniform extents come
///   from `xformOp:scale` and are multiplied in here.
/// - **Sphere**: `double radius` (default 1.0). Honoured uniformly
///   from the largest `xformOp:scale` component (Avian's `sphere`
///   collider is uniform).
/// - **Cylinder**: `double radius`, `double height` (defaults 1, 2).
///   Scale: X/Z components apply to radius (max), Y to height.
///
/// **Legacy fallback for `Cube`**: `width`/`height`/`depth` still
/// accepted so unmigrated `.usda` files keep working.
fn build_collider_from_usd(reader: &TextReader, sdf_path: &SdfPath) -> Option<Collider> {
    let Ok(val) = reader.get(sdf_path, "typeName") else { return None; };
    let Value::Token(ty) = &*val else { return None; };

    let scale = read_vec3_attribute(reader, sdf_path, "xformOp:scale")
        .map(|v| (v.x, v.y, v.z))
        .unwrap_or((1.0, 1.0, 1.0));

    match ty.as_str() {
        "Cube" => {
            if let (Some(width), Some(height), Some(depth)) = (
                reader.prim_attribute_value::<f64>(sdf_path, "width"),
                reader.prim_attribute_value::<f64>(sdf_path, "height"),
                reader.prim_attribute_value::<f64>(sdf_path, "depth"),
            ) {
                Some(Collider::cuboid(width, height, depth))
            } else {
                let size = reader.prim_attribute_value::<f64>(sdf_path, "size").unwrap_or(2.0);
                Some(Collider::cuboid(size * scale.0, size * scale.1, size * scale.2))
            }
        }
        "Sphere" => {
            let radius = reader.prim_attribute_value::<f64>(sdf_path, "radius").unwrap_or(1.0);
            // Avian's sphere is uniform — pick the max axis scale so a
            // user-authored bigger scale doesn't shrink the collider.
            let s = scale.0.max(scale.1).max(scale.2);
            Some(Collider::sphere(radius * s))
        }
        "Cylinder" => {
            let radius = reader.prim_attribute_value::<f64>(sdf_path, "radius").unwrap_or(1.0);
            let height = reader.prim_attribute_value::<f64>(sdf_path, "height").unwrap_or(2.0);
            // Avian's `Collider::cylinder` is Y-axis natively. The
            // UsdGeomCylinder.axis token is honoured by the entity's
            // Transform rotation (composed in `lunco-usd-bevy`) — for
            // standalone cylinder bodies that's enough, and for
            // compound children `collect_child_colliders_from_usd`
            // adds the axis rotation onto the child's local rotation.
            // Scale interpretation always treats Y as axial here; the
            // entity rotation will swing it to whichever world axis.
            let radial = scale.0.max(scale.2);
            Some(Collider::cylinder(radius * radial, height * scale.1))
        }
        _ => None,
    }
}

/// Reads the local transform from a USD prim.
fn read_transform_from_usd(reader: &TextReader, sdf_path: &SdfPath) -> Transform {
    let translation = read_vec3_attribute(reader, sdf_path, "xformOp:translate")
        .map(|v| Vec3::new(v.x as f32, v.y as f32, v.z as f32))
        .unwrap_or(Vec3::ZERO);

    // Read rotation as Euler angles (degrees from USD → radians for Bevy)
    let rotation = if let Some(rot) = read_vec3_attribute(reader, sdf_path, "xformOp:rotateXYZ") {
        Quat::from_euler(
            EulerRot::XYZ,
            (rot.x as f32).to_radians(),
            (rot.y as f32).to_radians(),
            (rot.z as f32).to_radians(),
        )
    } else {
        Quat::IDENTITY
    };

    Transform { translation, rotation, scale: Vec3::ONE }
}

/// Adds a collider component to an entity based on USD prim type and dimensions.
fn add_collider_from_usd(
    commands: &mut Commands,
    entity: Entity,
    reader: &TextReader,
    sdf_path: &SdfPath,
) {
    if let Some(collider) = build_collider_from_usd(reader, sdf_path) {
        commands.entity(entity).insert(collider);
    }
}

/// Deferred system that maps USD physics attributes to Avian3D components.
///
/// This system runs in the `Update` schedule and processes all `UsdPrimPath` entities
/// that haven't been marked with `UsdAvianProcessed` yet.
///
/// # USD Compound Rigid Body Standard
///
/// Per OpenUSD spec, a prim with `PhysicsRigidBodyAPI` aggregates all descendant
/// colliders into ONE compound rigid body. Children with only `PhysicsCollisionAPI`
/// contribute collider shapes but are NOT independent bodies.
///
/// # Processing
///
/// **Compound body root (PhysicsRigidBodyAPI):**
/// - Reads all child collider shapes from USD
/// - Builds ONE `Collider::compound()` on the parent
/// - Adds `RigidBody::Dynamic` + `SelectableRoot` + mass/damping/friction
///
/// **Collider children (PhysicsCollisionAPI only):**
/// - Become pure visuals — no RigidBody, no Collider
/// - Their shapes are included in the parent's compound collider
///
/// **Legacy fallback:** `physics:rigidBodyEnabled` attribute for old-style USD files.
/// Observer: fires once per entity, the moment `sync_usd_visuals` finishes
/// translating the USD prim (signalled by inserting `UsdVisualSynced`).
/// By that point the stage is loaded and `Mesh3d`/`Transform` are present —
/// safe to read schemas and attach physics components in one step.
fn process_usd_avian_prims(
    trigger: On<Add, UsdVisualSynced>,
    query: Query<&UsdPrimPath, Without<UsdAvianProcessed>>,
    q_child_of: Query<&ChildOf>,
    stages: Res<Assets<UsdStageAsset>>,
    mut commands: Commands,
) {
    let entity = trigger.entity;
    let Ok(prim_path) = query.get(entity) else { return; };
    {
        let Some(stage) = stages.get(&prim_path.stage_handle) else { return; };
        let Ok(sdf_path) = SdfPath::new(&prim_path.path) else { return; };

        let reader = (*stage.reader).clone();

        // Skip wheel prims — the sim plugin handles those
        if reader.prim_attribute_value::<f32>(&sdf_path, "physxVehicleWheel:radius").is_some() {
            commands.entity(entity).insert(UsdAvianProcessed);
            return;
        }

        // Detect API schemas
        let has_rigid_body_api = has_api_schema(&reader, &sdf_path, "PhysicsRigidBodyAPI");
        let has_collision_api = has_api_schema(&reader, &sdf_path, "PhysicsCollisionAPI");
        let has_terrain_api = has_api_schema(&reader, &sdf_path, "PhysxTerrainAPI");

        // ── TERRAIN HANDLING ──
        // Terrain is a static collider with the TerrainTile marker.
        if has_terrain_api {
            commands.entity(entity).insert((
                RigidBody::Static,
                lunco_terrain::TerrainTile,
            ));
            add_collider_from_usd(&mut commands, entity, &reader, &sdf_path);
            commands.entity(entity).insert(UsdAvianProcessed);
            return;
        }

        if has_rigid_body_api {
            // ── COMPOUND BODY ROOT ──
            // Read child collider shapes from USD and build compound collider
            let compound_shapes = collect_child_colliders_from_usd(&reader, &sdf_path);

            if !compound_shapes.is_empty() {
                let compound = Collider::compound(compound_shapes);
                commands.entity(entity).insert(compound);
            } else {
                // No children with colliders — try this prim itself
                add_collider_from_usd(&mut commands, entity, &reader, &sdf_path);
            }

            // Honour `bool physics:kinematicEnabled = true` for
            // bodies that should be externally controlled (gizmo,
            // scripts, MCP) without responding to gravity or impulses.
            // Kinematic bodies still participate in joint constraints
            // and contact events — that's the value here vs Static.
            let kinematic = reader
                .prim_attribute_value::<bool>(&sdf_path, "physics:kinematicEnabled")
                .unwrap_or(false);
            let body = if kinematic { RigidBody::Kinematic } else { RigidBody::Dynamic };
            commands.entity(entity).insert((
                body,
                lunco_core::SelectableRoot,
            ));

            // Map mass, damping, friction. Always insert a Mass —
            // `apply_gravity_to_rigid_bodies` filters on `With<Mass>`,
            // so a missing mass attribute (e.g. when the value lives on
            // a referenced base prim and openusd-rs's resolver doesn't
            // compose across the reference) would silently disable
            // gravity on the rover root. Default to 1000 kg, matching
            // the canonical rover mass authored in the base rover
            // .usda files.
            let mass = reader
                .prim_attribute_value::<f32>(&sdf_path, "physics:mass")
                .or_else(|| reader.prim_attribute_value::<f64>(&sdf_path, "physics:mass").map(|v| v as f32))
                .unwrap_or(1000.0);
            commands.entity(entity).insert(Mass(mass));
            if let Some(d) = reader.prim_attribute_value::<f32>(&sdf_path, "physics:linearDamping") {
                commands.entity(entity).insert(LinearDamping(d as f64));
            }
            if let Some(d) = reader.prim_attribute_value::<f32>(&sdf_path, "physics:angularDamping") {
                commands.entity(entity).insert(AngularDamping(d as f64));
            }
            if let Some(f) = reader.prim_attribute_value::<f32>(&sdf_path, "physics:friction") {
                commands.entity(entity).insert(Friction::new(f.into()));
            }

            commands.entity(entity).insert(UsdAvianProcessed);
        } else if has_collision_api {
            // ── COLLIDER CHILD ──
            // Part of parent's compound body — pure visual, no physics components.
            // Exception: root-level (no parent) → static collider.
            if q_child_of.get(entity).is_err() {
                commands.entity(entity).insert(RigidBody::Static);
                add_collider_from_usd(&mut commands, entity, &reader, &sdf_path);
            }

            commands.entity(entity).insert(UsdAvianProcessed);
        } else {
            // ── FALLBACK: legacy physics:rigidBodyEnabled ──
            if let Some(true) = reader.prim_attribute_value::<bool>(&sdf_path, "physics:rigidBodyEnabled") {
                commands.entity(entity).insert((
                    RigidBody::Dynamic,
                    lunco_core::SelectableRoot,
                ));
                if let Some(mass) = reader.prim_attribute_value::<f32>(&sdf_path, "physics:mass") {
                    commands.entity(entity).insert(Mass(mass));
                } else if let Some(mass) = reader.prim_attribute_value::<f64>(&sdf_path, "physics:mass") {
                    commands.entity(entity).insert(Mass(mass as f32));
                }
                if let Some(d) = reader.prim_attribute_value::<f32>(&sdf_path, "physics:linearDamping") {
                    commands.entity(entity).insert(LinearDamping(d as f64));
                }
                if let Some(d) = reader.prim_attribute_value::<f32>(&sdf_path, "physics:angularDamping") {
                    commands.entity(entity).insert(AngularDamping(d as f64));
                }
                if let Some(f) = reader.prim_attribute_value::<f32>(&sdf_path, "physics:friction") {
                    commands.entity(entity).insert(Friction::new(f.into()));
                }
                add_collider_from_usd(&mut commands, entity, &reader, &sdf_path);
            } else if let Some(false) = reader.prim_attribute_value::<bool>(&sdf_path, "physics:rigidBodyEnabled") {
                commands.entity(entity).insert(RigidBody::Static);
                add_collider_from_usd(&mut commands, entity, &reader, &sdf_path);
            }

            commands.entity(entity).insert(UsdAvianProcessed);
        }
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

                // Wheel-targeted joints are owned by `lunco-usd-sim` —
                // it spawns them synchronously inside `setup_physical_wheel`
                // alongside the wheel's `RigidBody`/`Collider`/`Motor`,
                // ensuring `JointCollisionDisabled` is in place before
                // any narrow-phase contact forms between wheel and
                // chassis. Building the same joint here would either
                // double up or race the wheel-body init.
                if let Some(b1) = body1.as_ref() {
                    if let Ok(b1_path) = SdfPath::new(b1) {
                        if reader.prim_attribute_value::<f32>(&b1_path, "physxVehicleWheel:radius").is_some() {
                            return;
                        }
                    }
                }

                match (body0, body1) {
                    (Some(body0_path), Some(body1_path)) => {
                        // OpenUSD standard: `UsdPhysicsRevoluteJoint.physics:axis`
                        // is a `uniform token` ("X" | "Y" | "Z"). Older
                        // authoring used a `physics:axis0` Vec3 — keep
                        // that as a fallback for any in-tree scenes
                        // that haven't been migrated yet.
                        let axis = read_token_attribute(&reader, &sdf_path, "physics:axis")
                            .and_then(|t| match t.as_str() {
                                "X" => Some(DVec3::X),
                                "Y" => Some(DVec3::Y),
                                "Z" => Some(DVec3::Z),
                                _ => None,
                            })
                            .or_else(|| read_vec3_attribute(&reader, &sdf_path, "physics:axis0"))
                            .unwrap_or(DVec3::Y);
                        // UsdPhysics `physics:localPos0/1` give the
                        // joint anchor on each body in that body's
                        // local frame. Without these, the joint forces
                        // both body centres to coincide — useful only
                        // when the bodies are co-located, which is
                        // rarely true in practice.
                        let local_pos0 = read_vec3_attribute(&reader, &sdf_path, "physics:localPos0")
                            .unwrap_or(DVec3::ZERO);
                        let local_pos1 = read_vec3_attribute(&reader, &sdf_path, "physics:localPos1")
                            .unwrap_or(DVec3::ZERO);
                        let limit_lower = reader.prim_attribute_value::<f64>(&sdf_path, "physics:limitLower")
                            .unwrap_or(f64::NEG_INFINITY);
                        let limit_upper = reader.prim_attribute_value::<f64>(&sdf_path, "physics:limitUpper")
                            .unwrap_or(f64::INFINITY);

                        info!("Detected USD joint {} -> {} <-> {}", type_name, body0_path, body1_path);

                        commands.entity(entity).insert(PendingUsdJoint {
                            body0_path,
                            body1_path,
                            axis,
                            local_pos0,
                            local_pos1,
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
    // **Avian readiness gate**: matching on `&Position` (added by
    // Avian's body-init systems alongside `BodyIslandNode`) ensures
    // we don't create a joint before Avian has admitted both bodies
    // into its island graph — without this the solver panics with
    // `Neither body … is in an island`. `process_usd_avian_prims`
    // queues the `RigidBody` insertion in our `Update`; Avian's
    // initialisation runs in its `PhysicsSchedule` (FixedUpdate),
    // so this query is empty for the first few frames after spawn,
    // and the joint stays in `PendingUsdJoint` until ready.
    q_bodies: Query<(Entity, &UsdPrimPath), With<Position>>,
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
                        .with_local_anchor1(pending.local_pos0)
                        .with_local_anchor2(pending.local_pos1)
                        .with_slider_axis(pending.axis)
                        .with_limits(pending.limit_lower, pending.limit_upper),
                ));
            }
            "PhysicsRevoluteJoint" => {
                commands.spawn((
                    RevoluteJoint::new(b0, b1)
                        .with_local_anchor1(pending.local_pos0)
                        .with_local_anchor2(pending.local_pos1)
                        .with_hinge_axis(pending.axis)
                        .with_angle_limits(pending.limit_lower, pending.limit_upper),
                ));
            }
            "PhysicsFixedJoint" => {
                commands.spawn((
                    FixedJoint::new(b0, b1)
                        .with_local_anchor1(pending.local_pos0)
                        .with_local_anchor2(pending.local_pos1),
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
                .or_else(|| op.prepended_items.first())
                .or_else(|| op.appended_items.first())
                .or_else(|| op.added_items.first())
            {
                return Some(target.as_str().to_string());
            }
        }
    }
    None
}

/// Reads a USD token attribute (e.g., `uniform token axis = "X"`).
fn read_token_attribute(reader: &TextReader, path: &SdfPath, attr: &str) -> Option<String> {
    if let Ok(val) = reader.get(path, attr) {
        match &*val {
            Value::Token(t) => return Some(t.clone()),
            Value::String(s) => return Some(s.clone()),
            _ => {}
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
    // **Fixed-size array forms first** (`Value::Vec3f`/`Value::Vec3d`).
    // USD's `point3f` and `float3` parse as `[f32; 3]`, `point3d` and
    // `double3` as `[f64; 3]`. Without this, `physics:localPos0` etc.
    // (declared `point3f` in our scenes) silently read as `None` and
    // defaulted joint anchors to zero — producing infinite-stiffness
    // corrective impulses that launched bodies into orbit.
    if let Some(v) = reader.prim_attribute_value::<[f32; 3]>(path, attr) {
        return Some(DVec3::new(v[0] as f64, v[1] as f64, v[2] as f64));
    }
    if let Some(v) = reader.prim_attribute_value::<[f64; 3]>(path, attr) {
        return Some(DVec3::new(v[0], v[1], v[2]));
    }
    // Vec<f32>/Vec<f64> array forms (rare in authored USD).
    if let Some(v) = reader.prim_attribute_value::<Vec<f64>>(path, attr) {
        if v.len() >= 3 { return Some(DVec3::new(v[0], v[1], v[2])); }
    }
    if let Some(v) = reader.prim_attribute_value::<Vec<f32>>(path, attr) {
        if v.len() >= 3 { return Some(DVec3::new(v[0] as f64, v[1] as f64, v[2] as f64)); }
    }
    None
}
