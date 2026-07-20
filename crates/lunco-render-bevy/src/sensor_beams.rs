//! The `range_beam` program driver — the render half of `lunco_cosim::RangeSensor`.
//!
//! `lunco-cosim` is a render-free simulation crate: it casts the ray and stores the
//! result (`distance`, `hit`). It never names a mesh or a material — doing so pulled
//! `bevy_render → wgpu + naga` into every build, including the `--no-ui` server and
//! the wasm worker.
//!
//! **The beam is not built here.** It is authored — a unit `Cylinder` with a bound
//! `Material`, a child of the sensor in `assets/vessels/sensors/altimeter.usda`. Its
//! colour, its width and its very existence are the author's, editable without a
//! compiler. All this driver does is stretch it to the distance the sensor reported.
//!
//! That split is the point:
//!
//! * the raycast is simulation → `lunco-cosim`, headless;
//! * the geometry and the look are authored → USD;
//! * the mapping from a live value to a transform is logic → here, in Rust.
//!
//! This replaced a `Gizmos` line. A gizmo has no depth, a fixed screen-space width
//! and a colour hardcoded in Rust: it drew over the terrain it measured and could not
//! be authored at all.
//!
//! See `docs/architecture/50-usd-driven-visuals.md` and `render-decoupling.md`.

use bevy::prelude::*;
use lunco_core::programs::{ProgramDriverAppExt, ProgramDriverId};
use lunco_cosim::sensors::RangeSensor;
use lunco_render::PbrLook;

/// The `lunco:program:id` the beam driver answers to.
const DRIVER_ID: &str = "range_beam";

/// The `lunco:program:id` the landing-point marker answers to.
const HIT_DRIVER_ID: &str = "range_hit";

/// Fallback marker radius, if the program prim authors no `lunco:param:radius`.
const DEFAULT_HIT_RADIUS: f64 = 0.06;

/// Fallback beam half-width, if the program prim authors no `lunco:param:width`.
/// A beam is a RAY: it should read as a line, not a pipe — thinner than the waypoint
/// route ribbon, which is a path you drive along rather than a measurement.
const DEFAULT_HALF_WIDTH: f64 = 0.03;

/// Fallback alphas, if the program prim authors no `lunco:param:hitAlpha` /
/// `lunco:param:missAlpha`. `hit` = the sensor reports real geometry; `miss` = it is
/// reporting its out-of-range fallback.
///
/// These only BLEND if the bound material resolves to `SurfaceAlpha::Blend` — which
/// means its `inputs:opacity` must be sub-1. An opaque material ignores alpha
/// entirely, so writing these would be a silent no-op.
const DEFAULT_HIT_ALPHA: f64 = 0.85;
const DEFAULT_MISS_ALPHA: f64 = 0.35;

pub(crate) fn build(app: &mut App) {
    // No `Assets<Mesh>`, no `GizmoConfigStore`, no render resource of any kind: this
    // writes a `Transform` and nothing else. The gizmo version had to be gated on the
    // gizmo store because a `Gizmos` param PANICS without `GizmoPlugin` — every
    // `MinimalPlugins` test and every headless app that links this crate. There is
    // nothing left here to gate.
    //
    // `Update`, and no `BigSpaceSystems` ordering. The gizmo anchored to the sensor's
    // `GlobalTransform`, which is a full frame stale in `Update` — a visible "moves
    // then snaps back" at speed, which is why that version ran in `PostUpdate` after
    // `PropagateHighPrecision`. The beam is a CHILD of the sensor with a purely local
    // transform, so big_space propagates it along with the lander for free. There is
    // no global transform to read and therefore no stale-frame hazard to order around.
    app.register_program_driver(DRIVER_ID, drive_range_beam);
    app.register_program_driver(HIT_DRIVER_ID, drive_range_hit);
}

/// Put the landing marker where the ray actually hit, and hide it when it did not.
///
/// Its own driver rather than a branch inside `drive_range_beam`: they are bound to
/// different prims, and a prim's driver is selected by the id IT authors. One system
/// reaching across to move a sibling would put the beam's program in charge of geometry
/// it does not own — and make the marker impossible to delete without editing Rust.
fn drive_range_hit(
    mut q_hits: Query<(
        &ProgramDriverId,
        &ChildOf,
        &mut Transform,
        &mut Visibility,
        Option<&lunco_core::ScriptParams>,
    )>,
    q_sensors: Query<&RangeSensor>,
) {
    for (id, parent, mut tf, mut vis, params) in q_hits.iter_mut() {
        if id.0 != HIT_DRIVER_ID {
            continue;
        }
        let Ok(s) = q_sensors.get(parent.parent()) else { continue };

        // HIDDEN on a miss. The sensor is reporting its out-of-range fallback, so there
        // is no landing point — parking the marker at the range limit would draw a
        // contact that never happened, which is worse than drawing nothing.
        //
        // `Visibility`, not a zero scale. Scripts scale to zero because rhai cannot set
        // an enum (`apply_dynamic` has no enum arm); a Rust driver has no such excuse,
        // and a zero-scaled mesh still costs a draw call and still answers a raycast.
        //
        // Guarded: `DerefMut` marks it `Changed` and re-propagates the visibility tree.
        let want = if s.hit { Visibility::Inherited } else { Visibility::Hidden };
        if *vis != want {
            *vis = want;
        }
        if !s.hit {
            continue;
        }

        let radius = params
            .and_then(|p| p.0.get("radius").copied())
            .unwrap_or(DEFAULT_HIT_RADIUS);
        // `distance`, NOT `max_distance` — the point the sensor actually reported.
        let axis = s.axis.normalize_or_zero();
        *tf = Transform {
            translation: (s.offset + axis * s.distance).as_vec3(),
            rotation: Quat::IDENTITY,
            scale: Vec3::splat(radius as f32),
        };
    }
}

/// Stretch an authored beam to the distance its sensor reported.
///
/// The beam prim is a child of the sensor, so this walks up one link to find the
/// `RangeSensor` — a beam belongs to the instrument that reports the range, so
/// the owner is where the number is.
fn drive_range_beam(
    mut q_beams: Query<(
        &ProgramDriverId,
        &ChildOf,
        &mut Transform,
        Option<&lunco_core::ScriptParams>,
        Option<&mut PbrLook>,
    )>,
    q_sensors: Query<&RangeSensor>,
) {
    for (id, parent, mut tf, params, look) in q_beams.iter_mut() {
        if id.0 != DRIVER_ID {
            continue;
        }
        let Ok(s) = q_sensors.get(parent.parent()) else { continue };
        let param = |key: &str, fallback: f64| {
            params
                .and_then(|p| p.0.get(key).copied())
                .unwrap_or(fallback)
        };

        // The stored `distance` when the cast hit, else the full range — so the beam
        // shows what the sensor actually REPORTED, not a fresh cast that could
        // disagree with the value the simulation is using.
        let len = if s.hit { s.distance } else { s.max_distance };
        let half_width = param("width", DEFAULT_HALF_WIDTH);

        // The authored prim is a UNIT cylinder (`radius = 1`, `height = 1`), because
        // `radius`/`height` are baked into the mesh at instantiation and never re-read
        // — scaling is the only live channel.
        let axis = s.axis.normalize_or_zero();
        *tf = Transform {
            translation: (s.offset + axis * len * 0.5).as_vec3(),
            rotation: Quat::from_rotation_arc(Vec3::Y, axis.as_vec3()),
            scale: Vec3::new(half_width as f32, len as f32, half_width as f32),
        };

        // Fade while the sensor reports its fallback rather than a real hit. Only the
        // ALPHA, and only a choice between two AUTHORED values — the colour is the
        // material's emissive, so retinting the beam is editing USD, not this file.
        //
        // Guarded, because `DerefMut` on a `PbrLook` marks it `Changed` and rebinds its
        // material. Alpha takes exactly two values, so the look cache holds two entries
        // for the session — which is why this needs no `unshared` (an unshared look is
        // for a value that varies continuously, and would otherwise mint one cached
        // material per frame, forever).
        if let Some(mut look) = look {
            let want = if s.hit {
                param("hitAlpha", DEFAULT_HIT_ALPHA)
            } else {
                param("missAlpha", DEFAULT_MISS_ALPHA)
            } as f32;
            if look.base_color.alpha != want {
                look.base_color.alpha = want;
            }
        }
    }
}
