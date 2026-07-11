use bevy::prelude::*;
use bevy::tasks::Task;
use bevy::render::render_resource::PrimitiveTopology;
use bevy::asset::{RenderAssetUsages, load_internal_asset};
use big_space::prelude::CellCoord;
use futures_lite::future;
use std::sync::Arc;
use crate::ephemeris::EphemerisResource;
use crate::registry::{CelestialBodyRegistry, CelestialReferenceFrame};
use lunco_time::WorldTime;

use bevy::shader::ShaderRef;
use bevy::render::render_resource::AsBindGroup;
use bevy::math::cubic_splines::CubicCardinalSpline;
use bevy::pbr::{MaterialExtension, ExtendedMaterial};
use bevy::camera::visibility::NoFrustumCulling;
use bevy_shader::Shader;

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
        // Returns the handle to our embedded shader (set by EmbeddedAssetsPlugin on wasm32)
        crate::embedded_assets::TRAJECTORY_SHADER_HANDLE.clone().into()
    }
}

/// Registers the embedded trajectory shader into the asset server.
pub struct TrajectoryShaderPlugin;

impl Plugin for TrajectoryShaderPlugin {
    fn build(&self, app: &mut App) {
        load_internal_asset!(
            app,
            crate::embedded_assets::TRAJECTORY_SHADER_HANDLE,
            "../../../assets/shaders/trajectory.wgsl",
            Shader::from_wgsl
        );
    }
}

/// Holds the compiled trajectory shader handle.
#[derive(Resource)]
pub struct TrajectoryShaderHandle(pub Handle<Shader>);

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
    /// Reference-frame offset that was subtracted from every point (the
    /// tracked body's position at `update_epoch`). Applied back as the view
    /// entity's cell + translation by `trajectory_alignment_system`, so the
    /// f32 mesh vertices stay SMALL near the tracked body. `ZERO` for
    /// un-anchored (mission/spacecraft) paths. See
    /// `spawn_trajectory_update_task`.
    pub anchor: bevy::math::DVec3,
}

#[derive(Component)]
pub struct TrajectoryTask(pub Task<TrajectoryData>);

pub struct TrajectoryData {
    pub points: Vec<bevy::math::DVec3>,
    pub epoch: f64,
    pub anchor: bevy::math::DVec3,
}

#[derive(Component)]
pub struct TrajectoryMeshMarker;

impl Plugin for TrajectoryPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<TrajectoryView>()
           .register_type::<TrajectoryFrame>()
           .register_type::<TrajectoryPath>();
           
        app.add_systems(Startup, trajectory_setup_system);
        
        // CHAINED: a rebuild must be ATOMIC within one frame.
        //
        // `handle_trajectory_tasks` writes `path.points` AND `path.anchor`
        // together; `trajectory_mesh_update_system` turns the points into f32
        // vertices; `trajectory_alignment_system` (PostUpdate) places the curve
        // using the anchor. The vertices are stored RELATIVE to the anchor, so
        // the two must agree.
        //
        // Unordered, the mesh system could run before `handle_trajectory_tasks`
        // and only pick the change up on the NEXT frame — while alignment
        // already applied the new anchor in this frame's PostUpdate. For that
        // one frame the curve was drawn a whole rebuild-step out of place:
        // ~1.7e6 m for the Moon line, ~1.3e9 m for Earth's. That is the "orbits
        // jumping around" flash, and rebuilds only fire while the clock runs,
        // which is why a paused scene never showed it.
        app.add_systems(Update, (
            spawn_trajectory_update_task,
            handle_trajectory_tasks,
            trajectory_mesh_init_system,
            trajectory_mesh_update_system,
            trajectory_alpha_update_system,
            trajectory_visibility_system,
            mission_visibility_system,
        ).chain());

        // Alignment must run in `PostUpdate`, NOT `Update`.
        //
        // A trajectory view is parented to a celestial frame and its local pose
        // is derived from that frame's CURRENT transform. The orbital view-pin
        // re-anchors the whole celestial tree in `PostUpdate` (after the camera
        // publishes `dir`/`distance`; see `lunco-avatar`). Aligning in `Update`
        // therefore used the PREVIOUS frame's pinned tree: while the user
        // dragged or zoomed, the orbit lines lagged the bodies by one frame and
        // swam against them ("the orbital lines still jitter"). Same one-frame
        // lag that made the whole sky wobble before the pin moved to PostUpdate.
        //
        // Ordered AFTER the re-pin (so the frame transforms it reads are this
        // frame's) and BEFORE `Propagate` (so the fresh local pose reaches this
        // frame's `GlobalTransform`s).
        app.add_systems(
            PostUpdate,
            trajectory_alignment_system
                .after(crate::placement::anchor_solar_frame_to_site)
                .before(bevy::transform::TransformSystems::Propagate),
        );

        // Drag diagnostic — reads the FINAL `GlobalTransform`s, so it must run
        // after propagation. Opt-in: `LUNCO_TRAJ_PROBE=1`.
        if std::env::var("LUNCO_TRAJ_PROBE").is_ok_and(|v| v == "1") {
            app.add_systems(
                PostUpdate,
                trajectory_probe_system.after(bevy::transform::TransformSystems::Propagate),
            );
        }

        // Whole-scene jump detector — per-frame, per-landmark discontinuity
        // attribution. Opt-in: `LUNCO_JUMP_PROBE=1`.
        if std::env::var("LUNCO_JUMP_PROBE").is_ok_and(|v| v == "1") {
            app.add_systems(
                PostUpdate,
                jump_probe_system.after(bevy::transform::TransformSystems::Propagate),
            );
        }
    }
}

/// Opt-in whole-scene jump detector: `LUNCO_JUMP_PROBE=1`.
///
/// Screenshots and sampled probes cannot catch single-frame glitches — this
/// runs AFTER propagation every frame and tracks each landmark's rendered
/// position relative to the floating-origin camera (world axes, so pure
/// camera rotation is invisible to it). A visible "jump" is a DISCONTINUITY
/// in that relative motion, i.e. a large second difference: smooth orbiting
/// (even fast dragging) produces a steady per-frame delta; a one-frame
/// convention flip / stale GT produces a delta spike. Logs the entity name,
/// the spike size, and the frame — plus a once-per-second heartbeat of the
/// largest spike seen so a silent log provably means "no jumps".
///
/// Landmarks: celestial bodies, reference-frame grids, trajectory views,
/// grid-anchored scene roots, and the `WorldGrid` (the root-composition
/// victim class of the 2026-07-10 regression).
#[allow(clippy::type_complexity)]
pub fn jump_probe_system(
    q_cam: Query<&GlobalTransform, With<big_space::prelude::FloatingOrigin>>,
    q_marks: Query<
        (Entity, Option<&Name>, &GlobalTransform),
        Or<(
            With<crate::registry::CelestialBody>,
            With<CelestialReferenceFrame>,
            With<TrajectoryView>,
            With<lunco_core::GridAnchor>,
            With<lunco_core::WorldGrid>,
        )>,
    >,
    q_parents: Query<&ChildOf>,
    q_names: Query<&Name>,
    mut last: Local<std::collections::HashMap<Entity, (bevy::math::DVec3, bevy::math::DVec3)>>,
    mut last_parent: Local<std::collections::HashMap<Entity, Entity>>,
    mut frame: Local<u64>,
    mut heartbeat: Local<(f64, String)>,
) {
    *frame += 1;
    let Ok(cam_gt) = q_cam.single() else { return };
    let cam = cam_gt.translation().as_dvec3();
    let label = |e: Entity, q: &Query<&Name>| -> String {
        q.get(e).map(|n| n.as_str().to_string()).unwrap_or_else(|_| format!("{e:?}"))
    };
    for (e, name, gt) in q_marks.iter() {
        // Attribute the tug-of-war directly: log every PARENT flip, jump or not.
        let parent = q_parents.get(e).map(|p| p.parent()).ok();
        if let Some(parent) = parent {
            match last_parent.get(&e) {
                Some(prev) if *prev != parent => {
                    bevy::log::warn!(
                        "[jump-probe] f{} {}: PARENT {} -> {}",
                        *frame,
                        name.map(|n| n.as_str()).unwrap_or("<unnamed>"),
                        label(*prev, &q_names),
                        label(parent, &q_names),
                    );
                    last_parent.insert(e, parent);
                }
                None => { last_parent.insert(e, parent); }
                _ => {}
            }
        }
        let p = gt.translation().as_dvec3() - cam;
        if let Some((prev_p, prev_d)) = last.get(&e).copied() {
            let d = p - prev_p;
            let jerk = (d - prev_d).length();
            // Tolerate smooth motion (epoch drift, drag) with frame-time
            // variation; flag genuine discontinuities. Headless uncapped runs
            // wobble ±30% in dt, so the floor sits above that noise (real
            // convention flips measured ≥3.5e8 m; rebuild snaps ~1.8e6 m).
            if jerk > 5.0e4_f64.max(0.75 * prev_d.length()) && jerk > 0.001 * p.length() {
                bevy::log::warn!(
                    "[jump-probe] f{} {}: JUMP {:.3e} m (motion {:.3e} -> {:.3e} m/frame, dist {:.3e}, parent {})",
                    *frame,
                    name.map(|n| n.as_str()).unwrap_or("<unnamed>"),
                    jerk,
                    prev_d.length(),
                    d.length(),
                    p.length(),
                    parent.map(|pe| label(pe, &q_names)).unwrap_or_else(|| "<none>".into()),
                );
            }
            if jerk > heartbeat.0 {
                *heartbeat = (jerk, name.map(|n| n.as_str().to_string()).unwrap_or_default());
            }
            last.insert(e, (p, d));
        } else {
            last.insert(e, (p, bevy::math::DVec3::ZERO));
        }
    }
    if *frame % 120 == 0 {
        bevy::log::info!(
            "[jump-probe] f{} heartbeat: max jerk since last = {:.3e} m ({})",
            *frame, heartbeat.0, heartbeat.1
        );
        *heartbeat = (0.0, String::new());
    }
}

/// Opt-in drag diagnostic: `LUNCO_TRAJ_PROBE=1`.
///
/// The orbit lines cannot be jitter-tested headlessly — rotate/zoom are raw mouse
/// input the API cannot inject, and `FocusEntityById`'s `distance` is ignored once
/// the pin owns the view. So log the invariant instead and let a human drag.
///
/// A view is a CHILD of its tracked body's grid, offset by `desired_local`. So the
/// RENDERED gap between the view and that grid must equal `|desired_local|` every
/// frame. Two independent numbers are printed:
///
/// * `gt_gap`   — from `GlobalTransform`s (what the renderer actually draws).
/// * `want`     — `|cell×edge + translation|`, the pose the aligner wrote.
///
/// If `gt_gap` tracks `want`, the curve is glued and any jitter is elsewhere.
/// If `gt_gap` jumps to ~the camera distance while `want` stays put, a
/// `GlobalTransform` writer is losing the `CellCoord`s (the bevy-compat pass —
/// see the doc 45 correction block — class 2).
#[allow(clippy::type_complexity)]
pub fn trajectory_probe_system(
    q_views: Query<(&Name, &TrajectoryView, &CellCoord, &Transform, &GlobalTransform, &ChildOf), With<TrajectoryPath>>,
    q_frames: Query<(&GlobalTransform, &big_space::prelude::Grid)>,
    mut tick: Local<u32>,
) {
    *tick += 1;
    if *tick % 20 != 0 {
        return;
    }
    for (name, _view, cell, tf, gt, child_of) in q_views.iter() {
        let Ok((parent_gt, parent_grid)) = q_frames.get(child_of.parent()) else {
            bevy::log::info!("[traj-probe] {name}: parent has NO Grid (unparented?)");
            continue;
        };
        let edge = parent_grid.cell_edge_length() as f64;
        let want = bevy::math::DVec3::new(
            cell.x as f64 * edge + tf.translation.x as f64,
            cell.y as f64 * edge + tf.translation.y as f64,
            cell.z as f64 * edge + tf.translation.z as f64,
        )
        .length();
        let gt_gap = (gt.translation() - parent_gt.translation()).length() as f64;
        bevy::log::info!(
            "[traj-probe] {name}: gt_gap={gt_gap:.4e} want={want:.4e} ratio={:.4} |gt|={:.4e} |parent_gt|={:.4e}",
            if want > 1.0 { gt_gap / want } else { f64::NAN },
            gt.translation().length(),
            parent_gt.translation().length(),
        );
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
        // No CellCoord at spawn: `trajectory_alignment_system` inserts one
        // when (and only when) it parents the view to a Grid — a cell-entity
        // anywhere else is invalid per big_space's hierarchy rules.
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
        // No CellCoord at spawn: `trajectory_alignment_system` inserts one
        // when (and only when) it parents the view to a Grid — a cell-entity
        // anywhere else is invalid per big_space's hierarchy rules.
    ));
}

pub fn spawn_trajectory_update_task(
    world: Res<WorldTime>,
    ephemeris: Res<EphemerisResource>,
    registry: Res<CelestialBodyRegistry>,
    mut commands: Commands,
    q_views: Query<(Entity, &TrajectoryView, &TrajectoryPath), Without<TrajectoryTask>>,
    q_frames: Query<&CelestialReferenceFrame>,
) {
    let current_epoch = world.epoch_jd;
    let pool = bevy::tasks::ComputeTaskPool::get();

    for (entity, view, path) in q_views.iter() {
        // Body orbit views (the tracked id has its own reference frame in
        // the scene — Earth around the Sun, Moon around the Earth) are
        // ANCHORED: points are stored relative to the tracked body's
        // position at the sampling epoch, and that anchor goes back into
        // the view entity's cell + translation (exact big_space math). The
        // f32 mesh vertices are then small exactly where the viewer looks —
        // at the body — instead of reference-frame magnitudes (~4e8 m for
        // the Moon around Earth, which cancels to ~64 m of per-frame
        // model-view wobble up close: the "moon offset from its jittering
        // orbit" report). The curve itself stays static in the reference
        // frame — the body slides along it. Mission/spacecraft trajectories
        // (no frame for the tracked id) keep zero anchor.
        let anchored = view.frame == TrajectoryFrame::Inertial
            && q_frames.iter().any(|f| f.ephemeris_id == view.tracked_id);
        let is_fixed = view.start_epoch.is_some() && view.end_epoch.is_some();
        let needs_update = if is_fixed {
            path.points.is_empty()
        } else {
            (path.update_epoch - current_epoch).abs() > view.sampling_step
                || path.points.is_empty()
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

                // Anchor: tracked body's reference-relative position at the
                // aligned epoch — subtracted from every sample so the curve
                // is expressed relative to the tracked body (see above).
                let anchor = if anchored {
                    let p_target = provider.global_position(view_copy.tracked_id, aligned_epoch);
                    let p_ref = provider.global_position(view_copy.reference_id, aligned_epoch);
                    crate::coords::ecliptic_to_bevy(p_target - p_ref)
                } else {
                    bevy::math::DVec3::ZERO
                };

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
                                let days_since_j2000 = jd - lunco_time::J2000_JD;
                                let angle = days_since_j2000 * desc.rotation_rate_rad_per_day;
                                let rot = bevy::math::DQuat::from_axis_angle(desc.polar_axis, angle);
                                rel_pos = rot.inverse() * rel_pos;
                            }
                        }

                        points.push(rel_pos - anchor);
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
                                let days_since_j2000 = jd - lunco_time::J2000_JD;
                                let angle = days_since_j2000 * desc.rotation_rate_rad_per_day;
                                let rot = bevy::math::DQuat::from_axis_angle(desc.polar_axis, angle);
                                rel_pos = rot.inverse() * rel_pos;
                            }
                        }

                        points.push(rel_pos - anchor);
                    }
                }

                TrajectoryData {
                    points,
                    epoch: aligned_epoch,
                    anchor,
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
            path.anchor = data.anchor;
            commands.entity(entity).remove::<TrajectoryTask>();
            info!("Trajectory updated for entity {:?} with {} points (anchor |{:.3e}| m). Tracking {}, Reference {}",
                entity, path.points.len(), path.anchor.length(), view.tracked_id, view.reference_id);
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
                if let Some(mut mesh) = meshes.get_mut(&mesh_handle.0) {
                    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, final_pts.clone());
                    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors.clone());
                }
            }
        }
    }
}

pub fn trajectory_alpha_update_system(
    world: Res<WorldTime>,
    mut meshes: ResMut<Assets<Mesh>>,
    q_paths: Query<(&TrajectoryPath, &TrajectoryView, &Children)>,
    q_marker: Query<&Mesh3d, With<TrajectoryMeshMarker>>,
) {
    // TODO(CQ-214): this rebuilds the full per-point ATTRIBUTE_COLOR Vec and
    // re-uploads it to the GPU for every trajectory, every frame, with no
    // change detection — even when the clock is paused or unchanged. Gate on
    // `world.is_changed()` (+ a per-view epoch/color stamp), and skip the
    // re-upload when the alpha curve hasn't moved. See
    // docs/code-quality-remediation.md (CQ-214).
    for (path, view, children) in q_paths.iter() {
        if path.points.len() < 2 { continue; }
        for child in children.iter() {
            if let Ok(mesh_handle) = q_marker.get(child) {
                if let Some(mut mesh) = meshes.get_mut(&mesh_handle.0) {
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
                        
                        let days_past = world.epoch_jd - pt_epoch;
                        let alpha = if days_past > 0.0 {
                            // Smoothly fade out the past trajectory over 10% of total duration (capped between 1 to 20 days)
                            let fade_days = (total_sampling_days * 0.1).clamp(1.0, 20.0);
                            let a = 1.0 - (days_past / fade_days);
                            // With additive blending at 15x brightness, we need alpha to approach zero, not 0.05!
                            a.max(0.001) as f32 // Gentle curve drop-off
                        } else {
                            1.0
                        };
                        
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
    world: Res<WorldTime>,
    mut q_views: Query<&mut TrajectoryView>,
) {
    for mut view in q_views.iter_mut() {
        if let (Some(start), Some(end)) = (view.start_epoch, view.end_epoch) {
            let should_be_visible = world.epoch_jd >= start && world.epoch_jd <= end;
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
    world: Res<WorldTime>,
    ephemeris: Option<Res<EphemerisResource>>,
    registry: Res<CelestialBodyRegistry>,
    q_frames: Query<(Entity, &CelestialReferenceFrame, Option<&big_space::prelude::Grid>, &Transform), Without<TrajectoryPath>>,
    q_bodies: Query<(Entity, &crate::registry::CelestialBody)>,
    mut q_vistas: Query<(Entity, &TrajectoryView, &TrajectoryPath, &mut Transform, Option<&mut CellCoord>, Option<&ChildOf>), Without<CelestialReferenceFrame>>,
) {
    let jd = world.epoch_jd;

    // Cancel a parent frame's BODY SPIN — and only that.
    //
    // `body_rotation_system` writes `Transform.rotation` on frames whose body has
    // a non-zero rotation rate; sampled trajectory points are inertial, so a view
    // under such a frame must un-spin. The Solar Grid also carries a rotation, but
    // it is the site-alignment `align` written by `anchor_solar_frame_to_site`,
    // which the whole sky (bodies included) is *supposed* to inherit. Cancelling
    // that would tilt the orbit lines out of the sky. Gate on exactly the same
    // condition `body_rotation_system` uses.
    let spin_inverse = |eph_id: i32, tf: &Transform| -> Quat {
        let spins = registry
            .bodies
            .iter()
            .any(|d| d.ephemeris_id == eph_id && d.rotation_rate_rad_per_day != 0.0);
        if spins { tf.rotation.inverse() } else { Quat::IDENTITY }
    };
    for (v_entity, view, path, mut transform, cell, current_parent) in q_vistas.iter_mut() {
        let mut target_parent = None;
        let mut parent_grid: Option<&big_space::prelude::Grid> = None;
        // Anchored views sit under a ROTATING body frame; the sampled points
        // are inertial, so the view cancels the parent's spin.
        let mut counter_rotation = Quat::IDENTITY;
        // For anchored views: the tracked body's CURRENT position in the SAME
        // reference frame `path.anchor` was sampled in, so the curve rides the
        // body continuously (cancels drift since the anchor epoch) — see the
        // placement write below.
        let mut tracked_translation: Option<bevy::math::DVec3> = None;

        if view.frame == TrajectoryFrame::BodyFixed {
            // Body-fixed points belong on the body's (spinning) reference-frame
            // GRID: the grid's rotation IS the body-fixed frame (inherit it —
            // `counter_rotation` stays IDENTITY), and big_space only propagates
            // a cell-entity whose direct parent is a `Grid` — a cell-entity
            // under a plain body entity is silently left to the f32 compat
            // pass (doc 45 correction block, class 2; the "Artemis 2
            // Moon-Relative: parent has NO Grid" probe warning).
            for (f_entity, frame, grid, _f_tf) in q_frames.iter() {
                if frame.ephemeris_id == view.reference_id {
                    target_parent = Some(f_entity);
                    parent_grid = grid;
                    break;
                }
            }
            // No frame grid for this body (simple planets on the Solar Grid):
            // fall back to the body entity as before.
            if target_parent.is_none() {
                for (b_entity, body) in q_bodies.iter() {
                    if body.ephemeris_id == view.reference_id {
                        target_parent = Some(b_entity);
                        break;
                    }
                }
            }
        } else if path.anchor != bevy::math::DVec3::ZERO {
            // ANCHORED body-orbit view (points stored relative to the tracked
            // body at the rebuild epoch). Parent to the TRACKED body's frame;
            // the placement write below subtracts the body's CURRENT position
            // so the curve stays fixed in inertial space and the body slides
            // along it (continuous anchor — kills the "offset from its orbit
            // unless I scroll away" drift-then-snap; KSA v2025.11.9 fix).
            for (f_entity, frame, grid, f_tf) in q_frames.iter() {
                if frame.ephemeris_id == view.tracked_id {
                    target_parent = Some(f_entity);
                    parent_grid = grid;
                    counter_rotation = spin_inverse(frame.ephemeris_id, f_tf);
                    // The tracked body's position relative to `reference_id`,
                    // at the CURRENT epoch — the same quantity, in the same
                    // frame, that `spawn_trajectory_update_task` sampled into
                    // `path.anchor`. Read from the provider in f64, NOT from
                    // the frame's `Transform`:
                    //
                    // * `Transform.translation` is parent-GRID-relative (Moon
                    //   frame → EMB, not → Earth), a different reference frame
                    //   than the anchor's, so the subtraction below was mixing
                    //   frames and left a body-scale constant offset.
                    // * It is also cell-BLIND. Since the grids carry real
                    //   `CellCoord`s the translation is only the sub-cell
                    //   remainder, and it WRAPS by a full cell edge (1e8 m for
                    //   the Moon in the EMB grid) whenever the body crosses a
                    //   boundary — the orbit line teleporting between frames.
                    //
                    // Sampling both ends from the provider makes the "now"
                    // point of the curve cancel to exactly the grid origin (=
                    // the tracked body), whatever f32 rounding the stored grid
                    // chain carries: the view is a CHILD of that grid, so it
                    // inherits the identical rounding.
                    tracked_translation = ephemeris.as_ref().map(|e| {
                        crate::coords::ecliptic_to_bevy(
                            e.provider.global_position(view.tracked_id, jd)
                                - e.provider.global_position(view.reference_id, jd),
                        )
                    });
                    break;
                }
            }
        } else {
            // UN-ANCHORED inertial view (mission/spacecraft paths — the tracked id
            // has no frame of its own, so the points were sampled straight in the
            // reference frame). It parents to the REFERENCE frame, which for
            // Earth-relative (399) or Moon-relative (301) missions is a grid that
            // `body_rotation_system` SPINS. The points are inertial, so without
            // this the curve rode the body's rotation — the Earth-relative Artemis
            // trajectory swept a full revolution per day.
            for (f_entity, frame, grid, f_tf) in q_frames.iter() {
                if frame.ephemeris_id == view.reference_id {
                    target_parent = Some(f_entity);
                    parent_grid = grid;
                    counter_rotation = spin_inverse(frame.ephemeris_id, f_tf);
                    break;
                }
            }
        }

        if let Some(parent_ent) = target_parent {
            let is_current_parent = current_parent.map(|p| p.parent() == parent_ent).unwrap_or(false);
            if !is_current_parent {
                // Trajectory views are NOT `GridAnchor`s — they parent to
                // `CelestialBody` / `CelestialReferenceFrame` entities. The
                // cell/translation are set just below; the deferred-vs-immediate
                // split is harmless because no observers fire on this archetype.
                #[allow(clippy::disallowed_methods)]
                commands.entity(parent_ent).add_child(v_entity);
            }
            // Desired local position in the parent frame. For anchored views,
            // `path.anchor` (body pos at the rebuild epoch) minus the body's
            // CURRENT position in that same frame = -drift. That keeps the
            // curve's "now" point glued to the rendered body as it orbits — no
            // rebuild-snap. Non-anchored/BodyFixed views want ZERO.
            //
            // `counter_rotation` (= parent spin inverse) converts the ecliptic
            // offset into the parent's LOCAL axes. A child's translation is
            // expressed in its parent's ROTATED frame, so writing the ecliptic
            // vector raw placed the curve at `spin * drift` instead of `drift`
            // — an error that swung around with the body's rotation. The mesh
            // vertices are ecliptic and un-spun by the view's own
            // `counter_rotation`, so both compose back to ecliptic exactly.
            let desired_local = match tracked_translation {
                Some(ft) => counter_rotation.as_dquat() * (path.anchor - ft),
                None => bevy::math::DVec3::ZERO,
            };
            // Split through the parent grid so the view stays within one cell
            // (otherwise recenter_large_transforms would fight a large drift
            // translation). A parent WITHOUT a Grid (BodyFixed falling back to
            // a plain body entity) must not keep a `CellCoord` at all: a
            // cell-entity under a non-grid parent is invalid per big_space's
            // hierarchy rules (cell-entities are direct grid children — the
            // validator flags it, and HP propagation silently skips it), so
            // the view becomes a plain low-precision Transform child instead.
            let new_translation = match parent_grid {
                Some(grid) => {
                    let (new_cell, t) = grid.translation_to_grid(desired_local);
                    match cell {
                        Some(mut cell) => {
                            if *cell != new_cell {
                                *cell = new_cell;
                            }
                        }
                        None => {
                            commands.entity(v_entity).insert(new_cell);
                        }
                    }
                    t
                }
                None => {
                    if cell.is_some() {
                        commands.entity(v_entity).remove::<CellCoord>();
                    }
                    desired_local.as_vec3()
                }
            };
            if transform.translation != new_translation || transform.rotation != counter_rotation {
                transform.translation = new_translation;
                transform.rotation = counter_rotation;
            }
        } else if view.reference_id == 10 {
            // Sun frame fallback: unparented → a plain Transform tree root;
            // it must not carry a `CellCoord` either.
            if transform.translation != Vec3::ZERO || transform.rotation != Quat::IDENTITY {
                transform.translation = Vec3::ZERO;
                transform.rotation = Quat::IDENTITY;
            }
            if cell.is_some() {
                commands.entity(v_entity).remove::<CellCoord>();
            }
        }
    }
}

