//! **Translating between reference frames** — the half that makes the frame types useful.
//!
//! Typed frames stop you *mixing* frames. This module lets you *change* them, which is the
//! thing you actually want: the Moon's trajectory looks completely different depending on where
//! you stand.
//!
//! ```text
//!   same Moon, same epochs, three frames:
//!     Center::Body(10)  in Solar        → a near-circle, wobbling (heliocentric: Earth's orbit
//!                                          with a small monthly ripple — it never loops back)
//!     Center::Body(399) in Solar        → the familiar ellipse (geocentric)
//!     Pair{399,301}     in Synodic      → a small closed blob (co-rotating: the Moon barely
//!                                          moves, and L1..L5 sit still beside it)
//! ```
//!
//! # Hub-and-spoke, like SPICE
//!
//! SPICE composes every transform through J2000; Orekit walks a tree; Astropy resolves a
//! transform graph. All three share one idea: **do not write an N×N table of conversions.**
//! There are ~6 frame kinds and an open set of centres — a direct A→B function for each pair is
//! dozens of functions, most of them never called, every one of them a place to get the
//! obliquity backwards (which this codebase has already done once, in the dark, at Shackleton).
//!
//! So everything converts through **one hub**: [`Solar`] (ecliptic J2000, Bevy axes, metres).
//! Every frame needs exactly two functions — *into* the hub and *out of* it — and any A→B is
//! their composition. Adding a frame is O(1), not O(N).
//!
//! # big_space: precision, not a frame
//!
//! A `big_space` grid is **not a reference frame** — it is a *precision encoding*. `(CellCoord
//! i64, Vec3 f32)` and one `f64` metre vector are the *same position in the same frame*, stored
//! differently: the split exists so an `f32` render transform never has to hold an AU-scale
//! number and lose metres to rounding.
//!
//! Typing the grid as a frame would be a category error — you would be typing the *storage*, and
//! every `translation_to_grid` would become a conversion that means nothing. So the seam is:
//!
//! - **frames** = semantics (what the axes mean, where the origin is);
//! - **big_space** = precision (how that number survives being an `f32` on the GPU).
//!
//! [`Pos<Solar>`] — f64, absolute, metres — is exactly what you hand a grid:
//!
//! ```ignore
//! let p: Pos<Solar> = tree.center_in_solar(Center::Body(301));
//! let (cell, offset) = grid.translation_to_grid(p.raw());   // precision split, same frame
//! ```
//!
//! What a grid DOES carry is a frame: the solar grid's axes are [`Solar`]; a body's grid is that
//! body's. That association is the grid entity's business (see `placement.rs`), not the number's
//! — which is why [`Pos`] stops at f64 and does not know about cells.
//!
//! # What a transform needs
//!
//! An epoch (`jd`) and the body registry: a body-fixed frame's orientation *is* a function of
//! time, and where a centre sits *is* a function of the ephemeris. That is why this is a struct
//! you build per query, not a set of free functions.

use bevy::math::DQuat;
use bevy::math::DVec3;
use bevy::prelude::{Component, Reflect, ReflectComponent};

use crate::coords::ecliptic_to_bevy;
use crate::ephemeris::EphemerisProvider;
use crate::frames::{
    BodyFixed, BodyId, BodyInertial, Center, LPoint, Pair, Pos, SiteEnu, Solar, Synodic,
};
use crate::geo;
use crate::registry::CelestialBodyRegistry;

/// Places an entity at a **libration point** of a two-body pair — Earth–Moon L1 is
/// `{ primary: 399, secondary: 301, point: L1 }`.
///
/// The third placement kind, beside [`GeodeticAnchor`] (on a body) and [`KeplerOrbit`]
/// (around a body). It lives here, with the solver that resolves it, exactly as
/// `KeplerOrbit` lives with `position_bevy_m` — [`pose.rs`] reads it and writes a
/// [`SolarFramePose`], after which a libration relay is just another link node.
///
/// **`up` is ZERO**, like an orbit: an L-point has no local horizon, so an elevation
/// mask against it is meaningless (and the kernel skips it, as it does for orbits).
///
/// # What this is and is not
///
/// It is a **frame origin**, not a trajectory. L1/L2/L3 come from the first-order
/// CR3BP series (good to ~1% of the L-point distance for Earth–Moon); L4/L5 are
/// exact. A station is never *at* an L-point — it flies a halo/Lissajous orbit
/// *around* one, which you integrate in [`Synodic`]. Author this for "a relay parked
/// at L1" and you get the point itself, which is the right level of fidelity for
/// connectivity geometry and the wrong one for station-keeping ΔV.
///
/// # A warning worth authoring against
///
/// L1 lies ON the primary–secondary line, so from the lunar surface it is within
/// **~1.4°** of Earth in the sky (0° at the sub-Earth point). Terrain that blocks
/// Earth blocks L1 too — an L1 relay does NOT cure a radio shadow. **L2** serves the
/// far side, where Earth is never visible. What L1 buys is range (~61,300 km from
/// the Moon versus 384,400 km to Earth — 6.3× closer) and freedom from DSN handover.
///
/// [`GeodeticAnchor`]: crate::geo::GeodeticAnchor
/// [`KeplerOrbit`]: crate::kepler::KeplerOrbit
/// [`SolarFramePose`]: crate::pose::SolarFramePose
/// [`pose.rs`]: crate::pose
#[derive(Component, Debug, Clone, Copy, Reflect)]
#[reflect(Component)]
pub struct LibrationAnchor {
    /// NAIF id of the primary (399 Earth for Earth–Moon, 10 Sun for Sun–Earth).
    pub primary: BodyId,
    /// NAIF id of the secondary (301 Moon for Earth–Moon).
    pub secondary: BodyId,
    /// Which of L1–L5.
    pub point: LPoint,
}

impl Default for LibrationAnchor {
    /// Earth–Moon L1 — the pair this simulator is about.
    fn default() -> Self {
        Self {
            primary: 399,
            secondary: 301,
            point: LPoint::L1,
        }
    }
}

/// A frame-conversion context: the epoch, the bodies, and where they are.
///
/// Build one per query. It borrows rather than owning so a system can make one from its existing
/// `Res<>`s without cloning the ephemeris.
pub struct FrameTree<'a> {
    pub jd: f64,
    pub registry: &'a CelestialBodyRegistry,
    pub ephemeris: &'a dyn EphemerisProvider,
}

impl<'a> FrameTree<'a> {
    pub fn new(
        jd: f64,
        registry: &'a CelestialBodyRegistry,
        ephemeris: &'a dyn EphemerisProvider,
    ) -> Self {
        Self {
            jd,
            registry,
            ephemeris,
        }
    }

    // ── The hub: where is a centre, in Solar? ────────────────────────────────────────────────

    /// The position of any [`Center`] in the hub frame, or `None` when the ephemeris
    /// does not carry the body it needs.
    ///
    /// This is the one place that knows how to *find* things, and everything else composes
    /// through it.
    ///
    /// **Why `Option`.** `global_position` returns one, and the alternative — falling back
    /// to `ZERO` — would place the body at the SUN'S CENTRE and report it with exactly the
    /// confidence of a real answer. That is the failure `pose.rs` already refuses by name
    /// ("skipping beats reporting a pose at the Sun's centre that looks exactly like a real
    /// one"). A missing body is a `None` all the way up; callers skip.
    pub fn center_in_solar(&self, center: Center) -> Option<Pos<Solar>> {
        match center {
            // The provider is heliocentric, so the Sun is the origin by construction. (The true
            // SSB differs from the Sun's centre by roughly a solar radius — under a thousandth
            // of an AU — and the provider does not model it; saying so beats pretending.)
            Center::Ssb => Some(Pos::<Solar>::ZERO),
            Center::Body(id) => self
                .ephemeris
                .global_position(id, self.jd)
                .map(ecliptic_to_bevy),
            Center::Libration {
                primary,
                secondary,
                point,
            } => self.libration_in_solar(primary, secondary, point),
            // A site's position needs its geodetic anchor, which lives in the scene, not here.
            // Returning the body centre is honest (the site is *on* that body) and wrong by at
            // most a body radius — but a caller that needs site precision must go through
            // `geo::solar_position_of_geodetic`, which has the anchor.
            Center::Site { body, .. } => self
                .ephemeris
                .global_position(body, self.jd)
                .map(ecliptic_to_bevy),
        }
    }

    /// A libration point of `primary`/`secondary`, in the hub frame.
    ///
    /// Collinear points (L1/L2/L3) use the standard CR3BP series in the mass ratio; the
    /// triangular points (L4/L5) are exact — they sit at the apex of an equilateral triangle on
    /// the primary–secondary line, and no approximation is involved.
    ///
    /// The collinear series is first-order in `(µ/3)^(1/3)`. That is good to ~1% of the L-point
    /// distance for Earth–Moon and much better for Sun–Earth. It is a *frame origin*, not a
    /// trajectory: if you are propagating a halo orbit you integrate in [`Synodic`], you do not
    /// lean on this number.
    /// `None` if either body is missing from the ephemeris or the registry — an
    /// L-point is defined by a PAIR, so half a pair is not a point, and a silent
    /// fallback to the primary's centre would put a relay 61,000 km from where the
    /// scene says it is.
    pub fn libration_in_solar(
        &self,
        primary: BodyId,
        secondary: BodyId,
        point: LPoint,
    ) -> Option<Pos<Solar>> {
        let p = self.center_in_solar(Center::Body(primary))?;
        let s = self.center_in_solar(Center::Body(secondary))?;
        let r = s - p;
        let d = r.length();
        if d <= 0.0 {
            return None;
        }

        // Both masses are REQUIRED: µ is the whole model. A missing gm silently
        // read as 0.0 gives µ = 0 or 1 and an L-point sitting exactly on a body.
        let gm_p = self.registry.get(primary)?.gm;
        let gm_s = self.registry.get(secondary)?.gm;
        let total = gm_p + gm_s;
        if total <= 0.0 {
            return None;
        }
        // µ = m2 / (m1 + m2)
        let mu = gm_s / total;
        // Hill radius factor — the scale of the collinear points' offset from the secondary.
        let hill = (mu / 3.0).powf(1.0 / 3.0);

        let unit = r * (1.0 / d);
        Some(match point {
            // Between the primaries, just inside the secondary.
            LPoint::L1 => s - unit * (d * hill),
            // Beyond the secondary.
            LPoint::L2 => s + unit * (d * hill),
            // Opposite the secondary, just beyond the primary's far side.
            LPoint::L3 => p - unit * (d * (1.0 + 5.0 * mu / 12.0)),
            // Equilateral — EXACT. Rotate the primary→secondary vector by ±60° about the orbit
            // normal, which we take from the pair's own motion rather than assuming the ecliptic.
            LPoint::L4 | LPoint::L5 => {
                let normal = self.pair_orbit_normal(primary, secondary)?;
                let sign = if matches!(point, LPoint::L4) {
                    1.0
                } else {
                    -1.0
                };
                let rot = DQuat::from_axis_angle(normal, sign * std::f64::consts::FRAC_PI_3);
                p + Pos::<Solar>::new(rot * r.raw())
            }
        })
    }

    /// The orbit normal of `secondary` about `primary`, from finite difference of the ephemeris.
    ///
    /// Finite difference, because the provider is position-only — there are no velocities
    /// anywhere in this crate. A one-hour step is small against any orbit we model and large
    /// against `f64` noise at AU scale.
    fn pair_orbit_normal(&self, primary: BodyId, secondary: BodyId) -> Option<DVec3> {
        const DT_DAYS: f64 = 1.0 / 24.0;
        let r0 = (self.center_in_solar(Center::Body(secondary))?
            - self.center_in_solar(Center::Body(primary))?)
        .raw();
        let later = FrameTree::new(self.jd + DT_DAYS, self.registry, self.ephemeris);
        let r1 = (later.center_in_solar(Center::Body(secondary))?
            - later.center_in_solar(Center::Body(primary))?)
        .raw();
        let n = r0.cross(r1);
        Some(if n.length_squared() > 0.0 {
            n.normalize()
        } else {
            // Degenerate (the two coincide, or the ephemeris is a stub) — fall back to ecliptic
            // north rather than producing a NaN that would silently poison every downstream
            // rotation.
            DVec3::Y
        })
    }

    // ── Into the hub ────────────────────────────────────────────────────────────────────────

    /// Body-fixed (rotating with the body) → Solar. `None` if the body is unknown.
    pub fn body_fixed_to_solar(&self, p: Pos<BodyFixed>) -> Option<Pos<Solar>> {
        let Center::Body(id) = p.center() else {
            // A body-fixed frame centred on something that is not a body is a contradiction;
            // saying so beats silently rotating by identity.
            debug_assert!(false, "body-fixed frame centred on {:?}", p.center());
            return None;
        };
        let desc = self.registry.get(id)?;
        let rot = geo::body_rotation(desc, self.jd);
        Some(self.center_in_solar(Center::Body(id))? + Pos::<Solar>::new(rot * p.raw()))
    }

    /// Body-centred inertial (NOT rotating — the frame Kepler elements live in) → Solar.
    /// `None` if the body is unknown.
    pub fn body_inertial_to_solar(&self, p: Pos<BodyInertial>) -> Option<Pos<Solar>> {
        let Center::Body(id) = p.center() else {
            debug_assert!(false, "body-inertial frame centred on {:?}", p.center());
            return None;
        };
        let desc = self.registry.get(id)?;
        // The equatorial lift. Skipping it is a live bug in `pose.rs` — it costs up to the
        // body's axial tilt (±23.4° of ground track on Earth), silently.
        let rot = geo::equatorial_frame(desc, self.jd);
        Some(self.center_in_solar(Center::Body(id))? + Pos::<Solar>::new(rot * p.raw()))
    }

    /// Synodic / co-rotating (the CR3BP frame; L1–L5 are stationary here) → Solar.
    ///
    /// Basis: +X along primary→secondary, +Y the orbit normal, +Z completing it. Origin at the
    /// pair's barycentre, which is what makes L-points sit still.
    pub fn synodic_to_solar(&self, p: Pos<Synodic>) -> Option<Pos<Solar>> {
        let Pair { primary, secondary } = p.pair();
        let rot = self.synodic_basis(primary, secondary)?;
        Some(self.pair_barycenter(primary, secondary)? + Pos::<Solar>::new(rot * p.raw()))
    }

    /// Solar → synodic. The inverse of [`synodic_to_solar`](Self::synodic_to_solar) — this is
    /// what turns a heliocentric trajectory into the co-rotating picture where a halo orbit
    /// looks like a halo orbit.
    pub fn solar_to_synodic(&self, p: Pos<Solar>, pair: Pair) -> Option<Pos<Synodic>> {
        let rot = self.synodic_basis(pair.primary, pair.secondary)?.inverse();
        let rel = p - self.pair_barycenter(pair.primary, pair.secondary)?;
        Some(Pos::<Synodic>::in_pair(pair, rot * rel.raw()))
    }

    fn synodic_basis(&self, primary: BodyId, secondary: BodyId) -> Option<DQuat> {
        let p = self.center_in_solar(Center::Body(primary))?;
        let s = self.center_in_solar(Center::Body(secondary))?;
        let x = (s - p).raw();
        if x.length_squared() <= 0.0 {
            return Some(DQuat::IDENTITY);
        }
        let x = x.normalize();
        let n = self.pair_orbit_normal(primary, secondary)?;
        // Orthonormalise: the normal is already perpendicular to x up to numerical noise, but
        // building a basis from a not-quite-orthogonal pair yields a skewed frame that shows up
        // as a slow drift, not an error.
        let z = x.cross(n).normalize();
        let y = z.cross(x).normalize();
        Some(DQuat::from_mat3(&bevy::math::DMat3::from_cols(x, y, z)))
    }

    fn pair_barycenter(&self, primary: BodyId, secondary: BodyId) -> Option<Pos<Solar>> {
        let p = self.center_in_solar(Center::Body(primary))?;
        let s = self.center_in_solar(Center::Body(secondary))?;
        // Both masses REQUIRED — a missing gm read as 0.0 silently puts the
        // barycentre on one of the bodies.
        let gm_p = self.registry.get(primary)?.gm;
        let gm_s = self.registry.get(secondary)?.gm;
        let total = gm_p + gm_s;
        if total <= 0.0 {
            return None;
        }
        Some(p + (s - p) * (gm_s / total))
    }

    // ── Re-centring within the hub ──────────────────────────────────────────────────────────

    /// Re-express a hub-frame position relative to a different centre.
    ///
    /// **This is what makes "the Moon around the Sun" and "the Moon around the Earth" the same
    /// data.** Feed a trajectory through it with `Center::Body(10)` and you get the heliocentric
    /// path; with `Center::Body(399)` you get the geocentric ellipse. Nothing else changes.
    pub fn relative_to(&self, p: Pos<Solar>, center: Center) -> Option<Pos<Solar>> {
        Some(p - self.center_in_solar(center)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frames::Ecliptic;

    /// A stub provider: Earth on +X at 1 AU, Moon just outside it. Enough to exercise the
    /// geometry without pulling in VSOP.
    struct Stub;
    impl EphemerisProvider for Stub {
        fn position(&self, body_id: i32, _jd: f64) -> Option<Pos<Ecliptic>> {
            match body_id {
                10 => Some(Pos::<Ecliptic>::ZERO), // Sun
                399 => Some(Pos::<Ecliptic>::new(DVec3::new(1.0, 0.0, 0.0))),
                301 => Some(Pos::<Ecliptic>::new(DVec3::new(1.00257, 0.0, 0.0))), // ~384 400 km out
                _ => None, // and an unknown body is NOT at the origin
            }
        }
        // Flat tree: the stub already returns heliocentric positions.
        fn global_position(&self, body_id: i32, jd: f64) -> Option<Pos<Ecliptic>> {
            self.position(body_id, jd)
        }
    }

    /// The real catalog — the stub supplies POSITIONS, the registry supplies the
    /// MASSES (`gm`) the CR3BP needs, and there is no second hand-written copy of
    /// those to drift.
    fn registry() -> CelestialBodyRegistry {
        CelestialBodyRegistry::default_system()
    }

    /// THE PROPERTY THE USER ASKED FOR: the same Moon, re-expressed, is a different trajectory.
    /// Heliocentric it is ~1 AU from the origin; geocentric it is ~384 000 km.
    #[test]
    fn the_moon_looks_different_depending_on_what_you_orbit() {
        let reg = registry();
        let tree = FrameTree::new(2_451_545.0, &reg, &Stub);

        let moon_solar = tree
            .center_in_solar(Center::Body(301))
            .expect("stub has the Moon");

        let heliocentric = tree.relative_to(moon_solar, Center::Body(10)).unwrap();
        let geocentric = tree.relative_to(moon_solar, Center::Body(399)).unwrap();

        let au = crate::coords::AU_TO_M;
        assert!(
            (heliocentric.length() - 1.00257 * au).abs() < 1.0e6,
            "heliocentric: the Moon is an AU from the Sun"
        );
        assert!(
            (geocentric.length() - 0.00257 * au).abs() < 1.0e6,
            "geocentric: the Moon is a few hundred thousand km from Earth — same data, same \
             instant, utterly different curve"
        );
        assert!(heliocentric.length() > geocentric.length() * 100.0);
    }

    /// L4 is exact: it sits at the same distance from BOTH primaries (equilateral triangle).
    #[test]
    fn the_triangular_points_are_equilateral() {
        let reg = registry();
        let tree = FrameTree::new(2_451_545.0, &reg, &Stub);
        let l4 = tree
            .libration_in_solar(10, 399, LPoint::L4)
            .expect("Sun+Earth are known");

        let sun = tree.center_in_solar(Center::Body(10)).unwrap();
        let earth = tree.center_in_solar(Center::Body(399)).unwrap();
        let d_sun = (l4 - sun).length();
        let d_earth = (l4 - earth).length();
        let d_pair = (earth - sun).length();

        assert!(
            (d_sun - d_pair).abs() / d_pair < 1.0e-6,
            "L4 is one leg from the primary"
        );
        assert!(
            (d_earth - d_pair).abs() / d_pair < 1.0e-6,
            "…and one leg from the secondary"
        );
    }

    /// The synodic frame is where L-points hold still — round-tripping through it must be
    /// lossless, or nothing built on top of it can be trusted.
    #[test]
    fn synodic_round_trips() {
        let reg = registry();
        let tree = FrameTree::new(2_451_545.0, &reg, &Stub);
        let pair = Pair {
            primary: 10,
            secondary: 399,
        };

        let original = Pos::<Solar>::new(DVec3::new(1.4e11, 3.0e9, -2.0e9));
        let syn = tree
            .solar_to_synodic(original, pair)
            .expect("Sun+Earth are known");
        let back = tree.synodic_to_solar(syn).expect("Sun+Earth are known");

        let err = (back - original).length();
        assert!(
            err < 1.0,
            "round-trip error {err} m — the basis is not orthonormal"
        );
    }

    /// An unknown body yields `None`, never a position.
    ///
    /// This is the whole reason `center_in_solar` returns `Option`: the old signature
    /// could only answer with a `Pos`, so a body the ephemeris has never heard of came
    /// back as `ZERO` — the SUN'S CENTRE — indistinguishable from a real answer. A
    /// relay placed there would be 1 AU from where the scene put it, silently.
    #[test]
    fn an_unknown_body_is_none_not_the_origin() {
        let reg = registry();
        let tree = FrameTree::new(2_451_545.0, &reg, &Stub);
        assert!(
            tree.center_in_solar(Center::Body(499)).is_none(),
            "Mars is not in the stub"
        );
        // …and an L-point needs BOTH bodies: half a pair is not a point.
        assert!(tree.libration_in_solar(399, 499, LPoint::L1).is_none());
        assert!(tree.libration_in_solar(499, 301, LPoint::L2).is_none());
    }

    /// **Earth–Moon L1 sits between the two, ~61,300 km from the Moon.**
    ///
    /// The number the connectivity work turns on: a relay there is 6.3× closer than
    /// Earth, and — because it lies ON the Earth–Moon line — it is within ~1.4° of
    /// Earth in the lunar sky, so it shares Earth's terrain shadow rather than curing
    /// it. L2 is the mirror image, beyond the far side.
    #[test]
    fn earth_moon_l1_sits_between_the_bodies_at_the_hill_radius() {
        let reg = registry();
        let tree = FrameTree::new(2_451_545.0, &reg, &Stub);

        let earth = tree.center_in_solar(Center::Body(399)).unwrap();
        let moon = tree.center_in_solar(Center::Body(301)).unwrap();
        let l1 = tree.libration_in_solar(399, 301, LPoint::L1).unwrap();
        let l2 = tree.libration_in_solar(399, 301, LPoint::L2).unwrap();

        let d_pair = (moon - earth).length();
        let l1_from_moon = (l1 - moon).length();
        let l2_from_moon = (l2 - moon).length();

        // Hill factor for Earth–Moon: (µ/3)^(1/3) ≈ 0.159 → ≈ 61,300 km.
        assert!(
            (l1_from_moon - 61_300_000.0).abs() < 2.0e6,
            "Earth–Moon L1 should be ~61,300 km from the Moon, got {:.0} km",
            l1_from_moon / 1000.0
        );
        // L1 is BETWEEN them: closer to Earth than the Moon is.
        assert!(
            (l1 - earth).length() < d_pair,
            "L1 must lie inside the pair"
        );
        // L2 is beyond: farther from Earth than the Moon is, by the same offset.
        assert!(
            (l2 - earth).length() > d_pair,
            "L2 must lie outside the pair"
        );
        assert!(
            (l1_from_moon - l2_from_moon).abs() < 1.0,
            "collinear L1/L2 are symmetric about the secondary in this first-order series"
        );
    }
}
