//! TEMPORARY spawn diagnostics — track a freshly-spawned entity's fate for a
//! few seconds to root-cause "spawn rover on rugged terrain → it disappears".
//!
//! Confirmed NON-repro on the flat sandbox scene (always-present ground
//! collider): every rover type, at safe height / resting height / tilted /
//! deeply embedded, spawns and stays. So this probe targets the remaining
//! hypothesis — **collider readiness on streamed CDLOD terrain**: at spawn time
//! it casts a ray straight down through the spawn point and logs whether a
//! ground collider is actually there (and at what height), then each frame it
//! aggregates the rover's rigid-body descendants' lowest world-Y, max
//! displacement, NaN, and body count — catching fall-through, explosion, or
//! despawn as it happens.
//!
//! REMOVE once the vanish is diagnosed (grep `[spawnwatch]`).

use avian3d::prelude::{Position, RigidBody, SpatialQuery, SpatialQueryFilter};
use bevy::math::DVec3;
use bevy::prelude::*;

/// Marker on a freshly-spawned root the diagnostic follows. Self-removes after
/// [`WATCH_FRAMES`] (or immediately once the tracked bodies all vanish).
#[derive(Component)]
pub struct SpawnWatch {
    /// Catalog entry id, for readable logs.
    pub name: String,
    /// The requested world spawn position (the exact point the click/API asked
    /// for) — the ray-down origin and the displacement baseline.
    pub spawn_pos: Vec3,
    frames_left: u32,
    probed_ground: bool,
    /// Peak rigid-body count seen — a later drop to 0 means "despawned".
    max_bodies: usize,
    /// One-shot latch for the possession-readiness report.
    reported_possess: bool,
}

/// Frame (into the watch) at which to emit the possession-readiness report —
/// ~1.5 s in, by when the async USD load + FSW/co-sim wiring has settled.
const POSSESS_REPORT_FRAME: u32 = 90;

/// `Controllable` — the exact possession gate from `lunco-avatar`
/// (`With<FlightSoftware> Or With<SimComponent>`). Possession resolves a click to
/// the nearest `SelectableRoot` and possesses only if THAT root matches this.
type Controllable = bevy::prelude::Or<(
    bevy::prelude::With<lunco_fsw::FlightSoftware>,
    bevy::prelude::With<lunco_cosim::SimComponent>,
)>;

/// ~4 s at the 60 Hz fixed step — long enough to see a fall-through settle or an
/// explosion fling the parts off.
const WATCH_FRAMES: u32 = 240;

/// Tag a spawned root so [`watch_spawned`] reports its fate.
pub fn tag_spawn_watch(commands: &mut Commands, root: Entity, name: String, spawn_pos: Vec3) {
    commands.entity(root).insert(SpawnWatch {
        name,
        spawn_pos,
        frames_left: WATCH_FRAMES,
        probed_ground: false,
        max_bodies: 0,
        reported_possess: false,
    });
}

/// Per-frame diagnostic for every [`SpawnWatch`] root (see module docs).
pub fn watch_spawned(
    mut commands: Commands,
    mut watched: Query<(Entity, &mut SpawnWatch)>,
    children: Query<&Children>,
    bodies: Query<(&GlobalTransform, Option<&Position>), With<RigidBody>>,
    // Possession gate: which entities are `Controllable`, and whether the root
    // (what a click resolves to) is `SelectableRoot`.
    controllable: Query<(), Controllable>,
    selectable: Query<(), With<lunco_core::SelectableRoot>>,
    spatial: SpatialQuery,
) {
    for (root, mut w) in watched.iter_mut() {
        // One-shot: is a ground collider actually present under the spawn point?
        // This is the crux of the streamed-terrain hypothesis — on flat ground
        // it always hits; a MISS (or a hit far below) means the rover was placed
        // over a not-yet-baked terrain tile and will free-fall.
        if !w.probed_ground {
            w.probed_ground = true;
            let origin = w.spawn_pos.as_dvec3() + DVec3::Y * 100.0;
            match spatial.cast_ray(origin, Dir3::NEG_Y, 400.0, true, &SpatialQueryFilter::default()) {
                Some(h) => {
                    let ground_y = origin.y - h.distance;
                    info!(
                        "[spawnwatch] {} spawn@({:.2},{:.2},{:.2}) — ground collider at y={:.3} ({:.2} m below spawn)",
                        w.name, w.spawn_pos.x, w.spawn_pos.y, w.spawn_pos.z,
                        ground_y, w.spawn_pos.y as f64 - ground_y
                    );
                }
                None => warn!(
                    "[spawnwatch] {} spawn@({:.2},{:.2},{:.2}) — NO ground collider under spawn point at spawn time (rover will free-fall)",
                    w.name, w.spawn_pos.x, w.spawn_pos.y, w.spawn_pos.z
                ),
            }
        }

        // Aggregate the rigid-body descendants (the chassis/wheels; the root
        // itself is just a transform anchor). Prefer avian's f64 `Position`
        // (authoritative world pose); fall back to `GlobalTransform`.
        let mut min_y = f32::INFINITY;
        let mut max_dist = 0.0f32;
        let mut n = 0usize;
        let mut nan = false;
        let mut stack = vec![root];
        while let Some(e) = stack.pop() {
            if let Ok(ch) = children.get(e) {
                stack.extend(ch.iter());
            }
            if let Ok((gt, pos)) = bodies.get(e) {
                let p = match pos {
                    Some(p) => p.0.as_vec3(),
                    None => gt.translation(),
                };
                if !p.is_finite() {
                    nan = true;
                }
                min_y = min_y.min(p.y);
                max_dist = max_dist.max((p - w.spawn_pos).length());
                n += 1;
            }
        }
        w.max_bodies = w.max_bodies.max(n);

        let f = WATCH_FRAMES - w.frames_left;

        // One-shot possession-readiness report once wiring has settled. This is
        // the exact gate `avatar_raycast_possession` applies: a click resolves to
        // the nearest `SelectableRoot`, then possesses only if THAT root is
        // `Controllable`. So the rover is possessable iff the spawn root is BOTH
        // `SelectableRoot` AND `Controllable`. If the FSW lands on a descendant
        // instead of the root, the click follows (not possesses) — the reported
        // "not possessable" symptom.
        if !w.reported_possess && f >= POSSESS_REPORT_FRAME {
            w.reported_possess = true;
            let root_selectable = selectable.get(root).is_ok();
            let root_controllable = controllable.get(root).is_ok();
            let mut ctrl_total = 0usize;
            let mut ctrl_on_descendant = false;
            let mut stack = vec![root];
            while let Some(e) = stack.pop() {
                if let Ok(ch) = children.get(e) {
                    stack.extend(ch.iter());
                }
                if controllable.get(e).is_ok() {
                    ctrl_total += 1;
                    if e != root {
                        ctrl_on_descendant = true;
                    }
                }
            }
            let possessable = root_selectable && root_controllable;
            info!(
                "[spawnwatch] {} POSSESS: possessable={} root_selectable={} root_controllable={} controllable_in_subtree={} fsw_on_descendant_only={}",
                w.name, possessable, root_selectable, root_controllable, ctrl_total,
                ctrl_on_descendant && !root_controllable
            );
        }

        let vanished = w.max_bodies > 0 && n == 0;
        let fell = min_y.is_finite() && min_y < w.spawn_pos.y - 5.0;
        let exploded = max_dist > 50.0 || nan;
        // Log a heartbeat every 15 frames, plus always on the first/last frame
        // and the instant a failure signature trips.
        if f % 15 == 0 || w.frames_left <= 1 || vanished || fell || exploded {
            let flag = if vanished {
                " <VANISHED: all bodies despawned>"
            } else if exploded {
                " <EXPLODED / NaN>"
            } else if fell {
                " <FELL THROUGH>"
            } else {
                ""
            };
            info!(
                "[spawnwatch] {} f={} bodies={}/{} min_y={:.2} max_dist={:.2} nan={}{}",
                w.name, f, n, w.max_bodies, min_y, max_dist, nan, flag
            );
        }

        if w.frames_left <= 1 || vanished {
            if let Ok(mut e) = commands.get_entity(root) {
                e.remove::<SpawnWatch>();
            }
        } else {
            w.frames_left -= 1;
        }
    }
}
