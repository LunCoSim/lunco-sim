//! Diagnostic for the reported cubemap FACE-SEAM artefact.
//!
//! Symptom: a sky containing a large, smooth, LOW-AMPLITUDE gradient (a Milky
//! Way band) rendered with straight-edged rectangular brightness terraces whose
//! edges lay on cubemap face boundaries. Point stars showed no such artefact.
//!
//! The projection is a pure function, so the claim is directly testable: sample
//! a smooth analytic sky, project it, and compare texels that sit on either side
//! of a face boundary but point in ALMOST THE SAME DIRECTION. If the projection
//! is continuous, those must agree to within the source's own gradient over that
//! tiny angular step. A terrace would show up as a large jump.
//!
//! Run: cargo test -p lunco-usd-bevy --test dome_projection_continuity

use bevy::color::LinearRgba;
use bevy::math::Vec3;
use lunco_usd_bevy::dome::{equirect_to_cubemap, Equirect};

/// The analytic sky under test: a broad, smooth, low-amplitude band around a
/// tilted great circle — the Milky Way's shape, and the exact signal that
/// triggered the artefact. Deliberately has NO high-frequency content, so any
/// discontinuity in the output is the projection's doing, not aliasing.
fn band_at(d: Vec3) -> f32 {
    let n = Vec3::new(0.0, (62.0f32).to_radians().cos(), (62.0f32).to_radians().sin());
    let s = d.normalize().dot(n);
    0.05 * (-(s / 0.13).powi(2)).exp()
}

fn make_equirect(w: u32, h: u32) -> Equirect {
    let mut texels = Vec::with_capacity((w * h) as usize);
    for y in 0..h {
        // Match `sample_dir`'s convention: v = acos(y)/PI, u = 0.5 + atan2(x,-z)/TAU
        let v = (y as f32 + 0.5) / h as f32;
        let theta = v * std::f32::consts::PI;
        for x in 0..w {
            let u = (x as f32 + 0.5) / w as f32;
            let phi = (u - 0.5) * std::f32::consts::TAU;
            let dir = Vec3::new(theta.sin() * phi.sin(), theta.cos(), -theta.sin() * phi.cos());
            let b = band_at(dir);
            texels.push([b, b, b, 1.0]);
        }
    }
    Equirect::from_texels(w, h, texels)
}

/// Read face texel as f32 out of the Rgba16Float cubemap blob.
fn texel(data: &[u8], size: u32, face: usize, px: u32, py: u32) -> f32 {
    let idx = ((face as u32 * size * size) + py * size + px) as usize * 4 * 2;
    half::f16::from_le_bytes([data[idx], data[idx + 1]]).to_f32()
}

fn face_dir_at(face: usize, px: u32, py: u32, size: u32) -> Vec3 {
    let a = 2.0 * (px as f32 + 0.5) / size as f32 - 1.0;
    let b = 2.0 * (py as f32 + 0.5) / size as f32 - 1.0;
    match face {
        0 => Vec3::new(1.0, -b, -a),
        1 => Vec3::new(-1.0, -b, a),
        2 => Vec3::new(a, 1.0, b),
        3 => Vec3::new(a, -1.0, -b),
        4 => Vec3::new(a, -b, 1.0),
        _ => Vec3::new(-a, -b, -1.0),
    }
    .normalize()
}

#[test]
fn projection_matches_the_analytic_sky_everywhere() {
    let src = make_equirect(4096, 2048);
    let size = 256;
    let img = equirect_to_cubemap(&src, size, LinearRgba::WHITE);
    let data = img.data.as_ref().expect("cubemap has CPU data");

    // Compare EVERY face texel against the analytic value in its own direction.
    // This localises the fault: if the projection is sound, error is bounded by
    // the source's bilinear reconstruction error, which for this band over a
    // 4096x2048 source is tiny (the band varies over tens of degrees).
    let mut worst = 0.0f32;
    let mut worst_at = (0, 0, 0);
    for face in 0..6 {
        for py in 0..size {
            for px in 0..size {
                let got = texel(data, size, face, px, py);
                let want = band_at(face_dir_at(face, px, py, size));
                let err = (got - want).abs();
                if err > worst {
                    worst = err;
                    worst_at = (face, px, py);
                }
            }
        }
    }
    println!("worst abs error {worst:.6} at face/px/py {worst_at:?} (band peak 0.05)");
    // 2% of the band's peak amplitude. A visible terrace is far larger than this.
    assert!(worst < 0.001, "projection deviates from the analytic sky by {worst}");
}

#[test]
fn adjacent_faces_agree_across_their_shared_edge() {
    let src = make_equirect(4096, 2048);
    let size = 256;
    let img = equirect_to_cubemap(&src, size, LinearRgba::WHITE);
    let data = img.data.as_ref().expect("cubemap has CPU data");

    // Walk the +X / +Z shared edge. The last column of +Z and the first column
    // of +X straddle it; their directions differ by one texel of arc, so their
    // values must too. A face-boundary terrace shows up here immediately.
    let mut worst = 0.0f32;
    for py in 0..size {
        let a = texel(data, size, 4, size - 1, py); // +Z, last column
        let b = texel(data, size, 0, 0, py); // +X, first column
        worst = worst.max((a - b).abs());
    }
    println!("worst +Z/+X seam delta {worst:.6} (band peak 0.05)");
    assert!(worst < 0.001, "face seam discontinuity of {worst}");
}
