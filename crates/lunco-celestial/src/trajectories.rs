use bevy::prelude::*;
use bevy::tasks::{Task, AsyncComputeTaskPool};
use bevy::render::render_resource::PrimitiveTopology;
use bevy::asset::RenderAssetUsages;
use big_space::prelude::CellCoord;
use futures_lite::future;
use std::sync::Arc;
use crate::ephemeris::EphemerisResource;
use crate::clock::CelestialClock;
use crate::registry::{CelestialBodyRegistry, CelestialReferenceFrame};

use bevy::shader::ShaderRef;
use bevy::render::render_resource::AsBindGroup;
use bevy::math::cubic_splines::CubicCardinalSpline;
use bevy::pbr::{MaterialExtension, ExtendedMaterial};
use bevy::camera::visibility::NoFrustumCulling;

pub struct TrajectoryPlugin;

#[derive(Asset, AsBindGroup, TypePath, Debug, Clone, Copy)]
pub struct TrajectoryExtension {
    #[uniform(100)]
    pub color: LinearRgba,
    #[uniform(100)]
    pub time: f32,
    #[uniform(100)]
    pub pulse_pos: f32,
    #[uniform(100)]
    pub pulse_width: f32,
    #[uniform(100)]
    pub noise_scale: f32,
    #[uniform(100)]
    pub emissive_mult: f32,
}

impl MaterialExtension for TrajectoryExtension {
    fn fragment_shader() -> ShaderRef {
        "shaders/trajectory.wgsl".into()
    }
}

pub type TrajectoryMaterial = ExtendedMaterial<StandardMaterial, TrajectoryExtension>;

impl Default for TrajectoryExtension {
    fn default() -> Self {
        Self {
            color: LinearRgba::WHITE,
            time: 0.0,
            pulse_pos: 0.0,
            pulse_width: 0.05,
            noise_scale: 100.0,
            emissive_mult: 10.0,
        }
    }
}

#[derive(Component, Reflect, Clone, Copy, Debug)]
#[reflect(Component)]
pub struct TrajectoryView {
    pub tracked_id: i32,
    pub reference_id: i32,
    pub frame: TrajectoryFrame,
    pub color: LinearRgba,
    pub is_visible: bool, // Controlled by mission range logic
    pub user_visible: bool, // Controlled by UI checkbox
    pub sampling_days: f64,
    pub sampling_step: f64,
    pub start_epoch: Option<f64>,
    pub end_epoch: Option<f64>,
}

#[derive(Reflect, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum TrajectoryFrame {
    #[default]
    Inertial,
    BodyFixed,
}

impl Default for TrajectoryView {
    fn default() -> Self {
        Self {
            tracked_id: 399,
            reference_id: 10,
            frame: TrajectoryFrame::Inertial,
            color: LinearRgba::WHITE,
            is_visible: true,
            user_visible: true,
            sampling_days: 200.0,
            sampling_step: 1.0,
            start_epoch: None,
            end_epoch: None,
        }
    }
}

#[derive(Component, Default, Reflect)]
#[reflect(Component)]
pub struct TrajectoryPath {
    pub points: Vec<bevy::math::DVec3>,
    pub update_epoch: f64,
}

#[derive(Component)]
pub struct TrajectoryTask(pub Task<TrajectoryData>);

pub struct TrajectoryData {
    pub points: Vec<bevy::math::DVec3>,
    pub epoch: f64,
}

#[derive(Component)]
pub struct TrajectoryMeshMarker;

impl Plugin for TrajectoryPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<TrajectoryView>()
           .register_type::<TrajectoryFrame>()
           .register_type::<TrajectoryPath>();
           
        app.add_systems(Startup, trajectory_setup_system);
        
        app.add_systems(Update, (
            spawn_trajectory_update_task,
            handle_trajectory_tasks,
            trajectory_mesh_init_system,
            trajectory_mesh_update_system,
            trajectory_alpha_update_system,
            trajectory_visibility_system,
            trajectory_alignment_system,
            mission_visibility_system,
        ));
    }
}

pub fn animate_trajectory_material(
    time: Res<Time>,
    mut materials: ResMut<Assets<TrajectoryMaterial>>,
) {
    let t = time.elapsed_secs();
    for (_, mat) in materials.iter_mut() {
        mat.extension.time = t;
        // No pulse or oscillation as requested
        mat.extension.pulse_pos = 0.0;
    }
}

pub fn trajectory_setup_system(
    mut commands: Commands,
) {
    // Initial spawning. Reference centering handled by alignment system.
    commands.spawn((
        Name::new("Earth Orbit View"),
        TrajectoryView {
            tracked_id: 399,
            reference_id: 10,
            frame: TrajectoryFrame::Inertial,
            color: LinearRgba::from(Color::srgba(0.0, 0.8, 1.0, 1.0)),
            is_visible: true,
            user_visible: true,
            sampling_days: 400.0,
            sampling_step: 0.5,
            start_epoch: None,
            end_epoch: None,
        },
        TrajectoryPath::default(),
        Transform::default(),
        GlobalTransform::default(),
        Visibility::default(),
        CellCoord::default(),
    ));

    commands.spawn((
        Name::new("Moon Orbit View"),
        TrajectoryView {
            tracked_id: 301,
            reference_id: 399,
            frame: TrajectoryFrame::Inertial,
            color: LinearRgba::from(Color::srgba(1.0, 0.9, 0.2, 1.0)),
            is_visible: true,
            user_visible: true,
            sampling_days: 30.0,
            sampling_step: 0.02,
            start_epoch: None,
            end_epoch: None,
        },
        TrajectoryPath::default(),
        Transform::default(),
        GlobalTransform::default(),
        Visibility::default(),
        CellCoord::default(),
    ));
}

pub fn spawn_trajectory_update_task(
    clock: Res<CelestialClock>,
    ephemeris: Res<EphemerisResource>,
    registry: Res<CelestialBodyRegistry>,
    mut commands: Commands,
    q_views: Query<(Entity, &TrajectoryView, &TrajectoryPath), Without<TrajectoryTask>>,
) {
    let current_epoch = clock.epoch;
    let pool = AsyncComputeTaskPool::get();
    
    for (entity, view, path) in q_views.iter() {
        let is_fixed = view.start_epoch.is_some() && view.end_epoch.is_some();
        let needs_update = if is_fixed {
            path.points.is_empty()
        } else {
            (path.update_epoch - current_epoch).abs() > view.sampling_step || path.points.is_empty()
        };

        if needs_update {
            let provider = Arc::clone(&ephemeris.provider);
            let registry_arc = Arc::new((*registry).clone());
            let view_copy = *view;
            
            let aligned_epoch = if is_fixed {
                // If fixed range, update_epoch is not moving
                view_copy.start_epoch.unwrap()
            } else {
                (current_epoch / view_copy.sampling_step).round() * view_copy.sampling_step
            };

            let task = pool.spawn(async move {
                let mut points = Vec::new();
                
                if view_copy.start_epoch.is_some() && view_copy.end_epoch.is_some() {
                    let start = view_copy.start_epoch.unwrap();
                    let end = view_copy.end_epoch.unwrap();
                    let count = ((end - start) / view_copy.sampling_step).ceil() as usize + 1;
                    points.reserve(count);
                    
                    for i in 0..count {
                        let jd = start + (i as f64) * view_copy.sampling_step;
                        if jd > end { break; } // Don't overshoot
                        
                        let p_target = provider.global_position(view_copy.tracked_id, jd);
                        let p_ref = provider.global_position(view_copy.reference_id, jd);
                        let mut rel_pos = crate::coords::ecliptic_to_bevy(p_target - p_ref);
                        
                        if view_copy.frame == TrajectoryFrame::BodyFixed {
                            if let Some(desc) = registry_arc.bodies.iter().find(|b| b.ephemeris_id == view_copy.reference_id) {
                                let days_since_j2000 = jd - 2_451_545.0;
                                let angle = days_since_j2000 * desc.rotation_rate_rad_per_day;
                                let rot = bevy::math::DQuat::from_axis_angle(desc.polar_axis, angle);
                                rel_pos = rot.inverse() * rel_pos;
                            }
                        }
                        
                        points.push(rel_pos);
                    }
                } else {
                    let half_count = (view_copy.sampling_days / view_copy.sampling_step / 2.0).ceil() as isize;
                    points.reserve((half_count * 2 + 1) as usize);
                    
                    for i in -half_count..=half_count {
                        let jd = aligned_epoch + (i as f64) * view_copy.sampling_step;
                        let p_target = provider.global_position(view_copy.tracked_id, jd);
                        let p_ref = provider.global_position(view_copy.reference_id, jd);
                        let mut rel_pos = crate::coords::ecliptic_to_bevy(p_target - p_ref);
                        
                        if view_copy.frame == TrajectoryFrame::BodyFixed {
                            if let Some(desc) = registry_arc.bodies.iter().find(|b| b.ephemeris_id == view_copy.reference_id) {
                                let days_since_j2000 = jd - 2_451_545.0;
                                let angle = days_since_j2000 * desc.rotation_rate_rad_per_day;
                                let rot = bevy::math::DQuat::from_axis_angle(desc.polar_axis, angle);
                                rel_pos = rot.inverse() * rel_pos;
                            }
                        }
                        
                        points.push(rel_pos);
                    }
                }
                
                TrajectoryData {
                    points,
                    epoch: aligned_epoch,
                }
            });
            
            commands.entity(entity).insert(TrajectoryTask(task));
        }
    }
}

pub fn handle_trajectory_tasks(
    mut commands: Commands,
    mut q_tasks: Query<(Entity, &mut TrajectoryTask, &mut TrajectoryPath, &TrajectoryView)>,
) {
    for (entity, mut task, mut path, view) in q_tasks.iter_mut() {
        if let Some(data) = future::block_on(future::poll_once(&mut task.0)) {
            path.points = data.points;
            path.update_epoch = data.epoch;
            commands.entity(entity).remove::<TrajectoryTask>();
            info!("Trajectory updated for entity {:?} with {} points. Tracking {}, Reference {}", 
                entity, path.points.len(), view.tracked_id, view.reference_id);
        }
    }
}

pub fn trajectory_mesh_init_system(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    q_new_views: Query<(Entity, &TrajectoryView), Added<TrajectoryPath>>,
) {
    for (entity, view) in q_new_views.iter() {
        let mut mesh = Mesh::new(
            PrimitiveTopology::LineStrip,
            RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
        );
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, Vec::<[f32; 3]>::new());
        mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, Vec::<[f32; 4]>::new());
        
        let mesh_handle = meshes.add(mesh);
        let color = view.color;
        let emissive_color = color * 15.0;
        
        let mat_handle = materials.add(StandardMaterial {
            base_color: Color::linear_rgba(emissive_color.red, emissive_color.green, emissive_color.blue, 1.0),
            unlit: true,
            alpha_mode: AlphaMode::Add,
            ..default()
        });

        commands.entity(entity).with_children(|parent| {
            parent.spawn((
                Mesh3d(mesh_handle),
                MeshMaterial3d(mat_handle),
                TrajectoryMeshMarker,
                Visibility::Visible,
                NoFrustumCulling,
                Transform::default(),
            ));
        });
    }
}

pub fn trajectory_mesh_update_system(
    mut meshes: ResMut<Assets<Mesh>>,
    q_paths: Query<(&TrajectoryPath, &TrajectoryView, &Children), Changed<TrajectoryPath>>,
    q_marker: Query<&Mesh3d, With<TrajectoryMeshMarker>>,
) {
    for (path, view, children) in q_paths.iter() {
        if path.points.is_empty() { continue; }
        
        let color = view.color;

        // Use Catmull-Rom spline for smooth curves (needs >= 4 points)
        let final_pts: Vec<[f32; 3]> = if path.points.len() >= 4 {
            let control_points: Vec<Vec3> = path.points.iter().map(|p| p.as_vec3()).collect();
            let spline = CubicCardinalSpline::new_catmull_rom(control_points);
            match spline.to_curve() {
                Ok(curve) => {
                    let n = (path.points.len() - 1) * 3;
                    curve.iter_positions(n).map(|p| p.to_array()).collect()
                }
                Err(_) => path.points.iter().map(|p| p.as_vec3().to_array()).collect(),
            }
        } else {
            path.points.iter().map(|p| p.as_vec3().to_array()).collect()
        };

        let colors: Vec<[f32; 4]> = vec![[color.red, color.green, color.blue, 1.0]; final_pts.len()];

        info!("Updating trajectory mesh with {} points", final_pts.len());

        for child in children.iter() {
            if let Ok(mesh_handle) = q_marker.get(child) {
                if let Some(mesh) = meshes.get_mut(&mesh_handle.0) {
                    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, final_pts.clone());
                    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors.clone());
                }
            }
        }
    }
}

pub fn trajectory_alpha_update_system(
    clock: Res<CelestialClock>,
    mut meshes: ResMut<Assets<Mesh>>,
    q_paths: Query<(&TrajectoryPath, &TrajectoryView, &Children)>,
    q_marker: Query<&Mesh3d, With<TrajectoryMeshMarker>>,
    mut last_update_jd: Local<f64>,
) {
    if (clock.epoch - *last_update_jd).abs() < 0.01 {
        return; // Only update alpha every ~14 minutes of simulation time to save performance
    }
    *last_update_jd = clock.epoch;

    for (path, view, children) in q_paths.iter() {
        if path.points.len() < 2 { continue; }
        for child in children.iter() {
            if let Ok(mesh_handle) = q_marker.get(child) {
                if let Some(mesh) = meshes.get_mut(&mesh_handle.0) {
                    let color = view.color;
                    let start_epoch = if let Some(s) = view.start_epoch {
                        s
                    } else {
                        path.update_epoch - (view.sampling_days / 2.0)
                    };
                    let total_sampling_days = if view.start_epoch.is_some() && view.end_epoch.is_some() {
                        view.end_epoch.unwrap() - view.start_epoch.unwrap()
                    } else {
                        view.sampling_days
                    };
                    
                    let num_points = mesh.attribute(Mesh::ATTRIBUTE_POSITION).unwrap().len();
                    
                    let colors: Vec<[f32; 4]> = (0..num_points).map(|i| {
                        let t = i as f64 / (num_points - 1) as f64;
                        let pt_epoch = start_epoch + t * total_sampling_days;
                        let alpha = if pt_epoch < clock.epoch { 0.05 } else { 1.0 };
                        [color.red, color.green, color.blue, alpha]
                    }).collect();
                    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
                    trace!("Trajectory alpha updated for {} points", num_points);
                }
            }
        }
    }
}


pub fn mission_visibility_system(
    clock: Res<CelestialClock>,
    mut q_views: Query<&mut TrajectoryView>,
) {
    for mut view in q_views.iter_mut() {
        if let (Some(start), Some(end)) = (view.start_epoch, view.end_epoch) {
            let should_be_visible = clock.epoch >= start && clock.epoch <= end;
            if view.is_visible != should_be_visible {
                view.is_visible = should_be_visible;
            }
        } else {
            // Non-mission trajectories are always active
            if !view.is_visible {
                view.is_visible = true;
            }
        }
    }
}


pub fn trajectory_visibility_system(
    q_views: Query<(&TrajectoryView, &Children), Changed<TrajectoryView>>,
    mut q_visibility: Query<&mut Visibility>,
) {
    for (view, children) in q_views.iter() {
        for child in children.iter() {
            if let Ok(mut vis) = q_visibility.get_mut(child) {
                // Combine mission-controlled visibility and user-controlled visibility
                let final_visible = view.is_visible && view.user_visible;
                // Use Visible instead of Inherited to prevent frustum culling of large meshes
                *vis = if final_visible { Visibility::Visible } else { Visibility::Hidden };
            }
        }
    }
}

pub fn trajectory_alignment_system(
    mut commands: Commands,
    q_frames: Query<(Entity, &CelestialReferenceFrame, &Transform, &CellCoord)>,
    mut q_vistas: Query<(Entity, &TrajectoryView, &mut Transform, &mut CellCoord, Option<&ChildOf>), Without<CelestialReferenceFrame>>,
) {
    for (v_entity, view, mut transform, mut cell, current_parent) in q_vistas.iter_mut() {
        let mut target_frame_found = false;
        for (f_entity, frame, _frame_tf, _frame_cell) in q_frames.iter() {
            if frame.ephemeris_id == view.reference_id {
                let is_current_parent = if let Some(p) = current_parent {
                    p.parent() == f_entity
                } else {
                    false
                };
                
                if !is_current_parent {
                    commands.entity(f_entity).add_child(v_entity); 
                }
                
                transform.translation = Vec3::ZERO;
                transform.rotation = Quat::IDENTITY;
                *cell = CellCoord::default();
                target_frame_found = true;
                break;
            }
        }
        
        if !target_frame_found && view.reference_id == 10 {
            // Sun frame fallback if needed, but solar_grid should be caught above
            transform.translation = Vec3::ZERO;
            *cell = CellCoord::default();
            transform.rotation = Quat::IDENTITY;
        }
    }
}

