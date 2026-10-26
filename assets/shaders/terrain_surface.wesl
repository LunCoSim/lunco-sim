import bevy_pbr::render::{
    pbr_lighting as lighting,
    mesh_functions,
    mesh_view_bindings::{lights, view},
    mesh_view_types,
    mesh_types,
    pbr_functions,
    pbr_types,
    shadows,
};
@if((LUNCO_NOISE_2D))
import super::lunco_noise::{fbm2d, fbm2d_gradient};
@if((!(LUNCO_NOISE_2D)))
import super::lunco_noise::{fbm_rot, fbm_rot_gradient};
import super::lunco_noise::noise_footprint_weight;

// Shared regolith SURFACE kernel for every terrain shader, via WESL import.
//
// This module is the single implementation of shared terrain surface operations.
// The material pipeline selects the noise dimensionality for the target platform
// with `LUNCO_NOISE_2D`; every terrain material then imports the same transfer,
// anti-aliasing, map, and bump helpers.




// Footprint fade thresholds, in screen pixels per noise period.
//
// Filter each octave between two and four pixels per period. This retains
// resolved close grain while removing subpixel slopes independently of the
// coarser octaves. Unresolved slope energy moves into GGX roughness.
const AA_CUT_PX: f32 = 2.0;
const AA_RAMP_PX: f32 = 2.0;
const BUMP_MAX_SLOPE: f32 = 0.65;

// Production and diagnostic vertices share the same camera-distance morph and
// parent-edge stitch law in forward, prepass, and shadow views.
fn terrain_morph_weight(
    world_position: vec3<f32>, eye: vec3<f32>, start: f32, end: f32,
    edge_mask: vec4<f32>, stitch_edges: vec4<f32>,
) -> f32 {
    var morph = 0.0;
    if (end > start) {
        morph = smoothstep(start, end, distance(world_position, eye));
    }
    // A fine edge meets its coarser neighbour on their shared parent lattice.
    let edge = edge_mask * stitch_edges;
    return max(morph, max(max(edge.x, edge.y), max(edge.z, edge.w)));
}

/// C1 material fade across a rectangular crop. Multiplying the axial weights
/// keeps the corner diagonal smooth and reaches zero on every exterior edge.
/// Width order is west, east, south, north, matching the geometry owner.
fn terrain_continuation_weight(uv: vec2<f32>, half_extent: f32, widths: vec4<f32>) -> f32 {
    let outside = max(abs(uv - vec2(0.5)) - vec2(0.5), vec2(0.0)) * 2.0 * half_extent;
    let width = vec2(select(widths.y, widths.x, uv.x < 0.0),
                     select(widths.z, widths.w, uv.y < 0.0));
    let weight = 1.0 - smoothstep(vec2(0.0), vec2(1.0), outside / max(width, vec2(1e-6)));
    return weight.x * weight.y;
}

/// Preserve the measured crop's texture footprint, then progressively average
/// its edge outside coverage. A clamped coordinate alone stretches individual
/// edge texels across the entire transition surface. Both continuation materials
/// use this footprint for colour and surface maps before their spatial fade.
fn terrain_continuation_gradients(uv: vec2<f32>) -> mat2x2<f32> {
    let dx = dpdx(uv);
    let dy = dpdy(uv);
    let outside = max(abs(uv - vec2(0.5)) - vec2(0.5), vec2(0.0));
    let blur = 2.0 * length(outside);
    if (blur > max(length(dx), length(dy))) {
        return mat2x2(vec2(blur, 0.0), vec2(0.0, blur));
    }
    return mat2x2(dx, dy);
}

/// Linear, clamped remap for authored colour layers. Relief applies its own
/// smooth height remap in `layer_gradient` so its normal remains continuous.
fn ramp(x: f32, lo: f32, hi: f32) -> f32 {
    return saturate((x - lo) / (hi - lo));
}

/// Footprint-based detail fade. `pw` is the world width of one pixel at the shading
/// point; `scale` is the layer's spatial frequency in 1/m. Returns 0 where the layer
/// would alias, 1 where it is comfortably resolved.
///
/// Note this is a HIGH-pass on period, not a distance fade: up close `pw` is small,
/// so the result saturates at 1 and every layer is fully on.
fn aa_fade(scale: f32, pw: f32) -> f32 {
    return noise_footprint_weight(scale * pw, vec2(AA_CUT_PX, AA_RAMP_PX));
}

/// The platform octave budget. Call sites pass the native octave count; the web
/// path uses a reduced budget for its 2D noise implementation.
fn oct(full: i32) -> i32 {
    @if((LUNCO_NOISE_2D)) return max(1, full / 2);
    @if((!(LUNCO_NOISE_2D))) return full;
}

/// Baked-map blend weights as a function of `r` — the ratio of one fragment's
/// screen-space WORLD footprint to the derived map's physical TEXEL spacing.
/// Returns `(weight_normal, weight_ao, weight_tone)`.
///
///   * normal fades IN when a pixel covers at least one map texel and OFF when the
///     view resolves below the map — blending a coarser normal there would only
///     blur the geometry and close-range procedural detail.
///   * AO and tone are physical surface data, so their weights are exactly one at
///     every distance. Texture mips filter their frequency; the camera must not
///     change their energy.
///
/// LIVES IN THE SHADER and is evaluated PER FRAGMENT because appearance must be
/// continuous when CDLOD substitutes one mesh depth for another. CPU-derived
/// per-tile weights and the later depth-plus-morph ratio both encoded topology in
/// the material, producing square changes in AO, tone and normal blending. The
/// fragment footprint is the renderer-standard detail signal and has no tile
/// identity to leak into the result.
fn map_weights(r: f32) -> vec3<f32> {
    let w_normal = clamp((r - 0.75) / 1.5, 0.0, 1.0);
    return vec3(w_normal, 1.0, 1.0);
}

/// Resolve one terrain material's map roles from the shared CPU source contract.
///
/// The streamed CDLOD and static-mesh paths must make the same decision when a
/// role is authored by USD versus supplied by the DEM bake. Keeping this in the
/// shared surface module prevents a path from silently treating a bound derived
/// texture as authored (or ignoring it altogether). The returned lanes are
/// `(normal, roughness, AO, relief-tone)` weights. Relief tone is a derived
/// colour fallback, so an authored albedo suppresses it by the same amount as
/// the procedural colour layers.
fn terrain_map_weights(
    map_footprint: f32,
    derived_surface_on: f32,
    derived_normal_on: f32,
    authored_surface_on: f32,
    authored_normal_on: f32,
    authored_albedo_weight: f32,
    authored_rough: f32,
    authored_ao: f32,
    authored_normal: f32,
) -> vec4<f32> {
    let derived = map_weights(map_footprint);
    var weight_normal = derived.x * derived_normal_on;
    var weight_ao = derived.y * derived_surface_on;
    var weight_rough = 0.35 * derived.y * derived_surface_on;
    var weight_tone = derived.z * derived_normal_on
        * (1.0 - clamp(authored_albedo_weight, 0.0, 1.0));
    if (authored_normal_on > 0.5) {
        weight_normal = authored_normal;
        weight_tone = 0.0;
    }
    if (authored_surface_on > 0.5) {
        weight_rough = authored_rough;
        weight_ao = authored_ao;
    }
    return vec4(weight_normal, weight_rough, weight_ao, weight_tone);
}

/// Resolve the packed surface map's ambient-occlusion channel.
///
/// AO is indirect-light visibility, not base colour. Keep this transfer shared
/// so the static and streamed terrain paths agree on the source semantics, then
/// feed the result to Bevy's `PbrInput.diffuse_occlusion`. Multiplying it into
/// albedo makes a low-frequency derived field look like broad paint patches and
/// also darkens direct sunlight, which is not what ambient occlusion means.
fn terrain_surface_occlusion(
    surface: vec4<f32>,
    weight_ao: f32,
    authored_surface_on: f32,
) -> f32 {
    var occlusion = mix(1.0, 0.4 + 0.6 * surface.g, weight_ao);
    if (authored_surface_on > 0.5) {
        occlusion = mix(1.0, surface.g, weight_ao);
    }
    return clamp(occlusion, 0.0, 1.0);
}

/// Apply lunar photometry and heightfield visibility to the engine-selected
/// Sun contribution only.
///
/// Bevy's `apply_pbr_lighting` returns the sum of direct and indirect light.
/// Applying the lunar response to `base_color` would also scale earthshine,
/// environment fill, and every other light. Rebuild the canonical Sun term,
/// including Bevy's native CSM shadow, and replace only that term. Ambient,
/// environment, and other authored lights remain untouched.
///
/// The CPU writes `sun_dir_world` from the structural Sun selection used by the
/// rest of the renderer. Matching that direction in Bevy's flat light buffer
/// keeps the fill light out of the terrain self-shadow term without assuming a
/// directional-light array index.
fn terrain_apply_sun_response(
    pbr_input: pbr_types::PbrInput,
    color: vec4<f32>,
    sun_dir_world: vec3<f32>,
    lunar_factor: f32,
    visibility: f32,
    blend: f32,
) -> vec4<f32> {
    let sun_length_sq = dot(sun_dir_world, sun_dir_world);
    if (sun_length_sq < 0.25) {
        return color;
    }

    let sun_dir = normalize(sun_dir_world);
    let output_color = pbr_input.material.base_color;
    let metallic = pbr_input.material.metallic;
    let perceptual_roughness = pbr_input.material.perceptual_roughness;
    let roughness = lighting::perceptualRoughnessToRoughness(perceptual_roughness);
    let NdotV = max(dot(pbr_input.N, pbr_input.V), 0.0001);
    let R = reflect(-pbr_input.V, pbr_input.N);
    let diffuse_color = pbr_functions::calculate_diffuse_color(
        output_color.rgb,
        metallic,
        pbr_input.material.specular_transmission,
        pbr_input.material.diffuse_transmission,
    );

    var lighting_input: lighting::LightingInput;
    lighting_input.layers[lighting::LAYER_BASE].NdotV = NdotV;
    lighting_input.layers[lighting::LAYER_BASE].N = pbr_input.N;
    lighting_input.layers[lighting::LAYER_BASE].R = R;
    lighting_input.layers[lighting::LAYER_BASE].perceptual_roughness = perceptual_roughness;
    lighting_input.layers[lighting::LAYER_BASE].roughness = roughness;
    lighting_input.P = pbr_input.world_position.xyz;
    lighting_input.V = pbr_input.V;
    lighting_input.diffuse_color = diffuse_color;
    lighting_input.metallic = metallic;
    lighting_input.F0_dielectric = pbr_functions::calculate_F0_dielectric(
        pbr_input.material.reflectance);
    lighting_input.F0_metallic = output_color.rgb;
    lighting_input.F_ab = lighting::F_AB(perceptual_roughness, NdotV);

    @if((STANDARD_MATERIAL_CLEARCOAT)) let clearcoat = pbr_input.material.clearcoat;
    @if((STANDARD_MATERIAL_CLEARCOAT)) let clearcoat_perceptual_roughness =
        pbr_input.material.clearcoat_perceptual_roughness;
    @if((STANDARD_MATERIAL_CLEARCOAT)) let clearcoat_roughness =
        lighting::perceptualRoughnessToRoughness(clearcoat_perceptual_roughness);
    @if((STANDARD_MATERIAL_CLEARCOAT)) let clearcoat_N = pbr_input.clearcoat_N;
    @if((STANDARD_MATERIAL_CLEARCOAT)) lighting_input.layers[lighting::LAYER_CLEARCOAT].NdotV =
        max(dot(clearcoat_N, pbr_input.V), 0.0001);
    @if((STANDARD_MATERIAL_CLEARCOAT)) lighting_input.layers[lighting::LAYER_CLEARCOAT].N = clearcoat_N;
    @if((STANDARD_MATERIAL_CLEARCOAT)) lighting_input.layers[lighting::LAYER_CLEARCOAT].R =
        reflect(-pbr_input.V, clearcoat_N);
    @if((STANDARD_MATERIAL_CLEARCOAT)) lighting_input.layers[lighting::LAYER_CLEARCOAT].perceptual_roughness =
        clearcoat_perceptual_roughness;
    @if((STANDARD_MATERIAL_CLEARCOAT)) lighting_input.layers[lighting::LAYER_CLEARCOAT].roughness = clearcoat_roughness;
    @if((STANDARD_MATERIAL_CLEARCOAT)) lighting_input.clearcoat_strength = clearcoat;

    @if((STANDARD_MATERIAL_ANISOTROPY)) lighting_input.anisotropy = pbr_input.anisotropy_strength;
    @if((STANDARD_MATERIAL_ANISOTROPY)) lighting_input.Ta = pbr_input.anisotropy_T;
    @if((STANDARD_MATERIAL_ANISOTROPY)) lighting_input.Ba = pbr_input.anisotropy_B;

    let view_z = dot(vec4<f32>(
        view.view_from_world[0].z,
        view.view_from_world[1].z,
        view.view_from_world[2].z,
        view.view_from_world[3].z,
    ), pbr_input.world_position);
    var sun_direct = vec3<f32>(0.0);
    for (var i: u32 = 0u; i < lights.n_directional_lights; i = i + 1u) {
        let light = &lights.directional_lights[i];
        if (dot(normalize(light.direction_to_light), sun_dir) < 0.9995) {
            continue;
        }

        var native_shadow = 1.0;
        if ((pbr_input.flags & mesh_types::MESH_FLAGS_SHADOW_RECEIVER_BIT) != 0u
                && (light.flags & mesh_view_types::DIRECTIONAL_LIGHT_FLAGS_SHADOWS_ENABLED_BIT) != 0u) {
            native_shadow = shadows::fetch_directional_shadow(
                i,
                pbr_input.world_position,
                pbr_input.world_normal,
                view_z,
                pbr_input.frag_coord.xy,
            );
        }
        sun_direct += lighting::directional_light(i, &lighting_input, true)
            * native_shadow;
    }

    // `directional_light` is a radiance contribution and must be non-negative.
    // Keep that invariant explicit at this extension boundary: a malformed or
    // transient material value must not turn reducing the Sun term into a bright
    // pixel. The native PBR result remains the source of ambient/earthshine.
    // Bevy applies view exposure to the complete direct-light accumulator in
    // `apply_pbr_lighting`. Apply the same factor before subtracting the
    // heightfield-occluded portion; otherwise a deep shadow subtracts a term
    // larger than the native PBR contribution and clamps the terrain to black.
    let sun_radiance = max(sun_direct * view.exposure, vec3<f32>(0.0));
    let terrain_visibility = mix(1.0, clamp(visibility, 0.0, 1.0), clamp(blend, 0.0, 1.0));
    let sun_response = clamp(lunar_factor, 0.0, 8.0) * terrain_visibility;
    return vec4(max(
        color.rgb + sun_radiance * (sun_response - 1.0),
        vec3<f32>(0.0),
    ), color.a);
}

/// Decode the normal-map convention shared by the DEM baker and terrain
/// shaders.  The result is in the DEM's local ENU frame, not in whichever
/// floating render frame is active for the current camera.
fn decode_dem_normal(encoded: vec3<f32>) -> vec3<f32> {
    return normalize(encoded * 2.0 - 1.0);
}

/// Convert a baked DEM-local ENU normal into the current render world through
/// the mesh instance.  This is the one coordinate boundary for derived terrain
/// normals: static meshes and streamed BigSpace tiles must both use it before
/// combining a map normal with `VertexOutput.world_normal` or scene lighting.
fn dem_normal_to_world(encoded: vec3<f32>, instance_index: u32) -> vec3<f32> {
    return mesh_functions::mesh_normal_local_to_world(
        decode_dem_normal(encoded), instance_index);
}

/// Raw FBM at a terrain-stable position, platform-correct. Use this for the un-ramped
/// tonal layers (dust wash, metre-scale grain) so they pick up the same noise
/// family and octave budget as the bump layers instead of calling `fbm`/`fbm2d`
/// directly, keeping all terrain materials on the same noise family and budget.
fn surface_fbm(p: vec3<f32>, octaves: i32, gain: f32) -> f32 {
    @if((LUNCO_NOISE_2D)) return fbm2d(p.xz, oct(octaves), gain);
    @if((!(LUNCO_NOISE_2D))) return fbm_rot(p, oct(octaves), gain);
}

// Procedural terrain detail is anchored to the authored DEM frame, not to the
// transient render-world frame. BigSpace rebases world positions as the camera
// and body move; sampling FBM from `VertexOutput.world_position` therefore makes
// the material slide and re-evaluate at different noise coordinates every frame.
// UVs are the existing DEM-global coordinate carried by both terrain meshes, so
// this stays batched and needs no per-tile material or new vertex attribute.
fn terrain_detail_position(uv: vec2<f32>, half_extent: f32) -> vec3<f32> {
    return vec3(
        (uv.x * 2.0 - 1.0) * half_extent,
        0.0,
        (uv.y * 2.0 - 1.0) * half_extent,
    );
}

// The procedural detail coordinate is DEM-local. Transform the interpolated
// render normal through the same mesh instance before bumping it, then cross the
// one boundary back to render-world once the local perturbation is complete.
fn terrain_detail_normal_to_local(world_normal: vec3<f32>, instance_index: u32) -> vec3<f32> {
    // Bevy maps local normals with inverse-transpose(M). Its inverse is
    // transpose(M), including when the terrain has nonuniform scale.
    let transform = mesh_functions::get_world_from_local(instance_index);
    let linear = mat3x3(transform[0].xyz, transform[1].xyz, transform[2].xyz);
    return normalize(transpose(linear) * world_normal);
}

fn terrain_detail_normal_to_world(local_normal: vec3<f32>, instance_index: u32) -> vec3<f32> {
    return mesh_functions::mesh_normal_local_to_world(local_normal, instance_index);
}

/// One ramped FBM layer sampled at terrain-stable position `p`.
fn layer_height(p: vec3<f32>, scale: f32, octaves: i32, gain: f32, lo: f32, hi: f32) -> f32 {
    @if((LUNCO_NOISE_2D)) return ramp(fbm2d(p.xz * scale, oct(octaves), gain), lo, hi);
    @if((!(LUNCO_NOISE_2D))) return ramp(fbm_rot(p * scale, oct(octaves), gain), lo, hi);
}

/// Value and gradient of a smooth, bounded FBM relief layer. The cubic height
/// remap brings its slope continuously to zero at both clipping thresholds.
/// Colour-only layers retain the linear `ramp`; relief needs a continuous normal.
/// The gradient is with respect to the input position in inverse metres.
fn layer_gradient(
    p: vec3<f32>, scale: f32, octaves: i32, gain: f32, lo: f32, hi: f32, pw: f32,
) -> vec4<f32> {
    var raw_height = 0.0;
    var raw_gradient = vec3<f32>(0.0);
    @if((LUNCO_NOISE_2D)) let sample = fbm2d_gradient(p.xz * scale, oct(octaves), gain,
        scale * pw, vec2(AA_CUT_PX, AA_RAMP_PX));
    @if((LUNCO_NOISE_2D)) raw_height = sample.x;
    @if((LUNCO_NOISE_2D)) raw_gradient = vec3(sample.y, 0.0, sample.z) * scale;
    @if((!(LUNCO_NOISE_2D))) let sample = fbm_rot_gradient(p * scale, oct(octaves), gain,
        scale * pw, vec2(AA_CUT_PX, AA_RAMP_PX));
    @if((!(LUNCO_NOISE_2D))) raw_height = sample.x;
    @if((!(LUNCO_NOISE_2D))) raw_gradient = sample.yzw * scale;
    let t = ramp(raw_height, lo, hi);
    let height = t * t * (3.0 - 2.0 * t);
    let gradient = raw_gradient * (6.0 * t * (1.0 - t) / (hi - lo));
    return vec4(height, gradient);
}

/// Fold an estimate of each octave's removed slope variance into GGX roughness.
/// The squared resolution weight matches the explicit gradient's energy;
/// normalization uses the complete authored spectrum, just like the FBM.
fn filter_detail_roughness(
    perceptual_roughness: f32, amplitude_m: f32, scale_per_m: f32,
    octaves: i32, gain: f32, pw: f32,
) -> f32 {
    var variance = 0.0;
    var amp = 1.0;
    var frequency = 1.0;
    var total = 0.0;
    for (var o = 0; o < oct(octaves); o++) {
        let weight = aa_fade(scale_per_m * frequency, pw);
        let slope = amp * frequency;
        variance += slope * slope * (1.0 - weight * weight);
        total += amp;
        amp *= gain;
        frequency *= 2.0;
    }
    let unresolved_rms_slope = 0.5 * amplitude_m * scale_per_m * sqrt(variance) / total;
    let alpha = clamp(perceptual_roughness, 0.05, 1.0);
    let alpha_sq = alpha * alpha;
    let filtered_alpha_sq = alpha_sq * alpha_sq
        + unresolved_rms_slope * unresolved_rms_slope;
    return sqrt(min(sqrt(filtered_alpha_sq), 1.0));
}

/// Perturb shading normal `n` by the gradient of one noise layer, and report that
/// layer's height through `out_h` so the caller can reuse it for albedo/roughness
/// without paying for a second FBM.
///
/// Uses the shared analytic gradient, so one noise-stack evaluation supplies
/// both the height and its shading slope. Projecting onto the tangent plane
/// keeps the perturbation perpendicular to the current surface normal.
fn bump_layer(
    n: vec3<f32>, p: vec3<f32>,
    scale: f32, octaves: i32, gain: f32, lo: f32, hi: f32,
    strength: f32, pw: f32, out_h: ptr<function, f32>,
) -> vec3<f32> {
    let sample = layer_gradient(p, scale, octaves, gain, lo, hi, pw);
    *out_h = sample.x;
    var surface_gradient = sample.yzw;
    // UV-backed terrain detail uses a planar DEM-local xz parameterization.
    // Do not add the unused 3D noise derivative along the constant local-y axis.
    @if((VERTEX_UVS_A)) surface_gradient.y = 0.0;
    let tangent_gradient = surface_gradient - n * dot(n, surface_gradient);
    let bump_slope = strength * tangent_gradient;
    // Smoothly saturate steep micro-relief slopes. Returning the base normal
    // only where a gradient crossed the hemisphere boundary made the noise
    // field break into visible patches under grazing light.
    let slope_scale = inverseSqrt(
        1.0 + dot(bump_slope, bump_slope) / (BUMP_MAX_SLOPE * BUMP_MAX_SLOPE),
    );
    let perturbed = n - bump_slope * slope_scale;
    if (length(perturbed) < 1e-3 || dot(perturbed, n) <= 0.0) {
        return n;
    }
    return normalize(perturbed);
}

// Sparse terrain-local stroke index. Header, cell lists and f64->f32 validation
// are owned by lunco-terrain-surface::annotations; camera/LOD never rebake it.
fn annotation_texel(data: texture_2d<f32>, index: u32) -> vec4<f32> {
    return textureLoad(data, vec2<i32>(i32(index % 256u), i32(index / 256u)), 0);
}

fn surface_annotation_color(data: texture_2d<f32>, p: vec2<f32>, base: vec4<f32>) -> vec4<f32> {
    // Derivatives precede nonuniform bounds/cell branches. The stroke remains
    // antialiased at grazing angles and when its physical width is subpixel.
    let footprint = fwidth(p);
    if (textureDimensions(data).x != 256u) { return base; }
    let bounds = annotation_texel(data, 0u);
    if (any(p < bounds.xy - footprint) || any(p > bounds.zw + footprint)) { return base; }
    let header = annotation_texel(data, 1u);
    let grid = u32(header.x);
    let normalized = clamp((p - bounds.xy) / (bounds.zw - bounds.xy), vec2<f32>(0.0), vec2<f32>(1.0));
    let cell_xy = min(vec2<u32>(normalized * f32(grid)), vec2<u32>(grid - 1u));
    var cell = annotation_texel(data, 2u + cell_xy.y * grid + cell_xy.x);
    var lo = bounds.xy + vec2<f32>(cell_xy) * (bounds.zw - bounds.xy) / f32(grid);
    var hi = lo + (bounds.zw - bounds.xy) / f32(grid);
    for (var depth = 0u; depth < u32(header.y); depth += 1u) {
        if (cell.y >= 0.0) { break; }
        let middle = (lo + hi) * 0.5;
        let quadrant = vec2<u32>(select(vec2<u32>(0u), vec2<u32>(1u), p >= middle));
        cell = annotation_texel(data, u32(cell.x) + quadrant.y * 2u + quadrant.x);
        lo = select(lo, middle, quadrant == vec2<u32>(1u));
        hi = select(middle, hi, quadrant == vec2<u32>(1u));
    }
    var tint = vec4<f32>(0.0);
    for (var i = 0u; i < u32(cell.y); i += 1u) {
        let segment = u32(annotation_texel(data, u32(cell.x) + i).x);
        let record = segment;
        let endpoints = annotation_texel(data, record);
        let delta = endpoints.zw - endpoints.xy;
        let t = clamp(dot(p - endpoints.xy, delta) / dot(delta, delta), 0.0, 1.0);
        let dist = length(p - endpoints.xy - delta * t);
        let radius = annotation_texel(data, record + 1u).x;
        let right = abs(vec2<f32>(-delta.y, delta.x) / length(delta));
        let aa = max(dot(right, footprint) * 0.5, 0.00001);
        let coverage = clamp((radius - dist + aa) / (2.0 * aa), 0.0, 1.0)
            - clamp((-radius - dist + aa) / (2.0 * aa), 0.0, 1.0);
        let color = annotation_texel(data, record + 2u);
        let alpha = coverage * color.a;
        // A joint is one stroke, not several overlapping transparent quads.
        if (alpha >= tint.a) { tint = vec4<f32>(color.rgb, alpha); }
    }
    return vec4<f32>(mix(base.rgb, tint.rgb, tint.a), base.a);
}
