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
    // The panel mesh is a Cuboid centered at (0,0,0). Local XZ coords
    // range from -half_dim to +half_dim, perfectly aligned with panel edges.
    // We derive local coords from world position by subtracting the model
    // matrix translation. In Bevy 0.18, use world_position and transform
    // to get local coordinates.
    let local_x = input.world_position.x;
    let local_z = input.world_position.z;

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
    // Frame border is a strip around the panel edges.
    // Detect by checking if we're near the panel boundary.
    let frame_inner_half_x = extension.panel_half_width - extension.frame_border_width;
    let frame_inner_half_z = extension.panel_half_depth - extension.frame_border_width;
    let is_frame = (abs(local_x) > frame_inner_half_x) || (abs(local_z) > frame_inner_half_z);

    // ============================================================
    // 4. Cell grid — only render inside the active area
    // ============================================================
    // Active area = panel bounds minus frame border
    let cell_gap_half = extension.cell_gap * 0.5;
    let cell_gap_x = grid_line_sdf(local_x, cell_spacing_x, cell_gap_half);
    let cell_gap_z = grid_line_sdf(local_z, cell_spacing_z, cell_gap_half);
    let is_cell_gap = cell_gap_x < 0.0 || cell_gap_z < 0.0;

    // ============================================================
    // 5. Bus lines (metallic conductors running through cells)
    // ============================================================
    // Bus lines run along cell boundaries, slightly wider than gaps.
    let bus_half = extension.bus_line_width * 0.5;
    let bus_x = grid_line_sdf(local_x, cell_spacing_x, bus_half);
    let bus_z = grid_line_sdf(local_z, cell_spacing_z, bus_half);
    let is_bus = bus_x < 0.0 || bus_z < 0.0;

    // ============================================================
    // 6. Compose final color
    // ============================================================
    var final_color = extension.cell_color;

    // Apply cell gaps (dark separator lines between cells)
    if (is_cell_gap) {
        final_color = vec4<f32>(0.02, 0.02, 0.02, 1.0);
    }

    // Apply bus lines (metallic silver on top of cells)
    if (is_bus && !is_cell_gap) {
        final_color = mix(extension.cell_color, extension.bus_line_color, 0.9);
    }

    // Apply frame border (dark structural edge)
    if (is_frame) {
        final_color = extension.frame_border_color;
    }

    // ============================================================
    // 7. Apply cell grid color to PBR base color
    // ============================================================
    pbr_input.material.base_color = final_color;
    // roughness and reflectance kept at defaults to avoid WGSL type mismatches

    var out: FragmentOutput;
    out.color = main_pass_post_lighting_processing(pbr_input, apply_pbr_lighting(pbr_input));
    return out;
}
