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
pub(crate) fn ecliptic_to_bevy(pos: bevy::math::DVec3) -> bevy::math::DVec3 {
    let au_to_m = 1.495_978_707e11;
    let pos_m = pos * au_to_m;
    bevy::math::DVec3::new(pos_m.x, pos_m.z, -pos_m.y)
}

pub(crate) use lunco_core::coords::world_position_seeded;
