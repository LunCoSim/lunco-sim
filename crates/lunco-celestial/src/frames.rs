//! **A general reference-frame system.** Zero-cost, and the point is what it makes impossible.
//!
//! Every position in this crate used to be a bare `DVec3`, with the frame living only in the
//! variable name (`rel_pos_au`, `pos_bevy_m`, `body_local`, `site_in_solar`). The compiler
//! could not tell eight frames apart — and **the two most expensive bugs preserved in this
//! code's own comments are both silent frame mixes**:
//!
//! 1. **The Shackleton sun.** VSOP2013/ELP return ICRF/**equatorial** vectors; the
//!    `EphemerisProvider` contract says **ecliptic**. Nobody rotated. Every "up" and "north"
//!    was off by the obliquity (23.44°), which at Shackleton put the sun **~45° below the
//!    horizon** — pitch-black ground — instead of the real grazing ~1°. It was *double*-wrong:
//!    `ecliptic_to_bevy` had itself been rotating by the obliquity to compensate, so the fix
//!    had to land in two files at once.
//! 2. **The sun published in the wrong frame.** An ecliptic (solar) direction was written into
//!    `SunDirectionWorld`, which consumers read as **site-ENU**. Terrain lit from nowhere.
//!
//! Both were **silent** — no panic, no NaN, just a world lit from the wrong place. That is
//! exactly the failure a type system is for.
//!
//! # The split: KIND is a type, IDENTITY is a value
//!
//! There are going to be a *lot* of frames — Earth-rotating and Earth-inertial, Moon-fixed and
//! Moon-inertial, one ENU per site, and the same again for every body someone adds. Writing a
//! newtype per frame does not scale, and a type per *body* would fight the fact that bodies
//! come from a **registry**, i.e. from data.
//!
//! So this module splits the problem the way the bugs actually split:
//!
//! - **Frame kind** — equatorial vs ecliptic vs body-fixed vs body-inertial vs topocentric — is
//!   a small closed set, it is what both incidents got wrong, and it is a **type parameter**.
//!   Mixing kinds does not compile.
//! - **Frame identity** — *which* body, *which* site — is open and data-driven, so it is a
//!   **value** ([`Frame::Origin`]), checked at run time. Mixing Moon-fixed with Earth-fixed
//!   trips a `debug_assert`, not a compile error.
//!
//! Adding "Earth rotating / non-rotating" is therefore **two marker structs and no new
//! machinery** — [`BodyFixed`] and [`BodyInertial`] already cover them, scoped by body id.
//!
//! # Cost
//!
//! None. `Pos<F>` is a `DVec3` plus a zero-sized `Origin` for global frames
//! (`size_of::<Pos<Solar>>() == size_of::<DVec3>()`, asserted in the tests). The refactor
//! changes no math — only what the compiler will let you *say*.
//!
//! # What is deliberately NOT typed
//!
//! The render/root frame (`OrbitalViewPin`, `LocalGravityField`) and grid-local
//! `CellCoord`+f32. They sit downstream of every conversion, have never been an incident site,
//! and typing them would drag four more crates in for no prevention value. **Type frames where
//! they get MIXED, not everywhere** — a type system nobody can afford to keep gets bypassed.

use std::fmt;
use std::marker::PhantomData;

use bevy::math::DVec3;

/// The length unit a frame is measured in. AU and metres differ by 1.5e11; conflating them is
/// its own bug class, so it rides along with the frame rather than being a separate wrapper.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    Au,
    Metre,
}

/// NAIF id of a body or barycentre (Sun 10, Earth 399, Moon 301, EMB 3, SSB 0).
pub type BodyId = i32;

/// A collinear/triangular libration point of a two-body pair.
///
/// `Reflect` + `Default` so it can be a field of the [`LibrationAnchor`] component
/// (which the USD bridge authors and the network reflects).
///
/// [`LibrationAnchor`]: crate::transform::LibrationAnchor
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, bevy::prelude::Reflect)]
pub enum LPoint {
    #[default]
    L1,
    L2,
    L3,
    L4,
    L5,
}

impl LPoint {
    /// Parse an authored token (`"L1"`…`"L5"`, case-insensitive). `None` if it names
    /// no such point — the caller says so rather than silently placing an L1.
    pub fn from_token(s: &str) -> Option<Self> {
        match s.trim().to_ascii_uppercase().as_str() {
            "L1" => Some(Self::L1),
            "L2" => Some(Self::L2),
            "L3" => Some(Self::L3),
            "L4" => Some(Self::L4),
            "L5" => Some(Self::L5),
            _ => None,
        }
    }
}

/// **What a frame is centred on.**
///
/// This is the *open* half of a frame, and it is a VALUE, not a type — which is the lesson
/// every mature tool in this space has already learned:
///
/// - **SPICE** (`spkezr(target, et, ref, obs)`) identifies frames and centres by runtime
///   integer id and composes transforms through a kernel-loaded graph. `L1` is not a special
///   case; it is just another centre.
/// - **Astropy** makes the *orientation* a class (ICRS, GCRS, ITRS) and carries the rest —
///   obstime, observer location — as frame **attributes**, i.e. values.
/// - **Orekit** is a runtime `Frame` tree with pluggable `TransformProvider`s.
///
/// None of them enumerate every frame as a distinct compile-time type, because the set is
/// open: bodies arrive from a registry, sites from a scene, libration points from a *pair*.
/// A type per frame would mean a type per body — which is precisely what our dynamic body
/// registry exists to avoid.
///
/// So: **orientation is typed, centre is checked.**
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Center {
    /// Solar-system barycentre (NAIF 0).
    Ssb,
    /// A body or barycentre, by NAIF id.
    Body(BodyId),
    /// A libration point of a primary/secondary pair — e.g. Earth–Moon L1 is
    /// `Libration { primary: 399, secondary: 301, point: L1 }`.
    ///
    /// It is a CENTRE, not a frame kind. The frame an L-point naturally lives in is the pair's
    /// [`Synodic`] (co-rotating) frame, where L1–L5 are stationary — which is why `Synodic`
    /// carries the same pair.
    Libration {
        primary: BodyId,
        secondary: BodyId,
        point: LPoint,
    },
    /// A surface site (a geodetic anchor on a body).
    Site { body: BodyId, id: u32 },
}

/// A reference frame *kind*.
///
/// `Origin` is what distinguishes two frames of the same kind: `()` for a globally unique frame
/// (there is only one ICRF), [`Center`] for one that is centred somewhere (there is an
/// Earth-fixed frame and a Moon-fixed frame, and they are not the same), and [`Pair`] for a
/// frame defined by two bodies (the Earth–Moon synodic frame).
pub trait Frame: Copy + Clone + fmt::Debug + 'static {
    const NAME: &'static str;
    const UNIT: Unit;
    /// `()` ⇒ globally unique. [`Center`] ⇒ centred. [`Pair`] ⇒ defined by a body pair.
    type Origin: Copy + Clone + PartialEq + fmt::Debug;
}

/// The primary/secondary pair that defines a co-rotating frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pair {
    pub primary: BodyId,
    pub secondary: BodyId,
}

/// **ICRF / equatorial, AU.** What VSOP2013, ELP/MPP02, and the IAU/WGCCRE pole tables actually
/// speak.
///
/// Nothing downstream accepts it. It reaches [`Ecliptic`] only through the obliquity rotation,
/// and that rotation is the *only* constructor — which is what makes the Shackleton bug a
/// compile error instead of a dark planet.
#[derive(Debug, Clone, Copy)]
pub struct Icrf;
impl Frame for Icrf {
    const NAME: &'static str = "ICRF/equatorial";
    const UNIT: Unit = Unit::Au;
    type Origin = ();
}

/// **Ecliptic J2000, AU.** The `EphemerisProvider` contract — now a type rather than a sentence
/// in a doc comment, which is the whole lesson of incident 1.
#[derive(Debug, Clone, Copy)]
pub struct Ecliptic;
impl Frame for Ecliptic {
    const NAME: &'static str = "ecliptic J2000";
    const UNIT: Unit = Unit::Au;
    type Origin = ();
}

/// **The solar frame: ecliptic J2000 on Bevy axes (Y = ecliptic north), METRES.** The crate's
/// one canonical math frame — all of `geo` lives here.
#[derive(Debug, Clone, Copy)]
pub struct Solar;
impl Frame for Solar {
    const NAME: &'static str = "solar (ecliptic, Bevy axes, m)";
    const UNIT: Unit = Unit::Metre;
    type Origin = ();
}

/// **Body-fixed, metres — the frame that ROTATES with a body** (ECEF for Earth, ME for the
/// Moon). Pole = +Y, prime meridian = +X.
///
/// Body-scoped: an Earth-fixed vector and a Moon-fixed vector are both `Pos<BodyFixed>`, and
/// combining them trips a `debug_assert` on the origin. Making them distinct *types* would mean
/// a type per body — which the registry, quite rightly, does not have.
#[derive(Debug, Clone, Copy)]
pub struct BodyFixed;
impl Frame for BodyFixed {
    const NAME: &'static str = "body-fixed";
    const UNIT: Unit = Unit::Metre;
    type Origin = Center;
}

/// **Body-centred INERTIAL, metres — the same origin as [`BodyFixed`] but NOT rotating** (ECI /
/// GCRF for Earth). This is the frame Kepler elements live in.
///
/// The distinction is load-bearing: `KeplerianElements::position_bevy_m` returns *this*, and
/// `placement.rs` lifts it with `equatorial_frame()` before use — while `pose.rs` currently does
/// **not**, so an orbiting node's solar pose and its rendered position are computed in different
/// frames. That is a live bug this type makes visible.
#[derive(Debug, Clone, Copy)]
pub struct BodyInertial;
impl Frame for BodyInertial {
    const NAME: &'static str = "body-inertial";
    const UNIT: Unit = Unit::Metre;
    type Origin = Center;
}

/// **Synodic / co-rotating, metres — the frame of a two-body pair, rotating with their line of
/// centres.** The natural home of the CR3BP: in the Earth–Moon synodic frame, **L1–L5 are fixed
/// points**.
///
/// Its origin is the [`Pair`], not a single body — which is exactly why "orientation" cannot be
/// collapsed into "centre", and why a newtype-per-frame scheme falls over here. A halo orbit
/// about Earth–Moon L2 is a `Pos<Synodic>` with `Pair { primary: 399, secondary: 301 }`, and its
/// centre is `Center::Libration { .., point: L2 }`.
#[derive(Debug, Clone, Copy)]
pub struct Synodic;
impl Frame for Synodic {
    const NAME: &'static str = "synodic (co-rotating)";
    const UNIT: Unit = Unit::Metre;
    type Origin = Pair;
}

/// **Site-ENU, metres** (East = +X, North = −Z, Up = +Y) — the frame a site-anchored scene
/// actually renders in.
///
/// A [`Solar`] direction is NOT one of these: it must be rotated by the site's `align`
/// quaternion first. That rotation is the fix for incident 2, and this type is what stops the
/// next person skipping it.
#[derive(Debug, Clone, Copy)]
pub struct SiteEnu;
impl Frame for SiteEnu {
    const NAME: &'static str = "site-ENU";
    const UNIT: Unit = Unit::Metre;
    type Origin = Center;
}

/// A position in frame `F`.
///
/// Cross-frame arithmetic does not compile. Same-kind/different-origin arithmetic
/// (Earth-fixed + Moon-fixed) trips a `debug_assert`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pos<F: Frame> {
    v: DVec3,
    origin: F::Origin,
    _f: PhantomData<F>,
}

impl<F: Frame<Origin = ()>> Pos<F> {
    /// Wrap a raw vector as being in this (globally unique) frame.
    ///
    /// **This is an assertion, not a conversion.** You are telling the compiler you already know
    /// the frame. Use it where the frame is established by the source — a provider's contract, a
    /// rotation you just applied — and *never* to silence a type error, which is exactly how both
    /// incidents happened.
    #[inline]
    pub const fn new(v: DVec3) -> Self {
        Self {
            v,
            origin: (),
            _f: PhantomData,
        }
    }

    pub const ZERO: Self = Self {
        v: DVec3::ZERO,
        origin: (),
        _f: PhantomData,
    };
}

impl<F: Frame<Origin = Center>> Pos<F> {
    /// Wrap a raw vector as being in the instance of this frame centred at `center`. See the
    /// caveat on [`Pos::new`] — this is an assertion, not a conversion.
    #[inline]
    pub const fn at(center: Center, v: DVec3) -> Self {
        Self {
            v,
            origin: center,
            _f: PhantomData,
        }
    }

    /// Convenience: centred on a body by NAIF id.
    #[inline]
    pub const fn at_body(body: BodyId, v: DVec3) -> Self {
        Self {
            v,
            origin: Center::Body(body),
            _f: PhantomData,
        }
    }

    /// What this frame is centred on.
    #[inline]
    pub const fn center(self) -> Center {
        self.origin
    }
}

impl<F: Frame<Origin = Pair>> Pos<F> {
    /// Wrap a raw vector as being in the co-rotating frame of `pair`.
    #[inline]
    pub const fn in_pair(pair: Pair, v: DVec3) -> Self {
        Self {
            v,
            origin: pair,
            _f: PhantomData,
        }
    }

    #[inline]
    pub const fn pair(self) -> Pair {
        self.origin
    }
}

impl<F: Frame> Pos<F> {
    /// The raw vector. Unwrapping is fine — the danger was never in the arithmetic, it was in
    /// handing the result to something that expected a different frame.
    #[inline]
    pub const fn raw(self) -> DVec3 {
        self.v
    }

    #[inline]
    pub fn length(self) -> f64 {
        self.v.length()
    }

    #[inline]
    pub fn normalize(self) -> Self {
        Self {
            v: self.v.normalize(),
            ..self
        }
    }

    /// Interpolate within one frame (an ephemeris table lookup between two samples).
    #[inline]
    pub fn lerp(self, rhs: Self, t: f64) -> Self {
        debug_assert!(self.same_origin(rhs), "lerp across two different origins");
        Self {
            v: self.v.lerp(rhs.v, t),
            ..self
        }
    }

    #[inline]
    pub fn length_squared(self) -> f64 {
        self.v.length_squared()
    }

    /// Down to `f32` for the render/GPU boundary. Lossy by nature — that is what big_space's
    /// cell split exists to contain — so this is the last thing you do, not something you do
    /// in the middle of the math.
    #[inline]
    pub fn as_vec3(self) -> bevy::math::Vec3 {
        self.v.as_vec3()
    }

    /// Re-express this vector in the same frame kind after a rotation you have already applied.
    /// Used by the conversion functions; not a way to change frames.
    #[inline]
    pub(crate) fn map(self, f: impl FnOnce(DVec3) -> DVec3) -> Self {
        Self {
            v: f(self.v),
            ..self
        }
    }

    #[inline]
    fn same_origin(self, other: Self) -> bool {
        self.origin == other.origin
    }
}

impl<F: Frame> std::ops::Add for Pos<F> {
    type Output = Self;
    #[inline]
    fn add(self, rhs: Self) -> Self {
        debug_assert!(
            self.same_origin(rhs),
            "{}: added vectors from two different origins ({:?} vs {:?}) — e.g. an Earth-fixed \
             vector and a Moon-fixed one. Same frame KIND, different frame.",
            F::NAME,
            self.origin,
            rhs.origin
        );
        Self {
            v: self.v + rhs.v,
            ..self
        }
    }
}

impl<F: Frame> std::ops::AddAssign for Pos<F> {
    #[inline]
    fn add_assign(&mut self, rhs: Self) {
        *self = *self + rhs;
    }
}

impl<F: Frame> std::ops::Sub for Pos<F> {
    type Output = Self;
    #[inline]
    fn sub(self, rhs: Self) -> Self {
        debug_assert!(
            self.same_origin(rhs),
            "{}: subtracted vectors from two different origins ({:?} vs {:?})",
            F::NAME,
            self.origin,
            rhs.origin
        );
        Self {
            v: self.v - rhs.v,
            ..self
        }
    }
}

impl<F: Frame> std::ops::Mul<f64> for Pos<F> {
    type Output = Self;
    #[inline]
    fn mul(self, rhs: f64) -> Self {
        Self {
            v: self.v * rhs,
            ..self
        }
    }
}

impl<F: Frame> std::ops::Neg for Pos<F> {
    type Output = Self;
    #[inline]
    fn neg(self) -> Self {
        Self { v: -self.v, ..self }
    }
}

/// Rotating a vector leaves it in the same frame — a rotation re-expresses a basis, it does not
/// change where you are standing. Frame CHANGES go through [`crate::transform::FrameTree`].
impl<F: Frame> std::ops::Mul<Pos<F>> for bevy::math::DQuat {
    type Output = Pos<F>;
    #[inline]
    fn mul(self, rhs: Pos<F>) -> Pos<F> {
        rhs.map(|v| self * v)
    }
}

// ── The names the rest of the crate uses ────────────────────────────────────────────────────
/// ICRF / equatorial, AU.
pub type IcrfAu = Pos<Icrf>;
/// Ecliptic J2000, AU — the `EphemerisProvider` contract.
pub type EclipticAu = Pos<Ecliptic>;
/// Solar frame (ecliptic, Bevy axes), metres — the canonical math frame.
pub type SolarM = Pos<Solar>;

impl IcrfAu {
    /// Kilometres → AU, staying in the ICRF. (ELP/MPP02 returns the Moon in km.)
    #[inline]
    pub fn from_km(v: DVec3, au_km: f64) -> Self {
        Self::new(v / au_km)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Zero-cost, or it gets bypassed the first time it is inconvenient.
    #[test]
    fn a_global_frame_vector_is_the_size_of_its_vector() {
        assert_eq!(std::mem::size_of::<SolarM>(), std::mem::size_of::<DVec3>());
        assert_eq!(
            std::mem::size_of::<EclipticAu>(),
            std::mem::size_of::<DVec3>()
        );
    }

    /// Same-frame arithmetic stays ergonomic — a type that makes correct code painful gets
    /// deleted, and then it protects nothing.
    #[test]
    fn same_frame_arithmetic_works() {
        let a = SolarM::new(DVec3::new(1.0, 2.0, 3.0));
        let b = SolarM::new(DVec3::new(1.0, 1.0, 1.0));
        assert_eq!((a - b).raw(), DVec3::new(0.0, 1.0, 2.0));
        assert_eq!((a * 2.0).raw(), DVec3::new(2.0, 4.0, 6.0));
    }

    /// A centred frame remembers WHAT it is centred on — this is what lets "Earth rotating" and
    /// "Moon rotating" be the same type without being the same frame, without a type per body.
    #[test]
    fn a_centred_frame_carries_its_center() {
        let earth = Pos::<BodyFixed>::at_body(399, DVec3::X);
        let moon = Pos::<BodyFixed>::at_body(301, DVec3::X);
        assert_eq!(earth.center(), Center::Body(399));
        assert_ne!(earth.center(), moon.center());
    }

    /// Mixing two bodies' body-fixed vectors is the runtime half of the guarantee.
    #[test]
    #[should_panic(expected = "two different origins")]
    fn adding_two_bodies_fixed_frames_is_caught() {
        let earth = Pos::<BodyFixed>::at_body(399, DVec3::X);
        let moon = Pos::<BodyFixed>::at_body(301, DVec3::X);
        let _ = earth + moon;
    }

    /// **L-points are the case that proves the design.** L1 is a CENTRE (open, data-driven), and
    /// the frame it is stationary in is the pair's SYNODIC orientation (closed, typed). Neither
    /// is expressible as a hardcoded newtype — and both fall out of this scheme for free.
    #[test]
    fn a_libration_point_is_a_center_in_a_synodic_frame() {
        let em = Pair {
            primary: 399,
            secondary: 301,
        };
        // A halo orbit sample about Earth–Moon L2, in the co-rotating frame.
        let halo = Pos::<Synodic>::in_pair(em, DVec3::new(1.0e7, 0.0, 5.0e6));
        assert_eq!(halo.pair(), em);

        let l2 = Center::Libration {
            primary: 399,
            secondary: 301,
            point: LPoint::L2,
        };
        assert_ne!(
            l2,
            Center::Libration {
                primary: 399,
                secondary: 301,
                point: LPoint::L1
            },
            "L1 and L2 are different centres of the same frame"
        );

        // A Sun–Earth L2 halo is a different frame entirely, and says so.
        let se = Pair {
            primary: 10,
            secondary: 399,
        };
        assert_ne!(halo.pair(), se);
    }

    // And the things that CANNOT be written — the whole point of the module:
    //
    //     coords::ecliptic_to_bevy(IcrfAu::new(v));   // ← does not compile (incident 1)
    //     let sun: Pos<SiteEnu> = solar_direction;    // ← does not compile (incident 2)
    //
    // Both were shipped bugs. Neither is expressible now.
}
