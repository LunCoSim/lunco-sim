//! Engine-exhaust plume for the general `ShaderMaterial`.
//!
//! The bound `Cone` is a FIXED BOUNDING VOLUME, authored at the plume's
//! full-throttle extent and never transformed again. Everything the plume does —
//! how far it reaches, how wide it blooms, how it shimmers — happens inside that
//! volume, here, from `throttle`.
//!
//! `throttle` is driven per-instance through `float inputs:throttle.connect` on
//! the bound gprim, straight off the vessel's own `throttle` output. The plume is
//! therefore a CONSEQUENCE of the engine's commanded state, on the same tick and
//! by the same number the vessel published.
//!
//! ## Why this ray-marches instead of shading the cone's skin
//!
//! Throttle changes the plume's LENGTH and its WIDTH, and a narrower plume is
//! strictly inside the bounding cone — it touches that cone's surface nowhere. So
//! there is no way to draw it by tinting the surface the rasteriser hands us: the
//! shape being drawn is a different, smaller cone that lives in the volume. Each
//! front-face fragment is therefore an entry point, and the fragment marches the
//! view ray through the bounding volume accumulating emission and extinction from
//! the CURRENT plume cone. That also gives the plume real depth — it is brighter
//! along the line of sight through its axis, which a skin never is.
//!
//! ## The shape, and where its numbers come from
//!
//! In mesh-local space the cone is `radius 1, height 1`: apex at `y = +0.5`, base
//! at `y = -0.5`, and its radius at height `y` is `0.5 - y`. The prim's authored
//! 180° flip puts the apex DOWNSTREAM, so with `a = y + 0.5` running 0 at the
//! nozzle end to 1 at the tip, the bounding surface is `r = 1 - a`.
//!
//! The current plume is the cone of length `a <= len` and half-width
//! `wid * (1 - a/len)`, where
//!
//!     len = throttle                       (normalised to the authored volume)
//!     wid = width_idle + (1 - width_idle) * throttle
//!
//! Width blooms fast and then saturates; length tracks throttle. Both are
//! FRACTIONS of the authored volume, which is what keeps the per-instance sizing
//! in USD — the outer shroud and the inner core differ only in their prim's
//! scale, and this file has no opinion about either.
//!
//! ## Flicker
//!
//! A steady plume reads as a decal. The shimmer is procedural value noise
//! (`lunco::noise`) advected along the plume axis by `globals.time`, so it is a
//! function of position and time evaluated per fragment — no state, no per-tick
//! script, and identical on every machine that renders the same second.
//!
//! It modulates DOWNWARD only (`1 - depth * …`). That is deliberate: the authored
//! cone is the full-throttle bound and the photometry model derives the plume's
//! light from that same bound, so a flicker that could overshoot would put light
//! outside the volume that emits it.
//!
//! ## The light is not in here
//!
//! Emissive geometry in a forward renderer illuminates nothing, so the plume's
//! `PointLight` is a separate prim driven from `LunCo.Propulsion.PlumePhotometry`.
//! Its colour is authored on that light (`inputs:color`) and must be kept as the
//! chroma of `core_color` below; its luminance parameter must be kept as
//! `core_color`'s Rec.709 luma. A shader parameter is deliberately not readable as
//! a connection source — that is what stops a render value feeding back into the
//! simulation — so this coupling is authored, not wired.
//!
//! Dynamic, self-describing parameters: the engine reflects the `Material`
//! struct (field names → offsets) and the `//!@` annotations straight out of
//! this file. Edit live (hot-reload) or via the Inspector / `SetObjectProperty`.

#import bevy_pbr::{
    mesh_functions,
    forward_io::VertexOutput,
    mesh_view_bindings::view,
    mesh_view_bindings::globals,
}
#import lunco::pbr_lit::lit
#import lunco::noise::vnoise

//!@ui      core_color    color "Axial colour (hot core)"
//!@default core_color    6.0,3.5,0.9
//!@ui      throttle      0 1   "Throttle (driven by the engine)"
//!@default throttle      0.0
//!@ui      edge_color    color "Flank colour (cooler outer gas)"
//!@default edge_color    3.0,1.0,0.12
//!@ui      width_idle    0 1   "Half-width fraction at zero throttle"
//!@default width_idle    0.28
//!@ui      flicker       0 1   "Flicker depth; 0 = steady"
//!@default flicker       1.0
//!@ui      flicker_speed 0 20  "Flicker advection speed along the axis"
//!@default flicker_speed 6.0
//!@ui      flicker_scale 0 40  "Flicker cell count across the plume"
//!@default flicker_scale 7.0
//!@ui      density       0 40  "Emission / extinction gain per unit local depth"
//!@default density       9.0
//!@ui      steps         4 64  "Ray-march samples through the volume"
//!@default steps         24
struct Material {
    core_color:    vec3<f32>,
    throttle:      f32,
    edge_color:    vec3<f32>,
    width_idle:    f32,
    flicker:       f32,
    flicker_speed: f32,
    flicker_scale: f32,
    density:       f32,
    steps:         f32,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(0)
var<uniform> mat: Material;

/// Plume density at a mesh-local point — 0 outside the current plume cone.
///
/// `len` and `wid` are the current plume's length and half-width as fractions of
/// the authored bounding volume, so this function is the shape and nothing else.
fn plume_density(p: vec3<f32>, len: f32, wid: f32) -> f32 {
    // Distance along the plume, 0 at the nozzle end, 1 at the bounding apex.
    let a = p.y + 0.5;
    let r = length(p.xz);
    // Outside the authored volume — the march is stepping through empty space
    // either side of the cone.
    if (a < 0.0 || a > 1.0 || r > 1.0 - a) {
        return 0.0;
    }
    // Station along the CURRENT plume, which is shorter than the volume.
    let ax = a / len;
    if (ax > 1.0) {
        return 0.0;
    }
    let half_width = wid * (1.0 - ax);
    let rn = r / max(half_width, 1e-4);
    if (rn > 1.0) {
        return 0.0;
    }

    // Radially: densest on the axis, falling to nothing at the flank. Axially:
    // thinning toward the tip, where the gas has expanded and cooled.
    let radial = 1.0 - rn * rn;
    let axial = 1.0 - ax * ax;
    var d = radial * axial;

    // Turbulence advected downstream. `a` enters the noise domain with time so
    // the cells travel WITH the exhaust rather than boiling in place.
    let n = vnoise(vec3<f32>(
        p.x * mat.flicker_scale,
        p.z * mat.flicker_scale,
        a * mat.flicker_scale + globals.time * mat.flicker_speed
    ));
    d *= 1.0 - clamp(mat.flicker, 0.0, 1.0) * 0.7 * (1.0 - n);

    return max(d, 0.0);
}

@fragment
fn fragment(input: VertexOutput, @builtin(front_facing) is_front: bool) -> @location(0) vec4<f32> {
    let t = clamp(mat.throttle, 0.0, 1.0);
    // A dead engine emits NOTHING — not a residual glow. The photometry model
    // gates its light to exactly zero at zero throttle for the same reason, and
    // the two must agree or a coasting shot picks up a plume with no light or a
    // light with no plume.
    if (t <= 0.0) {
        return vec4<f32>(0.0);
    }

    let len = max(t, 1e-3);
    let wid = mat.width_idle + (1.0 - mat.width_idle) * t;

    // Mesh-local ray. The fragment is on the volume's front face, so it IS the
    // entry point; the camera gives the direction.
    let local_from_world = mesh_functions::get_local_from_world(input.instance_index);
    let entry = (local_from_world * vec4<f32>(input.world_position.xyz, 1.0)).xyz;
    let eye = (local_from_world * vec4<f32>(view.world_position, 1.0)).xyz;
    let dir = normalize(entry - eye);

    // The longest chord of a unit-radius, unit-height cone is under 2.3; 2.5 is a
    // bound that needs no per-instance number.
    let steps = i32(clamp(mat.steps, 4.0, 64.0));
    let dt = 2.5 / f32(steps);

    var optical_depth = 0.0;
    var emitted = vec3<f32>(0.0);
    for (var i = 0; i < steps; i++) {
        // Half-step offset: sampling at the segment midpoint, so the entry face
        // itself does not get double weight.
        let p = entry + dir * (dt * (f32(i) + 0.5));
        let d = plume_density(p, len, wid);
        if (d <= 0.0) {
            continue;
        }
        // Colour by how far off-axis the sample is: the core stays yellow-white,
        // the flank cools to orange. This is a LOOK decision and belongs here.
        let tint = mix(mat.edge_color, mat.core_color, d);
        optical_depth += d * dt;
        emitted += tint * d * dt;
    }
    if (optical_depth <= 0.0) {
        return vec4<f32>(0.0);
    }

    // Beer–Lambert coverage, so a long line of sight through the axis saturates
    // instead of running away, and a grazing one stays thin.
    let alpha = 1.0 - exp(-mat.density * optical_depth);
    let emissive = emitted * mat.density;

    // Black albedo: the plume is a source, not a surface, so the sun must not add
    // a diffuse term to it. Routing through `lit` still gets the emissive the same
    // fog and tonemapping treatment as every other surface in frame.
    let c = lit(input, is_front, vec3<f32>(0.0), 1.0, 0.0, emissive);
    return vec4<f32>(c.rgb, alpha);
}
