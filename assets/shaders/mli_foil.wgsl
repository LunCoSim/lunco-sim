//! Multi-layer insulation (MLI) foil for the general `ShaderMaterial`.
//!
//! The gold "crinkled kapton blanket" every real lander wears. Procedural, in
//! UV space, no textures: a cellular facet pattern (each facet a randomly
//! tilted patch of foil) modulates both albedo and a fake facet normal, so the
//! blanket catches the sun in patches and shifts as the vehicle moves — the
//! signature MLI glitter. Runs one hash-noise evaluation per fragment plus
//! standard PBR; cost is on par with `solar_panel.wgsl`.
//!
//! Dynamic, self-describing parameters: the engine reflects the `Material`
//! struct (field names → offsets) and the `//!@` annotations straight out of
//! this file. Edit live (hot-reload) or via the Inspector / `SetObjectProperty`.

#import bevy_pbr::{
    forward_io::VertexOutput,
    pbr_types,
    pbr_functions,
    mesh_bindings::mesh,
    mesh_view_bindings::view,
}

//!@ui      foil_color   color "Foil colour"
//!@default foil_color   0.83,0.62,0.18
//!@ui      shade_color  color "Crease shade colour"
//!@default shade_color  0.45,0.30,0.08
//!@ui      crinkle      1 64  "Crinkle cells per U"
//!@default crinkle      22
//!@ui      facet_depth  0 1   "Facet contrast"
//!@default facet_depth  0.55
//!@ui      sheen        0 1   "Metallic sheen"
//!@default sheen        0.85
//!@engine  sun_vis
//!@default sun_vis      1
//!@ui      v_scale      0.1 10 "V scale / aspect ratio"
//!@default v_scale      1.0
struct Material {
    foil_color:  vec3<f32>,
    crinkle:     f32,
    shade_color: vec3<f32>,
    facet_depth: f32,
    sheen:       f32,
    sun_vis:     f32,  // engine-filled: horizon-shadow sun visibility
    v_scale:     f32,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(0)
var<uniform> mat: Material;

fn hash2(p: vec2<f32>) -> vec2<f32> {
    // Cheap 2D hash — good enough for facet jitter, stable across frames.
    let q = vec2<f32>(dot(p, vec2<f32>(127.1, 311.7)), dot(p, vec2<f32>(269.5, 183.3)));
    return fract(sin(q) * 43758.5453);
}

@fragment
fn fragment(input: VertexOutput, @builtin(front_facing) is_front: bool) -> @location(0) vec4<f32> {
    let uv = vec2<f32>(input.uv.x, input.uv.y * mat.v_scale);
    let cells = max(mat.crinkle, 1.0);

    // Voronoi-ish facets: nearest jittered cell centre claims the fragment.
    // The facet's hash tilts a pseudo-normal; creases (far from every centre)
    // darken toward `shade_color`.
    let g = uv * cells;
    let base = floor(g);
    var best_d = 8.0;
    var best_h = vec2<f32>(0.0);
    for (var oy = -1; oy <= 1; oy++) {
        for (var ox = -1; ox <= 1; ox++) {
            let cell = base + vec2<f32>(f32(ox), f32(oy));
            let h = hash2(cell);
            let d = distance(g, cell + h);
            if (d < best_d) { best_d = d; best_h = h; }
        }
    }

    // Facet tilt: each foil patch leans its own way; blended into the true
    // normal so PBR sun response varies patch to patch — the MLI glitter.
    let tilt = (best_h - 0.5) * 2.0 * mat.facet_depth;
    let n = normalize(normalize(input.world_normal) + vec3<f32>(tilt.x, tilt.y, tilt.x * tilt.y) * 0.35);

    // Crease shading: cell edges (large best_d) fall toward the shade colour.
    let crease = smoothstep(0.55, 1.0, best_d);
    let facet_lum = mix(1.0, 0.55 + 0.9 * best_h.x, mat.facet_depth);
    let color = mix(mat.foil_color * facet_lum, mat.shade_color, crease * 0.7);

    var pbr_input = pbr_types::pbr_input_new();
    pbr_input.flags = mesh[input.instance_index].flags; // keep SHADOW_RECEIVER
    pbr_input.frag_coord = input.position;
    pbr_input.world_position = input.world_position;
    pbr_input.world_normal = pbr_functions::prepare_world_normal(n, false, is_front);
    pbr_input.is_orthographic = view.clip_from_view[3].w == 1.0;
    pbr_input.N = pbr_input.world_normal;
    pbr_input.V = pbr_functions::calculate_view(input.world_position, pbr_input.is_orthographic);
    pbr_input.material.base_color = vec4(color, 1.0);
    // Foil: metallic, moderately rough so facet glints spread rather than spark.
    pbr_input.material.perceptual_roughness = 0.35;
    pbr_input.material.metallic = mat.sheen;
    pbr_input.material.reflectance = vec3(0.6);

    var out = pbr_functions::apply_pbr_lighting(pbr_input);
    // Smooth horizon-shadow terminator fade (engine-written visibility).
    out = vec4(out.rgb * mat.sun_vis, out.a);
    out = pbr_functions::main_pass_post_lighting_processing(pbr_input, out);
    return out;
}
