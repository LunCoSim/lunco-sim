use bevy::prelude::*;
use serde::Deserialize;
use std::fs;
use crate::trajectories::{TrajectoryView, TrajectoryPath, TrajectoryFrame};
use big_space::prelude::CellCoord;

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
    #[serde(default)]
    pub focus_on_start: bool,
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
    q_camera: Query<&GlobalTransform, With<lunco_camera::ObserverCamera>>,
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
        app.add_systems(Startup, load_missions_system);
        app.add_systems(Update, (
            update_spacecraft_position_system,
            spacecraft_alignment_system,
            mission_focus_system,
            spacecraft_visibility_system,
            spacecraft_billboard_system,
        ).chain());
    }
}

pub fn load_missions_system(mut commands: Commands, mut registry: ResMut<MissionRegistry>, mut meshes: ResMut<Assets<Mesh>>, mut materials: ResMut<Assets<StandardMaterial>>) {
    let missions_dir = "assets/missions";
    if let Ok(entries) = fs::read_dir(missions_dir) {
        for entry in entries.flatten() {
            if entry.path().extension().map(|e| e == "json").unwrap_or(false) {
                if let Ok(content) = fs::read_to_string(entry.path()) {
                    if let Ok(mission) = serde_json::from_str::<MissionData>(&content) {
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
                                CellCoord::default(),
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
                                CellCoord::default(),
                            ));

                            sc_ent.with_children(|parent| {
                                // Main Body (Service Module) - Darker metallic grey
                                parent.spawn((
                                    Mesh3d(meshes.add(Cylinder::new(radius_m, radius_m * 1.5).mesh())),
                                    MeshMaterial3d(materials.add(StandardMaterial {
                                        base_color: Color::srgb(0.2, 0.2, 0.2),
                                        metallic: 0.8,
                                        perceptual_roughness: 0.2,
                                        ..default()
                                    })),
                                    Name::new("Service Module"),
                                ));

                                // Capsule (Command Module) - Silver metallic
                                parent.spawn((
                                    Mesh3d(meshes.add(Cylinder::new(radius_m * 0.1, radius_m).mesh())),
                                    MeshMaterial3d(materials.add(StandardMaterial {
                                        base_color: Color::srgb(0.8, 0.8, 0.8),
                                        metallic: 1.0,
                                        perceptual_roughness: 0.1,
                                        ..default()
                                    })),
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
                                        MeshMaterial3d(materials.add(StandardMaterial {
                                            base_color: Color::srgb(0.0, 0.1, 0.4), // Dark blue solar cells
                                            emissive: LinearRgba::new(0.0, 0.2, 0.8, 1.0) * 2.0,
                                            metallic: 0.5,
                                            perceptual_roughness: 0.3,
                                            ..default()
                                        })),
                                        Transform::from_translation(Vec3::X * side * (radius_m + panel_width * 0.5)),
                                        Name::new(if side < 0.0 { "Solar Panel Left" } else { "Solar Panel Right" }),
                                    ));
                                }

                                // Billboard Label
                                parent.spawn((
                                    SpacecraftBillboard,
                                    Text2d::new(sc.name.clone()),
                                    TextFont {
                                        font_size: 100.0,
                                        ..default()
                                    },
                                    TextColor(Color::WHITE),
                                    Transform::from_translation(Vec3::Y * radius_m * 5.0),
                                ));
                            });
                            
                            if sc.focus_on_start {
                                sc_ent.insert(FocusOnStart);
                            }
                        }
                        
                        registry.missions.push(mission);
                    }
                }
            }
        }
    }
}

pub fn update_spacecraft_position_system(
    clock: Res<crate::clock::CelestialClock>,
    ephemeris: Res<crate::ephemeris::EphemerisResource>,
    mut q_spacecraft: Query<(&Spacecraft, &mut Transform, &mut CellCoord)>,
) {
    let jd = clock.epoch;
    for (sc, mut tf, mut cell) in q_spacecraft.iter_mut() {
        let p_target = ephemeris.provider.global_position(sc.ephemeris_id, jd);
        let p_ref = ephemeris.provider.global_position(sc.reference_id, jd);
        let rel_pos = crate::coords::ecliptic_to_bevy(p_target - p_ref);
        
        tf.translation = rel_pos.as_vec3(); 
        *cell = CellCoord::default(); 

        // Point solar panels towards the Sun
        // Sun ID is 10
        let p_sun = ephemeris.provider.global_position(10, jd);
        let to_sun = crate::coords::ecliptic_to_bevy(p_sun - p_target).as_vec3().normalize_or_zero();
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
    q_frames: Query<(Entity, &crate::registry::CelestialReferenceFrame)>,
    q_sc: Query<(Entity, &Spacecraft, Option<&ChildOf>)>,
) {
    for (sc_entity, sc, current_parent) in q_sc.iter() {
        for (f_entity, frame) in q_frames.iter() {
            if frame.ephemeris_id == sc.reference_id {
                let is_current_parent = if let Some(p) = current_parent {
                    p.parent() == f_entity
                } else {
                    false
                };
                
                if !is_current_parent {
                    commands.entity(sc_entity).set_parent_in_place(f_entity); 
                }
                break;
            }
        }
    }
}

#[derive(Component)]
pub struct FocusOnStart;

pub fn mission_focus_system(
    q_focus: Query<Entity, Added<FocusOnStart>>,
    mut q_camera: Query<&mut lunco_camera::ObserverCamera>,
    mut commands: Commands,
) {
    for ent in q_focus.iter() {
        if let Some(mut obs) = q_camera.iter_mut().next() {
            obs.focus_target = Some(ent);
            obs.distance = 100.0; // Close focus for spacecraft
            info!("Camera focused on spacecraft starting mission.");
        }
        commands.entity(ent).remove::<FocusOnStart>();
    }
}

pub fn spacecraft_visibility_system(
    clock: Res<crate::clock::CelestialClock>,
    mut q_sc: Query<(&Spacecraft, &mut Visibility)>,
) {
    for (sc, mut vis) in q_sc.iter_mut() {
        let mut mission_visible = true;
        if let (Some(start), Some(end)) = (sc.start_epoch_jd, sc.end_epoch_jd) {
            mission_visible = clock.epoch >= start && clock.epoch <= end;
        }
        
        let final_visible = mission_visible && sc.user_visible;
        let target_vis = if final_visible { Visibility::Inherited } else { Visibility::Hidden };
        if *vis != target_vis {
            *vis = target_vis;
        }
    }
}
