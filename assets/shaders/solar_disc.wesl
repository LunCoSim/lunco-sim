// Visible area of a uniform circular emitter above a locally straight skyline.
fn disc_above_skyline(elevation: f32, radius: f32, skyline: f32) -> f32 {
    let clearance = elevation - skyline;
    if (clearance <= -radius) { return 0.0; }
    if (clearance >= radius) { return 1.0; }
    let x = clamp(clearance / radius, -1.0, 1.0);
    return (acos(-x) + x * sqrt(max(0.0, 1.0 - x*x))) / 3.141592653589793;
}
