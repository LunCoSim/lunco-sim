# DEM Georeferencing — the raster is the source of truth

**Status:** implemented — [`crates/lunco-geotiff`](../../crates/lunco-geotiff/src/lib.rs)
is the one place georeferencing is encoded and decoded, shared by the writer
(`lunco-assets`) and the reader (`lunco-terrain-bake`). Companion to
[`55-scene-addressing-and-roots.md`](55-scene-addressing-and-roots.md) and
[`56-asset-resolution-and-cache.md`](56-asset-resolution-and-cache.md) — same
principle (*one source of truth, derive the rest*), applied to spatial reference
rather than asset identity.

## Principle

**The raster carries its own georeferencing, and everything else derives from
it.** The raster is the only copy that cannot disagree with the pixels it
describes. Position is never re-stated in a sidecar: `metadata.yaml` keeps only
provenance (source product, URL, sha256) — facts the raster cannot carry.

Without tags, an external GIS opens the heightmap as a bare grid in *pixel*
units: run Slope on it and every angle is wrong by the ground-sample factor,
with no warning. With them, QGIS and GDAL open the raster correctly scaled in
metres, and a DEM that leaves the pipeline can come back without its position
being re-supplied out of band.

## The frame we write

A **local metric frame centred on the crop**, not a planetary projection. The
sim works in scene metres with the crop centre at the origin, and the file says
exactly that: a user-defined equirectangular projection on the target body's
sphere, with its natural origin *at* the crop centre. Near its own origin,
projected metres ARE local metres — the file's frame and the sim's frame are the
same frame, no conversion, no sign to get wrong — while the projection
parameters record *where on the body* the crop sits, which a bare local grid
could not express.

Spacing is **node-based**: samples are nodes, so an extent spans `res − 1`
intervals (the Apollo 15 crop is the live case — 1002 m over 512 samples gives
1002/511 m spacing; getting this wrong scales the whole terrain by 512/511).

`GeoTransform` is the in-memory form — `pixel_size_m` (one spacing, both axes),
`origin_x_m`/`origin_y_m` (model position of the upper-left sample; +X east,
+Y north, Y decreasing per row), `body_radius_m`, and the crop-centre
`center_lat_deg`/`center_lon_deg`. `GeoTransform::centred_square` builds it for
a centred square crop.

## Writer contract (`write_geo_tags`)

Called by the bake (`crates/lunco-assets/src/process.rs`) on every emitted
heightmap. Writes:

- `ModelPixelScale` — one spacing for both axes (square pixels only).
- `ModelTiepoint` — raster (0,0) ↦ the model's upper-left corner.
- A `GeoKeyDirectory` (encoded by `geotiff-core`, which owns key ordering,
  value-offset indices, and ASCII terminators):
  - `GTModelType` = projected; `GTRasterType` = **`RasterPixelIsPoint`**,
    matching the node-based spacing.
  - User-defined geographic CS on the body's sphere:
    `GeogSemiMajorAxis` = `GeogSemiMinorAxis` = body radius, angular units
    degrees, `GeogCitation` naming the body for a human (`"Moon 2000"`).
  - User-defined projected CS: equirectangular, linear units metres,
    `ProjStdParallel1` = centre latitude, `ProjNatOriginLong` = centre
    longitude.

## Reader contract (`read_geo_tags`)

Reads back our own files, and third-party GeoTIFFs that fit the pipeline's
model:

- `ModelPixelScale` is required, and X/Y spacing must agree — the whole
  pipeline speaks ONE spacing for both axes, so an anisotropic raster is
  rejected here, not sampled skewed.
- `ModelTiepoint` is required and may anchor **any** raster coordinate — a
  third-party raster's tiepoint need not sit at (0,0); the reader shifts it
  back to the model position of raster (0,0).
- GeoKeys are resolved through the key directory, never positionally — a
  third-party file orders its doubles however it likes.
- `GTRasterType`: `PixelIsPoint` is taken as-is; `PixelIsArea` — the spec (and
  GDAL) default when the key is absent — anchors the *outer corner* of the
  corner pixel, so the origin is shifted half a pixel inward to the node
  convention the pipeline speaks; any other value is rejected.
- The body radius is read when present; its absence is not an error (a
  third-party raster may carry an EPSG code instead of user-defined axes).
- Failures are `GeoReadError` variants that name what is missing
  (`NoPixelScale`, `NoTiepoint`, `Malformed(…)`) — the caller's job is to tell
  a human how to fix the file, not to fail quietly.

## Consumers: no fallback

`crates/lunco-terrain-bake/src/dem.rs` (`read_geotiff_transform`,
`height_grid_from_geotiff`) takes the raster's extent from its own tags —
**no fallback**. A raster without georeferencing fails with
`DemError::NoGeoreferencing`, because a guessed extent would put terrain
silently at the wrong scale. This is the load-error-not-silent-precedence rule
in practice.

## `lunco:anchor:*` does not disappear

The USD anchor is load-bearing beyond the DEM: `lunco-usd-sim/src/celestial.rs`
uses it to place a **site** for sun/Earth geometry, and a scene may author an
anchor with no terrain at all. The rule is therefore narrower than "delete it":

> Where a scene references a georeferenced DEM, the DEM is authoritative for
> that terrain's extent and scale. A scene with no DEM authors its anchor
> exactly as before.

Author position once, in whichever artifact actually has it.

## Open: the lunar reference frame is not declared

The tags say *where on a sphere* the crop sits, but not **which lunar frame**
the coordinates are in. LRO/LOLA/NAC products are MOON_ME (mean-Earth); the
principal-axis frame MOON_PA differs by ≈ 875 m on the surface — larger than a
whole crop's placement tolerance. Nothing in the GeoKeys or the provenance
records ME vs PA, so a consumer must assume it. The fix is one GeoKey plus a
`frame: MOON_ME` provenance field alongside the existing source-product
provenance.
