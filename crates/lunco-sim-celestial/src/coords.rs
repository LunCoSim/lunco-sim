use bevy::math::DVec3;

/// Obliquity of the ecliptic (J2000), radians
pub const OBLIQUITY: f64 = 0.409_092_623_364_f64; // 23.4392911 degrees in radians
const AU_TO_M: f64 = 149_597_870_700.0;

/// Convert ecliptic J2000 (AU) → Bevy world space (meters, Y-up)
pub fn ecliptic_to_bevy(pos_au: DVec3) -> DVec3 {
    let obliquity = 23.44f64.to_radians();
    let cos_o = obliquity.cos();
    let sin_o = obliquity.sin();
    
    // Ecliptic to Equatorial
    let equatorial = DVec3::new(
        pos_au.x,
        pos_au.y * cos_o - pos_au.z * sin_o,
        pos_au.y * sin_o + pos_au.z * cos_o,
    );
    
    // Equatorial (X: Equinox, Y: Ecliptic Projection, Z: North Pole)
    // To Bevy (X: Right, Y: Up, Z: Back)
    // Map: Eq X -> B X, Eq Y -> B -Z (Forward), Eq Z -> B Y
    DVec3::new(
        equatorial.x * AU_TO_M, 
        equatorial.z * AU_TO_M, 
        -equatorial.y * AU_TO_M
    )
}
