//! Geodesy on spherical bodies (doc 43 §2.1-2.2).
//!
//! One canonical math frame — the **solar frame**: Bevy axes (Y-up), meters,
//! heliocentric; body centers come from
//! `ecliptic_to_bevy(EphemerisProvider::global_position(naif, jd))`.
//!
//! Body-fixed points use the SAME rotation the render grids use
//! ([`body_rotation`], shared with `body_rotation_system`), so math and
//! visuals cannot diverge: `DQuat::from_axis_angle(polar_axis,
//! days_since_j2000 · rate)`, angle 0 at J2000.
//!
//! Geodetic convention (spherical — sub-meter at km scale, no ellipsoid):
//! latitude about the body's `polar_axis` (north = +Y), longitude 0 on +X at
//! J2000, **east-positive** toward −Z (the direction
//! `DQuat::from_axis_angle(Y, +θ)` sweeps +X). ENU tangent basis matches the
//! terrain-georef scene convention: East=+X, North=−Z, Up=+Y in local scenes.

use bevy::math::{DQuat, DVec3};
use bevy::prelude::*;

use crate::registry::BodyDescriptor;

/// Spherical geodetic coordinates on a body (degrees, meters).
#[derive(Debug, Clone, Copy, PartialEq, Default, Reflect)]
pub struct Geodetic {
    /// Latitude in degrees, +north.
    pub lat_deg: f64,
    /// Longitude in degrees, +east.
    pub lon_deg: f64,
    /// Height above the body's mean sphere (meters).
    pub height_m: f64,
}

impl Geodetic {
    pub fn new(lat_deg: f64, lon_deg: f64, height_m: f64) -> Self {
        Self { lat_deg, lon_deg, height_m }
    }
}

/// Pins an entity to a geodetic point on a celestial body (ground stations,
/// site anchors). The entity's scene transform is ignored by comms math.
#[derive(Component, Debug, Clone, Copy, Reflect)]
#[reflect(Component)]
pub struct GeodeticAnchor {
    /// NAIF id of the body (399 Earth, 301 Moon).
    pub body: i32,
    pub geodetic: Geodetic,
}

/// Marks the scene-root [`GeodeticAnchor`] that defines the **site frame**:
/// the local scene origin sits at this geodetic point with ENU axes
/// (East=+X, North=−Z, Up=+Y). Inserted by the USD bridge on the root prim.
#[derive(Component, Debug, Clone, Copy, Default, Reflect)]
#[reflect(Component)]
pub struct SiteAnchor;

/// Rotation of `desc`'s body-fixed frame at `epoch_jd` — the exact formula
/// `body_rotation_system` applies to the render grids.
///
/// Composite: first orient the body-fixed +Y pole onto the body's actual
/// `polar_axis` (obliquity tilt — identity when the axis IS +Y, e.g. the
/// Moon's ecliptic-frame approximation), then spin about that axis. Body-fixed
/// coordinates ([`geodetic_to_body_fixed`]) always use +Y as the pole, so a
/// tilted axis MUST include this arc or surface points would spin about an
/// axis their latitude wasn't measured against (Earth ground stations 23°
/// off in the ecliptic world frame).
pub fn body_rotation(desc: &BodyDescriptor, epoch_jd: f64) -> DQuat {
    if desc.rotation_rate_rad_per_day == 0.0 {
        return DQuat::IDENTITY;
    }
    let days = epoch_jd - lunco_time::J2000_JD;
    let tilt = DQuat::from_rotation_arc(DVec3::Y, desc.polar_axis.normalize_or_zero());
    tilt * DQuat::from_axis_angle(DVec3::Y, days * desc.rotation_rate_rad_per_day)
}

/// Body-fixed cartesian position of a geodetic point (meters).
pub fn geodetic_to_body_fixed(geo: &Geodetic, radius_m: f64) -> DVec3 {
    let lat = geo.lat_deg.to_radians();
    let lon = geo.lon_deg.to_radians();
    let r = radius_m + geo.height_m;
    DVec3::new(
        r * lat.cos() * lon.cos(),
        r * lat.sin(),
        -r * lat.cos() * lon.sin(),
    )
}

/// Inverse of [`geodetic_to_body_fixed`] (exact on the sphere).
pub fn body_fixed_to_geodetic(p: DVec3, radius_m: f64) -> Geodetic {
    let r = p.length();
    let lat = (p.y / r.max(1e-9)).clamp(-1.0, 1.0).asin();
    let lon = (-p.z).atan2(p.x);
    Geodetic {
        lat_deg: lat.to_degrees(),
        lon_deg: lon.to_degrees(),
        height_m: r - radius_m,
    }
}

/// ENU tangent frame at a point, expressed in the **solar frame** (or any
/// frame the caller built it in). Scene axes map as East=+X, Up=+Y, North=−Z.
#[derive(Debug, Clone, Copy)]
pub struct LocalTangentFrame {
    pub origin: DVec3,
    pub east: DVec3,
    pub north: DVec3,
    pub up: DVec3,
}

impl LocalTangentFrame {
    /// Body-fixed ENU basis at a geodetic point (before body rotation).
    pub fn body_fixed(geo: &Geodetic, radius_m: f64) -> Self {
        let lon = geo.lon_deg.to_radians();
        let origin = geodetic_to_body_fixed(geo, radius_m);
        let up = origin.normalize_or_zero();
        let east = DVec3::new(-lon.sin(), 0.0, -lon.cos());
        let north = up.cross(east).normalize_or_zero();
        // Re-orthogonalize east (up is not exactly perpendicular to the
        // equatorial east direction off the equator).
        let east = north.cross(up).normalize_or_zero();
        Self { origin, east, north, up }
    }

    /// Rotate + translate the frame into another frame.
    pub fn transformed(&self, rotation: DQuat, translation: DVec3) -> Self {
        Self {
            origin: translation + rotation * self.origin,
            east: rotation * self.east,
            north: rotation * self.north,
            up: rotation * self.up,
        }
    }

    /// Local scene coordinates (East=+X, Up=+Y, North=−Z) → frame coords.
    pub fn to_frame(&self, local: DVec3) -> DVec3 {
        self.origin + self.east * local.x + self.up * local.y - self.north * local.z
    }

    /// Frame coords → local scene coordinates.
    pub fn from_frame(&self, p: DVec3) -> DVec3 {
        let d = p - self.origin;
        DVec3::new(d.dot(self.east), d.dot(self.up), -d.dot(self.north))
    }
}

/// Solar-frame position of a geodetic point on `desc` at `epoch_jd`, given the
/// body center's solar-frame position.
pub fn solar_position_of_geodetic(
    desc: &BodyDescriptor,
    geo: &Geodetic,
    body_center_solar: DVec3,
    epoch_jd: f64,
) -> DVec3 {
    body_center_solar + body_rotation(desc, epoch_jd) * geodetic_to_body_fixed(geo, desc.radius_m)
}

/// Solar-frame ENU tangent frame of a geodetic point on `desc` at `epoch_jd`.
pub fn solar_tangent_frame(
    desc: &BodyDescriptor,
    geo: &Geodetic,
    body_center_solar: DVec3,
    epoch_jd: f64,
) -> LocalTangentFrame {
    LocalTangentFrame::body_fixed(geo, desc.radius_m)
        .transformed(body_rotation(desc, epoch_jd), body_center_solar)
}

/// Segment–sphere occlusion: does the open interior of `p1→p2` dip inside the
/// sphere at `center` with radius `radius_m`? Endpoints on (or above) the
/// surface never occlude themselves: the closest-approach parameter clamps to
/// the segment ends, which sit at ≥ radius. A small margin absorbs float noise
/// for horizon-grazing links. Generic geometry — the `Occultation` query and
/// any authored sight-line test compose over it.
pub fn segment_hits_sphere(p1: DVec3, p2: DVec3, center: DVec3, radius_m: f64) -> bool {
    let d = p2 - p1;
    let len_sq = d.length_squared();
    if len_sq < 1.0 {
        return false;
    }
    let t = ((center - p1).dot(d) / len_sq).clamp(0.0, 1.0);
    if t <= 0.0 || t >= 1.0 {
        return false;
    }
    let closest = p1 + d * t;
    (closest - center).length() < radius_m - 0.5
}

#[cfg(test)]
mod tests {
    use super::*;

    fn moon() -> BodyDescriptor {
        crate::registry::CelestialBodyRegistry::default_system()
            .bodies
            .into_iter()
            .find(|b| b.ephemeris_id == 301)
            .unwrap()
    }

    #[test]
    fn geodetic_round_trip() {
        let r = 1737.0e3;
        for geo in [
            Geodetic::new(0.0, 0.0, 0.0),
            Geodetic::new(45.0, 90.0, 100.0),
            Geodetic::new(-89.45, -136.7, 1239.0),
            Geodetic::new(40.4314, -4.2481, 837.0),
        ] {
            let p = geodetic_to_body_fixed(&geo, r);
            let back = body_fixed_to_geodetic(p, r);
            assert!((back.lat_deg - geo.lat_deg).abs() < 1e-9, "{geo:?} → {back:?}");
            assert!(
                (back.lon_deg - geo.lon_deg).abs() < 1e-9 || geo.lat_deg.abs() > 89.999,
                "{geo:?} → {back:?}"
            );
            assert!((back.height_m - geo.height_m).abs() < 1e-6);
        }
    }

    #[test]
    fn enu_is_orthonormal_and_north_points_to_pole() {
        let f = LocalTangentFrame::body_fixed(&Geodetic::new(0.0, 0.0, 0.0), 1.0);
        assert!((f.up - DVec3::X).length() < 1e-12);
        assert!((f.north - DVec3::Y).length() < 1e-12, "north at equator = pole dir, got {:?}", f.north);
        assert!((f.east - DVec3::new(0.0, 0.0, -1.0)).length() < 1e-12);
        for geo in [Geodetic::new(30.0, 50.0, 0.0), Geodetic::new(-89.45, -136.7, 0.0)] {
            let f = LocalTangentFrame::body_fixed(&geo, 1737.0e3);
            assert!(f.east.dot(f.north).abs() < 1e-12);
            assert!(f.east.dot(f.up).abs() < 1e-12);
            assert!(f.north.dot(f.up).abs() < 1e-12);
            assert!((f.east.cross(f.north) - f.up * f.east.cross(f.north).dot(f.up)).length() < 1e-9);
        }
    }

    #[test]
    fn east_longitude_advances_with_body_rotation() {
        // A surface point carried by body rotation must advance in east
        // longitude: rotating the body by +θ about the pole moves the point
        // that WAS at lon λ to inertial direction λ+θ. Uses an untilted
        // (+Y) pole so `body_fixed_to_geodetic` (which measures about +Y)
        // reads the spin angle exactly — the real Moon's 1.5° Cassini tilt
        // would leak ~0.02° into the measured longitude.
        let mut desc = moon();
        desc.polar_axis = DVec3::Y;
        let geo = Geodetic::new(0.0, 10.0, 0.0);
        let jd0 = lunco_time::J2000_JD;
        let quarter_day = 0.25 * std::f64::consts::TAU / desc.rotation_rate_rad_per_day;
        let p0 = body_rotation(&desc, jd0) * geodetic_to_body_fixed(&geo, desc.radius_m);
        let p1 = body_rotation(&desc, jd0 + quarter_day) * geodetic_to_body_fixed(&geo, desc.radius_m);
        let inertial_lon0 = body_fixed_to_geodetic(p0, desc.radius_m).lon_deg;
        let inertial_lon1 = body_fixed_to_geodetic(p1, desc.radius_m).lon_deg;
        let delta = (inertial_lon1 - inertial_lon0).rem_euclid(360.0);
        assert!((delta - 90.0).abs() < 1e-6, "quarter turn should advance lon by 90°, got {delta}");
    }

    #[test]
    fn tangent_frame_round_trips_local_points() {
        let desc = moon();
        let center = DVec3::new(1.0e11, -2.0e10, 3.0e10);
        let f = solar_tangent_frame(&desc, &Geodetic::new(-89.45, -136.7, 1239.0), center, 2461000.5);
        let local = DVec3::new(12.0, 3.5, -40.0);
        let back = f.from_frame(f.to_frame(local));
        // f64 rounding at heliocentric magnitudes (~1e11 m) is ~2e-5 m.
        assert!((back - local).length() < 1e-3, "round-trip error {}", (back - local).length());
    }
}
