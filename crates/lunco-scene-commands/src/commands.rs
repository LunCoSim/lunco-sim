//! Command handlers for sandbox-edit world manipulation.
//!
//! - `SpawnEntity` — spawn from the catalog at a world position.
//! - `MoveEntity` — teleport an entity to an absolute world position.
//!   Mirrors what the gizmo does on drag: swap to Kinematic, update
//!   Transform/Position/LinearVelocity, so joint constraints
//!   propagate the move to coupled bodies. Lets API clients
//!   (MCP tools, automated tests) drive entity motion exactly the
//!   way a human would with the gizmo.

use bevy::prelude::*;
use bevy::math::DVec3;
use avian3d::prelude::{LinearVelocity, RigidBody};
use avian3d::physics_transform::Position;
use big_space::prelude::Grid;
use lunco_core::{on_command, register_commands, Command};
use lunco_obstacle_field::ObstacleFieldRoot;
// Appearance INTENT (render-free). `SetObjectProperty`'s PBR keys mutate `PbrLook`
// and its shader keys mutate `ShaderLook`; the render binders re-materialise on
// `Changed<PbrLook>` / `Changed<ShaderLook>`. This file names no material type —
// see `docs/architecture/render-decoupling.md`.
use lunco_materials::{ParamSchema, ParamType, ParamValue, ShaderLook};
use lunco_render::{PbrLook, SurfaceAlpha};
use lunco_usd::commands::ApplyUsdOp;
use lunco_usd::document::{UsdOp, LayerId};
use lunco_usd::registry::UsdDocumentRegistry;
use lunco_usd_bevy::{UsdPrimPath, UsdStageAsset};
use lunco_doc::{DocumentId, DocumentOrigin};
use lunco_doc_bevy::{RedoDocument, UndoDocument};
use crate::catalog::{SpawnCatalog, SpawnSource, spawn_usd_entry};

/// Spawn an entity from the catalog at a given world position.
///
/// The TYPE lives in `lunco_core::commands` (review A6): the networking crate
/// declares this command's wire channel and needs nothing but the type, so keeping
/// the definition in core is what let `lunco-networking` drop its dependency on
/// this crate. The HANDLER (`on_spawn_entity_command`) stays here, with the
/// catalog it spawns from. Re-exported so existing call sites are unchanged.
pub use lunco_core::SpawnEntity;

/// Detach a joint by despawning it.
#[Command(reflect_default)]
pub struct DetachJoint {
    /// The joint entity to despawn.
    pub target: Entity,
    /// Persistent (default) authors the joint's removal into the scene's runtime
    /// layer — so it journals, syncs, and survives reload — before despawning.
    /// Interactive just pops the live joint (a throwaway test), no journal. See
    /// [`lunco_core::EditIntent`]. Omitted by API callers → `Persistent`.
    #[serde(default)]
    pub intent: lunco_core::EditIntent,
}

impl Default for DetachJoint {
    fn default() -> Self {
        Self {
            target: Entity::PLACEHOLDER,
            intent: lunco_core::EditIntent::Persistent,
        }
    }
}


/// Force a re-scan of project USD files into the spawn catalog. Picks up
/// `*.usda` dropped into an already-open Twin mid-session (twin-open is
/// auto-scanned; this covers new files after that). Idempotent.
#[Command(default)]
pub struct RescanSpawnCatalog {}

/// Observer for [`RescanSpawnCatalog`].
#[on_command(RescanSpawnCatalog)]
pub fn on_rescan_spawn_catalog(
    _trigger: On<RescanSpawnCatalog>,
    twin_roots: Option<Res<lunco_assets::twin_source::TwinRoots>>,
    mut catalog: ResMut<crate::catalog::SpawnCatalog>,
) {
    if let Some(roots) = twin_roots.as_deref() {
        let n = crate::catalog::scan_usd_into_catalog(roots, &mut catalog);
        info!("RESCAN_SPAWN_CATALOG: +{n} USD asset(s)");
    }
}

/// Observer that handles DetachJoint commands — despawns the live joint entity in
/// BOTH modes (the visible effect). Persistence is a decoupled observer below.
#[on_command(DetachJoint)]
pub fn on_detach_joint(
    trigger: On<DetachJoint>,
    mut commands: Commands,
) {
    let cmd = trigger.event();
    if let Ok(mut entity) = commands.get_entity(cmd.target) {
        entity.try_despawn();
        info!("DETACH_JOINT: despawned joint entity {:?} ({:?})", cmd.target, cmd.intent);
    }
}

// ── Dock release, as an actuator on the normal intent→port machinery ─────────

/// Dock/release actuator. A vessel exposes a `release` command PORT; when it rises
/// past 0.5 the fixed joint attaching this vessel to another body is detached, once.
/// Driven exactly like throttle/steer: `Release` intent (KeyG) → the `_LanderControl`
/// profile's `release`→`release` binding → `SetPorts` → this port. Replaces the old
/// hardcoded G-to-detach special case; it works for any possessed vessel + dock joint.
#[derive(bevy::prelude::Component, Default, bevy::prelude::Reflect)]
#[reflect(Component)]
pub struct ReleaseActuator {
    /// Commanded release 0..1, written by the control binding.
    pub cmd: f32,
    /// Edge latch so a held key detaches only once.
    latched: bool,
}

/// Port backend exposing `release` on any entity carrying a [`ReleaseActuator`].
const RELEASE_BACKEND: lunco_core::ports::PortBackend = lunco_core::ports::PortBackend {
    list: |w, e, out| {
        if let Some(a) = w.get::<ReleaseActuator>(e) {
            out.push(lunco_core::ports::PortRef {
                name: "release".to_string(),
                direction: lunco_core::ports::PortDirection::InOut,
                value: a.cmd as f64,
            });
        }
    },
    read_output: |w, e, n| {
        if n != "release" { return None; }
        w.get::<ReleaseActuator>(e).map(|a| a.cmd as f64)
    },
    read_input: |w, e, n| {
        if n != "release" { return None; }
        w.get::<ReleaseActuator>(e).map(|a| a.cmd as f64)
    },
    write_input: |w, e, n, v| {
        if n != "release" { return false; }
        if let Some(mut a) = w.get_mut::<ReleaseActuator>(e) {
            a.cmd = v as f32;
            return true;
        }
        false
    },
    resolve_output: None,
    resolve_input: None,
    read_slot: None,
    write_slot: None,
};

/// Register [`RELEASE_BACKEND`] once the `PortRegistry` exists (after the cosim
/// builtins). `Option` so an app without cosim doesn't panic.
fn register_release_backend(reg: Option<ResMut<lunco_core::ports::PortRegistry>>) {
    if let Some(mut reg) = reg {
        reg.register(RELEASE_BACKEND);
    }
}

/// Give the CONTROL entity of every control-bound vessel a [`ReleaseActuator`], so
/// its `release` port is where the control binding actually writes. (A USD prim can
/// spawn several entities sharing one `UsdPrimPath` — the control/model entity vs the
/// physics-body entity a joint references; the binding targets the former, so the
/// actuator must live there. `joint_release_system` bridges to the joint by path.)
fn attach_release_actuator(
    mut commands: Commands,
    q: Query<Entity, (Added<lunco_core::ControlBinding>, Without<ReleaseActuator>)>,
) {
    for e in &q {
        // `try_insert`: scene-load churn (or a doc-backed reload) can despawn a
        // just-added ControlBinding entity before this deferred insert applies —
        // a plain `insert` then panics on the invalid entity. Same despawn-safe
        // idiom as gizmo/hardware/terrain-surface.
        commands.entity(e).try_insert(ReleaseActuator::default());
    }
}

/// Edge-detect the `release` command → detach the fixed joint attaching this vessel
/// to another body. The principled generalization of the old G-to-detach: any
/// possessed vessel, any dock joint, no per-scene name matching.
fn joint_release_system(
    mut vessels: Query<(&mut ReleaseActuator, &UsdPrimPath)>,
    joints: Query<(Entity, &avian3d::prelude::FixedJoint)>,
    body_paths: Query<&UsdPrimPath>,
    mut commands: Commands,
) {
    for (mut act, vpath) in &mut vessels {
        if act.cmd > 0.5 {
            if !act.latched {
                act.latched = true;
                // Bridge control-entity → physics-body by shared USD path: detach any
                // fixed joint whose bodies resolve to this vessel's prim path.
                for (je, j) in &joints {
                    let hit = [j.body1, j.body2].into_iter().any(|b| {
                        body_paths.get(b).is_ok_and(|p| p.path == vpath.path)
                    });
                    if hit {
                        info!("RELEASE: vessel {} detaching joint {je:?}", vpath.path);
                        // Runtime undock (a live physics action, not an authored scene
                        // edit) → Interactive so it doesn't journal.
                        commands.trigger(DetachJoint {
                            target: je,
                            intent: lunco_core::EditIntent::Interactive,
                        });
                    }
                }
            }
        } else {
            act.latched = false;
        }
    }
}

/// Persist a **`Persistent`** `DetachJoint` into the active USD document's runtime
/// overlay by authoring a `RemovePrim` — so the detachment journals, syncs, and
/// survives reload. Decoupled from [`on_detach_joint`] (which does the live
/// despawn), mirroring [`persist_move_to_runtime_layer`]: same active-doc +
/// ownership guard, same `LayerId::runtime()` target. `Interactive` detaches are
/// throwaway (no journal), so this early-returns for them.
pub fn persist_detach_to_runtime_layer(
    trigger: On<DetachJoint>,
    usd_registry: Res<UsdDocumentRegistry>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    q_prim: Query<&UsdPrimPath>,
    mut commands: Commands,
) {
    let cmd = trigger.event();
    if !cmd.intent.is_persistent() {
        return;
    }
    let Some((doc, path)) =
        authorable_prim(cmd.target, &q_prim, &usd_registry, workspace.as_deref())
    else {
        return;
    };

    commands.trigger(ApplyUsdOp {
        doc,
        op: UsdOp::RemovePrim {
            edit_target: LayerId::runtime(),
            path,
        },
    });
}

/// Observer that handles SpawnEntity commands.
#[on_command(SpawnEntity)]
pub fn on_spawn_entity_command(
    trigger: On<SpawnEntity>,
    mut commands: Commands,
    catalog: Res<SpawnCatalog>,
    asset_server: Res<AssetServer>,
    q_grids: Query<Entity, With<Grid>>,
    role: Res<lunco_core::NetworkRole>,
    dem: Query<(&GlobalTransform, &lunco_terrain_surface::stream_viz::DemHeightField)>,
) {
    let cmd = trigger.event();

    // On a pure client, spawning is the host's job: the command is captured and
    // sent to the host, which spawns the authoritative rover and replicates it
    // back (arriving via `apply_replicated_spawns`). Don't spawn locally, or the
    // client would get a duplicate with no server identity.
    if matches!(*role, lunco_core::NetworkRole::Client) {
        return;
    }

    let entry = match catalog.get(&cmd.entry_id) {
        Some(e) => e,
        None => {
            warn!("SPAWN_ENTITY: unknown entry '{}'", cmd.entry_id);
            return;
        }
    };

    // Prefer the requested grid; fall back to the first grid (a wire-applied
    // spawn may carry a grid id that doesn't resolve on this peer).
    let grid = match q_grids.get(cmd.target).ok().or_else(|| q_grids.iter().next()) {
        Some(g) => g,
        None => {
            warn!("SPAWN_ENTITY: no grid to spawn under");
            return;
        }
    };

    // Terrain-fit the drop height: snap to the DEM surface (+ the asset's spawn
    // lift) when streamed terrain covers this (x,z), so an API / headless / rhai
    // spawn lands ON the surface instead of free-falling when the collider under
    // the drop point hasn't baked yet. No DEM here (a flat scene, or an intentional
    // altitude) → the position is used exactly as given. The GUI palette path does
    // its own richer footprint fit before triggering, so its position arrives fitted.
    let mut position = cmd.position;
    if let Some(y) = lunco_terrain_surface::stream_viz::dem_ground_height(
        dem.iter(),
        position.x as f64,
        position.z as f64,
    ) {
        position.y = y as f32 + entry.spawn_lift;
    }

    info!("SPAWN_ENTITY: {} at {:?}", cmd.entry_id, position);

    let rotation = cmd.rotation.unwrap_or(Quat::IDENTITY);
    let result = spawn_usd_entry(&mut commands, &asset_server, entry, position, rotation, grid);

    // Networked identity (gap G2): a runtime instance gets a server-allocated
    // unique id (SkipContentStamp → assign_global_entity_ids mints
    // Authoritative, never colliding `Content`), is marked for transform
    // replication, and records what to replicate so the host can broadcast the
    // spawn to clients.
    commands.entity(result.root_entity).try_insert((
        lunco_core::SkipContentStamp,
        lunco_core::NetReplicate,
        lunco_core::NetSpawn {
            entry_id: cmd.entry_id.clone(),
            position,
        },
    ));
}

/// Client: instantiate rovers the host has replicated to us (M1 content
/// reconstruction — geometry loads locally, pinned to the host-allocated id).
/// No-op on host/standalone (queue stays empty).
pub fn apply_replicated_spawns(
    mut pending: ResMut<lunco_core::PendingReplicatedSpawns>,
    mut commands: Commands,
    catalog: Res<SpawnCatalog>,
    asset_server: Res<AssetServer>,
    q_grids: Query<Entity, With<Grid>>,
) {
    if pending.0.is_empty() {
        return;
    }
    // Wait until a grid exists (scene still loading) — keep the queue.
    let Some(grid) = q_grids.iter().next() else {
        return;
    };
    // Drain in place — the loop body touches only `commands`/`catalog`/
    // `asset_server`, never `pending`, so the old `.collect::<Vec<_>>()`
    // was a pure-waste allocation (CQ-216).
    for job in pending.0.drain(..) {
        let Some(entry) = catalog.get(&job.entry_id) else {
            warn!("REPL_SPAWN: unknown entry '{}'", job.entry_id);
            continue;
        };
        let pos = job.position;
        let result = spawn_usd_entry(&mut commands, &asset_server, entry, pos, Quat::IDENTITY, grid);
        // Pin the host id; mark runtime instance + replication target. Forced
        // Kinematic by `force_kinematic_proxies` so snapshots drive it.
        commands.entity(result.root_entity).try_insert((
            lunco_core::GlobalEntityId::from_raw(job.gid),
            lunco_core::SkipContentStamp,
            lunco_core::NetReplicate,
        ));
    }
}

/// Move an existing entity to an absolute world-space position.
///
/// Programmatic equivalent of grabbing the entity with the gizmo and
/// dragging it. The handler:
/// 1. Switches the body to `RigidBody::Kinematic` (if it has a
///    `RigidBody`) so Avian treats the new pose as authoritative
///    rather than fighting back via integration.
/// 2. Writes `Transform.translation` for renderer + scene-graph.
/// 3. Writes Avian's `Position` for the joint/contact solver.
/// 4. Sets a one-tick `LinearVelocity` consistent with the move so
///    any joint coupled to a dynamic body propagates the motion.
///
/// Designed for automated tests / MCP tool clients that need to
/// drive the world without a mouse. Single-shot — body type stays
/// Kinematic until another command (or a gizmo drag-end) restores it.
#[Command(default)]
pub struct MoveEntity {
    /// API-stable global entity ID (the `api_id` from `ListEntities`),
    /// resolved to a Bevy `Entity` in the observer via `ApiEntityRegistry`.
    ///
    /// Deliberately `u64`, not `Entity` — this is "**Pattern B**". The
    /// type-driven id codec (`crates/lunco-networking/PH2_ID_CODEC.md`)
    /// auto-converts only `Entity`-typed fields, so a `u64` field opts out and
    /// is resolved here instead. NOT migrated to `Entity` because this command
    /// is `#[Command(default)]`, which derives `Default`, and `Entity` has no
    /// `Default`. Leaving it `u64` is a cleanliness leftover, not a
    /// names/correctness issue — the codec no longer keys off field names at
    /// all, so this `u64` is simply ignored by it. (An earlier comment here
    /// blamed the resolver "dropping the generation"; that was stale — the
    /// codec preserves index+generation via `Entity::to_bits()`.)
    pub entity_id: u64,
    /// Target world-space translation.
    pub translation: Vec3,
}

/// Observer for `MoveEntity`.
#[on_command(MoveEntity)]
pub fn on_move_entity_command(
    trigger: On<MoveEntity>,
    time: Res<Time>,
    registry: Res<lunco_api::registry::ApiEntityRegistry>,
    mut commands: Commands,
    mut q: Query<(
        &mut Transform,
        Option<&mut Position>,
        Option<&mut LinearVelocity>,
    )>,
    q_rb: Query<&RigidBody>,
    q_marker: Query<&JustMovedKinematic>,
) {
    let cmd = trigger.event();
    let global_id = lunco_core::GlobalEntityId::from_raw(cmd.entity_id);
    let Some(target) = registry.resolve(&global_id) else {
        warn!("MOVE_ENTITY: no api_id={} in registry", cmd.entity_id);
        return;
    };
    let Ok((mut tf, pos_opt, lin_vel_opt)) = q.get_mut(target) else {
        warn!("MOVE_ENTITY: entity {:?} (api_id={}) has no Transform", target, cmd.entity_id);
        return;
    };

    let prev = tf.translation;
    tf.translation = cmd.translation;

    // Force the body to Kinematic for the duration of the move so
    // Avian treats the new pose as authoritative. RigidBody is an
    // immutable Avian component (no `&mut` access) — `insert`
    // replaces it. The original kind is stashed on the marker and
    // restored by `clear_kinematic_pulse_velocity` one tick later —
    // a move stream (gizmo drag) must keep the FIRST capture, or the
    // second move would capture the Kinematic we just inserted and
    // the body would stay Kinematic forever (the pre-2026-07-11 bug:
    // a moved rover hovered in mid-air permanently).
    let restore = match q_marker.get(target) {
        Ok(marker) => marker.restore,
        Err(_) => q_rb
            .get(target)
            .ok()
            .copied()
            .filter(|rb| !matches!(rb, RigidBody::Kinematic)),
    };
    if q_rb.get(target).is_ok() {
        commands.entity(target).try_insert(RigidBody::Kinematic);
    }

    if let Some(mut pos) = pos_opt {
        pos.0 = DVec3::new(
            cmd.translation.x as f64,
            cmd.translation.y as f64,
            cmd.translation.z as f64,
        );
    }

    // **Joint-propagation pulse**: set `LinearVelocity` to a one-tick
    // velocity equal to (delta / dt). Avian's joint constraint solver
    // operates on velocities — without this, kinematic teleports
    // don't drag joint-coupled dynamic bodies along. Position is
    // still set above so the body lands exactly where requested;
    // the velocity is purely a signal to the solver.
    //
    // The `JustMovedKinematic` marker (below) tells
    // `clear_kinematic_pulse_velocity` to zero the velocity after
    // exactly one physics tick. Without that follow-up, the body
    // would keep drifting at this velocity each tick.
    let dt = time.delta_secs().max(1.0 / 240.0) as f64;
    let delta = cmd.translation - prev;
    if let Some(mut lin_vel) = lin_vel_opt {
        lin_vel.0 = DVec3::new(
            delta.x as f64 / dt,
            delta.y as f64 / dt,
            delta.z as f64 / dt,
        );
    }
    commands.entity(target).try_insert(JustMovedKinematic { restore });

    info!(
        "MOVE_ENTITY: {:?} → ({:.3}, {:.3}, {:.3})",
        cmd.entity_id, cmd.translation.x, cmd.translation.y, cmd.translation.z
    );
}

/// Persist a runtime move into the active USD document's **runtime** layer
/// (Phase C4b producer). Observes `MoveEntity` alongside the physics handler
/// [`on_move_entity_command`] but is fully decoupled from it — it touches no
/// physics state.
///
/// Persistence is **guarded to authored-scene entities**: it fires only when the
/// moved entity carries a [`UsdPrimPath`] whose prim is owned by the active USD
/// document (present in its base or runtime layer). Palette/sim spawns that
/// aren't part of the authored scene are skipped, so this never authors stray
/// opinions for entities the document doesn't know about. The op targets the
/// runtime layer, so the move round-trips through the Twin journal and renders
/// via the composed view, while Save stays base-only.
pub fn persist_move_to_runtime_layer(
    trigger: On<MoveEntity>,
    api_registry: Res<lunco_api::registry::ApiEntityRegistry>,
    usd_registry: Res<UsdDocumentRegistry>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    q_prim: Query<&UsdPrimPath>,
    mut commands: Commands,
) {
    let cmd = trigger.event();
    let global_id = lunco_core::GlobalEntityId::from_raw(cmd.entity_id);
    let Some(target) = api_registry.resolve(&global_id) else { return };
    let Some((doc, path)) = authorable_prim(target, &q_prim, &usd_registry, workspace.as_deref())
    else {
        return;
    };

    let v = cmd.translation;
    commands.trigger(ApplyUsdOp {
        doc,
        op: UsdOp::SetTranslate {
            edit_target: LayerId::runtime(),
            path,
            value: [v.x as f64, v.y as f64, v.z as f64],
        },
    });
}

// ─────────────────────────────────────────────────────────────────────
// Document history — THE history
//
// The 3D editor has no private undo stack. Every editor mutation is
// authored as a `UsdOp` (the persisters above), so its history is the
// document's history: Lamport-ordered, op+inverse, journaled, networked.
// `UndoDocument`/`RedoDocument` are the generic verbs; each domain observes them
// and acts only on documents its own registry owns. USD's observers live in
// `lunco-usd` (the crate that owns `UsdDocumentRegistry`) — NOT here, so that a
// headless binary with documents but no 3D editor can still undo. The editor's
// only job is to bind the key.
// ─────────────────────────────────────────────────────────────────────

/// Ctrl+Z → undo, Ctrl+Shift+Z / Ctrl+Y → redo, on the **active document**.
///
/// The editor's edits are document ops, so this is the same history the Inspector, the
/// journal and every networked peer see — there is no second, in-memory editor stack to
/// disagree with it.
///
/// Ignored while egui holds the keyboard, so Ctrl+Z in a text field (the rhai editor, a
/// name box) edits the text instead of silently reverting the scene.
pub fn handle_undo_input(
    keys: Res<ButtonInput<KeyCode>>,
    egui_focus: Res<lunco_core::EguiFocus>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    mut commands: Commands,
) {
    if egui_focus.wants_keyboard {
        return;
    }
    if !keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]) {
        return;
    }
    let shift = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    let redo = keys.just_pressed(KeyCode::KeyY) || (shift && keys.just_pressed(KeyCode::KeyZ));
    let undo = !shift && keys.just_pressed(KeyCode::KeyZ);
    if !undo && !redo {
        return;
    }

    let Some(workspace) = workspace else { return };
    let Some(doc) = workspace.0.active_document else {
        info!("[undo] no active document — nothing to undo");
        return;
    };
    if redo {
        commands.trigger(RedoDocument { doc });
    } else {
        commands.trigger(UndoDocument { doc });
    }
}

/// The preamble EVERY persister repeats: resolve the active USD document, resolve the
/// entity's prim path, and ownership-guard it against that document.
///
/// Factored out because the duplication was load-bearing: each `persist_*` observer
/// re-derived this by hand, and the ones that forgot to (the transform gizmo, the
/// Inspector's delete) simply mutated the ECS and never reached the document — which
/// is exactly why a gizmo drag used to be invisible to save, undo, the journal and the
/// network. If an edit path can call this, it has no excuse not to author.
///
/// Returns `None` when there is no active USD document (headless, a Modelica doc, no
/// scene), when the entity is not USD-backed, or when its prim belongs to some other
/// document.
pub fn authorable_prim(
    entity: Entity,
    q_prim: &Query<&UsdPrimPath>,
    usd_registry: &UsdDocumentRegistry,
    workspace: Option<&lunco_workspace::WorkspaceResource>,
) -> Option<(lunco_doc::DocumentId, String)> {
    let doc = workspace?.0.active_document?;
    let host = usd_registry.host(doc)?;
    let prim = q_prim.get(entity).ok()?;
    let prim_sdf = lunco_usd_bevy::SdfPath::new(&prim.path).ok()?;
    let owned = host.document().data().spec(&prim_sdf).is_some()
        || host.document().runtime_data().spec(&prim_sdf).is_some();
    owned.then(|| (doc, prim.path.clone()))
}

// ─────────────────────────────────────────────────────────────────────
// DeleteEntity — removal, authored
// ─────────────────────────────────────────────────────────────────────

/// Delete an entity from the scene.
///
/// The typed verb for "remove this", replacing the ad-hoc `world.despawn(entity)` the
/// Inspector used to do in two places. A bare despawn is invisible to the document:
/// the prim survives in the layer, so the deletion never journals, never replicates,
/// never persists, and the next projection can bring the entity straight back.
///
/// This despawns AND (via [`persist_delete_to_runtime_layer`]) authors a `RemovePrim`
/// — which is what makes deletion undoable, because the document hands back an
/// `AddPrim` inverse for free.
// Plain `#[Command]`, not `#[Command(default)]`: `default` derives `Default`, and
// `Entity` has none — the same reason `DetachJoint` above is plain.
#[Command]
pub struct DeleteEntity {
    /// Entity to remove.
    pub target: Entity,
    /// `Persistent` (the default) authors the removal into the document; an
    /// `Interactive` delete is live-only and does not journal.
    #[serde(default)]
    #[reflect(default)]
    pub intent: lunco_core::EditIntent,
}

/// Live leg: despawn the entity and drop it from the selection.
#[on_command(DeleteEntity)]
pub fn on_delete_entity(
    trigger: On<DeleteEntity>,
    mut selected: ResMut<crate::SelectedEntities>,
    mut commands: Commands,
) {
    let _ = trigger;
    commands.entity(cmd.target).try_despawn();
    selected.entities.retain(|e| *e != cmd.target);
}

/// Authoring leg: remove the prim, so the deletion persists, journals, replicates —
/// and undoes. Same shape as every other `persist_*` observer.
pub fn persist_delete_to_runtime_layer(
    trigger: On<DeleteEntity>,
    usd_registry: Res<UsdDocumentRegistry>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    q_prim: Query<&UsdPrimPath>,
    mut commands: Commands,
) {
    let cmd = trigger.event();
    if !cmd.intent.is_persistent() {
        return;
    }
    let Some((doc, path)) =
        authorable_prim(cmd.target, &q_prim, &usd_registry, workspace.as_deref())
    else {
        return;
    };
    commands.trigger(ApplyUsdOp {
        doc,
        op: UsdOp::RemovePrim {
            edit_target: LayerId::runtime(),
            path,
        },
    });
}

/// Persist a `SetObjectProperty` **shader-param tune** into the active USD
/// document's **runtime overlay** (#4 — non-destructive layer tuning).
///
/// [`on_set_object_property`] mutates the live [`ShaderLook`] for immediate
/// feedback but writes nothing back to USD, so a tweak (e.g. a terrain
/// `weight_albedo`) is lost on reload. This decoupled observer authors the same
/// edit as a `SetAttribute` into `LayerId::runtime()` — the session overlay that
/// composes over the base layer and rides the Twin journal / `.lunco/runtime`
/// sidecar, while **Save stays base-only** (the authored `.usda` is never
/// dirtied). It mirrors [`persist_move_to_runtime_layer`]: same ownership guard,
/// same runtime target, fully decoupled from the live-mutation handler.
///
/// Scope: **scalar** params (covers every layer `weight_*` and roughness knob) on
/// entities carrying a [`ShaderLook`] whose prim the active document owns.
/// Colors/vectors and PBR props stay live-only for now.
///
/// The "is this a shader prim?" guard is a CLASSIFICATION query, so it asks the
/// *intent* (`With<ShaderLook>`), not the bound material: the intent exists headless
/// too, where nothing ever binds a `ShaderMaterial`.
pub fn persist_property_to_runtime_layer(
    trigger: On<SetObjectProperty>,
    api_registry: Res<lunco_api::registry::ApiEntityRegistry>,
    usd_registry: Res<UsdDocumentRegistry>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    q_prim: Query<&UsdPrimPath>,
    q_shader: Query<(), With<ShaderLook>>,
    mut commands: Commands,
) {
    let cmd = trigger.event();
    // Not shader *params*: `shader` swaps the material (no USD reader — the
    // `shaderPath` attribute was deliberately vetoed, so it stays live-only) and
    // `visible` is authored as standard `token visibility` by
    // [`persist_wheel_and_pbr_to_runtime_layer`]. Disjoint, so neither is
    // double-authored.
    if matches!(cmd.property.as_str(), "shader" | "visible") {
        return;
    }
    // Parse the value into a typed USD attribute. A single float persists as
    // `float`; three comma-separated floats persist as a `color3f` vector — the
    // shape shader colours/vectors (`cell_a`, `tint`, …) take. `read_authored_params`
    // reads BOTH back on reload (vec3 first, then scalar), so both round-trip with
    // no loader change. Any other arity (or a non-numeric value) stays live-only.
    let parts: Vec<&str> = cmd.value.split(',').collect();
    let floats: Vec<f32> = parts.iter().filter_map(|s| s.trim().parse::<f32>().ok()).collect();
    if floats.len() != parts.len() {
        return;
    }
    let (type_name, value) = match floats.len() {
        1 => ("float".to_string(), floats[0].to_string()),
        3 => (
            "color3f".to_string(),
            format!("({}, {}, {})", floats[0], floats[1], floats[2]),
        ),
        _ => return,
    };
    let Some(workspace) = workspace else { return };
    let Some(doc) = workspace.0.active_document else { return };
    let Some(host) = usd_registry.host(doc) else { return };

    let global_id = lunco_core::GlobalEntityId::from_raw(cmd.entity_id);
    let Some(target) = api_registry.resolve(&global_id) else { return };
    // Only shader-look prims (the layer-tuning case) — not PBR ones.
    if q_shader.get(target).is_err() {
        return;
    }
    let Ok(prim) = q_prim.get(target) else { return };

    // Ownership guard: only author for prims the active document actually holds
    // (base or runtime), so palette/sim spawns never get stray opinions.
    let Ok(prim_sdf) = lunco_usd_bevy::SdfPath::new(&prim.path) else { return };
    let owned = host.document().data().spec(&prim_sdf).is_some()
        || host.document().runtime_data().spec(&prim_sdf).is_some();
    if !owned {
        return;
    }

    // Author under `primvars:` with the snake_case field name — the same contract
    // `read_authored_params` reads back (which now normalizes camelCase too).
    let name = format!("primvars:{}", lunco_materials::to_snake_case(&cmd.property));
    commands.trigger(ApplyUsdOp {
        doc,
        op: UsdOp::SetAttribute {
            edit_target: LayerId::runtime(),
            path: prim.path.clone(),
            name,
            type_name,
            value,
        },
    });
}

/// One wheel-dynamics parameter — **the** single source of truth for it.
///
/// A wheel param has exactly three facets and they must never drift apart:
/// the names `SetObjectProperty` accepts for it, the live `WheelRaycast` field
/// it sets, and the USD attribute `lunco_usd_sim` reads back onto that field on
/// load. Two hand-synced tables (a `name → setter` match and a separate
/// `name → attr` match) had already drifted — `slip_stiffness` / `friction_mu`
/// were settable but not persistable, so tuning them was silently lost on
/// reload. One row per param makes that structurally impossible: a field cannot
/// exist in one table and not the other, because there is only one table.
pub(crate) struct WheelParam {
    /// Accepted `SetObjectProperty` names — the Rust field name first, USD-style
    /// aliases after (`radius`, `spring_stiffness`, …).
    pub aliases: &'static [&'static str],
    /// Live setter on `WheelRaycast`. Non-capturing closures coerce to `fn`.
    pub set: fn(&mut lunco_mobility::WheelRaycast, f64),
    /// The USD attribute the loader reads back into this field (`float`).
    pub usd_attr: &'static str,
}

/// Every wheel-dynamics parameter `SetObjectProperty` can tune. Each row's
/// `usd_attr` is a name `lunco_usd_sim`'s wheel loader actually reads, so every
/// tune round-trips through the runtime layer on reload.
pub(crate) const WHEEL_PARAMS: &[WheelParam] = &[
    WheelParam {
        aliases: &["drive_torque", "drive_torque_max"],
        set: |w, v| w.drive_torque_max = v,
        usd_attr: "physxVehicleEngine:peakTorque",
    },
    WheelParam {
        aliases: &["brake_torque", "brake_torque_max"],
        set: |w, v| w.brake_torque_max = v,
        usd_attr: "physxVehicleWheel:maxBrakeTorque",
    },
    WheelParam {
        aliases: &["slip_stiffness"],
        set: |w, v| w.slip_stiffness = v,
        usd_attr: "physxVehicleTire:longitudinalStiffness",
    },
    WheelParam {
        aliases: &["bearing_damping", "damping_rate"],
        set: |w, v| w.bearing_damping = v,
        usd_attr: "physxVehicleWheel:dampingRate",
    },
    WheelParam {
        aliases: &["friction_mu", "friction"],
        set: |w, v| w.friction_mu = v,
        usd_attr: "lunco:frictionCoefficient",
    },
    WheelParam {
        aliases: &["mass"],
        set: |w, v| w.mass = v,
        usd_attr: "physics:mass",
    },
    WheelParam {
        aliases: &["moi", "moment_of_inertia"],
        set: |w, v| w.moment_of_inertia = v,
        usd_attr: "physxVehicleWheel:moi",
    },
    WheelParam {
        aliases: &["wheel_radius", "radius"],
        set: |w, v| w.wheel_radius = v,
        usd_attr: "physxVehicleWheel:radius",
    },
    WheelParam {
        aliases: &["rest_length"],
        set: |w, v| w.rest_length = v,
        usd_attr: "physxVehicleSuspension:restLength",
    },
    WheelParam {
        aliases: &["spring_k", "spring_stiffness"],
        set: |w, v| w.spring_k = v,
        usd_attr: "physxVehicleSuspension:springStiffness",
    },
    WheelParam {
        aliases: &["damping_c", "spring_damping"],
        set: |w, v| w.damping_c = v,
        usd_attr: "physxVehicleSuspension:springDamping",
    },
];

/// Look a `SetObjectProperty` property name up in [`WHEEL_PARAMS`], or `None`
/// if it isn't a wheel field. Both the live-mutation path and the USD-authoring
/// path go through this one lookup.
pub(crate) fn wheel_param(name: &str) -> Option<&'static WheelParam> {
    WHEEL_PARAMS.iter().find(|p| p.aliases.contains(&name))
}

/// Persist a `SetObjectProperty` **wheel-dynamics**, **visibility** or **PBR
/// base-colour** tune into the active USD document's runtime overlay — the
/// counterpart of [`persist_property_to_runtime_layer`] for the property classes
/// it skips (it guards to shader-material prims). Fully decoupled + disjoint: it
/// authors for wheel-param names (via [`wheel_param`]), `visible` (standard USD
/// `token visibility`) or `base_color` on a PBR prim — all of
/// which the loader already reads back, so they round-trip on reload and ride the
/// Twin journal. Ownership-guarded and no-op without an active USD doc, like
/// every other persister.
pub fn persist_wheel_and_pbr_to_runtime_layer(
    trigger: On<SetObjectProperty>,
    api_registry: Res<lunco_api::registry::ApiEntityRegistry>,
    usd_registry: Res<UsdDocumentRegistry>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    q_prim: Query<&UsdPrimPath>,
    // "Is this a PBR (non-shader) prim?" — the `PbrLook` *intent*, which exists
    // headless as well as in a render build (the bound `StandardMaterial` is the
    // binder's business and this crate may not name it).
    q_std_mat: Query<(), With<PbrLook>>,
    mut commands: Commands,
) {
    let cmd = trigger.event();

    // Route the property to a USD attribute the loader reads back.
    let authored: Option<(String, &str, String)> =
        if let Some(param) = wheel_param(&cmd.property) {
            // Wheel dynamics → the single `WHEEL_PARAMS` row's USD attribute.
            cmd.value
                .trim()
                .parse::<f32>()
                .ok()
                .map(|v| (param.usd_attr.to_string(), "float", v.to_string()))
        } else if cmd.property == "visible" {
            // Visibility → standard USD `token visibility`, which the prim
            // instantiator already reads back (`inherited` / `invisible`), so a
            // hide survives reload instead of being a live-only ECS `Visibility`
            // write. A `token` literal is QUOTED in USD.
            let hidden = matches!(cmd.value.trim(), "false" | "0" | "hidden");
            let tok = if hidden { "invisible" } else { "inherited" };
            Some(("visibility".to_string(), "token", format!("\"{tok}\"")))
        } else if cmd.property == "base_color" {
            // PBR base colour → `primvars:displayColor` (the loader reads it back
            // into the prim's `PbrLook`). Linear r,g,b (drop any alpha).
            let f: Vec<f32> = cmd
                .value
                .split(',')
                .filter_map(|s| s.trim().parse::<f32>().ok())
                .collect();
            // ARRAY-valued: `UsdGeomGprim` declares `color3f[] primvars:displayColor`
            // with `constant` interpolation — one element for the whole prim. A
            // scalar `color3f` here is a type mismatch every other DCC falls back
            // to grey on.
            (f.len() >= 3).then(|| {
                (
                    "primvars:displayColor".to_string(),
                    "color3f[]",
                    format!("[({}, {}, {})]", f[0], f[1], f[2]),
                )
            })
        } else {
            None
        };
    let Some((name, type_name, value)) = authored else { return };

    // `base_color` only applies to PBR prims; wheel params resolve
    // regardless (the guard is cheap and disjoint from the shader persister).
    let Some(workspace) = workspace else { return };
    let Some(doc) = workspace.0.active_document else { return };
    let Some(host) = usd_registry.host(doc) else { return };
    let global_id = lunco_core::GlobalEntityId::from_raw(cmd.entity_id);
    let Some(target) = api_registry.resolve(&global_id) else { return };
    if cmd.property == "base_color" && q_std_mat.get(target).is_err() {
        return;
    }
    let Ok(prim) = q_prim.get(target) else { return };

    let Ok(prim_sdf) = lunco_usd_bevy::SdfPath::new(&prim.path) else { return };
    let owned = host.document().data().spec(&prim_sdf).is_some()
        || host.document().runtime_data().spec(&prim_sdf).is_some();
    if !owned {
        return;
    }

    commands.trigger(ApplyUsdOp {
        doc,
        op: UsdOp::SetAttribute {
            edit_target: LayerId::runtime(),
            path: prim.path.clone(),
            name,
            type_name: type_name.to_string(),
            value,
        },
    });
}

/// Persist a `SetEnvironmentLight` sun tweak into the active USD document's
/// runtime overlay — the environment twin of [`persist_property_to_runtime_layer`].
///
/// [`lunco_environment::on_set_environment_light`] mutates the live
/// `DirectionalLight` for immediate feedback but writes nothing back to USD, so a
/// sun tweak is lost on reload. This decoupled observer authors the changed
/// fields as `SetAttribute`s onto the sun's `DistantLight` prim in
/// `LayerId::runtime()`, using the SAME attribute names the loader
/// (`lunco_usd_bevy::light`) already reads back — so illuminance / colour /
/// shadow-range knobs round-trip on reload and ride the Twin journal like every
/// other USD edit. (Live peer-sync then follows the USD projection, exactly as
/// the move / property persisters do — no bespoke light broadcast.)
///
/// Scope: the fields with an existing loader reader. Sun **direction** (needs a
/// rotation-authoring op — there is no `SetRotate` yet) and the render-only knobs
/// (exposure / bloom / earthshine / ambient — no `DistantLight` attribute reads
/// them back yet) stay live-only for now.
///
/// Targets every non-earthshine `DistantLight` the active document owns
/// (`SetEnvironmentLight` itself is global). Ownership-guarded like the other
/// persisters; no-op when no USD doc is active (headless).
pub fn persist_environment_light_to_runtime_layer(
    trigger: On<lunco_environment::SetEnvironmentLight>,
    usd_registry: Res<UsdDocumentRegistry>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    q_sun: Query<
        (&UsdPrimPath, &Transform),
        (
            With<lunco_usd_bevy::UsdAuthoredLight>,
            With<DirectionalLight>,
            Without<lunco_environment::Earthshine>,
        ),
    >,
    mut commands: Commands,
) {
    let cmd = trigger.event();
    let Some(workspace) = workspace else { return };
    let Some(doc) = workspace.0.active_document else { return };
    let Some(host) = usd_registry.host(doc) else { return };

    // Collect only the fields that HAVE a matching loader reader, so every attr
    // authored here round-trips on reload (name, USD type, USD-literal value).
    let mut attrs: Vec<(&str, &str, String)> = Vec::new();
    if let Some(lux) = cmd.illuminance {
        attrs.push(("inputs:intensity", "float", lux.to_string()));
    }
    if let Some([r, g, b]) = cmd.sun_color {
        attrs.push(("inputs:color", "color3f", format!("({r}, {g}, {b})")));
    }
    if let Some(v) = cmd.shadow_max_distance {
        attrs.push(("lunco:shadow:maxDistance", "float", v.to_string()));
    }
    if let Some(v) = cmd.shadow_first_cascade_bound {
        attrs.push(("lunco:shadow:firstCascadeFarBound", "float", v.to_string()));
    }
    if let Some(v) = cmd.shadow_depth_bias {
        attrs.push(("lunco:shadow:depthBias", "float", v.to_string()));
    }
    if let Some(v) = cmd.shadow_normal_bias {
        attrs.push(("lunco:shadow:normalBias", "float", v.to_string()));
    }
    // Direction changes when yaw or pitch is specified.
    let direction_changed = cmd.sun_yaw.is_some() || cmd.sun_pitch.is_some();
    if attrs.is_empty() && !direction_changed {
        return;
    }

    for (prim, tf) in &q_sun {
        // Ownership guard: only author for suns the active document actually
        // holds (base or runtime), so an engine-fallback sun never gets opinions.
        let Ok(prim_sdf) = lunco_usd_bevy::SdfPath::new(&prim.path) else {
            continue;
        };
        let owned = host.document().data().spec(&prim_sdf).is_some()
            || host.document().runtime_data().spec(&prim_sdf).is_some();
        if !owned {
            continue;
        }
        for (name, type_name, value) in &attrs {
            commands.trigger(ApplyUsdOp {
                doc,
                op: UsdOp::SetAttribute {
                    edit_target: LayerId::runtime(),
                    path: prim.path.clone(),
                    name: (*name).to_string(),
                    type_name: (*type_name).to_string(),
                    value: value.clone(),
                },
            });
        }
        // Sun direction → `xformOp:rotateXYZ` via the new `SetRotate` op. Compute
        // the SAME final orientation the live handler does — YXZ yaw/pitch, the
        // unspecified axis kept from the current transform — then express it as
        // Euler XYZ **degrees** for USD. (Reading `cur` from the transform is
        // order-independent w.r.t. the live handler: a specified axis overrides
        // `cur`; an unspecified one the live handler leaves unchanged, so `cur`
        // is the same value either way.) Uses the runtime-overlay layer, exactly
        // like `persist_move_to_runtime_layer` does for translate.
        if direction_changed {
            let (cur_yaw, cur_pitch, _) = tf.rotation.to_euler(EulerRot::YXZ);
            let yaw = cmd.sun_yaw.unwrap_or(cur_yaw);
            let pitch = cmd.sun_pitch.unwrap_or(cur_pitch);
            let quat = Quat::from_euler(EulerRot::YXZ, yaw, pitch, 0.0);
            let (rx, ry, rz) = quat.to_euler(EulerRot::XYZ);
            commands.trigger(ApplyUsdOp {
                doc,
                op: UsdOp::SetRotate {
                    edit_target: LayerId::runtime(),
                    path: prim.path.clone(),
                    value: [
                        rx.to_degrees() as f64,
                        ry.to_degrees() as f64,
                        rz.to_degrees() as f64,
                    ],
                },
            });
        }
    }

    // Render knobs (exposure / bloom / ambient / earthshine) have no natural
    // light-prim home — they apply to global/camera state — so per the schema
    // decision they persist onto a dedicated `LuncoEnvironment` settings prim
    // (a singleton under the default prim). A projector in `lunco-sandbox` reads
    // them back on stage change and applies them, so the light loader stays pure.
    let mut env_attrs: Vec<(&str, &str, String)> = Vec::new();
    if let Some(v) = cmd.exposure_ev100 {
        env_attrs.push(("lunco:env:exposureEv100", "float", v.to_string()));
    }
    if let Some(v) = cmd.bloom_intensity {
        env_attrs.push(("lunco:env:bloomIntensity", "float", v.to_string()));
    }
    if let Some(v) = cmd.ambient_brightness {
        env_attrs.push(("lunco:env:ambientBrightness", "float", v.to_string()));
    }
    if let Some(v) = cmd.earthshine_illuminance {
        env_attrs.push(("lunco:env:earthshineIntensity", "float", v.to_string()));
    }
    if let Some([r, g, b]) = cmd.earthshine_color {
        env_attrs.push((
            "lunco:env:earthshineColor",
            "color3f",
            format!("({r}, {g}, {b})"),
        ));
    }
    if !env_attrs.is_empty() {
        let parent_path = lunco_usd_bevy::layer_default_prim(host.document().data())
            .map(|p| format!("/{p}"))
            .unwrap_or_else(|| "/".to_string());
        let env_path = if parent_path == "/" {
            "/Environment".to_string()
        } else {
            format!("{parent_path}/Environment")
        };
        // Ensure the settings prim exists, but only author `AddPrim` when it's
        // actually absent (else every render tweak would journal a redundant
        // AddPrim). Idempotent thereafter — SetAttribute overwrites in place.
        let exists = lunco_usd_bevy::SdfPath::new(&env_path)
            .ok()
            .map(|sdf| {
                host.document().data().spec(&sdf).is_some()
                    || host.document().runtime_data().spec(&sdf).is_some()
            })
            .unwrap_or(false);
        if !exists {
            commands.trigger(ApplyUsdOp {
                doc,
                op: UsdOp::AddPrim {
                    edit_target: LayerId::runtime(),
                    parent_path,
                    name: "Environment".to_string(),
                    type_name: Some(lunco_environment::LUNCO_ENVIRONMENT_PRIM_TYPE.to_string()),
                    reference: None,
                },
            });
        }
        for (name, type_name, value) in &env_attrs {
            commands.trigger(ApplyUsdOp {
                doc,
                op: UsdOp::SetAttribute {
                    edit_target: LayerId::runtime(),
                    path: env_path.clone(),
                    name: (*name).to_string(),
                    type_name: (*type_name).to_string(),
                    value: value.clone(),
                },
            });
        }
    }
}

/// Persist a runtime **spawn** into the active USD document's runtime layer
/// (Phase C4b producer). Observes `SpawnEntity` alongside the ECS spawn handler
/// [`on_spawn_entity_command`] but is fully decoupled from it — it touches no
/// world/entity state.
///
/// A spawn is recorded as a runtime prim that **`references` the spawned asset**
/// (`AddPrim{edit_target: runtime, reference}`) plus its drop position
/// (`SetTranslate{edit_target: runtime}`). The reference + transform compose
/// into the document's rendered/serialized view and ride the Twin journal, so
/// the spawn survives in session history and the composed scene — while Save
/// stays base-only (the runtime layer is never written to disk). Persisting is
/// gated to when a USD document is active; palette spawns with no active doc
/// (e.g. a headless server) are skipped.
pub fn persist_spawn_to_runtime_layer(
    trigger: On<SpawnEntity>,
    catalog: Res<SpawnCatalog>,
    usd_registry: Res<UsdDocumentRegistry>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    role: Res<lunco_core::NetworkRole>,
    // Monotonic per-session disambiguator for spawn prim names (the runtime
    // layer isn't persisted, so session scope is enough).
    mut spawn_seq: Local<u32>,
    mut commands: Commands,
) {
    let cmd = trigger.event();
    // Single-instantiation: `on_spawn_entity_command` ALWAYS directly instantiates
    // the spawn as an ECS entity (non-client). If we ALSO author it into the doc's
    // runtime layer here, the twin projection re-instantiates it as a SECOND entity
    // — the "double-instantiation" (two overlapping rovers; id-reuse then clobbers
    // one on doc reload). In a `Standalone` session there is no networked/web client
    // that needs the journal-authored copy, so the direct ECS spawn is the sole,
    // authoritative instance — skip persistence and let it be the only rover. (In a
    // `Host` session the runtime-layer op is still the channel a web client learns
    // the spawn from, so persistence stays; de-duplicating THAT double needs the
    // networking-aware fix and is handled separately.)
    if matches!(*role, lunco_core::NetworkRole::Standalone) {
        return;
    }
    let Some(workspace) = workspace else { return };
    let Some(doc) = workspace.0.active_document else { return };
    let Some(host) = usd_registry.host(doc) else { return };
    let Some(entry) = catalog.get(&cmd.entry_id) else { return };
    // The asset this spawn references (the only `SpawnSource` variant today).
    let SpawnSource::UsdFile(asset_path) = &entry.source;

    // Parent under the document's default prim (scene root) when it has one,
    // else at the stage root. `stage_default_prim` returns the bare prim name.
    let parent_path = lunco_usd_bevy::layer_default_prim(host.document().data())
        .map(|p| format!("/{p}"))
        .unwrap_or_else(|| "/".to_string());

    // Unique, valid USD identifier for the spawn prim.
    *spawn_seq += 1;
    let stem: String = cmd
        .entry_id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    let name = format!("{stem}_{}", *spawn_seq);
    let prim_path = if parent_path == "/" {
        format!("/{name}")
    } else {
        format!("{parent_path}/{name}")
    };

    let v = cmd.position;
    // 1) Author the referenced spawn prim into the runtime layer.
    commands.trigger(ApplyUsdOp {
        doc,
        op: UsdOp::AddPrim {
            edit_target: LayerId::runtime(),
            parent_path,
            name,
            type_name: None,
            reference: Some(asset_path.clone()),
        },
    });
    // 2) Record its drop position (applied after the AddPrim above).
    commands.trigger(ApplyUsdOp {
        doc,
        op: UsdOp::SetTranslate {
            edit_target: LayerId::runtime(),
            path: prim_path,
            value: [v.x as f64, v.y as f64, v.z as f64],
        },
    });
}

/// Marker inserted on a kinematic body that just received a
/// `MoveEntity` (or analogous teleport) with a one-tick velocity
/// pulse. [`clear_kinematic_pulse_velocity`] zeros that velocity
/// the frame after the pulse so the body doesn't drift.
#[derive(Component)]
pub struct JustMovedKinematic {
    /// The body kind to put back after the pulse tick — the Kinematic
    /// forced by `on_move_entity_command` is only "for the duration of
    /// the move". `None` = the body was already Kinematic (or has no
    /// RigidBody): restore nothing.
    pub restore: Option<RigidBody>,
}

/// Zeros the `LinearVelocity` of bodies marked with
/// [`JustMovedKinematic`], **after one physics tick has consumed
/// the velocity** for joint propagation.
///
/// Schedule: `FixedPostUpdate`. Bevy's main schedule order is
/// `RunFixedMainLoop` (FixedUpdate cycle) → `Update`. So when a
/// `MoveEntity` observer fires in Frame N's `Update` and sets
/// LinearVelocity + marker, the velocity must persist through the
/// *next* fixed-tick physics step (Frame N+1 `FixedUpdate`) before
/// being zeroed. Running this in `FixedPostUpdate` (which fires
/// after every `FixedUpdate` step) does exactly that:
///
/// - Frame N `Update`: `MoveEntity` sets velocity + inserts marker.
/// - Frame N+1 `FixedUpdate`: physics runs WITH the velocity;
///   Avian's joint solver sees the kinematic body moving and
///   propagates the motion through joints to coupled dynamic bodies.
/// - Frame N+1 `FixedPostUpdate`: this system runs, zeros velocity,
///   removes marker.
/// - Frame N+2 `FixedUpdate`: physics with velocity = 0; body
///   settled at its new position, no drift.
pub fn clear_kinematic_pulse_velocity(
    mut commands: Commands,
    mut q: Query<(Entity, &mut LinearVelocity, &JustMovedKinematic)>,
) {
    for (e, mut vel, marker) in q.iter_mut() {
        vel.0 = DVec3::ZERO;
        // Put the pre-move body kind back ("for the duration of the move").
        // Re-inserting RigidBody goes through avian's replace hook, which
        // wakes the island — a body released in mid-air falls.
        if let Some(kind) = marker.restore {
            commands.entity(e).try_insert(kind);
        }
        commands.entity(e).remove::<JustMovedKinematic>();
    }
}

// ─────────────────────────────────────────────────────────────────────
// SetObjectProperty — ONE general verb to set any property on an object
// ─────────────────────────────────────────────────────────────────────

/// Set a property on a scene object at runtime (live override — not persisted
/// to USD). One general command instead of many narrow ones; new properties
/// just add a `match` arm. Drive it from curl after a screenshot to iterate:
///
/// ```jsonc
/// {"command":"SetObjectProperty",
///  "params":{"entity_id":42,"property":"shader","value":"shaders/balloon.wgsl"}}
/// {"command":"SetObjectProperty",
///  "params":{"entity_id":42,"property":"wedge_count","value":"12"}}
/// {"command":"SetObjectProperty",
///  "params":{"entity_id":42,"property":"cell_a","value":"0.1,0.8,0.2"}}
/// ```
///
/// Recognised `property` values:
/// - `shader` → author a [`ShaderLook`] for that `.wgsl` (asset path); the render
///   binder turns it into a material.
/// - any parameter named by the shader's `Material` struct (e.g. `albedo`,
///   `wedge_count`, `cell_a`) → set that named value on the entity's `ShaderLook`
///   (requires `shader` set first, or a USD shader material). The shader's
///   reflected schema resolves the type; colours are `r,g,b`.
/// - `visible` → `true`/`false` toggles `Visibility`.
/// - Per-wheel tire-spin dynamics (target a single wheel entity by its `api_id`):
///   `drive_torque`, `brake_torque`, `slip_stiffness`, `bearing_damping`,
///   `friction_mu`, `mass`, `moi`, `wheel_radius`, `rest_length`, `spring_k`,
///   `damping_c` → set that `f64` field on the wheel's `WheelRaycast` live.
///   Each wheel is its own entity, so this gives independent per-wheel control.
#[Command(default)]
pub struct SetObjectProperty {
    /// API-stable global entity ID (the `api_id` from `ListEntities`), same
    /// resolution path as [`MoveEntity`] — `u64` "Pattern B", resolved in the
    /// observer; see [`MoveEntity`]'s `entity_id` for why it stays `u64`.
    pub entity_id: u64,
    /// Property name (see struct docs).
    pub property: String,
    /// Value; comma-separated `r,g,b` for colors, a single float for params,
    /// an asset path for `shader`, `true`/`false` for `visible`.
    pub value: String,
}

/// The `SetObjectProperty` PBR keys [`PbrLook`] can express.
///
/// These go through the **appearance-intent component**, not a material asset:
/// mutating `PbrLook` is enough, because `lunco-render-bevy`'s `Changed<PbrLook>`
/// binder re-materialises the entity. There is no longer an `Assets<StandardMaterial>`
/// fallback — an in-place asset write would have been actively wrong anyway (the
/// binder's handles are *shared by look*, so it would bleed onto every other entity
/// that looks the same), and naming the material would drag `bevy_pbr` (wgpu, naga)
/// into the headless server that links this file.
const PBR_LOOK_KEYS: &[&str] = &[
    "base_color",
    "emissive",
    "metallic",
    "roughness",
    "perceptual_roughness",
    "ior",
    "alpha",
    "opacity",
    "unlit",
    "double_sided",
];

/// Apply one PBR property addressed by `SetObjectProperty` to a [`PbrLook`] —
/// appearance **intent**, no material asset touched.
///
/// Value formats: colors are comma-separated **linear** `r,g,b[,a]` in 0..1 (so they
/// round-trip the Inspector's `color_edit_button_rgb`); scalars a single float;
/// booleans `true`/`1`/`yes`/`on`. Only the keys in [`PBR_LOOK_KEYS`] are understood;
/// anything else returns `false`.
/// Author a `PbrLook` edit into the USD document, so a material change persists,
/// journals, undoes and replicates like every other edit.
///
/// The look's USD home is a `UsdPreviewSurface` Shader reached through the geom's
/// `material:binding`. If the prim has no material yet, one is created
/// (`ensure_preview_surface_ops` — Looks scope + Material + Shader + binding) and
/// EVERY input is seeded from the current look, not just the edited one: a
/// freshly-created material must reproduce what is on screen, rather than snapping
/// the untouched channels to `UsdPreviewSurface`'s defaults.
///
/// `double_sided` is deliberately NOT a shader input — it is `uniform bool
/// doubleSided` on `UsdGeomGprim`, a property of the geometry — so it is authored
/// on the geom prim instead. `unlit` is render-only intent with no USD equivalent
/// (see [`lunco_usd::material::preview_surface_input`]) — it is the one knob a saved
/// scene will not carry, deliberately.
fn author_look_to_usd(commands: &mut Commands, target: Entity, key: &str, look: &PbrLook) {
    let look = look.clone();
    let key = key.to_string();
    commands.queue(move |world: &mut World| {
        let Some(doc) = crate::doc_resolve::resolve_doc_for_entity(world, target) else {
            return;
        };
        let Some(prim) = world.get::<UsdPrimPath>(target).cloned() else {
            return;
        };

        // `doubleSided` lives on the geometry, not the surface.
        if key == "double_sided" {
            world.trigger(ApplyUsdOp {
                doc,
                op: UsdOp::SetAttribute {
                    edit_target: LayerId::root(),
                    path: prim.path.clone(),
                    name: "doubleSided".into(),
                    type_name: "bool".into(),
                    value: look.double_sided.to_string(),
                },
            });
            return;
        }
        if lunco_usd::material::preview_surface_input(&key).is_none() {
            return; // `unlit` — render-only intent, no USD surface input to write.
        }

        // An existing bound shader, else create the material.
        let existing = crate::doc_resolve::bound_shader_prim(world, &prim);
        let (mut ops, shader, fresh) = match existing {
            Some(sp) => (Vec::new(), sp, false),
            None => match lunco_usd::material::ensure_preview_surface_ops(&prim.path) {
                Some((ops, shader)) => (ops, shader, true),
                None => return,
            },
        };

        let mut set = |attr: &str, ty: &str, value: String| {
            ops.push(UsdOp::SetAttribute {
                edit_target: LayerId::root(),
                path: shader.clone(),
                name: attr.into(),
                type_name: ty.into(),
                value,
            });
        };
        let c = |c: LinearRgba| format!("({}, {}, {})", c.red, c.green, c.blue);
        for (k, ty, v) in [
            ("base_color", "color3f", c(look.base_color)),
            ("emissive", "color3f", c(look.emissive)),
            ("metallic", "float", look.metallic.to_string()),
            ("roughness", "float", look.perceptual_roughness.to_string()),
            ("opacity", "float", look.base_color.alpha.to_string()),
            ("ior", "float", look.ior.to_string()),
        ] {
            // A fresh material seeds every input; an existing one writes only what
            // changed (so an unrelated authored input is not clobbered).
            if !fresh && !key_matches(&key, k) {
                continue;
            }
            if let Some((attr, _)) = lunco_usd::material::preview_surface_input(k) {
                set(attr, ty, v);
            }
        }
        for op in ops {
            world.trigger(ApplyUsdOp { doc, op: op.clone() });
        }
    });
}

/// Whether the edited look key names the same `UsdPreviewSurface` input as `slot`
/// (`roughness` and `perceptual_roughness` are one input; so are `alpha`/`opacity`).
fn key_matches(key: &str, slot: &str) -> bool {
    lunco_usd::material::preview_surface_input(key)
        == lunco_usd::material::preview_surface_input(slot)
}

fn apply_pbr_look(look: &mut PbrLook, key: &str, value: &str) -> bool {
    let f: Vec<f32> = value
        .split(',')
        .filter_map(|s| s.trim().parse::<f32>().ok())
        .collect();
    let parse_bool = |v: &str| matches!(v.trim(), "true" | "1" | "yes" | "on");
    match key {
        "base_color" => {
            if f.len() < 3 {
                return false;
            }
            let a = f.get(3).copied().unwrap_or(look.base_color.alpha);
            look.base_color = LinearRgba::new(f[0], f[1], f[2], a);
        }
        "emissive" => {
            if f.len() < 3 {
                return false;
            }
            look.emissive = LinearRgba::new(f[0], f[1], f[2], f.get(3).copied().unwrap_or(1.0));
        }
        "metallic" => {
            let Some(v) = f.first() else { return false };
            look.metallic = v.clamp(0.0, 1.0);
        }
        "roughness" | "perceptual_roughness" => {
            let Some(v) = f.first() else { return false };
            look.perceptual_roughness = v.clamp(0.0, 1.0);
        }
        // Index of refraction — `UsdPreviewSurface`'s `inputs:ior`. The specular knob;
        // Bevy's `reflectance` is derived from it (see `lunco-render-bevy`). 1.0 = no
        // Fresnel at all (vacuum); nothing physical goes below it.
        "ior" => {
            let Some(v) = f.first() else { return false };
            look.ior = v.max(1.0);
        }
        "alpha" | "opacity" => {
            let Some(v) = f.first() else { return false };
            let v = v.clamp(0.0, 1.0);
            look.base_color.alpha = v;
            look.alpha = if v >= 1.0 { SurfaceAlpha::Opaque } else { SurfaceAlpha::Blend };
        }
        "unlit" => look.unlit = parse_bool(value),
        "double_sided" => look.double_sided = parse_bool(value),
        _ => return false,
    }
    true
}

/// The reflected parameter schema of a shader **asset path**.
///
/// Read straight out of the loaded WGSL source (`Material` struct + `//!@`
/// annotations) rather than off a material — the schema is a property of the
/// *asset*, and reading it this way keeps the shader-param paths render-free.
/// `None` while the shader is still loading (or if it declares no `Material`), in
/// which case callers infer the type from the value's arity, exactly as the old
/// material path did with its empty default schema.
fn shader_schema(
    path: &str,
    asset_server: &AssetServer,
    shaders: &Assets<bevy::shader::Shader>,
) -> Option<ParamSchema> {
    let handle = asset_server.load::<bevy::shader::Shader>(path.to_string());
    let src = match &shaders.get(&handle)?.source {
        bevy::shader::Source::Wgsl(s) => s.as_ref().to_string(),
        _ => return None,
    };
    ParamSchema::parse(&src)
}

/// Parse one `SetObjectProperty` value into a typed [`ParamValue`] for `key`.
///
/// Same grammar (and the same type resolution) as the former
/// `lunco_materials::apply_param`: the field's type comes from the shader's
/// reflected schema when it is known, else from the value's arity; a vector field
/// takes `r,g,b` and is stored as a `Vec4` with alpha 1, which is what
/// `ShaderMaterial::set_color` did and what the shader's uniform block expects.
fn shader_param_value(schema: Option<&ParamSchema>, key: &str, value: &str) -> Option<ParamValue> {
    let ty = schema.and_then(|s| s.field(key)).map(|f| f.ty).unwrap_or_else(|| {
        match value.split(',').filter(|s| !s.trim().is_empty()).count() {
            0 | 1 => ParamType::F32,
            2 => ParamType::Vec2,
            3 => ParamType::Vec3,
            _ => ParamType::Vec4,
        }
    });
    match ty {
        ParamType::Vec3 | ParamType::Vec4 => {
            let n: Vec<f32> =
                value.split(',').filter_map(|s| s.trim().parse::<f32>().ok()).collect();
            (n.len() >= 3).then(|| ParamValue::Vec4([n[0], n[1], n[2], 1.0]))
        }
        _ => ParamValue::parse(ty, value),
    }
}

/// Give `target` a [`ShaderLook`] for `shader_path`, carrying over any params it
/// already had (so swapping the `.wgsl` keeps tuned values — what cloning the old
/// `ShaderMaterial` as a template used to do).
///
/// Drops the [`PbrLook`] intent: an entity that carries both draws twice, because
/// each binder materialises its own. See `lunco-render-bevy`'s caller contract.
pub(crate) fn author_shader_look(
    commands: &mut Commands,
    target: Entity,
    existing: Option<&ShaderLook>,
    shader_path: &str,
) {
    let mut look = existing.cloned().unwrap_or_default();
    look.shader = shader_path.to_string();
    commands.entity(target).remove::<PbrLook>().try_insert(look);
    commands.queue(move |world: &mut World| drop_bound_pbr_material(world, target));
}

/// Drop the concrete PBR material a render build already bound to `e`.
///
/// Removing the [`PbrLook`] *intent* stops the binder re-materialising the entity,
/// but the `MeshMaterial3d<StandardMaterial>` it inserted earlier stays put — and a
/// mesh carrying that AND the shader material draws twice. That component is
/// `bevy_pbr`'s and this crate may not name it (render-decoupling rule), so it is
/// resolved out of the type registry instead (`MaterialPlugin` registers it, and it
/// is `#[reflect(Component)]`).
///
/// No-op headless and in tests, where nothing ever bound a material — and a no-op the
/// day `lunco-render-bevy` grows an `On<Remove, PbrLook>` observer that unbinds its
/// own material, which is where this really belongs.
pub fn drop_bound_pbr_material(world: &mut World, e: Entity) {
    let Some(registry) = world.get_resource::<AppTypeRegistry>().cloned() else { return };
    let reflect_component = {
        let reg = registry.read();
        reg.get_with_short_type_path("MeshMaterial3d<StandardMaterial>")
            .and_then(|r| r.data::<bevy::ecs::reflect::ReflectComponent>())
            .cloned()
    };
    let Some(rc) = reflect_component else { return };
    if let Ok(mut entity) = world.get_entity_mut(e) {
        rc.remove(&mut entity);
    }
}

/// Observer for [`SetObjectProperty`].
#[on_command(SetObjectProperty)]
pub fn on_set_object_property(
    trigger: On<SetObjectProperty>,
    registry: Res<lunco_api::registry::ApiEntityRegistry>,
    asset_server: Res<AssetServer>,
    shaders: Res<Assets<bevy::shader::Shader>>,
    mut q_look: Query<&mut PbrLook>,
    mut q_shader_look: Query<&mut ShaderLook>,
    q_mesh: Query<(), With<Mesh3d>>,
    mut q_vis: Query<&mut Visibility>,
    mut q_wheel: Query<&mut lunco_mobility::WheelRaycast>,
    mut commands: Commands,
) {
    let cmd = trigger.event();
    let global_id = lunco_core::GlobalEntityId::from_raw(cmd.entity_id);
    let Some(target) = registry.resolve(&global_id) else {
        warn!("SET_PROPERTY: no api_id={} in registry", cmd.entity_id);
        return;
    };

    // Per-wheel tire-spin dynamics. Each wheel is its own entity, so addressing
    // a single `api_id` sets the field on just that wheel — independent control.
    if let Some(param) = wheel_param(&cmd.property) {
        let Ok(value) = cmd.value.trim().parse::<f64>() else {
            warn!("SET_PROPERTY: '{}' expects a number, got '{}'", cmd.property, cmd.value);
            return;
        };
        let Ok(mut wheel) = q_wheel.get_mut(target) else {
            warn!("SET_PROPERTY: entity {} has no WheelRaycast", cmd.entity_id);
            return;
        };
        (param.set)(&mut wheel, value);
        info!("SET_PROPERTY: wheel {} {} = {}", cmd.entity_id, cmd.property, value);
        return;
    }

    match cmd.property.as_str() {
        "shader" => {
            // Preserve existing uniforms if the object already has a shader look,
            // so swapping the .wgsl keeps tuned params.
            let existing = q_shader_look.get(target).ok().cloned();
            author_shader_look(&mut commands, target, existing.as_ref(), &cmd.value);
            info!("SET_PROPERTY: {} shader = {}", cmd.entity_id, cmd.value);
        }
        "visible" => {
            let Ok(mut vis) = q_vis.get_mut(target) else {
                warn!("SET_PROPERTY: entity {} has no Visibility", cmd.entity_id);
                return;
            };
            let v = cmd.value.trim();
            *vis = if matches!(v, "false" | "0" | "hidden") {
                Visibility::Hidden
            } else {
                Visibility::Visible
            };
        }
        // PBR properties — for props/rovers on a plain surface rather than a custom
        // shader. Explicit arm ([`PBR_LOOK_KEYS`]) so these names never get stolen by
        // the shader-param fallback below.
        //
        // The edit is a mutation of the entity's `PbrLook` *intent* component: the
        // render binder's `Changed<PbrLook>` system re-materialises it, so "edit the
        // material" is just "mutate a component" — no asset handles, and it works
        // headless (the intent is in the world; nothing binds it). A mesh with no
        // intent yet (a glTF import that brought its own material) is ADOPTED into an
        // intent, which is the only render-free way to keep this command working on
        // it; note that adoption starts from `PbrLook::default()`, so the import's own
        // textures are not carried over.
        key if PBR_LOOK_KEYS.contains(&key) => {
            if let Ok(mut look) = q_look.get_mut(target) {
                if apply_pbr_look(&mut look, key, &cmd.value) {
                    // ALSO author it into USD. Mutating `PbrLook` alone updates the
                    // screen and nothing else — the edit would never reach the
                    // document, so it would not save, journal, undo, or replicate.
                    // Every edit goes through `ApplyUsdOp`; this one was quietly
                    // exempt.
                    author_look_to_usd(&mut commands, target, key, &look);
                    info!("SET_PROPERTY: {} look {} = {}", cmd.entity_id, cmd.property, cmd.value);
                } else {
                    warn!("SET_PROPERTY: bad value '{}' for pbr '{}'", cmd.value, cmd.property);
                }
                return;
            }
            if q_mesh.get(target).is_err() {
                warn!("SET_PROPERTY: entity {} has no PbrLook / mesh", cmd.entity_id);
                return;
            }
            let mut look = PbrLook::default();
            if apply_pbr_look(&mut look, key, &cmd.value) {
                commands.entity(target).try_insert(look);
                info!("SET_PROPERTY: {} adopted a PbrLook, {} = {}", cmd.entity_id, cmd.property, cmd.value);
            } else {
                warn!("SET_PROPERTY: bad value '{}' for pbr '{}'", cmd.value, cmd.property);
            }
        }
        key => {
            // param/color → set the named value on the entity's shader look. The
            // binder swaps in the material for the new look (`Changed<ShaderLook>`).
            let Ok(mut look) = q_shader_look.get_mut(target) else {
                warn!("SET_PROPERTY: entity {} has no shader look — set 'shader' first", cmd.entity_id);
                return;
            };
            // USD authors params camelCase, WGSL declares them snake_case.
            let name = lunco_materials::to_snake_case(key);
            let schema = shader_schema(&look.shader, &asset_server, &shaders);
            match shader_param_value(schema.as_ref(), &name, &cmd.value) {
                Some(v) => {
                    look.values.insert(name, v);
                }
                None => warn!("SET_PROPERTY: unknown property '{}'", key),
            }
        }
    }
}

/// Point the free-flight avatar camera at an entity (by API id), from a fixed
/// side-on-and-above angle at `distance` metres. Lets API clients (MCP tools,
/// automated screenshots) frame a subject — e.g. a wheel — without hand-driving
/// the camera. `entity_id` is the API id from `ListEntities` (a `u64`), same as
/// [`MoveEntity`]/[`SetObjectProperty`].
#[Command(default)]
pub struct FocusEntityById {
    /// API id from `ListEntities` — `u64` "Pattern B", resolved in the observer
    /// via `ApiEntityRegistry`; see [`MoveEntity`]'s `entity_id` for why it
    /// stays `u64` and isn't auto-converted by the id codec.
    pub entity_id: u64,
    /// Camera distance from the target, metres. `<= 0` → default 6.
    pub distance: f32,
}

/// A focus request recorded by [`on_focus_entity_by_id`] and applied by
/// [`apply_pending_focus`] at the start of the NEXT frame (`First` schedule).
///
/// The command observer fires wherever the API dispatcher happens to sit in
/// the frame — including BETWEEN transform-propagation passes, where the
/// target's and the avatar's `GlobalTransform`s are momentarily in different
/// conventions (a site-anchored scene re-bases the solar hierarchy every
/// tick). Doing the math there teleported the avatar ~1e11 m into empty space
/// ("click on Earth → everything vanishes"). In `First`, nothing has written a
/// transform yet this frame, so last frame's fully-propagated GTs are
/// mutually consistent by construction.
#[derive(Resource, Debug, Clone, Copy)]
pub struct PendingFocus {
    pub target: Entity,
    pub distance: f32,
}

/// Observer: validate + record the focus; all spatial math happens in
/// [`apply_pending_focus`].
#[on_command(FocusEntityById)]
pub fn on_focus_entity_by_id(
    trigger: On<FocusEntityById>,
    registry: Res<lunco_api::registry::ApiEntityRegistry>,
    mut commands: Commands,
) {
    let cmd = trigger.event();
    let global_id = lunco_core::GlobalEntityId::from_raw(cmd.entity_id);
    let Some(target) = registry.resolve(&global_id) else {
        warn!("FOCUS_ENTITY: no api_id={} in registry", cmd.entity_id);
        return;
    };
    commands.insert_resource(PendingFocus { target, distance: cmd.distance });
    info!("FOCUS_ENTITY: queued focus on {target:?} at {} m", cmd.distance);
}

/// Applies a [`PendingFocus`] with frame-consistent transforms (`First`
/// schedule — see the type doc).
pub fn apply_pending_focus(
    pending: Option<Res<PendingFocus>>,
    q_target: Query<&GlobalTransform>,
    mut q_avatar: Query<
        (
            Entity,
            &mut Transform,
            &mut big_space::prelude::CellCoord,
            &ChildOf,
            &GlobalTransform,
            Option<&mut lunco_avatar::FreeFlightCamera>,
        ),
        With<lunco_core::Avatar>,
    >,
    q_grids: Query<&Grid>,
    q_celestial: Query<(), With<lunco_celestial::CelestialBody>>,
    mut commands: Commands,
    mut orbital_pin: Option<ResMut<lunco_celestial::OrbitalViewPin>>,
) {
    let Some(pending) = pending else { return };
    let (target, distance) = (pending.target, pending.distance);
    // Celestial bodies are ORBIT-scale targets: hand them to the avatar's
    // `FocusTarget` flow (OrbitCamera flies to the body's grid — doc 47
    // Phase 6 — with sunlit-side arrival). Local framing stays for
    // metre-scale subjects (wheels, rovers, props).
    if q_celestial.get(target).is_ok() {
        commands.remove_resource::<PendingFocus>();
        commands.trigger(lunco_avatar::FocusTarget { avatar: None, target });
        info!("FOCUS_ENTITY: celestial target {target:?} → orbit focus");
        return;
    }
    // Local framing from an orbital view: deactivate the mode and let
    // `orbital_exit_restore_system` migrate the camera back to the parked
    // surface pose this frame. `PendingFocus` is deliberately NOT consumed —
    // the framing re-runs next frame from the restored state, where the GT
    // delta math below is surface-convention again (running it now would
    // compute a pose in the CELESTIAL grid the camera is still parented to,
    // which the restore would then clobber).
    if let Some(pin) = orbital_pin.as_mut() {
        if pin.active {
            pin.active = false;
            info!("FOCUS_ENTITY: leaving orbital view first; focus retries next frame");
            return;
        }
    }
    commands.remove_resource::<PendingFocus>();
    let cmd = FocusEntityById { entity_id: 0, distance };
    let Ok(target_gt) = q_target.get(target) else {
        warn!("FOCUS_ENTITY: target {:?} has no GlobalTransform", target);
        return;
    };
    // Tolerate 0/≥1 avatars robustly. `single_mut()` errored when the avatar was
    // momentarily in a non-freeflight camera mode (FreeFlightCamera removed by
    // possess/follow/orbit) OR when more than one Avatar existed (USD avatar +
    // fallback) — both surfaced as "no Avatar" and killed double-click focus.
    // Take the first avatar; the FreeFlightCamera is now optional.
    let avatar_count = q_avatar.iter().count();
    let Some((avatar_ent, mut tf, mut cell, child_of, avatar_gt, ff_opt)) = q_avatar.iter_mut().next() else {
        warn!("FOCUS_ENTITY: no Avatar entity in the scene (count={avatar_count})");
        return;
    };
    // Work in the avatar→target DELTA, not the target's absolute
    // `GlobalTransform`. Both GTs are read in the same instant so whatever
    // convention/origin big_space happens to be mid-way through this frame
    // (site-anchored scenes re-base every tick) cancels in the difference —
    // reading the target GT alone teleported the avatar 1e11 m into empty
    // space when the observer fired between propagation passes. The delta is
    // applied to the avatar's LOCAL translation, which is valid because the
    // avatar's parent grid (WorldGrid) is unrotated wrt render space.
    let delta = target_gt.translation() - avatar_gt.translation();
    let dist = if cmd.distance > 0.1 { cmd.distance } else { 6.0 };
    // Camera sits mostly to the SIDE (+X, the wheel axle direction → we see
    // the spoke face) plus a little up and forward. (Celestial targets never
    // reach here — they take the orbit-focus early return above.)
    let dir = Vec3::new(1.0, 0.4, 0.25).normalize();
    let offset = dir * dist;
    // Grid-frame absolute target = camera's CELL-AWARE position + GT delta.
    // A previous orbit focus leaves the avatar cells away from the scene;
    // `tf.translation` alone is only the cell remainder there. Re-split the
    // final pose through the grid so a local focus also RESETS the cell (for
    // scene-scale positions `translation_to_grid` returns cell (0,0,0) + the
    // plain translation — the historical single-cell convention).
    if let Ok(grid) = q_grids.get(child_of.parent()) {
        let target_abs = grid.grid_position_double(&cell, &tf) + delta.as_dvec3();
        let (new_cell, new_translation) = grid.translation_to_grid(target_abs + offset.as_dvec3());
        *cell = new_cell;
        tf.translation = new_translation;
    } else {
        tf.translation = tf.translation + delta + offset;
    }
    // Aim back along the framing offset (camera → target).
    let d = (-offset).normalize();
    let (yaw, pitch) = ((-d.x).atan2(-d.z), d.y.clamp(-1.0, 1.0).asin());
    match ff_opt {
        // Free-flight rebuilds rotation from yaw/pitch every frame (YXZ euler), so
        // when it's present we must set those rather than the Transform rotation.
        Some(mut ff) => {
            ff.yaw = yaw;
            ff.pitch = pitch;
        }
        // Non-freeflight camera mode (orbit/spring/surface): the framing is
        // AUTHORITATIVE — leaving the old mode attached lets its system fly
        // the camera right back (an OrbitCamera on Earth reclaimed the camera
        // one frame after "focus rover" and the view never returned). Strip
        // the mode and reinstate free flight at the computed aim.
        None => {
            tf.rotation = Quat::from_euler(EulerRot::YXZ, yaw, pitch, 0.0);
            commands
                .entity(avatar_ent)
                .remove::<lunco_avatar::OrbitCamera>()
                .remove::<lunco_avatar::OrbitFrameSample>()
                .remove::<lunco_avatar::SunlitArrival>()
                .remove::<lunco_avatar::SpringArmCamera>()
                .remove::<lunco_avatar::ChaseCamera>()
                .remove::<lunco_avatar::SurfaceCamera>()
                .remove::<lunco_avatar::SurfaceRelativeMode>()
                .remove::<lunco_avatar::FrameBlend>()
                .try_insert(lunco_avatar::FreeFlightCamera { yaw, pitch, damping: None });
        }
    }
    info!(
        "FOCUS_ENTITY: framed api_id={} at {:.1} m (avatars={avatar_count})",
        cmd.entity_id, dist
    );
}

/// Aim the free-flight avatar camera: place it at `eye` and look at `target`
/// (both absolute world-space). The flexible primitive — the client computes the
/// angle (e.g. approach a wheel from its outboard side) and distance.
///
/// Authoritative: whatever camera mode the avatar is in (orbit focus on a
/// planet, spring-arm follow, surface mode), this strips it and reinstates a
/// `FreeFlightCamera` at the requested pose — an API client asking for a
/// specific view must always get it. `eye` is split into cell + local
/// translation through the avatar's parent grid, so it lands in the scene
/// frame even when a previous orbit focus left the camera cells away.
#[Command(default)]
pub struct SetCameraLookAt {
    pub eye: Vec3,
    pub target: Vec3,
}

/// Observer for [`SetCameraLookAt`].
#[on_command(SetCameraLookAt)]
pub fn on_set_camera_look_at(
    trigger: On<SetCameraLookAt>,
    mut q_avatar: Query<
        (
            Entity,
            &mut Transform,
            &mut big_space::prelude::CellCoord,
            &ChildOf,
            Option<&mut lunco_avatar::FreeFlightCamera>,
        ),
        With<lunco_core::Avatar>,
    >,
    q_grids: Query<&Grid>,
    q_world_grid: Query<Entity, With<lunco_core::WorldGrid>>,
    mut commands: Commands,
    mut orbital_pin: Option<ResMut<lunco_celestial::OrbitalViewPin>>,
) {
    let cmd = trigger.event();
    let Some((entity, mut tf, mut cell, child_of, ff_opt)) = q_avatar.iter_mut().next() else {
        warn!("SET_CAMERA: no Avatar entity in the scene");
        return;
    };
    // An explicit camera pose is a SURFACE-frame request: leave any orbital
    // view first so `eye`/`target` mean scene coordinates again.
    let was_orbital = orbital_pin.as_mut().is_some_and(|pin| {
        let a = pin.active;
        if a {
            pin.active = false;
        }
        a
    });
    if was_orbital {
        // The camera flew to the focused body's grid; bring it home in one
        // atomic migration — raw cell/translation writes below would be
        // interpreted in the CELESTIAL grid's frame. Removing the marker
        // keeps `orbital_exit_restore_system` from overriding this pose.
        commands
            .entity(entity)
            .remove::<lunco_avatar::OrbitalViewCamera>();
        if let Some(root) = q_world_grid.iter().next() {
            if let Ok(grid) = q_grids.get(root) {
                let (new_cell, new_translation) = grid.translation_to_grid(cmd.eye.as_dvec3());
                lunco_core::attach::migrate_to_grid(
                    &mut commands,
                    entity,
                    root,
                    new_cell,
                    Transform::from_translation(new_translation).with_rotation(tf.rotation),
                );
            }
        }
    } else if let Ok(grid) = q_grids.get(child_of.parent()) {
        let (new_cell, new_translation) = grid.translation_to_grid(cmd.eye.as_dvec3());
        *cell = new_cell;
        tf.translation = new_translation;
    } else {
        tf.translation = cmd.eye;
    }
    let look = cmd.target - cmd.eye;
    let (yaw, pitch) = if look.length() > 1e-4 {
        let d = look.normalize();
        ((-d.x).atan2(-d.z), d.y.clamp(-1.0, 1.0).asin())
    } else {
        let (y, p, _) = tf.rotation.to_euler(EulerRot::YXZ);
        (y, p)
    };
    if let Some(mut ff) = ff_opt {
        ff.yaw = yaw;
        ff.pitch = pitch;
    } else {
        commands
            .entity(entity)
            .remove::<lunco_avatar::OrbitCamera>()
            .remove::<lunco_avatar::OrbitFrameSample>()
            .remove::<lunco_avatar::SunlitArrival>()
            .remove::<lunco_avatar::SpringArmCamera>()
            .remove::<lunco_avatar::ChaseCamera>()
            .remove::<lunco_avatar::SurfaceCamera>()
            .remove::<lunco_avatar::SurfaceRelativeMode>()
            .remove::<lunco_avatar::FrameBlend>()
            .try_insert(lunco_avatar::FreeFlightCamera { yaw, pitch, damping: None });
    }
    info!(
        "SET_CAMERA: eye=({:.2},{:.2},{:.2}) target=({:.2},{:.2},{:.2})",
        cmd.eye.x, cmd.eye.y, cmd.eye.z, cmd.target.x, cmd.target.y, cmd.target.z
    );
}

/// Force-reload shader assets from disk so live WGSL edits apply without
/// restarting the app. Bypasses the file watcher (unreliable in this build):
/// calls [`AssetServer::reload`], which re-runs the loader and triggers
/// dependent material pipelines to rebuild. Empty `path` → reload the standard
/// `assets/shaders/*` set; otherwise reload just that path (e.g.
/// `"shaders/wheel.wgsl"`).
#[Command(default)]
pub struct ReloadShader {
    pub path: String,
}

/// Observer for [`ReloadShader`].
#[on_command(ReloadShader)]
pub fn on_reload_shader(trigger: On<ReloadShader>, asset_server: Res<AssetServer>) {
    let p = trigger.event().path.trim().to_string();
    let paths: Vec<String> = if p.is_empty() {
        ["shaders/wheel.wgsl", "shaders/balloon.wgsl", "shaders/solar_panel.wgsl"]
            .iter()
            .map(|s| s.to_string())
            .collect()
    } else {
        vec![p]
    };
    for path in paths {
        // Owned `String` → `AssetPath<'static>`, so the queued reload doesn't
        // borrow the (short-lived) trigger.
        asset_server.reload(path.clone());
        info!("RELOAD_SHADER: {}", path);
    }
}

/// Replace a shader asset's WGSL **source in place** from text sent over the
/// API, recompiling it live without touching disk or restarting. Overwrites the
/// `Shader` asset currently at `path` (e.g. `"shaders/wheel.wgsl"`), so every
/// material using it re-specializes its pipeline next frame. Compile/validation
/// outcome surfaces in the render log (naga errors on a bad shader). Pairs with
/// [`ReloadShader`] (disk) — this one is for pushing edits directly.
#[Command(default)]
pub struct SetShaderSource {
    /// Asset path of the shader to overwrite, e.g. `"shaders/wheel.wgsl"`.
    pub path: String,
    /// New WGSL source text.
    pub source: String,
}

/// Observer for [`SetShaderSource`].
#[on_command(SetShaderSource)]
pub fn on_set_shader_source(
    trigger: On<SetShaderSource>,
    asset_server: Res<AssetServer>,
    mut shaders: ResMut<Assets<bevy::shader::Shader>>,
    mut registry: ResMut<crate::shader_doc::ShaderRegistry>,
    guard: Option<Res<lunco_core::session::SyncApplyGuard>>,
) {
    let ev = trigger.event();
    if ev.path.is_empty() || ev.source.is_empty() {
        warn!("SET_SHADER_SOURCE: empty path or source");
        return;
    }
    // Record the edit into the Twin journal (`DomainKind::Shader`) via the shader
    // document registry — so it SYNCS + PERSISTS like a rhai/Modelica edit, not
    // just a local `Assets<Shader>` poke. Skip recording when this arrived from the
    // wire (`SyncApplyGuard` set): the originating peer already journaled it, and
    // the journal replay leg applies + hot-reloads it here — re-recording would
    // duplicate the entry.
    if guard.map_or(true, |g| g.0.is_none()) {
        registry.apply_source(&ev.path, ev.source.clone());
    }
    // Hot-reload: `load` returns the handle every material already holds, so
    // overwriting that asset id propagates the recompile to them.
    let handle = asset_server.load::<bevy::shader::Shader>(ev.path.clone());
    let shader = bevy::shader::Shader::from_wgsl(ev.source.clone(), ev.path.clone());
    let _ = shaders.insert(handle.id(), shader);
    info!(
        "SET_SHADER_SOURCE: recompiled {} from {} bytes of WGSL",
        ev.path,
        ev.source.len()
    );
}

// ─────────────────────────────────────────────────────────────────────────
// Live shader authoring — create from a template, import any `.wgsl` from the
// computer into the open Twin, and discover shaders dropped in the Twin folder.
// All persist into `<twin>/shaders/<name>.wgsl` (fallback `assets/shaders/`),
// register into the picker [`ShaderCatalog`], and can apply to an entity — no
// restart. The created/imported shaders are PBR-compatible self-describing
// shaders (see [`lunco_materials::shader_template`]).
// ─────────────────────────────────────────────────────────────────────────

/// The asset path a shader named `stem` would be installed at: under the
/// primary open Twin (`twin://<name>/shaders/<stem>.wgsl`) or the engine library
/// (`shaders/<stem>.wgsl`) when no Twin is open. Mirrors [`install_shader`]'s
/// destination logic so callers (e.g. the Inspector) can predict the path.
pub fn shader_asset_path_for(
    twin_roots: Option<&lunco_assets::twin_source::TwinRoots>,
    stem: &str,
) -> String {
    match twin_roots.and_then(|t| t.primary()) {
        Some((name, _)) => format!("twin://{name}/shaders/{stem}.wgsl"),
        None => format!("shaders/{stem}.wgsl"),
    }
}

/// Sanitise a free-text name into a safe lowercase file stem (`[a-z0-9_]`,
/// trimmed of leading/trailing `_`). Empty input → `"shader"`.
pub fn sanitize_stem(s: &str) -> String {
    let out: String = s
        .trim()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' { c.to_ascii_lowercase() } else { '_' })
        .collect();
    let out = out.trim_matches('_').to_string();
    if out.is_empty() { "shader".to_string() } else { out }
}

/// Core of [`CreateShader`]/[`ImportShader`]: validate the WGSL is a
/// prop-pickable dynamic shader, persist it into the open Twin (fallback
/// `assets/shaders/`), insert it live into [`Assets<Shader>`] so it renders
/// this frame, register it in the picker [`ShaderCatalog`], and optionally bind
/// it to `target` (API id; 0 = none). Returns the asset path on success.
#[allow(clippy::too_many_arguments)]
fn install_shader(
    stem: &str,
    source: &str,
    target: u64,
    twin_roots: Option<&lunco_assets::twin_source::TwinRoots>,
    asset_server: &AssetServer,
    shaders: &mut Assets<bevy::shader::Shader>,
    catalog: &mut lunco_materials::ShaderCatalog,
    registry: &lunco_api::registry::ApiEntityRegistry,
    q_look: &Query<&ShaderLook>,
    commands: &mut Commands,
) -> Option<String> {
    // Gate: must be a self-describing `Material` shader whose only engine field
    // (if any) is `sun_vis`. Otherwise it would render black / can't be driven.
    if !lunco_materials::is_prop_pickable_source(source) {
        warn!(
            "INSTALL_SHADER: '{stem}' is not a prop-pickable dynamic shader \
             (needs a `Material` struct; engine fields limited to `sun_vis`) — skipped"
        );
        return None;
    }

    // Destination: the primary open Twin's `shaders/` dir (portable, persists
    // with the Twin under a `twin://` asset path), else the engine library.
    let (asset_path, disk_path): (String, std::path::PathBuf) =
        match twin_roots.and_then(|t| t.primary()) {
            Some((name, root)) => (
                format!("twin://{name}/shaders/{stem}.wgsl"),
                root.join("shaders").join(format!("{stem}.wgsl")),
            ),
            None => (
                format!("shaders/{stem}.wgsl"),
                std::path::PathBuf::from("assets/shaders").join(format!("{stem}.wgsl")),
            ),
        };

    // Persist to disk (native). Non-fatal on failure — the in-memory insert
    // below still makes it usable this session.
    #[cfg(not(target_arch = "wasm32"))]
    {
        if let Some(parent) = disk_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match std::fs::write(&disk_path, source) {
            Ok(()) => info!("INSTALL_SHADER: wrote {}", disk_path.display()),
            Err(e) => warn!("INSTALL_SHADER: write {} failed: {e}", disk_path.display()),
        }
    }
    #[cfg(target_arch = "wasm32")]
    let _ = &disk_path;

    // Insert the compiled source live under the asset path, so any material
    // bound to it renders immediately (no disk round-trip / watcher wait).
    let shader_handle = asset_server.load::<bevy::shader::Shader>(asset_path.clone());
    let shader = bevy::shader::Shader::from_wgsl(source.to_string(), asset_path.clone());
    let _ = shaders.insert(shader_handle.id(), shader);

    // Make it pickable.
    catalog.add(asset_path.clone());

    // Optionally apply to a target entity (preserve any existing shader params).
    if target != 0 {
        let gid = lunco_core::GlobalEntityId::from_raw(target);
        match registry.resolve(&gid) {
            Some(ent) => {
                // Intent, not material: the binder loads the same `asset_path` we
                // just inserted the compiled source under, so it renders at once.
                author_shader_look(commands, ent, q_look.get(ent).ok(), &asset_path);
                info!("INSTALL_SHADER: applied {asset_path} to entity {target}");
            }
            None => warn!("INSTALL_SHADER: target id {target} not in registry"),
        }
    }

    info!("INSTALL_SHADER: registered {asset_path}");
    Some(asset_path)
}

/// Create a new dynamic shader from a built-in template (or supplied WGSL),
/// persist it into the open Twin (`<twin>/shaders/<name>.wgsl`, or
/// `assets/shaders/` when no Twin is open), register it in the picker, and
/// optionally bind it to a target entity — all live, no restart.
///
/// ```json
/// {"command":"CreateShader","params":{"name":"my_panel","template":"checker","target":42}}
/// {"command":"CreateShader","params":{"name":"custom","source":"<wgsl...>"}}
/// ```
#[Command(default)]
pub struct CreateShader {
    /// Display name / file stem, e.g. `"my_panel"` (sanitised to `[a-z0-9_]`).
    pub name: String,
    /// Template id when `source` is empty: `"solid"` (default) or `"checker"`.
    pub template: String,
    /// Full WGSL source. Empty → generate from `template`.
    pub source: String,
    /// API id of an entity to apply the new shader to. `0` = create only.
    pub target: u64,
}

/// Observer for [`CreateShader`].
#[allow(clippy::too_many_arguments)]
#[on_command(CreateShader)]
pub fn on_create_shader(
    trigger: On<CreateShader>,
    twin_roots: Option<Res<lunco_assets::twin_source::TwinRoots>>,
    asset_server: Res<AssetServer>,
    mut shaders: ResMut<Assets<bevy::shader::Shader>>,
    mut catalog: ResMut<lunco_materials::ShaderCatalog>,
    registry: Res<lunco_api::registry::ApiEntityRegistry>,
    q_look: Query<&ShaderLook>,
    mut commands: Commands,
) {
    let ev = trigger.event();
    let stem = sanitize_stem(&ev.name);
    let source = if ev.source.trim().is_empty() {
        lunco_materials::shader_template(&ev.template, &stem)
    } else {
        ev.source.clone()
    };
    install_shader(
        &stem,
        &source,
        ev.target,
        twin_roots.as_deref(),
        &asset_server,
        &mut shaders,
        &mut catalog,
        &registry,
        &q_look,
        &mut commands,
    );
}

/// Import an existing `.wgsl` file from anywhere on disk INTO the open Twin
/// (copies it to `<twin>/shaders/<name>.wgsl`), registers it in the picker, and
/// optionally binds it to a target entity. The file must be a prop-pickable
/// dynamic shader (a `Material` struct; engine fields limited to `sun_vis`).
///
/// ```json
/// {"command":"ImportShader","params":{"source_path":"/home/me/cool.wgsl","name":"cool","target":42}}
/// ```
#[Command(default)]
pub struct ImportShader {
    /// Filesystem path of the `.wgsl` to import (absolute or cwd-relative).
    pub source_path: String,
    /// Optional new stem; empty → keep the source file's own stem.
    pub name: String,
    /// API id of an entity to apply the imported shader to. `0` = import only.
    pub target: u64,
}

/// Observer for [`ImportShader`].
#[allow(clippy::too_many_arguments, unused_variables, unused_mut)]
#[on_command(ImportShader)]
pub fn on_import_shader(
    trigger: On<ImportShader>,
    twin_roots: Option<Res<lunco_assets::twin_source::TwinRoots>>,
    asset_server: Res<AssetServer>,
    mut shaders: ResMut<Assets<bevy::shader::Shader>>,
    mut catalog: ResMut<lunco_materials::ShaderCatalog>,
    registry: Res<lunco_api::registry::ApiEntityRegistry>,
    q_look: Query<&ShaderLook>,
    mut commands: Commands,
) {
    let ev = trigger.event();
    #[cfg(target_arch = "wasm32")]
    {
        warn!("IMPORT_SHADER: importing from a local file is native-only");
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let src = match std::fs::read_to_string(&ev.source_path) {
            Ok(s) => s,
            Err(e) => {
                warn!("IMPORT_SHADER: read '{}' failed: {e}", ev.source_path);
                return;
            }
        };
        let stem = if ev.name.trim().is_empty() {
            std::path::Path::new(&ev.source_path)
                .file_stem()
                .and_then(|s| s.to_str())
                .map(sanitize_stem)
                .unwrap_or_else(|| "shader".to_string())
        } else {
            sanitize_stem(&ev.name)
        };
        install_shader(
            &stem,
            &src,
            ev.target,
            twin_roots.as_deref(),
            &asset_server,
            &mut shaders,
            &mut catalog,
            &registry,
            &q_look,
            &mut commands,
        );
    }
}

/// Rescan the open Twins' `shaders/` folders (and `assets/shaders`) and register
/// any prop-pickable `.wgsl` into the picker [`ShaderCatalog`]. Lets you drop a
/// shader file into a Twin and pick it up without restarting.
#[Command(default)]
pub struct RescanShaders {}

/// THE shader scanner: register every project `*.wgsl` (engine library + open
/// Twins) into the picker catalog via the shared `lunco_assets::discovery`
/// walk — the same single scanner the spawn catalog uses for `*.usda`. No
/// filter: the picker lists all shaders and flags any whose `@engine` inputs a
/// part can't provide. Idempotent (`add` dedups). Returns the count added.
pub fn scan_wgsl_into_catalog(
    roots: &lunco_assets::twin_source::TwinRoots,
    catalog: &mut lunco_materials::ShaderCatalog,
) -> usize {
    let mut n = 0;
    for a in lunco_assets::discovery::list_assets(roots, "wgsl") {
        if catalog.add(a.asset_path) {
            n += 1;
        }
    }
    n
}

/// Populate BOTH catalogs (USD → spawn, WGSL → shaders) from the project. The
/// single scan entry point, driven by [`maintain_catalogs`] (Startup + on
/// Twin-set change) and the manual rescan commands — never a per-frame walk.
pub fn scan_all_catalogs(
    roots: &lunco_assets::twin_source::TwinRoots,
    spawn: &mut crate::catalog::SpawnCatalog,
    shaders: &mut lunco_materials::ShaderCatalog,
) {
    let s = crate::catalog::scan_usd_into_catalog(roots, spawn);
    let w = scan_wgsl_into_catalog(roots, shaders);
    if s > 0 || w > 0 {
        info!("CATALOG_SCAN: +{s} USD, +{w} shader(s)");
    }
}

/// The ONE catalog-population system. Scans the engine library once, then
/// re-scans whenever the set of open Twins changes (so a freshly-opened Twin's
/// files appear) — twin-open is async, so a guarded `Update` check is more
/// robust than racing the `TwinAdded` observer that registers the twin root.
/// It only *walks the disk* on first run and on change; every other frame it
/// early-returns after a cheap name-set comparison (no per-frame rescan).
pub fn maintain_catalogs(
    twin_roots: Option<Res<lunco_assets::twin_source::TwinRoots>>,
    mut spawn: ResMut<crate::catalog::SpawnCatalog>,
    mut shaders: ResMut<lunco_materials::ShaderCatalog>,
    mut last_twins: Local<Vec<String>>,
    mut did_first_scan: Local<bool>,
) {
    let Some(roots) = twin_roots.as_deref() else { return };
    let names = roots.names();
    if *did_first_scan && names == *last_twins {
        return;
    }
    *did_first_scan = true;
    *last_twins = names;
    scan_all_catalogs(roots, &mut spawn, &mut shaders);
}

/// Observer for [`RescanShaders`] — manual full re-scan of the shader catalog.
#[on_command(RescanShaders)]
pub fn on_rescan_shaders(
    _trigger: On<RescanShaders>,
    twin_roots: Option<Res<lunco_assets::twin_source::TwinRoots>>,
    mut catalog: ResMut<lunco_materials::ShaderCatalog>,
) {
    if let Some(roots) = twin_roots.as_deref() {
        let n = scan_wgsl_into_catalog(roots, &mut catalog);
        info!("RESCAN_SHADERS: +{n} shader(s)");
    }
}

/// Resolve a shader **asset path** to its **disk path**: `twin://<name>/<rel>` →
/// `<twin_root>/<rel>`; an engine path like `shaders/foo.wgsl` → `assets/<path>`.
#[cfg(not(target_arch = "wasm32"))]
fn asset_path_to_disk(
    path: &str,
    twin_roots: Option<&lunco_assets::twin_source::TwinRoots>,
) -> Option<std::path::PathBuf> {
    if let Some(rest) = path.strip_prefix("twin://") {
        let mut it = rest.splitn(2, '/');
        let name = it.next()?;
        let rel = it.next()?;
        Some(twin_roots?.root_of(name)?.join(rel))
    } else {
        Some(std::path::PathBuf::from("assets").join(path))
    }
}

/// Delete a shader: unregister it from the picker [`ShaderCatalog`] and remove
/// its `.wgsl` from disk (the twin's `shaders/` folder, or `assets/shaders`).
/// Entities currently using it keep their in-memory material for the session.
///
/// ```json
/// {"command":"DeleteShader","params":{"path":"twin://moonbase/shaders/old.wgsl"}}
/// ```
#[Command(default)]
pub struct DeleteShader {
    /// Asset path to remove (`twin://name/shaders/x.wgsl` or `shaders/x.wgsl`).
    pub path: String,
}

/// Observer for [`DeleteShader`].
#[allow(unused_variables)]
#[on_command(DeleteShader)]
pub fn on_delete_shader(
    trigger: On<DeleteShader>,
    twin_roots: Option<Res<lunco_assets::twin_source::TwinRoots>>,
    mut catalog: ResMut<lunco_materials::ShaderCatalog>,
) {
    let path = trigger.event().path.trim().to_string();
    if path.is_empty() {
        warn!("DELETE_SHADER: empty path");
        return;
    }
    let removed = catalog.remove(&path);
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(disk) = asset_path_to_disk(&path, twin_roots.as_deref()) {
        match std::fs::remove_file(&disk) {
            Ok(()) => info!("DELETE_SHADER: removed {path} ({})", disk.display()),
            Err(e) => warn!("DELETE_SHADER: unregistered {path}, file remove failed: {e}"),
        }
    }
    if !removed {
        warn!("DELETE_SHADER: '{path}' was not in the catalog");
    }
}

/// Plugin that registers SPAWN_ENTITY / MOVE_ENTITY / SET_OBJECT_PROPERTY /
/// FOCUS_ENTITY_BY_ID / SET_CAMERA_LOOK_AT / RELOAD_SHADER / SET_SHADER_SOURCE /
/// CREATE_SHADER / IMPORT_SHADER / RESCAN_SHADERS / DELETE_SHADER command
/// observers and the kinematic-pulse cleanup + twin shader auto-scan systems.
pub struct SpawnCommandPlugin;

/// Replace the flat USD-authored ground once an obstacle field exists: author a
/// `RemovePrim` op so the `Ground` prim leaves the active stage (the Twin document
/// stays consistent — a reload won't re-spawn it), and despawn its ECS projection
/// immediately so the generated heightfield is the only ground collider (also on
/// the headless server, where no viewport rebuild fires from the doc edit).
///
/// Lives here (not in the pure `lunco-obstacle-field` generator) because authoring
/// stage ops needs USD access. Change-driven via [`obstacle_field_scene_changed`]:
/// it scans only on frames where a field or a USD prim was just added, never
/// per-frame for the app lifetime.
fn remove_legacy_ground_prim(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    registry: Res<UsdDocumentRegistry>,
    ground: Query<(Entity, &UsdPrimPath)>,
) {
    for (entity, prim) in &ground {
        // The USD loader names entities by full prim path (e.g.
        // `/SandboxScene/Ground`); match the leaf. Generated terrain carries no
        // `UsdPrimPath`, so it can never match here.
        if prim.path.rsplit('/').next() != Some("Ground") {
            continue;
        }
        if let Some(doc) = doc_for_stage(&prim.stage_handle, &asset_server, &registry) {
            commands.trigger(ApplyUsdOp {
                doc,
                op: UsdOp::RemovePrim {
                    edit_target: LayerId::root(),
                    path: prim.path.clone(),
                },
            });
        }
        commands.entity(entity).try_despawn();
    }
}

/// Run condition for [`remove_legacy_ground_prim`]: a field exists and either it
/// or a USD prim was just added this frame. Handles both spawn orderings (field
/// before ground, ground before field) while keeping the system off every other
/// frame.
fn obstacle_field_scene_changed(
    fields_now: Query<(), With<ObstacleFieldRoot>>,
    fields_added: Query<(), Added<ObstacleFieldRoot>>,
    prims_added: Query<(), Added<UsdPrimPath>>,
) -> bool {
    !fields_now.is_empty() && (!fields_added.is_empty() || !prims_added.is_empty())
}

/// Resolve the open document that owns `stage_handle` by matching the stage asset
/// path against the registry (headless-safe — no viewport dependency).
fn doc_for_stage(
    stage_handle: &Handle<UsdStageAsset>,
    asset_server: &AssetServer,
    registry: &UsdDocumentRegistry,
) -> Option<DocumentId> {
    let asset_path = asset_server.get_path(stage_handle.id())?;
    let path_str = asset_path.path().to_string_lossy().into_owned();
    registry.ids().find(|id| {
        registry.host(*id).is_some_and(|h| match h.document().origin() {
            DocumentOrigin::File { path, .. } => path.to_string_lossy().ends_with(&path_str),
            _ => false,
        })
    })
}

// Generates `register_all_commands(app)` — every `#[Command]` this module owns,
// each wired type + observer together. `persist_*_to_runtime_layer` are NOT here:
// they are additional observers on the same verbs (the journaling/runtime-layer
// leg), not the command handlers, so they stay plain `add_observer`s.
register_commands!(
    on_create_shader,
    on_delete_entity,
    on_delete_shader,
    on_detach_joint,
    on_focus_entity_by_id,
    on_import_shader,
    on_move_entity_command,
    on_reload_shader,
    on_rescan_shaders,
    on_rescan_spawn_catalog,
    on_set_camera_look_at,
    on_set_object_property,
    on_set_shader_source,
    on_spawn_entity_command,
);

impl Plugin for SpawnCommandPlugin {
    fn build(&self, app: &mut App) {
        // Every `#[Command]` this crate owns — type + observer in one call, so a
        // verb can't end up observable-but-unconstructible (the old split wired
        // the observer by hand and then patched the type registry separately, and
        // whenever the second half was forgotten the command silently vanished
        // from the HTTP API / rhai / `discover_schema`).
        register_all_commands(app);
        // Dock release as an actuator on the intent→port machinery (replaces the
        // hardcoded G-to-detach): register the `release` port backend, attach a
        // ReleaseActuator to every control-bound vessel, and edge-detect → detach.
        app.register_type::<ReleaseActuator>();
        app.add_systems(Startup, register_release_backend);
        app.add_systems(Update, (attach_release_actuator, joint_release_system));
        // Persist a Persistent DetachJoint into the active doc's runtime layer.
        app.add_observer(persist_detach_to_runtime_layer);
        app.add_observer(persist_delete_to_runtime_layer);
        app.add_systems(
            Update,
            remove_legacy_ground_prim.run_if(obstacle_field_scene_changed),
        );
        // C4b: persist authored-scene moves into the active doc's runtime layer.
        app.add_observer(persist_move_to_runtime_layer);
        // C4b: persist palette/API spawns as referenced runtime-layer prims.
        app.add_observer(persist_spawn_to_runtime_layer);
        // #4: persist scalar shader-param tunes into the active doc's runtime
        // overlay (non-destructive; Save stays base-only). Decoupled from the
        // live-mutation handler above, like the move/spawn persisters.
        app.add_observer(persist_property_to_runtime_layer);
        // #15: persist wheel-dynamics tunes (suspension/drive → physxVehicle*) and
        // PBR base_color (→ primvars:displayColor) — the classes the shader-param
        // persister skips. Disjoint property sets, so both observers coexist.
        app.add_observer(persist_wheel_and_pbr_to_runtime_layer);
        // #14: persist a `SetEnvironmentLight` sun tweak (illuminance / colour /
        // shadow range) as `SetAttribute`s on the sun's DistantLight prim, using
        // the names the loader already reads back — so it round-trips + journals.
        app.add_observer(persist_environment_light_to_runtime_layer);
        // Applies the recorded focus at frame start, when last frame's fully
        // propagated GlobalTransforms are mutually consistent (see PendingFocus).
        app.add_systems(bevy::app::First, apply_pending_focus);
        // NOTE: `SelectEntity`/`on_select_entity` are editor-only (they drive the
        // Inspector highlight + gizmo) and live in the `ui`-gated `selection`
        // module; `SandboxEditPlugin` registers them. The headless server has no
        // selection, so they're absent here by design.
        // THE single catalog-population system: scans project USD → spawn
        // catalog and WGSL → shader catalog via the shared `lunco_assets`
        // discovery walk, once at first run and again only when the open-Twin
        // set changes (guarded — no per-frame disk walk). Replaces the old
        // per-catalog scanners (`populate_dynamic_spawn_catalog`,
        // `auto_scan_twin_shaders`, `discover_shaders`).
        app.add_systems(Update, maintain_catalogs);
        app.add_systems(FixedPostUpdate, clear_kinematic_pulse_velocity);
        // Resources this plugin's OWN systems read, so it stands alone without the
        // UI-layer `SandboxEditPlugin` / the render-layer `ShaderMaterialPlugin`
        // (e.g. a headless `--no-ui` server that adds only `SpawnCommandPlugin`).
        // `init_resource` is idempotent, so when those plugins also init these it's
        // a harmless no-op:
        //   - `SpawnCatalog`   — read by `maintain_catalogs` + `apply_replicated_spawns`;
        //   - `SelectedEntity` — read by `on_select_entity`;
        //   - `ShaderCatalog`  — read by `maintain_catalogs` (per-frame) + the shader
        //     command observers. Lives in `lunco_materials`; an empty one is fine on
        //     a server (shader discovery populates it but nothing renders it).
        app.init_resource::<crate::catalog::SpawnCatalog>();
        app.init_resource::<crate::SelectedEntities>();
        app.init_resource::<lunco_materials::ShaderCatalog>();
        // Client: instantiate host-replicated spawns. The rest of the old netcode
        // chain (interp / kinematic-pin / predict / reconcile / rollback) moved to
        // `lunco_networking::prediction::NetcodePredictionPlugin`; this one system
        // stayed because it spawns from the editor's `SpawnCatalog`. It was the
        // chain's FIRST system, and that ordering is preserved across the crate
        // boundary by the shared `lunco_core::NetcodeSet` (the prediction half runs
        // in `NetcodeSet::Predict`, configured `.after(InstantiateSpawns)` there).
        // No-op in single-player (the queue stays empty).
        app.add_systems(
            Update,
            apply_replicated_spawns.in_set(lunco_core::NetcodeSet::InstantiateSpawns),
        );
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_spawn_entity_struct_exists() {
        // Verify the struct can be constructed
        let cmd = super::SpawnEntity {
            target: bevy::prelude::Entity::PLACEHOLDER,
            entry_id: "test".to_string(),
            position: bevy::prelude::Vec3::ZERO,
            rotation: None,
        };
        assert_eq!(cmd.entry_id, "test");
    }

    // ── C4b: move-transform → runtime-layer persistence ─────────────────

    /// Build a headless app with the runtime-move producer wired and an active
    /// USD document containing `/World`, plus a sim entity bound to `prim_path`
    /// under api id `api_id`. Returns `(app, doc_id)`.
    fn app_with_runtime_producer(
        prim_path: &str,
        api_id: u64,
    ) -> (bevy::prelude::App, lunco_doc::DocumentId) {
        use bevy::prelude::*;
        use super::*;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        // UsdCommandsPlugin inserts UsdDocumentRegistry + the `on_apply_usd_op`
        // observer that processes the `ApplyUsdOp` our producer dispatches.
        app.add_plugins(lunco_usd::commands::UsdCommandsPlugin);
        app.init_resource::<lunco_api::registry::ApiEntityRegistry>();
        app.add_observer(persist_move_to_runtime_layer);

        let doc = {
            let mut reg = app.world_mut().resource_mut::<UsdDocumentRegistry>();
            reg.allocate(
                "#usda 1.0\ndef Xform \"World\"\n{\n}\n".to_string(),
                lunco_doc::DocumentOrigin::untitled("Scene.usda".to_string()),
            )
        };
        let mut ws = lunco_workspace::Workspace::default();
        ws.active_document = Some(doc);
        app.insert_resource(lunco_workspace::WorkspaceResource(ws));

        let ent = app
            .world_mut()
            .spawn(UsdPrimPath {
                stage_handle: Handle::default(),
                path: prim_path.to_string(),
            })
            .id();
        app.world_mut()
            .resource_mut::<lunco_api::registry::ApiEntityRegistry>()
            .assign(ent, lunco_core::GlobalEntityId::from_raw(api_id));
        app.update();
        (app, doc)
    }

    #[test]
    fn move_of_authored_prim_persists_to_runtime_layer() {
        use bevy::prelude::*;
        use super::*;
        use lunco_usd_bevy::usd_data::UsdDataExt;

        let (mut app, doc) = app_with_runtime_producer("/World", 42);
        app.world_mut().trigger(MoveEntity {
            entity_id: 42,
            translation: Vec3::new(3.0, 4.0, 5.0),
        });
        for _ in 0..3 {
            app.update();
        }

        let reg = app.world().resource::<UsdDocumentRegistry>();
        let docu = reg.host(doc).expect("doc alive").document();
        let world = lunco_usd_bevy::SdfPath::new("/World").unwrap();
        // The move landed in the RUNTIME layer...
        // TODO(usd-read-migration): switch to the generic UsdRead surface (`scalar`)
        // instead of the legacy `prim_attribute_value`, matching production (doc 21).
        assert_eq!(
            docu.runtime_data()
                .prim_attribute_value::<[f64; 3]>(&world, "xformOp:translate"),
            Some([3.0, 4.0, 5.0]),
            "authored-scene move persists to the runtime layer"
        );
        // ...and the base layer (what Save writes) stays clean.
        let attr = lunco_usd_bevy::SdfPath::new("/World.xformOp:translate").unwrap();
        assert!(docu.data().spec(&attr).is_none(), "base layer untouched");
        assert!(!docu.source().contains("xformOp:translate"), "save excludes runtime move");
    }

    // ── A10: ONE wheel-param table ──────────────────────────────────────

    /// The whole point of collapsing the two hand-synced tables: a wheel
    /// property cannot be settable-but-not-persistable (the drift that lost
    /// `slip_stiffness` / `friction_mu` tunes on every reload). One row = one
    /// param = a setter AND a USD attribute, always.
    #[test]
    fn every_wheel_param_has_both_a_setter_and_a_usd_attr() {
        use super::*;
        use std::collections::HashSet;

        assert!(!WHEEL_PARAMS.is_empty());
        let mut seen_alias: HashSet<&str> = HashSet::new();
        let mut seen_attr: HashSet<&str> = HashSet::new();
        for p in WHEEL_PARAMS {
            assert!(!p.aliases.is_empty(), "a param with no name is unreachable");
            assert!(!p.usd_attr.is_empty(), "every param must round-trip through USD");
            assert!(seen_attr.insert(p.usd_attr), "duplicate USD attr {}", p.usd_attr);
            for a in p.aliases {
                assert!(seen_alias.insert(a), "duplicate alias {a}");
                // Both consumers (live setter + USD persister) resolve through the
                // SAME lookup, so a name that sets a field always has an attr.
                let row = wheel_param(a).expect("alias resolves");
                assert_eq!(row.usd_attr, p.usd_attr);
            }
        }

        // The two names the old split tables disagreed about are now complete.
        for name in ["slip_stiffness", "friction_mu", "mass"] {
            let row = wheel_param(name).expect("wheel param exists");
            assert!(!row.usd_attr.is_empty(), "{name} persists to USD");
        }
        assert!(wheel_param("not_a_wheel_field").is_none());

        // Setters write the field they claim.
        let mut w = lunco_mobility::WheelRaycast::default();
        (wheel_param("slip_stiffness").unwrap().set)(&mut w, 1234.0);
        (wheel_param("friction_mu").unwrap().set)(&mut w, 0.5);
        assert_eq!(w.slip_stiffness, 1234.0);
        assert_eq!(w.friction_mu, 0.5);
    }

    // ── A8: one history — the document's ────────────────────────────────

    /// Ctrl+Z routes to `UndoDocument`, which pops the USD document's last op
    /// and applies its inverse. The editor keeps no private undo stack, so the
    /// journal and the editor can no longer disagree.
    #[test]
    fn undo_document_reverts_the_last_usd_op() {
        use bevy::prelude::*;
        use super::*;
        use lunco_doc::Document;
        use lunco_usd_bevy::usd_data::UsdDataExt;

        let (mut app, doc) = app_with_runtime_producer("/World", 42);
        // USD's half of the generic verb now lives in `lunco-usd` (see the note above
        // `handle_undo_input`), so the test wires the real observer from there.
        app.add_observer(lunco_usd::commands::on_undo_usd_document);
        app.world_mut().trigger(MoveEntity {
            entity_id: 42,
            translation: Vec3::new(3.0, 4.0, 5.0),
        });
        for _ in 0..3 {
            app.update();
        }
        let world_path = lunco_usd_bevy::SdfPath::new("/World").unwrap();
        let gen_after_move = {
            let reg = app.world().resource::<UsdDocumentRegistry>();
            let docu = reg.host(doc).unwrap().document();
            assert_eq!(
                docu.runtime_data()
                    .prim_attribute_value::<[f64; 3]>(&world_path, "xformOp:translate"),
                Some([3.0, 4.0, 5.0])
            );
            docu.generation()
        };

        // The editor's undo verb — the SAME one the journal / other domains use.
        app.world_mut().trigger(UndoDocument { doc });
        for _ in 0..3 {
            app.update();
        }

        let reg = app.world().resource::<UsdDocumentRegistry>();
        let docu = reg.host(doc).unwrap().document();
        assert!(
            docu.generation() > gen_after_move,
            "undo applies an inverse op (history moves forward, state moves back)"
        );
        assert_ne!(
            docu.runtime_data()
                .prim_attribute_value::<[f64; 3]>(&world_path, "xformOp:translate"),
            Some([3.0, 4.0, 5.0]),
            "the move is undone in the document, not just in ECS"
        );
    }

    #[test]
    fn move_of_unowned_entity_is_skipped() {
        use bevy::prelude::*;
        use super::*;
        use lunco_doc::Document;

        // Entity bound to a prim the document does NOT contain (e.g. a palette
        // spawn referencing an external asset).
        let (mut app, doc) = app_with_runtime_producer("/PaletteSpawn", 7);
        app.world_mut().trigger(MoveEntity {
            entity_id: 7,
            translation: Vec3::new(1.0, 2.0, 3.0),
        });
        for _ in 0..3 {
            app.update();
        }

        let reg = app.world().resource::<UsdDocumentRegistry>();
        let docu = reg.host(doc).expect("doc alive").document();
        // No op authored — the ownership guard skipped a non-document entity.
        assert_eq!(docu.generation(), 0, "un-owned entity move authors nothing");
        assert!(docu
            .runtime_data()
            .spec(&lunco_usd_bevy::SdfPath::new("/PaletteSpawn").unwrap())
            .is_none());
    }

    // ── C4b: spawn → referenced runtime-layer prim ──────────────────────

    #[test]
    fn spawn_persists_referenced_prim_to_runtime_layer() {
        use bevy::prelude::*;
        use super::*;
        use lunco_usd_bevy::usd_data::UsdDataExt;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(lunco_usd::commands::UsdCommandsPlugin);
        // Persistence is a HOST behaviour: the observer deliberately skips
        // `Standalone` (there the direct ECS spawn is the sole instance, and
        // authoring it into the runtime layer too would double-instantiate it).
        // A real app always has the role; this hand-built one must supply it or
        // the observer fails `Res` validation.
        app.insert_resource(lunco_core::NetworkRole::Host);
        app.add_observer(persist_spawn_to_runtime_layer);

        // Catalog with one spawnable asset.
        let mut catalog = SpawnCatalog::default();
        catalog.add_unique(crate::catalog::SpawnableEntry {
            id: "test_rover".into(),
            display_name: "Test Rover".into(),
            category: "Rovers".into(),
            source: SpawnSource::UsdFile("vessels/rovers/test_rover.usda".into()),
            spawn_lift: 0.0,
            default_transform: Transform::default(),
        });
        app.insert_resource(catalog);

        // Active USD doc whose default prim is /World.
        let doc = {
            let mut reg = app.world_mut().resource_mut::<UsdDocumentRegistry>();
            reg.allocate(
                "#usda 1.0\n(\n    defaultPrim = \"World\"\n)\ndef Xform \"World\"\n{\n}\n".to_string(),
                lunco_doc::DocumentOrigin::untitled("Scene.usda".to_string()),
            )
        };
        let mut ws = lunco_workspace::Workspace::default();
        ws.active_document = Some(doc);
        app.insert_resource(lunco_workspace::WorkspaceResource(ws));
        app.update();

        // Spawn it at a drop position.
        let grid = app.world_mut().spawn_empty().id();
        app.world_mut().trigger(SpawnEntity {
            target: grid,
            entry_id: "test_rover".into(),
            position: Vec3::new(2.0, 0.0, 7.0),
            rotation: None,
        });
        for _ in 0..3 {
            app.update();
        }

        let reg = app.world().resource::<UsdDocumentRegistry>();
        let docu = reg.host(doc).expect("doc alive").document();
        let prim = lunco_usd_bevy::SdfPath::new("/World/test_rover_1").unwrap();
        // The referenced spawn prim landed under the default prim, in RUNTIME...
        assert!(docu.runtime_data().spec(&prim).is_some(), "spawn prim authored in runtime layer");
        assert!(docu.data().spec(&prim).is_none(), "base layer untouched by spawn");
        // TODO(usd-read-migration): switch to the generic UsdRead surface (`scalar`)
        // instead of the legacy `prim_attribute_value`, matching production (doc 21).
        assert_eq!(
            docu.runtime_data().prim_attribute_value::<[f64; 3]>(&prim, "xformOp:translate"),
            Some([2.0, 0.0, 7.0]),
            "spawn drop position recorded in runtime layer"
        );
        // ...rides into the composed view as a resolvable reference...
        let composed = docu.composed_source();
        assert!(
            composed.contains("@vessels/rovers/test_rover.usda@"),
            "composed view must carry the spawn reference:\n{composed}"
        );
        // ...and is excluded from Save (base only).
        assert!(!docu.source().contains("test_rover"), "spawn leaked into save:\n{}", docu.source());
    }
}
