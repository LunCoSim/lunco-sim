//! Content-addressed **tile mesh bake cache** — `lunco-precompute` Substrate B
//! applied to the CDLOD tile bakes (terrain-substrate roadmap #6).
//!
//! A tile's geometry is a pure function of `(composed surface, quad node, mesh
//! resolution)`, so it is cacheable across runs and — because the key derives
//! from [`SurfaceOracle::surface_key`] (base DEM heights + modifier parameters)
//! — byte-identical across peers. A warm reload of the same terrain streams its
//! near-field from disk instead of re-sampling the oracle per vertex; a live
//! layer edit changes the surface key, so stale entries are simply never matched
//! (content-addressed → no invalidation).
//!
//! The on-disk format is a single raw little-endian blob (no serde): counts
//! header + the five vertex/index arrays. Bump [`CACHE_FORMAT_VERSION`] on any
//! bake-math or layout change.

use std::path::Path;

use lunco_terrain_core::{HeightSource, QuadCoord, Square};

use crate::oracle::SurfaceOracle;
use crate::tile_mesh::{bake_tile_mesh, TileMesh};

/// Bump when `bake_tile_mesh` math (heights, normals eps, morph snap, skirts,
/// detail gating) or the blob layout changes.
/// v2: crater profile is band-limited per tile step and continuous at its reach
/// (the layer's parameter `content_key` is unchanged, so only a version bump
/// retires tiles baked with the old aliasing profile).
/// v3: shading-normal `eps` is a fixed world scale across LOD depths (was
/// per-depth → per-depth brightness steps under the normal-driven lunar BRDF).
/// v4: vertex heights gated at 2·step (true Nyquist — 1·step kept rim-scale
/// features right at the sampling limit → mid-field sawtooth craters); morph
/// targets sample a 4·step parent-gated surface (were this tile's finer heights
/// on the 2×-spaced even lattice → aliased morph band + pop at the tile swap).
/// v5: vertex Y is now rebased by the tile-centre surface height (`origin_y`), so
/// a tile's mesh is LOCAL to its own big_space `CellCoord` in Y as well as X/Z
/// (DEM scenes anchored at absolute lunar elevation put geometry ~2 km from the
/// tile origin — one cell off the content — breaking LOD/culling/colliders).
const CACHE_FORMAT_VERSION: u64 = 6;

/// One tile bake as a [`lunco_precompute::Bake`] entry.
struct TileBake<'a> {
    oracle: &'a SurfaceOracle,
    coord: QuadCoord,
    region: Square,
    res: usize,
    dem_half_extent: f64,
    origin_xz: [f64; 2],
}

impl lunco_precompute::Bake for TileBake<'_> {
    type Output = TileMesh;
    const NAMESPACE: &'static str = "terrain/tiles";

    fn key(&self) -> u64 {
        let mut h = lunco_precompute::Fnv1a::new();
        h.write_u64(CACHE_FORMAT_VERSION);
        h.write_u64(self.oracle.surface_key());
        h.write_u64(self.coord.depth as u64);
        h.write_u64(self.coord.x as u64);
        h.write_u64(self.coord.z as u64);
        h.write_u64(self.res as u64);
        // region/origin derive from (root extent, coord), but fold them anyway so
        // a root-extent change can never alias.
        h.write_u64(self.region.center[0].to_bits());
        h.write_u64(self.region.center[1].to_bits());
        h.write_u64(self.region.half.to_bits());
        h.finish()
    }

    fn bake(&self) -> TileMesh {
        // Band-limit at TRUE Nyquist for this tile's vertex spacing (2 samples
        // per shortest wavelength — the collider path's convention). Morph
        // targets live on the parent's 2×-spaced lattice, so they sample a
        // 2×-coarser gate again: the fully-morphed tile IS the parent surface.
        let step = self.region.side() / (self.res.max(2) - 1) as f64;
        let limited = self.oracle.detail_limited(2.0 * step);
        let parent_limited = self.oracle.detail_limited(4.0 * step);
        // Anchor Y at the FULL-oracle surface height under the tile centre — the same
        // value `spawn_tile`/the collider ring use to place the tile's `CellCoord`, so
        // mesh geometry and entity origin agree. (Full oracle, not `limited`, so the
        // placement matches what the visualiser samples with `hf.0.height_at`.)
        let origin_y = self.oracle.height_at(self.region.center[0], self.region.center[1]);
        bake_tile_mesh(
            &limited,
            &parent_limited,
            self.region,
            self.res,
            self.dem_half_extent,
            self.origin_xz,
            origin_y,
        )
    }

    fn store(dir: &Path, out: &TileMesh) -> lunco_precompute::StorageResult<()> {
        lunco_precompute::store_blob(dir, "mesh.bin", &tile_mesh_to_bytes(out))
    }

    fn load(dir: &Path) -> Option<TileMesh> {
        tile_mesh_from_bytes(&lunco_precompute::load_blob(dir, "mesh.bin")?)
    }
}

/// Bake one CDLOD tile through the content-addressed disk cache: load on a key
/// hit, bake + persist on a miss. Pure either way; safe off-thread.
pub fn bake_tile_mesh_cached(
    oracle: &SurfaceOracle,
    coord: QuadCoord,
    region: Square,
    res: usize,
    dem_half_extent: f64,
    origin_xz: [f64; 2],
) -> TileMesh {
    lunco_precompute::bake_or_load(
        &TileBake { oracle, coord, region, res, dem_half_extent, origin_xz },
        &lunco_assets::cache_dir(),
    )
}

fn tile_mesh_to_bytes(m: &TileMesh) -> Vec<u8> {
    let verts = m.positions.len();
    let mut out = Vec::with_capacity(16 + verts * (3 + 3 + 3 + 3 + 2) * 4 + m.indices.len() * 4);
    out.extend_from_slice(&(verts as u64).to_le_bytes());
    out.extend_from_slice(&(m.indices.len() as u64).to_le_bytes());
    let push3 = |v: &[[f32; 3]], out: &mut Vec<u8>| {
        for p in v {
            for c in p {
                out.extend_from_slice(&c.to_le_bytes());
            }
        }
    };
    push3(&m.positions, &mut out);
    push3(&m.morph_targets, &mut out);
    push3(&m.morph_normals, &mut out);
    push3(&m.normals, &mut out);
    for p in &m.uvs {
        for c in p {
            out.extend_from_slice(&c.to_le_bytes());
        }
    }
    for i in &m.indices {
        out.extend_from_slice(&i.to_le_bytes());
    }
    out
}

fn tile_mesh_from_bytes(b: &[u8]) -> Option<TileMesh> {
    let mut off = 0usize;
    let take = |off: &mut usize, n: usize| -> Option<&[u8]> {
        let s = b.get(*off..*off + n)?;
        *off += n;
        Some(s)
    };
    let verts = u64::from_le_bytes(take(&mut off, 8)?.try_into().ok()?) as usize;
    let idx_count = u64::from_le_bytes(take(&mut off, 8)?.try_into().ok()?) as usize;
    // Sanity: total size must match exactly (corrupt / truncated → rebake).
    let expect = 16 + verts * (3 + 3 + 3 + 3 + 2) * 4 + idx_count * 4;
    if b.len() != expect {
        return None;
    }
    let read3 = |off: &mut usize| -> Option<Vec<[f32; 3]>> {
        let mut v = Vec::with_capacity(verts);
        for _ in 0..verts {
            let s = take(off, 12)?;
            v.push([
                f32::from_le_bytes(s[0..4].try_into().ok()?),
                f32::from_le_bytes(s[4..8].try_into().ok()?),
                f32::from_le_bytes(s[8..12].try_into().ok()?),
            ]);
        }
        Some(v)
    };
    let positions = read3(&mut off)?;
    let morph_targets = read3(&mut off)?;
    let morph_normals = read3(&mut off)?;
    let normals = read3(&mut off)?;
    let mut uvs = Vec::with_capacity(verts);
    for _ in 0..verts {
        let s = take(&mut off, 8)?;
        uvs.push([
            f32::from_le_bytes(s[0..4].try_into().ok()?),
            f32::from_le_bytes(s[4..8].try_into().ok()?),
        ]);
    }
    let mut indices = Vec::with_capacity(idx_count);
    for _ in 0..idx_count {
        indices.push(u32::from_le_bytes(take(&mut off, 4)?.try_into().ok()?));
    }
    Some(TileMesh { positions, morph_targets, morph_normals, normals, uvs, indices })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_bytes() {
        let m = TileMesh {
            positions: vec![[1.0, 2.0, 3.0], [4.0, 5.0, 6.0]],
            morph_targets: vec![[1.5, 2.0, 3.0], [4.0, 5.5, 6.0]],
            // Deliberately distinct from `normals` so a serializer that dropped or
            // aliased this array fails the roundtrip instead of passing by luck.
            morph_normals: vec![[0.0, 0.6, 0.8], [1.0, 0.0, 0.0]],
            normals: vec![[0.0, 1.0, 0.0], [0.0, 0.8, 0.6]],
            uvs: vec![[0.0, 0.0], [1.0, 1.0]],
            indices: vec![0, 1, 0],
        };
        let b = tile_mesh_to_bytes(&m);
        let r = tile_mesh_from_bytes(&b).expect("roundtrip");
        assert_eq!(m, r);
        // Truncated / padded blobs are rejected (→ rebake), never mis-parsed.
        assert!(tile_mesh_from_bytes(&b[..b.len() - 1]).is_none());
        let mut padded = b.clone();
        padded.push(0);
        assert!(tile_mesh_from_bytes(&padded).is_none());
    }
}
