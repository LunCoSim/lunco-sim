use crate::ephemeris::{EphemerisProvider, EphemerisResource};
use crate::registry::{BodyDescriptor, CelestialBodyRegistry, ReferenceFrame};
use bevy::asset::RenderAssetUsages;
use bevy::math::DVec3;
use bevy::prelude::*;
use bevy::tasks::Task;
use bevy_mesh::PrimitiveTopology;
use big_space::prelude::CellCoord;
use futures_lite::future;
use lunco_time::WorldTime;
use std::sync::Arc;

use bevy::camera::visibility::NoFrustumCulling;
use bevy::math::cubic_splines::CubicCardinalSpline;
use lunco_render::{PbrLook, SurfaceAlpha};

pub struct TrajectoryPlugin;

#[derive(Component, Reflect, Clone, Copy, Debug)]
#[reflect(Component)]
pub struct TrajectoryView {
    pub tracked_id: i32,
    pub reference_id: i32,
    pub frame: TrajectoryFrame,
    pub color: LinearRgba,
    pub is_visible: bool,   // Controlled by mission range logic
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
            tracked_id: crate::ephemeris_id::EARTH,
            reference_id: crate::ephemeris_id::SUN,
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
    /// f32 mesh vertices stay SMALL near the tracked body. `anchored` is the
    /// authoritative mode bit; the offset itself may legitimately be zero.
    pub anchor: bevy::math::DVec3,
    /// Whether the sampled points are relative to the tracked body's frame.
    pub anchored: bool,
    /// Monotonic revision of the committed sampled geometry. Mesh and alpha
    /// workers fence their results against this value instead of using an epoch
    /// as an identity stamp.
    pub geometry_revision: u64,
}

/// Minimum wall-clock seconds between trajectory rebuilds.
///
/// A body's orbit is a quasi-static ellipse — over one WALL second it is
/// imperceptibly different at realtime rates, because what actually moves is the body
/// *along* the curve, not the curve itself. So 1 Hz is plenty while realtime runs,
/// and high-rate Celestial transport holds the existing sample entirely. Each
/// rebuild re-samples 800–1500 ephemeris points and re-splines the mesh on the
/// compute task before the main schedule commits the prepared asset data.
const MIN_REBUILD_INTERVAL_SECS: f64 = 1.0;

/// The part of a view that determines sampled geometry.
#[derive(Clone, Copy, Debug, PartialEq)]
struct TrajectoryViewSignature {
    tracked_id: i32,
    reference_id: i32,
    frame: TrajectoryFrame,
    sampling_days: f64,
    sampling_step: f64,
    start_epoch: Option<f64>,
    end_epoch: Option<f64>,
}

fn trajectory_view_signature(view: &TrajectoryView) -> TrajectoryViewSignature {
    TrajectoryViewSignature {
        tracked_id: view.tracked_id,
        reference_id: view.reference_id,
        frame: view.frame,
        sampling_days: view.sampling_days,
        sampling_step: view.sampling_step,
        start_epoch: view.start_epoch,
        end_epoch: view.end_epoch,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TrajectorySamplingStatus {
    Pending,
    Ready,
    Empty,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct EphemerisRevision {
    available: bool,
    motion: u64,
}

fn ephemeris_revision(ephemeris: Option<&EphemerisResource>) -> EphemerisRevision {
    ephemeris.map_or(
        EphemerisRevision {
            available: false,
            motion: 0,
        },
        |ephemeris| EphemerisRevision {
            available: true,
            motion: ephemeris.provider.motion_revision(),
        },
    )
}

impl Default for TrajectorySamplingStatus {
    fn default() -> Self {
        Self::Pending
    }
}

/// Runtime bookkeeping for the sampled presentation. It is deliberately
/// separate from `TrajectoryPath`: the wall-clock scheduler must not make the
/// geometry component look changed and trigger a mesh rebuild.
#[derive(Component, Debug)]
pub(crate) struct TrajectoryRuntimeState {
    presentation_revision: u64,
    sampling_revision: u64,
    view_signature: Option<TrajectoryViewSignature>,
    resolved_sampling_revision: Option<u64>,
    resolved_frame_revision: Option<u64>,
    /// Provider availability and motion revision are part of the input
    /// identity, so provider insertion at runtime reopens a failed view.
    resolved_provider_revision: Option<EphemerisRevision>,
    status: TrajectorySamplingStatus,
    last_rebuild_real_secs: f64,
}

impl Default for TrajectoryRuntimeState {
    fn default() -> Self {
        Self {
            presentation_revision: 0,
            sampling_revision: 0,
            view_signature: None,
            resolved_sampling_revision: None,
            resolved_frame_revision: None,
            resolved_provider_revision: None,
            status: TrajectorySamplingStatus::Pending,
            last_rebuild_real_secs: 0.0,
        }
    }
}

impl TrajectoryRuntimeState {
    fn observe_view(&mut self, view: &TrajectoryView) {
        self.presentation_revision = self.presentation_revision.wrapping_add(1);
        let signature = trajectory_view_signature(view);
        if self.view_signature != Some(signature) {
            self.sampling_revision = self.sampling_revision.wrapping_add(1);
            self.view_signature = Some(signature);
            self.resolved_sampling_revision = None;
            self.status = TrajectorySamplingStatus::Pending;
        }
    }
}

/// Sampling and mesh preparation are bounded so malformed but finite authored
/// ranges cannot turn a visualization worker into an unbounded allocator.
const MAX_TRAJECTORY_SAMPLES: usize = 20_001;

#[derive(Component)]
struct TrajectoryTask(Task<TrajectoryData>);

struct TrajectoryData {
    signature: TrajectoryViewSignature,
    sampling_revision: u64,
    frame_revision: u64,
    provider_revision: EphemerisRevision,
    sample: Result<TrajectorySample, TrajectorySampleError>,
}

struct TrajectorySample {
    points: Vec<bevy::math::DVec3>,
    epoch: f64,
    anchor: bevy::math::DVec3,
    anchored: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TrajectorySampleError {
    InvalidSampling,
    MissingAnchor,
    MissingBodyDescriptor,
    NoSamples,
    NonFiniteSample,
    TooManySamples,
}

#[derive(Component)]
pub struct TrajectoryMeshMarker;

/// Change stamp for the per-vertex time-fade attribute.
#[derive(Component, Default)]
struct TrajectoryAlphaState {
    epoch_jd: Option<f64>,
    geometry_revision: Option<u64>,
    sampling_days: f64,
    start_epoch: Option<f64>,
    end_epoch: Option<f64>,
    num_points: usize,
}

/// Generation stamp for the committed geometry and its current appearance.
#[derive(Component, Default)]
struct TrajectoryMeshState {
    geometry_revision: Option<u64>,
    presentation_revision: Option<u64>,
    num_points: usize,
    failed: bool,
}

/// Presentation policy shared by the trajectory producers and the alpha pass.
#[derive(Resource, Default, Debug, Clone, Copy)]
struct TrajectoryPresentationState {
    hold_curve: bool,
}

#[inline]
fn trajectory_is_renderable(
    view: &TrajectoryView,
    state: &TrajectoryRuntimeState,
    path: &TrajectoryPath,
) -> bool {
    trajectory_is_active(view)
        && state.status == TrajectorySamplingStatus::Ready
        && !path.points.is_empty()
}

#[inline]
fn trajectory_mesh_is_current(
    path: &TrajectoryPath,
    runtime: &TrajectoryRuntimeState,
    mesh_state: Option<&TrajectoryMeshState>,
) -> bool {
    mesh_state.is_some_and(|state| {
        state.geometry_revision == Some(path.geometry_revision)
            && state.presentation_revision == Some(runtime.presentation_revision)
            && !state.failed
            && state.num_points > 0
    })
}

#[inline]
fn trajectory_is_presented(
    view: &TrajectoryView,
    runtime: &TrajectoryRuntimeState,
    path: &TrajectoryPath,
    mesh_state: Option<&TrajectoryMeshState>,
) -> bool {
    trajectory_is_renderable(view, runtime, path)
        && trajectory_mesh_is_current(path, runtime, mesh_state)
}

fn clear_trajectory_path(path: &mut TrajectoryPath) {
    path.points.clear();
    path.anchor = DVec3::ZERO;
    path.anchored = false;
    path.geometry_revision = path.geometry_revision.wrapping_add(1);
}

fn mark_sampling_resolution(
    state: &mut TrajectoryRuntimeState,
    status: TrajectorySamplingStatus,
    sampling_revision: u64,
    frame_revision: u64,
    provider_revision: EphemerisRevision,
) {
    state.status = status;
    state.resolved_sampling_revision = Some(sampling_revision);
    state.resolved_frame_revision = Some(frame_revision);
    state.resolved_provider_revision = Some(provider_revision);
}

fn trajectory_children_visible(
    entity: Entity,
    visible: bool,
    q_children: &Query<&Children>,
    q_marker: &Query<(), With<TrajectoryMeshMarker>>,
    q_visibility: &mut Query<&mut Visibility>,
) {
    let Ok(children) = q_children.get(entity) else {
        return;
    };
    for child in children.iter() {
        if q_marker.contains(child) {
            if let Ok(mut visibility) = q_visibility.get_mut(child) {
                let next = if visible {
                    Visibility::Visible
                } else {
                    Visibility::Hidden
                };
                if *visibility != next {
                    *visibility = next;
                }
            }
        }
    }
}

#[inline]
fn trajectory_is_active(view: &TrajectoryView) -> bool {
    view.is_visible && view.user_visible
}

#[inline]
fn alpha_update_is_needed(
    state: Option<&TrajectoryAlphaState>,
    epoch_jd: f64,
    path: &TrajectoryPath,
    view: &TrajectoryView,
    num_points: usize,
    hold_curve: bool,
) -> bool {
    let Some(state) = state else {
        return true;
    };
    (!hold_curve && state.epoch_jd != Some(epoch_jd))
        || state.geometry_revision != Some(path.geometry_revision)
        || state.sampling_days != view.sampling_days
        || state.start_epoch != view.start_epoch
        || state.end_epoch != view.end_epoch
        || state.num_points != num_points
}

#[inline]
fn celestial_clock_is_high_rate(
    regime: lunco_time::TimeRegime,
    domain_scale: Option<f64>,
    effective_rate: Option<f64>,
) -> bool {
    matches!(regime, lunco_time::TimeRegime::KinematicWarp)
        || domain_scale.is_some_and(|scale| scale.abs() > lunco_time::MAX_REALTIME_RATE)
        || effective_rate.is_some_and(|rate| rate.abs() > lunco_time::MAX_REALTIME_RATE)
}

impl Plugin for TrajectoryPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<TrajectoryView>()
            .register_type::<TrajectoryFrame>()
            .register_type::<TrajectoryPath>()
            .init_resource::<TrajectoryPresentationState>();

        // Trajectories are content: a scene declares them through
        // `lunco:trajectory:*` (`MissionTrajectoryDecl`), and no trajectory is
        // created for an undeclared body.
        //
        // The Update chain fences each stage of the asynchronous presentation.
        // Sampling commits points, anchor, and mode together; mesh preparation
        // and alpha preparation carry those revisions; stale results are
        // discarded before they can change the visible asset.
        app.add_systems(
            Update,
            (
                mission_visibility_system,
                track_trajectory_view_changes,
                spawn_trajectory_update_task,
                handle_trajectory_tasks,
                trajectory_mesh_update_system,
                handle_trajectory_mesh_tasks,
                trajectory_alpha_update_system,
                handle_trajectory_alpha_tasks,
                trajectory_visibility_system,
            )
                .chain(),
        );

        // The alignment write belongs in PostUpdate before transform
        // propagation: it reads the current celestial frame pose and publishes
        // the view's local BigSpace placement for this render frame.
        app.add_systems(
            PostUpdate,
            trajectory_alignment_system
                .before(bevy::transform::TransformSystems::Propagate)
                // Epoch motion uses the shared celestial cadence; a new or
                // edited frame assignment is handled immediately.
                .run_if(
                    crate::cadence::tracked_needs_solve()
                        .or_else(trajectory_frame_assignment_changed),
                ),
        );
    }
}

/// Converts Bevy's component change tick into explicit presentation and
/// sampling revisions. A visibility or colour edit only invalidates the
/// presentation; a geometry edit also invalidates the ephemeris sample.
fn track_trajectory_view_changes(
    mut commands: Commands,
    mut q_views: Query<
        (Entity, &TrajectoryView, Option<&mut TrajectoryRuntimeState>),
        Or<(Added<TrajectoryView>, Changed<TrajectoryView>)>,
    >,
) {
    for (entity, view, state) in q_views.iter_mut() {
        if let Some(mut state) = state {
            state.observe_view(view);
        } else {
            let mut state = TrajectoryRuntimeState::default();
            state.observe_view(view);
            commands.entity(entity).insert(state);
        }
    }
}

fn sample_trajectory(
    provider: &dyn EphemerisProvider,
    view: TrajectoryView,
    aligned_epoch: f64,
    anchored: bool,
    body_descriptor: Option<&BodyDescriptor>,
) -> Result<TrajectorySample, TrajectorySampleError> {
    if !view.sampling_step.is_finite() || view.sampling_step <= 0.0 {
        return Err(TrajectorySampleError::InvalidSampling);
    }
    if view.frame == TrajectoryFrame::BodyFixed && body_descriptor.is_none() {
        return Err(TrajectorySampleError::MissingBodyDescriptor);
    }

    let anchor = if anchored {
        let (Some(target), Some(reference)) = (
            provider.global_position(view.tracked_id, aligned_epoch),
            provider.global_position(view.reference_id, aligned_epoch),
        ) else {
            return Err(TrajectorySampleError::MissingAnchor);
        };
        let anchor = crate::coords::ecliptic_to_bevy(target - reference).raw();
        if !anchor.is_finite() {
            return Err(TrajectorySampleError::NonFiniteSample);
        }
        anchor
    } else {
        DVec3::ZERO
    };

    let mut points = Vec::new();
    let mut append_sample = |jd: f64| {
        let (Some(target), Some(reference)) = (
            provider.global_position(view.tracked_id, jd),
            provider.global_position(view.reference_id, jd),
        ) else {
            return Ok(());
        };
        let mut relative = crate::coords::ecliptic_to_bevy(target - reference).raw();
        if view.frame == TrajectoryFrame::BodyFixed {
            // The IAU model is the sole owner of the body-fixed conversion.
            let Some(body_descriptor) = body_descriptor else {
                return Err(TrajectorySampleError::MissingBodyDescriptor);
            };
            relative = crate::geo::body_rotation(body_descriptor, jd).inverse() * relative;
        }
        let point = relative - anchor;
        if !point.is_finite() {
            return Err(TrajectorySampleError::NonFiniteSample);
        }
        points.push(point);
        Ok(())
    };

    match (view.start_epoch, view.end_epoch) {
        (Some(start), Some(end)) => {
            if !start.is_finite() || !end.is_finite() || end < start {
                return Err(TrajectorySampleError::InvalidSampling);
            }
            let count = ((end - start) / view.sampling_step).ceil();
            if !count.is_finite() || count < 0.0 || count + 1.0 > MAX_TRAJECTORY_SAMPLES as f64 {
                return Err(TrajectorySampleError::TooManySamples);
            }
            for i in 0..=count as usize {
                let jd = start + i as f64 * view.sampling_step;
                if jd > end {
                    break;
                }
                append_sample(jd)?;
            }
        }
        (None, None) => {
            if !view.sampling_days.is_finite() || view.sampling_days < 0.0 {
                return Err(TrajectorySampleError::InvalidSampling);
            }
            let half_count = (view.sampling_days / view.sampling_step / 2.0).ceil();
            if !half_count.is_finite()
                || half_count < 0.0
                || 2.0 * half_count + 1.0 > MAX_TRAJECTORY_SAMPLES as f64
            {
                return Err(TrajectorySampleError::TooManySamples);
            }
            let half_count = half_count as usize;
            for i in -(half_count as isize)..=(half_count as isize) {
                append_sample(aligned_epoch + i as f64 * view.sampling_step)?;
            }
        }
        _ => return Err(TrajectorySampleError::InvalidSampling),
    }

    if points.is_empty() {
        return Err(TrajectorySampleError::NoSamples);
    }
    Ok(TrajectorySample {
        points,
        epoch: aligned_epoch,
        anchor,
        anchored,
    })
}

fn trajectory_needs_update_revisioned(
    view: &TrajectoryView,
    path: &TrajectoryPath,
    state: &TrajectoryRuntimeState,
    current_epoch: f64,
    hold_curve: bool,
    frame_revision: u64,
    provider_revision: EphemerisRevision,
) -> bool {
    let signature = trajectory_view_signature(view);
    if state.view_signature != Some(signature)
        || state.resolved_sampling_revision != Some(state.sampling_revision)
        || state.resolved_frame_revision != Some(frame_revision)
        || state.resolved_provider_revision != Some(provider_revision)
    {
        return true;
    }
    match state.status {
        TrajectorySamplingStatus::Pending => true,
        TrajectorySamplingStatus::Empty | TrajectorySamplingStatus::Failed => false,
        TrajectorySamplingStatus::Ready => {
            if path.points.is_empty() {
                return true;
            }
            if view.start_epoch.is_some() && view.end_epoch.is_some() {
                return false;
            }
            if hold_curve {
                return false;
            }
            (path.update_epoch - current_epoch).abs() > view.sampling_step
        }
    }
}

fn spawn_trajectory_update_task(
    world: Res<WorldTime>,
    real: Res<Time<bevy::time::Real>>,
    clocks: Option<Res<lunco_time::Clocks>>,
    resolved: Option<Res<lunco_time::ResolvedDomains>>,
    ephemeris: Option<Res<EphemerisResource>>,
    registry: Res<CelestialBodyRegistry>,
    mut commands: Commands,
    mut presentation: ResMut<TrajectoryPresentationState>,
    mut q_views: Query<
        (
            Entity,
            &TrajectoryView,
            &mut TrajectoryPath,
            &mut TrajectoryRuntimeState,
        ),
        (
            Without<TrajectoryTask>,
            Without<TrajectoryMeshTask>,
            Without<TrajectoryAlphaTask>,
        ),
    >,
    frame_index: Res<crate::ReferenceFrameIndex>,
    q_domains: Query<&lunco_time::TimeDomain>,
) {
    let celestial_domain = clocks
        .as_deref()
        .and_then(|clocks| q_domains.get(clocks.celestial).ok());
    let domain_scale = celestial_domain.map(|domain| domain.scale);
    let effective_rate =
        clocks
            .as_deref()
            .zip(resolved.as_deref())
            .and_then(|(clocks, resolved)| {
                let real_dt = real.delta_secs_f64();
                (real_dt > 0.0).then(|| resolved.delta(clocks.celestial) / real_dt)
            });
    let hold_curve = celestial_clock_is_high_rate(world.regime, domain_scale, effective_rate);
    presentation.hold_curve = hold_curve;
    let provider_revision = ephemeris_revision(ephemeris.as_deref());
    let now_real = real.elapsed_secs_f64();
    let pool = bevy::tasks::ComputeTaskPool::get();

    for (entity, view, mut path, mut state) in q_views.iter_mut() {
        if state.view_signature.is_none() {
            state.observe_view(view);
        }
        if !trajectory_is_active(view) {
            continue;
        }
        let needs_update = trajectory_needs_update_revisioned(
            view,
            &path,
            &state,
            world.epoch_jd,
            hold_curve,
            frame_index.revision,
            provider_revision,
        );
        if !needs_update {
            continue;
        }

        let required_frame = match view.frame {
            TrajectoryFrame::Inertial => ReferenceFrame::EclipticJ2000 {
                center: view.reference_id,
            },
            TrajectoryFrame::BodyFixed => ReferenceFrame::BodyFixed {
                body: view.reference_id,
            },
        };
        if frame_index.resolve(required_frame).is_none() {
            clear_trajectory_path(&mut path);
            let sampling_revision = state.sampling_revision;
            mark_sampling_resolution(
                &mut state,
                TrajectorySamplingStatus::Failed,
                sampling_revision,
                frame_index.revision,
                provider_revision,
            );
            continue;
        }

        let body_descriptor = (view.frame == TrajectoryFrame::BodyFixed)
            .then(|| {
                registry
                    .bodies
                    .iter()
                    .find(|body| body.ephemeris_id == view.reference_id)
                    .cloned()
            })
            .flatten();
        if view.frame == TrajectoryFrame::BodyFixed && body_descriptor.is_none() {
            clear_trajectory_path(&mut path);
            let sampling_revision = state.sampling_revision;
            mark_sampling_resolution(
                &mut state,
                TrajectorySamplingStatus::Failed,
                sampling_revision,
                frame_index.revision,
                provider_revision,
            );
            continue;
        }

        if state.status == TrajectorySamplingStatus::Ready
            && !path.points.is_empty()
            && now_real - state.last_rebuild_real_secs < MIN_REBUILD_INTERVAL_SECS
        {
            continue;
        }
        let Some(ephemeris) = ephemeris.as_deref() else {
            clear_trajectory_path(&mut path);
            let sampling_revision = state.sampling_revision;
            mark_sampling_resolution(
                &mut state,
                TrajectorySamplingStatus::Failed,
                sampling_revision,
                frame_index.revision,
                provider_revision,
            );
            continue;
        };

        state.last_rebuild_real_secs = now_real;
        state.status = TrajectorySamplingStatus::Pending;
        let sampling_revision = state.sampling_revision;
        let frame_revision = frame_index.revision;
        let signature = trajectory_view_signature(view);
        let view_copy = *view;
        let provider = Arc::clone(&ephemeris.provider);
        let anchored = view.frame == TrajectoryFrame::Inertial
            && frame_index
                .resolve(ReferenceFrame::EclipticJ2000 {
                    center: view.tracked_id,
                })
                .is_some();
        let aligned_epoch = view_copy.start_epoch.unwrap_or_else(|| {
            if view_copy.sampling_step.is_finite() && view_copy.sampling_step > 0.0 {
                (world.epoch_jd / view_copy.sampling_step).round() * view_copy.sampling_step
            } else {
                world.epoch_jd
            }
        });
        let task = pool.spawn(async move {
            TrajectoryData {
                signature,
                sampling_revision,
                frame_revision,
                provider_revision,
                sample: sample_trajectory(
                    provider.as_ref(),
                    view_copy,
                    aligned_epoch,
                    anchored,
                    body_descriptor.as_ref(),
                ),
            }
        });
        commands.entity(entity).insert(TrajectoryTask(task));
    }
}

fn handle_trajectory_tasks(
    mut commands: Commands,
    frame_index: Res<crate::ReferenceFrameIndex>,
    ephemeris: Option<Res<EphemerisResource>>,
    mut q_tasks: Query<(
        Entity,
        &mut TrajectoryTask,
        &mut TrajectoryPath,
        &TrajectoryView,
        &mut TrajectoryRuntimeState,
    )>,
) {
    let provider_revision = ephemeris_revision(ephemeris.as_deref());
    for (entity, mut task, mut path, view, mut state) in q_tasks.iter_mut() {
        let Some(data) = future::block_on(future::poll_once(&mut task.0)) else {
            continue;
        };
        commands.entity(entity).remove::<TrajectoryTask>();
        if data.signature != trajectory_view_signature(view)
            || data.sampling_revision != state.sampling_revision
            || data.frame_revision != frame_index.revision
            || data.provider_revision != provider_revision
        {
            continue;
        }

        match data.sample {
            Ok(sample) => {
                path.points = sample.points;
                path.update_epoch = sample.epoch;
                path.anchor = sample.anchor;
                path.anchored = sample.anchored;
                path.geometry_revision = path.geometry_revision.wrapping_add(1);
                mark_sampling_resolution(
                    &mut state,
                    TrajectorySamplingStatus::Ready,
                    data.sampling_revision,
                    data.frame_revision,
                    data.provider_revision,
                );
                debug!(
                    "Trajectory updated for entity {:?} with {} points (anchor |{:.3e}| m). Tracking {}, Reference {}",
                    entity,
                    path.points.len(),
                    path.anchor.length(),
                    view.tracked_id,
                    view.reference_id
                );
            }
            Err(error) => {
                clear_trajectory_path(&mut path);
                let status = if error == TrajectorySampleError::NoSamples {
                    TrajectorySamplingStatus::Empty
                } else {
                    TrajectorySamplingStatus::Failed
                };
                mark_sampling_resolution(
                    &mut state,
                    status,
                    data.sampling_revision,
                    data.frame_revision,
                    data.provider_revision,
                );
                warn!(
                    "Trajectory {:?} is not renderable ({error:?}); sampling is fenced until its input changes",
                    entity
                );
            }
        }
    }
}

#[derive(Component)]
struct TrajectoryMeshTask(Task<TrajectoryMeshData>);

struct TrajectoryMeshData {
    geometry_revision: u64,
    presentation_revision: u64,
    color: LinearRgba,
    epoch_jd: f64,
    sampling_days: f64,
    start_epoch: Option<f64>,
    end_epoch: Option<f64>,
    result: Result<(Vec<[f32; 3]>, Vec<[f32; 4]>), TrajectoryMeshError>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TrajectoryMeshError {
    SplineConstruction,
    EmptyGeometry,
}

/// Tessellate a trajectory off the main/UI schedule. A trajectory is a view;
/// its Catmull-Rom presentation must never make the input and egui cycles wait
/// for a spline over thousands of samples.
fn trajectory_mesh_points(
    points: &[bevy::math::DVec3],
) -> Result<Vec<[f32; 3]>, TrajectoryMeshError> {
    if points.is_empty() {
        return Err(TrajectoryMeshError::EmptyGeometry);
    }
    if points.len() >= 4 {
        let control_points: Vec<Vec3> = points.iter().map(|point| point.as_vec3()).collect();
        let spline = CubicCardinalSpline::new_catmull_rom(control_points);
        let curve = spline
            .to_curve()
            .map_err(|_| TrajectoryMeshError::SplineConstruction)?;
        let count = (points.len() - 1) * 3;
        Ok(curve
            .iter_positions(count)
            .map(|point| point.to_array())
            .collect())
    } else {
        Ok(points
            .iter()
            .map(|point| point.as_vec3().to_array())
            .collect())
    }
}

fn trajectory_color(color: LinearRgba) -> LinearRgba {
    let emissive = color * 15.0;
    LinearRgba::new(emissive.red, emissive.green, emissive.blue, 1.0)
}

fn trajectory_mesh_update_system(
    world: Res<WorldTime>,
    mut commands: Commands,
    q_paths: Query<
        (
            Entity,
            &TrajectoryPath,
            &TrajectoryView,
            &TrajectoryRuntimeState,
            Option<&TrajectoryMeshState>,
        ),
        (
            Without<TrajectoryTask>,
            Without<TrajectoryMeshTask>,
            Without<TrajectoryAlphaTask>,
        ),
    >,
) {
    let pool = bevy::tasks::ComputeTaskPool::get();
    for (entity, path, view, runtime, mesh_state) in q_paths.iter() {
        if !trajectory_is_renderable(view, runtime, path) {
            continue;
        }
        let mesh_is_current = mesh_state.is_some_and(|state| {
            state.geometry_revision == Some(path.geometry_revision)
                && state.presentation_revision == Some(runtime.presentation_revision)
                && (state.num_points > 0 || state.failed)
        });
        if mesh_is_current {
            continue;
        }

        let source = path.points.clone();
        let geometry_revision = path.geometry_revision;
        let presentation_revision = runtime.presentation_revision;
        let trajectory_epoch = path.update_epoch;
        let epoch_jd = world.epoch_jd;
        let sampling_days = view.sampling_days;
        let start_epoch = view.start_epoch;
        let end_epoch = view.end_epoch;
        let color = view.color;
        let task = pool.spawn(async move {
            let result = trajectory_mesh_points(&source).map(|points| {
                let fade_start = start_epoch.unwrap_or(trajectory_epoch - sampling_days / 2.0);
                let total_sampling_days = match (start_epoch, end_epoch) {
                    (Some(start), Some(end)) => end - start,
                    _ => sampling_days,
                };
                let colors = if source.len() < 2 {
                    vec![[1.0, 1.0, 1.0, 1.0]; points.len()]
                } else {
                    trajectory_alpha_colors(epoch_jd, fade_start, total_sampling_days, points.len())
                };
                (points, colors)
            });
            TrajectoryMeshData {
                geometry_revision,
                presentation_revision,
                color,
                epoch_jd,
                sampling_days,
                start_epoch,
                end_epoch,
                result,
            }
        });
        commands.entity(entity).insert(TrajectoryMeshTask(task));
    }
}

fn handle_trajectory_mesh_tasks(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut q_tasks: Query<(
        Entity,
        &mut TrajectoryMeshTask,
        &TrajectoryPath,
        &TrajectoryView,
        &TrajectoryRuntimeState,
        &Children,
    )>,
    mut q_marker: Query<(&Mesh3d, &mut PbrLook), With<TrajectoryMeshMarker>>,
    mut q_visibility: Query<&mut Visibility>,
) {
    for (entity, mut task, path, view, runtime, children) in q_tasks.iter_mut() {
        let Some(data) = future::block_on(future::poll_once(&mut task.0)) else {
            continue;
        };
        commands.entity(entity).remove::<TrajectoryMeshTask>();

        if data.geometry_revision != path.geometry_revision
            || data.presentation_revision != runtime.presentation_revision
            || !trajectory_is_renderable(view, runtime, path)
        {
            continue;
        }
        let (points, colors) = match data.result {
            Ok(result) => result,
            Err(error) => {
                warn!(
                    "Trajectory mesh {:?} was not built ({error:?}); presentation is hidden until its input changes",
                    entity
                );
                for child in children.iter() {
                    if q_marker.get(child).is_ok() {
                        if let Ok(mut visibility) = q_visibility.get_mut(child) {
                            *visibility = Visibility::Hidden;
                        }
                    }
                }
                commands.entity(entity).insert(TrajectoryMeshState {
                    geometry_revision: Some(data.geometry_revision),
                    presentation_revision: Some(data.presentation_revision),
                    num_points: 0,
                    failed: true,
                });
                continue;
            }
        };
        let num_points = colors.len();
        let alpha_epoch = data.epoch_jd;
        let alpha_geometry_revision = data.geometry_revision;
        let alpha_sampling_days = data.sampling_days;
        let alpha_start_epoch = data.start_epoch;
        let alpha_end_epoch = data.end_epoch;

        let mesh_handle = children
            .iter()
            .find_map(|child| q_marker.get(child).ok().map(|(mesh, _)| mesh.0.clone()));
        if let Some(mesh_handle) = mesh_handle {
            let mut committed = false;
            if let Some(mut mesh) = meshes.get_mut(&mesh_handle) {
                mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, points);
                mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
                committed = true;
            }
            if committed {
                if let Some(child) = children.iter().find(|child| q_marker.get(*child).is_ok()) {
                    if let Ok((_, mut look)) = q_marker.get_mut(child) {
                        look.base_color = trajectory_color(data.color);
                        look.unshared = true;
                    }
                }
            }
            if committed && path.points.len() >= 2 {
                commands.entity(entity).insert((
                    TrajectoryMeshState {
                        geometry_revision: Some(data.geometry_revision),
                        presentation_revision: Some(data.presentation_revision),
                        num_points,
                        failed: false,
                    },
                    TrajectoryAlphaState {
                        epoch_jd: Some(alpha_epoch),
                        geometry_revision: Some(alpha_geometry_revision),
                        sampling_days: alpha_sampling_days,
                        start_epoch: alpha_start_epoch,
                        end_epoch: alpha_end_epoch,
                        num_points,
                    },
                ));
            } else if committed {
                commands.entity(entity).insert(TrajectoryMeshState {
                    geometry_revision: Some(data.geometry_revision),
                    presentation_revision: Some(data.presentation_revision),
                    num_points,
                    failed: false,
                });
            } else {
                warn!(
                    "Trajectory mesh asset for {:?} is unavailable; presentation is hidden until its input changes",
                    entity
                );
                for child in children.iter() {
                    if q_marker.get(child).is_ok() {
                        if let Ok(mut visibility) = q_visibility.get_mut(child) {
                            *visibility = Visibility::Hidden;
                        }
                    }
                }
                commands.entity(entity).insert(TrajectoryMeshState {
                    geometry_revision: Some(data.geometry_revision),
                    presentation_revision: Some(data.presentation_revision),
                    num_points: 0,
                    failed: true,
                });
            }
            continue;
        }

        // Bevy 0.19's slab allocator allocates no storage for a zero-byte mesh,
        // but the render extraction path still attempts its copy. Do not create
        // a placeholder mesh: publish this child only with its first real point set.
        let mut mesh = Mesh::new(
            PrimitiveTopology::LineStrip,
            RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
        );
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, points);
        mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
        let mesh_handle = meshes.add(mesh);
        commands.entity(entity).insert(TrajectoryMeshState {
            geometry_revision: Some(data.geometry_revision),
            presentation_revision: Some(data.presentation_revision),
            num_points,
            failed: false,
        });
        if path.points.len() >= 2 {
            commands.entity(entity).insert(TrajectoryAlphaState {
                epoch_jd: Some(alpha_epoch),
                geometry_revision: Some(alpha_geometry_revision),
                sampling_days: alpha_sampling_days,
                start_epoch: alpha_start_epoch,
                end_epoch: alpha_end_epoch,
                num_points,
            });
        }
        // ALPHA 1.0 IS CORRECT HERE, and it is correct for the opposite reason to
        // `assets/shaders/starfield.wgsl`, which must output alpha 0 to be additive.
        // The trajectory material reaches bevy's premultiply path, so zeroing the
        // vertex alpha would make the line invisible rather than more additive.
        let look = PbrLook {
            base_color: trajectory_color(data.color),
            unlit: true,
            alpha: SurfaceAlpha::Add,
            unshared: true,
            ..default()
        };
        commands.entity(entity).with_children(|parent| {
            parent.spawn((
                Mesh3d(mesh_handle),
                look,
                TrajectoryMeshMarker,
                bevy::picking::Pickable::IGNORE,
                Visibility::Visible,
                NoFrustumCulling,
                Transform::default(),
                big_space::grid::propagation::LowPrecisionRoot,
            ));
        });
    }
}

#[derive(Component)]
struct TrajectoryAlphaTask(Task<TrajectoryAlphaData>);

struct TrajectoryAlphaData {
    epoch_jd: f64,
    geometry_revision: u64,
    sampling_days: f64,
    start_epoch: Option<f64>,
    end_epoch: Option<f64>,
    num_points: usize,
    colors: Vec<[f32; 4]>,
}

fn trajectory_alpha_colors(
    epoch_jd: f64,
    start_epoch: f64,
    total_sampling_days: f64,
    num_points: usize,
) -> Vec<[f32; 4]> {
    (0..num_points)
        .map(|i| {
            let t = i as f64 / (num_points - 1) as f64;
            let pt_epoch = start_epoch + t * total_sampling_days;

            let days_past = epoch_jd - pt_epoch;
            let alpha = if days_past > 0.0 {
                // Smoothly fade out the past trajectory over 10% of total duration
                // (capped between 1 to 20 days).
                let fade_days = (total_sampling_days * 0.1).clamp(1.0, 20.0);
                let a = 1.0 - (days_past / fade_days);
                // With additive blending at 15x brightness, alpha must approach
                // zero rather than 0.05.
                a.max(0.001) as f32
            } else {
                1.0
            };

            // RGB is 1.0 — the MATERIAL owns the tint. Alpha is the only
            // genuinely per-vertex part of this attribute.
            [1.0, 1.0, 1.0, alpha]
        })
        .collect()
}

fn trajectory_alpha_update_system(
    world: Res<WorldTime>,
    presentation: Res<TrajectoryPresentationState>,
    mut commands: Commands,
    meshes: Res<Assets<Mesh>>,
    q_paths: Query<
        (
            Entity,
            &TrajectoryPath,
            &TrajectoryView,
            &TrajectoryRuntimeState,
            &Children,
            Option<&TrajectoryMeshState>,
            Option<&TrajectoryAlphaState>,
        ),
        (
            Without<TrajectoryTask>,
            Without<TrajectoryAlphaTask>,
            Without<TrajectoryMeshTask>,
        ),
    >,
    q_marker: Query<&Mesh3d, With<TrajectoryMeshMarker>>,
) {
    // This system only schedules a worker. It never constructs the color
    // buffer or mutates a mesh, so a moving epoch cannot block the UI cycle.
    for (entity, path, view, runtime, children, mesh_state, state) in q_paths.iter() {
        if !trajectory_is_presented(view, runtime, path, mesh_state) || path.points.len() < 2 {
            continue;
        }
        let Some((num_points, has_colors)) = children.iter().find_map(|child| {
            let mesh_handle = q_marker.get(child).ok()?;
            let mesh = meshes.get(&mesh_handle.0)?;
            let positions = mesh.attribute(Mesh::ATTRIBUTE_POSITION)?;
            Some((
                positions.len(),
                mesh.attribute(Mesh::ATTRIBUTE_COLOR).is_some(),
            ))
        }) else {
            continue;
        };
        if !alpha_update_is_needed(
            state,
            world.epoch_jd,
            path,
            view,
            num_points,
            presentation.hold_curve,
        ) && has_colors
        {
            continue;
        }

        let epoch_jd = world.epoch_jd;
        let geometry_revision = path.geometry_revision;
        let trajectory_epoch = path.update_epoch;
        let sampling_days = view.sampling_days;
        let start_epoch = view.start_epoch;
        let end_epoch = view.end_epoch;
        let fade_start = start_epoch.unwrap_or(trajectory_epoch - sampling_days / 2.0);
        let total_sampling_days = match (start_epoch, end_epoch) {
            (Some(start), Some(end)) => end - start,
            _ => sampling_days,
        };
        let task = bevy::tasks::ComputeTaskPool::get().spawn(async move {
            TrajectoryAlphaData {
                epoch_jd,
                geometry_revision,
                sampling_days,
                start_epoch,
                end_epoch,
                num_points,
                colors: trajectory_alpha_colors(
                    epoch_jd,
                    fade_start,
                    total_sampling_days,
                    num_points,
                ),
            }
        });
        commands.entity(entity).insert(TrajectoryAlphaTask(task));
    }
}

fn handle_trajectory_alpha_tasks(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut q_tasks: Query<(
        Entity,
        &mut TrajectoryAlphaTask,
        &TrajectoryPath,
        &TrajectoryView,
        &TrajectoryRuntimeState,
        &Children,
        Option<&TrajectoryMeshState>,
    )>,
    q_marker: Query<&Mesh3d, With<TrajectoryMeshMarker>>,
) {
    for (entity, mut task, path, view, runtime, children, mesh_state) in q_tasks.iter_mut() {
        let Some(data) = future::block_on(future::poll_once(&mut task.0)) else {
            continue;
        };
        commands.entity(entity).remove::<TrajectoryAlphaTask>();

        if data.geometry_revision != path.geometry_revision
            || data.sampling_days != view.sampling_days
            || data.start_epoch != view.start_epoch
            || data.end_epoch != view.end_epoch
            || !trajectory_is_presented(view, runtime, path, mesh_state)
        {
            continue;
        }

        let mut updated = false;
        for child in children.iter() {
            let Ok(mesh_handle) = q_marker.get(child) else {
                continue;
            };
            let Some(mut mesh) = meshes.get_mut(&mesh_handle.0) else {
                continue;
            };
            let Some(positions) = mesh.attribute(Mesh::ATTRIBUTE_POSITION) else {
                continue;
            };
            if positions.len() != data.num_points {
                continue;
            }
            mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, data.colors.clone());
            updated = true;
            trace!("Trajectory alpha updated for {} points", data.num_points);
        }

        if updated {
            commands.entity(entity).insert(TrajectoryAlphaState {
                epoch_jd: Some(data.epoch_jd),
                geometry_revision: Some(data.geometry_revision),
                sampling_days: data.sampling_days,
                start_epoch: data.start_epoch,
                end_epoch: data.end_epoch,
                num_points: data.num_points,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hidden_trajectory_is_not_active() {
        let mut view = TrajectoryView::default();
        view.user_visible = false;
        assert!(!trajectory_is_active(&view));

        view.user_visible = true;
        view.is_visible = false;
        assert!(!trajectory_is_active(&view));

        view.is_visible = true;
        assert!(trajectory_is_active(&view));
    }

    #[test]
    fn alpha_stamp_skips_unchanged_clock_and_path() {
        let view = TrajectoryView::default();
        let path = TrajectoryPath {
            points: vec![bevy::math::DVec3::ZERO, bevy::math::DVec3::X],
            update_epoch: 2451545.0,
            ..Default::default()
        };
        let state = TrajectoryAlphaState {
            epoch_jd: Some(2451545.5),
            geometry_revision: Some(path.geometry_revision),
            sampling_days: view.sampling_days,
            start_epoch: view.start_epoch,
            end_epoch: view.end_epoch,
            num_points: 2,
        };

        assert!(!alpha_update_is_needed(
            Some(&state),
            2451545.5,
            &path,
            &view,
            2,
            false
        ));
        assert!(alpha_update_is_needed(
            Some(&state),
            2451545.6,
            &path,
            &view,
            2,
            false
        ));
        assert!(!alpha_update_is_needed(
            Some(&state),
            2451545.6,
            &path,
            &view,
            2,
            true
        ));
        assert!(alpha_update_is_needed(
            Some(&state),
            2451545.6,
            &TrajectoryPath {
                geometry_revision: path.geometry_revision + 1,
                ..path
            },
            &view,
            2,
            true
        ));
    }

    #[test]
    fn kinematic_warp_holds_an_existing_trajectory_sample() {
        let view = TrajectoryView::default();
        let path = TrajectoryPath {
            points: vec![bevy::math::DVec3::ZERO, bevy::math::DVec3::X],
            update_epoch: 2451545.0,
            ..Default::default()
        };

        let mut runtime = TrajectoryRuntimeState::default();
        runtime.observe_view(&view);
        runtime.status = TrajectorySamplingStatus::Ready;
        runtime.resolved_sampling_revision = Some(runtime.sampling_revision);
        runtime.resolved_frame_revision = Some(0);
        runtime.resolved_provider_revision = Some(EphemerisRevision {
            available: true,
            motion: 0,
        });
        assert!(!trajectory_needs_update_revisioned(
            &view,
            &path,
            &runtime,
            2451600.0,
            true,
            0,
            EphemerisRevision {
                available: true,
                motion: 0,
            },
        ));
        assert!(trajectory_needs_update_revisioned(
            &view,
            &TrajectoryPath::default(),
            &runtime,
            2451600.0,
            true,
            0,
            EphemerisRevision {
                available: true,
                motion: 0,
            },
        ));
    }

    #[test]
    fn view_revision_separates_presentation_from_sampling() {
        let mut view = TrajectoryView::default();
        let mut state = TrajectoryRuntimeState::default();
        state.observe_view(&view);
        let sampling_revision = state.sampling_revision;

        view.color = LinearRgba::RED;
        state.observe_view(&view);
        assert!(state.presentation_revision > 1);
        assert_eq!(state.sampling_revision, sampling_revision);

        view.sampling_step *= 2.0;
        state.observe_view(&view);
        assert!(state.sampling_revision > sampling_revision);
        assert_eq!(state.status, TrajectorySamplingStatus::Pending);
    }

    #[test]
    fn resolved_failure_does_not_reschedule_without_new_inputs() {
        let view = TrajectoryView::default();
        let mut state = TrajectoryRuntimeState::default();
        state.observe_view(&view);
        state.status = TrajectorySamplingStatus::Failed;
        state.resolved_sampling_revision = Some(state.sampling_revision);
        state.resolved_frame_revision = Some(3);
        state.resolved_provider_revision = Some(EphemerisRevision {
            available: true,
            motion: 4,
        });
        assert!(!trajectory_needs_update_revisioned(
            &view,
            &TrajectoryPath::default(),
            &state,
            2451545.0,
            false,
            3,
            EphemerisRevision {
                available: true,
                motion: 4,
            },
        ));
        assert!(trajectory_needs_update_revisioned(
            &view,
            &TrajectoryPath::default(),
            &state,
            2451545.0,
            false,
            4,
            EphemerisRevision {
                available: true,
                motion: 4,
            },
        ));
    }

    #[test]
    fn missing_provider_is_reopened_when_provider_appears() {
        let view = TrajectoryView::default();
        let mut state = TrajectoryRuntimeState::default();
        state.observe_view(&view);
        state.status = TrajectorySamplingStatus::Failed;
        state.resolved_sampling_revision = Some(state.sampling_revision);
        state.resolved_frame_revision = Some(3);
        state.resolved_provider_revision = Some(EphemerisRevision {
            available: false,
            motion: 0,
        });

        assert!(!trajectory_needs_update_revisioned(
            &view,
            &TrajectoryPath::default(),
            &state,
            2451545.0,
            false,
            3,
            EphemerisRevision {
                available: false,
                motion: 0,
            },
        ));
        assert!(trajectory_needs_update_revisioned(
            &view,
            &TrajectoryPath::default(),
            &state,
            2451545.0,
            false,
            3,
            EphemerisRevision {
                available: true,
                motion: 0,
            },
        ));
    }

    #[test]
    fn independently_scaled_celestial_clock_is_high_rate() {
        assert!(celestial_clock_is_high_rate(
            lunco_time::TimeRegime::RealtimePhysics,
            Some(100_000.0),
            None
        ));
        assert!(celestial_clock_is_high_rate(
            lunco_time::TimeRegime::RealtimePhysics,
            Some(1.0),
            Some(100_000.0)
        ));
        assert!(!celestial_clock_is_high_rate(
            lunco_time::TimeRegime::RealtimePhysics,
            Some(1.0),
            Some(1.0)
        ));
    }
}

pub fn mission_visibility_system(world: Res<WorldTime>, mut q_views: Query<&mut TrajectoryView>) {
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

fn trajectory_visibility_system(
    q_views: Query<
        (
            Entity,
            &TrajectoryView,
            &TrajectoryPath,
            &TrajectoryRuntimeState,
            Option<&TrajectoryMeshState>,
        ),
        Or<(
            Changed<TrajectoryView>,
            Changed<TrajectoryPath>,
            Changed<TrajectoryRuntimeState>,
            Changed<TrajectoryMeshState>,
        )>,
    >,
    q_children: Query<&Children>,
    q_marker: Query<(), With<TrajectoryMeshMarker>>,
    mut q_visibility: Query<&mut Visibility>,
) {
    for (entity, view, path, runtime, mesh_state) in q_views.iter() {
        trajectory_children_visible(
            entity,
            trajectory_is_presented(view, runtime, path, mesh_state),
            &q_children,
            &q_marker,
            &mut q_visibility,
        );
    }
}

fn trajectory_alignment_system(
    mut commands: Commands,
    frame_index: Res<crate::ReferenceFrameIndex>,
    q_grids: Query<&big_space::prelude::Grid>,
    q_parents: Query<&ChildOf>,
    // Trajectory views are the only mutable spatial entities in this system.
    // Excluding them from this read query makes the access disjoint while still
    // allowing the canonical BigSpace pose helper to walk every celestial
    // frame/grid ancestor.
    q_spatial: Query<(Option<&CellCoord>, &Transform), Without<TrajectoryPath>>,
    mut q_vistas: Query<
        (
            Entity,
            &TrajectoryView,
            &TrajectoryPath,
            Option<&TrajectoryRuntimeState>,
            Option<&TrajectoryMeshState>,
            &mut Transform,
            Option<&mut CellCoord>,
            Option<&ChildOf>,
        ),
        (Without<ReferenceFrame>,),
    >,
    q_view_children: Query<&Children>,
    q_traj_mesh: Query<(), With<TrajectoryMeshMarker>>,
    mut q_visibility: Query<&mut Visibility>,
) {
    for (v_entity, view, path, runtime, mesh_state, mut transform, cell, current_parent) in
        q_vistas.iter_mut()
    {
        let mut target_parent = None;
        let mut parent_grid: Option<&big_space::prelude::Grid> = None;
        // For anchored views: the tracked body's CURRENT position in the SAME
        // reference frame `path.anchor` was sampled in, so the curve rides the
        // body continuously (cancels drift since the anchor epoch) — see the
        // placement write below.
        let mut tracked_translation: Option<bevy::math::DVec3> = None;

        if view.frame == TrajectoryFrame::BodyFixed {
            // Body-fixed points belong on the body's rotating reference-frame
            // Grid. The grid rotation supplies the body-fixed axes.
            if let Some(f_entity) = frame_index.resolve(ReferenceFrame::BodyFixed {
                body: view.reference_id,
            }) {
                target_parent = Some(f_entity);
                parent_grid = q_grids.get(f_entity).ok();
            }
        } else if path.anchored {
            // Anchored points are relative to the tracked body at the sample
            // epoch. Parent to that body's frame and subtract its current
            // reference-frame position so the inertial curve remains stable.
            if let Some(f_entity) = frame_index.resolve(ReferenceFrame::EclipticJ2000 {
                center: view.tracked_id,
            }) {
                if let Ok(grid) = q_grids.get(f_entity) {
                    target_parent = Some(f_entity);
                    parent_grid = Some(grid);
                    // The tracked body's position relative to `reference_id`,
                    // at the CURRENT epoch — the same quantity, in the same
                    // frame, that `spawn_trajectory_update_task` sampled into
                    // `path.anchor`. Read from the complete BigSpace pose, not
                    // a sub-cell transform:
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
                    // The celestial frame cluster already owns the current
                    // tracked-body pose in BigSpace cells/transforms. Resolve
                    // that pose in the declared reference grid instead of
                    // evaluating both ephemeris endpoints again for every
                    // trajectory on every high-rate frame. This is the same
                    // canonical f64 hierarchy composition used by physics,
                    // placement, and surface coordinates; no second orbital
                    // calculation or coordinate conversion lives here.
                    tracked_translation = frame_index
                        .resolve(ReferenceFrame::EclipticJ2000 {
                            center: view.reference_id,
                        })
                        .and_then(|reference_grid| {
                            lunco_core::coords::pose_in_grid(
                                f_entity,
                                reference_grid,
                                &q_parents,
                                &q_grids,
                                &q_spatial,
                            )
                            .map(|(position, _)| position)
                        });
                }
            }
        } else {
            // Unanchored inertial mission/spacecraft paths are expressed in
            // the selected reference body's inertial axes. Parenting to that
            // explicit frame means no counter-rotation or special Sun path.
            if let Some(f_entity) = frame_index.resolve(ReferenceFrame::EclipticJ2000 {
                center: view.reference_id,
            }) {
                target_parent = Some(f_entity);
                parent_grid = q_grids.get(f_entity).ok();
            }
        }

        if path.anchored && tracked_translation.is_none() {
            trajectory_children_visible(
                v_entity,
                false,
                &q_view_children,
                &q_traj_mesh,
                &mut q_visibility,
            );
            continue;
        }

        if let (Some(parent_ent), Some(parent_grid)) = (target_parent, parent_grid) {
            let is_current_parent = current_parent
                .map(|p| p.parent() == parent_ent)
                .unwrap_or(false);
            let had_cell = cell.is_some();
            // Desired local position in the parent frame. For anchored views,
            // `path.anchor` (body pos at the rebuild epoch) minus the body's
            // CURRENT position in that same frame = -drift. That keeps the
            // curve's "now" point glued to the rendered body as it orbits — no
            // rebuild-snap. Non-anchored/BodyFixed views want ZERO.
            //
            let desired_local = match tracked_translation {
                Some(ft) => path.anchor - ft,
                None => bevy::math::DVec3::ZERO,
            };
            let (new_cell, new_translation) = parent_grid.translation_to_grid(desired_local);
            let next_transform =
                Transform::from_translation(new_translation).with_rotation(Quat::IDENTITY);
            if !is_current_parent {
                lunco_core::attach::migrate_to_grid(
                    &mut commands,
                    v_entity,
                    parent_ent,
                    new_cell,
                    next_transform,
                );
            } else {
                if let Some(mut cell) = cell {
                    if *cell != new_cell {
                        *cell = new_cell;
                    }
                } else {
                    commands.entity(v_entity).try_insert(new_cell);
                }
                if transform.translation != new_translation || transform.rotation != Quat::IDENTITY
                {
                    transform.translation = new_translation;
                    transform.rotation = Quat::IDENTITY;
                }
            }
            // The mesh child is a low-precision subtree below this cell entity.
            // Stamp that ownership when the view first receives its target
            // parent or its first cell, including the asynchronous spawn case.
            if !is_current_parent || !had_cell {
                if let Ok(children) = q_view_children.get(v_entity) {
                    for child in children.iter() {
                        if q_traj_mesh.contains(child) {
                            commands
                                .entity(child)
                                .try_insert(big_space::grid::propagation::LowPrecisionRoot);
                        }
                    }
                }
            }
            trajectory_children_visible(
                v_entity,
                runtime.is_some_and(|runtime| {
                    trajectory_is_presented(view, runtime, path, mesh_state)
                }),
                &q_view_children,
                &q_traj_mesh,
                &mut q_visibility,
            );
        } else {
            trajectory_children_visible(
                v_entity,
                false,
                &q_view_children,
                &q_traj_mesh,
                &mut q_visibility,
            );
        }
    }
}

/// A trajectory's frame assignment is structural state, independent of the
/// ephemeris cadence. This condition makes creation, edits, and an external
/// reparent converge to the declared frame in the same frame while leaving
/// epoch-only realignment on the shared celestial angular-error budget.
fn trajectory_frame_assignment_changed(
    changed: Query<
        (),
        (
            With<TrajectoryView>,
            Or<(
                Changed<TrajectoryView>,
                Changed<TrajectoryPath>,
                Changed<ChildOf>,
            )>,
        ),
    >,
    frame_index: Res<crate::ReferenceFrameIndex>,
) -> bool {
    frame_index.is_changed() || !changed.is_empty()
}
