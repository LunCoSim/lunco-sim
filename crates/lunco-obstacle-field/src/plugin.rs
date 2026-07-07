//! Bevy plugin: turn an `ObstacleFieldSpec` into a live heightfield collider +
//! visual mesh + scattered rocks with distance LOD.
//!
//! Phase 1 builds the whole field synchronously on a `RegenerateField` event.
//! Background tasking, chunk streaming, dynamic (pushable) rocks and the tuning
//! UI are layered on in later phases (see PLAN.md) — the generation core they
//! all call is already pure and deterministic in `sampler`/`field`/`assets`.

use avian3d::prelude::{AngularVelocity, Collider, LinearVelocity, Position, RigidBody};
use bevy::asset::RenderAssetUsages;
#[cfg(not(target_arch = "wasm32"))]
use bevy::camera::visibility::VisibilityRange;
use bevy::math::DVec3;
use bevy::prelude::*;
use bevy::render::render_resource::PrimitiveTopology;
use bevy_mesh::Indices;
use big_space::prelude::CellCoord;
use lunco_core::{ArticulatedVehicle, GridAnchor, WorldGrid, Command, on_command, register_commands};

use crate::assets::{bucket_index, bucket_sizes};
use crate::field::{build_height_grid, HeightGrid};
use crate::rock::faceted_rock_mesh;
use crate::sampler::{sample_layer, salt};
use crate::spec::{ObstacleFieldSpec, Pattern};

/// Number of shared rock meshes per size bucket (geometric size buckets).
const ROCK_BUCKETS: usize = 6;
/// Distinct faceted shapes generated per bucket; instances pick one by position.
const ROCK_VARIANTS: usize = 4;

/// Distance LOD margins (metres). Rocks fade out beyond `LOD_FAR`; the terrain
/// surface is always visible.
#[cfg(not(target_arch = "wasm32"))]
const LOD_FAR: f32 = 250.0;
#[cfg(not(target_arch = "wasm32"))]
const LOD_FADE: f32 = 50.0;

/// Render frames to keep physics frozen around a field rebuild. The heightfield
/// collider is torn down and re-created, so without this dynamic bodies (rovers)
/// fall through during the frame(s) before the new collider is re-integrated.
const PHYSICS_HOLD_FRAMES: u32 = 30;

/// Largest vertical re-seat we'll apply (m). Real terrain deltas are a few metres
/// (crater depth + rim); anything larger signals a bad sample, so we skip it
/// rather than fling a body.
const MAX_RESEAT_SHIFT: f64 = 30.0;

/// Field height grids kept across regenerations so resting bodies can be re-seated
/// by the local terrain delta (current − previous height). `reseat_pending` flags
/// a fresh rebuild for `reseat_bodies` to consume.
#[derive(Resource, Default)]
struct ObstacleFieldHeights {
    current: Option<HeightGrid>,
    previous: Option<HeightGrid>,
    reseat_pending: bool,
}

/// Tracks a short physics freeze around field regeneration.
#[derive(Resource, Default)]
struct PhysicsHold {
    frames_left: u32,
    /// True only when *this* system paused time — so we never unpause a freeze
    /// the user started manually.
    paused_by_us: bool,
}

/// Root of a generated field; despawned (recursively) on regeneration.
#[derive(Component)]
pub struct ObstacleFieldRoot;

/// The driveable terrain surface (heightfield collider + visual mesh).
#[derive(Component)]
pub struct FieldTerrain;

/// A scattered rock instance.
#[derive(Component)]
pub struct FieldRock;

/// Fire to (re)build the field from the current `ObstacleFieldSpec` resource.
///
/// A buffered message (Bevy 0.18 renamed buffered `Event` → `Message`).
#[derive(Message, Default)]
pub struct RegenerateField;

/// Registers the obstacle-field generator. The app supplies (or tunes) the
/// `ObstacleFieldSpec` resource and fires `RegenerateField`.
pub struct ObstacleFieldPlugin;

#[Command(default)]
pub struct UpdateObstacleFieldSpec {
    pub spec: ObstacleFieldSpec,
}

#[on_command(UpdateObstacleFieldSpec)]
fn on_update_obstacle_field_spec(
    trigger: On<UpdateObstacleFieldSpec>,
    mut spec: ResMut<ObstacleFieldSpec>,
    mut ev: MessageWriter<RegenerateField>,
    // Journal handle: present once wired (networked / persisted sessions). Every
    // local spec edit is recorded as a `DomainKind::ObstacleField` op so it
    // persists + syncs through the journal plane. Remote peers' edits arrive via
    // the replay leg (which sets the resource directly, NOT this command), so this
    // handler only ever fires for a *local* edit — no wire re-record to guard.
    journal: Option<Res<lunco_doc_bevy::JournalResource>>,
) {
    let next = trigger.event().spec.clone();
    if let Some(journal) = journal.as_ref() {
        crate::journal::record_set_spec(journal, &spec, &next);
    }
    *spec = next;
    ev.write(RegenerateField);
    info!("[ObstacleField] Spec updated and regeneration triggered.");
}

register_commands!(on_update_obstacle_field_spec);

impl Plugin for ObstacleFieldPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<ObstacleFieldSpec>()
            .register_type::<UpdateObstacleFieldSpec>()
            .init_resource::<ObstacleFieldSpec>()
            .init_resource::<ObstacleFieldHeights>()
            .init_resource::<PhysicsHold>()
            .add_message::<RegenerateField>()
            .init_resource::<ObstacleFieldMode>()
            .add_systems(Startup, trigger_initial)
            .add_systems(
                Update,
                // Chained so a rebuild → re-seat bodies → freeze all happen
                // the same frame, before next frame's physics step.
                (regenerate_obstacle_field, reseat_bodies, manage_physics_hold).chain(),
            );
        // NOTE: removing the legacy USD-authored `Ground` prim once a field exists
        // is owned by `lunco-sandbox-edit` (`remove_legacy_ground_prim`), which has
        // USD-stage access and authors a `RemovePrim` op so the Twin stays in sync.
        // This crate is a pure generator (core + terrain only) and never edits the
        // stage.
        register_all_commands(app);
    }
}

/// Freeze the sim for a few frames after a rebuild, then restore — long enough for
/// the re-seated colliders to settle before physics steps again. Goes through the
/// single transport authority (`TimeTransport.mode`, doc 19), which the spine maps
/// onto `relative_speed = 0` (and thus avian's `Time<Physics>`). Only unpauses if
/// *we* paused — a user's manual pause (same authority) survives a regen untouched.
fn manage_physics_hold(
    mut hold: ResMut<PhysicsHold>,
    transport: Option<ResMut<lunco_time::TimeTransport>>,
) {
    if hold.frames_left == 0 {
        return;
    }
    // No time spine in this context (e.g. a bare generator test) — nothing to
    // freeze through the transport; drop the hold so we don't spin forever.
    let Some(mut transport) = transport else {
        hold.frames_left = 0;
        hold.paused_by_us = false;
        return;
    };
    let is_paused = matches!(transport.mode, lunco_time::TransportMode::Paused);
    if !hold.paused_by_us && !is_paused {
        transport.mode = lunco_time::TransportMode::Paused;
        hold.paused_by_us = true;
    }
    hold.frames_left -= 1;
    if hold.frames_left == 0 && hold.paused_by_us {
        transport.mode = lunco_time::TransportMode::Playing;
        hold.paused_by_us = false;
    }
}

/// Who owns crater/rock generation from the shared [`ObstacleFieldSpec`].
///
/// The spec (and its Inspector + networked [`UpdateObstacleFieldSpec`]) is the
/// single source of truth either way; this only selects what the spec *drives*:
///
/// - [`Standalone`](ObstacleFieldMode::Standalone) (default): this plugin builds
///   its own flat-slab arena (a ±200 m heightfield with craters stamped in + rocks
///   scattered on top) and rebuilds it on [`RegenerateField`]. The pre-DEM test
///   scaffold.
/// - [`DemDelegated`](ObstacleFieldMode::DemDelegated): the real ground is a **DEM
///   terrain** (`lunco-terrain-surface`), which consumes the *same* spec — craters
///   stamp into the DEM height grid, rocks scatter on the DEM surface. This plugin
///   then builds **no** slab (it would float on / z-fight the DEM) and leaves
///   [`RegenerateField`] for the terrain crate to apply to the DEM.
///
/// So the moonbase sandbox sets `DemDelegated` and the *one* Inspector panel tunes
/// the DEM craters/rocks live; a slab-only scene leaves it `Standalone`.
// TODO: remove `Standalone` (and with it the slab build + the
// `stamp_crater`/`stamp_craters`/`build_height_grid`/`crater_delta` stamp chain
// in `field.rs`). No production path reaches it: the only `ObstacleFieldPlugin`
// consumer (the sandbox) hardcodes `DemDelegated`, and nothing — scene attr,
// API, rhai — can select Standalone at runtime. It survives as the enum default
// + unit-test scaffolding only. Removing it leaves
// `lunco_terrain_core::crater_profile` as the single crater cross-section.
#[derive(Resource, Clone, Copy, PartialEq, Eq, Debug)]
pub enum ObstacleFieldMode {
    Standalone,
    DemDelegated,
}

impl Default for ObstacleFieldMode {
    fn default() -> Self {
        Self::Standalone
    }
}

impl ObstacleFieldMode {
    /// True when this plugin owns the flat-slab build path.
    pub fn is_standalone(self) -> bool {
        matches!(self, ObstacleFieldMode::Standalone)
    }
}

fn trigger_initial(mode: Res<ObstacleFieldMode>, mut ev: MessageWriter<RegenerateField>) {
    if mode.is_standalone() {
        ev.write(RegenerateField);
    }
}

/// Build a Bevy `Mesh` from raw height-grid vertex arrays. The single
/// `MeshData`/`TileMesh` → `Mesh` glue, shared by the obstacle field, the static
/// DEM terrain, and the streaming LOD tiles (was duplicated in three places).
pub fn grid_mesh(
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    uvs: Vec<[f32; 2]>,
    indices: Vec<u32>,
) -> Mesh {
    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default());
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_indices(Indices::U32(indices));
    mesh
}

/// Cull range: full visibility up to `LOD_FAR`, cross-fade to hidden over `LOD_FADE`.
///
/// NATIVE ONLY — `VisibilityRange` drives bevy's `visibility_ranges` view binding
/// (group 0, binding 14), whose WebGL2 uniform fallback has a `min_binding_size`
/// mismatch in bevy 0.18 that invalidates every `pbr_opaque_mesh_pipeline` (black
/// viewport). On web the bounded, shared-mesh rocks stay always-visible instead.
#[cfg(not(target_arch = "wasm32"))]
fn rock_visibility_range() -> VisibilityRange {
    VisibilityRange {
        start_margin: 0.0..0.0,
        end_margin: LOD_FAR..(LOD_FAR + LOD_FADE),
        use_aabb: false,
    }
}

#[allow(clippy::too_many_arguments)]
fn regenerate_obstacle_field(
    mut events: MessageReader<RegenerateField>,
    mode: Res<ObstacleFieldMode>,
    spec: Res<ObstacleFieldSpec>,
    existing: Query<Entity, With<ObstacleFieldRoot>>,
    grids: Query<Entity, With<WorldGrid>>,
    mut hold: ResMut<PhysicsHold>,
    mut heights: ResMut<ObstacleFieldHeights>,
    mut commands: Commands,
    // Optional so the headless server (no render → no asset collections) still
    // builds the colliders. Physics is then identical on server + client; only
    // the client adds visuals. This keeps networked rover runs deterministic.
    meshes: Option<ResMut<Assets<Mesh>>>,
    materials: Option<ResMut<Assets<StandardMaterial>>>,
) {
    // In DEM-delegated mode the DEM terrain owns crater/rock generation from the
    // shared spec (see `lunco-terrain-surface`), so the flat slab must NOT build —
    // it would float on / z-fight the DEM. Leave the `RegenerateField` events for
    // the terrain crate's reader (cursors are per-system, so not consuming here is
    // correct). Tear down any stray slab from a prior Standalone run first.
    if !mode.is_standalone() {
        events.clear();
        for e in &existing {
            commands.entity(e).despawn();
        }
        return;
    }
    if events.is_empty() {
        return;
    }
    // Defer until the big_space world grid exists (created in setup_sandbox);
    // leave the message unread so we retry next frame.
    let Ok(grid_entity) = grids.single() else {
        return;
    };
    events.clear();

    // Freeze physics across the collider tear-down/rebuild so rovers don't fall
    // through the momentarily-absent ground (manage_physics_hold restores it).
    hold.frames_left = PHYSICS_HOLD_FRAMES;

    // Tear down any previous field.
    for e in &existing {
        commands.entity(e).despawn();
    }

    let mut meshes = meshes;
    let mut materials = materials;
    let render = meshes.is_some() && materials.is_some();

    let h = spec.region_half_extent;
    let res = spec.grid_resolution as usize;

    // --- Craters → heightfield ------------------------------------------------
    let crater_placements = if spec.craters.enabled {
        sample_layer(
            spec.seed,
            salt::CRATERS,
            spec.pattern,
            h,
            spec.count_for_density(spec.craters.density),
            spec.craters.size,
            0.0,
        )
    } else {
        Vec::new()
    };
    let grid = build_height_grid(res, h, &crater_placements, &spec.craters);

    let collider = Collider::heightfield(
        grid.to_avian_heights(),
        DVec3::new((2.0 * h) as f64, 1.0, (2.0 * h) as f64),
    );

    // Anchor the field into the big_space world grid at the origin cell. A
    // ±200 m field fits inside one 2000 m cell, so children need only ordinary
    // Transforms relative to this root.
    let root = commands
        .spawn((
            ObstacleFieldRoot,
            Name::new("ObstacleField"),
            CellCoord::default(),
            GridAnchor,
            ChildOf(grid_entity),
            Transform::IDENTITY,
            Visibility::Inherited,
        ))
        .id();

    // Terrain: collider always; visual mesh only when render assets exist.
    let mut terrain = commands.spawn((
        FieldTerrain,
        Name::new("ObstacleField/Terrain"),
        ChildOf(root),
        Transform::IDENTITY,
        RigidBody::Static,
        collider,
    ));
    if let (Some(meshes), Some(materials)) = (meshes.as_mut(), materials.as_mut()) {
        let crate::field::MeshData { positions, normals, uvs, indices } = grid.to_mesh_data();
        let mesh = meshes.add(grid_mesh(positions, normals, uvs, indices));
        let material = materials.add(StandardMaterial {
            base_color: Color::srgb(0.32, 0.30, 0.28),
            perceptual_roughness: 1.0,
            ..default()
        });
        terrain.insert((Mesh3d(mesh), MeshMaterial3d(material)));
    }

    // --- Rocks → bucketed scatter --------------------------------------------
    let mut rock_count = 0;
    if spec.rocks.enabled {
        let placements = sample_layer(
            spec.seed,
            salt::ROCKS,
            spec.pattern,
            h,
            spec.count_for_density(spec.rocks.density),
            spec.rocks.size,
            spec.rocks.dynamic_fraction,
        );
        rock_count = placements.len();

        // Shared per-bucket visual assets (client only). Each bucket gets several
        // faceted boulder shapes (merged cubes); instances pick one by position.
        let buckets = bucket_sizes(spec.rocks.size.min, spec.rocks.size.max, ROCK_BUCKETS);
        let visuals = match (meshes.as_mut(), materials.as_mut()) {
            (Some(meshes), Some(materials)) => {
                let mut rock_meshes: Vec<Handle<Mesh>> =
                    Vec::with_capacity(buckets.len() * ROCK_VARIANTS);
                for (bi, &r) in buckets.iter().enumerate() {
                    for v in 0..ROCK_VARIANTS {
                        let mesh_seed = spec.seed
                            ^ 0x9E37_79B9_7F4A_7C15u64
                            ^ ((bi as u64) << 8)
                            ^ ((v as u64) << 20);
                        rock_meshes.push(meshes.add(faceted_rock_mesh(mesh_seed, 4 + v, r)));
                    }
                }
                let rock_material = materials.add(StandardMaterial {
                    base_color: Color::srgb(0.22, 0.21, 0.20),
                    perceptual_roughness: 1.0,
                    // Generated facets aren't guaranteed outward-wound; render both
                    // sides so no triangles drop out.
                    double_sided: true,
                    cull_mode: None,
                    ..default()
                });
                Some((rock_meshes, rock_material))
            }
            _ => None,
        };

        for p in &placements {
            let y = grid.height_at(p.pos.x, p.pos.y);

            // Phase 1: every rock is static collidable decoration. The `dynamic`
            // flag is carried for phase 2 (PredictedDynamic pushables). The
            // collider entity stays UNSCALED — avian applies Transform scale to
            // colliders, so the per-bucket visual scale goes on a child instead
            // (otherwise the sphere collider would be double-sized). `Visibility`
            // keeps the visual child's inheritance chain intact (else B0004).
            let rock = commands
                .spawn((
                    FieldRock,
                    ChildOf(root),
                    Transform::from_xyz(p.pos.x, y, p.pos.y)
                        .with_rotation(Quat::from_rotation_y(p.yaw)),
                    Visibility::Inherited,
                    RigidBody::Static,
                    Collider::sphere(p.size as f64),
                ))
                .id();

            if let Some((rock_meshes, rock_material)) = &visuals {
                let bi = bucket_index(p.size, &buckets);
                // Pick a faceted variant deterministically from position.
                let variant =
                    (p.pos.x.to_bits() ^ p.pos.y.to_bits().rotate_left(16)) as usize % ROCK_VARIANTS;
                let scale = p.size / buckets[bi];
                let rock_child = commands.spawn((
                    Mesh3d(rock_meshes[bi * ROCK_VARIANTS + variant].clone()),
                    MeshMaterial3d(rock_material.clone()),
                    Transform::from_scale(Vec3::splat(scale)),
                    ChildOf(rock),
                )).id();
                // Distance LOD cull — native only (see `rock_visibility_range`).
                #[cfg(not(target_arch = "wasm32"))]
                commands.entity(rock_child).insert(rock_visibility_range());
                #[cfg(target_arch = "wasm32")]
                let _ = rock_child;
            }
        }
    }

    // Hand the new grid to `reseat_bodies` (chained next), keeping the previous
    // one so it can shift resting bodies by the local terrain delta.
    heights.previous = heights.current.take();
    heights.current = Some(grid);
    heights.reseat_pending = true;

    info!(
        "obstacle field: {} craters, {} rocks ({}m region, seed {:#x}, render={})",
        crater_placements.len(),
        rock_count,
        spec.region_size(),
        spec.seed,
        render
    );
}

/// Re-seat resting bodies onto the freshly-rebuilt surface. GENERAL + AUTOMATIC
/// for every entity type:
/// - An **articulated assembly** (an [`ArticulatedVehicle`] root plus its child
///   bodies) is shifted as ONE rigid unit by a single delta computed at the root.
///   Shifting its links by *differing* amounts would make the joints explode and
///   fling the whole thing — the bug this fixes.
/// - Any **standalone** dynamic body (prop, ball, debris, single-body rover) is
///   shifted by its own local delta.
///
/// Delta = `current − previous` surface height at the body's (x,z), preserving
/// each body's clearance. Physics is frozen (`PhysicsHold`) while this runs, so
/// bodies settle cleanly on resume. Avian stores dynamic pose in `Position`; we
/// shift `Transform` too so the frozen visual doesn't snap on resume.
///
/// Frame note: avian `Position` is world-space in the current single big_space
/// cell (the field sits at the origin cell), so sampling `height_at(x,z)` is
/// correct here; `MAX_RESEAT_SHIFT` guards against any bad sample.
#[allow(clippy::type_complexity)]
fn reseat_bodies(
    mut heights: ResMut<ObstacleFieldHeights>,
    children_q: Query<&Children>,
    mut bodies: Query<
        (
            Entity,
            &mut Position,
            &mut Transform,
            &mut LinearVelocity,
            &mut AngularVelocity,
            &RigidBody,
            Has<ArticulatedVehicle>,
        ),
        (Without<FieldRock>, Without<FieldTerrain>),
    >,
) {
    if !heights.reseat_pending {
        return;
    }
    heights.reseat_pending = false;
    let Some(current) = heights.current.as_ref() else {
        return;
    };
    let previous = heights.previous.as_ref();
    let h = current.half_extent;

    // Local terrain delta at world (x,z); None outside the field, for a
    // negligible change, or an implausible (guarded) jump.
    let delta_at = |x: f32, z: f32| -> Option<f64> {
        if x.abs() > h || z.abs() > h {
            return None;
        }
        let new_g = current.height_at(x, z);
        let old_g = previous.map(|g| g.height_at(x, z)).unwrap_or(0.0);
        let d = (new_g - old_g) as f64;
        if d.abs() < 1.0e-3 || d.abs() > MAX_RESEAT_SHIFT {
            None
        } else {
            Some(d)
        }
    };

    // Pass 1: claim every articulated group (root + all descendant bodies) with a
    // single shared delta keyed at the root — so the assembly moves rigidly and
    // its links are never shifted individually (even when the root's delta is 0).
    let mut group_delta: std::collections::HashMap<Entity, f64> = std::collections::HashMap::new();
    for (e, pos, _, _, _, _, is_root) in bodies.iter() {
        if !is_root {
            continue;
        }
        let d = delta_at(pos.0.x as f32, pos.0.z as f32).unwrap_or(0.0);
        group_delta.insert(e, d);
        let mut stack = vec![e];
        while let Some(cur) = stack.pop() {
            if let Ok(children) = children_q.get(cur) {
                for &c in children {
                    group_delta.insert(c, d);
                    stack.push(c);
                }
            }
        }
    }

    // Pass 2: apply. Grouped bodies use their group's shared delta; standalone
    // dynamic bodies use their own. Velocity is zeroed only for bodies we move.
    let mut moved = 0u32;
    for (e, mut pos, mut tf, mut lin, mut ang, rb, _) in bodies.iter_mut() {
        let delta = match group_delta.get(&e) {
            Some(&d) => d,
            None if matches!(rb, RigidBody::Dynamic) => {
                delta_at(pos.0.x as f32, pos.0.z as f32).unwrap_or(0.0)
            }
            None => 0.0,
        };
        if delta.abs() < 1.0e-6 {
            continue;
        }
        pos.0.y += delta;
        tf.translation.y += delta as f32;
        lin.0 = DVec3::ZERO;
        ang.0 = DVec3::ZERO;
        moved += 1;
    }
    if moved > 0 {
        info!("obstacle field: re-seated {moved} bodies onto new terrain");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

    /// The core robustness property: an articulated rover moves as ONE rigid unit.
    /// A wheel at a spot where the local terrain DIDN'T change must still shift by
    /// the ROOT's delta (not its own 0) — otherwise the joints explode and fling
    /// the rover. A standalone body at the same spot must NOT move (own delta 0).
    #[test]
    fn articulated_rover_shifts_as_one_unit() {
        let mut world = World::new();

        // previous = flat; current raises only the column nearest x=0 by +2 m, so
        // height_at(0,0)=+2 but height_at(25,0)=0 (9 samples over [-50,50] →
        // 12.5 m spacing; x=0 → index 4, x=25 → index 6).
        let prev = HeightGrid::new_flat(9, 50.0);
        let mut cur = HeightGrid::new_flat(9, 50.0);
        for z in 0..9 {
            cur.heights[z * 9 + 4] = 2.0;
        }
        assert!((cur.height_at(0.0, 0.0) - 2.0).abs() < 1e-3);
        assert!(cur.height_at(25.0, 0.0).abs() < 1e-3);

        world.insert_resource(ObstacleFieldHeights {
            current: Some(cur),
            previous: Some(prev),
            reseat_pending: true,
        });

        // Articulated rover: root at x=0 (delta +2), wheel child at x=25 (own delta 0).
        let wheel = world
            .spawn((
                Position(DVec3::new(25.0, 5.0, 0.0)),
                Transform::from_xyz(25.0, 5.0, 0.0),
                LinearVelocity(DVec3::splat(3.0)),
                AngularVelocity(DVec3::splat(3.0)),
                RigidBody::Dynamic,
            ))
            .id();
        let root = world
            .spawn((
                ArticulatedVehicle,
                Position(DVec3::new(0.0, 5.0, 0.0)),
                Transform::from_xyz(0.0, 5.0, 0.0),
                LinearVelocity(DVec3::splat(3.0)),
                AngularVelocity(DVec3::splat(3.0)),
                RigidBody::Dynamic,
            ))
            .id();
        world.entity_mut(root).add_child(wheel);

        // Standalone body at the SAME spot as the wheel — must stay put (own delta 0).
        let ball = world
            .spawn((
                Position(DVec3::new(25.0, 5.0, 0.0)),
                Transform::from_xyz(25.0, 5.0, 0.0),
                LinearVelocity(DVec3::splat(9.0)),
                AngularVelocity(DVec3::ZERO),
                RigidBody::Dynamic,
            ))
            .id();

        world.run_system_once(reseat_bodies).unwrap();

        // Root and wheel BOTH rise by the root's +2 (rigid unit), not their own.
        assert!((world.get::<Position>(root).unwrap().0.y - 7.0).abs() < 1e-6);
        assert!((world.get::<Position>(wheel).unwrap().0.y - 7.0).abs() < 1e-6, "wheel must use root delta");
        // Transform shifted too (visual stays in sync while frozen).
        assert!((world.get::<Transform>(wheel).unwrap().translation.y - 7.0).abs() < 1e-4);
        // Moved bodies have velocity zeroed.
        assert_eq!(world.get::<LinearVelocity>(root).unwrap().0, DVec3::ZERO);
        assert_eq!(world.get::<AngularVelocity>(wheel).unwrap().0, DVec3::ZERO);

        // Standalone ball at x=25 did NOT move (own delta 0) and kept its velocity.
        assert!((world.get::<Position>(ball).unwrap().0.y - 5.0).abs() < 1e-6, "free body should not move");
        assert_eq!(world.get::<LinearVelocity>(ball).unwrap().0, DVec3::splat(9.0));

        // reseat_pending consumed.
        assert!(!world.resource::<ObstacleFieldHeights>().reseat_pending);
    }
}

/// Convenience: a denser/larger preset for stress-testing rover mobility.
pub fn stress_spec(seed: u64) -> ObstacleFieldSpec {
    ObstacleFieldSpec {
        seed,
        region_half_extent: 150.0,
        pattern: Pattern::Clustered { clusters: 8, spread: 25.0 },
        ..default()
    }
}
