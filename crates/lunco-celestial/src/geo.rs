//! Geodesy on spherical bodies (doc 43 §2.1-2.2).
//!
//! One canonical math frame — the **solar frame**: Bevy axes (Y-up), meters,
//! heliocentric; body centers come from
//! `ecliptic_to_bevy(EphemerisProvider::global_position(naif, jd))`.
//!
//! Body-fixed points use the SAME rotation the render grids use
//! ([`body_rotation`], shared with `body_rotation_system`), so math and
//! visuals cannot diverge. That rotation is the **IAU/WGCCRE** model
//! ([`crate::iau`]): pole from `α₀/δ₀`, prime meridian from `W = W₀ + Ẇ·d`,
//! both mapped out of the ICRF into this engine's ecliptic-Bevy frame.
//!
//! It used to be `from_axis_angle(polar_axis, days · rate)` — the right RATE
//! with **no phase at all** (`W₀` simply absent). That left the Moon rotated
//! 38.3° from its true orientation (~1160 km at the equator — the near side did
//! not face Earth) and Earth's ground stations ~190° of longitude off.
//!
//! Geodetic convention (spherical — sub-meter at km scale, no ellipsoid):
//! latitude about the body's pole (body-fixed +Y), longitude 0 on body-fixed
//! +X, **east-positive** toward −Z (the direction `from_axis_angle(Y, +θ)`
//! sweeps +X — the same sense a prograde `Ẇ` advances `W`, which is what keeps
//! these longitudes IAU-east). Body-fixed +X is the body's true prime meridian
//! at the epoch, NOT "engine +X at J2000" — that conflation was the bug.
//! ENU tangent basis matches the terrain-georef scene convention: East=+X,
//! North=−Z, Up=+Y in local scenes.

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

/// Rotation of `desc`'s body-fixed frame at `epoch_jd` — the exact rotation
/// `body_rotation_system` applies to the render grids.
///
/// The full IAU/WGCCRE model, built in [`crate::iau::IauRotation::rotation_bevy`]:
/// it carries the body-fixed pole (+Y) onto the body's true pole AND the
/// body-fixed prime meridian (+X) onto its true prime meridian `W(t) = W₀ + Ẇ·d`.
///
/// **The `W₀` phase is the whole point.** `W₀` is published east of the node of
/// the body's equator on the ICRF equator — NOT of this engine's +X — so it can
/// never be pasted in as a spin angle here; `iau.rs` does the frame transform.
/// Bodies with no IAU elements (Sun, EMB) do not rotate.
pub fn body_rotation(desc: &BodyDescriptor, epoch_jd: f64) -> DQuat {
    match &desc.iau {
        Some(iau) => iau.rotation_bevy(epoch_jd),
        None => DQuat::IDENTITY,
    }
}

/// Rotation from the body's **equatorial inertial** frame into the engine
/// frame at `epoch_jd`: it tilts the engine's +Y onto the body's pole, but does
/// NOT spin with the body.
///
/// This is the frame Keplerian elements are referenced to (`kepler.rs`):
/// inclination is measured from the body's equator and RAAN about the body's
/// pole, exactly as `geo` measures latitude — so `i = 90°` really does fly over
/// the geographic poles.
///
/// The minimal arc from +Y to the pole rotates about `Y × pole`. For Earth that
/// axis is ±X, so the arc **fixes +X = the vernal equinox** — which lies in
/// Earth's equator. RAAN is therefore measured from the equinox, and is directly
/// comparable to a TLE's. (Before this existed, orbits were built about the
/// ECLIPTIC pole and `placement` cancelled the body rotation on top, so an
/// ISS-like i = 51.6° orbit had a ground-track latitude wrong by up to ±23.4°.)
pub fn equatorial_frame(desc: &BodyDescriptor, epoch_jd: f64) -> DQuat {
    DQuat::from_rotation_arc(DVec3::Y, desc.polar_axis(epoch_jd))
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

/// Segment–box occlusion: does `p1→p2` intersect the oriented box at `center`
/// with orientation `rot` and `half_extents` (in the box's own frame)? The
/// standard slab test, run in box-local space.
///
/// Generic geometry, exactly like [`segment_hits_sphere`] — this is the primitive
/// behind "a wall / habitat / lander blocks the sight-line". It deliberately knows
/// nothing about radio, sensors, or physics colliders: callers supply the box.
///
/// All endpoints and extents are f64 in one frame. Both endpoints INSIDE the box
/// counts as a hit (a node buried in geometry has no line out), which the slab
/// test gives for free.
///
/// # Why hand-rolled
///
/// Not for want of a library, and **not** for f64: `avian3d` re-exports
/// `parry3d_f64`, so `parry::shape::Cuboid::cast_local_ray` is already reachable in
/// double precision from this crate's dep tree, and would handle the
/// endpoints-inside case too.
///
/// It is for COUPLING. This module is pure, render-free, wasm-safe geometry that the
/// `Occultation` query and any authored sight-line test compose over; parry's API is
/// nalgebra `Isometry`/`Point`, which `lunco-celestial` otherwise never names. Taking
/// it would mean a `DVec3`↔nalgebra conversion at every call and a geometry primitive
/// that leans on the physics stack to answer a question physics is not being asked —
/// to save ~25 lines of slab test that the tests below pin exactly.
///
/// Revisit if this module ever needs a second shape: two hand-rolled primitives is
/// where the trade flips.
pub fn segment_hits_obb(
    p1: DVec3,
    p2: DVec3,
    center: DVec3,
    rot: DQuat,
    half_extents: DVec3,
) -> bool {
    let he = half_extents.abs();
    if he.min_element() <= 0.0 {
        return false; // a degenerate box occludes nothing
    }
    // Into box-local space: the box becomes an AABB centred on the origin.
    let inv = rot.conjugate();
    let a = inv * (p1 - center);
    let b = inv * (p2 - center);
    let d = b - a;

    // Slab test over the segment parameter t ∈ [0, 1].
    let (mut t_min, mut t_max) = (0.0_f64, 1.0_f64);
    for axis in 0..3 {
        let (o, dir, h) = (a[axis], d[axis], he[axis]);
        if dir.abs() < 1e-12 {
            // Parallel to this slab — miss unless already between its planes.
            if o < -h || o > h {
                return false;
            }
            continue;
        }
        let (mut lo, mut hi) = ((-h - o) / dir, (h - o) / dir);
        if lo > hi {
            std::mem::swap(&mut lo, &mut hi);
        }
        t_min = t_min.max(lo);
        t_max = t_max.min(hi);
        if t_min > t_max {
            return false;
        }
    }
    true
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

    // ── segment_hits_obb ─────────────────────────────────────────────────────

    /// A 1×1×1 box (half-extents 0.5) at the origin, unrotated.
    fn unit_box(p1: DVec3, p2: DVec3) -> bool {
        segment_hits_obb(p1, p2, DVec3::ZERO, DQuat::IDENTITY, DVec3::splat(0.5))
    }

    #[test]
    fn obb_hit_and_miss() {
        assert!(unit_box(DVec3::new(-5.0, 0.0, 0.0), DVec3::new(5.0, 0.0, 0.0)), "straight through");
        assert!(!unit_box(DVec3::new(-5.0, 9.0, 0.0), DVec3::new(5.0, 9.0, 0.0)), "passes well above");
        assert!(unit_box(DVec3::ZERO, DVec3::new(5.0, 0.0, 0.0)), "starts inside");
        assert!(unit_box(DVec3::splat(-0.1), DVec3::splat(0.1)), "entirely inside");
    }

    /// The segment is FINITE: geometry behind an endpoint does not occlude. A ray
    /// test here would sever every link with anything anywhere along the bearing.
    #[test]
    fn obb_ignores_geometry_beyond_the_segment() {
        // The box sits at the origin; the segment lives entirely at x ≥ 5.
        assert!(
            !unit_box(DVec3::new(5.0, 0.0, 0.0), DVec3::new(10.0, 0.0, 0.0)),
            "a box behind both endpoints must not occlude"
        );
        // …and the same segment reversed.
        assert!(!unit_box(DVec3::new(10.0, 0.0, 0.0), DVec3::new(5.0, 0.0, 0.0)));
    }

    /// Rotation is applied — the box is oriented, not an AABB. A long thin slab
    /// yawed 90° swaps which axis it spans.
    #[test]
    fn obb_applies_rotation() {
        let he = DVec3::new(0.25, 1.0, 4.0); // thin in X, long in Z
        let seg = (DVec3::new(0.0, 0.0, 3.0), DVec3::new(6.0, 0.0, 3.0)); // along X at z = 3

        // Unrotated: the slab spans z ∈ [-4, 4] and x ∈ [-0.25, 0.25] → the
        // segment starts at x = 0 (inside the slab) → hit.
        assert!(segment_hits_obb(seg.0, seg.1, DVec3::ZERO, DQuat::IDENTITY, he));

        // Yawed 90°: X and Z extents swap, so the slab now spans x ∈ [-4, 4],
        // z ∈ [-0.25, 0.25] — and z = 3 is well outside → miss.
        let yaw = DQuat::from_rotation_y(std::f64::consts::FRAC_PI_2);
        assert!(
            !segment_hits_obb(seg.0, seg.1, DVec3::ZERO, yaw, he),
            "a yawed slab must be tested as an OBB, not by its axis-aligned bounds"
        );
    }

    #[test]
    fn obb_degenerate_box_occludes_nothing() {
        let seg = (DVec3::new(-5.0, 0.0, 0.0), DVec3::new(5.0, 0.0, 0.0));
        assert!(!segment_hits_obb(seg.0, seg.1, DVec3::ZERO, DQuat::IDENTITY, DVec3::ZERO));
        assert!(!segment_hits_obb(
            seg.0,
            seg.1,
            DVec3::ZERO,
            DQuat::IDENTITY,
            DVec3::new(1.0, 0.0, 1.0),
        ));
    }

    /// Translated + rotated: the caller supplies grid-absolute poses, so the test
    /// has to be right away from the origin too.
    #[test]
    fn obb_off_origin() {
        let center = DVec3::new(1_737_400.0, 250.0, -900.0); // lunar-scale coordinate
        let he = DVec3::new(4.0, 2.0, 0.5);
        let rot = DQuat::from_rotation_y(0.3);
        let through = (center + DVec3::new(0.0, 0.0, -20.0), center + DVec3::new(0.0, 0.0, 20.0));
        let past = (
            center + DVec3::new(0.0, 40.0, -20.0),
            center + DVec3::new(0.0, 40.0, 20.0),
        );
        assert!(segment_hits_obb(through.0, through.1, center, rot, he), "through the box");
        assert!(!segment_hits_obb(past.0, past.1, center, rot, he), "40 m above the box");
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
        // that WAS at lon λ ahead by θ. Measure the advance IN THE BODY'S OWN
        // equatorial frame (`equatorial_frame().inverse()`), so the pole's tilt
        // — real now, derived from the IAU elements — doesn't leak into the
        // reading. (This used to force `polar_axis = +Y` to dodge that; the
        // field is gone, and this is the honest version of the same check.)
        let desc = moon();
        let geo = Geodetic::new(0.0, 10.0, 0.0);
        let jd0 = lunco_time::J2000_JD;
        let quarter_day = 0.25 * std::f64::consts::TAU / desc.rotation_rate_rad_per_day();
        let lon_at = |jd: f64| {
            let p_inertial = body_rotation(&desc, jd) * geodetic_to_body_fixed(&geo, desc.radius_m);
            let p_eq = equatorial_frame(&desc, jd).inverse() * p_inertial;
            body_fixed_to_geodetic(p_eq, desc.radius_m).lon_deg
        };
        let delta = (lon_at(jd0 + quarter_day) - lon_at(jd0)).rem_euclid(360.0);
        // Tolerance covers the pole's own slow motion + lunar physical libration
        // over the quarter turn (~6.8 d), both of which are now modelled.
        assert!((delta - 90.0).abs() < 0.5, "quarter turn should advance lon by 90°, got {delta}");
    }

    /// **P2 regression — the missing prime-meridian epoch (`W₀`).**
    ///
    /// A rotation model with the right RATE and no PHASE looks fine in every
    /// rate-only test (a quarter turn is still a quarter turn) and in every
    /// polar-site elevation test (longitude-insensitive at the pole). It is
    /// wrong by a fixed offset — 38.3° for the Moon, ~1160 km at the equator.
    ///
    /// Pin the phase directly: at J2000 the Moon's prime meridian (lon 0) must
    /// point `W₀ = 38.32°` east of the node of its equator, not 0°.
    #[test]
    fn moon_prime_meridian_has_the_w0_phase_at_j2000() {
        let desc = moon();
        let jd = lunco_time::J2000_JD;
        // Where lon 0 actually points, expressed in the body's equatorial frame
        // (whose +X is the engine's equinox direction, tilted onto the equator).
        let pm = equatorial_frame(&desc, jd).inverse()
            * (body_rotation(&desc, jd) * geodetic_to_body_fixed(&Geodetic::new(0.0, 0.0, 0.0), 1.0));
        let angle = (-pm.z).atan2(pm.x).to_degrees().rem_euclid(360.0);
        assert!(
            (angle - 38.3).abs() < 1.5,
            "the Moon's prime meridian must lead by W₀ ≈ 38.3° at J2000, got {angle:.2}° \
             (0° ⇒ the W₀ phase is missing again — the near side stops facing Earth)"
        );
    }

    /// **P3 regression — Kepler elements must be referenced to the body's
    /// equator, not the ecliptic.**
    ///
    /// A `i = 90°` orbit about Earth must pass over the GEOGRAPHIC (body-fixed)
    /// poles. Built about the ecliptic pole instead — which is what
    /// `position_bevy_m` + `placement`'s full-rotation cancellation used to do —
    /// the same orbit tops out at latitude 66.6°, i.e. 23.4° short: an Arctic
    /// Circle orbit sold as a polar one.
    #[test]
    fn polar_orbit_passes_over_the_geographic_poles() {
        use crate::kepler::KeplerianElements;
        let earth = crate::registry::CelestialBodyRegistry::default_system()
            .bodies
            .into_iter()
            .find(|b| b.ephemeris_id == 399)
            .unwrap();
        let el = KeplerianElements {
            semi_major_axis_m: 6_778.0e3,
            eccentricity: 0.0,
            inclination_deg: 90.0,
            raan_deg: 0.0,
            ..Default::default()
        };
        let period_days = el.period_s(earth.gm) / 86_400.0;

        let mut max_lat: f64 = 0.0;
        for k in 0..400 {
            let jd = el.epoch_jd + period_days * (k as f64) / 400.0;
            // The chain `placement` uses: elements → body equatorial frame →
            // inertial → body-fixed.
            let p_inertial = equatorial_frame(&earth, jd) * el.position_bevy_m(earth.gm, jd);
            let p_fixed = body_rotation(&earth, jd).inverse() * p_inertial;
            let lat = body_fixed_to_geodetic(p_fixed, earth.radius_m).lat_deg;
            max_lat = max_lat.max(lat.abs());
        }
        assert!(
            max_lat > 89.5,
            "an i=90° orbit must cross the geographic poles; peak |lat| was {max_lat:.2}° \
             (≈66.6° ⇒ the elements are referenced to the ECLIPTIC pole, 23.4° off Earth's)"
        );
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
