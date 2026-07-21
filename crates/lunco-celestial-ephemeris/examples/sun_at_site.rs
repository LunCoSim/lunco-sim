//! Where is the Sun (and Earth) in a site's sky, at a given epoch?
//!
//! A surface scene that references `solar_system.usda` gets its key light aimed
//! by the EPHEMERIS (`update_sun_light_system`), not by the authored
//! `xformOp:rotateXYZ` on its `DistantLight`. So a scene whose lighting is part
//! of the teaching — a low sun that throws a crater floor into shadow, a survey
//! whose shadow lengths were measured in QGIS — cannot just declare a sky: it
//! must also declare the DATE at which the real sun stands where the survey
//! says it does (`double lunco:time:epochJd` on the site-anchor prim).
//!
//! This tool finds that date. It runs the same math the renderer runs — the
//! IAU body rotation, the ENU tangent frame at the site, `ecliptic_to_bevy` —
//! so a match here is a match in the app.
//!
//! ```text
//! cargo run -p lunco-celestial --example sun_at_site -- <lat> <lon> [jd0] [days] [step_days]
//! cargo run -p lunco-celestial --example sun_at_site -- 26.0371 3.6584 --solve 110 8
//! ```
//!
//! Azimuth is degrees CLOCKWISE FROM NORTH (0 = N, 90 = E), matching
//! `lunco:sun:azimuthDeg` and `lunco_environment::solar`.

use bevy::math::DVec3;
use lunco_celestial::ephemeris::EphemerisProvider;
use lunco_celestial::geo::{solar_tangent_frame, Geodetic};
use lunco_celestial::registry::CelestialBodyRegistry;
use lunco_celestial_ephemeris::CelestialEphemerisProvider;

const MOON: i32 = 301;
const SUN: i32 = 10;
const EARTH: i32 = 399;

/// Azimuth (deg clockwise from north) and elevation (deg above horizon) of
/// `target` as seen from the geodetic point `geo` on `body`, at `jd`.
fn az_el(
    provider: &CelestialEphemerisProvider,
    registry: &CelestialBodyRegistry,
    body: i32,
    geo: &Geodetic,
    target: i32,
    jd: f64,
) -> Option<(f64, f64)> {
    let desc = registry.bodies.iter().find(|b| b.ephemeris_id == body)?;
    let p_body = lunco_celestial::coords::ecliptic_to_bevy(provider.global_position(body, jd)?).raw();
    let p_target =
        lunco_celestial::coords::ecliptic_to_bevy(provider.global_position(target, jd)?).raw();
    let frame = solar_tangent_frame(desc, geo, p_body, jd);
    // From the SITE, not the body centre: at Earth/Sun range the difference is
    // negligible, but the site offset is what makes a horizon-grazing polar sun
    // read correctly, and it costs nothing.
    let to_target: DVec3 = (p_target - frame.origin).normalize();
    let e = to_target.dot(frame.east);
    let n = to_target.dot(frame.north);
    let u = to_target.dot(frame.up);
    Some((e.atan2(n).to_degrees().rem_euclid(360.0), u.asin().to_degrees()))
}

/// Circular difference in degrees, in [-180, 180].
fn ang_diff(a: f64, b: f64) -> f64 {
    let d = (a - b).rem_euclid(360.0);
    if d > 180.0 {
        d - 360.0
    } else {
        d
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!(
            "usage: sun_at_site <lat> <lon> [jd0] [days] [step_days]\n       \
             sun_at_site <lat> <lon> --solve <azDeg> <elDeg> [jd0] [days]"
        );
        std::process::exit(2);
    }
    let lat: f64 = args[0].parse().expect("lat");
    let lon: f64 = args[1].parse().expect("lon");
    let geo = Geodetic::new(lat, lon, 0.0);

    let provider = CelestialEphemerisProvider::new();
    let registry = CelestialBodyRegistry::default_system();

    let solve = args.get(2).is_some_and(|a| a == "--solve");
    // J2000 + a quarter century — a date inside every shipped ephemeris table.
    let default_jd0 = 2_461_000.5; // 2025-11-09 TDB
    if solve {
        let want_az: f64 = args[3].parse().expect("azDeg");
        let want_el: f64 = args[4].parse().expect("elDeg");
        let jd0: f64 = args.get(5).and_then(|s| s.parse().ok()).unwrap_or(default_jd0);
        let days: f64 = args.get(6).and_then(|s| s.parse().ok()).unwrap_or(60.0);

        // Coarse sweep for the best hour, then bisect-free refine by minute.
        let mut best = (f64::MAX, jd0, 0.0, 0.0);
        let mut jd = jd0;
        while jd < jd0 + days {
            if let Some((az, el)) = az_el(&provider, &registry, MOON, &geo, SUN, jd) {
                let cost = ang_diff(az, want_az).powi(2) + (el - want_el).powi(2);
                if cost < best.0 {
                    best = (cost, jd, az, el);
                }
            }
            jd += 1.0 / 24.0;
        }
        let coarse = best.1;
        let mut jd = coarse - 1.0 / 24.0;
        while jd < coarse + 1.0 / 24.0 {
            if let Some((az, el)) = az_el(&provider, &registry, MOON, &geo, SUN, jd) {
                let cost = ang_diff(az, want_az).powi(2) + (el - want_el).powi(2);
                if cost < best.0 {
                    best = (cost, jd, az, el);
                }
            }
            jd += 1.0 / 1440.0;
        }
        let (_, jd, az, el) = best;
        let earth = az_el(&provider, &registry, MOON, &geo, EARTH, jd);
        println!("site {lat:.4} {lon:.4}  want sun az {want_az:.1} el {want_el:.1}");
        println!("  best JD {jd:.5}   sun az {az:.2}  el {el:.2}");
        match earth {
            Some((eaz, eel)) => println!("  earth at that epoch: az {eaz:.2}  el {eel:.2}"),
            None => println!("  earth: no ephemeris"),
        }
        println!("\n  custom double lunco:time:epochJd = {jd:.5}");
        return;
    }

    let jd0: f64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(default_jd0);
    let days: f64 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(29.5);
    let step: f64 = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(0.5);

    println!("site {lat:.4} {lon:.4}   (az = deg clockwise from north)");
    println!("{:>12}  {:>8} {:>8}   {:>8} {:>8}", "JD", "sun_az", "sun_el", "earth_az", "earth_el");
    let mut jd = jd0;
    while jd < jd0 + days {
        let s = az_el(&provider, &registry, MOON, &geo, SUN, jd);
        let e = az_el(&provider, &registry, MOON, &geo, EARTH, jd);
        match (s, e) {
            (Some((saz, sel)), Some((eaz, eel))) => println!(
                "{jd:>12.4}  {saz:>8.2} {sel:>8.2}   {eaz:>8.2} {eel:>8.2}"
            ),
            _ => println!("{jd:>12.4}  (no ephemeris)"),
        }
        jd += step;
    }
}
