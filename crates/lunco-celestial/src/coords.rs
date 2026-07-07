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
/// vectors (see `lunco-celestial-ephemeris::equatorial_to_ecliptic`), and the
/// ecliptic frame keeps the Moon's polar axis within 1.5° of +Y — which is
/// what the geodesy (`geo.rs`) and the registry's default `polar_axis = +Y`
/// assume. Earth's real tilted axis is carried per-body in the registry and
/// honored by `geo::body_rotation`.
pub(crate) fn ecliptic_to_bevy(pos: bevy::math::DVec3) -> bevy::math::DVec3 {
    let au_to_m = 1.495_978_707e11;
    let pos_m = pos * au_to_m;
    bevy::math::DVec3::new(pos_m.x, pos_m.z, -pos_m.y)
}

pub(crate) use lunco_core::coords::world_position_seeded;
