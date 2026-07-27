//! Bevy plugin: owns the shared [`ObstacleFieldSpec`] (the tunable crater/rock
//! knobs) and the [`UpdateObstacleFieldSpec`] command that edits it.
//!
//! The spec is *consumed* by the DEM terrain (`lunco-terrain-surface`), which
//! stamps craters into the DEM height grid and scatters rocks on the DEM surface.
//! This crate no longer builds any ground of its own: the flat-slab arena it used
//! to generate (the pre-DEM scaffold) floated on / z-fought the DEM and was
//! unreachable from any production path.
//!
//! The generation core the terrain calls is pure and deterministic and lives in
//! `sampler`/`field`/`assets`/`rock`.

use bevy::asset::RenderAssetUsages;
use bevy::prelude::*;
// `bevy_mesh`, not `bevy::render` — `PrimitiveTopology` is re-exported from
// `wgpu-types` and needs no render pipeline. See docs/architecture/render-decoupling.md.
use bevy_mesh::Indices;
use bevy_mesh::PrimitiveTopology;
use lunco_core::{on_command, register_commands, Command};

use crate::spec::{ObstacleFieldSpec, Pattern};

/// Registers the obstacle-field generator: the shared `ObstacleFieldSpec`
/// resource and the [`UpdateObstacleFieldSpec`] command that edits it. The DEM
/// terrain observes that command and rebuilds its crater/rock layers.
pub struct ObstacleFieldPlugin;

/// Replace the obstacle-field spec and regenerate the field. The whole spec is
/// sent, not a delta, so a caller that means to change one knob must send the
/// others back unchanged. Journaled as a `DomainKind::ObstacleField` op.
#[Command(default)]
pub struct UpdateObstacleFieldSpec {
    /// The complete new spec — density, seed, extent, size distribution.
    pub spec: ObstacleFieldSpec,
}

#[on_command(UpdateObstacleFieldSpec)]
fn on_update_obstacle_field_spec(
    trigger: On<UpdateObstacleFieldSpec>,
    mut spec: ResMut<ObstacleFieldSpec>,
    // Journal handle: present once wired (networked / persisted sessions). Every
    // local spec edit is recorded as a `DomainKind::ObstacleField` op so it
    // persists + syncs through the journal plane. Remote peers' edits arrive via
    // the replay leg (which sets the resource directly, NOT this command), so this
    // handler only ever fires for a *local* edit — no wire re-record to guard.
    journal: Option<Res<lunco_doc_bevy::JournalResource>>,
) {
    let next = trigger.event().spec.clone();
    if let Some(journal) = journal.as_ref() {
        crate::journal::record_set_spec(journal, &spec, &next);
    }
    *spec = next;
    info!("[ObstacleField] Spec updated; DEM terrain observers rebuild from it.");
}

register_commands!(on_update_obstacle_field_spec);

impl Plugin for ObstacleFieldPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<ObstacleFieldSpec>()
            .register_type::<UpdateObstacleFieldSpec>()
            .init_resource::<ObstacleFieldSpec>();
        // This crate is a pure generator (core + terrain only) and never edits the
        // stage: the DEM terrain owns the ground, and there is no field root to
        // displace a USD-authored one.
        register_all_commands(app);
    }
}

/// Build a Bevy `Mesh` from raw height-grid vertex arrays. The single
/// `MeshData`/`TileMesh` → `Mesh` glue, shared by the static DEM terrain and the
/// streaming LOD tiles (was duplicated in three places).
///
/// `usages` is the caller's choice because the two consumers genuinely differ:
/// - the **static** terrain keeps [`RenderAssetUsages::default()`]
///   (`MAIN_WORLD | RENDER_WORLD`) — `lunco-environment`'s horizon bake reads its
///   `ATTRIBUTE_POSITION` back on the CPU and rewrites its UVs;
/// - the **streamed LOD tiles** pass [`RenderAssetUsages::RENDER_WORLD`]: nothing
///   reads their CPU vertex data back (physics rides the collider ring, picking
///   rides the oracle), so keeping ~160 KB × up-to-1024 cached tiles resident in
///   `Assets<Mesh>` after GPU upload was pure waste — ~164 MB, doubled against
///   VRAM, on a wasm heap with a 2–4 GB ceiling.
pub fn grid_mesh(
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    uvs: Vec<[f32; 2]>,
    indices: Vec<u32>,
    usages: RenderAssetUsages,
) -> Mesh {
    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, usages);
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_indices(Indices::U32(indices));
    mesh
}

/// Convenience: a denser/larger preset for stress-testing rover mobility.
pub fn stress_spec(seed: u64) -> ObstacleFieldSpec {
    ObstacleFieldSpec {
        seed,
        region_half_extent: 150.0,
        pattern: Pattern::Clustered {
            clusters: 8,
            spread: 25.0,
        },
        ..default()
    }
}
