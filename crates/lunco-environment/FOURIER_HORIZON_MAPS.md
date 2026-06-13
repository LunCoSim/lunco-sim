# Fast Planetary Shadows using Fourier-Compressed Horizon Maps — Implementation Spec

**Source paper:** J. Fritsch, S. Schneegans, F. Friederichs, M. Flatken, M. Eisemann, A. Gerndt,
*"Fast Planetary Shadows using Fourier-Compressed Horizon Maps"*, High-Performance Graphics 2025,
Computer Graphics Forum Vol 44 No 8. DOI [10.2312/hpg20251171](https://doi.org/10.2312/hpg20251171).
DLR (Institute of Software Technology) / U Bremen / TU Braunschweig, Computer Graphics Lab.

- PDF: <https://elib.dlr.de/216096/1/hpg20251171.pdf> (open access, **CC-BY** — equations and
  pseudocode below are reproduced/paraphrased under that license, with attribution).
- EG Digital Library landing page (PDF + supplementary): <https://diglib.eg.org/handle/10.2312/hpg20251171>
- Supplementary zip (`paper1034_mm1.zip`, 176 MB):
  <https://diglib.eg.org/server/api/core/bitstreams/e9e24695-fb45-4a33-9af7-f9a8645231a3/content>
  Contents: `Demo-Video.mp4` (176 MB) + **`Pseudo-Algorithm.pdf`** (48 KB, the official
  bake + runtime pseudocode; transcribed in §F).
- **No public source-code release exists** (verified 2026-06-12, see §F). The proof-of-concept
  lives in a non-public CosmoScout VR branch. Everything below is reconstructed from the paper
  + the official pseudocode supplement.

Equation numbers "(Eq. N)" refer to the paper's numbering.

---

## A. Problem setup: the horizon function

### A.1 Definition (Eq. 1, Eq. 2)

For a point **p** on the terrain surface *M*, work in the **local tangent space at p**: the
plane through **p** with normal **N** pointing *away from the ellipsoid's center*. Define for
any other surface point **p′**:

- `θ_p(p′)` — **azimuth**: angle around the normal vector **N**,
- `α_p(p′)` — **elevation**: angle of **p′** *above the local tangent plane* (signed; below the
  plane is negative).

The **horizon function** is (Eq. 1):

```
h_p(θ) = max{ α_p(p′)  :  p′ ∈ M, θ_p(p′) = θ }
```

**What is stored: the elevation ANGLE itself, in radians.** Not the sine, not the slope.
(Confirmed by paper Fig. 4, y-axis "Altitude (radian)", values ≈ 0.03–0.09 rad for a MOLA
patch.) Theoretical range is `[−π/2, π/2]`; the bake initialises the running max to `−π/2`
(Algorithm 1), so a point at a local summit with nothing above its tangent plane keeps a
negative/near-zero horizon. In practice |h| is small (≲ 0.25 rad even for the lowest-frequency
Fourier term maxima — see §B.4 bit-depth discussion).

Binary (point-light) visibility for incident light direction `ω_i` (Eq. 2):

```
V(p, ω_i) = 1   if α_p(ω_i) > h_p(θ_p(ω_i))
            0   otherwise
```

i.e. compare the light's elevation against the reconstructed horizon elevation at the light's
azimuth — both expressed in the local tangent frame of **p**.

### A.2 Coordinate conventions

- Azimuth `θ ∈ [0, 2π)`. The bake constructs ray 0 in the **positive latitudinal direction**
  (local "north" on the ellipsoid) and obtains ray *k* of *K* by rotating around the outward
  normal **N** by `(k/K)·2π` radians (§3.1). The paper does **not** state the rotation
  handedness — see porting note G.2. The only hard requirement is that the runtime computes the
  sun's azimuth `θ_s` with the *identical* convention (same zero direction, same winding,
  around the same outward normal).
- Elevation is measured from the tangent plane toward **N** (positive up).
- The light source is projected onto the unit sphere around the shaded point **P** and
  expressed in these spherical coordinates: `(θ_s, α_s)` plus angular radius `r_s` for the sun
  disk (§3.3).

### A.3 Planetary curvature (curved variant — what the paper does)

The bake handles curvature *geometrically*, not with a flat-earth correction term (§3.1):

1. Take the texel's texture-space position `P0` and its DEM height; transform to a 3-D point
   `P0′` on/above the planetary **ellipsoid** (they use GDAL for map-projection ↔ geocentric
   transforms).
2. Construct **N** = outward ellipsoid normal at `P0′`. Each of the *K* rays `r_k` **lies in
   the tangent plane** (the plane through `P0′` with normal **N**).
3. March sample points `P′_{k,i} = P0′ + i·s·T` at regular interval `s` along the straight
   tangent-plane ray (`T` = ray direction; `s` = half the DEM pixel size, Algorithm 1).
4. Transform each `P′_{k,i}` **back into texture space** → `(p_i, h_i)` where `p_i` is the
   geographic/texture position and `h_i` is the sample point's altitude above the ellipsoid.
   Look up the interpolated DEM height `h′_i` at `p_i`, and build `P*_{k,i}` = the point at
   geographic position `p_i` raised to terrain height `h′_i` ("the point on the ellipsoid's
   surface directly above or below `P′_{k,i}`").
5. The elevation angle of that terrain sample is the angle at vertex `P0′` in the triangle
   `(P′_{k,i}, P0′, P*_{k,i})`, computed **via the law of cosines**:
   `α = ∠ P′_{k,i} P0′ P*_{k,i}` (signed: negative when the terrain point is below the
   tangent plane, i.e. `h′_i < h_i`).
6. `h_p(θ_k) = max_i α_i` along the ray.

Because the march happens in the *flat tangent plane* while the planet curves away underneath,
distant terrain naturally sinks below the plane — curvature falls out of the construction with
no explicit correction term. **Early-out** (Algorithm 1): once the tangent-plane sample's own
altitude `h_i` exceeds `max(D)` (the DEM's global maximum height), nothing can ever rise above
the plane again → skip to the next azimuth. This also implicitly bounds the march distance to
roughly `√(2·R·Δh_max)` (≈ 450 km on Mars for 30 km of relief).

### A.4 Flat-heightfield variant (degenerate case — not in the paper, trivial)

The paper only implements the curved/ellipsoidal variant. For a flat heightfield the
construction degenerates to the classic horizon-map bake: march in the heightfield plane,
`α_i = atan2(h′_i − h_0, i·s)`, same max-reduction, same everything downstream. Use this for
local/flat terrain tiles; use A.3 when texels map to an ellipsoid. The compression (§B) and
runtime (§D) are identical for both.

---

## B. Fourier compression

### B.1 Transform and truncation

Per texel, the sampled horizon function `h_p(θ_j)`, `j = 0..M−1` (paper bakes `M = 360`, 1°
steps) is run through a **DFT** ("we compute the discrete Fourier transform (DFT) of each
pixel's horizon function", §3.2), then **truncated to the lowest N complex coefficients**
(`H_p ← DFT(H_p); H_p ← H_p[0:16]`, Algorithm 1). The paper's shipped configuration is
**N = 16 complex coefficients** (harmonics k = 0..15); 4, 8, 12 evaluated as cheaper options.
Truncation = inherent low-pass filter on the horizon profile.

Why Fourier instead of more linear samples (§3.2):

- amplitude/phase form lets low-frequency features sit at *arbitrary azimuths* (traditional
  8-sample horizon maps erase features not aligned with the sample directions — Fig. 2);
- linear interpolation of complex coefficients (in rectangular/vector form) between texels
  produces sensible intermediate horizons;
- Fourier series are inherently periodic → horizon is automatically continuous and periodic
  in azimuth, no seam at θ = 0/2π.

### B.2 Reconstruction and the (implicit) normalization convention

Runtime reconstruction of the horizon elevation at azimuth `θ_s` (Eq. 3), with stored
coefficients `X_k = X_k^r + i·X_k^i`:

```
α_h(θ_s) = Σ_{k=0}^{N−1} ( X_k^r · cos(k·θ_s)  −  X_k^i · sin(k·θ_s) )
```

This is `Re[ Σ_k X_k · e^{i·k·θ_s} ]`. Note Eq. 3 has **no 1/M factor and no factor 2 on the
non-DC terms** — the normalization must be folded into the stored coefficients at bake time.
With the standard forward DFT `F_k = Σ_j h(θ_j)·e^{−i·2πkj/M}`, the values to store are:

```
X_0 = (1/M) · F_0            (the mean horizon; imaginary part is 0 for real input)
X_k = (2/M) · F_k   k ≥ 1    (one-sided spectrum, factor 2 from folding the conjugate half)
```

Then Eq. 3 reproduces the truncated real series exactly. (The paper never states this
explicitly; it is forced by Eq. 3's uniform weights. See porting note G.1.)

### B.3 Texture layout — the multi-resolution pyramid (§3.2 "Spatial resolution" + "Summary")

Two observations drive the layout:

1. **Coefficient magnitudes fall fast with frequency** (Fig. 8: mean |X_k| ≈ 0.23 for the
   lowest terms decaying to ≈ 0.0015 by k = 15; near-zero past index ~6).
   Low-frequency terms dominate the horizon *shape*; high-frequency terms add detail.
2. **Global spatial downsampling of all coefficients is ugly** (Fig. 7a): 2×2 box-filtering
   the whole map causes shadow/light bleeding at the *base* and *tip* of shadows, worst near
   occluder edges.

→ **Selective downsampling**: keep low-frequency coefficients at full spatial resolution,
store higher-frequency coefficients at progressively lower spatial resolution (filters of
increasing size). Fig. 7b (coefficients 12–15 box-filtered 8×8) shows only slight changes at
shadow bases and tips vs. ground truth; the residual error is "less disruptive" than global
downsampling because it only perturbs fine detail, not the gross shadow.

**Concrete shipped layout** (one texture, 4 mip levels — but the mip contents are NOT
filtered copies of level 0, each level holds *different data*):

| Mip level l | Spatial resolution | Channel R | Channel G | Channel B | Channel A |
|---|---|---|---|---|---|
| 0 | `r_x × r_y` (full) | X_0 | X_1 | X_2 | X_3 |
| 1 | `r_x/2 × r_y/2` | X_4 | X_5 | X_6 | X_7 |
| 2 | `r_x/4 × r_y/4` | X_8 | X_9 | X_10 | X_11 |
| 3 | `r_x/8 × r_y/8` | X_12 | X_13 | X_14 | X_15 |

Each cell is one **complex** coefficient: real + imaginary as **two 16-bit half floats packed
into one 32-bit (integer) channel** (high/low 16 bits). So the GPU texture is a 4-channel,
32-bit-per-channel texture (effectively `RGBA32UI`) with 4 populated mip levels; "four complex
Fourier coefficients stored in each mipmap level" (§3.3). Downsampling per level is repeated
2×2 box filtering of the coefficient planes (Algorithm 1: `H[i] ← Downsample(H[i], i mod 4)`
— wait, `l = i mod 4` is the paper's pseudocode; in combination with the block table above the
intent is level `l = ⌊i/4⌋`, i.e. coefficient i lives at resolution `r/2^⌊i/4⌋`; Fig. 7's
caption confirms: "coefficient k_i downsampled by 2^⌊i/4⌋ × 2^⌊i/4⌋". See note G.8.)

They stopped at 4 levels because "we observed diminishing returns after a minification factor
of 16" — and 4 levels × 4 channels exactly holds 16 coefficients.

### B.4 Bit depth (§3.2 "Bit depth")

- **32-bit float vs 16-bit half: no discernible difference** in rendered shadows → ship halfs.
- **8-bit unsigned** (normalized with per-coefficient min/max stored as metadata — separate
  ranges are mandatory because low-freq maxima ~0.23 vs high-freq ~0.0015 differ by two orders
  of magnitude): produces noticeable artifacts — *noisy penumbras* for compressed maps,
  banding for traditional maps (Fig. 6). Rejected.
- Memory per coefficient: `2·n·b` bits for n complex coefficients at b bits/component.

### B.5 On-disk format & sizes (§3.2 Summary, §4.1, Table 1)

- Persistent storage: **TIFF**, since it supports arbitrary numbers of layers with varying
  resolutions. Each 32-bit complex coefficient is stored as **two separate 16-bit float
  layers** (re, im) → 8 layers per resolution level, 32 layers total for fm16.
- Per 256×256 patch (Table 1):
  - High-resolution raw horizon map (360 samples/texel, 16-bit): **45 MB**
  - Traditional [Max88]-style, 12 samples: **1.5 MB**
  - **Fourier mipmapped, 16 coeff, 4 levels (fm16): 1.33 MB** (= 256²·16B · (1 + ¼ + 1/16 + 1/64))
  - Fourier non-mipmapped, 16 coeff full-res (f16): 4 MB
  
  i.e. fm16 beats the *traditional* map's memory while being substantially more accurate (§E).

### B.6 Dataset / LOD organisation used in the paper (§4.1)

- Mars MOLA DEM [FHL18], 200 m/px, ~100,000×50,000 px (~10 GB). Global bake done at
  **800 m/px**; selected showcase regions at the full 200 m/px.
- Global horizon map = **3,072 patches of 256×256**. For CosmoScout's LOD: a **5-level
  quadtree** built by merging + downsampling 2×2 patch groups; coarsest level = 12 base
  patches. Patches **slightly overlap** and are selected so every rendered patch covers a
  similar screen-space area (standard CosmoScout LOD behaviour [SZGG22]).
- The horizon-map patch resolution is independent of (can exceed) the DEM tile resolution
  visible at a given LOD — a reason to evaluate in the fragment shader (§D.4).

---

## C. Bake procedure

### C.1 What the paper does

**Offline, CPU, brute force** (§3.1, §4.1, Algorithm 1):

- Input DEM from GeoTIFF; coordinate transforms via **GDAL**.
- For each texel, for each of K azimuths (K = 360, 1° steps): ray march in the tangent plane
  as in §A.3. **Step size s = pixelSize/2.** Inner loop to `stepCount` with the
  `h_i > max(D)` early-out (no explicit stepCount given — the early-out is the real bound).
- Per ray keep `α_max` (init `−π/2`); that's `h_p(θ_k)`.
- Per texel: DFT the 360-vector, keep first 16 complex coefficients (with the normalization of
  §B.2 folded in).
- Per coefficient plane: repeated 2×2 box downsample to its pyramid level (§B.3).
- "Fully parallelizable... each pixel and each azimuthal direction completely independent.
  While this is sufficient for demonstrating our method, optimizing this algorithm presents an
  opportunity for future research." (§3.1)

**Reported cost:** ~**3 minutes per 256×256 patch** (naive sampling) → **150 compute-hours**
for the full Mars set at 800 m/px (§4.1). Conclusion (§5) flags this as the main limitation
and points to faster horizon algorithms (Stewart [Ste98] — O(M log M) sweep sharing occluder
work across texels; Timonen-Westerholm [TW10]; Tabik [TRa11]) as future work. Max-mipmap ray
acceleration [TIS08, JSS20] is the standard GPU alternative.

### C.2 Official pseudocode (Algorithm 1, transcribed from the CC-BY supplement)

```
Input : Digital elevation model D
Output: Fourier-compressed horizon map H

s ← GetPixelSize()/2                      // step size
foreach pixel p do
    Hp ← []                               // local horizon function
    for azimuth θ ← 0 to 2π do            // K = 360 steps
        T   ← GetTangent(p, θ)            // tangent-plane ray direction
        h0  ← InterpolateDEM(D, p)
        p′0 ← ToWorldSpace(p, h0)
        αmax ← −π/2
        for i ← 1 to stepCount do
            p′i ← p′0 + i·s·T             // march in tangent plane
            (pi, hi) ← ToTexSpace(p′i)    // geographic pos + altitude of ray point
            if hi > max(D) then next azimuth   // tangent plane above all terrain
            h′i ← InterpolateDEM(D, pi)
            p*i ← ToWorldSpace(pi, h′i)   // terrain point under/over ray point
            α   ← ∠ p′i p′0 p*i           // law of cosines, signed elevation
            αmax ← max(α, αmax)
        end
        Hp[θ] ← αmax
    end
    Hp ← DFT(Hp)                          // forward transform
    Hp ← Hp[0:16]                         // truncate to 16 complex coefficients
end
for i ← 0 to 16 do
    l ← level(i)                          // = ⌊i/4⌋, see §B.3 / note G.8
    H[i] ← Downsample(H[i], l)            // 2×2 box, applied l times
end
```

### C.3 WebGL2 adaptation (fragment passes only — our constraint, not in the paper)

The paper's bake is CPU/offline; nothing in it requires compute shaders. A fragment-pass GPU
bake decomposes cleanly:

1. **Horizon pass(es)** — render target = one `R16F`/`R32F` layer per azimuth *batch*.
   Fragment shader: for its texel, march A.3/A.4 for one azimuth (or loop a small batch of
   azimuths writing to MRT channels — WebGL2 gives 8 draw buffers × 4 channels = up to 32
   azimuths per pass). Accelerate with a height max-mipmap if bake time matters.
2. **DFT accumulation pass(es)** — the DFT is a plain sum over azimuth samples:
   `F_k += h(θ_j)·(cos(kθ_j), −sin(kθ_j))`. Two options:
   a. *Single gather pass:* if the per-azimuth horizon values live in an atlas/array layers,
      one fragment pass per coefficient block reads all M azimuth values for its texel and
      writes 4 complex coefficients to one `RGBA32F`(or 2×`RGBA16F`) target — 8 MRT targets
      = all 16 coefficients (re,im) in one pass, M texture reads per fragment.
   b. *Chunked accumulation:* march+accumulate fused — each pass processes an azimuth chunk and
      adds its partial `Σ h·e^{−ikθ}` into ping-ponged accumulation targets (additive blending
      onto float targets requires `EXT_float_blend`; ping-pong works everywhere).
3. **Downsample passes** — standard 2×2 box-filter fragment passes producing the per-block
   pyramid levels (render to individual mip levels of the output texture, or to separate
   textures — see G.4).
4. Apply the §B.2 scaling (1/M, 2/M) in pass 2.

WebGL2 format constraints: rendering to float targets needs `EXT_color_buffer_float`
(RGBA32F/RGBA16F renderable; ~universally available). `RGBA16F` is *filterable* in core
WebGL2; `RGBA32F` filtering needs `OES_texture_float_linear`; integer textures (`RGBA32UI`,
the paper's runtime packing) are **never filterable** — see §D.2/G.4 for the consequences.

---

## D. Runtime evaluation

### D.1 Official pseudocode (Algorithm 2, transcribed from the CC-BY supplement)

```
Input : Fourier-compressed horizon map H
Input : local solar azimuth θs, altitude αs, angular radius rs
Output: lighting factor L ∈ [0,1]

foreach vertex/fragment p do
    p′ ← GetTexcoord(p)
    C ← []                                   // 16 complex coefficients
    for l ← 0 to 3 do                        // 4 mip levels
        v ← InterpolateMipmap(H, p′, l)      // manual bilinear at level l
        C[l*4+0] ← v.R ; C[l*4+1] ← v.G
        C[l*4+2] ← v.B ; C[l*4+3] ← v.A
    end
    if middle-value approximation then
        α ← ReconstructAltitude(C, θs)                       // Eq. 3
        L ← Occlusion("mv", α, αs, rs)                       // Eqs. 4–5
    else if circular-segment approximation then
        αl ← ReconstructAltitude(C, θs − rs)
        αr ← ReconstructAltitude(C, θs + rs)
        L ← Occlusion("cs", αl, αr, αs, rs)                  // Eqs. 6–7
    else                                                     // n rectangular boxes
        for i ← 0 to n−1 do
            αi ← ReconstructAltitude(C, θs − rs + (2·rs/n)·(i+½))
        end
        L ← Occlusion("r", α0..αn−1, αs, rs)                 // Eqs. 8–10
    end
end
```

`L` multiplies the direct-light contribution (`V = 1 − O`; in the simplest case multiplied
straight into the fragment color, §3).

### D.2 Texture access & cost

- Upload: single 4-channel 32-bit-integer texture with 4 mip levels; each channel packs
  (re, im) as two halfs (§3.3).
- **Texture taps: 4 explicit-LOD fetch locations** (one per mip level) — but because the
  half-pair packing defeats hardware filtering, "this has to be implemented in shader code":
  manual bilinear = **4 `texelFetch` per level → 16 integer taps total**, plus unpack
  (`unpackHalf2x16` in GLSL ⇔ `unpack2x16float` in WGSL) and lerp. With an unpacked
  2-texture (`re`/`im` `RGBA16F`) variant you'd get hardware bilinear = 8 filtered taps,
  or 4 if re/im are interleaved per level pair — see G.4.
- The coefficients are read **once** per shaded point regardless of how many soft-shadow
  azimuth samples follow; each extra sample is only an extra partial inverse DFT
  (16 sin/cos + MADs). This is why sample count scales sub-linearly (§4.3): frame time for
  16-coeff fragment path goes 9.98 ms (1 sample) → 14.7 (3) → 18.6 (5) → 21.2 (7) on an
  RTX 2050 at 1080p (Table 3) — not 3×/5×/7×.
- Plain `sin`/`cos` GLSL intrinsics are used; "we assume sine and cosine functions to be
  reasonably optimized by GPU vendors" (§3.3). No recurrence/Chebyshev trick needed.

### D.3 Reconstruction (Eq. 3)

```
α_h(θ) = Σ_{k=0}^{15} ( C[k].re · cos(k·θ) − C[k].im · sin(k·θ) )
```

The number of coefficients actually *summed* is a **runtime quality knob** (presets use 8 or
16; Table 3) — fewer terms = stronger low-pass, never a hard failure ("the effect is gradual
and there is no clear cut-off where the shadows seem to be completely implausible", §3.2).
Reading only 8 coefficients also means touching only mip levels 0–1.

### D.4 Vertex vs fragment shader (§3.3)

The same code can run per-vertex (interpolating L across triangles) or per-fragment.
Trade-off stated in the paper: terrain triangles span multiple fragments → fragment shader is
invoked more often (slower); but the horizon map can out-resolve the DEM → fragment shader
shows more shadow detail. Presets: Low/Medium = vertex; Medium-Frag/High/Very-High = fragment
(Table 3). Shader-stage choice was the **largest performance factor** in their tests.

### D.5 Soft shadows from the sun's angular radius (§3.3, Fig. 9)

Sun = disk of angular radius `r` (paper uses r = 0.35° for Mars; limb darkening deliberately
ignored — uniform disk). Lighting factor = visible-area fraction of the disk above the horizon
curve. Three approximations, all operating in (azimuth, elevation) angle-angle space:

**(a) Middle value (mv)** — 1 horizon sample at θs, horizon assumed locally constant
(this is Max's original soft shadow). With `α_h` from Eq. 3 (Eqs. 4–5):

```
x = clamp((α_h − α_s) / r, −1, 1)
O = 0.5 + ( asin(x) + x·√(1 − x²) ) / π        // occluded disk fraction
```

(Standard disk-below-chord area fraction; O=0 fully lit, O=1 fully occluded.)

**(b) Circular segment (cs)** — 2 samples at `θ_s ± r`; treat the horizon as the straight
secant through `L = (θ_s − r, α_h(θ_s − r))` and `R = (θ_s + r, α_h(θ_s + r))`; exact
disk∩half-plane area. Project the vector L→C (C = sun center `(θ_s, α_s)`) onto the secant
L→R giving foot point C′; `d = |C C′|` is the chord's apothem (Eq. 6):

```
A_cs = r²·acos(1 − (r − d)/r) − d·√(r² − d²)       // = r²·acos(d/r) − d·√(r²−d²)
O    = A_cs/(πr²)        if C′_y ≤ C_y   (chord below center → segment is occluded part)
       1 − A_cs/(πr²)    if C′_y > C_y                                       (Eq. 7)
```

Exact for constant-slope horizon edges; smooths away features interior to the disk.

**(c) n rectangular boxes (r)** — n vertical boxes of width `2r/n` spanning the disk,
heights from the chord at each box center; captures interior horizon detail; converges to the
true area integral as n→∞ (Eqs. 8–10):

```
x_i = (r/n)·(2i + 1) − r               // box-center azimuth offsets, i = 0..n−1
h_i = 2·√(r² − x_i²)                   // box height (chord length)
O   = (1/n) · Σ_i clamp( (α_h(θ_s + x_i) − α_s)/h_i + 0.5 , 0, 1 )
```

(The clamp is implied by the geometry — each summand is "grey fraction of box i", Fig. 9d.
See note G.6 on the uniform 1/n weighting.)

**Which to use (§4.2 Fig. 14, §4.3):** with ground truth = 20-box disk at r = 20°: `cs` and
boxes up to n = 3 do **not** beat `mv` significantly; n = 5, 7 do. Recommendation: **`mv` by
default** (best quality/cost); switch to boxes with n ≥ 5 when performance is of little
concern. Presets "Very High x3/x5/x7" = boxes with n = 3/5/7.

### D.6 LOD integration

Per-patch compressed horizon textures ride the existing terrain quadtree (§4.1, §B.6): each
rendered terrain patch binds its horizon texture; patches overlap slightly; quadtree level
chosen by screen-space size. Because Fourier coefficients interpolate sensibly (§B.1),
cross-fading/blending between quadtree levels behaves like ordinary texture LOD. Frustum-local:
only in-view tiles' horizon maps are resident — out-of-frustum occluders are already encoded
in each texel's horizon, which is the whole point of horizon mapping (no occluder geometry
kept in memory, single render pass).

---

## E. Quality, limits, performance

### E.1 Artifacts & mitigations

| Artifact | Cause | Mitigation (paper) |
|---|---|---|
| Smoothed/wavy global shadow contour | truncation = low-pass (Gibbs-type over/undershoot visible as wiggles in Fig. 4's 4- and 16-coeff curves) | more coefficients; gradual degradation, "no clear cut-off" (§3.2). No windowing/σ-approximation attempted — open option (G.7) |
| Teardrop-shaped shadows around sharp isolated peaks; loss of knife-edge shadow lines | worst-case spectra of 90° synthetic edges (Blocks/Peaks scenes) | inherent; "heavy distortion only occurs in highly artificial scenes" (§4.2, LPIPS discussion) |
| Noisy penumbra | 8-bit coefficient quantization | use ≥16-bit halfs (§3.2, Fig. 6) |
| Shadow/light bleeding at shadow base & tip | global spatial downsampling of all coefficients | selective (per-block) downsampling — keep k = 0..3 full-res (Fig. 7, §3.2) |
| Multiple/striped false shadows at unsampled azimuths | **traditional** horizon maps' 8–12 fixed azimuth samples (Fig. 2) | the Fourier representation itself — error becomes azimuth-uniform (Fig. 12) |

### E.2 Accuracy vs traditional horizon maps (§4.2)

- Test scenes: MOLA 1–3 (real Mars patches) + synthetic worst cases Blocks, Peaks. Ground
  truth = 360-sample raw maps; comparison vs traditional 12-sample maps (12, not 8, to match
  fm16's memory). Hard binary shadows, sun elevation 6°, 180 azimuths; metrics PER/SSIM/LPIPS.
- fm16 and fm8 beat traditional on **all scenes and metrics except** LPIPS-on-Peaks.
  Typical PER: MOLA1 raw ≈ 0.31% vs fm16 ≈ 0.19% (f16 ≈ 0.06%); Blocks raw ≈ 4.2% (peaks
  8.5%) vs fm16 ≈ 1.4%. SSIM ≥ 0.94 everywhere.
- Error vs azimuth (Fig. 12): traditional = zero at its 12 sampled azimuths, big bumps
  between; fm16 ≈ flat ~0.2% at *all* azimuths → temporally stable as the sun moves.
- Coefficient count (Fig. 13): fm4 insufficient (worse than raw); fm8 → big jump; fm8→f8 gap
  small → the mip-level spatial downsampling costs little accuracy.

### E.3 Performance (Table 3; 1920×1080, static Mars view, GPU timers, 30 s averages)

Systems: notebook RTX 2050 (4 GB) / desktop RTX 4090 (24 GB).

| Preset | #Coeff | #Samples (soft) | Stage | Notebook ms | Desktop ms |
|---|---|---|---|---|---|
| None (no horizon mapping) | – | – | – | 1.21 | 0.107 |
| Low | 8 | 1 (mv) | Vertex | 2.45 | 0.204 |
| Medium | 16 | 1 (mv) | Vertex | 4.07 | 0.297 |
| Medium Frag | 8 | 1 (mv) | Fragment | 5.48 | 0.363 |
| High | 16 | 1 (mv) | Fragment | 9.98 | 0.659 |
| Very High x3 | 16 | 3 (boxes) | Fragment | 14.7 | 0.535 |
| Very High x5 | 16 | 5 (boxes) | Fragment | 18.6 | 0.684 |
| Very High x7 | 16 | 7 (boxes) | Fragment | 21.2 | 0.823 |

(Headline claim, Fig. 1: full scene lit in < 4 ms on the RTX 2050 notebook → the "Medium"
vertex-shader preset.) Memory: §B.5. Bake cost: §C.1.

---

## F. Reference code

### F.1 Search result: **no public code release** (as of 2026-06-12)

The paper says the method "was implemented into CosmoScout VR" (MIT-licensed,
<https://github.com/cosmoscout/cosmoscout-vr>) as proof of concept, but the implementation was
**not** merged into the public repository. Verified exhaustively:

- Full recursive git tree of `cosmoscout/cosmoscout-vr@main`: **zero** paths matching
  `horizon|fourier` (the terrain plugin is `plugins/csp-lod-bodies`).
- All 27 branches of the repo, all PRs/issues mentioning "horizon" (6, all unrelated:
  atmospheres/eclipses/navigation) or "fourier" (0).
- All 40 public forks (incl. `Schneegans/cosmoscout-vr` — `main` only) and all 48
  `cosmoscout` org repos; DLR GitHub orgs; GitHub user search for the first author
  (Jonathan Fritsch); web search. Nothing.
- The TU-BS page (<https://graphics.tu-bs.de/publications/fritsch2025fast>) and the EG
  diglib entry link only the PDF + supplementary video/pseudocode — no repo.

**Consequence: there is no GLSL to quote.** The authoritative implementation references are
the paper's equations (reproduced above, CC-BY) and the official supplementary
`Pseudo-Algorithm.pdf` (Algorithms 1–2, transcribed verbatim-in-structure in §C.2/§D.1; same
CC-BY licence; extracted from the supplementary zip URL at the top of this file, also saved
locally at `/tmp/fourier_horizon_pseudo_algorithm.pdf`). If a release appears later, watch
`cosmoscout/cosmoscout-vr` and the csp-lod-bodies terrain shaders
(`plugins/csp-lod-bodies/shaders/`) — that is where it would land.

### F.2 Reference WGSL skeleton (derived from Eq. 3–5 + Algorithm 2, original code)

```wgsl
// Horizon map: texture_2d<u32> (rgba32uint), 4 mip levels, each texel-channel = (re,im) packed half2.
fn load_coeffs(tex: texture_2d<u32>, uv: vec2<f32>, out_c: ptr<function, array<vec2<f32>, 16>>) {
    for (var l = 0; l < 4; l++) {
        let dim  = vec2<f32>(textureDimensions(tex, l));
        let st   = uv * dim - 0.5;
        let i0   = vec2<i32>(floor(st));
        let f    = fract(st);
        for (var ch = 0; ch < 4; ch++) {
            // manual bilinear over 4 texelFetch, unpack2x16float per tap (packing defeats HW filtering)
            let c00 = unpack2x16float(textureLoad(tex, clamp(i0,            vec2(0), vec2<i32>(dim) - 1), l)[ch]);
            let c10 = unpack2x16float(textureLoad(tex, clamp(i0 + vec2(1,0), vec2(0), vec2<i32>(dim) - 1), l)[ch]);
            let c01 = unpack2x16float(textureLoad(tex, clamp(i0 + vec2(0,1), vec2(0), vec2<i32>(dim) - 1), l)[ch]);
            let c11 = unpack2x16float(textureLoad(tex, clamp(i0 + vec2(1,1), vec2(0), vec2<i32>(dim) - 1), l)[ch]);
            (*out_c)[l * 4 + ch] = mix(mix(c00, c10, f.x), mix(c01, c11, f.x), f.y);
        }
    }
}

fn reconstruct_altitude(c: array<vec2<f32>, 16>, theta: f32, n_coeffs: u32) -> f32 { // Eq. 3
    var alpha = 0.0;
    for (var k = 0u; k < n_coeffs; k++) {
        let a = f32(k) * theta;
        alpha += c[k].x * cos(a) - c[k].y * sin(a);
    }
    return alpha;
}

fn occlusion_mv(alpha_h: f32, alpha_s: f32, r_s: f32) -> f32 {  // Eqs. 4–5
    let x = clamp((alpha_h - alpha_s) / r_s, -1.0, 1.0);
    return 0.5 + (asin(x) + x * sqrt(1.0 - x * x)) / 3.14159265;
}
// lighting factor L = 1.0 - occlusion_mv(reconstruct_altitude(c, theta_s, N), alpha_s, r_s)
```

---

## G. Porting notes — decisions the paper leaves open

1. **DFT normalization is implicit.** Eq. 3 sums stored coefficients with unit weights, so the
   bake must store `X_0 = mean(h)` and `X_k = (2/M)·DFT_k` for k ≥ 1 (§B.2). Get this wrong
   and shadows are uniformly wrong by a scale factor — easy to misdiagnose as an angle-units
   bug. Validate with a synthetic single-cosine horizon.
2. **Azimuth convention.** Zero = local north ("positive latitudinal direction"), rotation
   about the *outward* normal — but winding (E-from-N vs W-from-N) is unstated. Pick one; the
   only invariant is bake ⇔ runtime agreement for `θ_s`. A 1-peak test DEM with the sun swept
   360° catches a mismatch immediately (shadow rotates the wrong way / mirrored).
3. **`stepCount` / march range.** Unspecified; bounded in practice by the `h_i > max(D)`
   early-out, which needs the DEM's global max height as bake metadata. For tiled bakes the
   march must read *neighboring tiles*' heights (long-range occluders are the feature) — bake
   from a clipmap/atlas with generous apron, or accept a max-distance cutoff and document it.
4. **Texture format vs filtering (WebGL2).** The paper's `RGBA32UI`+`unpackHalf2x16` packing
   forces manual bilinear (16 `texelFetch`). WebGL2 alternative: **two `RGBA16F` textures**
   (re-plane, im-plane), each with 4 authored mip levels, sampled with
   `textureSampleLevel`-equivalent hardware bilinear → 8 filtered taps total and simpler code.
   `RGBA16F` is filterable in core WebGL2; renderable with `EXT_color_buffer_float` (or
   `EXT_color_buffer_half_float`). Cost: 2 samplers instead of 1. **Do not** let automatic
   mip selection near the textures — every level holds *different coefficients*; always
   explicit-LOD, and allocate exactly 4 levels (or pad unused tail levels).
5. **Per-fragment sun frame.** `(θ_s, α_s)` are defined in the *local tangent frame of the
   shaded point*. On planetary scale, computing them per-fragment from the local normal of the
   reference ellipsoid (not the displaced terrain normal!) is required; a per-patch constant
   sun direction is only valid for small patches. Use the smooth ellipsoid normal — the bake's
   elevation angles are measured against the *tangent plane of the ellipsoid*, not the terrain
   micro-normal.
6. **Eq. 10 weighting.** The paper weights all n boxes uniformly (1/n) although box areas
   differ (`h_i` varies). Exact area weighting is `Σ frac_i·h_i / Σ h_i` for the same samples
   at negligible extra cost — choose either; uniform matches the paper/figures.
7. **Gibbs ringing at sharp horizons.** Mild wiggles are accepted as-is in the paper. If they
   bother us on crater rims, a Lanczos-σ window on the stored coefficients
   (`X_k *= sinc(k/N)`) trades a slightly softer contour for monotone behaviour — *not*
   evaluated in the paper; flag any such deviation in comparisons.
8. **Pseudocode `l ← i mod 4` is a typo.** Algorithm 1's downsample loop says `i mod 4`, but
   §3.2's text ("first four coefficients... at full spatial resolution, the next four at
   r/2...") and Fig. 7b's caption (`2^⌊i/4⌋`) both say block-major: level `⌊i/4⌋`. Use
   `⌊i/4⌋`.
9. **Patch seams.** Patches must be baked with overlap (paper: "slightly overlapping" patches)
   or with apron texels so bilinear at borders doesn't mix unrelated horizons; horizon values
   near tile edges depend on terrain in neighbor tiles regardless (note 3).
10. **DC imaginary half-channel is free** (X_0 is real). Candidate home for metadata (e.g.
    max horizon angle for a conservative early-out: `α_s > α_max + r_s` → fully lit, skip the
    inverse DFT entirely — our addition, not the paper's).
11. **Flat vs curved bake.** Paper = ellipsoid only (§A.3). Our flat-heightfield variant
    (§A.4) is the natural first milestone: identical compression/runtime, simpler bake;
    curvature changes only how `α` per march sample is computed and adds the `h_i > max(D)`
    early-out's geometric meaning.
12. **Sun's angular radius** is an input we already have (sun angular diameter / 2, in
    radians). r = 0.35° (Mars) in the paper's perf tests; soft-shadow approximations validated
    up to r = 20°.
