//! Bevy plugin for streamed terrain.
//!
//! Wires the full DEM → oracle → streaming pipeline: the terrain build/edit
//! systems ([`crate::terrain`]), the `TerrainHeight` scripting query
//! ([`crate::query`]), off-thread derived surface/normal maps
//! ([`crate::derived_layers`]), camera-driven CDLOD visual tile streaming
//! ([`crate::stream_viz`]), the composable USD terrain-layer stack
//! ([`crate::terrain_layers`]), and the per-body heightfield collider ring +
//! physics-hold / tunnel / overturn rescues ([`crate::collider_ring`]). The
//! runtime LOD knobs live in [`crate::stream_viz::TerrainLodConfig`].

use bevy::prelude::*;

/// Streamed-terrain plugin — registers the DEM build, streaming, layer, and
/// collider-ring systems (see the module docs for the pipeline).
pub struct TerrainSurfacePlugin;

impl Plugin for TerrainSurfacePlugin {
    fn build(&self, app: &mut App) {
        use lunco_settings::AppSettingsExt;
        app.register_settings_section::<lunco_settings::TerrainSettings>();
        app.register_type::<crate::georef::TerrainGeoref>();
        app.register_type::<crate::stream_viz::TerrainShaderMode>();
        // Runtime-tunable LOD knobs (Inspector → "Terrain LOD") + the tile-mesh cache.
        app.init_resource::<crate::stream_viz::TerrainLodConfig>();
        app.register_type::<crate::stream_viz::TerrainLodConfig>();
        // `SetTerrainLod` — the same knobs, addressable from the API/scripts.
        crate::stream_viz::register_all_commands(app);
        app.init_resource::<crate::stream_viz::LodMeshCache>();
        app.init_resource::<crate::stream_viz::TerrainStreamStatus>();
        // Off by default: interactive play wants real-time-paced streaming. Set by
        // `lunco-sandbox` for the duration of an offline recording so the captured
        // tile set is a function of the frame index rather than of thread
        // scheduling. See `stream_viz::TerrainStreamLockstep`.
        app.init_resource::<crate::stream_viz::TerrainStreamLockstep>();
        // M3: spawn a static DEM terrain (mesh + heightfield collider) on the
        // `SpawnDemTerrain` command. See `crate::terrain`.
        crate::terrain::register(app);
        // Expose the DEM height field to the API / scripting surface as
        // `query("TerrainHeight", #{x, z})` — analytic height/normal/slope, no
        // raycast. See `crate::query`.
        crate::query::register_terrain_queries(app);
        // Analysis-overlay VIEW: the `TerrainOverlayParams` resource + `SetTerrainOverlay`
        // command + live-sync system that paints the slope-hazard transfer over the lit
        // tiles (in-material shading plane of Data→Transfer→Blend). See `crate::overlay`.
        crate::overlay::register(app);
        // P3b: bake DEM-derived surface (rough/AO/hazard) + normal layers off the
        // main thread and publish them as `TerrainDerivedMaps`. Inert headless
        // (gated on render assets existing). See `crate::derived_layers`.
        crate::derived_layers::register(app);
        // S3 (visual-only): opt-in camera-driven CDLOD tile streaming for SEEING
        // LODs. Inert unless a DEM is built with `lod_viz`. Physics still rides the
        // static heightfield collider. See `crate::stream_viz`.
        //
        // NO material store is initialised here any more. A tile states its
        // appearance as a `ShaderLook` and this crate never touches
        // `Assets<ShaderMaterial>` — so the headless server needs no render assets
        // and no `#[cfg]`; it simply never adds `LuncoRenderPlugin`, and the looks
        // sit in the world as inspectable data. See docs/architecture/render-decoupling.md.
        app.add_systems(
            Update,
            (
                crate::stream_viz::update_lod_tiles,
                // Late-bind: derived maps / shadow cache finish baking seconds
                // after the first tiles exist — restate the resident tiles' looks
                // (no tile churn, no re-bake).
                crate::stream_viz::bind_derived_maps_to_tiles,
                crate::stream_viz::bind_shadow_cache_to_tiles,
                // Change-driven: early-outs unless a `TerrainLodViz` removal
                // event fired this frame (stays in `Update` so its
                // `RemovedComponents` reader drains every frame).
                crate::stream_viz::despawn_orphaned_lod_tiles,
            ),
        );
        // Composable TERRAIN LAYER stack (authored as USD child layer prims; craters
        // stamp into the grid, rocks scatter on the surface). The parser registry maps
        // each `lunco:layer` type → a parser; register more with `App::add_terrain_layer`
        // — no changes to the build/scatter/regen systems. See `crate::terrain_layers`.
        app.init_resource::<crate::terrain_layers::TerrainLayerParserRegistry>();
        // Boulder meshes + the single boulder material, shared by every rock layer
        // (procedural scatter AND `PlaceRock`) so rocks batch instead of each one
        // adding a draw call + a bind group.
        app.init_resource::<crate::terrain_layers::SharedRockAssets>();
        app.add_systems(Update, crate::terrain_layers::scatter_terrain_layers);
        // M7 (physics): opt-in per-rover canonical-res heightfield COLLIDER ring.
        // Inert unless a DEM is built with `collider_ring`; then it replaces the
        // static collider with deterministic per-tile colliders streamed around the
        // dynamic bodies. See `crate::collider_ring`.
        app.add_systems(
            Update,
            (
                // AFTER the restamp swap: `finish_dem_restamp` writes the new
                // `DemHeightField` immediately (Mut) but hands the bounded
                // `ColliderDirtyRegion` over via deferred commands. Unordered,
                // the ring could observe the new oracle key with no region in
                // sight and fall back to invalidating the WHOLE ring on every
                // edit; the `.after` also inserts the sync point that makes the
                // region visible the same frame.
                crate::collider_ring::update_collider_ring
                    .after(crate::terrain::finish_dem_restamp),
                // Change-driven: early-outs unless a `TerrainColliderRing`
                // removal event fired this frame.
                crate::collider_ring::despawn_orphaned_collider_tiles,
            ),
        );
        // Freeze the sim while a DEM terrain is still building — and, on ring
        // terrains, until the ring tiles under every dynamic body are resident —
        // so rovers don't fall through the not-yet-ready collider (esp. web,
        // where the DEM load is slow). See `collider_ring::hold_physics_until_dem_ready`.
        // This is a `lunco_time::SimHolds` hold, NOT a transport pause: the user's
        // play state is untouched, so the scene does not open "paused" while the
        // DEM bakes and resumes on its own the moment the terrain is safe to step.
        app.add_systems(Update, crate::collider_ring::hold_physics_until_dem_ready);
        // NOTE: the "tunnel rescue" safety net was DELETED. It masked the real
        // defect — physics resumed one frame before the ring collider was live in
        // avian's broad-phase (`hold_physics_until_dem_ready` gated on queued map
        // membership, now on `ColliderAabb` liveness) AND the Dynamic wheels had no
        // CCD, so they free-fell through the one-sided heightfield. Both are fixed
        // (`SweptCcd` on the wheels + liveness-gated hold), so a body can no longer
        // end up under the terrain and needs no reseat.
        // One-time drop-onto-terrain placement for freshly-activated physical
        // rovers (marked `NeedsGroundSettle` in `activate_dynamic_bodies`): lift the
        // assembly so its wheels clear the one-sided heightfield instead of starting
        // embedded (authored chassis-at-surface + wheels-hang-below) and sinking.
        app.add_systems(Update, crate::collider_ring::settle_grounded_assemblies);
        // Overturn recovery: a `KeepUpright` vessel resting on its roof gets
        // righted (whole jointed assembly, rigidly) after a settle delay.
        app.add_systems(FixedUpdate, crate::collider_ring::rescue_overturned_vessels);
    }
}
