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

pub struct TrajectoryPlugin;

#[derive(Component, Reflect, Clone, Copy, Debug)]
#[reflect(Component)]
pub struct TrajectoryView {
    pub tracked_id: u32,
    pub reference_id: u32,
    pub frame: TrajectoryFrame,
    pub color: Color,
    pub is_visible: bool,
    pub sampling_days: f64,
    pub sampling_step: f64,
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
            color: Color::WHITE,
            is_visible: true,
            sampling_days: 200.0,
            sampling_step: 1.0,
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
            trajectory_visibility_system,
            trajectory_alignment_system,
        ));
    }
}

pub fn trajectory_setup_system(mut commands: Commands) {
    // Initial spawning. Reference centering handled by alignment system.
    commands.spawn((
        Name::new("Earth Orbit View"),
        TrajectoryView {
            tracked_id: 399,
            reference_id: 10,
            frame: TrajectoryFrame::Inertial,
            color: Color::srgba(0.0, 0.8, 1.0, 1.0),
            is_visible: true,
            sampling_days: 400.0,
            sampling_step: 2.0,
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
            color: Color::srgba(1.0, 0.9, 0.2, 1.0),
            is_visible: true,
            sampling_days: 30.0,
            sampling_step: 0.1,
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
        if (path.update_epoch - current_epoch).abs() > 2.0 || path.points.is_empty() {
            let provider = Arc::clone(&ephemeris.provider);
            let registry_arc = Arc::new((*registry).clone());
            let view_copy = *view;
            let aligned_epoch = (current_epoch / 2.0).round() * 2.0;

            let task = pool.spawn(async move {
                let half_count = (view_copy.sampling_days / view_copy.sampling_step / 2.0).ceil() as isize;
                let mut points = Vec::with_capacity((half_count * 2 + 1) as usize);
                
                for i in -half_count..=half_count {
                    let jd = aligned_epoch + (i as f64) * view_copy.sampling_step;
                    let p_target = provider.position(view_copy.tracked_id, jd);
                    let p_ref = provider.position(view_copy.reference_id, jd);
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
        let mesh_handle = meshes.add(Mesh::new(
            PrimitiveTopology::LineStrip,
            RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
        ));
        let color = view.color;
        // High intensity for visibility
        let emissive_color = LinearRgba::from(color) * 50.0;
        
        let mat_handle = materials.add(StandardMaterial {
            base_color: color,
            emissive: emissive_color,
            unlit: true,
            alpha_mode: AlphaMode::Opaque,
            ..default()
        });

        commands.entity(entity).with_children(|parent| {
            parent.spawn((
                Mesh3d(mesh_handle),
                MeshMaterial3d(mat_handle),
                TrajectoryMeshMarker,
                Visibility::default(),
                Transform::default(),
                // Aabb::from_min_max(Vec3::splat(-1e12), Vec3::splat(1e12)), 
                // NoFrustumCulling,
            ));
        });
    }
}

pub fn trajectory_mesh_update_system(
    mut meshes: ResMut<Assets<Mesh>>,
    q_paths: Query<(&TrajectoryPath, &Children), Changed<TrajectoryPath>>,
    q_marker: Query<&Mesh3d, With<TrajectoryMeshMarker>>,
) {
    for (path, children) in q_paths.iter() {
        for child in children.iter() {
            if let Ok(mesh_handle) = q_marker.get(child) {
                if let Some(mesh) = meshes.get_mut(&mesh_handle.0) {
                    let pts: Vec<[f32; 3]> = path.points.iter().map(|p| p.as_vec3().to_array()).collect();
                    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, pts);
                }
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
                *vis = if view.is_visible { Visibility::Inherited } else { Visibility::Hidden };
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
                    commands.entity(v_entity).set_parent_in_place(f_entity); 
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

