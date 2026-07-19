//! Procedural night sky for the general `ShaderMaterial` — stars and the Milky
//! Way evaluated **per view direction**, with no texture anywhere in the pipeline.
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
//! # The physics that makes it read as sky
//!
//! ## Stars
//!
//!   * Distributed **uniformly on the sphere**. The hash lattice below is a
//!     uniform 3D grid restricted to one radial shell, so its points are uniform
//!     per unit volume of that shell — which, projected radially, is uniform per
//!     steradian. No polar pile-up, and it holds by construction.
//!   * Magnitudes follow **N(<m) ∝ 10^(0.6 m)**: each magnitude step is ~4x more
//!     stars, each 10^0.4 = 2.512x fainter (Pogson's ratio). A few bright anchors
//!     over thousands of faint ones. Sampled by exactly inverting that CDF.
//!   * Colours span the **B-V** range of naked-eye stars, hot blue-white (O/B,
//!     ~25 000 K) through white (A/F) to amber (K/M, ~3500 K), centred so most
//!     read white.
//!   * They must **not scintillate**. On an airless body a star is a rock-steady
//!     point, and a starfield that crawls or pops during a recorded pan is the
//!     single most obvious tell. So the point spread is floored at half a pixel
//!     (`fwidth` of the direction gives the per-pixel angular step) and its
//!     amplitude scaled by 1/σ² so total flux is conserved when it is widened —
//!     the same analytic anti-aliasing discipline `regolith.wgsl` uses, and it
//!     matters more here.
//!
//! ## The Milky Way — why it is not a glowing stripe
//!
//! Modelled in **galactic coordinates** (l, b), because that is the frame the
//! structure is actually defined in:
//!
//!   * **Disc.** Surface brightness falls off **exponentially in |b|**, not as a
//!     Gaussian. An exponential is what an isothermal disc integrates to along a
//!     line of sight, and it is what gives the band its characteristic sharp
//!     bright core with wide faint wings. A Gaussian stripe reads as fog.
//!   * **The core is not the arms.** Toward Sagittarius (l = 0) we look through
//!     the bulge: much brighter, visibly THICKER, and warmer — yellow-white from
//!     an old metal-rich stellar population and further reddened by the dust in
//!     front of it. Toward the anticentre (l = 180°) the disc is faint, thin and
//!     slightly bluer (young disc stars). Uniform brightness and colour along the
//!     band is an instant tell.
//!   * **Dust is the whole trick.** The Great Rift — the dark channel running
//!     from Cygnus down through Sagittarius that appears to split the band in two
//!     — is not a gap in the stars. It is cold molecular dust in the foreground,
//!     absorbing the light of everything behind it. So it is applied here as
//!     **extinction**, `exp(-τ)`, over the accumulated glow, and not as a
//!     subtractive tint: absorption and subtraction only agree where the
//!     background is flat, and the band is anything but.
//!   * **Reddening comes free from doing that correctly.** Interstellar
//!     extinction goes roughly as 1/λ, so blue light is absorbed ~1.4x more
//!     strongly than red. Giving τ a per-channel weight of about (1.35, 1.0,
//!     0.72) means the band automatically turns amber where it is dimmed, which
//!     is exactly what a long exposure of the galactic centre shows.
//!   * **Dust has a smaller scale height than the stars.** The molecular disc is
//!     roughly a third as thick as the stellar disc, and that single fact is why
//!     the Rift lies along the band's spine and appears to bisect it rather than
//!     mottling it at random.
//!
//! ## Exposure, and where the exaggeration lives
//!
//! A plain `ShaderMaterial` is unlit: this returns a colour directly into the HDR
//! target, so it bypasses `view.exposure` and is tonemapped as-is. That is the
//! honest arrangement for this sky, because the sky is NOT radiometric. A real
//! photograph taken on the lunar surface in daylight has a pure black sky with no
//! stars in it — sunlit regolith sits around EV 15 and the brightest stars around
//! EV -3, and no single exposure holds both. We render them anyway.
//!
//! Bright stars are deliberately allowed to exceed 1.0 rather than being clamped,
//! so the pipeline's bloom is what makes them read as *bright* instead of as
//! white dots. Bright stars are additionally separated from faint ones by a wide
//! second glow lobe, so magnitude reads as SIZE as well as intensity — which is
//! how the eye judges it, and what a clipped one-pixel core cannot convey.
//!
//! Every dramatic choice is a dial (`brightness`, `band_intensity`, `core_boost`,
//! `dust_strength`, `star_density`), and the scene `.usda` records a
//! physically-honest set of values alongside the shipped ones.
//!
//! # Cost
//!
//! The sky covers a large fraction of frame, so this is written to be cheap:
//!
//!   * Lattice cells are rejected against the shell by their CENTRE, before any
//!     hashing — that discards roughly two thirds of the 27 neighbours for the
//!     price of one `length()`.
//!   * The galaxy is gated on its own analytic envelope: outside the band, zero
//!     noise is evaluated.
//!   * The band is low-frequency, so the dust FBM runs 3 octaves and the granular
//!     clumping reuses the same evaluation instead of taking a second one.
//!   * No loop bound depends on a quality knob.
//!
//! Dynamic, self-describing parameters: the engine reflects the `Material` struct
//! (field names → std140 offsets) and the `//!@` annotations straight out of this
//! file. Edit live (hot-reload) or via the Inspector / `SetObjectProperty`.

#import bevy_pbr::{
    forward_io::VertexOutput,
    mesh_view_bindings::view,
}
#import lunco::noise::vnoise_quintic

const PI: f32 = 3.14159265359;
/// One arcminute in radians — the natural unit for a star's rendered size.
/// The human eye resolves ~1 arcmin, and a real star is *far* smaller than that
/// (Betelgeuse, the largest naked-eye disc, is ~0.05 arcsec), so any visible
/// size here is point-spread, not the star.
const ARCMIN: f32 = 2.908882e-4;

/// Per-channel weighting of the dust optical depth — interstellar extinction
/// goes roughly as 1/λ, so at (R 650, G 550, B 445) nm the relative absorptions
/// are about 0.72 : 1.00 : 1.35. This is what reddens the band as it dims.
const EXTINCTION_RGB: vec3<f32> = vec3<f32>(0.72, 1.00, 1.35);

//!@ui      core_color      color   "Galactic bulge colour"
//!@default core_color      1.0,0.86,0.62
//!@ui      arm_color       color   "Outer disc colour"
//!@default arm_color       0.84,0.90,1.0
//!@ui      star_density    8 120   "Star density (shell cells/rad)"
//!@default star_density    46
//!@ui      limit_magnitude 3 9     "Limiting magnitude"
//!@default limit_magnitude 6.5
//!@ui      magnitude_slope 0.3 0.9 "log N(<m) slope"
//!@default magnitude_slope 0.6
//!@ui      color_spread    0 1     "Colour (B-V) spread"
//!@default color_spread    0.55
//!@ui      point_size      0.2 6   "Star size (arcmin)"
//!@default point_size      0.9
//!@ui      glow            0 1     "Star halo strength"
//!@default glow            0.35
//!@ui      brightness      0 0.5   "Star brightness"
//!@default brightness      0.05
//!@ui      seed            0 999   "Sky seed"
//!@default seed            37
//!@ui      band_intensity  0 0.5   "Milky Way intensity"
//!@default band_intensity  0.10
//!@ui      core_boost      0 8     "Galactic bulge boost"
//!@default core_boost      3.0
//!@ui      band_width      1 20    "Disc scale height (deg)"
//!@default band_width      5.0
//!@ui      dust_strength   0 6     "Dust extinction"
//!@default dust_strength   2.6
//!@ui      dust_scale      2 40    "Dust structure scale"
//!@default dust_scale      11
//!@ui      pole_tilt       0 180   "Galactic pole tilt (deg)"
//!@default pole_tilt       62
//!@ui      pole_azimuth    0 360   "Galactic pole azimuth (deg)"
//!@default pole_azimuth    20
//!@ui      center_roll     0 360   "Galactic centre roll (deg)"
//!@default center_roll     120
struct Material {
    core_color:      vec3<f32>,
    star_density:    f32,
    arm_color:       vec3<f32>,
    limit_magnitude: f32,
    magnitude_slope: f32,
    color_spread:    f32,
    point_size:      f32,
    glow:            f32,
    brightness:      f32,
    seed:            f32,
    band_intensity:  f32,
    core_boost:      f32,
    band_width:      f32,
    dust_strength:   f32,
    dust_scale:      f32,
    pole_tilt:       f32,
    pole_azimuth:    f32,
    center_roll:     f32,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(0)
var<uniform> mat: Material;

// --- hashing --------------------------------------------------------------

/// Three decorrelated values in [0,1) from an integer lattice cell.
///
/// Arithmetic (sin-free) hash: call sites add `mat.seed` (0–999) to the cell
/// coords, and at that magnitude a `sin`-based hash aliases against f32
/// precision differently per driver's polynomial — this one stays exact.
fn hash33(p: vec3<f32>) -> vec3<f32> {
    var p3 = fract(p * vec3(0.1031, 0.1030, 0.0973));
    p3 += dot(p3, p3.yxz + 33.33);
    return fract((p3.xxy + p3.yxx) * p3.zyx);
}

/// Three octaves, returning the coarse sum in `.x` and the finest octave in `.y`.
///
/// Two-for-one deliberately: the dust lanes want the coarse field and the
/// unresolved-star granularity wants the fine one, and taking a second FBM to
/// get the second would double the cost of the most expensive thing on screen.
/// That specialisation is why this stays here rather than folding into
/// `lunco::noise::fbm`, which returns a scalar and would need a second pass.
///
/// `vnoise_quintic`, not `vnoise`: the dust band is a large smooth low-frequency
/// gradient, and the cubic interpolant's second-derivative break at the lattice
/// planes reads as rectangular brightness terraces across the sky. See the note
/// on `vnoise_quintic` in `lunco::noise`.
fn fbm3(p: vec3<f32>) -> vec2<f32> {
    let n0 = vnoise_quintic(p);
    let n1 = vnoise_quintic(p * 2.0);
    let n2 = vnoise_quintic(p * 4.0);
    return vec2((n0 + 0.5 * n1 + 0.25 * n2) / 1.75, n2);
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
///
/// At the default slope the dynamic range across ~25 000 stars is ~1000:1, i.e.
/// about 7.5 magnitudes — a mag 6.5 sky floor up to a Sirius-class anchor.
fn star_flux(u: f32, slope: f32) -> f32 {
    return pow(max(u, 1.0e-6), -0.4 / max(slope, 0.05));
}

/// Colour ramp indexed by a B-V-like parameter `t`: 0 = hot blue, 1 = cool orange.
/// Linear-space multipliers, matched to the naked-eye range: B0 stars (~25 000 K)
/// read faintly blue rather than saturated, and even M giants (~3500 K) are amber,
/// not red — the eye's scotopic response desaturates everything at these
/// luminances, and so does a sensor at these exposures.
fn star_color(t: f32) -> vec3<f32> {
    if (t < 0.5) {
        let k = t / 0.5;
        return vec3(0.72 + 0.28 * k, 0.82 + 0.18 * k, 1.0);   // blue → white
    }
    let k = (t - 0.5) / 0.5;
    return vec3(1.0, 0.98 - 0.30 * k, 0.94 - 0.56 * k);       // white → orange
}

// --- galactic frame -------------------------------------------------------

/// Orthonormal galactic basis for this scene: `.pole` is galactic north
/// (b = +90°), `.center` points at l = 0 (Sagittarius A*), `.side` completes it
/// so that `l = atan2(d·side, d·center)`.
///
/// Authored as three angles rather than derived, because these scenes have **no
/// equatorial frame to derive it from** — a marketing shot on an unlocated patch
/// of mare has a local horizon and nothing else, so the sky's orientation is a
/// framing decision and belongs in the `.usda`.
///
/// For a scene that DOES carry a real J2000 equatorial frame, do not eyeball it:
/// the galactic pole is at RA 12h51m26.28s, Dec +27°07'42.0" and the centre at
/// RA 17h45m37.2s, Dec −28°56'10.2", and the canonical equatorial→galactic
/// rotation (ESA, the Hipparcos/Tycho catalogue introduction) is
///
///     [ −0.0548755604  −0.8734370902  −0.4838350155 ]
///     [ +0.4941094279  −0.4448296300  +0.7469822445 ]
///     [ −0.8676661490  −0.1980763734  +0.4559837762 ]
///
/// Feed the scene's equatorial axes through that and use the resulting rows as
/// `center`, `side`, `pole` — the same structure this function builds by hand.
struct Galactic {
    pole:   vec3<f32>,
    center: vec3<f32>,
    side:   vec3<f32>,
}

fn galactic_frame() -> Galactic {
    let t = radians(mat.pole_tilt);
    let a = radians(mat.pole_azimuth);
    let pole = normalize(vec3(sin(t) * sin(a), cos(t), sin(t) * cos(a)));
    // Any vector not parallel to the pole seeds the basis; the roll below is what
    // actually places the galactic centre, so the seed's own phase is irrelevant.
    var seed_axis = vec3(0.0, 1.0, 0.0);
    if (abs(pole.y) > 0.9) {
        seed_axis = vec3(1.0, 0.0, 0.0);
    }
    let u = normalize(cross(seed_axis, pole));
    let v = cross(pole, u);
    let r = radians(mat.center_roll);
    let center = u * cos(r) + v * sin(r);
    return Galactic(pole, center, cross(pole, center));
}

/// Integrated light of the galaxy along direction `d`, after dust.
fn milky_way(d: vec3<f32>, g: Galactic) -> vec3<f32> {
    if (mat.band_intensity <= 0.0) {
        return vec3(0.0);
    }
    let sb = clamp(dot(d, g.pole), -1.0, 1.0);
    let b = asin(sb);                                  // galactic latitude
    let l = atan2(dot(d, g.side), dot(d, g.center));   // longitude, 0 = centre
    let dl = abs(l);                                   // angle from the centre

    // Scale height: the disc is visibly THICKER toward the bulge and thins to a
    // ribbon at the anticentre. (We are inside the disc, so this is line-of-sight
    // depth through it, not the disc's own geometry.)
    let h0 = max(radians(mat.band_width), 1.0e-3);
    let h = h0 * (0.55 + 0.85 * exp(-dl / 0.9));

    // Exponential, not Gaussian — see the module docs. Sharp spine, wide wings.
    let disc = exp(-abs(b) / h);
    // Longitudinal falloff. The inner galaxy dominates; the anticentre keeps a
    // ~12% floor so the band stays continuous all the way round, as it does in a
    // dark-sky panorama.
    let arm = 0.12 + 0.88 * exp(-dl / 1.15);
    // The bulge: a separate, much brighter and rounder component at l ≈ 0,
    // ~24° x 16°. This is the Sagittarius star cloud region.
    let bulge = exp(-(dl / 0.42) * (dl / 0.42) - (b / 0.28) * (b / 0.28));

    let envelope = arm * disc + mat.core_boost * bulge;
    // Nothing below the visible floor evaluates any noise at all — the analytic
    // gate is what keeps this cheap over the ~half of the sky the band never
    // reaches.
    if (envelope < 1.0e-3) {
        return vec3(0.0);
    }

    let n = fbm3(d * mat.dust_scale + mat.seed);
    // Unresolved star clouds — the band is grainy, not smooth (Scutum, M24).
    let clump = 0.75 + 0.55 * n.y;

    // Colour: the bulge is warm and old, the outer disc cool and young.
    let core_frac = saturate(mat.core_boost * bulge / max(envelope, 1.0e-6));
    let tint = mix(mat.arm_color, mat.core_color, core_frac);
    var glow = tint * (mat.band_intensity * envelope * clump);

    // --- dust -------------------------------------------------------------
    // Molecular dust is confined far closer to the plane than the stars are
    // (scale height roughly a third), which is exactly why the Great Rift lies
    // ALONG the band's spine and appears to split it rather than mottling it.
    let dust_h = h * 0.38;
    let dust_profile = exp(-abs(b) / dust_h);
    // Contrast-stretched noise: dust is patchy and near-opaque in the cores of
    // the clouds, not a gentle haze.
    let cloud = saturate((n.x - 0.30) / 0.42);
    // The Great Rift proper: a deterministic dark channel from Cygnus (l ≈ 80°)
    // down through Sagittarius (l ≈ 0°), which is the structure that reads as
    // "the Milky Way" to anyone who has seen it.
    let rift_c = 0.75;
    let rift = exp(-((l - rift_c) / 0.85) * ((l - rift_c) / 0.85));
    let tau = mat.dust_strength * dust_profile * cloud * cloud * (0.55 + 1.1 * rift);
    // ABSORPTION, not subtraction — and per-channel, so dimming reddens.
    glow *= exp(-tau * EXTINCTION_RGB);

    return glow;
}

// --- stars ----------------------------------------------------------------

/// Accumulated star light along direction `d`.
///
/// Stars live on a **unit-cell lattice restricted to one radial shell** of radius
/// `S = star_density`. For each of the 27 lattice cells neighbouring `d·S` we
/// hash out a jittered point, keep it only if it falls inside the shell, and
/// project it radially onto the sphere. Because the lattice is uniform in volume
/// and the shell has constant thickness, the surviving points are uniform in
/// *solid angle*.
///
/// Star count is the shell volume, ≈ 4π·S² — ~27 000 at the default S = 46, the
/// same order as a dark-sky photograph resolves (the naked eye sees ~9000 to
/// mag 6.5; the surplus stands in for the faint background).
///
/// `px` is the angular size of one pixel, and is the anti-aliasing input.
fn stars(d: vec3<f32>, px: f32) -> vec3<f32> {
    let s_grid = max(mat.star_density, 4.0);
    let p = d * s_grid;
    let base = floor(p);

    let sigma_pt = max(mat.point_size, 0.05) * ARCMIN;
    let sigma = max(sigma_pt, px * 0.55);
    // Flux conservation when the pixel floor widens the profile — this is what
    // stops a sub-pixel star scintillating as the camera pans.
    let shrink = (sigma_pt * sigma_pt) / (sigma * sigma);
    let inv2s2 = 1.0 / (2.0 * sigma * sigma);
    // A wide faint halo. Real point sources acquire one from the optics; without
    // it a bright star is the same single pixel as a faint one.
    let halo_s = sigma * 6.0;
    let inv2h2 = 1.0 / (2.0 * halo_s * halo_s);
    // Sized for the WIDE glow lobe (1/0.12 ≈ 8.3x the near lobe's variance),
    // so a bright star is not clipped into a hard-edged disc by the early-out.
    let cull2 = halo_s * halo_s * 90.0;

    // `limit_magnitude` acts as a fractional gate on the lattice's fixed count,
    // referenced to mag 6.5 (the classic naked-eye limit) so the default is 1.0.
    let count_frac = pow(10.0, mat.magnitude_slope * (mat.limit_magnitude - 6.5));

    var acc = vec3(0.0);
    for (var i = -1; i <= 1; i++) {
        for (var j = -1; j <= 1; j++) {
            for (var k = -1; k <= 1; k++) {
                let cell = base + vec3(f32(i), f32(j), f32(k));
                // Shell rejection on the cell CENTRE, before any hashing. A
                // jittered point sits within the cell, so a centre more than 1.0
                // from the shell can never land in it. This discards ~2/3 of the
                // 27 neighbours for one `length()`, and it is the single biggest
                // saving in the shader.
                let rc = length(cell + 0.5);
                if (abs(rc - s_grid) > 1.0) {
                    continue;
                }
                let h = hash33(cell + mat.seed);
                let sp = cell + h;
                let r = length(sp);
                // One shell only — overlapping shells would multiply the apparent
                // density and correlate stars along the line of sight into faint
                // radial streaks.
                if (abs(r - s_grid) > 0.5) {
                    continue;
                }
                let g = hash33(sp * 1.7 + 19.3 + mat.seed);
                if (g.z > count_frac) {
                    continue;
                }

                let sd = sp / r;
                // Chord² ≈ angle² for the sub-degree separations that matter, and
                // it avoids `acos`, which loses all its precision exactly here.
                let dv = sd - d;
                let a2 = dot(dv, dv);
                if (a2 > cull2) {
                    continue;
                }

                let flux = star_flux(g.x, mat.magnitude_slope) * mat.brightness * shrink;
                // Bell-shaped B-V from two deviates (sum of uniforms → triangular),
                // so most stars read white and the extremes stay rare.
                let t = clamp(0.5 + mat.color_spread * (g.y + h.x - 1.0), 0.0, 1.0);
                let core = exp(-a2 * inv2s2);
                // TWO-LOBE glow. A single narrow Gaussian makes every star the same
                // one-pixel dot regardless of magnitude — once the core clips, extra
                // flux has nowhere to go and bright and faint stars look identical.
                // The wide lobe gives a bright star visible EXTENT, so magnitude
                // reads as size as well as intensity, which is how the eye actually
                // judges it. Both lobes are normalised by their own area so the wide
                // one adds reach without washing the field grey.
                //
                // (An earlier revision drew diffraction spikes on the brightest
                // stars — the aperture-vane pattern of a real camera. REMOVED: they
                // read as drawn crosses, not as photography. If bright stars ever
                // look flat again, widen this lobe; do not bring the spikes back.)
                let near = mat.glow * exp(-a2 * inv2h2) / 36.0;
                // The wide lobe is kept deliberately WEAK and fairly tight. Its job
                // is to give a bright star a little visible extent, not a disc:
                // because its amplitude scales with flux like everything else, a
                // ~1000x Sirius-class anchor turns any generous setting here into a
                // fat soft blob that reads as bokeh or falling snow. That is the
                // exact failure the original baked generator warned about (it had
                // to abandon a 3x3 sigma-0.8 splat for the same reason), and it is
                // most visible at a WIDE field of view, where `px` is large and the
                // lobe is measured in pixels rather than arcminutes.
                let wide = mat.glow * exp(-a2 * inv2h2 * 0.30) / 500.0;
                let amp = core + near + wide;

                acc += star_color(t) * (flux * amp);
            }
        }
    }
    return acc;
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    // THE VIEW RAY, not the surface point direction. The sky mesh is a finite
    // sphere, but shading by `normalize(hit - eye)` makes it behave as one at
    // infinity: the value at a pixel depends only on where that pixel looks, so
    // the sky does not parallax as the camera translates inside the dome, and
    // the dome's radius is free to be whatever keeps it inside the far plane.
    let d = normalize(in.world_position.xyz - view.world_position);

    // Angular size of one pixel, in radians. `d` is a unit vector, so the
    // screen-space derivative of it IS the per-pixel angular step, which makes
    // the star anti-aliasing resolution-independent and correct under any FOV,
    // including a long cinematic lens. Computed before any branch: `fwidth`
    // needs uniform control flow.
    let px = max(length(fwidth(d)), 1.0e-7);

    let g = galactic_frame();
    let color = stars(d, px) + milky_way(d, g);
    // Returned straight, with no `apply_pbr_lighting` and no fog: this is an
    // emissive backdrop, not a surface. Values above 1.0 are left ALONE so the
    // pipeline's bloom sees them — clamping here is what makes bright stars read
    // as flat white dots. Tonemapping in the post pass is the only thing that
    // touches this afterwards.
    return vec4(color, 1.0);
}
