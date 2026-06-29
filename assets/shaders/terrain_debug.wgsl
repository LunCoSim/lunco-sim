//! Terrain LOD/DEM debug visualisation for the **dynamic** `ShaderMaterial`.
//!
//! Self-describing: the engine reflects the `Material` struct (field names →
//! std140 offsets) and the `//!@` annotations straight out of this file
//! (`crate::dyn_params`). Switch the visualisation **live** — no rebuild —
//! via the Inspector or `SetObjectProperty` (set `mode`), and edit this `.wgsl`
//! to hot-reload. No native/compiled material is involved.
//!
//! `mode`: 0 = off (neutral) · 1 = elevation heatmap (turbo over
//! [elev_min, elev_max], world Y) · 2 = slope (green flat → red steep) ·
//! 3 = LOD/tile colour (flat palette by quadtree depth `lod`). `grid` > 0.5
//! rings tile edges from the DEM-global UVs. Deliberately **unlit** (just a
//! top-down shade for relief) so debug colours are readable even in the
//! airless-Moon shadow cores.

#import bevy_pbr::forward_io::VertexOutput

// Dynamic, self-describing parameters (reflected from this struct + annotations).
//!@ui      mode      0 3   "Debug mode (0 off,1 elev,2 slope,3 lod)"
//!@default mode      2
//!@ui      elev_min  -1000 3000  "Elevation min (m)"
//!@default elev_min  -600
//!@ui      elev_max  -1000 3000  "Elevation max (m)"
//!@default elev_max  2000
//!@ui      lod       0 12  "LOD depth (tile colour)"
//!@default lod       0
//!@ui      grid      0 1   "Tile-edge grid overlay"
//!@default grid      0
struct Material {
    mode:     f32,
    elev_min: f32,
    elev_max: f32,
    lod:      f32,
    grid:     f32,
}
@group(#{MATERIAL_BIND_GROUP}) @binding(0)
var<uniform> material: Material;

// Polynomial approximation of the "turbo" colormap.
fn turbo(t_in: f32) -> vec3<f32> {
    let t = clamp(t_in, 0.0, 1.0);
    let r = 0.13572138 + t * (4.61539260 + t * (-42.66032258 + t * (132.13108234 + t * (-152.94239396 + t * 59.28637943))));
    let g = 0.09140261 + t * (2.19418839 + t * (4.84296658 + t * (-14.18503333 + t * (4.27729857 + t * 2.82956604))));
    let b = 0.10667330 + t * (12.64194608 + t * (-60.58204836 + t * (110.36276771 + t * (-89.90310912 + t * 27.34824973))));
    return clamp(vec3<f32>(r, g, b), vec3<f32>(0.0), vec3<f32>(1.0));
}

// Distinct flat colour per LOD depth (wraps after 8).
fn lod_color(lod: u32) -> vec3<f32> {
    switch (lod % 8u) {
        case 0u:  { return vec3<f32>(0.20, 0.40, 1.00); }
        case 1u:  { return vec3<f32>(0.20, 0.85, 0.95); }
        case 2u:  { return vec3<f32>(0.20, 0.90, 0.35); }
        case 3u:  { return vec3<f32>(0.85, 0.95, 0.20); }
        case 4u:  { return vec3<f32>(1.00, 0.60, 0.15); }
        case 5u:  { return vec3<f32>(1.00, 0.25, 0.20); }
        case 6u:  { return vec3<f32>(0.95, 0.30, 0.85); }
        default:  { return vec3<f32>(0.90, 0.90, 0.90); }
    }
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> @location(0) vec4<f32> {
    let n = normalize(in.world_normal);
    var col = vec3<f32>(0.5, 0.5, 0.5);

    let mode = material.mode;
    if mode > 0.5 && mode < 1.5 {
        let denom = max(material.elev_max - material.elev_min, 1e-3);
        col = turbo((in.world_position.y - material.elev_min) / denom);
    } else if mode > 1.5 && mode < 2.5 {
        let up = clamp(n.y, 0.0, 1.0);
        col = mix(vec3<f32>(0.85, 0.15, 0.10), vec3<f32>(0.10, 0.65, 0.15), up);
    } else if mode > 2.5 {
        col = lod_color(u32(material.lod + 0.5));
    }

    if material.grid > 0.5 {
        // World-space 256 m grid (no UV dependency — robust on any mesh).
        let w = in.world_position.xz / 256.0;
        let g = abs(fract(w - 0.5) - 0.5) / fwidth(w);
        col = mix(vec3<f32>(0.0), col, smoothstep(0.0, 1.5, min(g.x, g.y)));
    }

    // Lighting-independent top-down shade so relief reads in shadow.
    let shade = 0.45 + 0.55 * clamp(n.y, 0.0, 1.0);
    return vec4<f32>(col * shade, 1.0);
}
