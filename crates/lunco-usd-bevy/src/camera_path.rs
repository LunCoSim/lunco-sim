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
//! **The path is evaluated once per RENDER frame, on the render frame's own clock.**
//! ([`drive_camera_paths`] samples, [`apply_camera_paths`] writes; both `PostUpdate`,
//! chained.) This is deliberate and was NOT the original design — see below.
//!
//! **Why not the fixed cadence.** The curve used to be evaluated in `FixedPostUpdate`
//! and the render pose interpolated between the two bracketing fixed samples by
//! `Time<Fixed>::overstep_fraction()`. Two independent defects, both measured:
//!
//! 1. **The sample pair was not a fixed-step bracket.** The time a path is evaluated
//!    at comes from [`ResolvedDomains`], which `advance_and_resolve_domains` fills in
//!    `PreUpdate` — once per RENDER frame, not per fixed step (`lunco-time`,
//!    `build_domain_tree`). So on a frame running two fixed steps the driver ran twice
//!    against the *same* resolved `t` and produced `prev == target` (the camera froze
//!    for that frame); on a frame running none it did not run at all while the
//!    smoother kept interpolating a stale pair. Path motion was therefore quantised to
//!    render-frame boundaries in a way that depended on frame timing. The
//!    `.after(DomainResolveSet)` ordering on the `FixedPostUpdate` system could not
//!    help — that set lives in `PreUpdate`, so the constraint was silently inert.
//!
//! 2. **`overstep_fraction()` is wall-clock derived.** It is the residual of the fixed
//!    accumulator, fed from `Time<Virtual>`, which by default derives from
//!    `Time<Real>`. Two runs of the same scene reach a given recorded frame index with
//!    different residuals, so the captured camera transform differed between runs —
//!    offline recording was not reproducible. It only *appeared* reproducible because
//!    `lunco-workbench`'s recorder pins the frame delta with
//!    `TimeUpdateStrategy::ManualDuration`, which happens to make the residual
//!    constant. Nothing stated that coupling and nothing detected its violation.
//!
//! A camera path is an **analytic function of time** — there is no integrated state to
//! advance, so there is nothing a fixed cadence buys it. Sampling it directly at the
//! render frame's resolved `t` removes both defects at once and deletes the
//! prev/target bracket entirely: the pose is a pure function of the domain clock, so
//! it is reproducible by construction rather than by an undocumented invariant held up
//! by a resource the recorder happens to set.

use crate::{UsdPrimPath, UsdRead};
use bevy::math::cubic_splines::{
    CubicBezier, CubicCardinalSpline, CubicGenerator, CyclicCubicGenerator,
};
use bevy::math::DVec3;
use bevy::prelude::*;
use big_space::prelude::{CellCoord, Grid};
use lunco_core::{on_command, Command};
use lunco_time::{Clocks, Playback, ResolvedDomains, TimeBinding, TimeDomain, TransportMode};

/// Which standard basis the curve interpolates with (`uniform token basis`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CurveBasis {
    /// Passes THROUGH its points — what hand-placed control points want.
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
    /// `Tangent` (or `Target`, when the whole-path `lunco:path:lookAt` relation
    /// is authored) key at t=0, so the lookup below needs no special case.
    pub aim: Vec<AimKey>,
    /// The USD path behind every `Target` aim key, kept for the path's life.
    ///
    /// The aim target is bound BY PATH, continuously, not by entity once at
    /// resolve. Two measured failure modes force this:
    ///
    /// 1. **Not spawned yet.** The path must resolve the moment its camera
    ///    exists — the recording gate releases on the recorder's start EDGE, so
    ///    a path that waited for its aim target could miss the release and hold
    ///    at frame 0 forever. A target that spawns later (async reference) must
    ///    still bind. Until then the key holds its `Tangent` placeholder.
    /// 2. **Stale entity.** A prim path can be carried by more than one entity
    ///    (spatial twin + data-side twin), and the entity first matched can
    ///    lack a `Transform` or be despawned/replaced by a later load phase.
    ///    Either way `world_pose` fails every frame and the camera silently
    ///    holds one rotation for the whole episode — the recorded symptom was
    ///    58 s of starfield with the vehicle never in frame.
    ///
    /// [`bind_aim_targets`] walks these every frame and patches
    /// [`Self::aim`] whenever the live, `Transform`-carrying entity for the
    /// path differs from what the key currently holds.
    pub aim_sources: Vec<AimTargetSource>,
    /// Control points, in the curve prim's local space.
    pub points: Vec<Vec3>,
    pub basis: CurveBasis,
    /// `wrap = "periodic"` — the curve closes.
    pub periodic: bool,
}

/// This prim's `typeName` is not `BasisCurves`, so it can never become a
/// [`CameraPath`]. Inserted by [`resolve_camera_paths`] to retire the prim from
/// its scan.
///
/// Keyed on `typeName` and NOTHING ELSE, deliberately. `resolve_camera_paths`
/// re-examined every prim without a `CameraPath` on every frame — reading the
/// canonical stage, building an `SdfPath`, and querying `typeName` for each. On
/// the summer-space-school twin that measured **37.3 ms/s** (0.562 ms/frame over
/// 2659 frames), spent almost entirely on prims that were never candidates.
///
/// A prim's `typeName` is fixed for the entity's life: changing it re-authors the
/// prim, which respawns the entity, which arrives without this marker. The
/// *other* early-out — a `BasisCurves` with no `lunco:path:camera` rel — is
/// deliberately NOT marked, because that rel can be authored onto an existing
/// curve by a live edit and must still resolve. Curves are a handful of prims per
/// scene, so re-scanning them costs nothing.
#[derive(Component)]
pub struct NotACameraPath;

/// The authored USD path behind one `Target` aim key — see [`CameraPath::aim_sources`].
#[derive(Debug, Clone)]
pub struct AimTargetSource {
    /// Index into [`CameraPath::aim`] (stable: the track is sorted once, at resolve).
    pub key: usize,
    /// The target prim's USD path, looked up against spawned prims each frame.
    pub path: String,
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

/// Marks a camera whose pose is owned by a [`CameraPath`], and carries the pose
/// [`drive_camera_paths`] sampled this frame for [`apply_camera_paths`] to write.
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
    /// Whether the path currently owns the camera's rotation. False during an
    /// [`AimMode::Manual`] stretch, where the writer sets position only and
    /// leaves look direction to the user — writing a stale target would fight
    /// the mouse.
    pub aim_owned: bool,
    /// False until the first successful sample. [`drive_camera_paths`] can bail for
    /// reasons that are transient during load (clock not resolved, the curve's grid
    /// ancestry not yet spawned), and [`apply_camera_paths`] must not write a
    /// zero-initialised pose over the camera in the meantime.
    pub primed: bool,
}

/// Normalised position along the curve for a domain time `t` and a playback span.
///
/// Extracted so the mapping is testable without an `App`: it is the one piece of
/// [`drive_camera_paths`] that is pure arithmetic, and it is where an off-by-one in
/// the loop/clamp policy would hide. The playhead is already wrapped or clamped by
/// `step_playhead` per the domain's own loop policy — looping is the domain's
/// business, not ours — so this only has to guard a degenerate span.
fn path_u(t: f64, start: f64, end: f64) -> f32 {
    let span = (end - start).max(f64::EPSILON);
    (((t - start) / span) as f32).clamp(0.0, 1.0)
}

/// The frozen link between a camera path's clock and the clock it really hangs on.
///
/// Present with `TimeDomain::scale == 0` ⇒ the shot is held at its first frame:
/// whoever owns the readiness condition releases it via [`release_camera_path_gate`].
/// This crate deliberately does not know what "ready" means — terrain streaming is
/// the sandbox's business, and `lunco-usd-bevy` has no terrain dependency.
#[derive(Component)]
pub struct CameraPathGate {
    /// The clock the shot actually hangs on (wall or sim), gated through this domain.
    pub parent: Entity,
}

/// Start a held shot, `parent_t` being the gate parent's resolved time NOW.
///
/// The offset makes the gate's own time continuous across the release (it reads 0
/// while frozen, and 0 at the instant of release). Without it the gate would jump
/// from 0 to `parent_t`, the path would see that whole span as one delta, and the
/// shot would open several seconds in — the exact frames the hold existed to save.
///
/// Idempotent: releasing an already-running gate is a no-op.
pub fn release_camera_path_gate(domain: &mut TimeDomain, parent_t: f64) {
    if domain.scale != 0.0 {
        return;
    }
    domain.offset = -parent_t;
    domain.scale = 1.0;
}

/// What [`CameraPathTransport`] does to the addressed path.
///
/// `serde` as well as `Reflect`: `#[Command]` types cross the HTTP/MCP wire, so
/// every field type has to be (de)serializable — the variant names are the wire
/// form (`"Play"` / `"Pause"` / `"Rewind"`).
#[derive(
    Reflect,
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    lunco_core::serde::Serialize,
    lunco_core::serde::Deserialize,
)]
#[serde(crate = "lunco_core::serde")]
pub enum CameraPathAction {
    /// Roll the shot: release the engine hold (idempotent) **and** clear any user
    /// pause. The one explicit way to start a path outside a recording.
    #[default]
    Play,
    /// User pause. Leaves the gate alone — a paused shot that has not started yet
    /// stays held, and `Play` is still what starts it.
    Pause,
    /// Scrub the playhead back to the path's range start. Does not change
    /// play/pause, so rewinding a rolling shot restarts it and rewinding a paused
    /// one parks it at frame 0 ready to `Play`.
    Rewind,
}

/// **Transport verb for an authored camera path** — play / pause / rewind, addressed
/// by the path prim's USD path (full path or its leaf, like [`SetActiveCamera`]).
///
/// Exists because path release is otherwise owned entirely by the offline recorder
/// (`start_camera_paths_when_recording_starts` in `lunco-luncosim`), and in an
/// ordinary interactive session no recorder ever runs — so an authored path would
/// sit held at its first frame forever. This is the deliberate, *explicit* answer to
/// that: one verb the user (or a script, or the HTTP API) invokes. It is NOT a
/// second automatic release. Two things racing to start the same shot on their own
/// initiative is exactly the non-determinism the recorder-owned release was
/// introduced to kill; adding a fallback here would reintroduce it.
///
/// Typed [`Command`], so it is reachable everywhere with no per-language binding:
/// rhai `cmd("CameraPathTransport", #{ path: "/World/Shot01", action: "Play" })`,
/// the HTTP API, and MCP.
///
/// # Per-shot camera paths are now viable
///
/// The campaign is authored as ONE continuous 58 s curve spanning six shots. That
/// was forced by the *previous* design, where every gate released simultaneously on
/// a single global terrain-ready event — several short per-shot paths would all have
/// started at once, so only a curve that was already continuous could survive it.
///
/// That constraint is gone. Release is now per-path and demand-driven: the recorder
/// releases on its own start edge, and this command addresses ONE path by prim path.
/// A scene can therefore author a separate short `BasisCurves` path per shot and
/// drive each independently. Nothing in the campaign does that yet — noted so
/// whoever authors shots next knows they are no longer stuck with one long curve.
#[Command(default)]
pub struct CameraPathTransport {
    /// The path prim's USD path (e.g. `/World/Shots/Shot01`), or just its leaf
    /// (`Shot01`).
    pub path: String,
    /// Play, pause, or rewind.
    pub action: CameraPathAction,
}

/// Handler for [`CameraPathTransport`]. See that type for why this verb exists.
///
/// Reaching the clock takes two hops, because a path owns *two* domain entities and
/// they mean different things (see `resolve_camera_paths`):
/// `CameraPath::domain` is the PLAYBACK domain (carries the playhead), and its
/// `TimeDomain::parent` is the GATE domain (carries the engine hold, `scale == 0`).
/// Play has to touch both — the gate to start the clock, the playhead to clear a
/// user pause — which is why this cannot just be a `release_camera_path_gate` call.
#[on_command(CameraPathTransport)]
pub fn camera_path_transport(
    trigger: On<CameraPathTransport>,
    resolved: Res<ResolvedDomains>,
    q_paths: Query<(&UsdPrimPath, &CameraPath)>,
    q_gate: Query<&CameraPathGate>,
    // One query for both entities: the playback domain and the gate domain are both
    // `TimeDomain`s. Borrows are taken one at a time via `get_mut`, so this is fine
    // even though the two entities are fetched from the same query.
    mut q_domains: Query<(&mut TimeDomain, Option<&mut Playback>)>,
) {
    let want = cmd.path.trim();
    let hit = q_paths.iter().find(|(p, _)| {
        let s = p.path.as_str();
        s == want || s.rsplit('/').next() == Some(want)
    });
    let Some((_, path)) = hit else {
        warn!("[camera-path] CameraPathTransport: no camera path at '{want}'");
        return;
    };

    // Hop 1: the playback domain — the playhead and the user's play/pause bit.
    let Ok((playback_domain, playback)) = q_domains.get_mut(path.domain) else {
        warn!("[camera-path] CameraPathTransport: path '{want}' has no playback domain");
        return;
    };
    let gate_entity = playback_domain.parent;
    if let Some(mut pb) = playback {
        match cmd.action {
            CameraPathAction::Play => pb.mode = TransportMode::Playing,
            CameraPathAction::Pause => pb.mode = TransportMode::Paused,
            // `start` is the authored range start, which `resolve_camera_paths` sets
            // to 0.0 — go through the field rather than hardcoding it, so a path with
            // an authored non-zero range rewinds to its own beginning.
            CameraPathAction::Rewind => pb.head = pb.start,
        }
    }

    // Hop 2: the gate domain — the engine hold. Only `Play` touches it, and only to
    // release. Nothing here ever re-freezes a gate: re-arming would make the shot's
    // time origin depend on when it was re-armed, which is the reproducibility bug
    // the recorder-owned release fixed. `Rewind` moves the PLAYHEAD instead, which
    // is the deterministic way back to frame 0.
    if cmd.action == CameraPathAction::Play {
        let Some(gate_entity) = gate_entity else {
            return;
        };
        let Ok(parent) = q_gate.get(gate_entity).map(|g| g.parent) else {
            return;
        };
        let Some(parent_t) = resolved.get(parent) else {
            warn!("[camera-path] CameraPathTransport: '{want}' gate parent clock not resolved yet");
            return;
        };
        if let Ok((mut gate_domain, _)) = q_domains.get_mut(gate_entity) {
            // Idempotent — a no-op on an already-rolling shot.
            release_camera_path_gate(&mut gate_domain, parent_t);
        }
    }
    info!("[camera-path] {want}: {:?}", cmd.action);
}

/// Resolve `BasisCurves` prims carrying `lunco:path:camera` into [`CameraPath`]s,
/// spawning each path's driven clock. Retries next frame while the camera prim
/// has not spawned yet (async scene load).
///
/// Non-curve prims retire from the scan after one look — see [`NotACameraPath`].
pub fn resolve_camera_paths(
    canonical: NonSend<crate::CanonicalStages>,
    clocks: Option<Res<Clocks>>,
    q_new: Query<(Entity, &UsdPrimPath), (Without<CameraPath>, Without<NotACameraPath>)>,
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
            // Permanent verdict — retire this prim from the scan. See
            // `NotACameraPath` for why `typeName` is the only safe key.
            commands.entity(entity).try_insert(NotACameraPath);
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
        let times = match crate::read_curve_real_array(reader, &path, "lunco:path:aim:times") {
            Ok(Some(times)) => times,
            Ok(None) => Vec::new(),
            Err(()) => {
                error!(
                    "[camera-path] {} has authored aim times with an unsupported value type",
                    prim.path
                );
                continue;
            }
        };
        let modes = match crate::read_curve_token_array(reader, &path, "lunco:path:aim:modes") {
            Ok(Some(modes)) => modes,
            Ok(None) => Vec::new(),
            Err(()) => {
                error!(
                    "[camera-path] {} has authored aim modes with an unsupported value type",
                    prim.path
                );
                continue;
            }
        };
        if times.len() != modes.len()
            || times.iter().any(|time| !time.is_finite() || *time < 0.0)
            || modes
                .iter()
                .any(|mode| !["tangent", "target", "manual"].contains(&mode.as_str()))
        {
            error!(
                "[camera-path] {} has invalid or non-parallel aim times and modes",
                prim.path
            );
            continue;
        }
        // The RAW rel paths, not entity lookups. A "target" key is authored
        // against a PRIM PATH, and the entity for that path is a moving target:
        // it may not have spawned yet (async reference), may be shadowed by a
        // non-spatial twin carrying the same path, or may be replaced by a later
        // load phase. Every one of those, bound once here, produced a camera
        // silently holding one rotation for a whole take. So resolve authors
        // only the KEYS (with a tangent placeholder) plus the path each target
        // key came from; `bind_aim_targets` owns entity binding, every frame.
        let target_paths = reader.rel_targets(&path, "lunco:path:aim:targets");
        let target_count = modes
            .iter()
            .filter(|mode| mode.as_str() == "target")
            .count();
        if target_count != target_paths.len() {
            error!(
                "[camera-path] {} has {} target aim modes but {} aim target relationships",
                prim.path,
                target_count,
                target_paths.len()
            );
            continue;
        }

        // (key, target path) — folded into indices after the sort.
        let mut keys: Vec<(AimKey, Option<String>)> = Vec::new();
        let mut next_target = 0usize;
        for (i, t) in times.iter().enumerate() {
            let (mode, source) = match modes.get(i).map(String::as_str) {
                Some("target") => {
                    let tp = target_paths.get(next_target);
                    next_target += 1;
                    let Some(tp) = tp else {
                        unreachable!("target count was validated before key construction")
                    };
                    (AimMode::Tangent, Some(tp.to_string()))
                }
                Some("manual") => (AimMode::Manual, None),
                Some("tangent") => (AimMode::Tangent, None),
                _ => unreachable!("aim modes are validated by the USD schema"),
            };
            keys.push((AimKey { t: *t, mode }, source));
        }
        keys.sort_by(|a, b| a.0.t.total_cmp(&b.0.t));
        if keys.is_empty() {
            // No track: the whole-path rel, else tangent. One key, so `aim_at`
            // needs no empty case.
            let source = reader.rel_target(&path, "lunco:path:lookAt");
            keys.push((
                AimKey {
                    t: 0.0,
                    mode: AimMode::Tangent,
                },
                source,
            ));
        }
        let mut aim: Vec<AimKey> = Vec::with_capacity(keys.len());
        let mut aim_sources: Vec<AimTargetSource> = Vec::new();
        for (i, (key, source)) in keys.into_iter().enumerate() {
            aim.push(key);
            if let Some(p) = source {
                aim_sources.push(AimTargetSource { key: i, path: p });
            }
        }

        // `points3` accepts both `point3f[]` and `point3d[]` — a curve authored in
        // double precision is still a curve, and a strict `point3f[]` read reported
        // it as "no points", which is a misleading diagnostic for a type mismatch.
        let mut points: Vec<Vec3> = reader
            .points3(&path, "points")
            .into_iter()
            .map(Vec3::from)
            .collect();
        // `curveVertexCounts` partitions `points` into separate curves on one prim.
        // A camera rides ONE curve — the first — so slice it out rather than
        // interpolating across curve boundaries as if the batch were one polyline.
        let counts = match crate::read_curve_int_array(reader, &path, "curveVertexCounts") {
            Ok(Some(counts)) if !counts.is_empty() => counts,
            Ok(Some(_)) | Ok(None) | Err(()) => {
                error!(
                    "[camera-path] {} has no usable authored curveVertexCounts",
                    prim.path
                );
                continue;
            }
        };
        let Some(&first) = counts.first() else {
            continue;
        };
        let Ok(first) = usize::try_from(first) else {
            error!(
                "[camera-path] {} has a negative curveVertexCounts entry",
                prim.path
            );
            continue;
        };
        let total = counts.iter().try_fold(0usize, |total, count| {
            usize::try_from(*count)
                .ok()
                .and_then(|count| total.checked_add(count))
        });
        if first < 2 || first > points.len() || total != Some(points.len()) {
            error!(
                "[camera-path] {} has curveVertexCounts inconsistent with points",
                prim.path
            );
            continue;
        }
        if first < points.len() {
            if counts.len() > 1 {
                warn!(
                    "[camera-path] {} carries {} curves — driving the first ({first} pts)",
                    prim.path,
                    counts.len()
                );
            }
            points.truncate(first);
        }
        if points.is_empty() {
            warn!("[camera-path] {} has no `points`", prim.path);
            continue;
        }
        if points.len() < 2 {
            warn!("[camera-path] {} needs at least 2 points", prim.path);
            continue;
        }
        // Captured before `points` moves into the component below. The log used to
        // re-read the attribute with a strict `scalar::<Vec<[f32; 3]>>`, so a
        // `point3d[]` curve — which `points3` reads perfectly well — was reported as
        // "0 pts" while working correctly. A diagnostic that contradicts the thing it
        // is diagnosing is worse than no diagnostic.
        let n_points = points.len();

        let ty = match crate::read_curve_token(reader, &path, "type", "cubic", &["linear", "cubic"])
        {
            Ok(ty) => ty,
            Err(()) => continue,
        };
        let basis = if ty == "linear" {
            CurveBasis::Linear
        } else {
            match crate::read_curve_token(
                reader,
                &path,
                "basis",
                "bezier",
                &["bezier", "catmullRom"],
            ) {
                Ok(basis) if basis == "bezier" => CurveBasis::Bezier,
                Ok(_) => CurveBasis::CatmullRom,
                Err(()) => continue,
            }
        };
        let wrap = match crate::read_curve_token(
            reader,
            &path,
            "wrap",
            "nonperiodic",
            &["nonperiodic", "periodic", "pinned"],
        ) {
            Ok(wrap) => wrap,
            Err(()) => continue,
        };
        if wrap == "pinned" {
            error!(
                "[camera-path] {} uses unsupported BasisCurves wrap=pinned",
                prim.path
            );
            continue;
        }
        let periodic = wrap == "periodic";
        let duration = match reader.real(&path, "lunco:path:duration") {
            Some(value) if value.is_finite() && value > 0.0 => value,
            Some(value) => {
                error!(
                    "[camera-path] {} has invalid duration {value}; expected a finite positive value",
                    prim.path
                );
                continue;
            }
            None if reader.has_authored_attribute(&path, "lunco:path:duration") => {
                error!(
                    "[camera-path] {} has authored duration with an unsupported value type",
                    prim.path
                );
                continue;
            }
            None => 60.0,
        };
        // Pause is WHERE THE CLOCK HANGS, not a flag: "real" keeps the shot
        // running while the sim is paused, "sim" freezes with it (the default —
        // authored motion is part of the scene, doc 19 §11b).
        let clock = match crate::read_curve_token(
            reader,
            &path,
            "lunco:path:clock",
            "sim",
            &["sim", "real"],
        ) {
            Ok(clock) => clock,
            Err(()) => continue,
        };
        let on_wall = clock == "real";
        let parent = if on_wall {
            clocks.interaction
        } else {
            clocks.sim
        };

        // The shot hangs off its real clock through a GATE domain, frozen at birth
        // (`scale = 0`). A driven clock advances by its PARENT's delta, so a frozen
        // parent yields 0 and the playhead holds exactly — the mechanism
        // `resolve_one` already documents, reused rather than re-invented.
        //
        // Deliberately NOT `Playback::mode = Paused`: that bit is the user's
        // play/pause intent (the transport, the P key). Engine readiness and user
        // intent sharing one bit is what forced the `paused_by_us` bookkeeping that
        // this codebase deleted once already — an engine hold must never be
        // expressible as a user pause. Held here, the user can still pause, scrub
        // and resume a shot that has not started; the two compose instead of
        // fighting.
        let gate = commands
            .spawn((
                Name::new(format!("CameraPathGate:{}", prim.path)),
                TimeDomain::derived(Some(parent), 0.0, 0.0),
                CameraPathGate { parent },
            ))
            .id();

        let domain = commands
            .spawn((
                Name::new(format!("CameraPath:{}", prim.path)),
                TimeDomain::derived(Some(gate), 0.0, 1.0),
                Playback {
                    start: 0.0,
                    end: duration,
                    looping: periodic,
                    ..default()
                },
            ))
            .id();

        commands.entity(entity).try_insert(CameraPath {
            camera,
            domain,
            aim,
            aim_sources,
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
            .try_insert((
                crate::camera::UsdCameraPose::Path,
                CellCoord::default(),
                lunco_core::GridAnchor,
                ChildOf(grid),
                // Bind the camera to THIS path's clock, not the shared preview.
                TimeBinding { domain },
                CameraPathDriven {
                    target_world: DVec3::ZERO,
                    target_rot: Quat::IDENTITY,
                    aim_owned: true,
                    primed: false,
                },
                // Fence the interactive camera stack out. `MountedCamera` above is
                // the only follower THIS crate knows about; the avatar's camera
                // modes (free-flight, spring-arm, orbit) live a crate away, run in
                // the same `PostUpdate`, and write the same `Transform` — measured
                // as a whole take recorded at the spawn heading while the path
                // moved the eye. The marker is the cross-crate contract; the
                // avatar side honours it (guards + strip system).
                lunco_core::CinematicCameraLock,
            ));
        // `InteractionEased` is a second Transform owner: it interpolates
        // stepped avatar poses in PostUpdate. A path owns the complete pose and
        // must remove that history atomically when the lock is installed, or
        // its spawn-pose sample can overwrite the first path sample.
        commands
            .entity(camera)
            .remove::<lunco_time::InteractionEased>();
        info!(
            "[camera-path] {} → {:?} ({:?}, {} pts, {}s, {})",
            prim.path,
            camera,
            basis,
            n_points,
            duration,
            if on_wall { "wall clock" } else { "sim clock" }
        );
    }
}

/// Keep every `Target` aim key bound to the live, spatial entity for its prim path.
///
/// Runs every frame; paths with no target keys filter out immediately. For each
/// [`AimTargetSource`] the current best entity is the one that carries BOTH the
/// prim path and a `Transform` — `drive_camera_paths` aims via `world_pose`,
/// which starts from the entity's own `Transform`, so a data-side twin without
/// one is not a usable target no matter how well its path matches. When the best
/// entity differs from what the key holds (first spawn, respawn, or an earlier
/// bind to a non-spatial twin) the key is patched in place; when no usable
/// entity exists the key keeps its `Tangent` placeholder (fresh path) or its
/// stale `Target` (drive holds the last good rotation) until one appears.
pub fn bind_aim_targets(
    q_targets: Query<(Entity, &UsdPrimPath, Has<CellCoord>), With<Transform>>,
    mut q_paths: Query<(&UsdPrimPath, &mut CameraPath)>,
) {
    for (prim, mut path) in q_paths.iter_mut() {
        if path.aim_sources.is_empty() {
            continue;
        }
        // Bypass change detection until something actually rebinds: `iter_mut`
        // hands out `Mut`, and marking every path changed every frame would be
        // noise for any downstream `Changed<CameraPath>` reader.
        let path = path.bypass_change_detection();
        for i in 0..path.aim_sources.len() {
            let source = &path.aim_sources[i];
            // Best candidate for the path: prefer a grid-anchored entity
            // (`CellCoord`) over a bare-`Transform` one. A physics vehicle is
            // grid-direct; a bare-`Transform` match may be a data-side twin
            // whose pose never tracks the vehicle.
            let found = q_targets
                .iter()
                .filter(|(_, p, _)| p.path.as_str() == source.path.as_str())
                .max_by_key(|(_, _, has_cell)| *has_cell)
                .map(|(e, _, _)| e);
            let Some(e) = found else {
                if let AimMode::Target(stale) = path.aim[source.key].mode {
                    warn_once!(
                        "[camera-path] {}: aim target {} ({stale:?}) no longer has a \
                         Transform-carrying entity — holding stale bind",
                        prim.path,
                        source.path
                    );
                }
                continue;
            };
            let key = source.key;
            if path.aim[key].mode != AimMode::Target(e) {
                info!(
                    "[camera-path] {}: aim key {key} bound to {} ({e:?})",
                    prim.path, source.path
                );
                path.aim[key].mode = AimMode::Target(e);
            }
        }
    }
}

/// Evaluate each path at its clock's time and record the camera's pose for this
/// frame. [`apply_camera_paths`] writes it.
///
/// `PostUpdate`, once per render frame, ordered after `lunco_time::DomainResolveSet`
/// — the resolved time this reads is itself produced once per render frame, so
/// evaluating any more often than that only re-reads the same `t`. See the module doc
/// for the fixed-cadence design this replaced and why it was neither smooth nor
/// reproducible.
///
/// Split from the write purely to avoid a query conflict: sampling needs `&Transform`
/// over ALL prims (to compose the curve's and the aim target's world poses), while the
/// write needs `&mut Transform` on the cameras, and the camera is inside the former
/// set. Two chained systems in one schedule is cheaper than a `ParamSet` and keeps
/// each body readable.
pub fn drive_camera_paths(
    resolved: Res<ResolvedDomains>,
    q_paths: Query<(Entity, &CameraPath)>,
    q_playback: Query<&Playback>,
    q_parents: Query<&ChildOf>,
    q_grids: Query<&Grid>,
    q_spatial: Query<(Option<&CellCoord>, &Transform)>,
    mut q_cams: Query<&mut CameraPathDriven>,
    faults: Option<ResMut<lunco_core::RuntimeFaults>>,
    mut tick: Local<u32>,
) {
    let mut faults = faults;
    if faults
        .as_deref()
        .is_some_and(lunco_core::RuntimeFaults::active)
    {
        return;
    }
    *tick = tick.wrapping_add(1);
    let chatty = (*tick).is_multiple_of(100);
    for (curve_entity, path) in q_paths.iter() {
        let Ok(pb) = q_playback.get(path.domain) else {
            continue;
        };
        let Some(t) = resolved.get(path.domain) else {
            continue;
        };
        let u = path_u(t, pb.start, pb.end);

        // Control points are the CURVE prim's own geometry, so they are in its
        // local space. `world_pose` walks the grid hierarchy, giving the curve's
        // GRID-ABSOLUTE pose — so the sample lands in the same frame the camera's
        // `(cell, local)` is written in. Reading `GlobalTransform` here instead
        // would be the render frame: the classic bug.
        let Some((curve_pos, curve_rot)) =
            lunco_core::coords::world_pose(curve_entity, &q_parents, &q_grids, &q_spatial)
                .map(|(p, r)| (p.0, r.0))
        else {
            // Transient during load (grid ancestry not spawned). If it
            // PERSISTS the camera is never primed and never driven — a whole
            // take records from the camera's spawn pose, so say it once.
            warn_once!(
                "[camera-path] curve {curve_entity:?} has no resolvable world pose — \
                 camera not driven"
            );
            continue;
        };
        let at = |u: f32| -> DVec3 {
            let local = eval_curve(&path.points, path.basis, path.periodic, u);
            curve_pos + curve_rot * local.as_dvec3()
        };
        let world = at(u);
        if !world.is_finite() {
            if faults.as_deref_mut().is_some_and(|faults| {
                faults.raise(
                    "camera-path-nonfinite-eye",
                    Some(curve_entity),
                    "camera path",
                    format!("eye={world:?}, t={t}, u={u}"),
                )
            }) {
                error!("[camera-path] terminal runtime failure: non-finite eye pose at t={t:.3}");
            }
            continue;
        }

        // Aim, per the track in force at this instant. Direction is a DIFFERENCE
        // of two grid-absolute points, so it is small and safe in f32 — unlike the
        // positions themselves.
        let mut invalid_target = false;
        let look_dir = match path.aim_at(t) {
            AimMode::Target(e) => {
                match lunco_core::coords::world_pose(e, &q_parents, &q_grids, &q_spatial) {
                    Some((target, _)) => {
                        let dir = (target.0 - world).as_vec3();
                        if !target.0.is_finite() || !dir.is_finite() {
                            invalid_target = true;
                            if faults.as_deref_mut().is_some_and(|faults| {
                                faults.raise(
                                    "camera-path-nonfinite-target",
                                    Some(e),
                                    format!("target={e:?}"),
                                    format!("eye={world:?}, target={target:?}, look={dir:?}"),
                                )
                            }) {
                                error!(
                                    "[camera-path] terminal runtime failure: non-finite target pose for {e:?}"
                                );
                            }
                        }
                        info_once!(
                            "[camera-path] target aim live: eye {:?} -> target {:?}",
                            world,
                            target
                        );
                        Some(dir)
                    }
                    None => {
                        // Target despawned (or its Transform is gone) — hold the
                        // last rotation. Loud, because a whole take can record
                        // with the camera frozen on one heading if this
                        // persists; `bind_aim_targets` is responsible for
                        // rebinding a live entity.
                        warn_once!(
                            "[camera-path] aim target {e:?} has no resolvable pose — \
                             holding last rotation"
                        );
                        None
                    }
                }
            }
            AimMode::Tangent => Some((at((u + 1e-3).min(1.0)) - world).as_vec3()),
            // Hands off: position only, so free-look (or any other system) owns
            // the rotation for this stretch.
            AimMode::Manual => None,
        };

        if invalid_target {
            continue;
        }
        if look_dir.is_some_and(|dir| !dir.is_finite()) {
            if faults.as_deref_mut().is_some_and(|faults| {
                faults.raise(
                    "camera-path-nonfinite-look",
                    Some(curve_entity),
                    "camera path",
                    format!("eye={world:?}, look={look_dir:?}, t={t}, u={u}"),
                )
            }) {
                error!(
                    "[camera-path] terminal runtime failure: non-finite look direction at t={t:.3}"
                );
            }
            continue;
        }

        if chatty {
            info!(
                "[camera-path] drive t={t:.2} u={u:.3} eye={world:?} aim={:?} look={look_dir:?}",
                path.aim_at(t)
            );
        }
        if let Ok(mut driven) = q_cams.get_mut(path.camera) {
            // The pose is the curve's value at `t` — no blend with any previous
            // sample, so it carries no history and no dependence on when the frame
            // happened to land. That is what makes a recorded frame index reproduce
            // the identical transform across runs.
            driven.target_world = world;
            driven.aim_owned = !matches!(path.aim_at(t), AimMode::Manual);
            if let Some(dir) = look_dir {
                if dir.length_squared() > 1e-9 {
                    driven.target_rot = Transform::default().looking_to(dir, Vec3::Y).rotation;
                }
            }
            driven.primed = true;
        }
    }
}

/// Walk up a `ChildOf` chain to the enclosing `Grid`.
fn find_grid(
    from: Entity,
    q_parents: &Query<&ChildOf>,
    q_is_grid: &Query<(), With<Grid>>,
) -> Option<Entity> {
    let mut node = q_parents.get(from).ok()?.parent();
    for _ in 0..16 {
        if q_is_grid.contains(node) {
            return Some(node);
        }
        node = q_parents.get(node).ok()?.parent();
    }
    None
}

/// Write each path-driven camera's sampled pose into its `(CellCoord, Transform)`.
///
/// **No interpolation, and deliberately so.** This used to lerp between two fixed-step
/// samples by `Time<Fixed>::overstep_fraction()`. That fraction is a wall-clock
/// residual (`Time<Fixed>` ← `Time<Virtual>` ← `Time<Real>`), which made the recorded
/// transform at a given frame index differ run to run; and the pair it blended was
/// never actually a fixed-step bracket, because the sample time comes from
/// `ResolvedDomains`, which is filled once per render frame. See the module doc for
/// the full measurement.
///
/// `drive_camera_paths` now samples the curve at exactly this frame's time, so the
/// pose is already correct for this instant and there is nothing left to interpolate
/// toward. Smoothness comes from the curve being continuous in `t` and `t` advancing
/// every render frame — not from a filter.
pub fn apply_camera_paths(
    q_grids: Query<&Grid>,
    mut q: Query<(&mut CellCoord, &mut Transform, &ChildOf, &CameraPathDriven)>,
    faults: Option<Res<lunco_core::RuntimeFaults>>,
    mut tick: Local<u32>,
) {
    if faults.is_some_and(|faults| faults.active()) {
        return;
    }
    *tick = tick.wrapping_add(1);
    let chatty = (*tick).is_multiple_of(100);
    for (mut cell, mut tf, child_of, driven) in q.iter_mut() {
        if !driven.primed {
            continue;
        }
        let Ok(grid) = q_grids.get(child_of.parent()) else {
            if chatty {
                warn!("[camera-path] apply: camera parent is not a grid — write skipped");
            }
            continue;
        };
        if chatty {
            info!(
                "[camera-path] apply world={:?} owned={} rot={:?}",
                driven.target_world, driven.aim_owned, driven.target_rot
            );
        }
        // Re-bin the GRID-ABSOLUTE position into `(cell, local)` — the same write-back
        // `follow_mounted_cameras` does, and the reason the sample is kept absolute:
        // a local `Transform` alone is meaningless across a cell boundary.
        //
        // Note this reads NOTHING from the camera's own current pose. The sample is
        // the whole state, so there is no lag, no history, and no snap-vs-ease case —
        // there is no "current" to be far away from.
        let (new_cell, new_local) = grid.translation_to_grid(driven.target_world);
        *cell = new_cell;
        tf.translation = new_local;

        // Only when the path owns the aim — during a `Manual` stretch the user is
        // steering and writing a stale target would fight the mouse.
        if driven.aim_owned {
            tf.rotation = driven.target_rot;
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
            CurveBasis::Bezier => eval_bezier(points, periodic, u),
            CurveBasis::CatmullRom => eval_catmull_rom(points, periodic, u),
        },
    }
}

fn eval_linear(points: &[Vec3], periodic: bool, u: f32) -> Vec3 {
    let segs = if periodic {
        points.len()
    } else {
        points.len() - 1
    };
    let (i, f) = segment(segs, u);
    let a = points[i % points.len()];
    let b = points[(i + 1) % points.len()];
    a.lerp(b, f)
}

/// Catmull-Rom: interpolates its control points, so the curve goes THROUGH the
/// points you place. Periodic curves wrap. Non-periodic ones follow USD's end
/// conditions: the first and last CVs are TANGENT PHANTOMS, so the curve spans
/// p₁…pₙ₋₂ with `n − 3` segments (UsdGeomBasisCurves segment counting). Fewer
/// than 4 CVs cannot form a cubic segment — degrade to the polygon.
///
/// The numeric core is `bevy_math`'s [`CubicCardinalSpline`] at tension 0.5 —
/// the same generator `lunco-celestial/src/trajectories.rs` uses, and the same
/// basis matrix the old hand-rolled evaluator carried. Only the USD CV
/// bookkeeping lives here:
/// - cyclic: bevy's `to_curve_cyclic` segment `i` reads exactly the window
///   `(pᵢ₋₁, pᵢ, pᵢ₊₁, pᵢ₊₂) mod n` USD prescribes, so `t = u·n` is direct;
/// - open: bevy MIRRORS phantom endpoints (n − 1 segments over p₀…pₙ₋₁),
///   whereas USD says the authored ends ARE the phantoms, so sampling starts
///   one segment in — `t = 1 + u·(n − 3)` — and bevy's mirrored end segments
///   are never touched.
fn eval_catmull_rom(points: &[Vec3], periodic: bool, u: f32) -> Vec3 {
    let n = points.len();
    if !periodic && n < 4 {
        return eval_linear(points, false, u);
    }
    let u = u.clamp(0.0, 1.0);
    let spline = CubicCardinalSpline::new_catmull_rom(points.iter().copied());
    let sampled = if periodic {
        spline.to_curve_cyclic().map(|c| c.position(u * n as f32))
    } else {
        spline
            .to_curve()
            .map(|c| c.position(1.0 + u * (n - 3) as f32))
    };
    // `n ≥ 2` is guaranteed by the callers above, so this is unreachable — but
    // the polygon is a saner fallback than a panic in a render-path system.
    sampled.unwrap_or_else(|_| eval_linear(points, periodic, u))
}

/// Cubic Bezier: 4 CVs per segment, consecutive segments sharing an endpoint.
///
/// Segment counting follows UsdGeomBasisCurves: a `nonperiodic` cubic bezier
/// carries `4 + 3(segs − 1)` CVs, a `periodic` one exactly `3·segs`. The
/// periodic form authors no closing CV — the final segment borrows the first CV
/// back as its endpoint, which is what makes the loop close rather than stop.
/// Too few CVs to form even one cubic segment degrades to the control polygon.
///
/// Segment windows are gathered here (that indexing IS the USD wrapping rule);
/// the Bernstein evaluation itself is `bevy_math`'s [`CubicBezier`].
fn eval_bezier(points: &[Vec3], periodic: bool, u: f32) -> Vec3 {
    let n = points.len();
    let segs = if periodic { n / 3 } else { (n - 1) / 3 };
    if segs == 0 {
        return eval_linear(points, periodic, u);
    }
    // Wrapping the index is the whole of periodicity here: only the closing
    // segment's `b + 3` ever reaches `n`, and there it lands back on CV 0.
    let windows = (0..segs).map(|i| {
        let b = i * 3;
        [
            points[b % n],
            points[(b + 1) % n],
            points[(b + 2) % n],
            points[(b + 3) % n],
        ]
    });
    match CubicBezier::new(windows).to_curve() {
        Ok(curve) => curve.position(u.clamp(0.0, 1.0) * segs as f32),
        // Unreachable (`segs ≥ 1` above), but degrade rather than panic.
        Err(_) => eval_linear(points, periodic, u),
    }
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
            assert!(
                (got - *want).length() < 1e-5,
                "u={u} got {got:?} want {want:?}"
            );
        }
    }

    #[test]
    fn periodic_curve_closes_without_a_seam() {
        let p = ring();
        let start = eval_curve(&p, CurveBasis::CatmullRom, true, 0.0);
        let end = eval_curve(&p, CurveBasis::CatmullRom, true, 1.0);
        assert!(
            (start - end).length() < 1e-5,
            "loop must close: {start:?} vs {end:?}"
        );
    }

    /// USD end conditions for a nonperiodic catmullRom: the first and last CVs
    /// are tangent phantoms, so the curve spans p₁…pₙ₋₂. This pins the segment
    /// offset into the `bevy_math` curve — bevy's own `to_curve` mirrors extra
    /// phantoms and spans p₀…pₙ₋₁, so sampling without the `+1` offset would
    /// wrongly start the shot at the authored phantom p₀.
    #[test]
    fn open_catmull_rom_spans_the_interior_cvs_only() {
        let p = vec![
            Vec3::new(-1.0, 0.0, 0.0), // tangent phantom
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 1.0, 0.0),
            Vec3::new(2.0, 0.0, 0.0),
            Vec3::new(3.0, 0.0, 0.0), // tangent phantom
        ];
        let start = eval_curve(&p, CurveBasis::CatmullRom, false, 0.0);
        let end = eval_curve(&p, CurveBasis::CatmullRom, false, 1.0);
        assert!(
            (start - p[1]).length() < 1e-5,
            "u=0 is p1, not the phantom p0"
        );
        assert!(
            (end - p[3]).length() < 1e-5,
            "u=1 is pₙ₋₂, not the phantom pₙ₋₁"
        );
        // Interior knot: n − 3 = 2 segments, so u = 0.5 sits exactly on p2.
        let mid = eval_curve(&p, CurveBasis::CatmullRom, false, 0.5);
        assert!((mid - p[2]).length() < 1e-5, "interior knot interpolated");
    }

    /// Fewer than 4 CVs cannot form an open cubic segment — the documented
    /// degradation is the control polygon.
    #[test]
    fn open_catmull_rom_under_four_cvs_degrades_to_the_polygon() {
        let p = vec![
            Vec3::ZERO,
            Vec3::new(2.0, 0.0, 0.0),
            Vec3::new(2.0, 2.0, 0.0),
        ];
        let got = eval_curve(&p, CurveBasis::CatmullRom, false, 0.25);
        assert!(
            (got - Vec3::new(1.0, 0.0, 0.0)).length() < 1e-5,
            "3 CVs = polygon midpoint of the first edge: {got:?}"
        );
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
        assert!(
            r_cr > r_lin,
            "catmullRom must bulge past the chord: {r_cr} vs {r_lin}"
        );
        assert!(r_cr < 1.05, "…without overshooting the circle: {r_cr}");
    }

    #[test]
    fn path_u_is_a_pure_function_of_domain_time() {
        // The determinism property the fixed-cadence + `overstep_fraction()` design
        // did NOT have: the pose depends on the domain clock and nothing else, so a
        // given `t` maps to a given point on the curve regardless of frame timing,
        // frame rate, or how many fixed steps the frame happened to run.
        let p = ring();
        let sample = |t: f64| eval_curve(&p, CurveBasis::CatmullRom, true, path_u(t, 0.0, 8.0));
        assert_eq!(sample(3.0), sample(3.0), "same t ⇒ same pose, bit for bit");
        // …and distinct times genuinely move the camera (the multi-fixed-step defect
        // was `prev == target`, i.e. two evaluations that could not differ).
        assert!(
            (sample(3.0) - sample(3.1)).length() > 1e-4,
            "distinct times must produce distinct poses"
        );
    }

    #[test]
    fn path_u_clamps_and_survives_a_degenerate_span() {
        assert_eq!(
            path_u(-5.0, 0.0, 10.0),
            0.0,
            "before the span clamps to the start"
        );
        assert_eq!(
            path_u(50.0, 0.0, 10.0),
            1.0,
            "after the span clamps to the end"
        );
        assert!((path_u(5.0, 0.0, 10.0) - 0.5).abs() < 1e-6);
        // A zero-length `Playback` span must not divide by zero and produce NaN — a
        // NaN `u` propagates into the camera's Transform and the view goes black,
        // which is a spectacularly unhelpful symptom for "duration = 0".
        assert!(
            path_u(1.0, 4.0, 4.0).is_finite(),
            "degenerate span must stay finite"
        );
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

    /// `wrap = "periodic"` means the closing segment ends on CV 0, so `u = 1`
    /// lands exactly where `u = 0` did — a loop with no seam to jump across.
    /// 6 CVs = 2 periodic segments (`3·segs`), none of them a closing endpoint.
    #[test]
    fn periodic_bezier_closes_onto_its_first_cv() {
        let p = vec![
            Vec3::ZERO,
            Vec3::new(1.0, 1.0, 0.0),
            Vec3::new(2.0, 1.0, 0.0),
            Vec3::new(3.0, 0.0, 0.0),
            Vec3::new(2.0, -1.0, 0.0),
            Vec3::new(1.0, -1.0, 0.0),
        ];
        let start = eval_curve(&p, CurveBasis::Bezier, true, 0.0);
        let end = eval_curve(&p, CurveBasis::Bezier, true, 1.0);
        assert!((start - p[0]).length() < 1e-5, "periodic start is CV 0");
        assert!(
            (end - start).length() < 1e-5,
            "periodic end wraps back onto the start"
        );
        // The same CVs read nonperiodically span only one segment (4 + 3(segs−1)
        // ⇒ segs = 1) and stop on CV 3 — the wrap is what adds the return leg.
        let open_end = eval_curve(&p, CurveBasis::Bezier, false, 1.0);
        assert!(
            (open_end - p[3]).length() < 1e-5,
            "nonperiodic stops at its last endpoint"
        );
    }
}
