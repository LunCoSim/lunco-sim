//! Built-in **craters** layer.
//!
//! Two contributions, both deterministic from the same placements:
//! - **stamp** every crater into the working grid → the streamed tiles + heightfield
//!   collider get drivable basins everywhere (coarse at distance);
//! - **scatter** a dedicated **high-fidelity crater mesh** for each NEAR crater —
//!   radially tessellated with MANY polygons at the rim and FEW on the floor
//!   ("adaptive, more on top"), built on the smooth pre-crater base so it doesn't
//!   double the stamped basin, drawn with the SAME `terrain_geomorph` regolith
//!   material as the tiles (no seam), and LOD-culled. This is the crater-scoped
//!   alternative to curvature-aware LOD in the shared core tiler.

use std::sync::Arc;

use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::VisibilityRange;
use bevy::math::Vec2;
use bevy::prelude::*;
use bevy::render::render_resource::PrimitiveTopology;
use bevy_mesh::Indices;
use lunco_obstacle_field::field::{crater_delta, HeightGrid};
use lunco_obstacle_field::spec::{CraterLayer, SizeDist};
use lunco_materials::{ParamValue, ShaderMaterial, ATTRIBUTE_MORPH_TARGET};

use super::{LayerAttrSource, LayerScatterCx, TerrainLayer, TerrainScatterEntity};

/// Radial segments per crater ring.
const CRATER_SEGMENTS: usize = 48;
/// Near crater overlays fully visible to `LOD_FAR`, cross-fade out over `LOD_FADE`.
const LOD_FAR: f32 = 450.0;
const LOD_FADE: f32 = 120.0;

/// Stamps the big drivable impact basins into the DEM grid AND overlays a crisp
/// adaptive mesh for the near ones.
struct CraterStampLayer {
    craters: CraterLayer,
    seed: u64,
    /// Half-extent (m) of the near-field region that gets the high-fidelity overlay
    /// meshes. Far craters ride the (coarser) stamped tile geometry.
    detail_region: f32,
}

impl TerrainLayer for CraterStampLayer {
    fn id(&self) -> &'static str {
        "craters"
    }

    fn stamp(&self, grid: &mut HeightGrid) {
        let n = crate::terrain::stamp_spec_craters(grid, &self.craters, self.seed);
        if n > 0 {
            info!("[terrain-layer/craters] stamped {n} crater(s) (±{:.0} m)", grid.half_extent);
        }
    }

    fn scatter(&self, cx: &mut LayerScatterCx) {
        // The overlay needs the SMOOTH pre-crater base (so it doesn't double the
        // already-stamped basin) + render assets. Headless / no-base → tiles only.
        let Some(base) = cx.base_grid else { return };
        if cx.meshes.is_none() || cx.shader_materials.is_none() {
            return;
        }

        let placements = crate::terrain::crater_placements(&self.craters, self.seed, base.half_extent);
        let region = self.detail_region.min(base.half_extent);

        // One shared regolith material (same `terrain_geomorph` shader as the tiles,
        // morph disabled) so the overlay craters shade identically — no seam.
        let shader = cx.asset_server.load("shaders/terrain_geomorph.wgsl");
        let material = {
            let sm = cx.shader_materials.as_deref_mut().unwrap();
            let mut m = ShaderMaterial::default();
            m.shader = shader.clone();
            m.vertex_shader = Some(shader);
            // "never" morph band → the @vertex CDLOD stage passes POSITION through.
            m.set_many([("morph_start", ParamValue::F32(1.0e20)), ("morph_end", ParamValue::F32(1.0e21))]);
            sm.add(m)
        };

        // Build a mesh per near crater (bowls vary by size + local slope), then spawn.
        let meshes = cx.meshes.as_deref_mut().unwrap();
        let mut built: Vec<(Handle<Mesh>, Vec2, f32)> = Vec::new();
        for p in &placements {
            if p.pos.x.abs() > region || p.pos.y.abs() > region {
                continue;
            }
            let depth = p.size * self.craters.depth_ratio;
            let rim = depth * self.craters.rim_height_ratio;
            let mesh = crater_overlay_mesh(base, p.pos, p.size, depth, rim);
            let base_y = base.height_at(p.pos.x, p.pos.y);
            built.push((meshes.add(mesh), p.pos, base_y));
        }

        let count = built.len();
        let terrain = cx.terrain;
        for (handle, pos, base_y) in built {
            cx.commands.spawn((
                Name::new("CraterMesh"),
                TerrainScatterEntity,
                ChildOf(terrain),
                Mesh3d(handle),
                MeshMaterial3d(material.clone()),
                Transform::from_xyz(pos.x, base_y, pos.y),
                Visibility::Inherited,
                VisibilityRange {
                    start_margin: 0.0..0.0,
                    end_margin: LOD_FAR..(LOD_FAR + LOD_FADE),
                    use_aabb: false,
                },
            ));
        }
        if count > 0 {
            info!("[terrain-layer/craters] overlaid {count} high-fidelity crater mesh(es) (±{:.0} m)", region);
        }
    }
}

/// A radially-tessellated crater mesh in the terrain-local frame, ORIGIN-centred on
/// the crater (the entity is placed at the crater's base height). Rings are dense at
/// the rim and sparse on the floor/apron ("many polys on top, few on the bottom").
/// Heights = smooth base delta + the shared [`crater_delta`] profile, plus a small
/// lift so the crisp overlay reliably wins the depth test over the coarse stamped
/// basin in the tiles underneath.
fn crater_overlay_mesh(base: &HeightGrid, center: Vec2, radius: f32, depth: f32, rim: f32) -> Mesh {
    // Concentric ring radii (normalised d): coarse floor → DENSE rim → coarse apron.
    let mut ds: Vec<f32> = Vec::new();
    let mut zone = |a: f32, b: f32, n: usize| {
        for i in 1..=n {
            ds.push(a + (b - a) * (i as f32 / n as f32));
        }
    };
    zone(0.0, 0.80, 4); // floor (few)
    zone(0.80, 1.15, 14); // wall + rim (many)
    zone(1.15, 1.50, 4); // ejecta apron (few)

    let base_center = base.height_at(center.x, center.y);
    let lift = 0.10 + 0.03 * depth;
    let height = |d: f32, off: Vec2| -> f32 {
        let w = center + off;
        base.height_at(w.x, w.y) - base_center + crater_delta(d, depth, rim) + lift
    };

    let mut positions: Vec<[f32; 3]> = Vec::with_capacity(1 + ds.len() * CRATER_SEGMENTS);
    let mut uvs: Vec<[f32; 2]> = Vec::with_capacity(positions.capacity());
    // Apex (centre).
    positions.push([0.0, crater_delta(0.0, depth, rim) + lift, 0.0]);
    uvs.push([0.5, 0.5]);
    for &d in &ds {
        for k in 0..CRATER_SEGMENTS {
            let ang = (k as f32 / CRATER_SEGMENTS as f32) * std::f32::consts::TAU;
            let (sin, cos) = ang.sin_cos();
            let off = Vec2::new(cos, sin) * (d * radius);
            positions.push([off.x, height(d, off), off.y]);
            uvs.push([0.5 + 0.5 * cos * d / 1.5, 0.5 + 0.5 * sin * d / 1.5]);
        }
    }

    let mut indices: Vec<u32> = Vec::new();
    let seg = CRATER_SEGMENTS as u32;
    // Centre fan → first ring. Wound so the front face (and thus `compute_normals`)
    // points UP: with +Y up and ring vertices advancing CCW in math-angle (cos,sin),
    // the up-facing triangle viewed from above is `[0, b, a]` (apex → next → current).
    for k in 0..seg {
        let a = 1 + k;
        let b = 1 + (k + 1) % seg;
        indices.extend_from_slice(&[0, b, a]);
    }
    // Strips between consecutive rings (same up-facing winding).
    for r in 0..ds.len() as u32 - 1 {
        let r0 = 1 + r * seg;
        let r1 = 1 + (r + 1) * seg;
        for k in 0..seg {
            let k1 = (k + 1) % seg;
            let a = r0 + k;
            let b = r0 + k1;
            let c = r1 + k;
            let d2 = r1 + k1;
            indices.extend_from_slice(&[a, b, c, b, d2, c]);
        }
    }

    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default());
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions.clone());
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_indices(Indices::U32(indices));
    mesh.compute_normals();
    // The `terrain_geomorph` @vertex stage reads a morph target; with the "never"
    // band it's unused, but the attribute must exist → identity (= positions).
    mesh.insert_attribute(ATTRIBUTE_MORPH_TARGET, positions);
    mesh
}

/// Parse a `lunco:layer = "craters"` prim: `density` (per ha, required > 0),
/// `sizeMode` (modal rim radius m), `depthRatio`, `rimRatio`, `detailRegionM` (near
/// overlay half-extent), `seed`. DEM-scale size range bracketing the modal radius.
pub(super) fn parse_crater_layer(a: &dyn LayerAttrSource) -> Option<Arc<dyn TerrainLayer>> {
    let density = a.get_f32("density").unwrap_or(0.0);
    if density <= 0.0 {
        return None;
    }
    let mode = a.get_f32("sizeMode").unwrap_or(22.0);
    let craters = CraterLayer {
        enabled: true,
        density,
        size: SizeDist::new(8.0, mode, 40.0, 0.7),
        depth_ratio: a.get_f32("depthRatio").unwrap_or(0.3),
        rim_height_ratio: a.get_f32("rimRatio").unwrap_or(0.5),
    };
    let seed = a.get_i64("seed").map(|s| s as u64).unwrap_or(0xC0FFEE);
    let detail_region = a.get_f32("detailRegionM").unwrap_or(400.0);
    Some(Arc::new(CraterStampLayer { craters, seed, detail_region }))
}

/// Construct a crater layer directly (the quick `SpawnDemTerrain` command path /
/// programmatic use; the USD path uses [`parse_crater_layer`]).
pub fn make_crater_layer(density: f32, size_mode: f32, depth_ratio: f32, seed: u64) -> Arc<dyn TerrainLayer> {
    Arc::new(CraterStampLayer {
        craters: CraterLayer {
            enabled: true,
            density,
            size: SizeDist::new(8.0, size_mode, 40.0, 0.7),
            depth_ratio,
            rim_height_ratio: 0.5,
        },
        seed,
        detail_region: 400.0,
    })
}
