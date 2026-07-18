# DEM Georeferencing — the raster should be the source of truth

**Status:** open problem, written up 2026-07-18. No code changed. Companion to
[`55-scene-addressing-and-roots.md`](55-scene-addressing-and-roots.md) and
[`56-asset-resolution-and-cache.md`](56-asset-resolution-and-cache.md) — same
principle (*one source of truth, derive the rest*), applied to spatial reference
rather than asset identity.

Found while assembling the ДЗЗ handoff pack for the Summer Space School twin,
where an external GIS team must analyse the same DEM the sim runs on.

## The defect

**We emit a DEM with no georeferencing, then re-state that georeferencing twice
elsewhere, and validate neither against the other.**

The heightmap this pipeline writes
(`crates/lunco-assets/src/process.rs:543`, `TiffEncoder`) is a **plain TIFF**.
Verified on the shipped Apollo 15 crop: 14 TIFF tags, and of the GeoTIFF set —
`ModelPixelScale` (33550), `ModelTiepoint` (33922), `ModelTransformation`
(34264), `GeoKeyDirectory` (34735) — **none are present**. The doc comment at
`crates/lunco-terrain-bake/src/dem.rs:7` calls the output a "float32 GeoTIFF".
It is not one.

The same facts then live in two other places:

| Fact | GeoTIFF tags | `metadata.yaml` | USD |
|---|---|---|---|
| Centre lat/lon | *(absent)* | `coordinates.lat/lon` | `lunco:anchor:lat` / `:lon` |
| Body | *(absent)* | — | `lunco:anchor:body` |
| Ground sample | *(absent)* | `size_x_m` / `resolution_x` | — |
| Elevation range | *(absent)* | `elevation_min_m` / `_max_m` | — |

`metadata.yaml` is parsed at `dem.rs:58-101`; the USD anchor is read in four
crates (`lunco-usd-terrain/src/lib.rs:1090`, `lunco-terrain-surface/src/georef.rs`,
`lunco-celestial/src/placement.rs:474`, `lunco-usd-sim/src/celestial.rs:67`).

**Nothing checks that the two agree.** `dem.rs:151` validates `resolution_x`
against the raster's actual width — good, and precisely the pattern that is
missing for position. A scene whose `lunco:anchor:lat` disagrees with its DEM's
`coordinates.lat` loads silently and puts the terrain somewhere the survey says
it isn't. In the school twin the two agree, by hand, with nothing enforcing it.

## Why it bites

1. **External tools get nothing.** QGIS opens our heightmap as a bare 512×512
   grid in *pixel* units. Run Slope on it and every angle is wrong by the
   ground-sample factor — 1.957 for the school DEM — with no warning. A GIS team
   analysing our terrain silently produces numbers that do not match the sim.
   The stopgap shipped for the school is a hand-written `.tfw` worldfile, which
   is exactly the sidecar this document argues against: it lives outside the
   raster, in a pipeline-managed folder, and is lost on the next re-process.
2. **Round-tripping is lossy.** A DEM that leaves our pipeline cannot come back
   without its position being re-supplied out of band.
3. **Three-way drift.** Three copies, no cross-validation, and the raster — the
   thing that actually holds the data — is the copy carrying *no* position at all.

## Target

**The raster carries its own georeferencing, and everything else derives from
it.**

1. **Write real GeoTIFF tags** in the bake step: `ModelPixelScale` +
   `ModelTiepoint`, and a `GeoKeyDirectory` describing a user-defined geographic
   CS on the target body's sphere. This is standard practice for planetary data —
   GDAL and ISIS both round-trip lunar equirectangular this way, with the sphere
   radius carried in `GeogSemiMajorAxis` / `GeogSemiMinorAxis`. The `tiff` crate
   we already encode with supports arbitrary tags.
2. **Read them back** into a `DemGeoref` and let it feed terrain placement.
3. **Stop authoring what the raster knows.** Where a scene references a
   georeferenced DEM, its position comes from the raster.
4. **`metadata.yaml` keeps only what the raster cannot carry** — provenance
   (source product, URL, sha256), which is `Assets.toml`'s domain anyway. The
   elevation min/max are a *cache* of a value derivable from the payload; keep
   them if useful, but they are not truth.

### `lunco:anchor:*` does not simply disappear

It is load-bearing beyond the DEM: `lunco-usd-sim/src/celestial.rs:67` uses the
same anchor to place a **site** for sun/Earth geometry, and a scene may author an
anchor with no terrain at all. So the rule is narrower than "delete it":

> Where a scene references a georeferenced DEM, the DEM is authoritative for that
> terrain's position. The scene must not re-author it, and a disagreement is a
> **load error**, not a silent precedence rule.

A scene with no DEM keeps authoring its anchor exactly as today. This also
answers what a *user's* twin does: author position once, in whichever artifact
actually has it.

## Open questions

- **Which CRS do we write?** A true lunar equirectangular (centre lon, sphere
  1737400 m) is the interoperable answer, but the sim works in a local metric
  frame centred on the crop, and every conversion is a chance to flip a sign.
  The school pack deliberately tells the GIS team to stay in the local frame for
  exactly that reason. Writing *both* — a projected CS plus an explicit local
  tiepoint — is possible and needs a decision.
- **Axis convention must be stated in the tags, not in prose.** Our scene frame
  is north = −Z, east = +X, so a north-up raster's row order and the scene's Z
  differ in sign. That flip currently lives only in `SURVEY.md`'s method section
  and in the handoff doc's checklist. It should be implied by the geotransform.
- **Migration.** Existing baked DEMs have no tags. Re-bake from source, or
  tolerate untagged rasters with a loud warning and the USD anchor as fallback?
  The fallback path is how three-way drift got here, so prefer re-baking.

## Not blocking the 2026-07-25 school

The worldfile unblocks the GIS team today. This is the correct fix afterwards,
and the school is the evidence for why it matters: the first time an external
team touched our DEM, the missing tags were the first thing they would have hit.
