use bevy::prelude::*;
use lunco_render::{PbrLook, WorldLabel};
use serde::Deserialize;
use std::fs;
use crate::trajectories::{TrajectoryView, TrajectoryPath, TrajectoryFrame};
use big_space::prelude::CellCoord;
use lunco_assets::assets_dir;

#[derive(Debug, Deserialize, Resource, Default)]
pub struct MissionRegistry {
    pub missions: Vec<MissionData>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct MissionData {
    pub id: String,
    pub name: String,
    pub description: String,
    pub trajectories: Vec<MissionTrajectory>,
    pub spacecraft: Option<MissionSpacecraft>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct MissionTrajectory {
    pub name: String,
    pub tracked_id: i32,
    pub reference_id: i32,
    pub color: [f32; 4],
    pub sampling_days: f64,
    pub sampling_step: f64,
    pub frame: String,
    pub user_visible: Option<bool>,
    pub start_epoch_jd: Option<f64>,
    pub end_epoch_jd: Option<f64>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct MissionSpacecraft {
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
        // Missions belong to the solar-system context: gated on the celestial
        // hierarchy being wanted (doc 43 — the sandbox runs this plugin with
        // the hierarchy off by default), and re-armed if it enables later.
        app.add_systems(
            Update,
            load_missions_system.run_if(
                |config: Res<crate::CelestialConfig>,
                 mut loaded: bevy::prelude::Local<bool>| {
                    if !config.spawn_hierarchy || *loaded {
                        return false;
                    }
                    *loaded = true;
                    true
                },
            ),
        );
        app.add_systems(Update, (
            update_spacecraft_position_system,
            spacecraft_alignment_system,
            spacecraft_visibility_system,
            spacecraft_billboard_system,
        ).chain());
    }
}

pub fn load_missions_system(
    mut commands: Commands,
    mut registry: ResMut<MissionRegistry>,
    mut meshes: ResMut<Assets<Mesh>>,
    #[cfg(target_arch = "wasm32")] embedded: Option<Res<crate::embedded_assets::EmbeddedMissionData>>,
) {
    // Helper: process a single mission JSON string
    let spawn_mission = |commands: &mut Commands, meshes: &mut ResMut<Assets<Mesh>>, registry: &mut ResMut<MissionRegistry>, content: &str| {
        if let Ok(mission) = serde_json::from_str::<MissionData>(content) {
            info!("Loaded mission: {}", mission.name);

            // Spawn trajectories
            for traj in &mission.trajectories {
                let frame = match traj.frame.as_str() {
                    "BodyFixed" => TrajectoryFrame::BodyFixed,
                    _ => TrajectoryFrame::Inertial,
                };

                commands.spawn((
                    Name::new(traj.name.clone()),
                    TrajectoryView {
                        tracked_id: traj.tracked_id,
                        reference_id: traj.reference_id,
                        frame,
                        color: LinearRgba::from(Color::srgba(traj.color[0], traj.color[1], traj.color[2], traj.color[3])),
                        is_visible: true,
                        user_visible: traj.user_visible.unwrap_or(true),
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
            }

            // Spawn spacecraft
            if let Some(sc) = &mission.spacecraft {
                let radius_m = sc.marker_radius_km.unwrap_or(500.0) * 1000.0;
                let hit_radius_m = sc.hit_radius_km.unwrap_or(1000.0) * 1000.0;

                let mut sc_ent = commands.spawn((
                    Name::new(sc.name.clone()),
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
                            Mesh3d(meshes.add(Cuboid::new(panel_width, panel_height, panel_thickness).mesh())),
                            PbrLook {
                                base_color: LinearRgba::from(Color::srgb(0.0, 0.1, 0.4)), // Dark blue solar cells
                                emissive: LinearRgba::new(0.0, 0.2, 0.8, 1.0) * 2.0,
                                metallic: 0.5,
                                perceptual_roughness: 0.3,
                                ..default()
                            },
                            Transform::from_translation(Vec3::X * side * (radius_m + panel_width * 0.5)),
                            Name::new(if side < 0.0 { "Solar Panel Left" } else { "Solar Panel Right" }),
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

            registry.missions.push(mission);
        }
    };

    #[cfg(not(target_arch = "wasm32"))]
    {
        // Desktop: load from filesystem
        let missions_dir = assets_dir().join("missions");
        if let Ok(entries) = fs::read_dir(missions_dir) {
            for entry in entries.flatten() {
                if entry.path().extension().map(|e| e == "json").unwrap_or(false) {
                    // Read through lunco-storage (clippy-banned `std::fs::read_to_string`,
                    // wasm-incompatible). `fs::read_dir` above isn't on the ban list and the
                    // whole block is `cfg(not(wasm32))` — wasm uses the embedded data path.
                    use lunco_storage::Storage;
                    if let Ok(bytes) = lunco_storage::FileStorage::new()
                        .read_sync(&lunco_storage::StorageHandle::File(entry.path()))
                    {
                        if let Ok(content) = String::from_utf8(bytes) {
                            spawn_mission(&mut commands, &mut meshes, &mut registry, &content);
                        }
                    }
                }
            }
        }
    }

    #[cfg(target_arch = "wasm32")]
    {
        // Web: use embedded mission data
        if let Some(embedded) = embedded {
            spawn_mission(&mut commands, &mut meshes, &mut registry, &embedded.artemis_2);
        }
    }
}

pub fn update_spacecraft_position_system(
    world: Res<lunco_time::WorldTime>,
    ephemeris: Res<crate::ephemeris::EphemerisResource>,
    q_grids: Query<&big_space::prelude::Grid>,
    mut q_spacecraft: Query<(&Spacecraft, &mut Transform, Option<&mut CellCoord>, Option<&ChildOf>)>,
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
        let Some(p_sun) = ephemeris.provider.global_position(10, jd) else { continue };
        let to_sun = crate::coords::ecliptic_to_bevy(p_sun - p_target).raw().as_vec3().normalize_or_zero();
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
    q_frames: Query<(Entity, &crate::registry::CelestialReferenceFrame, Has<big_space::prelude::Grid>)>,
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
                        commands.entity(sc_entity).insert(CellCoord::default());
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
                                    .insert(big_space::grid::propagation::LowPrecisionRoot);
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
        let target_vis = if final_visible { Visibility::Inherited } else { Visibility::Hidden };
        if *vis != target_vis {
            *vis = target_vis;
        }
    }
}
