use super::*;
fn sample() -> Soil {
    Soil {
        kc: 0.0,
        kphi: 100_000.0,
        n: 1.0,
        cohesion: 100.0,
        phi: 0.5,
        shear_length: 0.02,
    }
}
#[test]
fn plate_reference() {
    let s = sample();
    assert!((pressure(s, 0.2, 0.01).unwrap() - 1000.0).abs() < 1e-9);
    assert!(pressure(s, 0.0, 0.01).is_err());
}
#[test]
fn wheel_load_matches_closed_form() {
    let s = sample();
    let r: f64 = 0.3;
    let z: f64 = 0.03;
    let b = 0.2;
    let a = (2.0 * r * z - z * z).sqrt();
    let exact = b * s.kphi * (r * r * (a / r).asin() - (r - z) * a);
    let got = wheel(s, r, b, z, 0.0, 4096).unwrap();
    assert!((got.0 - exact).abs() / exact < 1e-6);
    assert_eq!(got.1, 0.0);
}
#[test]
fn uniform_patch_shear_matches_closed_form() {
    let s = sample();
    let length = 0.1;
    let slip = 0.2;
    let p = 1000.0;
    let cap = s.cohesion + p * s.phi.tan();
    let exact =
        cap * (length - s.shear_length / slip * (1.0 - (-slip * length / s.shear_length).exp()));
    let got: f64 = (0..4096)
        .map(|i| shear(s, p, slip * length * (i as f64 + 0.5) / 4096.0).unwrap() * length / 4096.0)
        .sum();
    assert!((got - exact).abs() / exact < 1e-7);
}
#[test]
fn load_equilibrium_and_grid_convergence() {
    let s = sample();
    let z = equilibrium(s, 0.3, 0.2, 100.0, 256).unwrap();
    assert!((wheel(s, 0.3, 0.2, z, 0.0, 256).unwrap().0 - 100.0).abs() < 1e-8);
    let a = equilibrium(s, 0.3, 0.2, 100.0, 64).unwrap();
    let b = equilibrium(s, 0.3, 0.2, 100.0, 128).unwrap();
    let c = equilibrium(s, 0.3, 0.2, 100.0, 4096).unwrap();
    assert!((b - c).abs() < (a - c).abs());
}
#[test]
fn bounds_symmetry_and_rejection() {
    let s = sample();
    let (n, t) = wheel(s, 0.3, 0.2, 0.02, 0.2, 256).unwrap();
    let (_, reverse) = wheel(s, 0.3, 0.2, 0.02, -0.2, 256).unwrap();
    assert!((t + reverse).abs() < 1e-10);
    let a = (2.0 * 0.3 * 0.02 - 0.02_f64.powi(2)).sqrt();
    assert!(t > 0.0 && t <= 2.0 * a * 0.2 * s.cohesion + n * s.phi.tan());
    assert!(wheel(s, 0.3, 0.2, 0.2, 0.1, 256).is_err());
    assert!(wheel(s, 0.3, 0.2, 0.02, 0.1, 0).is_err());
    assert!(equilibrium(s, 0.3, 0.2, 1e9, 256).is_err());
    assert!(pressure(Soil { n: f64::NAN, ..s }, 0.2, 0.01).is_err());
}
