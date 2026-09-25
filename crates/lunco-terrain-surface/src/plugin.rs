//! Bevy plugin for streamed terrain.
//!
//! Wires the authoritative DEM → oracle → physics pipeline. Camera-driven LOD,
//! derived visual maps, and terrain overlays are installed separately by
//! [`TerrainSurfaceVisualizationPlugin`], so a server does not schedule them.

use bevy::prelude::*;

/// Update phases owned by the terrain substrate.
///
/// The support index is deliberately a named phase rather than relying on
/// plugin insertion order. Physics producers can order their admission phase
/// before this set, so a body promoted during the current update is included in
/// the same support decision that gates its first physics step.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TerrainSurfaceSet {
    /// Build or recompose the authoritative DEM/oracle before visual products
    /// consume its surface identity.
    Build,
    /// Project changed Avian bodies/colliders/joints into terrain support data.
    PhysicsSupportCache,
    /// Apply streamed-tile shadow intent after the finalized BigSpace render
    /// frame has been published and any terrain-local cache decision is ready.
    RenderShadowBinding,
}

/// Authoritative terrain plugin — registers DEM construction, queries, layers,
/// and collider-ring systems. It is suitable for GUI, headless, and server apps.
pub struct TerrainSurfacePlugin;

impl Plugin for TerrainSurfacePlugin {
    fn build(&self, app: &mut App) {
        use lunco_settings::AppSettingsExt;
        app.init_resource::<lunco_physics::PhysicsInitializationExternalValidator>();
        app.world_mut()
            .resource_mut::<lunco_physics::PhysicsInitializationExternalValidator>()
            .0 = true;
        lunco_settings::ensure_download_settings(app);
        app.register_settings_section::<lunco_settings::TerrainSettings>();
        app.register_type::<crate::georef::TerrainGeoref>();
        app.register_type::<crate::georef::FlatSiteSurface>();
        // Terrain layer realization uses this shared quality setting in GUI
        // and headless compositions. Visual LOD resources are installed by
        // `TerrainSurfaceVisualizationPlugin` only.
        app.init_resource::<lunco_render::RenderingQualitySettings>();
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
        // The active physics support contract is inspectable at RUNTIME: the ring
        // component is reflected and registered, so the Inspector and reflection
        // API can inspect or retune the authored physics lattice live.
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
                .in_set(TerrainSurfaceSet::PhysicsSupportCache)
                .in_set(lunco_physics::PhysicsSupportSet::Consume),
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
                // `DemHeightField` immediately and publishes its shared
                // `TerrainSurfaceChange` via deferred commands. The sync point
                // makes the matching surface key and dirty bounds visible
                // before the ring chooses bounded or full invalidation.
                crate::collider_ring::update_collider_ring
                    .after(crate::terrain::finish_dem_restamp)
                    .after(crate::collider_ring::update_physics_support_cache),
                // Explicit ring edits mark resident tiles stale so the active
                // physics lattice reaches the ground already under the wheels.
                // Change-driven — the query is empty on every frame nobody edits
                // the ring.
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
        // This is a `lunco_physics::PhysicsHolds` hold, NOT a transport pause:
        // the user's play state is untouched, so the scene does not open
        // "paused" while the DEM bakes and resumes on its own the moment the
        // terrain is safe to step.
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
        // Validate authored dynamic poses after the terrain collider is live.
        // This is a diagnostic/admission boundary only: it never moves a body
        // or invents a support pose.
        app.add_systems(
            Update,
            crate::collider_ring::validate_initial_physics_poses
                .in_set(lunco_physics::PhysicsSupportSet::Consume),
        );
        // NO automatic overturn recovery. A vessel on its roof stays there until
        // someone recovers it — the Recover tool, or `recover::vessel(id)` from
        // rhai, both landing on the `RecoverVessel` command in `collider_ring`.
        // The old `FixedUpdate` auto-righting hid the terrain/suspension problem
        // that put the rover there in the first place.
    }
}

/// Camera-driven terrain products for applications with a visual surface.
///
/// Keep this plugin out of headless/server compositions. Physics terrain and
/// height queries remain available through [`TerrainSurfacePlugin`].
pub struct TerrainSurfaceVisualizationPlugin;

impl Plugin for TerrainSurfaceVisualizationPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<lunco_core_runtime::AsyncWorkAdmissionPlugin>() {
            app.add_plugins(lunco_core_runtime::AsyncWorkAdmissionPlugin);
        }
        app.register_type::<crate::stream_viz::TerrainVisualFocus>();
        app.init_resource::<lunco_render::RenderingQualitySettings>();
        crate::stream_viz::register_all_commands(app);
        app.add_observer(crate::stream_viz::invalidate_removed_shader_look_ready);
        app.init_resource::<crate::stream_viz::LodMeshCache>();
        app.init_resource::<crate::stream_viz::TerrainStreamStatus>();
        app.init_resource::<crate::stream_viz::TerrainDetailDemands>();
        app.init_resource::<lunco_viewport_core::SceneViewport>();
        app.init_resource::<crate::stream_viz::TerrainStreamFrameDriven>();
        app.init_resource::<crate::stream_viz::TerrainStreamCadence>();
        app.init_resource::<crate::stream_viz::TerrainCoverResults>();
        #[cfg(not(target_arch = "wasm32"))]
        app.init_resource::<crate::stream_viz::TerrainTileBakeResults>();
        app.add_systems(PreUpdate, crate::stream_viz::advance_terrain_stream_cadence);
        crate::overlay::register(app);
        crate::derived_layers::register(app);

        app.configure_sets(
            PostUpdate,
            TerrainSurfaceSet::RenderShadowBinding
                .after(big_space::prelude::BigSpaceSystems::PropagateLowPrecision),
        );
        app.add_systems(
            Update,
            (
                (
                    crate::stream_viz::cancel_removed_terrain_preparation,
                    crate::stream_viz::mark_terrain_visual_foci,
                    crate::stream_viz::collect_terrain_detail_demands,
                    crate::stream_viz::update_lod_tiles,
                    crate::stream_viz::retire_terrain_tiles,
                )
                    .chain(),
                crate::stream_viz::bind_terrain_maps_to_materials,
                crate::stream_viz::sync_removed_terrain_maps_to_materials,
                crate::stream_viz::despawn_orphaned_lod_tiles,
            )
                .in_set(lunco_core::RuntimeCycleSet::Visualization),
        );
        // Camera cover selection is presentation work at a wall-clock cadence.
        // Tile completion and residency commits remain in Update so the active
        // view can use completed work without waiting for another selection pass.
        app.add_systems(
            PostUpdate,
            crate::stream_viz::bind_shadow_cache_to_tiles
                .in_set(TerrainSurfaceSet::RenderShadowBinding)
                .in_set(lunco_core::RuntimeCycleSet::Visualization),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visual_terrain_work_is_opt_in_at_composition() {
        let mut app = App::new();
        app.add_plugins(TerrainSurfacePlugin);
        assert!(
            app.world()
                .get_resource::<crate::stream_viz::TerrainDetailDemands>()
                .is_none()
        );
        assert!(
            app.world()
                .get_resource::<crate::stream_viz::TerrainStreamCadence>()
                .is_none()
        );

        app.add_plugins(TerrainSurfaceVisualizationPlugin);
        assert!(
            app.world()
                .get_resource::<crate::stream_viz::TerrainDetailDemands>()
                .is_some()
        );
        assert!(
            app.world()
                .get_resource::<crate::stream_viz::TerrainStreamCadence>()
                .is_some()
        );
    }
}
