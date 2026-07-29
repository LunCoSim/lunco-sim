//! USD-authored **screen-constant markers** — geometry that subtends a fixed
//! angle regardless of range, so a thing that is physically correct but
//! sub-pixel still reads as a shape on screen.
//!
//! ```usda
//! def Sphere "Marker" (
//!     prepend apiSchemas = ["LunCoMarkerAPI"]
//! )
//! {
//!     double radius = 1                          # UNIT geometry — the scale is ours
//!     float lunco:marker:angularSizeDeg = 0.4    # how big it reads, always
//!     float lunco:marker:showBeyondM = 200000    # nothing at close range
//! }
//! ```
//!
//! ## Why this exists rather than a bigger model
//!
//! A 70 m dish on a true-scale Earth is 1/91,000 of the body: correct, and
//! invisible from any camera that can see the whole globe. The `structures/
//! ground_stations/README.md` states the case and names the missing capability —
//! "a screen-constant-size marker pin … does not exist in the engine today" —
//! and notes it is general (waypoints, POIs and orbiting vessels want the same
//! thing), which is why it lands here as a `lunco:marker:*` vocabulary any prim
//! can author and not as a ground-station feature.
//!
//! Inflating the dish model instead would be a lie about a measured quantity:
//! the aperture is what `1.16 · DishHead.scale` says it is, and the link budget
//! reads it.
//!
//! ## The two knobs, and why the second one is not optional
//!
//! `angularSizeDeg` is the whole point: apparent size is held constant, so the
//! marker is a dot from lunar distance and shrinks to nothing as you approach.
//!
//! `showBeyondM` is what keeps that honest. Angular-constant scaling never stops
//! — at 50 m a 0.4° marker is a 0.35 m ball sitting on the pedestal, a prop
//! nobody asked for parked in the middle of a facility that is already fully
//! modelled. Below the threshold the real geometry is doing the job, so the
//! marker hides.

use bevy::prelude::*;
use big_space::prelude::{CellCoord, Grid};
use lunco_render::camera::SceneCamera;

/// Marks a USD-authored mission waypoint so presentation systems can keep its
/// visual and route geometry on the same composed terrain surface.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct WaypointMarker;

/// A prim that asked to be drawn at a constant apparent size.
///
/// The geometry it is authored on must be UNIT-sized (radius/half-extent 1),
/// following the engine's "unit prim + scale" convention: this component owns
/// `Transform.scale` outright and overwrites it every frame it is on screen.
#[derive(Component, Clone, Copy, Debug)]
pub struct ScreenConstantMarker {
    /// Apparent diameter, degrees.
    pub angular_deg: f32,
    /// Camera distance (m) below which the marker hides and the real geometry
    /// takes over.
    pub show_beyond_m: f32,
}

impl Default for ScreenConstantMarker {
    fn default() -> Self {
        Self {
            // SIZE THIS AGAINST THE TARGET'S OWN DISC, not against the screen.
            // Earth seen from the Moon is 1.9° wide, so anything approaching half
            // a degree is a QUARTER OF THE PLANET — a blob, not a site marker.
            // 0.15° is ~8% of that disc: a dot you can point at, on a globe that
            // still reads as a globe. (In pixels: ~6 on a 1900-wide 50° view.)
            angular_deg: 0.15,
            show_beyond_m: 200_000.0,
        }
    }
}

/// Scale (= radius, on unit geometry) that makes `angular_deg` of apparent
/// DIAMETER at `distance`. Shared by the system and its tests so the size
/// contract is asserted against the arithmetic that actually runs.
fn marker_scale(angular_deg: f32, distance: f64) -> f64 {
    distance * ((angular_deg as f64).to_radians() * 0.5).tan()
}

/// Hold every [`ScreenConstantMarker`] at its authored apparent size.
///
/// Distance comes from [`lunco_core::coords::world_position`] on both ends, not
/// from `GlobalTransform`: these markers sit on OTHER bodies' grids (a ground
/// station is parented to Earth's rotating grid, 384,000 km from the scene the
/// camera is standing in), and an f32 world position at that range has already
/// lost the metres this scale is computed from.
/// The queries are split on `Without<ScreenConstantMarker>` and resolved through
/// `world_position_seeded`: this system holds `&mut Transform` on the markers, so a
/// second query that could also match them for READ is a B0001 conflict panic. The
/// seeded walk takes each subject's own cell+transform by hand and only uses the
/// (disjoint) query for its ANCESTORS — which are never markers.
#[allow(clippy::type_complexity)]
pub fn scale_screen_constant_markers(
    q_camera: Query<
        (Entity, &Camera, Option<&CellCoord>, &Transform),
        (With<SceneCamera>, Without<ScreenConstantMarker>),
    >,
    mut q_markers: Query<(
        Entity,
        &ScreenConstantMarker,
        Option<&CellCoord>,
        &mut Transform,
        &mut Visibility,
    )>,
    q_parents: Query<&ChildOf>,
    q_grids: Query<&Grid>,
    q_spatial: Query<(Option<&CellCoord>, &Transform), Without<ScreenConstantMarker>>,
) {
    if q_markers.is_empty() {
        return;
    }
    // The ACTIVE scene camera, for the same reason `update_globe_lod` filters on
    // one: an offscreen preview camera picked by iteration order would resize
    // every marker in the world to suit a view nobody is looking through.
    let Some((cam_entity, _, cam_cell, cam_tf)) = q_camera.iter().find(|(_, c, _, _)| c.is_active)
    else {
        return;
    };
    let cam = lunco_core::coords::world_position_seeded(
        cam_entity,
        cam_cell.unwrap_or(&CellCoord::default()),
        cam_tf,
        &q_parents,
        &q_grids,
        &q_spatial,
    );

    for (entity, marker, cell, mut tf, mut vis) in q_markers.iter_mut() {
        let pos = lunco_core::coords::world_position_seeded(
            entity,
            cell.unwrap_or(&CellCoord::default()),
            &tf,
            &q_parents,
            &q_grids,
            &q_spatial,
        );
        let distance = (pos.0 - cam.0).length();
        if distance < marker.show_beyond_m as f64 {
            if *vis != Visibility::Hidden {
                *vis = Visibility::Hidden;
            }
            continue;
        }
        if *vis != Visibility::Inherited {
            *vis = Visibility::Inherited;
        }
        // Unit geometry has RADIUS 1, so the scale that subtends `angular_deg`
        // of DIAMETER is the half-angle tangent times the range.
        let scale = marker_scale(marker.angular_deg, distance) as f32;
        if (tf.scale.x - scale).abs() > f32::EPSILON.max(scale * 1e-4) {
            tf.scale = Vec3::splat(scale);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LUNAR_DISTANCE_M: f64 = 384_400_000.0;
    const EARTH_RADIUS_M: f64 = 6_371_000.0;

    /// The size contract: doubling the range doubles the scale, so the apparent
    /// size is unchanged. This is the whole feature in one assertion.
    #[test]
    fn scale_is_linear_in_range() {
        let near = marker_scale(0.15, 1.0e6);
        let far = marker_scale(0.15, 2.0e6);
        assert!((far / near - 2.0).abs() < 1e-9);
    }

    /// THE SIZING TRAP, pinned. The marker's angle is not measured against the
    /// screen, it is measured against the thing it marks: Earth from the Moon is
    /// only 1.9° wide, so a half-degree marker is a quarter of the planet. The
    /// first cut of this feature shipped 0.45° for exactly that reason — it looks
    /// modest as a number and is a 3000 km blob in the frame.
    ///
    /// Assert the marker stays a small fraction of the disc it sits on, at the
    /// range this simulator exists to study.
    #[test]
    fn marker_is_a_dot_on_the_earth_disc_not_a_blob() {
        let m = ScreenConstantMarker::default();
        let radius = marker_scale(m.angular_deg, LUNAR_DISTANCE_M);
        let fraction = radius / EARTH_RADIUS_M;
        assert!(
            (0.02..0.15).contains(&fraction),
            "marker radius {radius:.0} m is {:.1}% of Earth's — a site marker must \
             read as a dot on the disc, not cover it",
            fraction * 100.0
        );
    }

    /// Below the threshold the real geometry owns the frame. Guards the other
    /// end of the same trap: angular scaling never stops on its own, so without
    /// this a station would wear a ball at walking distance.
    #[test]
    fn close_range_is_the_models_job() {
        let m = ScreenConstantMarker::default();
        assert!(
            m.show_beyond_m > 1000.0,
            "a surface view must show no marker"
        );
    }
}
