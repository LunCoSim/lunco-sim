//! Bevy plugin for streamed terrain.
//!
//! Wires the full DEM → oracle → streaming pipeline: the terrain build/edit
//! systems ([`crate::terrain`]), the `TerrainHeight` scripting query
//! ([`crate::query`]), off-thread derived surface/normal maps
//! ([`crate::derived_layers`]), camera-driven CDLOD visual tile streaming
//! ([`crate::stream_viz`]), the composable USD terrain-layer stack
//! ([`crate::terrain_layers`]), and the per-body heightfield collider ring +
//! physics-hold / tunnel / overturn rescues ([`crate::collider_ring`]). The
//! visual quality knobs live in [`lunco_render::RenderingQualitySettings`].

use bevy::prelude::*;

/// Update phases owned by the terrain substrate.
///
/// The support index is deliberately a named phase rather than relying on
/// plugin insertion order. Physics producers can order their admission phase
/// before this set, so a body promoted during the current update is included in
/// the same support decision that gates its first physics step.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TerrainSurfaceSet {
    /// Project changed Avian bodies/colliders/joints into terrain support data.
    PhysicsSupportCache,
}

/// Streamed-terrain plugin — registers the DEM build, streaming, layer, and
/// collider-ring systems (see the module docs for the pipeline).
pub struct TerrainSurfacePlugin;

impl Plugin for TerrainSurfacePlugin {
    fn build(&self, app: &mut App) {
        use lunco_settings::AppSettingsExt;
        app.register_settings_section::<lunco_settings::TerrainSettings>();
        app.register_type::<crate::georef::TerrainGeoref>();
        app.register_type::<crate::georef::FlatSiteSurface>();
        app.register_type::<crate::stream_viz::TerrainShaderMode>();
        app.register_type::<crate::stream_viz::TerrainVisualFocus>();
        // The streamed mesh cache and LOD controls are rendering-quality resources even when
        // this plugin runs headless: the CPU-side cache still needs the same
        // authoritative limit as the graphical client. The workbench's
        // settings registration may replace this default with persisted user
        // values later in plugin construction.
        app.init_resource::<lunco_render::RenderingQualitySettings>();
        // `SetTerrainRenderingQuality` — the same knobs, addressable from the API/scripts.
        crate::stream_viz::register_all_commands(app);
        app.init_resource::<crate::stream_viz::LodMeshCache>();
        app.init_resource::<crate::stream_viz::TerrainStreamStatus>();
        app.init_resource::<crate::stream_viz::TerrainDetailDemands>();
        // Off by default: interactive play wants real-time-paced streaming. Set by
        // `lunco-luncosim` for the duration of an offline recording so the captured
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
        // Publish the terrain oracle's complete rigid pose in the active
        // physics frame before any Update-stage input/tool consumer runs.
        app.add_systems(
            PreUpdate,
            crate::surface_query::update_terrain_physics_frame_poses,
        );
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
        // Contact tuning is reachable at RUNTIME, not only at compile time: both
        // types are reflected + registered, so the Inspector derives an editor for
        // them and the reflection API can set them live.
        app.register_type::<crate::collider_ring::TerrainColliderRing>();
        app.register_type::<avian3d::prelude::NarrowPhaseConfig>();
        app.init_resource::<crate::collider_ring::PhysicsSupportCache>();
        app.configure_sets(Update, TerrainSurfaceSet::PhysicsSupportCache);
        // Physics owns the support contract; this cache turns Avian's change
        // events into a stable assembly projection. Ring selection and the
        // readiness hold both consume that projection instead of rebuilding the
        // physics topology on every render frame.
        app.add_systems(
            Update,
            crate::collider_ring::update_physics_support_cache
                .in_set(TerrainSurfaceSet::PhysicsSupportCache),
        );
        app.add_systems(
            Update,
            (
                (
                    crate::stream_viz::mark_terrain_visual_foci,
                    crate::stream_viz::collect_terrain_detail_demands,
                    crate::stream_viz::update_lod_tiles,
                    crate::stream_viz::retire_terrain_tiles,
                )
                    .chain(),
                // Late-bind: derived maps / shadow cache finish baking seconds
                // after the first tiles exist — restate the resident tiles' looks
                // (no tile churn, no re-bake).
                crate::stream_viz::bind_derived_maps_to_tiles,
                // Authored maps land later still — the layer binder needs the
                // composed stage — and re-land on every live weight/map edit.
                crate::stream_viz::bind_authored_maps_to_tiles,
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
        app.init_resource::<crate::terrain_layers::TerrainScatterQualitySignature>();
        app.add_systems(
            Update,
            crate::terrain_layers::mark_terrain_scatter_quality_changed
                .run_if(resource_changed::<lunco_render::RenderingQualitySettings>)
                .before(crate::terrain_layers::scatter_terrain_layers),
        );
        app.add_systems(
            Update,
            crate::terrain_layers::scatter_terrain_layers
                .after(crate::terrain::start_dem_restamp)
                .after(crate::terrain::finish_dem_restamp),
        );
        // The frame contract the whole analytic surface rests on: a DEM terrain
        // is grid-direct at the origin cell, so oracle coordinates ARE world-grid
        // coordinates (`crate::surface_query`). Checked when a terrain appears,
        // not assumed in a comment — an authored transform on a terrain prim is
        // honoured by no two subsystems the same way.
        app.add_systems(Update, crate::surface_query::report_unreachable_dem_frame);
        // M7 (physics): opt-in canonical-resolution heightfield COLLIDER ring.
        // Inert unless a DEM is built with `collider_ring`; then it replaces the
        // static collider with deterministic per-tile colliders streamed around the
        // dynamic physical bodies and their support footprints. See
        // `crate::collider_ring`.
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
                    .after(crate::terrain::finish_dem_restamp)
                    .after(crate::collider_ring::update_physics_support_cache),
                // Graphics quality is read directly by visual LOD selection;
                // retune existing collider rings from the same profile before
                // the Changed<TerrainColliderRing> invalidation runs.
                crate::collider_ring::sync_ring_quality
                    .run_if(resource_changed::<lunco_render::RenderingQualitySettings>)
                    .before(crate::collider_ring::invalidate_ring_on_retune),
                // Quality projection and explicit ring edits both mark resident
                // tiles stale so the active lattice reaches the ground already
                // under the wheels. Change-driven — the query is empty on every
                // frame nobody edits the ring.
                crate::collider_ring::invalidate_ring_on_retune
                    .before(crate::collider_ring::update_collider_ring),
                // Change-driven: early-outs unless a `TerrainColliderRing`
                // removal event fired this frame.
                crate::collider_ring::despawn_orphaned_collider_tiles,
            ),
        );
        // Freeze the sim while a DEM terrain is still building — and, on ring
        // terrains, until the ring tiles under every dynamic body are resident —
        // so dynamic bodies don't fall through the not-yet-ready collider (esp. web,
        // where the DEM load is slow). See `collider_ring::hold_physics_until_dem_ready`.
        // This is a `lunco_time::SimHolds` hold, NOT a transport pause: the user's
        // play state is untouched, so the scene does not open "paused" while the
        // DEM bakes and resumes on its own the moment the terrain is safe to step.
        app.add_systems(
            Update,
            crate::collider_ring::hold_physics_until_dem_ready
                .after(crate::collider_ring::update_collider_ring),
        );
        // NOTE: the "tunnel rescue" safety net was DELETED. It masked the real
        // defect — physics resumed one frame before the ring collider was live in
        // avian's broad-phase (`hold_physics_until_dem_ready` gated on queued map
        // membership, now on `ColliderAabb` liveness) AND the Dynamic wheels had no
        // CCD, so they free-fell through the one-sided heightfield. Both are fixed
        // (`SweptCcd` on the wheels + liveness-gated hold), so a body can no longer
        // end up under the terrain and needs no reseat.
        // One-time drop-onto-terrain placement for freshly-activated physical
        // newly activated assemblies (marked `NeedsGroundSettle` by their physics
        // owner): lift the
        // assembly so its wheels clear the one-sided heightfield instead of starting
        // embedded (authored chassis-at-surface + wheels-hang-below) and sinking.
        app.add_systems(Update, crate::collider_ring::settle_grounded_assemblies);
        // NO automatic overturn recovery. A vessel on its roof stays there until
        // someone recovers it — the Recover tool, or `recover::vessel(id)` from
        // rhai, both landing on the `RecoverVessel` command in `collider_ring`.
        // The old `FixedUpdate` auto-righting hid the terrain/suspension problem
        // that put the rover there in the first place.
    }
}
