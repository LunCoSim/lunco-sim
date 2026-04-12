//! Solar Panel Material Extension Shader
//!
//! Renders a realistic photovoltaic cell grid pattern procedurally — no textures needed.
//!
//! ## Visual Features
//! 1. **Cell Grid**: Individual silicon cells separated by thin gaps
//! 2. **Bus Lines**: Metallic conductor lines between cells (horizontal + vertical)
//! 3. **Frame Border**: Dark edge around the cell array
//! 4. **Glass Reflections**: Subtle specular highlights from light sources
//!
//! ## Coordinate System
//!
//! The shader uses local mesh-space XZ coordinates (relative to panel center) to
//! ensure the cell grid is always anchored to the panel edges, regardless of
//! where the panel is placed in the world.

#import bevy_pbr::forward_io::VertexOutput
#import bevy_pbr::pbr_fragment::pbr_input_from_standard_material
#import bevy_pbr::pbr_functions::{apply_pbr_lighting, main_pass_post_lighting_processing}
#import bevy_pbr::forward_io::FragmentOutput

/// Solar panel extension uniforms — matches SolarPanelExtension Rust struct.
struct SolarPanelExtension {
    panel_half_width: f32,       // Half of panel width (meters)
    panel_half_depth: f32,       // Half of panel depth (meters)
    cell_rows: f32,              // Number of cells along width (X)
    cell_cols: f32,              // Number of cells along depth (Z)
    cell_color: vec4<f32>,       // Silicon cell color (RGBA)
    bus_line_color: vec4<f32>,   // Metallic bus bar color
    frame_border_color: vec4<f32>, // Frame edge color
    cell_gap: f32,               // Gap between cells (meters)
    bus_line_width: f32,         // Width of bus lines (meters)
    frame_border_width: f32,     // Visible frame border width (meters)
    glass_reflectivity: f32,     // Surface reflectivity (0-1)
    glass_roughness: f32,        // Surface roughness (0-1)
    specular_intensity: f32,     // Sun glare intensity
}

@group(#{MATERIAL_BIND_GROUP}) @binding(100)
var<uniform> extension: SolarPanelExtension;

/// Compute signed distance to nearest line in a periodic grid.
///
/// Given a local coordinate `p` and grid `spacing`, returns the perpendicular
/// distance from `p` to the nearest grid line centered at each cell boundary.
/// Negative values mean the point is inside the line region.
fn grid_line_sdf(p: f32, spacing: f32, line_half_width: f32) -> f32 {
    let coord = p / spacing;
    let dist_to_center = abs(fract(coord - 0.5) - 0.5) * spacing;
    return dist_to_center - line_half_width;
}

@fragment
fn fragment(
    input: VertexOutput,
    @builtin(front_facing) is_front: bool,
) -> FragmentOutput {
    var pbr_input = pbr_input_from_standard_material(input, is_front);

    // ============================================================
    // 1. Local mesh coordinates (panel is centered at origin)
    // ============================================================
    // The panel mesh is a Cuboid centered at (0,0,0).
    // We use UV coordinates mapped to local panel space.
    // UV [0,1] → local [-half, +half]
    let local_x = (input.uv.x - 0.5) * extension.panel_half_width * 2.0;
    let local_z = (input.uv.y - 0.5) * extension.panel_half_depth * 2.0;

    // ============================================================
    // 2. Cell spacing derived from panel dimensions / cell count
    // ============================================================
    let active_width = extension.panel_half_width * 2.0;
    let active_depth = extension.panel_half_depth * 2.0;
    let cell_spacing_x = active_width / extension.cell_rows;
    let cell_spacing_z = active_depth / extension.cell_cols;

    // ============================================================
    // 3. Frame border detection
    // ============================================================
    let frame_inner_half_x = extension.panel_half_width - extension.frame_border_width;
    let frame_inner_half_z = extension.panel_half_depth - extension.frame_border_width;
    let is_frame = (abs(local_x) > frame_inner_half_x) || (abs(local_z) > frame_inner_half_z);

    // ============================================================
    // 4. Cell grid — only render inside the active area
    // ============================================================
    let cell_gap_half = extension.cell_gap * 0.5;
    let cell_gap_x = grid_line_sdf(local_x, cell_spacing_x, cell_gap_half);
    let cell_gap_z = grid_line_sdf(local_z, cell_spacing_z, cell_gap_half);
    let is_cell_gap = cell_gap_x < 0.0 || cell_gap_z < 0.0;

    // ============================================================
    // 5. Bus lines (metallic conductors running through cells)
    // ============================================================
    let bus_half = extension.bus_line_width * 0.5;
    let bus_x = grid_line_sdf(local_x, cell_spacing_x, bus_half);
    let bus_z = grid_line_sdf(local_z, cell_spacing_z, bus_half);
    let is_bus = bus_x < 0.0 || bus_z < 0.0;

    // ============================================================
    // 6. Compose final color
    // ============================================================
    var final_color = extension.cell_color;

    if (is_cell_gap) {
        final_color = vec4<f32>(0.02, 0.02, 0.02, 1.0);
    }

    if (is_bus && !is_cell_gap) {
        final_color = mix(extension.cell_color, extension.bus_line_color, 0.9);
    }

    if (is_frame) {
        final_color = extension.frame_border_color;
    }

    // ============================================================
    // 7. Solar panel cell grid rendering
    // ============================================================
    pbr_input.material.base_color = final_color;
    pbr_input.material.perceptual_roughness = extension.glass_roughness;
    pbr_input.material.reflectance = vec3<f32>(extension.glass_reflectivity);

    var out: FragmentOutput;
    out.color = main_pass_post_lighting_processing(pbr_input, apply_pbr_lighting(pbr_input));
    return out;
}
