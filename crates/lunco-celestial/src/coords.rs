/// Converts J2000 **Ecliptic** coordinates (AU) to Bevy meters (~1.5e11 scale).
///
/// Pure axis swap — NO obliquity rotation. The solar/world frame is ecliptic
/// J2000 with Bevy Y = ecliptic north:
///   Bevy X = Ecl X (vernal equinox) · Bevy Y = Ecl Z (ecliptic north) ·
///   Bevy Z = −Ecl Y  (right-handed, Y-up)
///
/// History: this function used to ALSO rotate by the obliquity (making the
/// world frame equatorial), which compensated the ephemeris provider's
/// mislabeled equatorial output. The provider now returns true ecliptic
/// vectors (see `lunco-celestial-ephemeris::equatorial_to_ecliptic`).
///
/// Body ORIENTATION enters the same frame by the same two steps, in
/// `iau::icrf_to_bevy` — the IAU/WGCCRE poles and prime meridians are published
/// in the ICRF, and that function is the one explicit place they cross into
/// this frame. Positions and orientations therefore share one obliquity
/// constant; if they ever disagree, every "north" in the sim is wrong by 23.4°
/// (which has happened here before).
/// **The one and only way to get a [`SolarM`] out of ephemeris data.**
///
/// It takes an [`EclipticAu`], so a raw `DVec3` — or, critically, an [`IcrfAu`] fresh out of
/// VSOP/ELP — cannot be handed to it. That is what makes the Shackleton incident (equatorial
/// vectors fed to ecliptic geodesy, sun 45° below the horizon) a compile error rather than a
/// dark planet.
///
/// `pub` on purpose: `lunco-celestial-ephemeris` re-implemented this by hand in its own tests
/// because it was `pub(crate)`. A conversion that people have to copy is a conversion that
/// will drift.
pub fn ecliptic_to_bevy(pos: crate::frames::EclipticAu) -> crate::frames::SolarM {
    let pos_m = pos.raw() * AU_TO_M;
    crate::frames::SolarM::new(bevy::math::DVec3::new(pos_m.x, pos_m.z, -pos_m.y))
}

/// Metres in an astronomical unit. One definition, used by both this crate and the ephemeris
/// provider — the constant was previously written out three times.
pub const AU_TO_M: f64 = 1.495_978_707e11;

/// Kilometres in an astronomical unit.
pub const AU_KM: f64 = 149_597_870.7;

pub(crate) use lunco_core::coords::world_position_seeded;
