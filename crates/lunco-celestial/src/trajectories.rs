use crate::ephemeris::EphemerisResource;
use crate::registry::{CelestialBodyRegistry, ReferenceFrame};
use bevy::asset::RenderAssetUsages;
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

// REMOVED (2026-07-13, render decoupling): `TrajectoryExtension` /
// `TrajectoryMaterial` (an `ExtendedMaterial<StandardMaterial, _>`),
// `TrajectoryShaderPlugin` and `TrajectoryShaderHandle`. The material type was
// DEAD — declared, `AsBindGroup`-derived, and never instantiated anywhere in the
// workspace (`trajectory_mesh_update_system` uses a plain unlit `PbrLook`).
// All it did was pull `bevy_pbr` +
// `bevy_shader` (→ naga) into every binary that links this crate, and register a
// `trajectory.wgsl` no pipeline ever read. `assets/shaders/trajectory.wgsl` is left
// on disk; nothing loads it.

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
    /// f32 mesh vertices stay SMALL near the tracked body. `ZERO` for
    /// un-anchored (mission/spacecraft) paths. See
    /// `spawn_trajectory_update_task`.
    pub anchor: bevy::math::DVec3,
    /// `Time<Real>` seconds at the last rebuild — the wall-clock rate limiter.
    ///
    /// A trajectory is a **view** of a slowly-varying ellipse, and its rebuild cost is
    /// real (1 500–2 400 ephemeris samples, then a mesh rebuild + GPU upload). The
    /// rebuild trigger is `|epoch − update_epoch| > sampling_step`, which is a *sim*
    /// condition — so once the celestial clock runs fast enough (100 000× advances the
    /// epoch ~1.2 days per WALL second), the trigger is open on every render frame.
    /// Active views are still bounded by the wall-clock interval below; hidden views
    /// do not sample at all.
    ///
    /// The view does not need to track a 100 000× clock at 60 Hz to look right. At
    /// realtime rates this caps rebuilds in WALL time; during high-rate Celestial
    /// transport (including an independent domain scale) the existing curve is
    /// held until realtime transport resumes.
    pub last_rebuild_real_secs: f64,
}

/// Minimum wall-clock seconds between trajectory rebuilds.
///
/// A body's orbit is a quasi-static ellipse — over one WALL second it is
/// imperceptibly different at realtime rates, because what actually moves is the body
/// *along* the curve, not the curve itself. So 1 Hz is plenty while realtime runs,
/// and high-rate Celestial transport holds the existing sample entirely. Each
/// rebuild re-samples 800–1500 ephemeris points and re-splines the mesh on the
/// main thread.
const MIN_REBUILD_INTERVAL_SECS: f64 = 1.0;

#[derive(Component)]
pub struct TrajectoryTask(pub Task<TrajectoryData>);

pub struct TrajectoryData {
    pub points: Vec<bevy::math::DVec3>,
    pub epoch: f64,
    pub anchor: bevy::math::DVec3,
}

#[derive(Component)]
pub struct TrajectoryMeshMarker;

/// Change stamp for the per-vertex time-fade attribute.
///
/// The trajectory mesh owns the geometry stamp (`Changed<TrajectoryPath>`). The
/// alpha attribute has a different producer: the current world epoch. Keep its
/// stamp on the trajectory entity so a paused or unchanged clock does not cause
/// a full color-buffer allocation and GPU upload every frame.
#[derive(Component, Default)]
struct TrajectoryAlphaState {
    epoch_jd: Option<f64>,
    path_epoch: Option<f64>,
    sampling_days: f64,
    start_epoch: Option<f64>,
    end_epoch: Option<f64>,
    num_points: usize,
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
) -> bool {
    let Some(state) = state else {
        return true;
    };
    state.epoch_jd != Some(epoch_jd)
        || state.path_epoch != Some(path.update_epoch)
        || state.sampling_days != view.sampling_days
        || state.start_epoch != view.start_epoch
        || state.end_epoch != view.end_epoch
        || state.num_points != num_points
}

#[inline]
fn trajectory_needs_update(
    view: &TrajectoryView,
    path: &TrajectoryPath,
    current_epoch: f64,
    hold_curve: bool,
) -> bool {
    if path.points.is_empty() {
        return true;
    }
    if view.start_epoch.is_some() && view.end_epoch.is_some() {
        return false;
    }
    // During high celestial warp the body/frame pose consumers continue to solve
    // from the current epoch, but the rendered orbit curve is deliberately a
    // held presentation sample. Rebuilding thousands of points once per wall
    // second adds no useful visual fidelity at 100000x and causes a main-thread
    // mesh/GPU upload spike. Re-entering realtime physics reopens this trigger.
    if hold_curve {
        return false;
    }
    (path.update_epoch - current_epoch).abs() > view.sampling_step
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
            .register_type::<TrajectoryPath>();

        // NO Rust-spawned trajectory views. An orbit line is CONTENT — a scene
        // says which paths it wants drawn, with `lunco:trajectory:*` on a prim
        // (`LunCoMissionTrajectoryAPI` → `MissionTrajectoryDecl` →
        // `spawn_declared_missions`). The Earth and Moon orbit views now live in
        // `assets/celestial/solar_system.usda` next to the bodies they belong to,
        // authored `userVisible = false`.
        //
        // The history is why this matters twice over. First these were an
        // unconditional `Startup` spawn, so every scene — including the flat
        // sandbox arena that asks for no sky — got orbit geometry for planets it
        // had not declared. Gating them on "the scene declared bodies" fixed the
        // arena but kept the deeper error: a scene could ask for a SKY and get
        // ORBIT LINES it never mentioned, with no way to say no. Declaring the
        // Sun is not asking for a 400-day Earth ellipse across your horizon.
        //
        // Now the only way an orbit line exists is that a prim asked for it, and
        // an unauthored `userVisible` reads as OFF — see `MissionTrajectoryDecl`.

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
        app.add_systems(
            Update,
            (
                mission_visibility_system,
                spawn_trajectory_update_task,
                handle_trajectory_tasks,
                trajectory_mesh_update_system,
                trajectory_alpha_update_system,
                trajectory_visibility_system,
            )
                .chain(),
        );

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
        // Read the already-projected celestial frame and write the local pose
        // before `Propagate`, so the fresh pose reaches this frame's
        // `GlobalTransform`s. The solar hierarchy is never re-posed as a pin.
        app.add_systems(
            PostUpdate,
            trajectory_alignment_system
                .before(bevy::transform::TransformSystems::Propagate)
                // Same angular budget as the rest of the celestial cluster: an
                // orbit line whose bodies moved <0.01° has not visibly moved
                // either, and re-placing it every frame cost 2.6 ms. A newly
                // authored or edited trajectory is nevertheless a structural
                // frame change and must be mounted in this frame, even when the
                // celestial epoch is standing still.
                .run_if(
                    crate::cadence::tracked_needs_solve()
                        .or_else(trajectory_frame_assignment_changed),
                ),
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
/// position relative to the origin-tracking anchor (world axes, so pure
/// camera rotation is invisible to it). A visible "jump" is a DISCONTINUITY
/// in that relative motion, i.e. a large second difference: smooth orbiting
/// (even fast dragging) produces a steady per-frame delta; a one-frame
/// convention flip / stale GT produces a delta spike. Logs the entity name,
/// the spike size, and the frame — plus a once-per-second heartbeat of the
/// largest spike seen so a silent log provably means "no jumps".
///
/// Landmarks: celestial bodies, reference-frame grids, trajectory views,
/// grid-anchored scene roots, the active avatar, streamed terrain, and the
/// `WorldGrid` (the root-composition victim class of the 2026-07-10
/// regression).  The avatar and terrain are deliberately included as a pair:
/// a floating-origin rebranch may move both in world coordinates, but it must
/// never change their relative rendered pose.
#[allow(clippy::type_complexity)]
pub fn jump_probe_system(
    q_cam: Query<&GlobalTransform, With<big_space::prelude::FloatingOrigin>>,
    q_marks: Query<
        (Entity, Option<&Name>, &GlobalTransform),
        Or<(
            With<crate::registry::CelestialBody>,
            With<ReferenceFrame>,
            With<TrajectoryView>,
            With<lunco_core::GridAnchor>,
            With<lunco_core::WorldGrid>,
            With<lunco_core::Avatar>,
            With<lunco_terrain_surface::stream_viz::TerrainLodViz>,
        )>,
    >,
    q_parents: Query<&ChildOf>,
    q_names: Query<&Name>,
    mut last: Local<std::collections::HashMap<Entity, (bevy::math::DVec3, bevy::math::DVec3)>>,
    mut last_parent: Local<std::collections::HashMap<Entity, Entity>>,
    mut frame: Local<u64>,
    mut heartbeat: Local<(f64, String)>,
    mut trace: Local<Option<Option<String>>>,
) {
    *frame += 1;
    let Ok(cam_gt) = q_cam.single() else { return };
    let cam = cam_gt.translation().as_dvec3();
    // LUNCO_GT_TRACE=<name substring>: dump matching landmarks' camera-relative
    // GT EVERY frame. Post-analysis of the series distinguishes smooth motion,
    // f32-quat ULP stepping (~1e4 m at 1.5e11), and compat-pass f32 buckets
    // (1.5e11·2⁻²³ ≈ 1.8e4 m) — different residual mechanisms, different fixes.
    if trace.is_none() {
        *trace = Some(std::env::var("LUNCO_GT_TRACE").ok());
    }
    let label = |e: Entity, q: &Query<&Name>| -> String {
        q.get(e)
            .map(|n| n.as_str().to_string())
            .unwrap_or_else(|_| format!("{e:?}"))
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
                None => {
                    last_parent.insert(e, parent);
                }
                _ => {}
            }
        }
        let p = gt.translation().as_dvec3() - cam;
        if let Some(Some(filter)) = trace.as_ref() {
            let n = name.map(|n| n.as_str()).unwrap_or("");
            if !filter.is_empty() && n.contains(filter.as_str()) {
                bevy::log::info!(
                    "[gt-trace] f{} {}: {:.3} {:.3} {:.3}",
                    *frame,
                    n,
                    p.x,
                    p.y,
                    p.z
                );
            }
        }
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
                    parent
                        .map(|pe| label(pe, &q_names))
                        .unwrap_or_else(|| "<none>".into()),
                );
            }
            if jerk > heartbeat.0 {
                *heartbeat = (
                    jerk,
                    name.map(|n| n.as_str().to_string()).unwrap_or_default(),
                );
            }
            last.insert(e, (p, d));
        } else {
            last.insert(e, (p, bevy::math::DVec3::ZERO));
        }
    }
    if (*frame).is_multiple_of(120) {
        bevy::log::info!(
            "[jump-probe] f{} heartbeat: max jerk since last = {:.3e} m ({})",
            *frame,
            heartbeat.0,
            heartbeat.1
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
    q_views: Query<
        (
            &Name,
            &TrajectoryView,
            &CellCoord,
            &Transform,
            &GlobalTransform,
            &ChildOf,
        ),
        With<TrajectoryPath>,
    >,
    q_frames: Query<(&GlobalTransform, &big_space::prelude::Grid)>,
    mut tick: Local<u32>,
) {
    *tick += 1;
    if !(*tick).is_multiple_of(20) {
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

pub fn spawn_trajectory_update_task(
    world: Res<WorldTime>,
    real: Res<Time<bevy::time::Real>>,
    clocks: Option<Res<lunco_time::Clocks>>,
    resolved: Option<Res<lunco_time::ResolvedDomains>>,
    ephemeris: Option<Res<EphemerisResource>>,
    registry: Res<CelestialBodyRegistry>,
    mut commands: Commands,
    mut q_views: Query<(Entity, &TrajectoryView, &mut TrajectoryPath), Without<TrajectoryTask>>,
    frame_index: Res<crate::ReferenceFrameIndex>,
    q_domains: Query<&lunco_time::TimeDomain>,
) {
    let Some(ephemeris) = ephemeris else {
        return;
    };
    let current_epoch = world.epoch_jd;
    let now_real = real.elapsed_secs_f64();
    let pool = bevy::tasks::ComputeTaskPool::get();
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

    for (entity, view, mut path) in q_views.iter_mut() {
        // Visibility is a consumer contract, not just a renderer hint. A hidden
        // orbit must not spend ephemeris/task/mesh budget until the user or a
        // mission range makes it active.
        if !trajectory_is_active(view) {
            continue;
        }

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
            && frame_index
                .resolve(ReferenceFrame::EclipticJ2000 {
                    center: view.tracked_id,
                })
                .is_some();
        let needs_update = trajectory_needs_update(view, &path, current_epoch, hold_curve);

        // Wall-clock rate limit for realtime transport. The trigger above is a SIM
        // condition, so a fast realtime rate can open it repeatedly; high-rate
        // Celestial transport holds existing geometry instead. The first build
        // (`points.is_empty()`) is never delayed.
        if needs_update
            && !path.points.is_empty()
            && now_real - path.last_rebuild_real_secs < MIN_REBUILD_INTERVAL_SECS
        {
            continue;
        }

        if needs_update {
            path.last_rebuild_real_secs = now_real;
            let provider = Arc::clone(&ephemeris.provider);
            let registry_arc = Arc::new((*registry).clone());
            let view_copy = *view;

            let aligned_epoch =
                if let (Some(start), Some(_end)) = (view_copy.start_epoch, view_copy.end_epoch) {
                    // If fixed range, update_epoch is not moving.
                    start
                } else {
                    (current_epoch / view_copy.sampling_step).round() * view_copy.sampling_step
                };

            let task = pool.spawn(async move {
                let mut points = Vec::new();

                // Anchor: tracked body's reference-relative position at the
                // aligned epoch — subtracted from every sample so the curve
                // is expressed relative to the tracked body (see above).
                let anchor = if anchored {
                    // No ephemeris for either end ⇒ no anchor. Falling back to ZERO would pin
                    // the trajectory to the Sun's centre and look like a real answer.
                    match (
                        provider.global_position(view_copy.tracked_id, aligned_epoch),
                        provider.global_position(view_copy.reference_id, aligned_epoch),
                    ) {
                        (Some(p_target), Some(p_ref)) => {
                            crate::coords::ecliptic_to_bevy(p_target - p_ref).raw()
                        }
                        _ => bevy::math::DVec3::ZERO,
                    }
                } else {
                    bevy::math::DVec3::ZERO
                };

                if let (Some(start), Some(end)) = (view_copy.start_epoch, view_copy.end_epoch) {
                    let count = ((end - start) / view_copy.sampling_step).ceil() as usize + 1;
                    points.reserve(count);

                    for i in 0..count {
                        let jd = start + (i as f64) * view_copy.sampling_step;
                        if jd > end {
                            break;
                        } // Don't overshoot

                        // A sample we cannot compute is a sample we do not plot — it used to
                        // become a point at the Sun's centre, dragging a spurious line across
                        // the whole solar system.
                        let (Some(p_target), Some(p_ref)) = (
                            provider.global_position(view_copy.tracked_id, jd),
                            provider.global_position(view_copy.reference_id, jd),
                        ) else {
                            continue;
                        };
                        let mut rel_pos = crate::coords::ecliptic_to_bevy(p_target - p_ref).raw();

                        if view_copy.frame == TrajectoryFrame::BodyFixed {
                            if let Some(desc) = registry_arc
                                .bodies
                                .iter()
                                .find(|b| b.ephemeris_id == view_copy.reference_id)
                            {
                                // Share `geo::body_rotation` — the IAU model — rather than
                                // re-deriving a rotation here. This local copy was a THIRD
                                // spelling of the body rotation, and it was doubly wrong:
                                // no `W₀` phase (like the original `geo`) AND it spun about
                                // the polar axis without first mapping body-fixed +Y onto
                                // it, so body-fixed ground tracks were tilted as well as
                                // rotated.
                                rel_pos = crate::geo::body_rotation(desc, jd).inverse() * rel_pos;
                            }
                        }

                        points.push(rel_pos - anchor);
                    }
                } else {
                    let half_count =
                        (view_copy.sampling_days / view_copy.sampling_step / 2.0).ceil() as isize;
                    points.reserve((half_count * 2 + 1) as usize);

                    for i in -half_count..=half_count {
                        let jd = aligned_epoch + (i as f64) * view_copy.sampling_step;
                        let (Some(p_target), Some(p_ref)) = (
                            provider.global_position(view_copy.tracked_id, jd),
                            provider.global_position(view_copy.reference_id, jd),
                        ) else {
                            continue; // no data for this sample — plot nothing, invent nothing
                        };
                        let mut rel_pos = crate::coords::ecliptic_to_bevy(p_target - p_ref).raw();

                        if view_copy.frame == TrajectoryFrame::BodyFixed {
                            if let Some(desc) = registry_arc
                                .bodies
                                .iter()
                                .find(|b| b.ephemeris_id == view_copy.reference_id)
                            {
                                // Share `geo::body_rotation` — the IAU model — rather than
                                // re-deriving a rotation here. This local copy was a THIRD
                                // spelling of the body rotation, and it was doubly wrong:
                                // no `W₀` phase (like the original `geo`) AND it spun about
                                // the polar axis without first mapping body-fixed +Y onto
                                // it, so body-fixed ground tracks were tilted as well as
                                // rotated.
                                rel_pos = crate::geo::body_rotation(desc, jd).inverse() * rel_pos;
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

            commands.entity(entity).try_insert(TrajectoryTask(task));
        }
    }
}

pub fn handle_trajectory_tasks(
    mut commands: Commands,
    mut q_tasks: Query<(
        Entity,
        &mut TrajectoryTask,
        &mut TrajectoryPath,
        &TrajectoryView,
    )>,
) {
    for (entity, mut task, mut path, view) in q_tasks.iter_mut() {
        if let Some(data) = future::block_on(future::poll_once(&mut task.0)) {
            path.points = data.points;
            path.update_epoch = data.epoch;
            path.anchor = data.anchor;
            commands.entity(entity).remove::<TrajectoryTask>();
            debug!(
                "Trajectory updated for entity {:?} with {} points (anchor |{:.3e}| m). Tracking {}, Reference {}",
                entity,
                path.points.len(),
                path.anchor.length(),
                view.tracked_id,
                view.reference_id
            );
        }
    }
}

pub fn trajectory_mesh_update_system(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    q_paths: Query<(Entity, &TrajectoryPath, &TrajectoryView, &Children), Changed<TrajectoryPath>>,
    q_marker: Query<&Mesh3d, With<TrajectoryMeshMarker>>,
) {
    for (entity, path, view, children) in q_paths.iter() {
        if !trajectory_is_active(view) || path.points.is_empty() {
            continue;
        }

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

        // `ATTRIBUTE_COLOR` is NOT written here when the alpha pass will write it.
        //
        // ⚠ VERTEX COLOUR MULTIPLIES `base_color`, IT DOES NOT REPLACE IT. bevy's
        // `pbr_fragment.wgsl` seeds `material.base_color` from the vertex colour and
        // then does `base_color *= material.base_color` — so anything this attribute
        // carries in RGB is applied ON TOP of the tint the material already holds.
        // Writing `view.color` here therefore SQUARES the tint (the material carries
        // `view.color * 15`, so the line renders `view.color² * 15`): every channel
        // below 1.0 is pulled down against the strongest one, and the line comes out
        // more saturated and darker than the colour anybody authored. It used to do
        // exactly that, on the belief — stated in this comment — that a per-vertex
        // copy of a constant "says nothing".
        //
        // So RGB is 1.0: the MATERIAL owns the tint outright, and this attribute
        // carries only what is genuinely per-vertex — the ALPHA, which fades the past
        // half of the orbit out. `trajectory_alpha_update_system` runs immediately
        // after this system in the same `.chain()`, in the same frame, and overwrites
        // the whole attribute from the vertex count it finds; building a full
        // `Vec<[f32; 4]>` here only to throw it away one system later is an
        // allocation and a GPU upload per rebuild, and a trajectory rebuild is
        // thousands of vertices.
        //
        // The one case the alpha pass declines is a path with fewer than two points,
        // where there is no time axis to fade along. Seed those here, so the
        // attribute's vertex count can never disagree with `POSITION`.
        let colors: Option<Vec<[f32; 4]>> =
            (path.points.len() < 2).then(|| vec![[1.0, 1.0, 1.0, 1.0]; final_pts.len()]);

        let mesh_handle = children.iter().find_map(|child| q_marker.get(child).ok());
        if let Some(mesh_handle) = mesh_handle {
            if let Some(mut mesh) = meshes.get_mut(&mesh_handle.0) {
                mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, final_pts);
                if let Some(colors) = colors {
                    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
                }
            }
            continue;
        }

        // Bevy 0.19's slab allocator allocates no storage for a zero-byte mesh,
        // but the render extraction path still attempts its copy. Do not create a
        // placeholder mesh: publish this child only with its first real point set.
        let mut mesh = Mesh::new(
            PrimitiveTopology::LineStrip,
            RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
        );
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, final_pts);
        if let Some(colors) = colors {
            mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
        }
        let mesh_handle = meshes.add(mesh);
        // ALPHA 1.0 IS CORRECT HERE, and it is correct for the opposite reason to
        // `assets/shaders/starfield.wgsl`, which must output alpha 0 to be additive.
        // Both take bevy's `BLEND_PREMULTIPLIED_ALPHA` pass (`src + dst * (1 - src.a)`),
        // where a non-zero alpha eats the background — but a StandardMaterial goes
        // through `premultiply_alpha()`, which under `ALPHA_MODE_ADD` returns
        // `vec4(rgb * a, 0.0)` and does the premultiply itself. The starfield is a
        // custom `ShaderMaterial` whose fragment never reaches that function, so it
        // has to premultiply by hand. Zeroing the alpha here would not make this
        // *more* additive, it would make every trajectory line INVISIBLE — `rgb * 0`.
        let emissive_color = view.color * 15.0;
        let look = PbrLook {
            base_color: LinearRgba::new(
                emissive_color.red,
                emissive_color.green,
                emissive_color.blue,
                1.0,
            ),
            unlit: true,
            alpha: SurfaceAlpha::Add,
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

fn trajectory_alpha_update_system(
    world: Res<WorldTime>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    q_paths: Query<(
        Entity,
        &TrajectoryPath,
        &TrajectoryView,
        &Children,
        Option<&TrajectoryAlphaState>,
    )>,
    q_marker: Query<&Mesh3d, With<TrajectoryMeshMarker>>,
) {
    // The alpha curve is a presentation projection of the world epoch. It is
    // updated only when that epoch, the path, the sampling range, or the mesh
    // vertex count changes. In particular, a paused/unchanged clock does not
    // rebuild the full color buffer.
    for (entity, path, view, children, state) in q_paths.iter() {
        if !trajectory_is_active(view) || path.points.len() < 2 {
            continue;
        }

        let mut updated = false;
        let mut updated_num_points = None;
        for child in children.iter() {
            if let Ok(mesh_handle) = q_marker.get(child) {
                if let Some(mut mesh) = meshes.get_mut(&mesh_handle.0) {
                    let num_points = mesh.attribute(Mesh::ATTRIBUTE_POSITION).unwrap().len();
                    if !alpha_update_is_needed(state, world.epoch_jd, path, view, num_points)
                        && mesh.attribute(Mesh::ATTRIBUTE_COLOR).is_some()
                    {
                        continue;
                    }

                    let start_epoch = if let Some(s) = view.start_epoch {
                        s
                    } else {
                        path.update_epoch - (view.sampling_days / 2.0)
                    };
                    let total_sampling_days =
                        if let (Some(start), Some(end)) = (view.start_epoch, view.end_epoch) {
                            end - start
                        } else {
                            view.sampling_days
                        };

                    let colors: Vec<[f32; 4]> = (0..num_points)
                        .map(|i| {
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

                            // RGB is 1.0 — the MATERIAL owns the tint, and a copy of
                            // it here would multiply in a second time. Alpha is the
                            // fade, and under `AlphaMode::Add` bevy premultiplies it
                            // for us (`vec4(rgb * a, 0.0)` in `premultiply_alpha`),
                            // so alpha DIMS the line rather than erasing what is
                            // behind it. See the seed site in the rebuild system.
                            [1.0, 1.0, 1.0, alpha]
                        })
                        .collect();
                    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
                    updated = true;
                    updated_num_points = Some(num_points);
                    trace!("Trajectory alpha updated for {} points", num_points);
                }
            }
        }

        if updated {
            commands.entity(entity).insert(TrajectoryAlphaState {
                epoch_jd: Some(world.epoch_jd),
                path_epoch: Some(path.update_epoch),
                sampling_days: view.sampling_days,
                start_epoch: view.start_epoch,
                end_epoch: view.end_epoch,
                num_points: updated_num_points.expect("updated trajectory has mesh points"),
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
            path_epoch: Some(path.update_epoch),
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
            2
        ));
        assert!(alpha_update_is_needed(
            Some(&state),
            2451545.6,
            &path,
            &view,
            2
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

        assert!(!trajectory_needs_update(&view, &path, 2451600.0, true));
        assert!(trajectory_needs_update(
            &view,
            &TrajectoryPath::default(),
            2451600.0,
            true
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
                *vis = if final_visible {
                    Visibility::Visible
                } else {
                    Visibility::Hidden
                };
            }
        }
    }
}

pub fn trajectory_alignment_system(
    mut commands: Commands,
    world: Res<WorldTime>,
    ephemeris: Option<Res<EphemerisResource>>,
    frame_index: Res<crate::ReferenceFrameIndex>,
    q_grids: Query<&big_space::prelude::Grid>,
    mut q_vistas: Query<
        (
            Entity,
            &TrajectoryView,
            &TrajectoryPath,
            &mut Transform,
            Option<&mut CellCoord>,
            Option<&ChildOf>,
        ),
        (Without<ReferenceFrame>,),
    >,
    q_view_children: Query<&Children>,
    q_traj_mesh: Query<(), With<TrajectoryMeshMarker>>,
) {
    let jd = world.epoch_jd;

    for (v_entity, view, path, mut transform, cell, current_parent) in q_vistas.iter_mut() {
        let mut target_parent = None;
        let mut parent_grid: Option<&big_space::prelude::Grid> = None;
        // For anchored views: the tracked body's CURRENT position in the SAME
        // reference frame `path.anchor` was sampled in, so the curve rides the
        // body continuously (cancels drift since the anchor epoch) — see the
        // placement write below.
        let mut tracked_translation: Option<bevy::math::DVec3> = None;

        if view.frame == TrajectoryFrame::BodyFixed {
            // Body-fixed points belong on the body's (spinning) reference-frame
            // GRID: the grid's rotation IS the body-fixed frame, and big_space only propagates
            // a cell-entity whose direct parent is a `Grid` — a cell-entity
            // under a plain body entity is silently left to the f32 compat
            // pass (doc 45 correction block, class 2; the "Artemis 2
            // Moon-Relative: parent has NO Grid" probe warning).
            if let Some(f_entity) = frame_index.resolve(ReferenceFrame::BodyFixed {
                body: view.reference_id,
            }) {
                target_parent = Some(f_entity);
                parent_grid = q_grids.get(f_entity).ok();
            }
        } else if path.anchor != bevy::math::DVec3::ZERO {
            // ANCHORED body-orbit view (points stored relative to the tracked
            // body at the rebuild epoch). Parent to the TRACKED body's frame;
            // the placement write below subtracts the body's CURRENT position
            // so the curve stays fixed in inertial space and the body slides
            // along it (continuous anchor — kills the "offset from its orbit
            // unless I scroll away" drift-then-snap; KSA v2025.11.9 fix).
            if let Some(f_entity) = frame_index.resolve(ReferenceFrame::EclipticJ2000 {
                center: view.tracked_id,
            }) {
                if let Ok(grid) = q_grids.get(f_entity) {
                    target_parent = Some(f_entity);
                    parent_grid = Some(grid);
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
                    tracked_translation = ephemeris.as_ref().and_then(|e| {
                        let p_target = e.provider.global_position(view.tracked_id, jd)?;
                        let p_ref = e.provider.global_position(view.reference_id, jd)?;
                        Some(crate::coords::ecliptic_to_bevy(p_target - p_ref).raw())
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
            // Re-stamp the mesh children's `LowPrecisionRoot` on the two
            // transitions that make this view a VALID cell-entity parent.
            // big_space's `tag_low_precision_roots` strips the marker while
            // the view is still unparented/cell-less (spawn-order window at
            // scene load), and its re-tag only fires on the CHILD's
            // Changed<ChildOf>/Added<Transform> — never again. Without the
            // marker NO pass owns the mesh's GlobalTransform (the compat
            // walk is severed at the Transform-less WorldRoot), so the
            // polyline renders stale — visible trajectory-line jitter.
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
        Or<(
            Changed<TrajectoryView>,
            Changed<TrajectoryPath>,
            Changed<ChildOf>,
        )>,
    >,
) -> bool {
    !changed.is_empty()
}
