# Open review: lunar regional surface

The local site DEM is continuous with an extrapolated collar and a datum-aligned
sphere. The collar is not regional terrain. A 2.048 km NOBILE03 crop viewed
from 6 km altitude shows a rectangular detail boundary and dark exterior bands.
The same pattern appears around the 1 km Apollo 15 crop. With the globe shader
temporarily displaying albedo, the NOBILE03 exterior still shows a broad
brightness transition; disabling the native shadow map does not remove the
bands. Both geometry/lighting and material continuity require attention.

Separately, stale directional shadow-map depth produced broad black patches in
the Griffin ground view. Native full-attachment clearing restores the lander's
shadow without those patches. It does not supply the missing regional terrain:
a fresh 6 km school view still exposes the rectangular site boundary.

For the NOBILE03 site, the 20 m LOLA south-pole DEM covers the declared centre.
An exploratory source read compared 4,008 points over the current site crop:
the orthographic regional samples minus the existing local samples have median
-0.10 m and RMS 0.53 m after the Twin's declared height offset. This is not a
certificate of registration or source uncertainty. The current local crop uses
a centred equirectangular projection, which differs from the site's tangent
frame by about 7 m at its corners. A fresh orthographic NOBILE03 bake differs
from the current site by up to 1.32 m in height. Replacing the site bake would
therefore change existing physical contacts and needs its own mission baseline
and validation; the regional integration must retain the current local surface.

A 20 km diagnostic bake at 20 m posts shows real regional craters in a
production render, but it replaces the site and reported three refused visual
refinements; it is not an accepted solution. Over that crop the independently
prepared 80 m LOLA product differs from the 20 m product by 0.86 m RMS and up
to 32.3 m. Finer sampling of the 80 m product does not restore the missing
relief. The 20 m product is still too coarse to replace the landing surface.
NASA's [LOLA error analysis](https://ntrs.nasa.gov/citations/20205009660)
also identifies sampling gaps and slope as sources of DEM interpolation error.
The existing NOBILE03 source cannot simply be cropped larger at this centre:
the native processor rejects a 3,072 m window with 9.4% missing data and a
2,560 m window because its void interpolation did not converge. Filling those
areas as though they were measured would introduce another artificial surface.

For the Apollo 15 site at 26.0371° N, 3.6584° E, NASA's
[SLDEM2015 512 ppd tile 00N–30N, 000–045](https://imbrium.mit.edu/BROWSE/SLDEM2015/TILES/)
covers the site. The [USGS product description](https://www.usgs.gov/media/images/lro-lola-and-kaguya-terrain-camera-dem-merge-60n60s-512ppd)
states approximately 59 m equatorial posting and 3–4 m typical vertical
accuracy. It is a regional candidate, not a substitute for the 2 m local DTM;
its datum, registration and transition must be checked against the site before
admission. The archive offers the tile as a 164 MB JP2 or 1.35 GB float IMG;
the PDS reader supports bounded sample-window reads and registers this product's
`SIMPLE CYLINDRICAL` pixel centres from its detached label. The label declares
32-bit little-endian kilometre heights and an `OFFSET` that is the 1,737.4 km
reference radius. The decoder applies the label's generic affine offset, so the
processing recipe must explicitly remove the reference radius and convert to
metres. A 20 km, 1,001-square-post crop of the real IMG completed through the
asset processor with a pinned source checksum and 24,036 KiB peak RSS. Declaring
both the image and the detached label in the production pipeline produced a
byte-identical crop with 23,268 KiB peak RSS. Its centre is -1,946.684 m, and
25 posts agree with independently range-read regional samples within 0.000064 m.
This checks the crop, not its registration with the local surface or its visual
integration.

A four-post HTTP range read of the source at the declared site centre, using
the [product label](https://imbrium.mit.edu/DATA/SLDEM2015/TILES/FLOAT_IMG/SLDEM2015_512_00N_30N_000_045_FLOAT.LBL)
and bilinear interpolation, gives -1,946.684 m relative to its reference sphere.
The live Apollo terrain query at local (0, 0) returned -1,947.457 m: a 0.773 m
difference at this one location. This is a preliminary datum sanity check, not
registration evidence; surrounding samples, projection orientation and source
uncertainty still need comparison.

A follow-up 5×5 comparison sampled the retained terrain at 200 m spacing over
an 800 m square and read the corresponding SLDEM posts with HTTP ranges. Using
a spherical orthographic site frame and bilinear regional interpolation, the
regional-minus-local differences have median -3.11 m, RMS 3.63 m, minimum
-7.21 m and maximum 6.35 m. The source's ~59 m posting does not resolve the
local 2 m surface, and this small grid is not a registration certificate. The
sample coordinates and raw results are in
`target/orbital-apollo-registration/comparison.json` for this run.

The currently prepared Apollo site GeoTIFF does not declare its lunar frame.
LROC describes its map-projected products as using the mean-Earth reference
frame ([coordinate guidance](https://doi.org/10.3847/psj/ad54c6)), but the
existing artifact cannot prove that fact by itself. A diagnostic source rebake
with `frame = "MOON_ME"` differs from the retained site's 262,144 float64
samples by at most 2.66e-10 m (RMS 2.54e-11 m). This is not bit-identical, so
the integration must preserve the retained local grid and admit provenance
explicitly rather than silently swapping it for the rebake.

The retained Apollo grid and a 20 km regional crop prepared with the same
equirectangular projection passed `regional_probe` with `MOON_ME` explicitly
supplied for the local artifact, a 500 m transition, and zero regional datum
offset. The local boundary samples remained bit-identical and the largest
one-millimetre-stencil slope difference across the boundary was 0.000105.
Incorrect frame and insufficient regional coverage were rejected. This is a
sampling diagnostic, not a runtime surface or a registration certificate.
The local raster center is -1947.4076094074112 m; the previous terrain loader's
`f32` round trip represented it as -1947.4075927734375 m before analytic
modifiers. The loader and web grid-cache format now preserve the `f64` value.
On this pair, a 20 m square crossing the local boundary required 70 raster-cell
enclosures and produced a 6.94 m conservative height range. Solar traversal
must account for this work without making cache residency change its budget.
For a single quadtree to preserve the site's existing collider lattice exactly,
its regional half extent must be a power-of-two multiple of the local half
extent. A 16 km Apollo crop has the required 8,000 m half extent for the existing
500 m local half extent and passes the same sampling probe. An unaligned 20 km
domain would move collider sample positions even if its spacing were reduced.

For NOBILE03, twelve byte-range-verified tiles from NASA's
[20 m south-pole product](https://pgda.gsfc.nasa.gov/products/90) were decoded
into a diagnostic source window. The selected tiles, source offsets and hashes
are recorded in `target/orbital-nobile-source/window-provenance.json`. A
16.384 km equirectangular crop with the existing site's declared height offset
and `MOON_ME` frame passed the registered sampler probe against the retained
2.048 km local grid. Its centre is bit-identical, the largest edge slope
difference at a 1 mm stencil is 0.0000067, and a 20 m cross-edge window uses
22 cell enclosures with a 2.60 m conservative height range. This diagnostic
window is not a reproducible production dataset declaration: the diagnostic
manifest points to the locally derived window, not the whole NASA object.

The broader bounding box of [LROC NOBILE01](https://data.lroc.im-ldi.com/lroc/view_rdr_product/NAC_DTM_NOBILE01)
does not imply continuous coverage. Its 277,438,562-byte 3 m DTM matched
the archive MD5; SHA-256 is
`da45938606929814b3796f352875692742d9ca6a3b18b5c3a2cebd79e56ae102`.
A 1,025-square coverage test over the Nobile 16.384 km window found 60,579
valid samples out of 1,050,625 (5.77%), mostly in narrow, interrupted swaths.
It cannot replace the continuous regional DEM without a coverage-aware mosaic.

In the Nobile 8 km production view, disabling only the local normal-map
contribution removed the prominent crop-shaped shading boundary; disabling only
local albedo did not. The material now fades local normal influence inside the
registered overlap while retaining it near the crop center. The production view
confirms a smoother boundary; it does not establish that all orbital views or
source-data artifacts are resolved.

The NASA [5 m south-pole site products](https://pgda.gsfc.nasa.gov/products/78)
also do not cover this scene center. Their published GeoTIFF footprints were
checked against the Griffin site at 84.72672255° S, 29.14428685° E in the
products' south-polar stereographic frame. The nearest footprint, Site20v2,
ends about 8.8 km from the center; Site06, at Nobile Rim 1, ends about 19.8 km
away. A nearby site name alone is therefore not evidence of coverage.

The Apollo regional DEM has north-south streaks in its original SLDEM2015
samples, visible in finite differences before processing. A live normal
visualization reproduces them; an unlit render and a flat-normal diagnostic do
not. Disabling local imagery, local normals and the local horizon-shadow input
does not remove them. Published work also identifies streak artifacts in
SLDEM2015 ([lunar image-generation study](https://doi.org/10.3390/aerospace12020092)).
They need an explicitly specified visual treatment or a better registered
source; changing physical heights merely to improve the image would alter the
measured surface contract.

The [LROC Apollo 15 surrounding-terrain DTM](https://data.lroc.im-ldi.com/lroc/view_rdr_product/NAC_DTM_APOLLO15_2_E260N0036)
is an alternative at 5 m per pixel. Its 127,404,864-byte attached-label IMG
matched the archive MD5; its SHA-256 is
`d9d111a70c7b9a1e9288e8c6cb43a4a3585aafb11398efa13cd6ac7a653bdfdb`.
A diagnostic 8 km crop reaches missing data at its eastern edge and the DEM
processor rejected its unresolved voids; a centered 4 km crop completed and
its 513-by-513 coverage mask contains no missing samples.
Assuming `MOON_ME` for this diagnostic only, its sampled elevations differ
from the retained local grid by -2.60 m median and 2.65 m RMS over 1,089
points. LROC's [product record](https://data.lroc.im-ldi.com/lroc/view_rdr/NAC_DTM_APOLLO15_2)
reports 1.79 m RMS against 21 LOLA tracks, and its [coordinate guidance](https://doi.org/10.3847/psj/ad54c6)
states that LRO teams align map-projected products to the DE421 mean-Earth
frame. These are useful provenance, but the site-specific horizontal and
vertical registration still needs validation before the datasets are combined.
The 4 km footprint also needs an outer regional tier; it cannot replace the
16 km source outright.

A diagnostic raster composed the 4 km candidate with the 16 km SLDEM using a
1 km transition, retaining the existing 1 km landing surface as the runtime's
innermost grid. The outer raster has 20 m posts, so this experiment downsamples
the 5 m source and is not a production tiering implementation. The settled
orbital render reduced the stripes near the site but still showed source
artifacts farther out. The scene remained ready and fault-free; streaming
reported two refused visual refinements. Frame registration, a reproducible
dataset pipeline, full-fidelity tiering, and the refused refinements remain
open.

In the Nobile diagnostic, the 8 km camera settled with no refused splits at
the 256-tile setting. At the initial ground view, a paused exploratory run
reported 112, 136 and 31 refused splits at budgets of 256, 512 and 1,024,
respectively. Contact demand changed during that run, so the counts are not a
controlled performance comparison. Do not change the user's quality setting
based on these figures; characterize the contact and camera workload first.

The diagnostic regional materials use the LROC WAC 643 nm empirical
normalized mosaic, which is reflectance at a reference illumination/view
geometry rather than an intrinsic albedo. Both source files were checked
against archive byte counts and MD5 metadata. The Apollo crop has complete
coverage; the Nobile crop preserves its missing samples in alpha. A red
diagnostic image confirmed the GPU binding, then the measured image was restored.
The color overlap softens the crop transition, but the site footprint remains
visible at orbital altitude.

## Required integration

The terrain layer can declare a registered regional source alongside its local
DEM. The native loader, `SurfaceOracle`, visual streaming, collider admission,
queries, and celestial collar have an in-progress shared-domain path. In an
isolated Apollo scene the production app loaded the regional source, retained
the original centre height and returned a measured height 1 km from the centre.
Seeking the authored Apollo date exposed stretched site imagery outside its
footprint and a solar visibility work-budget fault. A bounded per-tick batch
shares conservative enclosures across sample rays while charging the same work
with cold and warm retained caches. The Apollo scene remained ready for 25
seconds after seeking the previously failing daylight date; a full restart
also returned to readiness without the previously observed retired-source
continuation fault. The web worker and orbital material transition remain
unfinished. Adding regional relief only to the collar
would leave height queries, collision and solar visibility on a different surface.
The web worker still rejects paired regional requests. Grid-cache hashing,
encoding, decoding and OPFS persistence run in the DEM worker. An isolated
Chromium run returned coarse and full stages on a cold request, persisted the
OPFS entry, and returned a bit-identical full grid on a warm request, including
262,144 `f64` samples. Corrupting the isolated cache entry caused a fresh
coarse/full bake and reproduced the same grid. Full web-scene and responsiveness
acceptance remain open.
The existing Nobile scene's dataset gate reports its source inputs ready but
the retained processed artifact stale under the revised pipeline. An isolated
rebuild produced height, albedo and coverage arrays identical to the retained
site. A temporary scene using that verified copy and the diagnostic regional
source returned a height at 2 km and reached readiness without the previous
startup solar fault. Readiness briefly dropped again during the subsequent
observation before recovering. A later residual-transition run stayed ready
for two captured views but still showed the local footprint. The diagnostic regional source is not yet a
reproducible production dataset declaration.

- Terrain owns one immutable composed height source, with the local site surface
  exact inside its footprint and a measured regional source outside. Register
  frames, offsets and sample positions explicitly; reject ambiguous inputs. A
  bounded transition must preserve height and gradient at the local edge and
  expose its uncertainty rather than changing measured interior samples.
- Terrain visual tiles, height queries, solar occlusion and physics colliders
  must sample the same composed source. Physics support retains its authored
  sampling quality; visual refinement does not decide contact readiness. Local
  edits, local rock scatter and local material maps retain their original scope.
- Local and regional imagery must be registered and photometrically compatible.
  A normalized orbital image is a reflectance observation under its stated
  reference geometry, not automatically intrinsic albedo. The global body's
  presentation image is not a calibrated local albedo source.
- Data admission, preparation and teardown remain scene-scoped and asynchronous.
  Missing or incompatible declared regional data must be reported by its owner.
  Regional geometry needs bounded tiling and collision admission rather than a
  single high-resolution collar mesh.

## Evidence still required

Production scenes for both Twins must be inspected at matched orbital, descent
and ground views, across the boundary and at multiple Sun angles. Compare
heights, normals and visual/physics contacts inside the retained site and in
the transition, include missing-data and misregistration rejection, and report
terrain loading, GPU cost and memory. The `TerrainLodStatus` resident cover and
derived-map readiness are necessary but do not alone prove the rendered result.
