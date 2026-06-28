//! CDLOD visual render data (milestone S2) — the pure mapping from a selected
//! quadtree node to its GPU draw instance, plus the shared grid geometry. The
//! Bevy material + WGSL vertex stage (morph + height-texture fetch + normals)
//! consume these; this module stays Bevy-free so the mapping is unit-tested.
//!
//! One R32Float texture holds the whole DEM. Every node draws the SAME unit grid
//! mesh, transformed by a per-node [`NodeInstance`]: `world = world_offset +
//! unit_xz * world_scale` for placement, and `uv = uv_offset + unit_xz * uv_scale`
//! to fetch height from the shared texture. So streaming a node costs one instance
//! + a small uniform, never a mesh rebuild (see `docs/terrain-streaming-IMPL.md`).

use crate::quadtree::Selected;

/// Per-node GPU instance data: where to place the shared grid in the world, and
/// which sub-rectangle of the DEM height texture it samples. `unit_xz ∈ [0,1]²`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NodeInstance {
    /// World XZ of the node's `(0,0)` corner (metres).
    pub world_offset: [f32; 2],
    /// World side length (metres) — `unit_xz * world_scale` spans the node.
    pub world_scale: f32,
    /// Texture-UV of the node's `(0,0)` corner (`[0,1]`).
    pub uv_offset: [f32; 2],
    /// Texture-UV side length the node covers.
    pub uv_scale: f32,
    /// CDLOD geomorph band (metres) — fully morphed to the parent at `morph_end`.
    pub morph_start: f32,
    pub morph_end: f32,
    /// Node depth (debug / shader tint / LOD-specific tuning).
    pub depth: u32,
}

/// Map a [`Selected`] node to its GPU instance, given the root half-extent (the
/// DEM's `half_extent`, origin-centred). UV maps the world XZ range
/// `[-H, H] → [0, 1]` — the exact inverse of how the shader addresses the
/// heightfield (mirrors `horizon.rs` planar UVs `(local.xz - min)/size`).
pub fn node_instance(sel: &Selected, root_half_extent: f64) -> NodeInstance {
    let h = root_half_extent;
    let side = sel.region.side();
    // (0,0) corner of the node in world XZ.
    let wx = sel.region.center[0] - sel.region.half;
    let wz = sel.region.center[1] - sel.region.half;
    let inv = 1.0 / (2.0 * h);
    // morph_end may be INFINITY for the root (no parent); clamp to a large finite
    // value so the shader never sees a non-finite uniform (→ NaN morph).
    let morph_end = if sel.morph_end.is_finite() { sel.morph_end } else { (4.0 * h).max(side) };
    let morph_start = if sel.morph_start.is_finite() { sel.morph_start } else { morph_end };
    NodeInstance {
        world_offset: [wx as f32, wz as f32],
        world_scale: side as f32,
        uv_offset: [((wx + h) * inv) as f32, ((wz + h) * inv) as f32],
        uv_scale: (side * inv) as f32,
        morph_start: morph_start as f32,
        morph_end: morph_end as f32,
        depth: sel.coord.depth as u32,
    }
}

/// CDLOD geomorph factor at `dist` for a node's `[morph_start, morph_end]` band:
/// `0` = use this node's own geometry, `1` = fully snapped to the parent grid.
/// Smoothstep gives C¹ continuity so the transition has no visible kink.
pub fn morph_factor(dist: f64, morph_start: f64, morph_end: f64) -> f64 {
    if morph_end <= morph_start {
        return 0.0;
    }
    let t = ((dist - morph_start) / (morph_end - morph_start)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t) // smoothstep
}

/// Vertices + indices of the shared `n × n`-vertex unit grid (`(n-1)²` quads, two
/// triangles each). Positions are `[ux, 0, uz]` with `ux, uz ∈ [0,1]`; the shader
/// scales to the node region and lifts Y from the height texture. `n` clamped ≥ 2.
pub fn unit_grid(n: usize) -> (Vec<[f32; 3]>, Vec<u32>) {
    let n = n.max(2);
    let mut positions = Vec::with_capacity(n * n);
    let inv = 1.0 / (n as f32 - 1.0);
    for iz in 0..n {
        for ix in 0..n {
            positions.push([ix as f32 * inv, 0.0, iz as f32 * inv]);
        }
    }
    let mut indices = Vec::with_capacity((n - 1) * (n - 1) * 6);
    let row = n as u32;
    for iz in 0..(n as u32 - 1) {
        for ix in 0..(n as u32 - 1) {
            let i = iz * row + ix;
            // Two CCW triangles per quad.
            indices.extend_from_slice(&[i, i + row, i + 1, i + 1, i + row, i + row + 1]);
        }
    }
    (positions, indices)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quadtree::{QuadCoord, Quadtree};

    #[test]
    fn root_instance_spans_full_texture() {
        let q = Quadtree::new(8000.0, 6, 4.0, 8000.0);
        // A lone root selection (max_depth 0 → root is emitted as-is).
        let root_only = Quadtree::new(8000.0, 0, 4.0, 8000.0);
        let sel = root_only.select([0.0, 0.0]);
        assert_eq!(sel.len(), 1);
        let inst = node_instance(&sel[0], q.root_half_extent);
        assert_eq!(inst.world_offset, [-8000.0, -8000.0]);
        assert_eq!(inst.world_scale, 16000.0);
        assert_eq!(inst.uv_offset, [0.0, 0.0]);
        assert!((inst.uv_scale - 1.0).abs() < 1e-6);
        assert!(inst.morph_end.is_finite());
    }

    #[test]
    fn child_instance_maps_to_quadrant_uv() {
        let q = Quadtree::new(8000.0, 6, 4.0, 8000.0);
        // Top-left child (x=0,z=0) of the root: world [-8000,0]², uv [0,0.5]².
        let child = QuadCoord { depth: 1, x: 0, z: 0 };
        let region = q.region(child);
        let sel = Selected { coord: child, region, morph_start: 1000.0, morph_end: 2000.0 };
        let inst = node_instance(&sel, q.root_half_extent);
        assert_eq!(inst.world_offset, [-8000.0, -8000.0]);
        assert_eq!(inst.world_scale, 8000.0);
        assert_eq!(inst.uv_offset, [0.0, 0.0]);
        assert!((inst.uv_scale - 0.5).abs() < 1e-6);
        // Opposite child maps to the far UV quadrant.
        let far = QuadCoord { depth: 1, x: 1, z: 1 };
        let inst2 = node_instance(&Selected { coord: far, region: q.region(far), morph_start: 1.0, morph_end: 2.0 }, q.root_half_extent);
        assert!((inst2.uv_offset[0] - 0.5).abs() < 1e-6);
        assert!((inst2.uv_offset[1] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn morph_factor_is_clamped_and_monotonic() {
        assert_eq!(morph_factor(0.0, 100.0, 200.0), 0.0); // before band
        assert_eq!(morph_factor(500.0, 100.0, 200.0), 1.0); // past band
        let mid = morph_factor(150.0, 100.0, 200.0);
        assert!((mid - 0.5).abs() < 1e-9); // smoothstep midpoint
        // Degenerate band → no morph (avoids div by zero).
        assert_eq!(morph_factor(150.0, 200.0, 200.0), 0.0);
    }

    #[test]
    fn unit_grid_counts() {
        let (pos, idx) = unit_grid(4);
        assert_eq!(pos.len(), 16); // 4×4 verts
        assert_eq!(idx.len(), 3 * 3 * 6); // 9 quads × 2 tris × 3
        // Corners at the unit square bounds.
        assert_eq!(pos[0], [0.0, 0.0, 0.0]);
        assert_eq!(pos[15], [1.0, 0.0, 1.0]);
        // Every index in range.
        assert!(idx.iter().all(|&i| (i as usize) < pos.len()));
    }
}
