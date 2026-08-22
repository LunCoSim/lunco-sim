//! The `range_beam` program driver — the render half of a raw Avian ray query.
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
use lunco_cosim::avian_queries::RaycastObservation;
use lunco_render::PbrLook;

/// The `info:id` the beam driver answers to.
const DRIVER_ID: &str = "range_beam";

/// The `info:id` the landing-point marker answers to.
const HIT_DRIVER_ID: &str = "range_hit";

/// Read a required positive authored visual parameter. Missing and malformed
/// values are distinct from a meaningful zero: this driver never invents a
/// geometry scale when the USD program prim omitted one.
fn authored_positive_param(params: Option<&lunco_core::ScriptParams>, key: &str) -> Option<f64> {
    params
        .and_then(|p| p.0.get(key).copied())
        .filter(|value| value.is_finite() && *value > 0.0)
}

/// Read a required authored alpha. It is a unit interval by USD Preview
/// Surface semantics; clamping would hide a malformed scene opinion.
fn authored_alpha_param(params: Option<&lunco_core::ScriptParams>, key: &str) -> Option<f64> {
    params
        .and_then(|p| p.0.get(key).copied())
        .filter(|value| value.is_finite() && (0.0..=1.0).contains(value))
}

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
    q_sensors: Query<&RaycastObservation>,
) {
    for (id, parent, mut tf, mut vis, params) in q_hits.iter_mut() {
        if id.0 != HIT_DRIVER_ID {
            continue;
        }
        let Ok(s) = q_sensors.get(parent.parent()) else {
            continue;
        };

        // HIDDEN on a miss. The sensor is reporting its out-of-range fallback, so there
        // is no landing point — parking the marker at the range limit would draw a
        // contact that never happened, which is worse than drawing nothing.
        //
        // `Visibility`, not a zero scale. Scripts scale to zero because rhai cannot set
        // an enum (`apply_dynamic` has no enum arm); a Rust driver has no such excuse,
        // and a zero-scaled mesh still costs a draw call and still answers a raycast.
        //
        // Guarded: `DerefMut` marks it `Changed` and re-propagates the visibility tree.
        let want = if s.hit_valid {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        if *vis != want {
            *vis = want;
        }
        if !s.hit_valid {
            continue;
        }

        let Some(radius) = authored_positive_param(params, "radius") else {
            *vis = Visibility::Hidden;
            continue;
        };
        // `distance`, NOT `max_distance` — the point the sensor actually reported.
        let axis = s.axis.normalize_or_zero();
        let want = Transform {
            translation: (s.offset + axis * s.distance).as_vec3(),
            rotation: Quat::IDENTITY,
            scale: Vec3::splat(radius as f32),
        };
        // Guarded like the `Visibility` write above: `DerefMut` marks the
        // `Transform` `Changed` and re-runs propagation + the GlobalTransform
        // rewrite for this subtree even when the sensor reading has not moved.
        if *tf != want {
            *tf = want;
        }
    }
}

/// Stretch an authored beam to the distance its sensor reported.
///
/// The beam prim is a child of the sensor, so this walks up one link to find the
/// `RaycastObservation` — a beam belongs to the instrument that reports the raw hit, so
/// the owner is where the number is.
fn drive_range_beam(
    mut q_beams: Query<(
        &ProgramDriverId,
        &ChildOf,
        &mut Transform,
        Option<&lunco_core::ScriptParams>,
        Option<&mut PbrLook>,
        &mut Visibility,
    )>,
    q_sensors: Query<&RaycastObservation>,
) {
    for (id, parent, mut tf, params, look, mut vis) in q_beams.iter_mut() {
        if id.0 != DRIVER_ID {
            continue;
        }
        let Ok(s) = q_sensors.get(parent.parent()) else {
            continue;
        };
        // The stored `distance` when the cast hit, else the full range — so the beam
        // shows what the sensor actually REPORTED, not a fresh cast that could
        // disagree with the value the simulation is using.
        let len = if s.hit_valid {
            s.distance
        } else {
            s.max_distance
        };
        let Some(half_width) = authored_positive_param(params, "width") else {
            *vis = Visibility::Hidden;
            continue;
        };
        let Some(hit_alpha) = authored_alpha_param(params, "hitAlpha") else {
            *vis = Visibility::Hidden;
            continue;
        };
        let Some(miss_alpha) = authored_alpha_param(params, "missAlpha") else {
            *vis = Visibility::Hidden;
            continue;
        };

        // The authored prim is a UNIT cylinder (`radius = 1`, `height = 1`), because
        // `radius`/`height` are baked into the mesh at instantiation and never re-read
        // — scaling is the only live channel.
        let axis = s.axis.normalize_or_zero();
        let want = Transform {
            translation: (s.offset + axis * len * 0.5).as_vec3(),
            rotation: Quat::from_rotation_arc(Vec3::Y, axis.as_vec3()),
            scale: Vec3::new(half_width as f32, len as f32, half_width as f32),
        };
        // Guarded like the `PbrLook` alpha write below: an unconditional write
        // fires `Changed<Transform>` and re-propagates the subtree every frame
        // even while the sensor reports the same range.
        if *tf != want {
            *tf = want;
        }

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
            let want = if s.hit_valid { hit_alpha } else { miss_alpha } as f32;
            if look.base_color.alpha != want {
                look.base_color.alpha = want;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{authored_alpha_param, authored_positive_param};
    use lunco_core::ScriptParams;
    use std::collections::HashMap;

    fn params(values: [(&str, f64); 3]) -> ScriptParams {
        ScriptParams(
            values
                .into_iter()
                .map(|(key, value)| (key.to_string(), value))
                .collect::<HashMap<_, _>>(),
        )
    }

    #[test]
    fn visual_parameters_are_required_and_not_clamped() {
        let valid = params([("radius", 0.06), ("hitAlpha", 0.85), ("missAlpha", 0.35)]);
        assert_eq!(authored_positive_param(Some(&valid), "radius"), Some(0.06));
        assert_eq!(authored_alpha_param(Some(&valid), "hitAlpha"), Some(0.85));
        assert_eq!(authored_alpha_param(Some(&valid), "missAlpha"), Some(0.35));

        let missing = ScriptParams(HashMap::new());
        assert_eq!(authored_positive_param(Some(&missing), "radius"), None);
        assert_eq!(authored_alpha_param(Some(&missing), "hitAlpha"), None);

        let invalid = params([("radius", -1.0), ("hitAlpha", 1.1), ("missAlpha", f64::NAN)]);
        assert_eq!(authored_positive_param(Some(&invalid), "radius"), None);
        assert_eq!(authored_alpha_param(Some(&invalid), "hitAlpha"), None);
        assert_eq!(authored_alpha_param(Some(&invalid), "missAlpha"), None);
    }
}
