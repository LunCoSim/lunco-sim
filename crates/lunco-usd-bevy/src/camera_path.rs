//! Camera paths: a `UsdGeomBasisCurves` prim drives a camera along its curve.
//!
//! ```usda
//! def BasisCurves "CraterPath" {
//!     uniform token type = "cubic"
//!     uniform token basis = "catmullRom"      # passes THROUGH its points
//!     uniform token wrap = "periodic"         # closed loop, no seam case
//!     int[] curveVertexCounts = [12]
//!     point3f[] points = [(18, 1981, 70), ...]
//!
//!     rel lunco:path:camera = </MoonbaseScene/CraterOrbit>
//!     rel lunco:path:lookAt = </MoonbaseScene/AimTarget>   # optional
//!     double lunco:path:duration = 60
//!     token lunco:path:clock = "real"          # "real" | "sim" (default)
//! }
//! ```
//!
//! **Why `BasisCurves` and not `xformOp:translate.timeSamples` or a USD spline.**
//! USD's attribute splines (`Ts`) are **scalar-only** — the spec says a spline
//! "defines a scalar value… (double, float, or half)" — so a `double3` translate
//! can never be one. `Ts` is right for `focalLength`, never for position. A path
//! through space is a *curve*, and USD already has that primitive with the bases
//! we want. Using it means the path is portable USD, and it **renders itself**:
//! the trajectory is a real prim in the scene and in usdview, not a debug gizmo
//! that exists only in our viewport. `timeSamples` remains linear-between-keys —
//! it is what made the first orbit a visible 12-gon.
//!
//! **Time is a per-object driven domain** (doc 19: *"Replay this object = a driven
//! clock"*). Each path owns a `TimeDomain` + `Playback` entity, so paths replay,
//! loop and scrub independently of each other AND of the shared animation preview.
//! `lunco:path:clock = "real"` hangs it on the wall root so the shot plays while
//! the sim is paused — the same re-parent that runs the sky while paused. Pause is
//! never a flag here; it is *where the clock hangs*.
//!
//! **Cadence ≠ clock.** The curve is evaluated in `FixedPostUpdate`
//! ([`drive_camera_paths`]) — the same fixed cadence the follow camera solves on —
//! and the camera's `Transform` is eased toward that target at render rate
//! ([`smooth_camera_paths`]), so motion is smooth between fixed steps regardless of
//! frame rate.

use crate::{UsdPrimPath, UsdRead};
use bevy::math::DVec3;
use bevy::prelude::*;
use big_space::prelude::{CellCoord, Grid};
use lunco_time::{Clocks, Playback, ResolvedDomains, TimeBinding, TimeDomain};

/// Which standard basis the curve interpolates with (`uniform token basis`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CurveBasis {
    /// Passes THROUGH every point — what hand-placed control points want.
    CatmullRom,
    /// Cubic Bezier: 4 points per segment, endpoints shared (1 + 3n points).
    Bezier,
    /// `type = "linear"` — the polygon. Honest about what it is.
    Linear,
}

/// Where the camera looks during a stretch of the shot.
///
/// Aim is a **track over time**, not one setting for the whole path: a real shot
/// locks onto A, then swings to B, then hands control back. Modelling it as a
/// single `lookAt` rel cannot express that, and baking per-point rotations makes
/// every re-frame a twelve-point hand edit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AimMode {
    /// Lock onto a prim. Move the prim, the whole stretch re-aims — one drag
    /// instead of re-orienting every control point (Blender's Track-To).
    Target(Entity),
    /// Face along the direction of travel.
    Tangent,
    /// Hands off — the driver writes position only, leaving rotation alone, so
    /// the user (or any other system) owns the look direction.
    Manual,
}

/// One entry in the aim track: `mode` applies from `t` until the next entry.
/// Held, not interpolated — like `lunco:activeCamera` cuts (doc 35).
#[derive(Debug, Clone, Copy)]
pub struct AimKey {
    /// Start time, seconds on the path's own clock.
    pub t: f64,
    pub mode: AimMode,
}

/// A resolved camera path. Lives on the `BasisCurves` prim's entity.
#[derive(Component)]
pub struct CameraPath {
    /// The camera this curve drives.
    pub camera: Entity,
    /// This path's own driven clock (`TimeDomain` + `Playback`).
    pub domain: Entity,
    /// Aim track, sorted by time. Never empty — an unauthored track is a single
    /// `Tangent` (or `Target`, when the legacy whole-path `lunco:path:lookAt` rel
    /// is authored) key at t=0, so the lookup below needs no special case.
    pub aim: Vec<AimKey>,
    /// Control points, in the curve prim's local space.
    pub points: Vec<Vec3>,
    pub basis: CurveBasis,
    /// `wrap = "periodic"` — the curve closes.
    pub periodic: bool,
}

impl CameraPath {
    /// The aim mode in force at `t` — the last key at or before it (held).
    pub fn aim_at(&self, t: f64) -> AimMode {
        self.aim
            .iter()
            .rev()
            .find(|k| k.t <= t)
            .or_else(|| self.aim.first())
            .map(|k| k.mode)
            .unwrap_or(AimMode::Tangent)
    }
}

/// Marks a camera whose pose is owned by a [`CameraPath`], and carries the
/// fixed-step target that [`smooth_camera_paths`] eases toward.
///
/// Also the "hands off" flag for `camera_mount`: a path-driven camera authors no
/// `timeSamples`, so it is not `UsdAnimated` and the mount resolver would happily
/// claim it and pin it to a snapshot — the exact bug §8a describes, re-entering
/// through a different door.
///
/// **The target is GRID-ABSOLUTE (`DVec3`), not a parent-local `Vec3`**, and the
/// camera is rigged grid-direct exactly like a mounted camera. Writing a big
/// parent-local translation instead does not merely lose precision — it does not
/// converge: big_space re-bins the oversized local offset into `CellCoord` every
/// frame, the driver writes the same local value back, and the cell counter
/// climbs without bound (observed: `cell.y` 56 → 387 while `tf.y` stayed put, the
/// camera silently ascending to ~690 km). A grid-direct entity's position IS
/// `(cell, local)`; write both, or fight the engine and lose.
#[derive(Component)]
pub struct CameraPathDriven {
    /// Grid-absolute target position (double precision — a path can sit far from
    /// the floating origin, which is the whole reason for the grid).
    pub target_world: DVec3,
    pub target_rot: Quat,
    /// The PREVIOUS fixed sample. Render-rate motion interpolates `prev → target`
    /// across the fixed step; it does not chase `target` with an easing filter.
    ///
    /// An exponential ease (what this used to do) is a LAG filter, and it only
    /// looked smooth while the per-step delta was tiny. On frames with no fixed
    /// step the target is stale and the camera catches up; on frames with one it
    /// jumps and the camera falls behind — a speed oscillation that reads as the
    /// camera juddering back and forth. Invisible at 90 m / 60 s (~0.15 m per
    /// step), gross at 800 m / 15 s (~5.2 m per step). The curve is an analytic
    /// function of time, so there is nothing to guess at: bracket the render
    /// instant with the two fixed samples and interpolate, exactly as
    /// `bevy_transform_interpolation` does for physics.
    pub prev_world: DVec3,
    /// The previous fixed sample's rotation, interpolated the same way.
    pub prev_rot: Quat,
    /// Whether the path currently owns the camera's rotation. False during an
    /// [`AimMode::Manual`] stretch, where the smoother writes position only and
    /// leaves look direction to the user — writing a stale target would fight
    /// the mouse.
    pub aim_owned: bool,
    /// False until the first fixed evaluation, so the camera snaps to the path
    /// instead of easing in from wherever it spawned.
    pub primed: bool,
}

/// Resolve `BasisCurves` prims carrying `lunco:path:camera` into [`CameraPath`]s,
/// spawning each path's driven clock. Retries next frame while the camera prim
/// has not spawned yet (async scene load).
pub fn resolve_camera_paths(
    canonical: NonSend<crate::CanonicalStages>,
    clocks: Option<Res<Clocks>>,
    q_new: Query<(Entity, &UsdPrimPath), Without<CameraPath>>,
    q_prims: Query<(Entity, &UsdPrimPath)>,
    q_parents: Query<&ChildOf>,
    q_is_grid: Query<(), With<Grid>>,
    mut commands: Commands,
) {
    let Some(clocks) = clocks else { return };
    for (entity, prim) in q_new.iter() {
        let Some(cs) = canonical.get(prim.stage_handle.id()) else {
            continue;
        };
        let view = cs.view();
        let reader = &view;
        let Ok(path) = crate::SdfPath::new(prim.path.as_str()) else {
            continue;
        };
        if reader.type_name(&path).as_deref() != Some("BasisCurves") {
            continue;
        }
        let Some(cam_path) = reader.rel_target(&path, "lunco:path:camera") else {
            continue; // a plain curve, not a camera path
        };
        // The camera prim may not have spawned yet — retry next frame.
        let Some((camera, _)) = q_prims
            .iter()
            .find(|(_, p)| p.path.as_str() == cam_path.as_str())
        else {
            continue;
        };
        let by_path = |t: &crate::SdfPath| {
            q_prims
                .iter()
                .find(|(_, p)| p.path.as_str() == t.as_str())
                .map(|(e, _)| e)
        };

        // ── Aim track ────────────────────────────────────────────────────────
        // `lunco:path:aim:times` + `lunco:path:aim:modes` (+ `…:targets` rel, one
        // entry per "target" mode, in order). Held: each key rules until the next.
        //
        //   double[] lunco:path:aim:times = [0, 20, 40]
        //   token[]  lunco:path:aim:modes = ["target", "target", "manual"]
        //   rel      lunco:path:aim:targets = [</Hab>, </Lander>]
        //
        // Absent ⇒ fall back to the whole-path `lunco:path:lookAt` rel, else
        // tangent. So the simple case stays a one-liner and the track is opt-in.
        let times = reader
            .scalar::<Vec<f64>>(&path, "lunco:path:aim:times")
            .unwrap_or_default();
        let modes = reader
            .scalar::<Vec<String>>(&path, "lunco:path:aim:modes")
            .unwrap_or_default();
        let targets: Vec<Entity> = reader
            .rel_targets(&path, "lunco:path:aim:targets")
            .iter()
            .filter_map(by_path)
            .collect();

        let mut aim: Vec<AimKey> = Vec::new();
        let mut next_target = 0usize;
        for (i, t) in times.iter().enumerate() {
            let mode = match modes.get(i).map(String::as_str) {
                Some("target") => {
                    let e = targets.get(next_target).copied();
                    next_target += 1;
                    match e {
                        Some(e) => AimMode::Target(e),
                        None => {
                            warn!(
                                "[camera-path] {}: aim key {i} is \"target\" but \
                                 `lunco:path:aim:targets` has no {next_target}th entry — \
                                 falling back to tangent",
                                prim.path
                            );
                            AimMode::Tangent
                        }
                    }
                }
                Some("manual") => AimMode::Manual,
                _ => AimMode::Tangent,
            };
            aim.push(AimKey { t: *t, mode });
        }
        aim.sort_by(|a, b| a.t.total_cmp(&b.t));
        if aim.is_empty() {
            // No track: the whole-path rel, else tangent. One key, so `aim_at`
            // needs no empty case.
            let mode = reader
                .rel_target(&path, "lunco:path:lookAt")
                .and_then(|t| by_path(&t))
                .map(AimMode::Target)
                .unwrap_or(AimMode::Tangent);
            aim.push(AimKey { t: 0.0, mode });
        }

        let Some(points) = reader
            .scalar::<Vec<[f32; 3]>>(&path, "points")
            .map(|v| v.into_iter().map(Vec3::from).collect::<Vec<_>>())
        else {
            warn!("[camera-path] {} has no `points`", prim.path);
            continue;
        };
        if points.len() < 2 {
            warn!("[camera-path] {} needs at least 2 points", prim.path);
            continue;
        }

        let cubic = reader.text(&path, "type").as_deref() != Some("linear");
        let basis = match reader.text(&path, "basis").as_deref() {
            _ if !cubic => CurveBasis::Linear,
            Some("bezier") => CurveBasis::Bezier,
            // catmullRom is the USD default that passes through its points.
            _ => CurveBasis::CatmullRom,
        };
        let periodic = reader.text(&path, "wrap").as_deref() == Some("periodic");
        let duration = reader.scalar::<f64>(&path, "lunco:path:duration").unwrap_or(60.0);
        // Pause is WHERE THE CLOCK HANGS, not a flag: "real" keeps the shot
        // running while the sim is paused, "sim" freezes with it (the default —
        // authored motion is part of the scene, doc 19 §11b).
        let on_wall = reader.text(&path, "lunco:path:clock").as_deref() == Some("real");
        let parent = if on_wall { clocks.interaction } else { clocks.sim };

        let domain = commands
            .spawn((
                Name::new(format!("CameraPath:{}", prim.path)),
                TimeDomain::derived(Some(parent), 0.0, 1.0),
                Playback {
                    start: 0.0,
                    end: duration.max(f64::EPSILON),
                    looping: periodic,
                    ..default()
                },
            ))
            .id();

        commands.entity(entity).insert(CameraPath {
            camera,
            domain,
            aim,
            points,
            basis,
            periodic,
        });
        // Rig the camera GRID-DIRECT — the same rig `camera_mount` builds, and for
        // the same reason: big_space wants a camera's position expressed as
        // `(CellCoord, Transform)`, and `FloatingOrigin` may only sit on a
        // grid-direct entity.
        //
        // Do NOT instead leave it parented under the scene prim and write a big
        // parent-local translation. That does not converge: big_space re-bins the
        // oversized local offset into the cell each frame, the driver writes the
        // same local back, and the cell climbs without bound — observed as `cell.y`
        // 56 → 387 with the camera ascending to ~690 km, staring at empty space.
        //
        // Only `MountedCamera` is dropped: that is the *follower*, which would
        // fight us for the Transform. The rig itself is exactly what we need, so we
        // keep it and take over the writes. (`Without<CameraPathDriven>` on the
        // mount systems is the steady-state guard; this is the catch-up for the
        // race, since a path resolves frames after its camera spawns.)
        let Some(grid) = find_grid(camera, &q_parents, &q_is_grid) else {
            continue; // grid not spawned yet — retry next frame
        };
        commands
            .entity(camera)
            .remove::<crate::camera_mount::MountedCamera>()
            .insert((
                CellCoord::default(),
                lunco_core::GridAnchor,
                ChildOf(grid),
                // Bind the camera to THIS path's clock, not the shared preview.
                TimeBinding { domain },
                CameraPathDriven {
                    target_world: DVec3::ZERO,
                    target_rot: Quat::IDENTITY,
                    prev_world: DVec3::ZERO,
                    prev_rot: Quat::IDENTITY,
                    aim_owned: true,
                    primed: false,
                },
            ));
        info!(
            "[camera-path] {} → {:?} ({:?}, {} pts, {}s, {})",
            prim.path,
            camera,
            basis,
            // periodic curves wrap, so every point starts a segment
            reader
                .scalar::<Vec<[f32; 3]>>(&path, "points")
                .map(|p| p.len())
                .unwrap_or(0),
            duration,
            if on_wall { "wall clock" } else { "sim clock" }
        );
    }
}

/// Evaluate each path at its clock's time and write the camera's target pose.
///
/// `FixedPostUpdate`: the same fixed cadence the follow camera solves on, so the
/// motion is frame-rate independent and reproducible. Rendering interpolates
/// ([`smooth_camera_paths`]).
pub fn drive_camera_paths(
    resolved: Res<ResolvedDomains>,
    q_paths: Query<(Entity, &CameraPath)>,
    q_playback: Query<&Playback>,
    q_parents: Query<&ChildOf>,
    q_grids: Query<&Grid>,
    q_spatial: Query<(Option<&CellCoord>, &Transform)>,
    mut q_cams: Query<&mut CameraPathDriven>,
) {
    for (curve_entity, path) in q_paths.iter() {
        let Ok(pb) = q_playback.get(path.domain) else {
            continue;
        };
        let Some(t) = resolved.get(path.domain) else {
            continue;
        };
        let span = (pb.end - pb.start).max(f64::EPSILON);
        // Normalised position along the curve. The playhead is already wrapped or
        // clamped by `step_playhead` per the domain's own loop policy — looping is
        // the domain's business, not ours.
        let u = (((t - pb.start) / span) as f32).clamp(0.0, 1.0);

        // Control points are the CURVE prim's own geometry, so they are in its
        // local space. `world_pose` walks the grid hierarchy, giving the curve's
        // GRID-ABSOLUTE pose — so the sample lands in the same frame the camera's
        // `(cell, local)` is written in. Reading `GlobalTransform` here instead
        // would be the render frame: the classic bug.
        let Some((curve_pos, curve_rot)) =
            lunco_core::coords::world_pose(curve_entity, &q_parents, &q_grids, &q_spatial)
        else {
            continue;
        };
        let at = |u: f32| -> DVec3 {
            let local = eval_curve(&path.points, path.basis, path.periodic, u);
            curve_pos + curve_rot * local.as_dvec3()
        };
        let world = at(u);

        // Aim, per the track in force at this instant. Direction is a DIFFERENCE
        // of two grid-absolute points, so it is small and safe in f32 — unlike the
        // positions themselves.
        let look_dir = match path.aim_at(t) {
            AimMode::Target(e) => {
                match lunco_core::coords::world_pose(e, &q_parents, &q_grids, &q_spatial) {
                    Some((target, _)) => Some((target - world).as_vec3()),
                    None => None, // target despawned — hold the last rotation
                }
            }
            AimMode::Tangent => Some((at((u + 1e-3).min(1.0)) - world).as_vec3()),
            // Hands off: position only, so free-look (or any other system) owns
            // the rotation for this stretch.
            AimMode::Manual => None,
        };

        if let Ok(mut driven) = q_cams.get_mut(path.camera) {
            // Retire the old sample before taking the new one: these two bracket
            // the fixed step that `smooth_camera_paths` interpolates across. On
            // the first evaluation both ends are the new sample, so the camera
            // starts ON the path instead of sliding in from wherever it spawned.
            if driven.primed {
                driven.prev_world = driven.target_world;
                driven.prev_rot = driven.target_rot;
            } else {
                driven.prev_world = world;
            }
            driven.target_world = world;
            driven.aim_owned = !matches!(path.aim_at(t), AimMode::Manual);
            if let Some(dir) = look_dir {
                if dir.length_squared() > 1e-9 {
                    driven.target_rot = Transform::default().looking_to(dir, Vec3::Y).rotation;
                    if !driven.primed {
                        driven.prev_rot = driven.target_rot;
                    }
                }
            }
            driven.primed = true;
        }
    }
}

/// Walk up a `ChildOf` chain to the enclosing `Grid`.
fn find_grid(from: Entity, q_parents: &Query<&ChildOf>, q_is_grid: &Query<(), With<Grid>>) -> Option<Entity> {
    let mut node = q_parents.get(from).ok()?.parent();
    for _ in 0..16 {
        if q_is_grid.contains(node) {
            return Some(node);
        }
        node = q_parents.get(node).ok()?.parent();
    }
    None
}

/// Ease each path-driven camera toward its fixed-step target at render rate.
///
/// The fixed evaluation ticks at the physics cadence; without this the camera
/// would visibly step at that rate on a faster display. Mirrors how the follow
/// camera eases its Transform between `FixedPostUpdate` writes.
pub fn smooth_camera_paths(
    fixed: Res<Time<Fixed>>,
    q_grids: Query<&Grid>,
    mut q: Query<(&mut CellCoord, &mut Transform, &ChildOf, &CameraPathDriven)>,
) {
    // Where this render instant falls inside the current fixed step, 0..1. The
    // camera is placed BETWEEN the two samples that bracket it, so its speed is
    // whatever the curve says — it neither stalls on a frame without a fixed step
    // nor lurches on one with it.
    let s = fixed.overstep_fraction() as f64;

    for (mut cell, mut tf, child_of, driven) in q.iter_mut() {
        if !driven.primed {
            continue;
        }
        let Ok(grid) = q_grids.get(child_of.parent()) else {
            continue;
        };
        // Interpolate in GRID-ABSOLUTE space, then re-bin. Interpolating the local
        // `Transform` alone would be wrong across a cell boundary — the local value
        // jumps when the cell changes, so a lerp of locals would smear the camera
        // across a whole cell. Same write-back `follow_mounted_cameras` does.
        //
        // Note this reads NOTHING from the camera's own current pose: the sample
        // pair is the whole state. That is what makes it deterministic and
        // lag-free, and it is why there is no snap-vs-ease special case left —
        // there is no "current" to be far away from.
        let world = driven.prev_world.lerp(driven.target_world, s);
        let (new_cell, new_local) = grid.translation_to_grid(world);
        *cell = new_cell;
        tf.translation = new_local;

        // Only when the path owns the aim — during a `Manual` stretch the user is
        // steering and writing a stale target would fight the mouse.
        if driven.aim_owned {
            tf.rotation = driven.prev_rot.slerp(driven.target_rot, s as f32);
        }
    }
}

/// Evaluate the curve at normalised `u` ∈ [0, 1].
///
/// Uniform in the curve parameter, NOT arc length — so points spaced unevenly
/// make the camera speed up through sparse stretches. Fine for an even orbit;
/// a shot with clustered points wants arc-length reparameterisation (doc 50 §9.7).
pub fn eval_curve(points: &[Vec3], basis: CurveBasis, periodic: bool, u: f32) -> Vec3 {
    match points.len() {
        0 => Vec3::ZERO,
        1 => points[0],
        _ => match basis {
            CurveBasis::Linear => eval_linear(points, periodic, u),
            CurveBasis::Bezier => eval_bezier(points, u),
            CurveBasis::CatmullRom => eval_catmull_rom(points, periodic, u),
        },
    }
}

fn eval_linear(points: &[Vec3], periodic: bool, u: f32) -> Vec3 {
    let segs = if periodic { points.len() } else { points.len() - 1 };
    let (i, f) = segment(segs, u);
    let a = points[i % points.len()];
    let b = points[(i + 1) % points.len()];
    a.lerp(b, f)
}

/// Catmull-Rom: interpolates every control point, so the curve goes THROUGH the
/// points you place. Open curves duplicate the end points as phantom neighbours
/// (USD's own end-point handling), periodic ones wrap.
fn eval_catmull_rom(points: &[Vec3], periodic: bool, u: f32) -> Vec3 {
    let n = points.len();
    let segs = if periodic { n } else { n - 1 };
    let (i, f) = segment(segs, u);
    let idx = |k: isize| -> Vec3 {
        if periodic {
            points[(((i as isize + k) % n as isize + n as isize) % n as isize) as usize]
        } else {
            points[(i as isize + k).clamp(0, n as isize - 1) as usize]
        }
    };
    let (p0, p1, p2, p3) = (idx(-1), idx(0), idx(1), idx(2));
    let (t, t2, t3) = (f, f * f, f * f * f);
    // Standard uniform Catmull-Rom basis (tension 0.5).
    0.5 * ((2.0 * p1)
        + (-p0 + p2) * t
        + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t2
        + (-p0 + 3.0 * p1 - 3.0 * p2 + p3) * t3)
}

/// Cubic Bezier: 4 points per segment with shared endpoints (1 + 3n points).
fn eval_bezier(points: &[Vec3], u: f32) -> Vec3 {
    let n = points.len();
    let segs = ((n.saturating_sub(1)) / 3).max(1);
    let (i, f) = segment(segs, u);
    let b = i * 3;
    if b + 3 >= n {
        return points[n - 1];
    }
    let (p0, p1, p2, p3) = (points[b], points[b + 1], points[b + 2], points[b + 3]);
    let mt = 1.0 - f;
    p0 * (mt * mt * mt) + p1 * (3.0 * mt * mt * f) + p2 * (3.0 * mt * f * f) + p3 * (f * f * f)
}

/// Split `u` into (segment index, local fraction).
fn segment(segs: usize, u: f32) -> (usize, f32) {
    let segs = segs.max(1);
    let x = (u.clamp(0.0, 1.0)) * segs as f32;
    let i = (x.floor() as usize).min(segs - 1);
    (i, x - i as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ring() -> Vec<Vec3> {
        // Four points on a radius-1 circle in the XZ plane.
        vec![
            Vec3::new(0.0, 0.0, 1.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, -1.0),
            Vec3::new(-1.0, 0.0, 0.0),
        ]
    }

    #[test]
    fn catmull_rom_passes_through_every_control_point() {
        // The property that makes it right for hand-placed points: u at a knot
        // returns that knot exactly, so dragging a point moves the curve THROUGH
        // where you put it (a Bezier hull would only approach it).
        let p = ring();
        for (i, want) in p.iter().enumerate() {
            let u = i as f32 / p.len() as f32; // periodic: 4 segments
            let got = eval_curve(&p, CurveBasis::CatmullRom, true, u);
            assert!((got - *want).length() < 1e-5, "u={u} got {got:?} want {want:?}");
        }
    }

    #[test]
    fn periodic_curve_closes_without_a_seam() {
        let p = ring();
        let start = eval_curve(&p, CurveBasis::CatmullRom, true, 0.0);
        let end = eval_curve(&p, CurveBasis::CatmullRom, true, 1.0);
        assert!((start - end).length() < 1e-5, "loop must close: {start:?} vs {end:?}");
    }

    #[test]
    fn catmull_rom_is_smooth_where_linear_is_a_polygon() {
        // The whole point of the change. Midway between two control points, the
        // linear path cuts the chord (radius < 1) while Catmull-Rom bulges out
        // toward the true circle — i.e. it is not a 12-gon.
        let p = ring();
        let u = 0.125; // midpoint of the first periodic segment
        let lin = eval_curve(&p, CurveBasis::Linear, true, u);
        let cr = eval_curve(&p, CurveBasis::CatmullRom, true, u);
        let r_lin = (lin.x * lin.x + lin.z * lin.z).sqrt();
        let r_cr = (cr.x * cr.x + cr.z * cr.z).sqrt();
        assert!(r_lin < 0.72, "chord midpoint should cut inside: {r_lin}");
        assert!(r_cr > r_lin, "catmullRom must bulge past the chord: {r_cr} vs {r_lin}");
        assert!(r_cr < 1.05, "…without overshooting the circle: {r_cr}");
    }

    #[test]
    fn bezier_hits_its_segment_endpoints() {
        let p = vec![
            Vec3::ZERO,
            Vec3::new(0.0, 1.0, 0.0),
            Vec3::new(1.0, 1.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
        ];
        assert!((eval_curve(&p, CurveBasis::Bezier, false, 0.0) - p[0]).length() < 1e-5);
        assert!((eval_curve(&p, CurveBasis::Bezier, false, 1.0) - p[3]).length() < 1e-5);
    }
}
