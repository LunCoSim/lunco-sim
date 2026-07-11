//! Deterministic placement sampling.
//!
//! Everything here is pure CPU and reproducible from a `u64` seed (ChaCha8) — no
//! ECS, no assets, no entropy source. That is what lets the planner run on a
//! background task and lets every networked client regenerate an identical field
//! from the replicated spec alone.

use bevy::math::Vec2;
use rand::seq::SliceRandom;
use rand::RngExt;
use rand_chacha::ChaCha8Rng;
use rand_chacha::rand_core::SeedableRng;

use crate::spec::{Pattern, SizeDist};

/// One placed object in world XZ (ground height resolved later from the heightfield).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    /// World position on the XZ plane (metres).
    pub pos: Vec2,
    /// Characteristic size (radius) in metres.
    pub size: f32,
    /// Yaw rotation in radians.
    pub yaw: f32,
    /// Whether this instance should be an interactive (dynamic) body.
    pub dynamic: bool,
}

/// Per-layer salt mixed into the master seed so two layers with the same spec
/// don't produce correlated point sets.
pub mod salt {
    pub const CRATERS: u64 = 0x1111_2222_3333_4444;
    pub const ROCKS: u64 = 0x5555_6666_7777_8888;
}

fn rng_for(seed: u64, salt: u64) -> ChaCha8Rng {
    ChaCha8Rng::seed_from_u64(seed ^ salt)
}

/// Standard-normal sample via Box–Muller (avoids a rand_distr dependency).
fn gaussian(rng: &mut ChaCha8Rng) -> f32 {
    // u1 in (0,1] to keep ln() finite.
    let u1: f32 = 1.0 - rng.random::<f32>();
    let u2: f32 = rng.random::<f32>();
    (-2.0 * u1.ln()).sqrt() * (std::f32::consts::TAU * u2).cos()
}

impl SizeDist {
    /// Draw a size, log-normally distributed around `mode`, clamped to `[min, max]`.
    pub fn sample(&self, rng: &mut ChaCha8Rng) -> f32 {
        if self.sigma <= 0.0 || self.mode <= 0.0 {
            return self.mode.clamp(self.min, self.max);
        }
        // Log-normal with the given mode: mode = exp(mu - sigma^2) → mu = ln(mode) + sigma^2.
        let mu = self.mode.ln() + self.sigma * self.sigma;
        let v = (mu + self.sigma * gaussian(rng)).exp();
        v.clamp(self.min, self.max)
    }
}

/// Generate placements for one layer.
///
/// `count` is the requested object count; PoissonDisk treats it as an upper
/// bound (the region may not fit that many at `min_spacing`).
pub fn sample_layer(
    seed: u64,
    salt: u64,
    pattern: Pattern,
    half_extent: f32,
    count: usize,
    size: SizeDist,
    dynamic_fraction: f32,
) -> Vec<Placement> {
    let mut rng = rng_for(seed, salt);
    let positions = match pattern {
        Pattern::Uniform => uniform_positions(&mut rng, half_extent, count),
        Pattern::PoissonDisk { min_spacing } => {
            // Fill the whole region (blue noise), then shuffle before truncating
            // so a requested count below capacity is spread evenly rather than
            // clumped near Bridson's growth seed. Any subset stays ≥ min_spacing.
            let mut p = poisson_positions(&mut rng, half_extent, min_spacing, usize::MAX);
            p.shuffle(&mut rng);
            p.truncate(count);
            p
        }
        Pattern::Clustered { clusters, spread } => {
            clustered_positions(&mut rng, half_extent, count, clusters.max(1), spread)
        }
    };

    positions
        .into_iter()
        .map(|pos| Placement {
            pos,
            size: size.sample(&mut rng),
            yaw: rng.random::<f32>() * std::f32::consts::TAU,
            dynamic: rng.random::<f32>() < dynamic_fraction,
        })
        .collect()
}

fn uniform_positions(rng: &mut ChaCha8Rng, h: f32, count: usize) -> Vec<Vec2> {
    (0..count)
        .map(|_| Vec2::new(rng.random_range(-h..=h), rng.random_range(-h..=h)))
        .collect()
}

fn clustered_positions(
    rng: &mut ChaCha8Rng,
    h: f32,
    count: usize,
    clusters: u32,
    spread: f32,
) -> Vec<Vec2> {
    let centers: Vec<Vec2> = (0..clusters)
        .map(|_| Vec2::new(rng.random_range(-h..=h), rng.random_range(-h..=h)))
        .collect();
    (0..count)
        .map(|i| {
            let c = centers[i % centers.len()];
            let off = Vec2::new(gaussian(rng), gaussian(rng)) * spread;
            (c + off).clamp(Vec2::splat(-h), Vec2::splat(h))
        })
        .collect()
}

/// Bridson Poisson-disk sampling over the square `[-h, h]²` with `min_spacing`
/// (blue noise — evenly spaced, no clumping). `cap` bounds output size.
fn poisson_positions(rng: &mut ChaCha8Rng, h: f32, min_spacing: f32, cap: usize) -> Vec<Vec2> {
    let r = min_spacing.max(0.01);
    let size = 2.0 * h;
    let cell = r / std::f32::consts::SQRT_2;
    let cols = (size / cell).ceil() as usize + 1;
    let rows = cols;
    let mut grid: Vec<i32> = vec![-1; cols * rows];
    let mut points: Vec<Vec2> = Vec::new();
    let mut active: Vec<usize> = Vec::new();

    let to_grid = |p: Vec2| -> (usize, usize) {
        let gx = (((p.x + h) / cell).floor() as usize).min(cols - 1);
        let gy = (((p.y + h) / cell).floor() as usize).min(rows - 1);
        (gx, gy)
    };

    // Seed with one random point.
    let first = Vec2::new(rng.random_range(-h..=h), rng.random_range(-h..=h));
    let (gx, gy) = to_grid(first);
    grid[gy * cols + gx] = 0;
    points.push(first);
    active.push(0);

    const K: usize = 30;
    while let Some(&active_idx) = active.last() {
        let origin = points[active_idx];
        let mut placed = false;
        for _ in 0..K {
            let ang = rng.random::<f32>() * std::f32::consts::TAU;
            let rad = rng.random_range(r..=2.0 * r);
            let cand = origin + Vec2::new(ang.cos(), ang.sin()) * rad;
            if cand.x < -h || cand.x > h || cand.y < -h || cand.y > h {
                continue;
            }
            let (cgx, cgy) = to_grid(cand);
            // Check the 5x5 neighbourhood for a too-close existing point.
            let mut ok = true;
            'scan: for dy in -2i32..=2 {
                for dx in -2i32..=2 {
                    let nx = cgx as i32 + dx;
                    let ny = cgy as i32 + dy;
                    if nx < 0 || ny < 0 || nx as usize >= cols || ny as usize >= rows {
                        continue;
                    }
                    let gi = grid[ny as usize * cols + nx as usize];
                    if gi >= 0 && points[gi as usize].distance(cand) < r {
                        ok = false;
                        break 'scan;
                    }
                }
            }
            if ok {
                grid[cgy * cols + cgx] = points.len() as i32;
                points.push(cand);
                active.push(points.len() - 1);
                placed = true;
                if points.len() >= cap {
                    return points;
                }
                break;
            }
        }
        if !placed {
            active.pop();
        }
    }
    points
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec_size() -> SizeDist {
        SizeDist::new(0.5, 1.0, 4.0, 0.5)
    }

    #[test]
    fn deterministic_same_seed() {
        let a = sample_layer(42, salt::ROCKS, Pattern::Uniform, 50.0, 100, spec_size(), 0.1);
        let b = sample_layer(42, salt::ROCKS, Pattern::Uniform, 50.0, 100, spec_size(), 0.1);
        assert_eq!(a, b, "same seed must produce identical placements");
    }

    #[test]
    fn different_seed_differs() {
        let a = sample_layer(1, salt::ROCKS, Pattern::Uniform, 50.0, 100, spec_size(), 0.1);
        let b = sample_layer(2, salt::ROCKS, Pattern::Uniform, 50.0, 100, spec_size(), 0.1);
        assert_ne!(a, b);
    }

    #[test]
    fn sizes_in_bounds() {
        let p = sample_layer(7, salt::ROCKS, Pattern::Uniform, 50.0, 500, spec_size(), 0.0);
        for pl in &p {
            assert!(pl.size >= 0.5 && pl.size <= 4.0, "size {} out of bounds", pl.size);
            assert!(pl.pos.x.abs() <= 50.0 && pl.pos.y.abs() <= 50.0);
        }
    }

    #[test]
    fn poisson_respects_spacing() {
        let p = poisson_positions(&mut rng_for(9, salt::ROCKS), 30.0, 4.0, 10_000);
        for i in 0..p.len() {
            for j in (i + 1)..p.len() {
                assert!(p[i].distance(p[j]) >= 4.0 - 1e-3, "points too close");
            }
        }
        assert!(p.len() > 10, "poisson should fill the region");
    }

    #[test]
    fn dynamic_fraction_roughly_honoured() {
        let p = sample_layer(3, salt::ROCKS, Pattern::Uniform, 50.0, 1000, spec_size(), 0.25);
        let dyn_count = p.iter().filter(|p| p.dynamic).count();
        let frac = dyn_count as f32 / p.len() as f32;
        assert!((frac - 0.25).abs() < 0.05, "dynamic fraction {frac} off");
    }
}
