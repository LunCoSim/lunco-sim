/// Converts J2000 Ecliptic coordinates (AU) to Bevy Meters (approx 1.5e11 scale),
/// applying the obliquity rotation (23.44°) to reach J2000 Equatorial J-up frame.
pub fn ecliptic_to_bevy(pos: bevy::math::DVec3) -> bevy::math::DVec3 {
    let au_to_m = 1.495_978_707e11;
    let pos_m = pos * au_to_m;
    
    // Obliquity of the Ecliptic (J2000)
    let epsilon = (23.439281f64).to_radians();
    let (sin_e, cos_e) = epsilon.sin_cos();
    
    // Rotate around X axis: Ecliptic (x, y, z) -> Equatorial (x', y', z')
    let x = pos_m.x;
    let y = pos_m.y * cos_e - pos_m.z * sin_e;
    let z = pos_m.y * sin_e + pos_m.z * cos_e;
    
    // Map to Bevy Y-up axes: 
    // Bevy X = Eq X
    // Bevy Y = Eq Z (North Pole)
    // Bevy Z = -Eq Y 
    // (This is a standard right-handed mapping where Y is Up)
    bevy::math::DVec3::new(x, z, -y)
}

pub use lunco_core::coords::get_absolute_pos_in_root_double_ghost_aware;
