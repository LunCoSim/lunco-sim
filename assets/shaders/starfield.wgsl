//! Procedural night sky for the general `ShaderMaterial` — stars evaluated
//! **per view direction**, with no texture anywhere in the pipeline.
//!
//! NOT AN ASTROMETRIC CATALOGUE. Star positions here are pseudo-random, derived
//! from a spatial hash. This exists so lunar scenes stop rendering against a pure
//! black void; it is a marketing/visual asset and nothing should ever navigate by
//! it or use it for star-tracker work.
//!
//! # Why a shader and not a baked equirectangular image
//!
//! This replaces `textures/starfield_4k.png` and the offline Python generator
//! that wrote it. A committed PNG freezes every parameter at generation time,
//! puts a second implementation of the sky outside the engine, and means the sky
//! cannot be tuned from the scene. Here the `.usda` authors the numbers and the
//! WGSL is the single source of truth — edit `inputs:*` on the `Shader` prim (or
//! this file, which hot-reloads) and the sky changes.
//!
//! It also makes a whole class of artifact structurally impossible. The baked
//! path had two: a longitude seam, because the generator's `fbm` was not periodic
//! at the wrap meridian; and unexplained rectangular brightness terraces on
//! cubemap face boundaries whenever the diffuse Milky Way band was enabled. Both
//! need an equirect parameterisation, a wrap meridian, a resampling step and an
//! 8-bit encode. This shader has none of them: it is a continuous function of a
//! `vec3` direction, evaluated at full float precision, once per pixel.
//!
//! # Physical shaping (carried over from the generator, which worked it out)
//!
//!   * Stars are distributed **uniformly on the sphere**. The hash grid below is
//!     a uniform 3D lattice restricted to one radial shell, so the stars it emits
//!     are uniform *per unit volume of that shell* — which, projected radially, is
//!     uniform *per steradian*. No polar pile-up, and no direction is special.
//!   * The magnitude distribution follows **N(<m) ∝ 10^(0.6 m)** — each magnitude
//!     step is ~4x more stars, and each is 10^0.4 = 2.512x fainter. Hence a few
//!     bright anchors and thousands of faint ones. This is sampled *exactly* by
//!     inverting the CDF (see `star_flux`), not approximated with a `u^7` curve
//!     the way the generator did.
//!   * Star colours span the **B-V** range of naked-eye stars: hot blue-white
//!     (O/B, ~20 000 K) through white (A/F, ~7000 K) to warm orange (K/M,
//!     ~3500 K). Most naked-eye stars read white, so the spread is centred.
//!   * A **Milky Way** band lies along a great circle, rendered as diffuse
//!     unresolved starlight with dust-lane mottling. The lanes are dark nebulae in
//!     *front* of the band, so they subtract — that is what keeps it from looking
//!     airbrushed.
//!
//! From the Moon there is no atmosphere, so there is no twinkle, no extinction
//! near the horizon, and stars stay point-like right down to the terminator.
//!
//! # Brightness is in FINAL-IMAGE units, deliberately
//!
//! A plain `ShaderMaterial` is unlit: this returns a colour directly into the HDR
//! target, so it bypasses `view.exposure` and is tonemapped as-is. That is the
//! honest arrangement for this sky, because the sky is NOT radiometric. A real
//! photograph taken on the lunar surface in daylight has a pure black sky with no
//! stars in it — sunlit regolith sits around EV 15 and the brightest stars around
//! EV -3, and no single exposure holds both. We render them anyway, for legibility,
//! so `brightness` is tuned against the *picture* rather than against the scene's
//! photometry — and it therefore does not need re-tuning when a scene's
//! `lunco:env:exposureEv100` moves.
//!
//! Dynamic, self-describing parameters: the engine reflects the `Material` struct
//! (field names → std140 offsets) and the `//!@` annotations straight out of this
//! file. Edit live (hot-reload) or via the Inspector / `SetObjectProperty`.

#import bevy_pbr::{
    forward_io::VertexOutput,
    mesh_view_bindings::view,
}

const PI: f32 = 3.14159265359;
/// One arcminute in radians — the natural unit for a star's rendered size.
/// The human eye resolves ~1 arcmin, and a real star is *far* smaller than that
/// (Betelgeuse, the largest naked-eye disc, is ~0.05 arcsec), so any visible
/// size here is point-spread, not the star.
const ARCMIN: f32 = 2.908882e-4;

//!@ui      band_color      color   "Milky Way colour"
//!@default band_color      1.0,0.97,0.90
//!@ui      star_density    8 120   "Star density (shell cells/rad)"
//!@default star_density    40
//!@ui      limit_magnitude 3 9     "Limiting magnitude"
//!@default limit_magnitude 6.5
//!@ui      magnitude_slope 0.3 0.9 "log N(<m) slope"
//!@default magnitude_slope 0.6
//!@ui      color_spread    0 1     "Colour (B-V) spread"
//!@default color_spread    0.55
//!@ui      point_size      0.2 6   "Star size (arcmin)"
//!@default point_size      0.9
//!@ui      glow            0 1     "Halo strength"
//!@default glow            0.35
//!@ui      seed            0 999   "Sky seed"
//!@default seed            37
//!@ui      band_intensity  0 0.2   "Milky Way intensity"
//!@default band_intensity  0.045
//!@ui      band_tilt       0 90    "Galactic plane tilt (deg)"
//!@default band_tilt       62
//!@ui      band_width      2 30    "Milky Way width (deg)"
//!@default band_width      8
//!@ui      brightness      0 0.3   "Star brightness"
//!@default brightness      0.03
struct Material {
    band_color:      vec3<f32>,
    star_density:    f32,
    limit_magnitude: f32,
    magnitude_slope: f32,
    color_spread:    f32,
    point_size:      f32,
    glow:            f32,
    seed:            f32,
    band_intensity:  f32,
    band_tilt:       f32,
    band_width:      f32,
    brightness:      f32,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(0)
var<uniform> mat: Material;

// --- hashing --------------------------------------------------------------

/// Three decorrelated values in [0,1) from an integer lattice cell.
///
/// Cell coordinates stay small here (|p| ≈ `star_density`, i.e. tens), which is
/// the regime where the classic `sin`-based hash is well behaved — it degrades
/// only once the argument is large enough that `sin`'s period aliases against
/// float precision.
fn hash33(p: vec3<f32>) -> vec3<f32> {
    let q = vec3<f32>(
        dot(p, vec3(127.1, 311.7, 74.7)),
        dot(p, vec3(269.5, 183.3, 246.1)),
        dot(p, vec3(113.5, 271.9, 124.6)),
    );
    return fract(sin(q) * 43758.5453123);
}

fn hash13(p: vec3<f32>) -> f32 {
    var p3 = fract(p * 0.1031);
    p3 += dot(p3, p3.zyx + 31.32);
    return fract((p3.x + p3.y) * p3.z);
}

/// 3D value noise. Sampled on the **direction vector**, so it is a function on
/// the sphere with no parameterisation — hence no seam, at any meridian, ever.
fn vnoise(p: vec3<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    let n000 = hash13(i);
    let n100 = hash13(i + vec3(1.0, 0.0, 0.0));
    let n010 = hash13(i + vec3(0.0, 1.0, 0.0));
    let n110 = hash13(i + vec3(1.0, 1.0, 0.0));
    let n001 = hash13(i + vec3(0.0, 0.0, 1.0));
    let n101 = hash13(i + vec3(1.0, 0.0, 1.0));
    let n011 = hash13(i + vec3(0.0, 1.0, 1.0));
    let n111 = hash13(i + vec3(1.0, 1.0, 1.0));
    return mix(
        mix(mix(n000, n100, u.x), mix(n010, n110, u.x), u.y),
        mix(mix(n001, n101, u.x), mix(n011, n111, u.x), u.y),
        u.z,
    );
}

/// Normalised to ~0..1 regardless of octave count.
fn fbm(p: vec3<f32>, octaves: i32) -> f32 {
    var sum = 0.0;
    var amp = 1.0;
    var total = 0.0;
    var q = p;
    for (var o = 0; o < octaves; o++) {
        sum += amp * vnoise(q);
        total += amp;
        amp *= 0.5;
        q *= 2.0;
    }
    return sum / total;
}

// --- stellar photometry ---------------------------------------------------

/// Relative flux of a star drawn from the observed magnitude distribution.
///
/// The counts obey `N(<m) ∝ 10^(slope·m)` with slope ≈ 0.6 for a locally uniform
/// stellar disc, so for a uniform deviate `u ∈ (0,1]` the exact inverse CDF is
///
///     m = m_limit + log10(u) / slope
///
/// (u = 1 ⇒ the faintest star at the limiting magnitude, u → 0 ⇒ the rare bright
/// ones). Pogson's ratio then converts magnitude to flux, `F ∝ 10^(-0.4 m)`, and
/// normalising against the limiting magnitude collapses the whole thing to
///
///     F = 10^(-0.4·(m - m_limit)) = u^(-0.4/slope)
///
/// which is one `pow` and needs neither `m_limit` nor a logarithm at runtime.
/// `m_limit` still matters: it sets how many stars exist at all (see
/// `limit_magnitude`'s use in the density gate below).
///
/// At the default slope the dynamic range across ~20 000 stars is ~700:1, i.e.
/// about seven magnitudes — a mag 6.5 sky floor up to a Sirius-class anchor.
fn star_flux(u: f32, slope: f32) -> f32 {
    return pow(max(u, 1.0e-6), -0.4 / max(slope, 0.05));
}

/// Colour ramp indexed by a B-V-like parameter `t`: 0 = hot blue, 1 = cool orange.
/// Values are linear-space multipliers, matched to the naked-eye range: B0 stars
/// (~25 000 K) read faintly blue rather than saturated, and even M giants
/// (~3500 K) are amber, not red — the eye's scotopic response desaturates
/// everything at these luminances.
fn star_color(t: f32) -> vec3<f32> {
    if (t < 0.5) {
        let k = t / 0.5;
        return vec3(0.72 + 0.28 * k, 0.82 + 0.18 * k, 1.0);   // blue → white
    }
    let k = (t - 0.5) / 0.5;
    return vec3(1.0, 0.98 - 0.30 * k, 0.94 - 0.56 * k);       // white → orange
}

// --- the sky --------------------------------------------------------------

/// Accumulated star light along direction `d`.
///
/// Stars live on a **unit-cell lattice restricted to one radial shell** of radius
/// `S = star_density`. For each of the 27 lattice cells neighbouring `d·S` we
/// hash out a jittered point, keep it only if it falls inside the shell
/// `|p| ∈ [S-0.5, S+0.5]`, and project it radially onto the sphere. Because the
/// lattice is uniform in volume and the shell has constant thickness, the
/// surviving points are uniform in *solid angle* — the correctness argument for
/// "no polar pile-up", and it holds by construction rather than by sampling.
///
/// Star count is then just the shell volume, ≈ 4π·S² — 20 000 stars at the
/// default S = 40, the same order as the naked-eye sky (~9000 to mag 6.5, more
/// once you allow the faint background this is standing in for).
///
/// `px` is the angular size of one pixel (radians) and is the anti-aliasing
/// input: a star is never rendered narrower than a pixel, and its amplitude is
/// scaled by 1/σ² when it is widened, so total flux is conserved. Without that,
/// a sub-pixel point source scintillates as the camera pans — which is what a
/// star does through an atmosphere, and precisely what it must NOT do in vacuum.
fn stars(d: vec3<f32>, px: f32) -> vec3<f32> {
    let s_grid = max(mat.star_density, 4.0);
    let p = d * s_grid;
    let base = floor(p);

    // Physical point-spread, floored at half a pixel (see above).
    let sigma_pt = max(mat.point_size, 0.05) * ARCMIN;
    let sigma = max(sigma_pt, px * 0.55);
    // Flux conservation when the pixel floor widens the profile.
    let shrink = (sigma_pt * sigma_pt) / (sigma * sigma);
    let inv2s2 = 1.0 / (2.0 * sigma * sigma);
    // A wide, faint halo around each star. Real point sources acquire one from
    // the optics (diffraction + scatter in the lens), and without it bright stars
    // read as identical pixels to faint ones — the halo is the only cue that
    // survives being one pixel wide.
    let halo_s = sigma * 6.0;
    let inv2h2 = 1.0 / (2.0 * halo_s * halo_s);

    // More stars than the magnitude limit allows are simply not drawn: the shell
    // fixes the count, so `limit_magnitude` acts as a fractional gate on it.
    // Referenced to mag 6.5 (the classic naked-eye limit) so the default is 1.0.
    let count_frac = pow(10.0, mat.magnitude_slope * (mat.limit_magnitude - 6.5));

    var acc = vec3(0.0);
    for (var i = -1; i <= 1; i++) {
        for (var j = -1; j <= 1; j++) {
            for (var k = -1; k <= 1; k++) {
                let cell = base + vec3(f32(i), f32(j), f32(k));
                let h = hash33(cell + mat.seed);
                let sp = cell + h;
                let r = length(sp);
                // One shell only — three overlapping shells would triple the
                // apparent density and, worse, correlate stars along the line of
                // sight into faint radial streaks.
                if (abs(r - s_grid) > 0.5) {
                    continue;
                }
                // Second, decorrelated hash for the star's own properties.
                let g = hash33(sp * 1.7 + 19.3 + mat.seed);
                if (g.z > count_frac) {
                    continue;
                }

                let sd = sp / r;
                // Chord² ≈ angle² for the sub-degree separations that matter, and
                // it avoids `acos`, which loses all precision exactly here (near
                // dot = 1).
                let dv = sd - d;
                let a2 = dot(dv, dv);
                if (a2 > halo_s * halo_s * 25.0) {
                    continue;
                }

                let flux = star_flux(g.x, mat.magnitude_slope) * mat.brightness * shrink;
                // Bell-shaped B-V from two deviates (sum of uniforms → triangular),
                // so most stars read white and the extremes are rare, as observed.
                let t = clamp(0.5 + mat.color_spread * (g.y + h.x - 1.0), 0.0, 1.0);
                let core = exp(-a2 * inv2s2);
                let halo = mat.glow * exp(-a2 * inv2h2) / 36.0;
                acc += star_color(t) * (flux * (core + halo));
            }
        }
    }
    return acc;
}

/// Diffuse unresolved starlight along the galactic plane.
///
/// A Gaussian across a great circle, modulated by 3D FBM for clumping and by a
/// second, offset FBM for the dust lanes — which SUBTRACT, because they are dark
/// nebulae in front of the band rather than gaps in it.
///
/// Everything here is a function of the direction vector, so there is no
/// meridian at which the noise can fail to close: the seam the baked generator
/// had (its `fbm` ran u from 0 to 26 and the hash at 26 ≠ the hash at 0) cannot
/// be expressed in this formulation.
fn milky_way(d: vec3<f32>) -> vec3<f32> {
    if (mat.band_intensity <= 0.0) {
        return vec3(0.0);
    }
    // Pole of the galactic plane, tilted off the scene's vertical so the band
    // crosses frame diagonally rather than ringing the horizon.
    let tilt = radians(mat.band_tilt);
    let pole = vec3(0.0, cos(tilt), sin(tilt));
    let s = dot(d, pole);
    let w = max(radians(mat.band_width), 1.0e-3);
    let band = exp(-(s / w) * (s / w));
    if (band < 1.0e-4) {
        return vec3(0.0);
    }

    let q = d * 9.0 + mat.seed;
    let clump = 0.45 + 0.85 * fbm(q, 4);
    let lane = fbm(q * 1.7 + 3.1, 3);
    let lane_mul = 0.30 + 0.70 * saturate((lane - 0.30) / 0.45);
    return mat.band_color * (mat.band_intensity * band * clump * lane_mul);
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    // THE VIEW RAY, not the surface point direction. The sky mesh is a finite
    // sphere, but shading by `normalize(hit - eye)` makes it behave as one at
    // infinity: the value at a pixel depends only on where that pixel looks, so
    // the sky does not parallax as the camera translates inside the dome, and
    // the dome's radius is free to be whatever keeps it inside the far plane.
    let d = normalize(in.world_position.xyz - view.world_position);

    // Angular size of one pixel, in radians. `d` is a unit vector, so the screen
    // -space derivative of it IS the per-pixel angular step — which makes the
    // star anti-aliasing resolution-independent and correct under any FOV,
    // including a zoomed cinematic lens. Computed before any branch: `fwidth`
    // needs uniform control flow.
    let px = max(length(fwidth(d)), 1.0e-7);

    let color = stars(d, px) + milky_way(d);
    // Returned straight, with no `apply_pbr_lighting` and no fog: this is an
    // emissive backdrop, not a surface. Tonemapping in the post pass is the only
    // thing that touches it afterwards.
    return vec4(color, 1.0);
}
