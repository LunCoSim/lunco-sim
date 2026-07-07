//! Built-in **craters** layer.
//!
//! A crater layer contributes an **analytic** [`Craters`] height modifier to the
//! terrain's [`SurfaceOracle`](crate::oracle::SurfaceOracle) — craters as *math you
//! sample*, not pixels you stamp. The CDLOD tile baker and the collider ring both
//! sample the ONE composed source at their own resolution, so:
//!
//! - a rim resolves as sharply as the nearest tile tessellates (sub-metre near the
//!   camera), unbounded by the DEM grid spacing;
//! - the rover drives exactly the bowl it sees — visuals and contact converge by
//!   construction;
//! - craters follow the surrounding relief because they are deltas ON it.
//!
//! History: v1 rasterised bowls into the working `HeightGrid` (rims bounded by grid
//! resolution — the blocky "staircase" rims) after v0's separate floating overlay
//! mesh caused the pedestal/mismatch bugs. The analytic modifier is the design the
//! whole oracle substrate was built for; see `docs/architecture/terrain-substrate.md`.
//!
//! Placement is the deterministic complete-spatial-randomness set from
//! [`crate::terrain::crater_placements`] — identical on every peer from the seed.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

use lunco_obstacle_field::spec::{CraterLayer, SizeDist};
use lunco_terrain_core::{Crater, Craters};

use super::{LayerAttrSource, TerrainLayer};
use crate::oracle::HeightContribution;

/// Memoized crater fields keyed by their content hash. Composing the terrain
/// layer stack re-runs on EVERY live edit (a brush/flatten re-parses the whole
/// stack), and regenerating the analytic crater set means Poisson-placing
/// thousands of craters on the main thread each time — even though an edit leaves
/// the crater spec untouched. The content key already uniquely identifies the
/// generated field, so cache by it: an edit becomes an `Arc` clone, not a
/// re-placement. Bounded (a handful of live specs); cleared if it ever grows past
/// a slider-drag's worth of distinct values.
static CRATER_CACHE: LazyLock<Mutex<HashMap<u64, HeightContribution>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// The composable crater layer: deterministic placements → analytic [`Craters`]
/// modifier on the surface oracle.
struct CraterFieldLayer {
    craters: CraterLayer,
    seed: u64,
}

/// Deterministic uniform draw in `[0, 1)` from `(seed, salt, index)` — one
/// independent stream per salt, identical on every peer.
fn hash01(seed: u64, salt: u64, i: usize) -> f64 {
    let mut h = lunco_precompute::Fnv1a::new();
    h.write_u64(seed);
    h.write_u64(salt);
    h.write_u64(i as u64);
    (h.finish() >> 11) as f64 / (1u64 << 53) as f64
}

impl TerrainLayer for CraterFieldLayer {
    fn id(&self) -> &'static str {
        "craters"
    }

    fn height_modifier(&self, half_extent: f32) -> Option<HeightContribution> {
        // Content key: every parameter that shapes the placements + profile, so
        // downstream content-addressed bakes (derived maps, tiles) re-key on any
        // live crater tweak. Computed FIRST (cheap — no placement) so an unchanged
        // spec is served from `CRATER_CACHE` without re-Poisson-placing.
        let mut key = lunco_precompute::Fnv1a::new();
        // Placement/morphology-algorithm version: bump when the same spec yields
        // different craters (blue-noise → CSR = v2; per-crater degradation
        // states = v3; power-law size mix = v4; full-range SFD + octave overprint
        // + degradation bowl shape + secondary clusters = v5) so content-addressed
        // downstream bakes re-key.
        key.write_u64(5);
        key.write_u64(self.seed);
        key.write_u64(half_extent.to_bits() as u64);
        key.write_u64(self.craters.density.to_bits() as u64);
        key.write_u64(self.craters.size.min.to_bits() as u64);
        key.write_u64(self.craters.size.mode.to_bits() as u64);
        key.write_u64(self.craters.size.max.to_bits() as u64);
        key.write_u64(self.craters.depth_ratio.to_bits() as u64);
        key.write_u64(self.craters.rim_height_ratio.to_bits() as u64);
        let content_key = key.finish();

        if let Some(hit) = CRATER_CACHE.lock().unwrap().get(&content_key).cloned() {
            return Some(hit);
        }

        let placements = crate::terrain::crater_placements(&self.craters, self.seed, half_extent);
        if placements.is_empty() {
            return None;
        }
        // Shared per-crater morphology from a degradation state `u` and the
        // authored fresh ratios. A real surface is dominated by old craters —
        // shallow, soft, nearly rimless, flat-floored — with a fresh sharp
        // paraboloid minority; the authored depth/rim ratios are the FRESH
        // (u = 0) endpoint. Identical fresh profiles everywhere is what read as
        // procedural stamping ("unrealistic craters").
        let morph = |radius: f64, u: f64, depth_scatter: f64| -> Crater {
            // Bowl shallows with age (fresh 1.0 → ghost 0.15), the rim lip
            // erodes faster than the bowl, the whole profile rounds off
            // (softness feeds the same closed-form blur as the Nyquist gate),
            // and the floor flattens (paraboloid p=2 fresh → flat dish p=6).
            let depth_k = 1.0 - 0.85 * u.powf(0.7);
            let rim_k = (1.0 - u) * (1.0 - u);
            let softness = 0.03 + 0.45 * u * u;
            let depth = radius * self.craters.depth_ratio as f64 * depth_k * depth_scatter;
            Crater {
                center: [0.0, 0.0],
                radius,
                depth,
                rim_height: depth * self.craters.rim_height_ratio as f64 * rim_k,
                softness,
                bowl_power: 2.0 + 4.0 * u,
            }
        };
        let mut craters: Vec<Crater> = placements
            .iter()
            .enumerate()
            .map(|(i, p)| {
                // SIZE-FREQUENCY: the placement sampler's log-normal-around-mode
                // sizes read as a monotonous same-scale field. Real regolith is
                // SATURATED with small craters and punctuated by rare large ones
                // — power-law N(>r) ∝ r^-1.8 across the FULL authored range
                // [min, max] for 85% of the population; 15% keep the authored
                // log-normal size (the authored-scale minority).
                let su = hash01(self.seed, 0x517E_0001, i); // size-mix stream
                let radius = if su < 0.85 {
                    let a = 1.8_f64;
                    let rmin = self.craters.size.min.max(0.5) as f64;
                    let rmax = (self.craters.size.max as f64).max(rmin + 0.1);
                    let q = (rmin / rmax).powf(a);
                    let uu = hash01(self.seed, 0x517E_0002, i); // size-draw stream
                    rmin * (1.0 - uu * (1.0 - q)).powf(-1.0 / a)
                } else {
                    p.size as f64
                };
                let u = hash01(self.seed, 0x0DE6_4ADE, i);
                // ±15% depth scatter breaks exact self-similarity across the field.
                let depth_scatter = 0.85 + 0.30 * hash01(self.seed, 0x0DE6_5CA7, i);
                let mut c = morph(radius, u, depth_scatter);
                c.center = [p.pos.x as f64, p.pos.y as f64];
                c
            })
            .collect();
        // SECONDARIES: real fields aren't pure CSR — large impacts throw ejecta
        // that gouges chains/clusters of small, shallow craters around them.
        // Re-home ~15% of the small population near a large parent: range
        // 2–5 parent radii, azimuth in a narrow lobe per parent (→ chains),
        // size a few % of the parent, born degraded (shallow, soft).
        let parents: Vec<usize> =
            (0..craters.len()).filter(|&i| craters[i].radius >= 12.0).collect();
        if !parents.is_empty() {
            for i in 0..craters.len() {
                if craters[i].radius >= 12.0 || hash01(self.seed, 0x5EC0_0001, i) >= 0.15 {
                    continue;
                }
                let pj = parents[(hash01(self.seed, 0x5EC0_0002, i) * parents.len() as f64)
                    as usize
                    % parents.len()];
                let parent = craters[pj];
                let lobe = hash01(self.seed, 0x5EC0_0003, pj) * std::f64::consts::TAU;
                let az = lobe + (hash01(self.seed, 0x5EC0_0004, i) - 0.5) * 0.9;
                let range = parent.radius * (2.0 + 3.0 * hash01(self.seed, 0x5EC0_0005, i));
                let radius = (parent.radius
                    * (0.04 + 0.08 * hash01(self.seed, 0x5EC0_0006, i)))
                .max(self.craters.size.min.max(0.5) as f64);
                let u = 0.45 + 0.5 * hash01(self.seed, 0x5EC0_0007, i);
                let mut c = morph(radius, u, 0.8);
                c.center = [
                    parent.center[0] + az.cos() * range,
                    parent.center[1] + az.sin() * range,
                ];
                craters[i] = c;
            }
        }
        bevy::log::info!(
            "[terrain-layer/craters] composed {} analytic crater(s) (±{:.0} m)",
            craters.len(),
            half_extent
        );
        let contrib = HeightContribution {
            modifier: Arc::new(Craters::new(craters)),
            content_key,
        };
        let mut cache = CRATER_CACHE.lock().unwrap();
        // Live slider-drag tuning mints a distinct key per value — cap the cache so
        // it can't grow without bound across a session of tweaking.
        if cache.len() >= 32 {
            cache.clear();
        }
        cache.insert(content_key, contrib.clone());
        Some(contrib)
    }

    fn stamp_spec(&self) -> Option<lunco_terrain_bake::StampSpec> {
        Some(lunco_terrain_bake::StampSpec::Craters { layer: self.craters, seed: self.seed })
    }
}

/// Parse a `lunco:layer = "craters"` prim: `density` (per ha, required > 0),
/// `sizeMode` (modal rim radius m), `depthRatio`, `rimRatio`, `seed`. DEM-scale size
/// range brackets the modal radius.
pub(super) fn parse_crater_layer(a: &dyn LayerAttrSource) -> Option<Arc<dyn TerrainLayer>> {
    let density = a.get_f32("density").unwrap_or(0.0);
    if density <= 0.0 {
        return None;
    }
    let mode = a.get_f32("sizeMode").unwrap_or(22.0);
    let craters = CraterLayer {
        enabled: true,
        density,
        // 2–60 m radii: the SFD floor sits at the smallest crater the finest
        // visual LOD (~0.65 m vertex pitch) fully resolves; the tail past the
        // mode is the rare large-crater "punctuation". Sub-2 m is the overzoom
        // synthesiser's band. The old 8–40 m band with a hump at the mode was
        // the single biggest procedural tell — every crater the same order of
        // magnitude.
        size: SizeDist::new(2.0, mode, 60.0, 0.7),
        // Fresh simple craters measure d/D ≈ 0.2 → depth = 0.4·r.
        depth_ratio: a.get_f32("depthRatio").unwrap_or(0.4),
        // Fresh rim/depth: 0.18 × depth 0.4·r gives rim/D ≈ 0.036 — the measured
        // lunar value. Taller lips were both the "wall" rovers hit nosing into
        // young craters and a strong fake-crater cue.
        rim_height_ratio: a.get_f32("rimRatio").unwrap_or(0.18),
    };
    let seed = a.get_i64("seed").map(|s| s as u64).unwrap_or(0xC0FFEE);
    Some(Arc::new(CraterFieldLayer { craters, seed }))
}

/// Build a crater layer from a typed [`CraterLayer`] (e.g. the Inspector's
/// `ObstacleFieldSpec.craters`) so live tuning can rebuild the terrain's crater
/// layer directly — honouring every authored field (density, depth, rim, full size
/// distribution).
pub fn crater_layer(craters: CraterLayer, seed: u64) -> Arc<dyn TerrainLayer> {
    Arc::new(CraterFieldLayer { craters, seed })
}

/// Construct a crater layer directly (the quick `SpawnDemTerrain` command path /
/// programmatic use; the USD path uses [`parse_crater_layer`]).
pub fn make_crater_layer(density: f32, size_mode: f32, depth_ratio: f32, seed: u64) -> Arc<dyn TerrainLayer> {
    Arc::new(CraterFieldLayer {
        craters: CraterLayer {
            enabled: true,
            density,
            size: SizeDist::new(2.0, size_mode, 60.0, 0.7),
            depth_ratio,
            rim_height_ratio: 0.18,
        },
        seed,
    })
}
