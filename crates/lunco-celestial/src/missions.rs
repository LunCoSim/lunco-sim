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
}

#[derive(Component)]
pub struct Spacecraft {
    pub ephemeris_id: i32,
    pub reference_id: i32,
    pub start_epoch_jd: Option<f64>,
    pub end_epoch_jd: Option<f64>,
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
                                    color: Color::srgba(traj.color[0], traj.color[1], traj.color[2], traj.color[3]),
                                    is_visible: true,
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
                            // Instead of a tiny realistic model, we use a giant UI marker sphere (100km radius)
                            // so it's always clearly visible from anywhere in the Earth-Moon system.
                            let marker_mesh = meshes.add(Sphere::new(100_000.0).mesh()); 
                            let marker_mat = materials.add(StandardMaterial {
                                base_color: Color::srgb(0.0, 1.0, 1.0),
                                emissive: LinearRgba::from(Color::srgb(0.0, 10.0, 10.0)),
                                unlit: true,
                                ..default()
                            });

                            let mut ent = commands.spawn((
                                Name::new(sc.name.clone()),
                                Spacecraft {
                                    ephemeris_id: sc.ephemeris_id,
                                    reference_id: sc.reference_id,
                                    start_epoch_jd: sc.start_epoch_jd,
                                    end_epoch_jd: sc.end_epoch_jd,
                                },
                                Mesh3d(marker_mesh), 
                                MeshMaterial3d(marker_mat),
                                Transform::from_scale(Vec3::splat(sc.scale)),
                                GlobalTransform::default(),
                                Visibility::default(),
                                CellCoord::default(),
                            ));
                            
                            if sc.focus_on_start {
                                ent.insert(FocusOnStart);
                            }
                        }
                        
                        registry.missions.push(mission);
                    }
                }
            }
        }
    }
}

pub fn spacecraft_visibility_system(
    clock: Res<crate::clock::CelestialClock>,
    mut q_sc: Query<(&Spacecraft, &mut Visibility)>,
) {
    for (sc, mut vis) in q_sc.iter_mut() {
        if let (Some(start), Some(end)) = (sc.start_epoch_jd, sc.end_epoch_jd) {
            let should_be_visible = clock.epoch >= start && clock.epoch <= end;
            let target_vis = if should_be_visible { Visibility::Inherited } else { Visibility::Hidden };
            if *vis != target_vis {
                *vis = target_vis;
            }
        }
    }
}


#[derive(Component)]
pub struct FocusOnStart;

pub fn mission_focus_system(
    q_focus: Query<Entity, Added<FocusOnStart>>,
    mut q_camera: Query<&mut crate::camera::ObserverCamera>,
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
