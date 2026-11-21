//! Isolated quasi-static quadrature experiment, not production soil physics.
use std::{env, fs, process::ExitCode, time::Instant};
#[derive(Clone, Copy)]
struct Soil {
    kc: f64,
    kphi: f64,
    n: f64,
    cohesion: f64,
    phi: f64,
    shear_length: f64,
}
const MAX_SINKAGE_RATIO: f64 = 0.2;
const MAX_SLIP: f64 = 0.3;
fn valid(s: Soil) -> Result<(), String> {
    if ![s.kc, s.kphi, s.n, s.cohesion, s.phi, s.shear_length]
        .iter()
        .all(|x| x.is_finite())
        || s.kc < 0.0
        || s.kphi < 0.0
        || s.kc + s.kphi <= 0.0
        || s.n <= 0.0
        || s.cohesion < 0.0
        || !(0.0..std::f64::consts::FRAC_PI_2).contains(&s.phi)
        || s.shear_length <= 0.0
    {
        return Err("invalid soil parameters".into());
    }
    Ok(())
}
fn finite(x: f64) -> Result<f64, String> {
    if x.is_finite() {
        Ok(x)
    } else {
        Err("numerical overflow".into())
    }
}
fn pressure(s: Soil, b: f64, z: f64) -> Result<f64, String> {
    valid(s)?;
    if !b.is_finite() || b <= 0.0 || !z.is_finite() || z < 0.0 {
        return Err("invalid width/depth".into());
    }
    finite((s.kc / b + s.kphi) * z.powf(s.n))
}
fn shear(s: Soil, p: f64, j: f64) -> Result<f64, String> {
    valid(s)?;
    if !p.is_finite() || p < 0.0 || !j.is_finite() {
        return Err("invalid pressure/displacement".into());
    }
    finite((s.cohesion + p * s.phi.tan()) * (-(-j.abs() / s.shear_length).exp_m1()) * j.signum())
}
fn wheel(s: Soil, r: f64, b: f64, z: f64, slip: f64, cells: usize) -> Result<(f64, f64), String> {
    valid(s)?;
    if !r.is_finite()
        || r <= 0.0
        || !b.is_finite()
        || b <= 0.0
        || !z.is_finite()
        || z < 0.0
        || z > MAX_SINKAGE_RATIO * r
        || !slip.is_finite()
        || slip.abs() > MAX_SLIP
        || !(1..=1_000_000).contains(&cells)
    {
        return Err("wheel case outside declared domain".into());
    }
    let a = finite((z * (2.0 * r - z)).sqrt())?;
    if z == 0.0 {
        return Ok((0.0, 0.0));
    }
    let dx = 2.0 * a / cells as f64;
    let (mut normal, mut traction) = (0.0, 0.0);
    for i in 0..cells {
        let x = -a + (i as f64 + 0.5) * dx;
        // Symmetric circular indentation; vertical foundation stress on projected area.
        let depth = z - r + (r * r - x * x).sqrt();
        let p = pressure(s, b, depth.max(0.0))?;
        // Prescribed displacement proxy, not full rolling-wheel kinematics.
        let j = slip * (a - x);
        normal += p * b * dx;
        traction += shear(s, p, j)? * b * dx;
    }
    Ok((finite(normal)?, finite(traction)?))
}
fn equilibrium(s: Soil, r: f64, b: f64, load: f64, cells: usize) -> Result<f64, String> {
    if !load.is_finite() || load <= 0.0 {
        return Err("load must be finite and positive".into());
    }
    let (mut lo, mut hi) = (0.0, MAX_SINKAGE_RATIO * r);
    if wheel(s, r, b, hi, 0.0, cells)?.0 < load {
        return Err("load exceeds shallow-sinkage domain".into());
    }
    for _ in 0..64 {
        let mid = (lo + hi) * 0.5;
        if wheel(s, r, b, mid, 0.0, cells)?.0 < load {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let z = (lo + hi) * 0.5;
    if (wheel(s, r, b, z, 0.0, cells)?.0 - load).abs() > 1e-9 * load {
        return Err("equilibrium residual exceeds tolerance".into());
    }
    Ok(z)
}
fn run() -> Result<(), String> {
    let args: Vec<_> = env::args().collect();
    if args.len() != 2 {
        return Err("usage: soil-spike <cases.csv>".into());
    }
    let input = fs::read_to_string(&args[1]).map_err(|e| e.to_string())?;
    let mut lines = input.lines();
    if lines.next()
        != Some("radius_m,width_m,load_n,slip,kc,kphi,n,cohesion_pa,phi_rad,shear_length_m,cells")
    {
        return Err("unexpected CSV header".into());
    }
    let start = Instant::now();
    let mut results = Vec::new();
    for (row, line) in lines.enumerate() {
        let values: Vec<_> = line.split(',').collect();
        if values.len() != 11 {
            return Err(format!("row {}: expected 11 fields", row + 2));
        }
        let v: Vec<f64> = values[..10]
            .iter()
            .map(|x| x.parse::<f64>().map_err(|e| e.to_string()))
            .collect::<Result<_, _>>()?;
        let cells = values[10].parse::<usize>().map_err(|e| e.to_string())?;
        let s = Soil {
            kc: v[4],
            kphi: v[5],
            n: v[6],
            cohesion: v[7],
            phi: v[8],
            shear_length: v[9],
        };
        let z =
            equilibrium(s, v[0], v[1], v[2], cells).map_err(|e| format!("row {}: {e}", row + 2))?;
        let (normal, traction) = wheel(s, v[0], v[1], z, v[3], cells)?;
        // The n=1 closed form is independent of midpoint quadrature.
        let analytic = if s.n == 1.0 {
            let a = (2.0 * v[0] * z - z * z).sqrt();
            Some(v[1] * (s.kc / v[1] + s.kphi) * (v[0] * v[0] * (a / v[0]).asin() - (v[0] - z) * a))
        } else {
            None
        };
        results.push(format!(
            "{},{},{},{},{:.12},{:.12},{:.12},{:.12},{}",
            row + 1,
            v[2],
            v[3],
            cells,
            z,
            normal,
            traction,
            v[2] * s.phi.tan(),
            analytic
                .map(|a| format!("{:.12}", (normal - a).abs() / v[2]))
                .unwrap_or_default()
        ));
    }
    if results.is_empty() {
        return Err("no experiment cases".into());
    }
    eprintln!(
        "cases={} elapsed_ms={:.3}; quasi-static kernel only",
        results.len(),
        start.elapsed().as_secs_f64() * 1000.0
    );
    println!(
        "case,load_n,slip,cells,sinkage_m,normal_n,gross_shear_n,cohesionless_coulomb_bound_n,relative_n1_load_error"
    );
    for result in results {
        println!("{result}");
    }
    Ok(())
}
fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}
#[cfg(test)]
mod tests;
