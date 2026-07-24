//! Missions as SCENE-DECLARED content (doc 49), not as "anything found on disk".
//!
//! A mission used to be loaded because `assets/missions/*.json` existed and the
//! scene happened to declare a sky. That made "has a Moon" mean "also gets
//! Artemis II", so a lunar-landing FILM silently had a crewed Orion trajectory
//! spawned into it. Declaring a body and requesting a specific mission are
//! different statements, and only the scene can make the second one.
//!
//! So a mission is now opted into exactly the way a sky is: the scene REFERENCES
//! the mission's USD file, that file's prims carry `lunco:mission:*` /
//! `lunco:trajectory:*` / `lunco:spacecraft:*`, and `lunco-usd-sim` projects them
//! onto the declaration components below. No mission prim ⇒ no mission. There is
//! no filesystem scan and no implicit set.

use crate::trajectories::{TrajectoryFrame, TrajectoryPath, TrajectoryView};
use bevy::prelude::*;
use big_space::prelude::CellCoord;
use lunco_render::{PbrLook, WorldLabel};

/// Ids of the missions spawned into the current scene. Diagnostic/UI only — the
/// authority is the declaration components, which live on the USD prim entities
/// and are torn down with them.
#[derive(Debug, Resource, Default)]
pub struct MissionRegistry {
    pub missions: Vec<String>,
}

/// A scene-authored declaration that this mission should be shown — the ECS
/// projection of USD's `LunCoMissionAPI`.
///
/// **This is the switch that turns a mission on**, exactly as
/// [`CelestialBodyDecl`](crate::CelestialBodyDecl) is the switch for the sky, and
/// it is deliberately a SEPARATE switch: a scene that wants the Moon has not
/// thereby asked for Artemis II.
#[derive(Component, Debug, Clone)]
pub struct MissionDecl {
    /// Stable mission id (`"artemis-2"`).
    pub id: String,
    /// Display name (`"Artemis II"`).
    pub name: String,
    /// One-line human description.
    pub description: String,
}

/// One trajectory a mission asks to be drawn — projection of
/// `LunCoMissionTrajectoryAPI`.
///
/// Every field here is VISUALISATION config. The state vectors are not in USD and
/// never were: the curve is sampled at runtime from the ephemeris provider using
/// `tracked_id` / `reference_id`, so this prim says *how to draw* a trajectory,
/// not *where the spacecraft is*.
#[derive(Component, Debug, Clone)]
pub struct MissionTrajectoryDecl {
    pub name: String,
    pub tracked_id: i32,
    pub reference_id: i32,
    pub color: [f32; 4],
    pub sampling_days: f64,
    pub sampling_step: f64,
    /// `"BodyFixed"` or `"Inertial"`.
    pub frame: String,
    pub user_visible: Option<bool>,
    pub start_epoch_jd: Option<f64>,
    pub end_epoch_jd: Option<f64>,
}

/// The mission's spacecraft marker — projection of `LunCoMissionSpacecraftAPI`.
#[derive(Component, Debug, Clone)]
pub struct MissionSpacecraftDecl {
    pub name: String,
    pub ephemeris_id: i32,
    pub reference_id: i32,
    pub scale: f32,
    pub start_epoch_jd: Option<f64>,
    pub end_epoch_jd: Option<f64>,
    pub marker_radius_km: Option<f32>,
    pub hit_radius_km: Option<f32>,
    pub marker_color: Option<[f32; 4]>,
}

/// Stamped on a declaring prim entity once its trajectories/spacecraft have been
/// spawned, so the spawner is idempotent.
///
/// Entity-scoped ON THE DECLARATION, not a `Local` bool and not a global "already
/// ran" flag: a `Local` can never be reset, so a scene reload — which despawns
/// these prim entities and re-creates them — would never re-spawn the mission.
/// Because the marker dies with the declaration, teardown-then-reload just works.
#[derive(Component, Debug, Clone, Copy)]
pub struct MissionSpawned;

use lunco_core::Spacecraft;

#[derive(Component)]
pub struct SpacecraftBillboard;

pub fn spacecraft_billboard_system(
    mut q_billboards: Query<(&mut Transform, &ChildOf), With<SpacecraftBillboard>>,
    q_camera: Query<&GlobalTransform, (With<Camera>, With<lunco_core::Avatar>)>,
    q_global: Query<&GlobalTransform>,
) {
    if let Some(cam_gtf) = q_camera.iter().next() {
        let cam_rot = cam_gtf.compute_transform().rotation;
        for (mut tf, child_of) in q_billboards.iter_mut() {
            // To make a child face the camera in global space, we need to cancel out parent rotation
            if let Ok(p_gtf) = q_global.get(child_of.parent()) {
                let p_rot = p_gtf.compute_transform().rotation;
                tf.rotation = p_rot.inverse() * cam_rot;
            } else {
                tf.rotation = cam_rot;
            }
        }
    }
}

pub struct MissionPlugin;

impl Plugin for MissionPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MissionRegistry>();
        // A mission spawns because THIS SCENE declared it, full stop. The old gate
        // was `celestial bodies declared && registry empty`, i.e. "has a sky" — so
        // every lunar scene got every mission in `assets/missions/`, and a landing
        // film silently acquired Artemis II. Declaring a body and requesting a
        // mission are different statements; only the second one may spawn a mission.
        //
        // No filesystem scan survives here: the query below is empty unless a prim
        // carries `MissionTrajectoryDecl`/`MissionSpacecraftDecl`, which only the
        // USD bridge writes and only for an authored `lunco:mission:*` prim.
        app.add_systems(Update, spawn_declared_missions);
        app.add_systems(
            Update,
            (
                update_spacecraft_position_system,
                spacecraft_alignment_system,
                spacecraft_visibility_system,
                spacecraft_billboard_system,
            )
                .chain(),
        );
    }
}

/// Spawn the trajectories and spacecraft the loaded scene DECLARED.
///
/// Runs every frame and does nothing at all unless a prim carries a declaration
/// component, which only the USD bridge writes and only for an authored
/// `lunco:mission:*` prim. There is no `assets/missions` scan: a mission arrives
/// because a scene referenced its USD file, the same way a sky arrives because a
/// scene referenced `solar_system.usda`.
///
/// The `Without<MissionSpawned>` filter is the idempotency guard, and it is
/// scoped to the DECLARING ENTITY rather than being a `Local` bool or a global
/// "already ran" flag — a `Local` can never be reset, so a scene reload (which
/// despawns these prim entities) would never re-spawn the mission. The marker
/// dies with the declaration, so teardown-then-reload just works.
///
/// A prim may legitimately carry more than one declaration (a spacecraft prim
/// that is also the mission root). All three loops read the pre-stamp state
/// because `Commands` are deferred to the end of the system, so such a prim is
/// handled once by each loop in a single pass and skipped entirely thereafter.
pub fn spawn_declared_missions(
    mut commands: Commands,
    mut registry: ResMut<MissionRegistry>,
    mut meshes: ResMut<Assets<Mesh>>,
    q_missions: Query<(Entity, &MissionDecl), Without<MissionSpawned>>,
    q_trajectories: Query<(Entity, &MissionTrajectoryDecl), Without<MissionSpawned>>,
    q_spacecraft: Query<(Entity, &MissionSpacecraftDecl), Without<MissionSpawned>>,
) {
    for (decl_entity, mission) in q_missions.iter() {
        info!("[mission] scene declares {} ({})", mission.name, mission.id);
        registry.missions.push(mission.id.clone());
        commands.entity(decl_entity).try_insert(MissionSpawned);
    }

    for (decl_entity, traj) in q_trajectories.iter() {
        let frame = match traj.frame.as_str() {
            "BodyFixed" => TrajectoryFrame::BodyFixed,
            _ => TrajectoryFrame::Inertial,
        };

        commands.spawn((
            Name::new(traj.name.clone()),
            // Owned by the celestial subsystem — torn down on scene reload.
            crate::big_space_setup::CelestialDerived,
            TrajectoryView {
                tracked_id: traj.tracked_id,
                reference_id: traj.reference_id,
                frame,
                color: LinearRgba::from(Color::srgba(
                    traj.color[0],
                    traj.color[1],
                    traj.color[2],
                    traj.color[3],
                )),
                is_visible: true,
                // Unauthored ⇒ OFF. An orbit line is an ANALYSIS overlay: a
                // 400-day Earth ellipse drawn across a surface scene's sky is
                // not what "this world has an Earth" asked for. A scene that
                // wants the line says `lunco:trajectory:userVisible = true`,
                // and the UI toggle can still raise one at runtime.
                user_visible: traj.user_visible.unwrap_or(false),
                sampling_days: traj.sampling_days,
                sampling_step: traj.sampling_step,
                start_epoch: traj.start_epoch_jd,
                end_epoch: traj.end_epoch_jd,
            },
            TrajectoryPath::default(),
            Transform::default(),
            GlobalTransform::default(),
            Visibility::default(),
            // NO eager CellCoord: `trajectory_alignment_system` inserts
            // it atomically with the grid parent (doc 45 — a cell-entity
            // without a grid parent is class 2; the validator flags the
            // pre-parenting window).
        ));
        commands.entity(decl_entity).try_insert(MissionSpawned);
    }

    for (decl_entity, sc) in q_spacecraft.iter() {
        commands.entity(decl_entity).try_insert(MissionSpawned);

        let radius_m = sc.marker_radius_km.unwrap_or(500.0) * 1000.0;
        let hit_radius_m = sc.hit_radius_km.unwrap_or(1000.0) * 1000.0;

        let mut sc_ent = commands.spawn((
            Name::new(sc.name.clone()),
            crate::big_space_setup::CelestialDerived,
            Spacecraft {
                name: sc.name.clone(),
                ephemeris_id: sc.ephemeris_id,
                reference_id: sc.reference_id,
                start_epoch_jd: sc.start_epoch_jd,
                end_epoch_jd: sc.end_epoch_jd,
                hit_radius_m,
                user_visible: true,
            },
            Transform::from_scale(Vec3::splat(sc.scale)),
            GlobalTransform::default(),
            Visibility::default(),
            // NO eager CellCoord — `spacecraft_alignment_system` inserts
            // it together with the frame-grid parent (see above).
        ));

        sc_ent.with_children(|parent| {
            // Appearance is stated as INTENT (`PbrLook`); `lunco-render-bevy`
            // binds the `StandardMaterial`. Identical looks share one material,
            // so the two solar panels below cost one, not two.
            // Main Body (Service Module) - Darker metallic grey
            parent.spawn((
                Mesh3d(meshes.add(Cylinder::new(radius_m, radius_m * 1.5).mesh())),
                PbrLook {
                    base_color: LinearRgba::from(Color::srgb(0.2, 0.2, 0.2)),
                    metallic: 0.8,
                    perceptual_roughness: 0.2,
                    ..default()
                },
                Name::new("Service Module"),
            ));

            // Capsule (Command Module) - Silver metallic
            parent.spawn((
                Mesh3d(meshes.add(Cylinder::new(radius_m * 0.1, radius_m).mesh())),
                PbrLook {
                    base_color: LinearRgba::from(Color::srgb(0.8, 0.8, 0.8)),
                    metallic: 1.0,
                    perceptual_roughness: 0.1,
                    ..default()
                },
                Transform::from_translation(Vec3::Y * radius_m * 1.25),
                Name::new("Command Module"),
            ));

            // Solar Panels (Left and Right) - Blue solar look
            let panel_width = radius_m * 4.0;
            let panel_height = radius_m * 0.8;
            let panel_thickness = radius_m * 0.1;

            for side in [-1.0, 1.0] {
                parent.spawn((
                    Mesh3d(
                        meshes.add(Cuboid::new(panel_width, panel_height, panel_thickness).mesh()),
                    ),
                    PbrLook {
                        base_color: LinearRgba::from(Color::srgb(0.0, 0.1, 0.4)), // Dark blue solar cells
                        emissive: LinearRgba::new(0.0, 0.2, 0.8, 1.0) * 2.0,
                        metallic: 0.5,
                        perceptual_roughness: 0.3,
                        ..default()
                    },
                    Transform::from_translation(Vec3::X * side * (radius_m + panel_width * 0.5)),
                    Name::new(if side < 0.0 {
                        "Solar Panel Left"
                    } else {
                        "Solar Panel Right"
                    }),
                ));
            }

            // Billboard label, as INTENT. `Text2d` lives in `bevy_sprite`,
            // whose `bevy_sprite_render` feature pulls `bevy_render` → wgpu +
            // naga — and this one label was the last thing dragging the whole
            // GPU stack into the `--no-ui` server. The spacecraft's *name* is
            // simulation data and stays here; the glyphs are not, and
            // `lunco-render-bevy` builds them from `WorldLabel` in render
            // builds. See docs/architecture/render-decoupling.md.
            parent.spawn((
                SpacecraftBillboard,
                WorldLabel::new(sc.name.clone(), 100.0),
                Transform::from_translation(Vec3::Y * radius_m * 5.0),
            ));
        });
    }
}

pub fn update_spacecraft_position_system(
    world: Res<lunco_time::WorldTime>,
    ephemeris: Res<crate::ephemeris::EphemerisResource>,
    q_grids: Query<&big_space::prelude::Grid>,
    mut q_spacecraft: Query<(
        &Spacecraft,
        &mut Transform,
        Option<&mut CellCoord>,
        Option<&ChildOf>,
    )>,
) {
    let jd = world.epoch_jd;
    for (sc, mut tf, cell, child_of) in q_spacecraft.iter_mut() {
        // P8(d): a spacecraft whose ephemeris CSV failed to fetch used to be placed at its
        // reference body's centre — inside the Earth, looking exactly like a real position.
        // Now it simply is not moved.
        let (Some(p_target), Some(p_ref)) = (
            ephemeris.provider.global_position(sc.ephemeris_id, jd),
            ephemeris.provider.global_position(sc.reference_id, jd),
        ) else {
            continue;
        };
        let rel_pos = crate::coords::ecliptic_to_bevy(p_target - p_ref).raw();

        // Split through the parent (reference) grid so the spacecraft stays
        // within one cell — precise placement instead of a raw f32 at up to
        // ~4e8 m (32 m ULP) for cislunar trajectories. `look_to` below only
        // sets rotation from a direction, so it is unaffected by the split.
        // The cell is Optional: it arrives one frame after spawn, together
        // with the grid parent (spacecraft_alignment_system) — until then the
        // pose is a raw f32, matching the no-grid fallback.
        match (cell, child_of.and_then(|c| q_grids.get(c.parent()).ok())) {
            (Some(mut cell), Some(grid)) => {
                let (new_cell, new_translation) = grid.translation_to_grid(rel_pos);
                tf.translation = new_translation;
                if *cell != new_cell {
                    *cell = new_cell;
                }
            }
            (cell, _) => {
                tf.translation = rel_pos.as_vec3();
                // A stale non-zero cell would still compose into the pose.
                if let Some(mut cell) = cell {
                    if *cell != CellCoord::default() {
                        *cell = CellCoord::default();
                    }
                }
            }
        }

        // Point solar panels towards the Sun
        // Sun ID is 10
        let Some(p_sun) = ephemeris.provider.global_position(10, jd) else {
            continue;
        };
        let to_sun = crate::coords::ecliptic_to_bevy(p_sun - p_target)
            .raw()
            .as_vec3()
            .normalize_or_zero();
        if to_sun.length_squared() > 0.0 {
            // Bevy's look_to makes Local -Z point at the target.
            // Our panels are in the XY plane (width X, height Y), so they face +Z and -Z.
            // Pointing -Z at the sun ensures the panels are oriented correctly.
            tf.look_to(to_sun, Vec3::Y);
        }
    }
}

pub fn spacecraft_alignment_system(
    mut commands: Commands,
    q_frames: Query<(
        Entity,
        &crate::registry::CelestialReferenceFrame,
        Has<big_space::prelude::Grid>,
    )>,
    q_sc: Query<(Entity, &Spacecraft, Option<&ChildOf>)>,
    q_children: Query<&Children>,
) {
    for (sc_entity, sc, current_parent) in q_sc.iter() {
        for (f_entity, frame, frame_is_grid) in q_frames.iter() {
            if frame.ephemeris_id == sc.reference_id {
                let is_current_parent = if let Some(p) = current_parent {
                    p.parent() == f_entity
                } else {
                    false
                };

                if !is_current_parent {
                    // Spacecraft here are NOT `GridAnchor`s, so the atomic-
                    // migration contract doesn't apply; `set_parent_in_place`'s
                    // Transform clobber self-heals next frame when
                    // `update_spacecraft_position_system` rewrites the pose.
                    #[allow(clippy::disallowed_methods)]
                    commands.entity(sc_entity).set_parent_in_place(f_entity);
                    // The cell arrives WITH the grid parent (doc 45: a
                    // cell-entity must be a direct grid child — spawning with
                    // an eager CellCoord tripped the validator in the
                    // pre-parenting window). Frames without a Grid get no
                    // cell; the position system falls back to raw f32 there.
                    if frame_is_grid {
                        commands.entity(sc_entity).try_insert(CellCoord::default());
                        // Re-stamp the mesh/billboard children as low-precision
                        // subtree roots: big_space strips the marker while the
                        // spacecraft is still an invalid parent (pre-cell), and
                        // never re-tags without a child-side trigger — leaving
                        // their GlobalTransforms unowned (same trap as the
                        // trajectory meshes in trajectories.rs).
                        if let Ok(children) = q_children.get(sc_entity) {
                            for child in children.iter() {
                                commands
                                    .entity(child)
                                    .try_insert(big_space::grid::propagation::LowPrecisionRoot);
                            }
                        }
                    }
                }
                break;
            }
        }
    }
}

pub fn spacecraft_visibility_system(
    world: Res<lunco_time::WorldTime>,
    mut q_sc: Query<(&Spacecraft, &mut Visibility)>,
) {
    for (sc, mut vis) in q_sc.iter_mut() {
        let mut mission_visible = true;
        if let (Some(start), Some(end)) = (sc.start_epoch_jd, sc.end_epoch_jd) {
            mission_visible = world.epoch_jd >= start && world.epoch_jd <= end;
        }

        let final_visible = mission_visible && sc.user_visible;
        let target_vis = if final_visible {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        if *vis != target_vis {
            *vis = target_vis;
        }
    }
}
