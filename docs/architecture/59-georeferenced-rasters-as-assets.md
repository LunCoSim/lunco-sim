# Georeferenced Rasters as Assets

**Status:** proposal, 2026-07-19. Companion to
[`57-dem-georeferencing.md`](57-dem-georeferencing.md) (writing georeferencing out)
and [`56-asset-resolution-and-cache.md`](56-asset-resolution-and-cache.md)
(identity → bytes). This one is about reading georeferencing **in**.

Driven by the Summer Space School handoff: an external GIS team analyses our DEM
and returns rasters — slope, cast shadow, viewshed — plus a route. Today none of
them can enter a scene.

## Do not build an importer

The instinct is a GIS import subsystem. That would be a mistake: it duplicates
machinery that already exists and works.

We already have identity → bytes resolution (`lunco://`, `twin://`, the
content-addressed cache), hot-reload, and dependency tracking — all of it hanging
off Bevy's asset system. An importer sitting beside that gets none of it, and
every feature it eventually needs (caching, reload, resolution) is a
re-implementation.

**A georeferenced raster is an asset. Give it a loader.**

```
@twin://SummerSpaceSchool/analysis/slope.tif@
        │
        └── AssetLoader ──> GeoRaster { pixels, georef, nodata }
                                 │
             ┌───────────────────┼────────────────────┐
             ▼                   ▼                    ▼
        DEM height field    terrain raster layer   validation
```

Everything upstream of the loader is solved. Everything downstream is a consumer.

## The asset

```rust
/// A raster that knows where it is.
pub struct GeoRaster {
    pub width: u32,
    pub height: u32,
    pub data: RasterData,
    /// Pixel→world mapping read from the file's GeoTIFF tags. `None` for a raster
    /// that carries none — which is a fact worth propagating, not papering over.
    pub georef: Option<GeoTransform>,
    pub nodata: Option<f64>,
}

pub enum RasterData {
    /// Elevation, or any continuous scalar field.
    F32(Vec<f32>),
    /// Colour or a classified/paletted product.
    Rgba8(Vec<u8>),
}
```

`georef` is `Option` deliberately. A raster with no tags is not an error — our own
shipped `heightmap.tif` has none today (see `57`) — but the distinction must
survive into the consumer, which can then say *"this raster cannot be verified
against the DEM"* rather than silently assuming alignment.

### Why not just `Image`

Bevy's `Image` is a GPU upload target. It has no room for a geotransform, no f32
elevation semantics, and no nodata. Loading a DEM as an `Image` would throw away
exactly the information this document exists to preserve. `GeoRaster` **produces**
an `Image` for the colour case; it is not one.

## The extension does not say what the raster means

A `.tif` may be elevation or imagery. Resolving that in the loader would be
guesswork.

**Resolve it at the consumer, in USD.** The asset is "a georeferenced raster"; the
scene attribute says what role it plays:

```usda
asset lunco:layer:demSource     = @terrain/apollo15@              # elevation
asset lunco:layer:albedoSource  = @twin://…/analysis/slope.tif@   # colour
```

This is the same split the codebase already makes between a format and its meaning,
and it means one loader serves every raster role we add later.

## Validation is the point

With `georef` present, a consumer can do what no PNG pipeline can:

1. Compare the imported raster's footprint against the DEM's.
2. Refuse — loudly, on the `StatusBus` — when they disagree.

Today's alternative is the convention *"same pixel dimensions ⇒ same ground"*.
That happens to hold (tile UVs are already DEM-global,
`tile_mesh.rs:76`, so a same-size raster lands pixel-for-pixel), and it is exactly
the kind of silent assumption that produced the 34.4° WSW error and the six copies
of `26.6`. A hazard overlay that is subtly misaligned is worse than no overlay:
it is confidently wrong about which ground is safe.

> This closes the loop with `57`. There we write tags so our DEM can leave; here we
> read them so their analysis can return, **and be checked on arrival**.

## What has to be built

Ordered, with the load-bearing item flagged.

1. **`GeoRaster` + `AssetLoader` for `.tif`.** The decoder already exists —
   `decode_geotiff_f64` (`terrain-bake/src/dem.rs:111`) — but is a private height
   path. Lift it to return `GeoRaster`, add the geo tags (`ModelPixelScale`,
   `ModelTiepoint`, `GeoKeyDirectory`) that `dem.rs:108` currently ignores, and
   make the existing DEM path its first consumer. The `tiff` crate is pure Rust,
   so this works on wasm too.
2. **`asset lunco:layer:albedoSource` / `mineralSource`** in `schema.usda`, beside
   `demSource:180`.
3. **A `raster` layer kind** — a parser next to `terrain_layers/shader.rs`,
   registered at `terrain_layers/mod.rs:328`, reading the asset via the existing
   `LayerAttrSource::get_asset` (`mod.rs:209`).
4. **⚠ A way for a `TerrainLayer` to publish a texture.** The trait has four verbs
   — `height_modifier`, `stamp`, `scatter`, `configure` — and none of them can hand
   an image to the render path. **This is the only genuinely new architecture**;
   1–3 follow existing patterns exactly.
5. **Footprint validation** against the DEM, reporting on the `StatusBus`.

The GPU side needs nothing new: `TextureLayer` already declares six slots
(`materials/src/look.rs:73`), `terrain_layered.wgsl` binds them with mix weights,
and `shader_look.rs:73-83` maps every one. `Albedo` and `Mineral` are simply never
populated on terrain and their weights default to 0.

### Scoping trap

The albedo/mineral bindings live in `terrain_layered.wgsl` — the **static-mesh**
path. The streamed-tile shader `terrain_geomorph.wgsl` declares only bindings 6–11.
Any scene using streamed terrain (the school twin does) needs those bindings added
to the streaming shader as well. Confirm which path the target scene renders
through before estimating.

## Routes are not rasters

The fourth product — the route — needs none of this. Waypoints are USD prims
(`docs/waypoints-in-usd-design.md`), so a GIS route becomes a `.usda` overlay via a
coordinate conversion at authoring time. **No engine change.** It is also the fix
for the duplicated `route()` in the school lessons, so it pays twice and should be
done first regardless of whether raster import ever happens.

## Recommendation

**Not before 2026-07-25.** The school works without it: the remote-sensing team's
analysis lives in QGIS, the sim renders its own hazard overlay, and the two agree
because they share the DEM and the sun vector the handoff pack publishes.

When it is built, the highest-value first consumer is the **cast-shadow map** —
`ShadowCache` is already a bound slot (`stream_viz.rs:782`) filled by the engine's
own horizon bake, so importing one substitutes a producer rather than inventing a
slot.
