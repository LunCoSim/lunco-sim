//! Command handlers for scene-edit world manipulation.
//!
//! - `SpawnEntity` — spawn from the catalog at a world position.
//! - `MoveEntity` — teleport an entity to an absolute world position.
//! - `TransformEntity` — teleport an entity's complete pose in one command.
//!   This is the command path used by the gizmo on drag-end: swap to Kinematic,
//!   update the authoritative Transform/cell pose, and let the BigSpace/Avian
//!   adapter propagate it to coupled bodies. Lets API clients (MCP tools,
//!   automated tests) drive entity motion exactly the way a human would with
//!   the gizmo.

use avian3d::prelude::{AngularVelocity, LinearVelocity, RigidBody};
use bevy::math::{DQuat, DVec3};
use bevy::prelude::*;
use big_space::prelude::{CellCoord, Grid};
use lunco_core::{on_command, register_commands, Command, SpawnEntity};
// Appearance INTENT (render-free). `SetObjectProperty`'s PBR keys mutate `PbrLook`
// and its shader keys mutate `ShaderLook`; the render binders re-materialise on
// `Changed<PbrLook>` / `Changed<ShaderLook>`. This file names no material type —
// see `docs/architecture/render-decoupling.md`.
use crate::catalog::{spawn_usd_entry, SpawnAnchor, SpawnCatalog, SpawnSource};
use lunco_doc_bevy::DocumentRegistry;
use lunco_doc_bevy::{RedoDocument, UndoDocument};
use lunco_materials::{ParamSchema, ParamValue, ShaderLook};
use lunco_render::{PbrLook, SurfaceAlpha};
use lunco_usd::commands::{ApplyUsdOp, ApplyUsdOps};
use lunco_usd::document::UsdDocument;
use lunco_usd::document::{LayerId, UsdOp};
use lunco_usd_bevy::{UsdPrimPath, UsdSceneRoot};

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

/// Observer for [`RescanSpawnCatalog`]. Forgets what has been read so far, so
/// the dispatch below re-reads every asset — an edit to a file already scanned
/// is exactly what a manual rescan is for. The reads land asynchronously; the
/// catalogue fills in over the next frames (`drain_usd_scan`).
#[on_command(RescanSpawnCatalog)]
pub fn on_rescan_spawn_catalog(
    _trigger: On<RescanSpawnCatalog>,
    twin_roots: Option<Res<lunco_assets::twin_source::TwinRoots>>,
    manifest: Res<lunco_assets::discovery::AssetManifest>,
    mut scan: ResMut<crate::catalog::CatalogScan>,
    settings: Res<lunco_settings::DownloadSettings>,
) {
    if let Some(roots) = twin_roots.as_deref() {
        scan.forget();
        let n = crate::catalog::dispatch_usd_scan(&manifest, roots, &mut scan, &settings);
        info!("RESCAN_SPAWN_CATALOG: re-reading {n} USD asset(s)");
    }
}

/// Observer that handles DetachJoint commands — despawns the live joint entity in
/// BOTH modes (the visible effect). Persistence is a decoupled observer below.
#[on_command(DetachJoint)]
pub fn on_detach_joint(trigger: On<DetachJoint>, mut commands: Commands) {
    let cmd = trigger.event();
    if let Ok(mut entity) = commands.get_entity(cmd.target) {
        entity.try_despawn();
        info!(
            "DETACH_JOINT: despawned joint entity {:?} ({:?})",
            cmd.target, cmd.intent
        );
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
        if n != "release" {
            return None;
        }
        w.get::<ReleaseActuator>(e).map(|a| a.cmd as f64)
    },
    read_input: |w, e, n| {
        if n != "release" {
            return None;
        }
        w.get::<ReleaseActuator>(e).map(|a| a.cmd as f64)
    },
    write_input: |w, e, n, v| {
        if n != "release" {
            return false;
        }
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
/// its `release` port is where the control binding actually writes. A USD prim can
/// spawn several entities sharing one `UsdPrimPath`; the control binding targets
/// the control/model entity, while `joint_release_system` bridges to the physics
/// body by path.
fn attach_release_actuator(
    mut commands: Commands,
    q: Query<
        (
            Entity,
            &lunco_core::ControlBinding,
            Option<&ReleaseActuator>,
        ),
        Changed<lunco_core::ControlBinding>,
    >,
) {
    for (e, binding, actuator) in &q {
        let has_release = binding.has_intent(lunco_core::architecture::UserIntent::Release);
        if has_release && actuator.is_none() {
            // `try_insert`: scene-load churn (or a doc-backed reload) can despawn a
            // just-added ControlBinding entity before this deferred insert applies —
            // a plain `insert` then panics on the invalid entity. Same despawn-safe
            // idiom as gizmo/hardware/terrain-surface.
            commands.entity(e).try_insert(ReleaseActuator::default());
        } else if !has_release && actuator.is_some() {
            commands.entity(e).try_remove::<ReleaseActuator>();
        }
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
                    let hit = [j.body1, j.body2]
                        .into_iter()
                        .any(|b| body_paths.get(b).is_ok_and(|p| p.path == vpath.path));
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
    usd_registry: Res<DocumentRegistry<UsdDocument>>,
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

/// Lower one document-backed spawn into the single USD mutation that owns its
/// live entity, journal entry, reload behaviour, and network propagation.
fn runtime_spawn_ops(
    entry_id: &str,
    asset_path: &str,
    parent_path: &str,
    position: DVec3,
    rotation: DQuat,
) -> (String, Vec<UsdOp>) {
    let stem: String = entry_id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    // OpIds are process-unique, JS-safe and already the identity source for
    // document mutations. Unlike a Local<u32>, this cannot collide with a
    // restored runtime-layer spawn after reload.
    let name = format!("{stem}_{}", lunco_core::OpId::new());
    let parent_path = parent_path.trim_end_matches('/');
    let parent_path = if parent_path.is_empty() {
        "/".to_string()
    } else {
        parent_path.to_string()
    };
    let prim_path = if parent_path == "/" {
        format!("/{name}")
    } else {
        format!("{parent_path}/{name}")
    };
    let (rx, ry, rz) = rotation.to_euler(EulerRot::XYZ);
    let ops = vec![
        UsdOp::AddPrim {
            edit_target: LayerId::runtime(),
            parent_path,
            name,
            // A catalog reference is mounted on an Xform instance root. Keep
            // that authored root type in the runtime layer so the referenced
            // root's applied schemas compose onto the same prim; a bare
            // references arc only contributes the child namespace in the
            // live stage and leaves the body root typeless.
            type_name: Some("Xform".to_string()),
            // The runtime layer is authored under the mounted scene. A bare
            // library path would therefore resolve relative to that scene
            // (for example `scenes/tests/structures/...`) instead of the
            // engine asset source. Keep the USD reference source-qualified at
            // the ownership boundary; the catalog remains free to expose its
            // discovery spelling to UI consumers.
            reference: Some(lunco_assets::engine_asset_uri(asset_path)),
        },
        UsdOp::SetAttribute {
            edit_target: LayerId::runtime(),
            path: prim_path.clone(),
            name: "lunco:catalogId".to_string(),
            type_name: "string".to_string(),
            value: entry_id.to_string(),
        },
        UsdOp::SetTranslate {
            edit_target: LayerId::runtime(),
            path: prim_path.clone(),
            value: position.to_array(),
        },
        UsdOp::SetRotate {
            edit_target: LayerId::runtime(),
            path: prim_path.clone(),
            value: [rx.to_degrees(), ry.to_degrees(), rz.to_degrees()],
        },
    ];
    (prim_path, ops)
}

#[on_command(SpawnEntity)]
pub fn on_spawn_entity_command(
    trigger: On<SpawnEntity>,
    mut commands: Commands,
    catalog: Res<SpawnCatalog>,
    asset_server: Res<AssetServer>,
    active_frame: Res<lunco_core::ActivePhysicsFrame>,
    q_scene_root: Query<(Entity, &UsdPrimPath), With<UsdSceneRoot>>,
    q_parents: Query<&ChildOf>,
    q_grids: Query<&Grid>,
    q_spatial: Query<(Option<&CellCoord>, &Transform)>,
    role: Res<lunco_core::NetworkRole>,
    backed: Res<lunco_usd::twin_projection::DocBackedTwinScenes>,
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

    if entry.is_route_marker() {
        warn!(
            "SPAWN_ENTITY: '{}' is a route marker and cannot be spawned independently; use AddRuntimeWaypoint with an explicit vessel",
            cmd.entry_id
        );
        return;
    }

    if q_grids.get(active_frame.0).is_err() {
        warn!(
            active_frame = ?active_frame.0,
            "SPAWN_ENTITY: active physics frame is not a BigSpace Grid"
        );
        return;
    }
    let Ok((scene_root, scene_root_prim)) = q_scene_root.single() else {
        warn!(
            "SPAWN_ENTITY: expected one mounted scene root for '{}'",
            cmd.entry_id
        );
        return;
    };

    // The public command is expressed in the semantic active physics frame.
    // Convert once to the mounted scene root's local frame, which is the actual
    // parent used by both authored and runtime top-level prims. This handles the
    // ordinary world-grid scene root and the site root that is itself promoted
    // to the rotating ENU Grid with the same formula.
    let requested_position = DVec3::from_array(cmd.position);
    let requested_rotation = cmd
        .rotation
        .map(DQuat::from_array)
        .unwrap_or(DQuat::IDENTITY)
        .normalize();
    let Some((position, rotation)) = lunco_core::coords::pose_in_parent_local(
        requested_position,
        requested_rotation,
        scene_root,
        active_frame.0,
        &q_parents,
        &q_grids,
        &q_spatial,
    ) else {
        warn!(
            ?scene_root,
            active_frame = ?active_frame.0,
            "SPAWN_ENTITY: scene root is not attached to the active physics frame"
        );
        return;
    };
    if !position.is_finite() || !rotation.is_finite() {
        warn!("SPAWN_ENTITY: non-finite pose for '{}'", cmd.entry_id);
        return;
    }
    let Ok(scene_grid) = q_grids.get(scene_root) else {
        warn!(
            ?scene_root,
            "SPAWN_ENTITY: scene root is not a BigSpace Grid"
        );
        return;
    };
    let (spawn_cell, spawn_local_position) = scene_grid.translation_to_grid(position);

    // A document-backed running scene is projected from USD. Author the spawn
    // there and let that ONE projection instantiate it. This is also the one
    // journal/network/reload path. Raw-file/headless scenes have no document to
    // author into and therefore use the direct ECS + NetSpawn path below.
    if let Some(doc) = lunco_usd::twin_projection::scene_document_for(
        &backed,
        &asset_server,
        scene_root_prim.stage_handle.id(),
    ) {
        let SpawnSource::UsdFile(asset_path) = &entry.source;
        let (prim_path, ops) = runtime_spawn_ops(
            &cmd.entry_id,
            asset_path,
            &scene_root_prim.path,
            position,
            rotation,
        );
        info!("SPAWN_ENTITY: authoring {} at {:?}", prim_path, position);
        commands.trigger(ApplyUsdOps {
            doc,
            label: format!("Spawn {}", entry.display_name),
            ops,
        });
        return;
    }

    info!(
        "SPAWN_ENTITY: directly instantiating {} at {:?}",
        cmd.entry_id, position
    );
    let result = spawn_usd_entry(
        &mut commands,
        &asset_server,
        entry,
        spawn_cell,
        spawn_local_position,
        rotation.as_quat(),
        SpawnAnchor::scene_root(scene_root),
    );

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
            position: requested_position,
            rotation: requested_rotation,
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
    active_frame: Res<lunco_core::ActivePhysicsFrame>,
    q_scene_root: Query<Entity, With<UsdSceneRoot>>,
    q_parents: Query<&ChildOf>,
    q_grids: Query<&Grid>,
    q_spatial: Query<(Option<&CellCoord>, &Transform)>,
    diagnostics: Option<ResMut<lunco_core::RuntimeDiagnostics>>,
) {
    if pending.0.is_empty() {
        return;
    }
    // Wait until the scene anchor exists (scene still loading) — keep the queue.
    // It is the only legal anchor, so there is nothing to do without it.
    let root_count = q_scene_root.iter().count();
    let Some(scene_root) = q_scene_root.single().ok() else {
        if let Some(mut diagnostics) = diagnostics {
            if root_count > 1 {
                diagnostics.replace_producer(
                    "scene-spawn",
                    [lunco_core::RuntimeDiagnostic {
                        code: "scene-spawn".to_string(),
                        severity: lunco_core::DiagnosticSeverity::Error,
                        producer: "scene-spawn".to_string(),
                        subject: "UsdSceneRoot".to_string(),
                        message: format!(
                            "replicated spawn requires exactly one UsdSceneRoot, found {root_count}"
                        ),
                    }],
                );
            } else {
                diagnostics.replace_producer("scene-spawn", std::iter::empty());
            }
        }
        return;
    };
    if let Some(mut diagnostics) = diagnostics {
        diagnostics.replace_producer("scene-spawn", std::iter::empty());
    }
    // Drain in place — the loop body touches only `commands`/`catalog`/
    // `asset_server`, never `pending`, so the old `.collect::<Vec<_>>()`
    // was a pure-waste allocation (CQ-216).
    for job in pending.0.drain(..) {
        let Some(entry) = catalog.get(&job.entry_id) else {
            warn!("REPL_SPAWN: unknown entry '{}'", job.entry_id);
            continue;
        };
        let Some((Some(cell), local_position, local_rotation)) =
            lunco_core::coords::pose_in_grid_to_parent_storage(
                job.position,
                job.rotation,
                scene_root,
                active_frame.0,
                &q_parents,
                &q_grids,
                &q_spatial,
            )
        else {
            warn!(
                ?scene_root,
                active_frame = ?active_frame.0,
                "REPL_SPAWN: cannot express replicated pose in the scene-root Grid"
            );
            continue;
        };
        let result = spawn_usd_entry(
            &mut commands,
            &asset_server,
            entry,
            cell,
            local_position,
            local_rotation.as_quat(),
            SpawnAnchor::scene_root(scene_root),
        );
        // Pin the host id; mark runtime instance + replication target. Forced
        // Kinematic by `force_kinematic_proxies` so snapshots drive it.
        commands.entity(result.root_entity).try_insert((
            lunco_core::GlobalEntityId::from_raw(job.gid),
            lunco_core::SkipContentStamp,
            lunco_core::NetReplicate,
        ));
    }
}

/// Move an existing entity to a position in the active physics frame.
///
/// Programmatic equivalent of grabbing the entity with the gizmo and
/// dragging it. The handler:
/// 1. Switches the body to `RigidBody::Kinematic` (if it has a
///    `RigidBody`) so Avian treats the new pose as authoritative
///    rather than fighting back via integration.
/// 2. Converts the active-frame target once into the entity's actual parent
///    and BigSpace cell/local storage.
/// 3. Lets the BigSpace physics bridge derive Avian's pose from that one
///    authoritative storage write.
/// 4. Sets a one-tick `LinearVelocity` consistent with the move so
///    any joint coupled to a dynamic body propagates the motion.
///
/// Designed for automated tests / MCP tool clients that need to
/// drive the world without a mouse. Single-shot — body type stays
/// Kinematic until another command (or a gizmo drag-end) restores it.
#[Command(default)]
pub struct MoveEntity {
    /// API-stable global entity ID from `ListEntities`, resolved to the live
    /// Bevy entity by `ApiEntityRegistry`.
    pub entity_id: u64,
    /// Target translation in the semantic [`lunco_core::ActivePhysicsFrame`].
    /// The concrete BigSpace grid, the entity's actual parent, and the cell/local
    /// split are internal storage details resolved by the observer. The wire
    /// representation is f64 so positions retain precision across API/network
    /// round trips.
    pub translation: [f64; 3],
}

/// Maximum one-command displacement for a physics body.
///
/// `MoveEntity` and `TransformEntity` are the scene-edit commit verbs, so an
/// input discontinuity must not turn one frame of pointer movement into a
/// kilometre-scale teleport. Large deliberate teleports remain available for
/// non-physics scene entities; dynamic and kinematic bodies use the authored
/// scene bounds and this local continuity guard.
pub const MAX_MOVE_ENTITY_DISPLACEMENT: f64 = 500.0;

/// Return the shortest angular displacement from `previous` to `target`.
///
/// Kinematic bodies expose angular velocity to Avian in the same active physics
/// frame as their rotation. A complete pose edit therefore needs the rotational
/// counterpart to the linear one-tick pulse used by [`MoveEntity`].
fn shortest_angular_delta(previous: DQuat, target: DQuat) -> DVec3 {
    let mut delta = target.normalize() * previous.normalize().inverse();
    // `q` and `-q` represent the same orientation. Keep the pulse on the
    // shortest arc so a sign-only wire difference cannot request a full turn.
    if delta.w < 0.0 {
        delta = -delta;
    }
    delta.to_scaled_axis()
}

/// Observer for `MoveEntity`.
#[on_command(MoveEntity)]
pub fn on_move_entity_command(
    trigger: On<MoveEntity>,
    time: Res<Time>,
    registry: Res<lunco_api::registry::ApiEntityRegistry>,
    active_frame: Res<lunco_core::ActivePhysicsFrame>,
    mut commands: Commands,
    mut spatial: ParamSet<(
        Query<(Option<&CellCoord>, &Transform)>,
        Query<(&mut Transform, Option<&mut LinearVelocity>)>,
    )>,
    q_parents: Query<&ChildOf>,
    q_grids: Query<&Grid>,
    q_rb: Query<&RigidBody>,
    q_marker: Query<&JustMovedKinematic>,
    bounds: Option<Res<lunco_physics::WorldBounds>>,
) {
    let cmd = trigger.event();
    if cmd.translation.iter().any(|value| !value.is_finite()) {
        warn!(
            "MOVE_ENTITY: rejecting non-finite active-frame target for api_id={}",
            cmd.entity_id
        );
        return;
    }
    let global_id = lunco_core::GlobalEntityId::from_raw(cmd.entity_id);
    let Some(target) = registry.resolve(&global_id) else {
        warn!("MOVE_ENTITY: no api_id={} in registry", cmd.entity_id);
        return;
    };
    let target_abs = DVec3::from_array(cmd.translation);
    if q_grids.get(active_frame.0).is_err() {
        warn!(
            active_frame = ?active_frame.0,
            "MOVE_ENTITY: active physics frame is not a BigSpace Grid"
        );
        return;
    }
    // Read and invert the complete hierarchy before taking mutable component
    // access. The command always speaks the active frame; storage may be a
    // plain parent-local Transform or a BigSpace `(CellCoord, Transform)` pair.
    // One canonical conversion owns both cases.
    let (prev_abs, old_cell, new_cell, new_local) = {
        let q_spatial = spatial.p0();
        let Ok((old_cell, _)) = q_spatial.get(target) else {
            warn!(
                "MOVE_ENTITY: entity {:?} (api_id={}) has no Transform",
                target, cmd.entity_id
            );
            return;
        };
        let Some((prev_abs, _)) = lunco_core::coords::pose_in_grid(
            target,
            active_frame.0,
            &q_parents,
            &q_grids,
            &q_spatial,
        ) else {
            warn!(
                ?target,
                active_frame = ?active_frame.0,
                "MOVE_ENTITY: entity is not connected to the active physics frame"
            );
            return;
        };
        let Some((new_cell, new_local)) = lunco_core::coords::position_in_grid_to_parent_local(
            target,
            target_abs,
            active_frame.0,
            &q_parents,
            &q_grids,
            &q_spatial,
        ) else {
            warn!(
                ?target,
                active_frame = ?active_frame.0,
                "MOVE_ENTITY: cannot express target in the entity parent frame"
            );
            return;
        };
        (prev_abs, old_cell.copied(), new_cell, new_local)
    };
    let delta = target_abs - prev_abs;
    let physics_body = q_rb
        .get(target)
        .is_ok_and(|rb| !matches!(rb, RigidBody::Static));
    if physics_body {
        if delta.length_squared() > MAX_MOVE_ENTITY_DISPLACEMENT.powi(2) {
            warn!(
                "MOVE_ENTITY: rejecting {:.1} m physics-body jump for api_id={} (limit {:.1} m)",
                delta.length(),
                cmd.entity_id,
                MAX_MOVE_ENTITY_DISPLACEMENT,
            );
            return;
        }
        if bounds
            .as_deref()
            .is_some_and(|world| world.escaped(target_abs))
        {
            warn!(
                "MOVE_ENTITY: rejecting physics-body target outside world bounds for api_id={} at {:?}",
                cmd.entity_id, target_abs,
            );
            return;
        }
    }
    let mut writable = spatial.p1();
    let Ok((mut tf, lin_vel_opt)) = writable.get_mut(target) else {
        warn!(
            "MOVE_ENTITY: entity {:?} (api_id={}) disappeared during move",
            target, cmd.entity_id
        );
        return;
    };
    tf.translation = new_local;
    if let Some(new_cell) = new_cell {
        commands.entity(target).try_insert(new_cell);
    } else if old_cell.is_some() {
        commands.entity(target).try_remove::<CellCoord>();
    }

    // Force the body to Kinematic for the duration of the move so Avian treats
    // the new pose as authoritative. The original kind is stashed on the
    // marker and restored after the one-tick propagation pulse. A repeated
    // move keeps the first captured kind rather than capturing the temporary
    // Kinematic state.
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
    // Active-frame delta: this remains precise and independent of the internal
    // cell split and of any translating/rotating celestial ancestors.
    if let Some(mut lin_vel) = lin_vel_opt {
        lin_vel.0 = delta / dt;
    }
    commands.entity(target).try_insert(JustMovedKinematic {
        restore,
        angular_pulse: false,
    });

    info!(
        "MOVE_ENTITY: {:?} → ({:.3}, {:.3}, {:.3})",
        cmd.entity_id, cmd.translation[0], cmd.translation[1], cmd.translation[2]
    );
}

/// Set an entity's world ORIENTATION — the rotational twin of [`MoveEntity`].
///
/// Reachable as `cmd("RotateEntity", #{entity_id, rotation: [x, y, z, w]})`, or
/// `set_world_rotation(id, q)` from the rhai prelude. The quaternion is the same
/// `[x, y, z, w]` form `world_rotation(id)` returns and `qrot` consumes, so a
/// script can read an orientation, transform it, and write it back without ever
/// converting representation.
///
/// The public quaternion is expressed in [`lunco_core::ActivePhysicsFrame`], the
/// same semantic frame as `MoveEntity`. Rotation is not frame-invariant: a
/// rotating body Grid and a rotated assembly parent both change the local
/// quaternion that must be stored on the entity. The observer performs that
/// hierarchy conversion once.
///
/// Written through `Transform`, never through avian's `Rotation`, for exactly
/// the reason `MoveEntity` never hand-writes `Position`:
/// `BigSpacePhysicsBridgePlugin::pose_to_position` fires on the external
/// `Transform` write and derives the physics pose from it (carrying it to
/// jointed descendants); a hand-written `Rotation` is a second, wronger opinion
/// that the bridge's writeback then undoes. The body is pinned Kinematic for the
/// move, as `MoveEntity` does, so the solver treats the new pose as
/// authoritative rather than fighting it. When `AngularVelocity` is present,
/// the live handler also publishes a bounded one-tick angular pulse so jointed
/// bodies receive the rotation; cleanup clears it after the physics step.
#[Command(default)]
pub struct RotateEntity {
    /// API-stable global entity ID from `ListEntities`, resolved to the live
    /// Bevy entity by `ApiEntityRegistry`.
    pub entity_id: u64,
    /// Target world orientation as `[x, y, z, w]`. Normalised on arrival — a
    /// quaternion that has been interpolated or sampled is unit only to float
    /// tolerance, and refusing it would make this fail for poses that are
    /// perfectly usable. A degenerate (near-zero) quaternion IS refused: it
    /// names no orientation, and silently substituting identity would spin the
    /// body to an attitude the caller never asked for.
    pub rotation: [f64; 4],
}

/// Observer for `RotateEntity`.
#[on_command(RotateEntity)]
pub fn on_rotate_entity_command(
    trigger: On<RotateEntity>,
    time: Res<Time>,
    registry: Res<lunco_api::registry::ApiEntityRegistry>,
    active_frame: Res<lunco_core::ActivePhysicsFrame>,
    mut commands: Commands,
    mut spatial: ParamSet<(
        Query<(Option<&CellCoord>, &Transform)>,
        Query<(&mut Transform, Option<&mut AngularVelocity>)>,
    )>,
    q_parents: Query<&ChildOf>,
    q_grids: Query<&Grid>,
    q_rb: Query<&RigidBody>,
    q_marker: Query<&JustMovedKinematic>,
) {
    let cmd = trigger.event();
    let global_id = lunco_core::GlobalEntityId::from_raw(cmd.entity_id);
    let Some(target) = registry.resolve(&global_id) else {
        warn!("ROTATE_ENTITY: no api_id={} in registry", cmd.entity_id);
        return;
    };
    let q_in = DQuat::from_array(cmd.rotation);
    if !q_in.is_finite() || q_in.length_squared() < 1e-12 {
        warn!(
            "ROTATE_ENTITY: {:?} (api_id={}) given a degenerate quaternion {:?} — \
             refusing rather than substituting identity",
            target, cmd.entity_id, cmd.rotation
        );
        return;
    }
    let q_in = q_in.normalize();
    let (previous_rotation, local_rotation) = {
        let q_spatial = spatial.p0();
        let Some((_, previous_rotation)) = lunco_core::coords::pose_in_grid(
            target,
            active_frame.0,
            &q_parents,
            &q_grids,
            &q_spatial,
        ) else {
            warn!(
                ?target,
                active_frame = ?active_frame.0,
                "ROTATE_ENTITY: entity is not connected to the active physics frame"
            );
            return;
        };
        let Some(local_rotation) = lunco_core::coords::rotation_in_grid_to_parent_local(
            target,
            q_in,
            active_frame.0,
            &q_parents,
            &q_grids,
            &q_spatial,
        ) else {
            warn!(
                ?target,
                active_frame = ?active_frame.0,
                "ROTATE_ENTITY: entity is not connected to the active physics frame"
            );
            return;
        };
        (previous_rotation, local_rotation)
    };
    let mut writable = spatial.p1();
    let Ok((mut tf, angular_velocity)) = writable.get_mut(target) else {
        warn!(
            "ROTATE_ENTITY: entity {:?} (api_id={}) has no Transform",
            target, cmd.entity_id
        );
        return;
    };
    tf.rotation = local_rotation.as_quat();
    if let Some(mut angular_velocity) = angular_velocity {
        let dt = time.delta_secs().max(1.0 / 240.0) as f64;
        angular_velocity.0 = (shortest_angular_delta(previous_rotation, q_in) / dt)
            .clamp_length_max(lunco_physics::MAX_KINEMATIC_DRIVE_SPEED);
    }

    // Same Kinematic pin as `MoveEntity`: an authored pose on a Dynamic body is
    // otherwise just an initial condition the solver immediately argues with.
    // `restore` remembers what to put back, and prefers an existing marker's
    // value so two writes in one frame don't latch Kinematic permanently.
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
        commands.entity(target).try_insert(JustMovedKinematic {
            restore,
            angular_pulse: true,
        });
    }

    info!(
        "ROTATE_ENTITY: {:?} → [{:.3}, {:.3}, {:.3}, {:.3}]",
        cmd.entity_id, cmd.rotation[0], cmd.rotation[1], cmd.rotation[2], cmd.rotation[3]
    );
}

/// Set an entity's complete active-frame pose as one scene edit.
///
/// This is the compound counterpart to [`MoveEntity`] and [`RotateEntity`].
/// Interactive editors use it when translation and rotation are produced by
/// one gesture, so live seating and document persistence share one semantic
/// command and one undo/change-set boundary.
/// For physics bodies, the live handler publishes bounded one-tick linear and
/// angular pulses when the corresponding Avian components are present, allowing
/// joint constraints to consume the complete pose edit before cleanup.
#[Command(default)]
pub struct TransformEntity {
    /// API-stable global entity ID from `ListEntities`, resolved to the live
    /// Bevy entity by `ApiEntityRegistry`.
    pub entity_id: u64,
    /// Target translation in the explicit active physics frame.
    pub translation: [f64; 3],
    /// Target orientation in the explicit active physics frame, `[x,y,z,w]`.
    pub rotation: [f64; 4],
}

/// Live observer for [`TransformEntity`].
#[on_command(TransformEntity)]
pub fn on_transform_entity_command(
    trigger: On<TransformEntity>,
    time: Res<Time>,
    registry: Res<lunco_api::registry::ApiEntityRegistry>,
    active_frame: Res<lunco_core::ActivePhysicsFrame>,
    mut commands: Commands,
    mut spatial: ParamSet<(
        Query<(Option<&CellCoord>, &Transform)>,
        Query<(
            &mut Transform,
            Option<&mut LinearVelocity>,
            Option<&mut AngularVelocity>,
        )>,
    )>,
    q_parents: Query<&ChildOf>,
    q_grids: Query<&Grid>,
    q_rb: Query<&RigidBody>,
    q_marker: Query<&JustMovedKinematic>,
    bounds: Option<Res<lunco_physics::WorldBounds>>,
) {
    let cmd = trigger.event();
    if cmd.translation.iter().any(|value| !value.is_finite()) {
        warn!(
            "TRANSFORM_ENTITY: rejecting non-finite translation for api_id={}",
            cmd.entity_id
        );
        return;
    }
    let rotation = DQuat::from_array(cmd.rotation);
    if !rotation.is_finite() || rotation.length_squared() < 1.0e-12 {
        warn!(
            "TRANSFORM_ENTITY: rejecting degenerate rotation for api_id={}",
            cmd.entity_id
        );
        return;
    }
    let global_id = lunco_core::GlobalEntityId::from_raw(cmd.entity_id);
    let Some(target) = registry.resolve(&global_id) else {
        warn!("TRANSFORM_ENTITY: no api_id={}", cmd.entity_id);
        return;
    };
    let target_position = DVec3::from_array(cmd.translation);
    if q_grids.get(active_frame.0).is_err() {
        warn!(
            active_frame = ?active_frame.0,
            "TRANSFORM_ENTITY: active physics frame is not a BigSpace Grid"
        );
        return;
    }

    let (previous_position, previous_rotation, old_cell, new_cell, new_translation, new_rotation) = {
        let q_spatial = spatial.p0();
        let Ok((old_cell, _)) = q_spatial.get(target) else {
            warn!("TRANSFORM_ENTITY: entity {:?} has no Transform", target);
            return;
        };
        let Some((previous_position, previous_rotation)) = lunco_core::coords::pose_in_grid(
            target,
            active_frame.0,
            &q_parents,
            &q_grids,
            &q_spatial,
        ) else {
            warn!(
                ?target,
                active_frame = ?active_frame.0,
                "TRANSFORM_ENTITY: entity is not connected to the active physics frame"
            );
            return;
        };
        let Some((new_cell, new_translation)) =
            lunco_core::coords::position_in_grid_to_parent_local(
                target,
                target_position,
                active_frame.0,
                &q_parents,
                &q_grids,
                &q_spatial,
            )
        else {
            warn!(
                ?target,
                active_frame = ?active_frame.0,
                "TRANSFORM_ENTITY: cannot express translation in the entity parent frame"
            );
            return;
        };
        let Some(new_rotation) = lunco_core::coords::rotation_in_grid_to_parent_local(
            target,
            rotation.normalize(),
            active_frame.0,
            &q_parents,
            &q_grids,
            &q_spatial,
        ) else {
            warn!(
                ?target,
                active_frame = ?active_frame.0,
                "TRANSFORM_ENTITY: cannot express rotation in the entity parent frame"
            );
            return;
        };
        (
            previous_position,
            previous_rotation,
            old_cell.copied(),
            new_cell,
            new_translation,
            new_rotation,
        )
    };

    let delta = target_position - previous_position;
    let physics_body = q_rb
        .get(target)
        .is_ok_and(|rb| !matches!(rb, RigidBody::Static));
    if physics_body {
        if delta.length_squared() > MAX_MOVE_ENTITY_DISPLACEMENT.powi(2) {
            warn!(
                "TRANSFORM_ENTITY: rejecting {:.1} m physics-body jump for api_id={} (limit {:.1} m)",
                delta.length(),
                cmd.entity_id,
                MAX_MOVE_ENTITY_DISPLACEMENT,
            );
            return;
        }
        if bounds
            .as_deref()
            .is_some_and(|world| world.escaped(target_position))
        {
            warn!(
                "TRANSFORM_ENTITY: rejecting physics-body target outside world bounds for api_id={} at {:?}",
                cmd.entity_id, target_position,
            );
            return;
        }
    }

    {
        let mut writable = spatial.p1();
        let Ok((mut tf, lin_vel_opt, angular_vel_opt)) = writable.get_mut(target) else {
            warn!(
                "TRANSFORM_ENTITY: entity {:?} disappeared during move",
                target
            );
            return;
        };
        tf.translation = new_translation;
        tf.rotation = new_rotation.as_quat();
        if let Some(mut lin_vel) = lin_vel_opt {
            let dt = time.delta_secs().max(1.0 / 240.0) as f64;
            lin_vel.0 = delta / dt;
        }
        if let Some(mut angular_vel) = angular_vel_opt {
            let dt = time.delta_secs().max(1.0 / 240.0) as f64;
            angular_vel.0 = (shortest_angular_delta(previous_rotation, rotation.normalize()) / dt)
                .clamp_length_max(lunco_physics::MAX_KINEMATIC_DRIVE_SPEED);
        }
    }
    match (new_cell, old_cell) {
        (Some(cell), _) => {
            commands.entity(target).try_insert(cell);
        }
        (None, Some(_)) => {
            commands.entity(target).try_remove::<CellCoord>();
        }
        (None, None) => {}
    }

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
        commands.entity(target).try_insert(JustMovedKinematic {
            restore,
            angular_pulse: true,
        });
    }

    // The transform write above is the storage authority. The BigSpace bridge
    // derives Avian Position/Rotation from it; never create a second Position
    // writer here.
    info!(
        "TRANSFORM_ENTITY: {:?} → position={:?}, rotation={:?}",
        cmd.entity_id, target_position, rotation
    );
}

/// Persist [`TransformEntity`] as one runtime-layer USD change set.
pub fn persist_transform_to_runtime_layer(
    trigger: On<TransformEntity>,
    api_registry: Res<lunco_api::registry::ApiEntityRegistry>,
    active_frame: Res<lunco_core::ActivePhysicsFrame>,
    usd_registry: Res<DocumentRegistry<UsdDocument>>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    q_prim: Query<&UsdPrimPath>,
    q_parents: Query<&ChildOf>,
    q_grids: Query<&Grid>,
    q_spatial: Query<(Option<&CellCoord>, &Transform)>,
    mut commands: Commands,
) {
    let cmd = trigger.event();
    let global_id = lunco_core::GlobalEntityId::from_raw(cmd.entity_id);
    let Some(target) = api_registry.resolve(&global_id) else {
        return;
    };
    let Some((doc, path)) = authorable_prim(target, &q_prim, &usd_registry, workspace.as_deref())
    else {
        return;
    };
    let Some((cell, local_translation)) = lunco_core::coords::position_in_grid_to_parent_local(
        target,
        DVec3::from_array(cmd.translation),
        active_frame.0,
        &q_parents,
        &q_grids,
        &q_spatial,
    ) else {
        warn!(
            ?target,
            active_frame = ?active_frame.0,
            "TRANSFORM_ENTITY: authored entity is disconnected; not persisting"
        );
        return;
    };
    let Some(local_rotation) = lunco_core::coords::rotation_in_grid_to_parent_local(
        target,
        DQuat::from_array(cmd.rotation).normalize(),
        active_frame.0,
        &q_parents,
        &q_grids,
        &q_spatial,
    ) else {
        return;
    };
    let Some(parent) = q_parents.get(target).ok().map(ChildOf::parent) else {
        return;
    };
    let authored_translation = match (cell, q_grids.get(parent).ok()) {
        (Some(cell), Some(grid)) => {
            grid.grid_position_double(&cell, &Transform::from_translation(local_translation))
        }
        (None, None) => local_translation.as_dvec3(),
        _ => {
            warn!(
                ?target,
                ?parent,
                "TRANSFORM_ENTITY: inconsistent Grid storage; not persisting"
            );
            return;
        }
    };
    let (rx, ry, rz) = local_rotation.to_euler(EulerRot::XYZ);
    commands.trigger(ApplyUsdOps {
        doc,
        label: "Transform entity".to_string(),
        ops: vec![
            UsdOp::SetTranslate {
                edit_target: LayerId::runtime(),
                path: path.clone(),
                value: authored_translation.to_array(),
            },
            UsdOp::SetRotate {
                edit_target: LayerId::runtime(),
                path,
                value: [rx.to_degrees(), ry.to_degrees(), rz.to_degrees()],
            },
        ],
    });
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
    active_frame: Res<lunco_core::ActivePhysicsFrame>,
    usd_registry: Res<DocumentRegistry<UsdDocument>>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    q_prim: Query<&UsdPrimPath>,
    q_parents: Query<&ChildOf>,
    q_grids: Query<&Grid>,
    q_spatial: Query<(Option<&CellCoord>, &Transform)>,
    mut commands: Commands,
) {
    let cmd = trigger.event();
    let global_id = lunco_core::GlobalEntityId::from_raw(cmd.entity_id);
    let Some(target) = api_registry.resolve(&global_id) else {
        return;
    };
    let Some((doc, path)) = authorable_prim(target, &q_prim, &usd_registry, workspace.as_deref())
    else {
        return;
    };

    let Some((cell, local)) = lunco_core::coords::position_in_grid_to_parent_local(
        target,
        DVec3::from_array(cmd.translation),
        active_frame.0,
        &q_parents,
        &q_grids,
        &q_spatial,
    ) else {
        warn!(
            ?target,
            active_frame = ?active_frame.0,
            "MOVE_ENTITY: authored entity is disconnected from the active physics frame; not persisting an ambiguous transform"
        );
        return;
    };
    // USD xformOps are authored relative to the prim's parent. A direct Grid
    // child is internally split by BigSpace, so reassemble that one parent-local
    // f64 value before authoring; a plain parent already returned its local value.
    let parent = q_parents
        .get(target)
        .expect("coordinate conversion proved that the parent exists")
        .parent();
    let authored = match (cell, q_grids.get(parent).ok()) {
        (Some(cell), Some(grid)) => {
            grid.grid_position_double(&cell, &Transform::from_translation(local))
        }
        (None, None) => local.as_dvec3(),
        _ => {
            error!(
                ?target,
                ?parent,
                "MOVE_ENTITY: coordinate conversion returned inconsistent Grid storage"
            );
            return;
        }
    };
    commands.trigger(ApplyUsdOp {
        doc,
        op: UsdOp::SetTranslate {
            edit_target: LayerId::runtime(),
            path,
            value: authored.to_array(),
        },
    });
}

// ─────────────────────────────────────────────────────────────────────
/// Persist an active-frame orientation using the same parent-local conversion
/// as [`on_rotate_entity_command`]. USD owns the authored local xform; the
/// active BigSpace grid is a runtime semantic frame and must never leak into
/// that stored value.
pub fn persist_rotation_to_runtime_layer(
    trigger: On<RotateEntity>,
    api_registry: Res<lunco_api::registry::ApiEntityRegistry>,
    active_frame: Res<lunco_core::ActivePhysicsFrame>,
    usd_registry: Res<DocumentRegistry<UsdDocument>>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    q_prim: Query<&UsdPrimPath>,
    q_parents: Query<&ChildOf>,
    q_grids: Query<&Grid>,
    q_spatial: Query<(Option<&CellCoord>, &Transform)>,
    mut commands: Commands,
) {
    let cmd = trigger.event();
    let global_id = lunco_core::GlobalEntityId::from_raw(cmd.entity_id);
    let Some(target) = api_registry.resolve(&global_id) else {
        return;
    };
    let Some((doc, path)) = authorable_prim(target, &q_prim, &usd_registry, workspace.as_deref())
    else {
        return;
    };
    let requested = DQuat::from_array(cmd.rotation);
    if !requested.is_finite() || requested.length_squared() < 1.0e-12 {
        return;
    }
    let Some(local) = lunco_core::coords::rotation_in_grid_to_parent_local(
        target,
        requested.normalize(),
        active_frame.0,
        &q_parents,
        &q_grids,
        &q_spatial,
    ) else {
        warn!(
            ?target,
            active_frame = ?active_frame.0,
            "ROTATE_ENTITY: authored entity is disconnected from the active physics frame; not persisting an ambiguous transform"
        );
        return;
    };
    let (rx, ry, rz) = local.to_euler(EulerRot::XYZ);
    commands.trigger(ApplyUsdOp {
        doc,
        op: UsdOp::SetRotate {
            edit_target: LayerId::runtime(),
            path,
            value: [rx.to_degrees(), ry.to_degrees(), rz.to_degrees()],
        },
    });
}

// Document history — THE history
//
// The 3D editor has no private undo stack. Every editor mutation is
// authored as a `UsdOp` (the persisters above), so its history is the
// document's history: Lamport-ordered, op+inverse, journaled, networked.
// `UndoDocument`/`RedoDocument` are the generic verbs; each domain observes them
// and acts only on documents its own registry owns. USD's observers live in
// `lunco-usd` (the crate that owns `DocumentRegistry<UsdDocument>`) — NOT here, so that a
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
/// Shared resolution for document-backed scene edits. Callers use the returned
/// document and prim path to author the corresponding USD operation.
///
/// Returns `None` when there is no active USD document (headless, a Modelica doc, no
/// scene), when the entity is not USD-backed, or when its prim belongs to some other
/// document.
pub fn authorable_prim(
    entity: Entity,
    q_prim: &Query<&UsdPrimPath>,
    usd_registry: &DocumentRegistry<UsdDocument>,
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

/// A generic delete may not remove a mounted component. The mount command owns
/// the coordinated removal of its component, exact joint, and socket
/// occupancy; this guard keeps the generic entity verb from bypassing that
/// invariant. It deliberately checks the applied schema, not a path spelling.
fn is_mount_component(
    registry: &DocumentRegistry<UsdDocument>,
    doc: lunco_doc::DocumentId,
    path: &str,
) -> bool {
    let Ok(path) = lunco_usd_bevy::SdfPath::new(path) else {
        return false;
    };
    registry.host(doc).is_some_and(|host| {
        let composed = host.document().composed();
        lunco_usd_bevy::has_api_schema(&composed, &path, "LunCoMountAttachmentAPI")
    })
}

// ─────────────────────────────────────────────────────────────────────
// DeleteEntity — removal, authored
// ─────────────────────────────────────────────────────────────────────

/// Delete an entity from the scene.
///
/// The typed verb for "remove this" authors a `RemovePrim` in the backing
/// document, so deletion is journaled, replicated, persisted, and undoable.
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
    usd_registry: Option<Res<DocumentRegistry<UsdDocument>>>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    q_prim: Query<&UsdPrimPath>,
    mut commands: Commands,
) {
    let cmd = trigger.event();
    if let Some(registry) = usd_registry.as_deref() {
        let attachment = authorable_prim(cmd.target, &q_prim, registry, workspace.as_deref());
        if let Some((doc, path)) = attachment {
            if is_mount_component(registry, doc, &path) {
                warn!(
                    "DELETE_ENTITY rejected for attached component {}; use DetachComponent",
                    path
                );
                return;
            }
        }
    }
    commands.entity(cmd.target).try_despawn();
    selected.entities.retain(|e| *e != cmd.target);
}

/// Authoring leg: remove the prim, so the deletion persists, journals, replicates —
/// and undoes. Same shape as every other `persist_*` observer.
pub fn persist_delete_to_runtime_layer(
    trigger: On<DeleteEntity>,
    usd_registry: Res<DocumentRegistry<UsdDocument>>,
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
    if is_mount_component(&usd_registry, doc, &path) {
        warn!(
            "DELETE_ENTITY persistence rejected for attached component {}; use DetachComponent",
            path
        );
        return;
    }
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
    usd_registry: Res<DocumentRegistry<UsdDocument>>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    q_prim: Query<&UsdPrimPath>,
    q_shader: Query<(), With<ShaderLook>>,
    mut commands: Commands,
) {
    let cmd = trigger.event();
    // Not shader *params*: `shader` swaps the material (no USD reader — the
    // `shaderPath` attribute was deliberately vetoed, so it stays live-only) and
    // `visible` is authored as standard `token visibility` by
    // [`persist_wheel_to_runtime_layer`]. Disjoint, so neither is
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
    let floats: Vec<f32> = parts
        .iter()
        .filter_map(|s| s.trim().parse::<f32>().ok())
        .collect();
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
    let Some(doc) = workspace.0.active_document else {
        return;
    };
    let Some(host) = usd_registry.host(doc) else {
        return;
    };

    let global_id = lunco_core::GlobalEntityId::from_raw(cmd.entity_id);
    let Some(target) = api_registry.resolve(&global_id) else {
        return;
    };
    // Only shader-look prims (the layer-tuning case) — not PBR ones.
    if q_shader.get(target).is_err() {
        return;
    }
    let Ok(prim) = q_prim.get(target) else { return };

    // Ownership guard: only author for prims the active document actually holds
    // (base or runtime), so palette/sim spawns never get stray opinions.
    let Ok(prim_sdf) = lunco_usd_bevy::SdfPath::new(&prim.path) else {
        return;
    };
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

fn default_float_type() -> String {
    "float".to_string()
}

/// Author a native USD attribute connection (`connectionPaths`) onto a prim.
#[Command]
pub struct SetUsdConnection {
    /// Target entity or prim root.
    pub target: Entity,
    /// Attribute name (e.g. `inputs:angle` or `inputs:earth_azimuth`).
    pub name: String,
    /// Attribute type name (e.g. `float`). Defaults to `float`.
    #[serde(default = "default_float_type")]
    #[reflect(default)]
    pub type_name: String,
    /// Absolute property paths this attribute connects to (e.g. `["/SandboxScene/Skid_Raycast_1/Comms/EarthTrackerController.outputs:az"]`).
    pub sources: Vec<String>,
}

#[on_command(SetUsdConnection)]
pub fn on_set_usd_connection(
    trigger: On<SetUsdConnection>,
    usd_registry: Res<DocumentRegistry<UsdDocument>>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    q_prim: Query<&UsdPrimPath>,
    mut commands: Commands,
) {
    let cmd = trigger.event();
    let Some((doc, path)) =
        authorable_prim(cmd.target, &q_prim, &usd_registry, workspace.as_deref())
    else {
        return;
    };
    commands.trigger(ApplyUsdOp {
        doc,
        op: UsdOp::SetConnection {
            edit_target: LayerId::runtime(),
            path,
            name: cmd.name.clone(),
            type_name: if cmd.type_name.is_empty() {
                "float".to_string()
            } else {
                cmd.type_name.clone()
            },
            sources: cmd.sources.clone(),
        },
    });
}

/// Author one standard USD attribute in the active document's runtime layer.
///
/// This is the generic authoring verb for data-driven editor tools. It does not
/// add a LunCo schema or mutate an ECS component: the USD type and literal are
/// passed to the document's typed `UsdOp::SetAttribute` path, so composed USD
/// remains the source of truth. A tool such as `nurbs.rhai` can therefore edit
/// `point3f[] points` without a Rust handler for every geometry type.
#[Command(default)]
pub struct SetUsdAttribute {
    /// Absolute USD prim path owned by the active document.
    pub path: String,
    /// Attribute name, for example `points` or `inputs:radius`.
    pub name: String,
    /// USD type name, for example `point3f[]`, `float`, or `token`.
    pub type_name: String,
    /// USD literal, exactly as it would appear in USDA (except `string`, which
    /// is raw content according to `UsdOp::SetAttribute`'s contract).
    pub value: String,
}

#[on_command(SetUsdAttribute)]
pub fn on_set_usd_attribute(
    trigger: On<SetUsdAttribute>,
    usd_registry: Res<DocumentRegistry<UsdDocument>>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    mut commands: Commands,
) {
    let cmd = trigger.event();
    if cmd.path.is_empty() || cmd.name.is_empty() || cmd.type_name.is_empty() {
        warn!("SET_USD_ATTRIBUTE: path, name, and type_name are required");
        return;
    }
    let Ok(path) = lunco_usd_bevy::SdfPath::new(&cmd.path) else {
        warn!("SET_USD_ATTRIBUTE: invalid prim path `{}`", cmd.path);
        return;
    };
    let Some(doc) = workspace.as_deref().and_then(|ws| ws.0.active_document) else {
        debug!("SET_USD_ATTRIBUTE: no active document");
        return;
    };
    let Some(host) = usd_registry.host(doc) else {
        warn!("SET_USD_ATTRIBUTE: active document {doc} is unavailable");
        return;
    };
    if host.document().data().spec(&path).is_none()
        && host.document().runtime_data().spec(&path).is_none()
    {
        warn!(
            "SET_USD_ATTRIBUTE: prim `{}` is not owned by the active document",
            cmd.path
        );
        return;
    }
    commands.trigger(ApplyUsdOp {
        doc,
        op: UsdOp::SetAttribute {
            edit_target: LayerId::runtime(),
            path: cmd.path.clone(),
            name: cmd.name.clone(),
            type_name: cmd.type_name.clone(),
            value: cmd.value.clone(),
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
    /// The single public `SetObjectProperty` name for this parameter.
    pub name: &'static str,
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
        name: "brake_torque",
        set: |w, v| w.brake_torque_max = v,
        usd_attr: "physxVehicleWheel:maxBrakeTorque",
    },
    WheelParam {
        name: "slip_stiffness",
        set: |w, v| w.slip_stiffness = v,
        usd_attr: "physxVehicleTire:longitudinalStiffness",
    },
    WheelParam {
        name: "bearing_damping",
        set: |w, v| w.bearing_damping = v,
        usd_attr: "physxVehicleWheel:dampingRate",
    },
    WheelParam {
        name: "friction_mu",
        set: |w, v| w.friction_mu = v,
        usd_attr: "physics:dynamicFriction",
    },
    WheelParam {
        name: "mass",
        set: |w, v| w.mass = v,
        usd_attr: "physxVehicleWheel:mass",
    },
    WheelParam {
        name: "moi",
        set: |w, v| w.moment_of_inertia = v,
        usd_attr: "physxVehicleWheel:moi",
    },
    WheelParam {
        name: "wheel_radius",
        set: |w, v| w.wheel_radius = v,
        usd_attr: "physxVehicleWheel:radius",
    },
];

/// Look a canonical `SetObjectProperty` name up in [`WHEEL_PARAMS`], or `None`
/// if it isn't a wheel field. Both the live-mutation path and the USD-authoring
/// path go through this one lookup.
pub(crate) fn wheel_param(name: &str) -> Option<&'static WheelParam> {
    WHEEL_PARAMS.iter().find(|p| p.name == name)
}

/// Persist a `SetObjectProperty` **wheel-dynamics** or **visibility** tune into
/// the active USD document's runtime overlay — the
/// counterpart of [`persist_property_to_runtime_layer`] for the property classes
/// it skips. Fully decoupled + disjoint: it authors wheel-param names (via
/// [`wheel_param`]) or `visible` (standard USD `token visibility`). PBR intent
/// is authored by the command handler through the canonical UsdPreviewSurface
/// path below; keeping it out of this observer avoids two USD representations for
/// one property.
pub fn persist_wheel_to_runtime_layer(
    trigger: On<SetObjectProperty>,
    api_registry: Res<lunco_api::registry::ApiEntityRegistry>,
    usd_registry: Res<DocumentRegistry<UsdDocument>>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    q_prim: Query<&UsdPrimPath>,
    mut commands: Commands,
) {
    let cmd = trigger.event();

    // Route the property to a USD attribute the loader reads back.
    let authored: Option<(String, &str, String)> = if let Some(param) = wheel_param(&cmd.property) {
        // Wheel dynamics → the single `WHEEL_PARAMS` row's USD attribute.
        cmd.value
            .trim()
            .parse::<f32>()
            .ok()
            .map(|v| (param.usd_attr.to_string(), "float", v.to_string()))
    } else if matches!(
        cmd.property.as_str(),
        "rest_length" | "spring_k" | "damping_c"
    ) {
        // `springStrength` / `springDamperRate` are NVIDIA's canonical
        // PhysxVehicleSuspensionAPI names; `restLength` has no PhysX
        // equivalent, so it lives under the lunco: namespace.
        let usd_attr = match cmd.property.as_str() {
            "rest_length" => "lunco:suspension:restLength",
            "spring_k" => "physxVehicleSuspension:springStrength",
            "damping_c" => "physxVehicleSuspension:springDamperRate",
            _ => unreachable!(),
        };
        cmd.value
            .trim()
            .parse::<f32>()
            .ok()
            .map(|v| (usd_attr.to_string(), "float", v.to_string()))
    } else if cmd.property == "visible" {
        // Visibility → standard USD `token visibility`, which the prim
        // instantiator already reads back (`inherited` / `invisible`), so a
        // hide survives reload instead of being a live-only ECS `Visibility`
        // write. A `token` literal is QUOTED in USD.
        let hidden = matches!(cmd.value.trim(), "false" | "0" | "hidden");
        let tok = if hidden { "invisible" } else { "inherited" };
        Some(("visibility".to_string(), "token", format!("\"{tok}\"")))
    } else {
        None
    };
    let Some((name, type_name, value)) = authored else {
        return;
    };

    let Some(workspace) = workspace else { return };
    let Some(doc) = workspace.0.active_document else {
        return;
    };
    let Some(host) = usd_registry.host(doc) else {
        return;
    };
    let global_id = lunco_core::GlobalEntityId::from_raw(cmd.entity_id);
    let Some(target) = api_registry.resolve(&global_id) else {
        return;
    };
    let Ok(prim) = q_prim.get(target) else { return };

    let Ok(prim_sdf) = lunco_usd_bevy::SdfPath::new(&prim.path) else {
        return;
    };
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
/// Scope: the fields with an existing loader reader. The render-only knobs have
/// no `DistantLight` attribute that reads them back, so they persist elsewhere —
/// exposure / bloom / earthshine onto the `LunCoEnvironment` settings prim, and
/// **ambient** onto a dedicated untextured `DomeLight` (`Environment/AmbientFill`),
/// which is the standard USD spelling of uniform environment illumination and the
/// only thing `GlobalAmbientLight` is composed from. Because that composition is a
/// SUM over domes, the authored intensity is solved (`requested − other domes`)
/// rather than assigned; see the ambient block at the end of this function.
///
/// Targets every non-earthshine `DistantLight` the active document owns
/// (`SetEnvironmentLight` itself is global). Ownership-guarded like the other
/// persisters; no-op when no USD doc is active (headless).
pub fn persist_environment_light_to_runtime_layer(
    trigger: On<lunco_environment::SetEnvironmentLight>,
    usd_registry: Res<DocumentRegistry<UsdDocument>>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    q_sun: Query<
        (&UsdPrimPath, &Transform),
        (
            With<lunco_usd_bevy::UsdAuthoredLight>,
            With<DirectionalLight>,
            Without<lunco_environment::Earthshine>,
        ),
    >,
    // The body fill, which now has a prim of its own to be written to — the
    // exact complement of `q_sun`, so no light is addressed twice.
    q_earthshine: Query<
        &UsdPrimPath,
        (
            With<lunco_usd_bevy::UsdAuthoredLight>,
            With<DirectionalLight>,
            With<lunco_environment::Earthshine>,
        ),
    >,
    mut commands: Commands,
) {
    let cmd = trigger.event();
    let Some(workspace) = workspace else { return };
    let Some(doc) = workspace.0.active_document else {
        return;
    };
    let Some(host) = usd_registry.host(doc) else {
        return;
    };

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
        // Standard `UsdLuxShadowAPI`, not an invented `lunco:` name.
        attrs.push(("inputs:shadow:distance", "float", v.to_string()));
    }
    if let Some(v) = cmd.shadow_first_cascade_bound {
        // The one renderer-specific knob: cascaded shadow maps are a rasterizer
        // technique UsdLux has no attribute for.
        attrs.push(("lunco:shadow:firstCascadeFarBound", "float", v.to_string()));
    }
    // Direction changes when yaw or pitch is specified.
    let direction_changed = cmd.sun_yaw.is_some() || cmd.sun_pitch.is_some();
    if attrs.is_empty() && !direction_changed {
        return;
    }

    let parent_path = lunco_usd_bevy::layer_default_prim(host.document().data())
        .map(|p| format!("/{p}"))
        .unwrap_or_else(|| "/".to_string());
    let env_path = if parent_path == "/" {
        "/Environment".to_string()
    } else {
        format!("{parent_path}/Environment")
    };
    // Resolve the ambient solve before queuing any other edits. A malformed
    // authored DomeLight must reject the whole command rather than allowing the
    // unrelated sun/environment edits through while silently fabricating a
    // different ambient value.
    let ambient_plan = if let Some(requested) = cmd.ambient_brightness {
        let fill_path = format!("{env_path}/AmbientFill");
        let composed = host.document().composed_arc();
        let fill_sdf = lunco_usd_bevy::SdfPath::new(&fill_path).ok();
        match lunco_usd_bevy::untextured_dome_intensity_sum(&composed, fill_sdf.as_ref()) {
            Ok(others) => Some((requested, others, fill_path)),
            Err(_) => {
                error!(
                    "[scene-commands] refusing environment update: authored DomeLight \
                     intensity, exposure, or texture data is malformed or unresolved"
                );
                return;
            }
        }
    } else {
        None
    };

    for (prim, tf) in &q_sun {
        // Ownership guard: only author for suns the active document actually
        // holds (base or runtime), so an unowned runtime entity never gets opinions.
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

    // ── Earthshine TINT → its own `DistantLight` prim, standard UsdLux ───────
    //
    // Same treatment as the sun above, because it is the same kind of thing: a
    // light with an authored prim, whose tint is `inputs:color`. The loader
    // reads it back, so the round trip needs no second spelling.
    //
    // Its `inputs:intensity` is NOT persisted. That value is derived from
    // Earth's phase every frame by `drive_earthshine_from_phase`, so authoring
    // it would journal a number the next frame overwrites — a persisted
    // opinion that never survives contact with its own driver.
    //
    // Empty when the scene declares no body to reflect from — a scene gets the
    // fill by declaring the body, so there is nothing to write and nothing to
    // invent. `on_set_environment_light` is where that is reported.
    let mut fill_attrs: Vec<(&str, &str, String)> = Vec::new();
    if let Some([r, g, b]) = cmd.earthshine_color {
        fill_attrs.push(("inputs:color", "color3f", format!("({r}, {g}, {b})")));
    }
    if !fill_attrs.is_empty() {
        for prim in &q_earthshine {
            // Ownership guard, exactly as for the sun: only author onto fills the
            // active document actually holds.
            let Ok(prim_sdf) = lunco_usd_bevy::SdfPath::new(&prim.path) else {
                continue;
            };
            let owned = host.document().data().spec(&prim_sdf).is_some()
                || host.document().runtime_data().spec(&prim_sdf).is_some();
            if !owned {
                continue;
            }
            for (name, type_name, value) in &fill_attrs {
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
        }
    }

    // Render knobs (exposure / bloom / ambient) have no natural
    // light-prim home — they apply to global/camera state — so per the schema
    // decision they persist onto a dedicated `LunCoEnvironment` settings prim
    // (a singleton under the default prim). A projector in `lunco-luncosim` reads
    // them back on stage change and applies them, so the light loader stays pure.
    let mut env_attrs: Vec<(&str, &str, String)> = Vec::new();
    if let Some(v) = cmd.exposure_ev100 {
        env_attrs.push(("lunco:env:exposureEv100", "float", v.to_string()));
    }
    if let Some(v) = cmd.bloom_intensity {
        env_attrs.push(("lunco:env:bloomIntensity", "float", v.to_string()));
    }
    // Ambient is authored through the standard untextured `DomeLight` path
    // below; it is not an environment scalar.
    // Earthshine is not among them: it has a light prim, so it persists onto it
    // like the sun does. See the earthshine block above.
    // Ambient shares the `Environment` scope but NOT the custom-attribute
    // mechanism, so the prim has to be ensured for either reason.
    if env_attrs.is_empty() && cmd.ambient_brightness.is_none() {
        return;
    }

    // Ensure the settings prim exists, but only author `AddPrim` when it's
    // actually absent (else every render tweak would journal a redundant
    // AddPrim). Idempotent thereafter — SetAttribute overwrites in place.
    let prim_missing = |path: &str| {
        !lunco_usd_bevy::SdfPath::new(path)
            .ok()
            .map(|sdf| {
                host.document().data().spec(&sdf).is_some()
                    || host.document().runtime_data().spec(&sdf).is_some()
            })
            .unwrap_or(false)
    };
    if prim_missing(&env_path) {
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

    // ── Ambient → a dedicated untextured `DomeLight`, not a custom attribute ──
    //
    // UsdLux has no "ambient light"; an untextured `DomeLight` is the standard
    // spelling, and `lunco_usd_bevy::light::on_usd_light_added` composes
    // `GlobalAmbientLight::brightness` as the SUM over every such dome. That sum
    // is what the inspector's slider reads back.
    //
    // Hence the subtraction. The slider reads a TOTAL but writes ONE dome, so
    // authoring the requested value verbatim onto `AmbientFill` would ADD to
    // whatever the scene already authors (e.g. a `RegolithBounce` dome at 2600):
    // ask for 50, compose 2650, and the slider jumps to 2650 on the next frame.
    // Authoring `requested - others` makes the composed total land exactly on the
    // request, so the knob is stable under its own feedback.
    if let Some((requested, others, fill_path)) = ambient_plan {
        // Read the composed (base ⊕ runtime) layer data, so a fill dome authored
        // by an earlier drag — which lives only in the runtime overlay — is seen
        // and correctly EXCLUDED from "other domes" rather than subtracted from
        // itself, which would ratchet the value down on every drag.
        let intensity = lunco_usd_bevy::ambient_fill_intensity(requested, others);

        if lunco_usd_bevy::ambient_fill_saturates(requested, others) {
            warn!(
                "[scene-commands] ambient {requested} is below the {others} already \
                 contributed by other authored DomeLights; `{fill_path}` clamped to 0 \
                 and the scene will stay brighter than requested. Lower those domes' \
                 `inputs:intensity` instead."
            );
        }

        if prim_missing(&fill_path) {
            commands.trigger(ApplyUsdOp {
                doc,
                op: UsdOp::AddPrim {
                    edit_target: LayerId::runtime(),
                    parent_path: env_path,
                    name: "AmbientFill".to_string(),
                    type_name: Some("DomeLight".to_string()),
                    reference: None,
                },
            });
        }
        commands.trigger(ApplyUsdOp {
            doc,
            op: UsdOp::SetAttribute {
                edit_target: LayerId::runtime(),
                path: fill_path,
                name: "inputs:intensity".to_string(),
                type_name: "float".to_string(),
                value: intensity.to_string(),
            },
        });
    }
}

/// Marker inserted on a kinematic body that just received a
/// `MoveEntity` (or analogous teleport) with a one-tick velocity pulse.
/// Complete pose/rotation edits also set `angular_pulse`, so the cleanup clears
/// the matching angular velocity after the solver consumes it.
/// [`clear_kinematic_pulse_velocity`] performs that cleanup the frame after the
/// pulse so the body doesn't drift.
#[derive(Component)]
pub struct JustMovedKinematic {
    /// The body kind to put back after the pulse tick — the Kinematic
    /// forced by `on_move_entity_command` is only "for the duration of
    /// the move". `None` = the body was already Kinematic (or has no
    /// RigidBody): restore nothing.
    pub restore: Option<RigidBody>,
    /// Whether the command also published a one-tick angular velocity pulse.
    pub angular_pulse: bool,
}

/// Zeros the `LinearVelocity` and, when requested, `AngularVelocity` of bodies
/// marked with [`JustMovedKinematic`], **after one physics tick has consumed the
/// velocity** for joint propagation.
///
/// Schedule: `FixedPostUpdate`. Bevy's main schedule order is
/// `RunFixedMainLoop` (FixedUpdate cycle) → `Update`. So when a
/// `MoveEntity` observer fires in Frame N's `Update` and sets
/// LinearVelocity + marker, the velocity must persist through the
/// *next* fixed-tick physics step (Frame N+1 `FixedUpdate`) before
/// being zeroed. Running this in `FixedPostUpdate` (which fires
/// after every `FixedUpdate` step) does exactly that:
///
/// - Frame N `Update`: a scene pose command sets velocity + inserts marker.
/// - Frame N+1 `FixedUpdate`: physics runs WITH the velocity;
///   Avian's joint solver sees the kinematic body moving and
///   propagates the motion through joints to coupled dynamic bodies.
/// - Frame N+1 `FixedPostUpdate`: this system runs, zeros velocity,
///   removes marker.
/// - Frame N+2 `FixedUpdate`: physics with velocity = 0; body
///   settled at its new position, no drift.
pub fn clear_kinematic_pulse_velocity(
    mut commands: Commands,
    mut q: Query<(
        Entity,
        Option<&mut LinearVelocity>,
        Option<&mut AngularVelocity>,
        &JustMovedKinematic,
    )>,
) {
    for (e, linear, angular, marker) in q.iter_mut() {
        if let Some(mut linear) = linear {
            linear.0 = DVec3::ZERO;
        }
        if marker.angular_pulse {
            if let Some(mut angular) = angular {
                angular.0 = DVec3::ZERO;
            }
        }
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
/// {"type":"ExecuteCommand","command":"SetObjectProperty",
///  "params":{"entity_id":42,"property":"shader","value":"shaders/balloon.wgsl"}}
/// {"type":"ExecuteCommand","command":"SetObjectProperty",
///  "params":{"entity_id":42,"property":"wedge_count","value":"12"}}
/// {"type":"ExecuteCommand","command":"SetObjectProperty",
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
///   `brake_torque`, `slip_stiffness`, `bearing_damping`, `friction_mu`, `mass`,
///   `moi`, `wheel_radius`, `rest_length`, `spring_k`, `damping_c` → set that
///   `f64` field on the wheel's `WheelRaycast` live. Each wheel is its own entity,
///   so this gives independent per-wheel control. Motor torque and no-load speed
///   are owned by the composed Modelica motor prim; edit its authored
///   `inputs:stall_torque` / `inputs:no_load_speed` attributes instead of
///   addressing a wheel-local drive parameter.
#[Command(default)]
pub struct SetObjectProperty {
    /// API-stable global entity ID from `ListEntities`, resolved to the live
    /// Bevy entity by `ApiEntityRegistry`.
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
/// binder re-materialises the entity. Keeping material ownership in the render
/// binder prevents shared handles from leaking edits between entities and keeps
/// `bevy_pbr` out of the headless command layer.
const PBR_LOOK_KEYS: &[&str] = &[
    "base_color",
    "emissive",
    "metallic",
    "roughness",
    "ior",
    "alpha",
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
            None => {
                let schemas = crate::doc_resolve::geom_api_schemas(world, &prim);
                match lunco_usd::material::ensure_preview_surface_ops(&prim.path, &schemas) {
                    Some((ops, shader)) => (ops, shader, true),
                    None => return,
                }
            }
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
            world.trigger(ApplyUsdOp {
                doc,
                op: op.clone(),
            });
        }
    });
}

/// Whether the edited look key names the same `UsdPreviewSurface` input as `slot`
/// (`roughness` and `alpha` are the canonical command keys).
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
        "roughness" => {
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
        "alpha" => {
            let Some(v) = f.first() else { return false };
            let v = v.clamp(0.0, 1.0);
            look.base_color.alpha = v;
            look.alpha = if v >= 1.0 {
                SurfaceAlpha::Opaque
            } else {
                SurfaceAlpha::Blend
            };
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
/// `None` while the shader is still loading (or if it declares no `Material`) is
/// an unavailable edit target, not permission to infer a type.
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
/// The field's type comes from the shader's reflected schema. Unknown fields,
/// unavailable schemas, engine-owned fields, and malformed component text are
/// rejected; RGB receives the explicit opaque-alpha convention only for a
/// reflected `vec4` field.
fn shader_param_value(schema: Option<&ParamSchema>, key: &str, value: &str) -> Option<ParamValue> {
    let schema = schema?;
    let field = schema.field(key)?;
    if schema.is_engine(key) {
        return None;
    }
    ParamValue::parse_authoring(field.ty, value)
}

/// Give `target` a [`ShaderLook`] for `shader_path`, carrying over any parameters
/// it already has so swapping the `.wgsl` keeps tuned values.
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
    let Some(registry) = world.get_resource::<AppTypeRegistry>().cloned() else {
        return;
    };
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
    mut q_susp: Query<&mut lunco_mobility::Suspension>,
    mut commands: Commands,
) {
    let cmd = trigger.event();
    let global_id = lunco_core::GlobalEntityId::from_raw(cmd.entity_id);
    let Some(target) = registry.resolve(&global_id) else {
        warn!("SET_PROPERTY: no api_id={} in registry", cmd.entity_id);
        return;
    };

    // Per-wheel suspension tuning (both joint-based and raycast).
    match cmd.property.as_str() {
        "rest_length" | "spring_k" | "damping_c" => {
            let Ok(value) = cmd.value.trim().parse::<f64>() else {
                warn!(
                    "SET_PROPERTY: '{}' expects a number, got '{}'",
                    cmd.property, cmd.value
                );
                return;
            };
            let Ok(mut susp) = q_susp.get_mut(target) else {
                warn!(
                    "SET_PROPERTY: entity {} has no Suspension component",
                    cmd.entity_id
                );
                return;
            };
            match cmd.property.as_str() {
                "rest_length" => {
                    susp.rest_length = value;
                }
                "spring_k" => {
                    susp.spring_k = value;
                }
                "damping_c" => {
                    susp.damping_c = value;
                }
                _ => {}
            }
            info!(
                "SET_PROPERTY: suspension {} {} = {}",
                cmd.entity_id, cmd.property, value
            );
            return;
        }
        _ => {}
    }

    // Per-wheel tire-spin dynamics. Each wheel is its own entity, so addressing
    // a single `api_id` sets the field on just that wheel — independent control.
    if let Some(param) = wheel_param(&cmd.property) {
        let Ok(value) = cmd.value.trim().parse::<f64>() else {
            warn!(
                "SET_PROPERTY: '{}' expects a number, got '{}'",
                cmd.property, cmd.value
            );
            return;
        };
        let Ok(mut wheel) = q_wheel.get_mut(target) else {
            warn!("SET_PROPERTY: entity {} has no WheelRaycast", cmd.entity_id);
            return;
        };
        (param.set)(&mut wheel, value);
        info!(
            "SET_PROPERTY: wheel {} {} = {}",
            cmd.entity_id, cmd.property, value
        );
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
                    info!(
                        "SET_PROPERTY: {} look {} = {}",
                        cmd.entity_id, cmd.property, cmd.value
                    );
                } else {
                    warn!(
                        "SET_PROPERTY: bad value '{}' for pbr '{}'",
                        cmd.value, cmd.property
                    );
                }
                return;
            }
            if q_mesh.get(target).is_err() {
                warn!(
                    "SET_PROPERTY: entity {} has no PbrLook / mesh",
                    cmd.entity_id
                );
                return;
            }
            let mut look = PbrLook::default();
            if apply_pbr_look(&mut look, key, &cmd.value) {
                author_look_to_usd(&mut commands, target, key, &look);
                commands.entity(target).try_insert(look);
                info!(
                    "SET_PROPERTY: {} adopted a PbrLook, {} = {}",
                    cmd.entity_id, cmd.property, cmd.value
                );
            } else {
                warn!(
                    "SET_PROPERTY: bad value '{}' for pbr '{}'",
                    cmd.value, cmd.property
                );
            }
        }
        key => {
            // param/color → set the named value on the entity's shader look. The
            // binder swaps in the material for the new look (`Changed<ShaderLook>`).
            let Ok(mut look) = q_shader_look.get_mut(target) else {
                warn!(
                    "SET_PROPERTY: entity {} has no shader look — set 'shader' first",
                    cmd.entity_id
                );
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
    /// API-stable global entity ID from `ListEntities`, resolved to the live
    /// Bevy entity by `ApiEntityRegistry`.
    pub entity_id: u64,
    /// Camera distance from the target, metres. `<= 0` → default 6.
    pub distance: f32,
}

/// Set the render-free runtime focus to the composed USD prim at `path`.
///
/// This is separate from the editor's `SelectEntityByPath`: a headless
/// recorder has no Inspector, gizmo, or picking state to maintain, but
/// runtime-authored surfaces still need a stable subject for scoped telemetry.
/// The authored USD path remains stable across entity ids and scene reloads.
#[Command(default)]
pub struct FocusEntityByPath {
    /// Absolute composed USD prim path (for example `/World/Lander`).
    pub path: String,
}

#[on_command(FocusEntityByPath)]
pub fn on_focus_entity_by_path(
    trigger: On<FocusEntityByPath>,
    q_paths: Query<(Entity, &UsdPrimPath)>,
    mut selected: ResMut<crate::SelectedEntities>,
) {
    let cmd = trigger.event();
    let Some(target) = q_paths
        .iter()
        .find(|(_, prim)| prim.path == cmd.path)
        .map(|(entity, _)| entity)
    else {
        warn!("FOCUS_ENTITY_BY_PATH: no composed prim at `{}`", cmd.path);
        return;
    };

    if selected.entities != [target] {
        selected.entities.clear();
        selected.entities.push(target);
    }
    info!("FOCUS_ENTITY_BY_PATH: focused `{}` ({target:?})", cmd.path);
}

/// A focus request recorded by [`on_focus_entity_by_id`] and applied by
/// [`apply_pending_focus`] at the start of the NEXT frame (`First` schedule).
///
/// The command observer fires wherever the API dispatcher happens to sit in
/// the frame, so this transaction is applied from `First` after any queued
/// orbit-return commands have flushed. Spatial math uses the authoritative
/// `(CellCoord, Transform)` chain through `lunco_core::coords`; derived
/// `GlobalTransform` is never a camera-placement input.
#[derive(Resource, Debug, Clone, Copy)]
pub struct PendingFocus {
    pub target: Entity,
    pub distance: f32,
}

fn replace_focus_diagnostic(
    diagnostics: &mut Option<ResMut<lunco_core::RuntimeDiagnostics>>,
    message: Option<String>,
) {
    if let Some(diagnostics) = diagnostics.as_deref_mut() {
        diagnostics.replace_producer(
            "scene-focus",
            message.map(|message| lunco_core::RuntimeDiagnostic {
                code: "scene-focus".to_string(),
                severity: lunco_core::DiagnosticSeverity::Error,
                producer: "scene-focus".to_string(),
                subject: "PendingFocus".to_string(),
                message,
            }),
        );
    }
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
    commands.insert_resource(PendingFocus {
        target,
        distance: cmd.distance,
    });
    info!(
        "FOCUS_ENTITY: queued focus on {target:?} at {} m",
        cmd.distance
    );
}

/// Applies a [`PendingFocus`] from authoritative BigSpace poses (`First`
/// schedule — see the type doc).
pub fn apply_pending_focus(
    pending: Option<Res<PendingFocus>>,
    mut q_avatar: Query<
        (
            Entity,
            &mut Transform,
            &mut big_space::prelude::CellCoord,
            &ChildOf,
            Option<&mut lunco_avatar::FreeFlightCamera>,
            Has<lunco_avatar::OrbitViewReturn>,
        ),
        (With<lunco_core::Avatar>, With<lunco_core::LocalAvatar>),
    >,
    q_grids: Query<&Grid>,
    q_parents: Query<&ChildOf>,
    q_spatial: Query<
        (Option<&big_space::prelude::CellCoord>, &Transform),
        Without<lunco_core::Avatar>,
    >,
    q_celestial: Query<(), With<lunco_celestial::CelestialBody>>,
    q_celestial_decl: Query<(), With<lunco_celestial::CelestialBodyDecl>>,
    q_children: Query<&Children>,
    mut commands: Commands,
    mut orbital_pin: Option<ResMut<lunco_celestial::OrbitalViewPin>>,
    local_avatar: Option<Res<lunco_core::TheLocalAvatar>>,
    mut diagnostics: Option<ResMut<lunco_core::RuntimeDiagnostics>>,
) {
    let Some(pending) = pending else { return };
    let (target, distance) = (pending.target, pending.distance);
    // Celestial bodies are ORBIT-scale targets: hand them to the avatar's
    // `FocusTarget` flow (OrbitCamera flies in the body's explicit inertial
    // view grid with sunlit-side arrival). Local framing stays for
    // metre-scale subjects (wheels, rovers, props).
    let mut is_celestial = q_celestial.get(target).is_ok() || q_celestial_decl.get(target).is_ok();
    let mut pending = vec![target];
    for _ in 0..8 {
        let mut next = Vec::new();
        for parent in pending.drain(..) {
            if let Ok(children) = q_children.get(parent) {
                for child in children.iter() {
                    if q_celestial.get(child).is_ok() || q_celestial_decl.get(child).is_ok() {
                        is_celestial = true;
                    }
                    next.push(child);
                }
            }
        }
        if is_celestial || next.is_empty() {
            break;
        }
        pending = next;
    }
    if is_celestial {
        commands.remove_resource::<PendingFocus>();
        commands.trigger(lunco_avatar::FocusTarget {
            avatar: None,
            target,
        });
        info!("FOCUS_ENTITY: celestial target {target:?} → orbit focus");
        return;
    }
    // A local target is authored in the pre-orbit scene frame. Restore the
    // avatar's exact orbit-entry transaction first, then retry this retained
    // focus next First frame. Applying a local delta while the camera is still
    // in an inertial body grid mixes semantic frames.
    if let Some(avatar) = local_avatar.as_deref().and_then(|slot| slot.0) {
        if q_avatar
            .get(avatar)
            .is_ok_and(|(_, _, _, _, _, orbit_return)| orbit_return)
        {
            commands.trigger(lunco_avatar::ReleaseVessel { target: avatar });
            info!("FOCUS_ENTITY: restored pre-orbit frame; local focus retries next frame");
            return;
        }
    }
    if let Some(pin) = orbital_pin.as_mut() {
        pin.active = false;
    }
    commands.remove_resource::<PendingFocus>();
    let Some(avatar_ent) = local_avatar.as_deref().and_then(|slot| slot.0) else {
        let message = "no authoritative LocalAvatar is available for local focus".to_string();
        warn!("FOCUS_ENTITY: {message}");
        replace_focus_diagnostic(&mut diagnostics, Some(message));
        return;
    };
    let Ok((avatar_ent, mut tf, mut cell, child_of, ff_opt, _)) = q_avatar.get_mut(avatar_ent)
    else {
        let message =
            format!("authoritative LocalAvatar {avatar_ent:?} has no complete focus state");
        warn!("FOCUS_ENTITY: {message}");
        replace_focus_diagnostic(&mut diagnostics, Some(message));
        return;
    };
    replace_focus_diagnostic(&mut diagnostics, None);
    let Ok(grid) = q_grids.get(child_of.parent()) else {
        let message = format!(
            "authoritative LocalAvatar {avatar_ent:?} is not parented directly under a BigSpace Grid"
        );
        warn!("FOCUS_ENTITY: {message}");
        replace_focus_diagnostic(&mut diagnostics, Some(message));
        return;
    };
    let avatar_pos = grid.grid_position_double(&cell, &tf);
    let target_pos = if target == avatar_ent {
        avatar_pos
    } else {
        let Some((target_pos, _)) = lunco_core::coords::pose_in_grid(
            target,
            child_of.parent(),
            &q_parents,
            &q_grids,
            &q_spatial,
        ) else {
            let message =
                format!("target {target:?} has no complete pose in the avatar's BigSpace frame");
            warn!("FOCUS_ENTITY: {message}");
            replace_focus_diagnostic(&mut diagnostics, Some(message));
            return;
        };
        target_pos
    };
    let dist = if distance > 0.1 { distance } else { 6.0 };
    // Camera sits mostly to the SIDE (+X, the wheel axle direction → we see
    // the spoke face) plus a little up and forward. (Celestial targets never
    // reach here — they take the orbit-focus early return above.)
    let dir = Vec3::new(1.0, 0.4, 0.25).normalize();
    let offset = dir * dist;
    // Re-split the complete target-relative pose through the owning Grid.
    // This preserves cell precision even when the camera was previously in an
    // inertial orbit grid; no render-space value participates in placement.
    let (new_cell, new_translation) = grid.translation_to_grid(target_pos + offset.as_dvec3());
    cell.set_if_neq(new_cell);
    if tf.translation != new_translation {
        tf.translation = new_translation;
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
                .remove::<lunco_avatar::SunlitArrival>()
                .remove::<lunco_avatar::SpringArmCamera>()
                .remove::<lunco_avatar::SurfaceCamera>()
                .remove::<lunco_avatar::SurfaceRelativeMode>()
                .try_insert(lunco_avatar::FreeFlightCamera {
                    yaw,
                    pitch,
                    damping: None,
                });
        }
    }
    info!(
        "FOCUS_ENTITY: framed target={target:?} at {:.1} m (avatar={avatar_ent:?})",
        dist
    );
}

/// Aim the free-flight avatar camera: place it at `eye` and look at `target`
/// (both absolute world-space). The flexible primitive — the client computes the
/// angle (e.g. approach a wheel from its outboard side) and distance.
///
/// Authoritative: whatever camera mode the avatar is in (orbit focus on a
/// planet, spring-arm follow, surface mode), this strips it and reinstates a
/// `FreeFlightCamera` at the requested pose — an API client asking for a
/// specific view must always get it. `eye` and `target` speak the semantic
/// [`lunco_core::ActivePhysicsFrame`]; the concrete grid is resolved from that
/// resource so a previous orbit focus or a canonical render-only grid cannot
/// put the camera in a different frame.
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
        (With<lunco_core::Avatar>, With<lunco_core::LocalAvatar>),
    >,
    active_frame: Res<lunco_core::ActivePhysicsFrame>,
    q_grids: Query<&Grid>,
    mut commands: Commands,
    mut orbital_pin: Option<ResMut<lunco_celestial::OrbitalViewPin>>,
    local_avatar: Option<Res<lunco_core::TheLocalAvatar>>,
    mut diagnostics: Option<ResMut<lunco_core::RuntimeDiagnostics>>,
) {
    let cmd = trigger.event();
    let Some(entity) = local_avatar.as_deref().and_then(|slot| slot.0) else {
        let message = "no authoritative LocalAvatar is available for SetCameraLookAt".to_string();
        warn!("SET_CAMERA: {message}");
        replace_focus_diagnostic(&mut diagnostics, Some(message));
        return;
    };
    let Ok((entity, mut tf, mut cell, child_of, ff_opt)) = q_avatar.get_mut(entity) else {
        let message = format!("authoritative LocalAvatar {entity:?} has no complete camera state");
        warn!("SET_CAMERA: {message}");
        replace_focus_diagnostic(&mut diagnostics, Some(message));
        return;
    };
    replace_focus_diagnostic(&mut diagnostics, None);
    // Explicit camera coordinates use the same active physics frame as
    // MoveEntity and route projection. Never select a grid by marker/component
    // type here: render and physics roots may legitimately differ.
    if let Some(pin) = orbital_pin.as_mut() {
        pin.active = false;
    }
    let root = active_frame.0;
    let Ok(grid) = q_grids.get(root) else {
        warn!(
            ?root,
            "SET_CAMERA: active physics frame has no Grid component"
        );
        return;
    };
    let (new_cell, new_translation) = grid.translation_to_grid(cmd.eye.as_dvec3());
    if child_of.parent() == root {
        cell.set_if_neq(new_cell);
        if tf.translation != new_translation {
            tf.translation = new_translation;
        }
    } else {
        lunco_core::attach::migrate_to_grid(
            &mut commands,
            entity,
            root,
            new_cell,
            Transform::from_translation(new_translation).with_rotation(tf.rotation),
        );
    }
    // An explicit world-space camera command starts a new free-flight view; it
    // does not retain a hidden return transaction or surface gravity binding.
    commands
        .entity(entity)
        .remove::<lunco_avatar::OrbitViewReturn>()
        .remove::<lunco_avatar::SurfaceRelativeMode>()
        .remove::<lunco_environment::GravityBody>();
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
            .remove::<lunco_avatar::OrbitViewReturn>()
            .remove::<lunco_avatar::SunlitArrival>()
            .remove::<lunco_avatar::SpringArmCamera>()
            .remove::<lunco_avatar::SurfaceCamera>()
            .remove::<lunco_avatar::SurfaceRelativeMode>()
            .remove::<lunco_environment::GravityBody>()
            .try_insert(lunco_avatar::FreeFlightCamera {
                yaw,
                pitch,
                damping: None,
            });
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
        [
            "shaders/wheel.wgsl",
            "shaders/balloon.wgsl",
            "shaders/solar_panel.wgsl",
        ]
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
    if guard.is_none_or(|g| g.0.is_none()) {
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
) -> Result<String, lunco_assets::TwinRootsError> {
    Ok(
        match twin_roots.map(|t| t.primary()).transpose()?.flatten() {
            Some((name, _)) => lunco_assets::twin_uri(&name, format!("shaders/{stem}.wgsl")),
            None => format!("shaders/{stem}.wgsl"),
        },
    )
}

/// Sanitise a free-text name into a safe lowercase file stem (`[a-z0-9_]`,
/// trimmed of leading/trailing `_`). Empty input → `"shader"`.
pub fn sanitize_stem(s: &str) -> String {
    let out: String = s
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    let out = out.trim_matches('_').to_string();
    if out.is_empty() {
        "shader".to_string()
    } else {
        out
    }
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
    // Gate: must be a self-describing `Material` shader, and every `//!@engine`
    // field it declares must be one a plain prop entity actually receives —
    // `prop_fillable` in the engine-param registry. Otherwise it would render
    // black (e.g. terrain-only inputs) / can't be driven.
    if !lunco_materials::is_prop_pickable_source(source) {
        warn!(
            "INSTALL_SHADER: '{stem}' is not a prop-pickable dynamic shader \
             (needs a `Material` struct; every `//!@engine` field must be \
             prop-fillable per the engine-param registry) — skipped"
        );
        return None;
    }

    // Destination: the primary open Twin's `shaders/` dir (portable, persists
    // with the Twin under a `twin://` asset path), else the engine library.
    let primary = match twin_roots
        .map(|t| t.primary())
        .transpose()
        .map(|primary| primary.flatten())
    {
        Ok(primary) => primary,
        Err(error) => {
            error!("INSTALL_SHADER: Twin registry unavailable: {error}");
            return None;
        }
    };
    let (asset_path, disk_path): (String, std::path::PathBuf) = match primary {
        Some((name, root)) => (
            lunco_assets::twin_uri(&name, format!("shaders/{stem}.wgsl")),
            root.join("shaders").join(format!("{stem}.wgsl")),
        ),
        None => (
            format!("shaders/{stem}.wgsl"),
            lunco_assets::assets_dir_abs()
                .join("shaders")
                .join(format!("{stem}.wgsl")),
        ),
    };

    // Persist to disk (native). Non-fatal on failure — the in-memory insert
    // below still makes it usable this session.
    #[cfg(not(target_arch = "wasm32"))]
    {
        match lunco_storage::write_file_sync(&disk_path, source.as_bytes()) {
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
/// {"type":"ExecuteCommand","command":"CreateShader","params":{"name":"my_panel","template":"checker","target":42}}
/// {"type":"ExecuteCommand","command":"CreateShader","params":{"name":"custom","source":"<wgsl...>"}}
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
/// dynamic shader: a `Material` struct, and every `//!@engine` field it declares
/// must be prop-fillable per the engine-param registry.
///
/// ```json
/// {"type":"ExecuteCommand","command":"ImportShader","params":{"source_path":"/home/me/cool.wgsl","name":"cool","target":42}}
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
        let src = match lunco_assets::read_asset_file_string(std::path::Path::new(&ev.source_path))
        {
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
    manifest: &lunco_assets::discovery::AssetManifest,
    roots: &lunco_assets::twin_source::TwinRoots,
    catalog: &mut lunco_materials::ShaderCatalog,
) -> usize {
    let mut n = 0;
    let assets = match lunco_assets::discovery::list_assets(manifest, roots, "wgsl") {
        Ok(assets) => assets,
        Err(error) => {
            error!("SHADER_CATALOG: Twin registry unavailable: {error}");
            return 0;
        }
    };
    for a in assets {
        if catalog.add(a.asset_path) {
            n += 1;
        }
    }
    n
}

/// The ONE catalog-population system. Scans the engine library once, then
/// re-scans whenever the set of open Twins changes (so a freshly-opened Twin's
/// files appear) — twin-open is async, so a guarded `Update` check is more
/// robust than racing the `TwinAdded` observer that registers the twin root.
///
/// # Driven by its inputs
///
/// This re-enumerates exactly when one of its two inputs changes: the engine-library
/// [`AssetManifest`](lunco_assets::discovery::AssetManifest), or the set of open
/// Twins. Every other frame it early-returns on a cheap comparison — no per-frame
/// walk.
///
/// The catalog uses resource change detection instead of a write-once scan latch.
/// This lets a manifest that arrives late trigger its first real scan.
///
/// The two catalogs differ in what they need from a file. Shaders are catalogued by
/// *name* — enumeration is the whole job, so it finishes here. Spawnables are
/// catalogued by what the USD *says* (`lunco:spawnable`), which means reading it:
/// this only *dispatches* those reads, and `drain_usd_scan` folds them in as
/// they complete.
pub fn maintain_catalogs(
    twin_roots: Option<Res<lunco_assets::twin_source::TwinRoots>>,
    manifest: Res<lunco_assets::discovery::AssetManifest>,
    mut scan: ResMut<crate::catalog::CatalogScan>,
    mut shaders: ResMut<lunco_materials::ShaderCatalog>,
    mut last_twins: Local<Vec<String>>,
    settings: Res<lunco_settings::DownloadSettings>,
) {
    let Some(roots) = twin_roots.as_deref() else {
        return;
    };

    let names = match roots.names() {
        Ok(names) => names,
        Err(error) => {
            error!("CATALOG_SCAN: Twin registry unavailable: {error}");
            return;
        }
    };
    let twins_changed = names != *last_twins;
    if !manifest.is_changed() && !twins_changed {
        return;
    }
    *last_twins = names;

    let s = crate::catalog::dispatch_usd_scan(&manifest, roots, &mut scan, &settings);
    let w = scan_wgsl_into_catalog(&manifest, roots, &mut shaders);
    if s > 0 || w > 0 {
        info!("CATALOG_SCAN: reading {s} USD asset(s), +{w} shader(s)");
    }
}

/// Observer for [`RescanShaders`] — manual full re-scan of the shader catalog.
#[on_command(RescanShaders)]
pub fn on_rescan_shaders(
    _trigger: On<RescanShaders>,
    twin_roots: Option<Res<lunco_assets::twin_source::TwinRoots>>,
    manifest: Res<lunco_assets::discovery::AssetManifest>,
    mut catalog: ResMut<lunco_materials::ShaderCatalog>,
) {
    if let Some(roots) = twin_roots.as_deref() {
        let n = scan_wgsl_into_catalog(&manifest, roots, &mut catalog);
        info!("RESCAN_SHADERS: +{n} shader(s)");
    }
}

/// Delete a shader: unregister it from the picker [`ShaderCatalog`] and remove
/// its `.wgsl` from disk (the twin's `shaders/` folder, or `assets/shaders`).
/// Entities currently using it keep their in-memory material for the session.
///
/// ```json
/// {"type":"ExecuteCommand","command":"DeleteShader","params":{"path":"twin://moonbase/shaders/old.wgsl"}}
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
    schemes: Option<Res<lunco_assets::SchemeRegistry>>,
    mut catalog: ResMut<lunco_materials::ShaderCatalog>,
) {
    let path = trigger.event().path.trim().to_string();
    if path.is_empty() {
        warn!("DELETE_SHADER: empty path");
        return;
    }
    let removed = catalog.remove(&path);
    // `twin://<name>/<rel>` → the Twin root, a bare `shaders/foo.wgsl` → the
    // shipped library: both are the registry's job, so this crate re-derives
    // neither root (a copy here once joined a bare relative `"assets"`, resolving
    // against the CWD instead of the library path the loader uses).
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(schemes) = schemes.as_ref() {
        match schemes.local_path(&path) {
            Ok(Some(disk)) => match lunco_storage::delete_file_sync(&disk) {
                Ok(()) => info!("DELETE_SHADER: removed {path} ({})", disk.display()),
                Err(e) => warn!("DELETE_SHADER: unregistered {path}, file remove failed: {e}"),
            },
            Ok(None) => {}
            Err(error) => error!("DELETE_SHADER: asset scheme registry unavailable: {error}"),
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

/// Freeze physics and advance it deliberately, one frame at a time.
///
/// The verb a cutscene or an offline recording wants, and the reason it is NOT
/// `SetTimeTransport`: pausing the world clock also stops `FixedUpdate`, so the
/// scenario script that paused it never runs again to unpause itself — the shot
/// hangs and a recording spools frames forever. A physics hold freezes
/// `Time<Physics>` while `Time<Virtual>` (and so the script) keeps running.
///
/// * `{"hold": true}` — freeze the world; the script keeps ticking.
/// * `{"steps": 1}` — let exactly one frame of physics through, then re-freeze.
/// * `{"hold": false}` — hand the world back to normal simulation.
///
/// Steps only apply while held; queued with nothing holding they are dropped rather
/// than banked against an unrelated hold (a terrain bake, say).
#[Command(default)]
pub struct StepPhysics {
    /// Raise (`Some(true)`) / release (`Some(false)`) the cinematic hold; `None`
    /// leaves it as-is so a step can be sent on its own.
    pub hold: Option<bool>,
    /// Frames of physics to let through the hold. `None` = 0.
    pub steps: Option<u32>,
}

#[on_command(StepPhysics)]
fn on_step_physics(
    trigger: On<StepPhysics>,
    mut holds: ResMut<lunco_physics::PhysicsHolds>,
    mut req: ResMut<lunco_physics::PhysicsStepRequest>,
) {
    let cmd = trigger.event();
    if let Some(hold) = cmd.hold {
        holds.set(lunco_physics::PhysicsHolds::CINEMATIC, hold);
        // Releasing drops any unspent debt: the world is running again, so owed
        // frames are meaningless and must not survive into the next hold.
        if !hold {
            req.clear();
        }
    }
    if let Some(steps) = cmd.steps {
        req.request(steps);
    }
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
    on_focus_entity_by_path,
    on_import_shader,
    on_move_entity_command,
    on_rotate_entity_command,
    on_reload_shader,
    on_rescan_shaders,
    on_rescan_spawn_catalog,
    crate::lint_command::on_run_lint,
    on_set_camera_look_at,
    on_set_object_property,
    on_set_shader_source,
    on_set_usd_attribute,
    on_set_usd_connection,
    on_spawn_entity_command,
    on_step_physics,
    on_transform_entity_command,
);

impl Plugin for SpawnCommandPlugin {
    fn build(&self, app: &mut App) {
        // Catalog/source reads may fetch browser-served assets. Keep the
        // settings resource available when this headless-safe plugin is used
        // without the GUI dataset plugin.
        lunco_settings::ensure_download_settings(app);
        // Every `#[Command]` this crate owns — type + observer in one call, so a
        // verb can't end up observable-but-unconstructible (the old split wired
        // the observer by hand and then patched the type registry separately, and
        // whenever the second half was forgotten the command silently vanished
        // from the HTTP API / rhai / `discover_schema`).
        register_all_commands(app);
        // Runtime waypoint creation and collision-sensor arrival are shared by
        // the GUI click path and the deterministic headless scene runner.
        crate::runtime_waypoint::register(app);
        // The READ verb for the same entities. Registered here so any binary with
        // the scene verbs answers `QueryEntity` too — the headless server included.
        crate::entity_query::register(app);
        // SpawnEntity consumes an entry id from this catalog; expose the exact
        // discovered catalog to scripting and API clients from its owner.
        crate::catalog::register_query(app);
        // The AUTHORED read beside the spawned one: composed USD attributes, so
        // asset invariants are checkable from rhai/Python/HTTP and not just Rust.
        crate::usd_prim_query::register(app);
        // Parse-only asset pre-flight ("does this file compile?") — pure file
        // checks, so it answers even while no scene is loaded.
        crate::validate::register(app);
        // `RunLint` + the `LintReport` read-back. Nothing lints on load or on a
        // physics cadence: the linter is an explicit verb called from rhai, HTTP
        // or MCP after an authoring/preflight change.
        crate::lint_command::register(app);
        // Selection → telemetry focus, so every host that has the scene verbs has
        // scoped telemetry (the sandbox, the workbench, a headless server driven
        // by `SelectEntity`). Render-free: `lunco-signal` is a ring buffer of
        // f64s, not a UI. See `crate::mirror_selection_to_telemetry_focus`.
        app.init_resource::<crate::SelectedEntities>();
        app.init_resource::<lunco_signal::TelemetryFocus>();
        app.add_systems(Update, crate::mirror_selection_to_telemetry_focus);
        // Dock release as an actuator on the intent→port machinery (replaces the
        // hardcoded G-to-detach): register the `release` port backend, attach a
        // ReleaseActuator to every control-bound vessel, and edge-detect → detach.
        app.register_type::<ReleaseActuator>();
        app.add_systems(Startup, register_release_backend);
        app.add_systems(Update, (attach_release_actuator, joint_release_system));
        // Persist a Persistent DetachJoint into the active doc's runtime layer.
        app.add_observer(persist_detach_to_runtime_layer);
        app.add_observer(persist_delete_to_runtime_layer);
        // C4b: persist authored-scene moves into the active doc's runtime layer.
        app.add_observer(persist_move_to_runtime_layer);
        app.add_observer(persist_rotation_to_runtime_layer);
        app.add_observer(persist_transform_to_runtime_layer);
        // #4: persist scalar shader-param tunes into the active doc's runtime
        // overlay (non-destructive; Save stays base-only). Decoupled from the
        // live-mutation handler above, like the move/spawn persisters.
        app.add_observer(persist_property_to_runtime_layer);
        // #15: persist wheel-dynamics tunes (suspension/drive → physxVehicle*)
        // and visibility. PBR intent is authored by SetObjectProperty's
        // canonical UsdPreviewSurface path.
        app.add_observer(persist_wheel_to_runtime_layer);
        // #14: persist a `SetEnvironmentLight` sun tweak (illuminance / colour /
        // shadow range) as `SetAttribute`s on the sun's DistantLight prim, using
        // the names the loader already reads back — so it round-trips + journals.
        app.add_observer(persist_environment_light_to_runtime_layer);
        // Applies the recorded focus at frame start after any orbit-return
        // transaction has flushed (see PendingFocus). The solver reads the
        // authoritative BigSpace pose chain, not derived GlobalTransforms.
        app.add_systems(bevy::app::First, apply_pending_focus);
        // NOTE: `SelectEntity`/`on_select_entity` are editor-only (they drive the
        // Inspector highlight + gizmo) and live in the `ui`-gated `selection`
        // module; `SceneEditPlugin` registers them. The headless server has no
        // selection, so they're absent here by design.
        // THE single catalog-population system: scans project USD → spawn
        // catalog and WGSL → shader catalog via the shared `lunco_assets`
        // discovery walk, once at first run and again only when the open-Twin
        // set changes (guarded — no per-frame disk walk). Replaces the old
        // per-catalog scanners (`populate_dynamic_spawn_catalog`,
        // `auto_scan_twin_shaders`, `discover_shaders`).
        // Enumerate → dispatch reads → fold results in. `drain` runs after
        // `maintain` so a read that completes between frames lands the moment
        // the app looks, not a frame later.
        app.add_systems(
            Update,
            (maintain_catalogs, crate::catalog::drain_usd_scan).chain(),
        );
        app.add_systems(FixedPostUpdate, clear_kinematic_pulse_velocity);
        // Resources this plugin's OWN systems read, so it stands alone without the
        // UI-layer `SceneEditPlugin` / the render-layer `ShaderMaterialPlugin`
        // (e.g. a headless `--no-ui` server that adds only `SpawnCommandPlugin`).
        // The host must install `lunco_assets::register_lunco_asset_sources`
        // before Bevy's asset plugin; that shared asset boundary owns the
        // `AssetManifest` and `TwinRoots` resources consumed here.
        // `init_resource` is idempotent, so when those plugins also init these it's
        // a harmless no-op:
        //   - `SpawnCatalog`   — read by `maintain_catalogs` + `apply_replicated_spawns`;
        //   - `SelectedEntity` — read by `on_select_entity`;
        //   - `ShaderCatalog`  — read by `maintain_catalogs` (per-frame) + the shader
        //     command observers. Lives in `lunco_materials`; an empty one is fine on
        //     a server (shader discovery populates it but nothing renders it).
        app.init_resource::<crate::catalog::SpawnCatalog>();
        // `CatalogScan` — the async read pipeline `maintain_catalogs` dispatches
        // into. `AssetMetaStore` — what the scanned files said about themselves;
        // the catalogue is derived from it, and the Scenarios menu reads its
        // standard USD `doc` metadata straight out (no second cache, no second parse).
        app.init_resource::<crate::catalog::CatalogScan>();
        app.init_resource::<crate::catalog::AssetMetaStore>();
        app.init_resource::<crate::SelectedEntities>();
        // A selection names entities of the scene that made it. Those ids die
        // with that scene, and Bevy reuses generations — so a selection carried
        // across a reload is at best an inspector showing nothing and at worst a
        // panel editing whatever now holds the recycled id. Scene state, so it
        // unloads with the scene.
        app.add_systems(
            lunco_core::SceneTeardown,
            |mut selected: ResMut<crate::SelectedEntities>| {
                if !selected.entities.is_empty() {
                    selected.entities.clear();
                }
            },
        );
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
    fn set_camera_uses_the_active_physics_frame_for_noncanonical_grid() {
        use super::*;
        use big_space::prelude::{CellCoord, Grid};

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_observer(on_set_camera_look_at);

        let canonical_render_grid = app
            .world_mut()
            .spawn((
                Grid::new(2_000.0, 0.0),
                lunco_core::WorldGrid,
                GlobalTransform::default(),
            ))
            .id();
        let active_physics_grid = app
            .world_mut()
            .spawn((Grid::new(2_000.0, 0.0), GlobalTransform::default()))
            .id();
        app.insert_resource(lunco_core::ActivePhysicsFrame(active_physics_grid));
        app.insert_resource(lunco_core::TheLocalAvatar::default());

        let avatar = app
            .world_mut()
            .spawn((
                lunco_core::Avatar,
                lunco_core::LocalAvatar,
                CellCoord::ZERO,
                Transform::default(),
                GlobalTransform::default(),
                ChildOf(active_physics_grid),
                lunco_avatar::FreeFlightCamera {
                    yaw: 0.0,
                    pitch: 0.0,
                    damping: None,
                },
            ))
            .id();

        app.world_mut().trigger(SetCameraLookAt {
            eye: Vec3::new(0.0, 2_500.0, 0.0),
            target: Vec3::ZERO,
        });
        app.update();

        assert_eq!(
            app.world().get::<ChildOf>(avatar).unwrap().parent(),
            active_physics_grid,
            "camera placement must not migrate into the render-only WorldGrid"
        );
        let cell = *app.world().get::<CellCoord>(avatar).unwrap();
        let translation = app.world().get::<Transform>(avatar).unwrap().translation;
        let composed_y = cell.y as f64 * 2_000.0 + translation.y as f64;
        assert!((composed_y - 2_500.0).abs() < 1.0e-3);
        assert_ne!(canonical_render_grid, active_physics_grid);
    }

    #[test]
    fn focus_uses_authoritative_grid_pose_not_render_global_transform() {
        use super::*;
        use big_space::prelude::{CellCoord, Grid};

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);

        let grid = app
            .world_mut()
            .spawn((
                Grid::new(1_000.0, 0.0),
                Transform::default(),
                GlobalTransform::default(),
            ))
            .id();
        let target = app
            .world_mut()
            .spawn((
                CellCoord::new(2, 0, -1),
                Transform::from_xyz(25.0, 3.0, -10.0),
                // This deliberately stale render pose must not affect focus.
                GlobalTransform::from(Transform::from_xyz(-1.0e11, 2.0e11, 3.0e11)),
                ChildOf(grid),
            ))
            .id();
        let avatar = app
            .world_mut()
            .spawn((
                lunco_core::Avatar,
                lunco_core::LocalAvatar,
                CellCoord::new(1, 0, 0),
                Transform::from_xyz(4.0, 6.0, 8.0),
                GlobalTransform::from(Transform::from_xyz(7.0e10, -8.0e10, 9.0e10)),
                ChildOf(grid),
                lunco_avatar::FreeFlightCamera {
                    yaw: 0.0,
                    pitch: 0.0,
                    damping: None,
                },
            ))
            .id();

        app.insert_resource(PendingFocus {
            target,
            distance: 6.0,
        });
        app.insert_resource(lunco_core::TheLocalAvatar(Some(avatar)));
        app.add_systems(bevy::app::First, apply_pending_focus);
        app.update();

        let grid = app.world().get::<Grid>(grid).unwrap();
        let target_pos = DVec3::new(2_025.0, 3.0, -1_010.0);
        let offset = Vec3::new(1.0, 0.4, 0.25).normalize() * 6.0;
        let actual = {
            let cell = app.world().get::<CellCoord>(avatar).unwrap();
            let transform = app.world().get::<Transform>(avatar).unwrap();
            grid.grid_position_double(cell, transform)
        };
        assert!((actual - (target_pos + offset.as_dvec3())).length() < 1.0e-3);

        let freeflight = app
            .world()
            .get::<lunco_avatar::FreeFlightCamera>(avatar)
            .unwrap();
        let direction = (-offset).normalize();
        assert!((freeflight.yaw - (-direction.x).atan2(-direction.z)).abs() < 1.0e-6);
        assert!((freeflight.pitch - direction.y.asin()).abs() < 1.0e-6);
    }

    #[test]
    fn shader_property_values_require_the_reflected_schema() {
        let schema = lunco_materials::ParamSchema::parse(
            "//!@engine sun_dir\n\
             struct Material { color: vec4<f32>, amount: f32, sun_dir: vec3<f32> }",
        )
        .expect("shader schema");

        assert_eq!(
            super::shader_param_value(Some(&schema), "color", "0.1,0.2,0.3"),
            Some(lunco_materials::ParamValue::Vec4([0.1, 0.2, 0.3, 1.0]))
        );
        assert!(super::shader_param_value(Some(&schema), "unknown", "1").is_none());
        assert!(super::shader_param_value(Some(&schema), "amount", "1,bad").is_none());
        assert!(super::shader_param_value(Some(&schema), "sun_dir", "1,2,3").is_none());
        assert!(super::shader_param_value(None, "amount", "1").is_none());
    }

    #[test]
    fn test_spawn_entity_struct_exists() {
        // Verify the struct can be constructed
        let cmd = super::SpawnEntity {
            entry_id: "test".to_string(),
            position: [0.0; 3],
            rotation: None,
        };
        assert_eq!(cmd.entry_id, "test");
    }

    #[test]
    fn spawn_pose_is_converted_to_scene_root_axes_once() {
        use super::*;
        use bevy::ecs::system::SystemState;
        use big_space::prelude::{CellCoord, Grid};

        let mut world = World::new();
        let active_grid = lunco_core::WorldGridConfig::default().grid();
        let root_cell = CellCoord::new(200, -100, 350);
        let root_rotation = DQuat::from_rotation_x(0.7) * DQuat::from_rotation_y(-1.1);
        let root_transform =
            Transform::from_xyz(0.25, -0.5, 0.75).with_rotation(root_rotation.as_quat());
        let active = world.spawn((active_grid, GlobalTransform::default())).id();
        let expected_local = DVec3::new(12.25, 0.89, -44.5);
        let expected_rotation = DQuat::from_rotation_y(0.3);
        let scene_root = world
            .spawn((root_cell, root_transform, ChildOf(active)))
            .id();

        let mut state: SystemState<(
            Query<&ChildOf>,
            Query<&Grid>,
            Query<(Option<&big_space::prelude::CellCoord>, &Transform)>,
        )> = SystemState::new(&mut world);
        let (parents, grids, spatial) = state.get(&world).unwrap();
        let (root_position, stored_root_rotation) =
            lunco_core::coords::grid_relative_pose(scene_root, active, &parents, &grids, &spatial)
                .expect("scene root pose is available in the active physics frame");
        let stored_root_rotation = stored_root_rotation.normalize();
        assert!(
            root_position.length() > 1.0e5,
            "cell translation was not composed"
        );
        let requested_position = root_position + stored_root_rotation * expected_local;
        let requested_rotation = stored_root_rotation * expected_rotation;
        let (actual_position, actual_rotation) = lunco_core::coords::pose_in_parent_local(
            requested_position,
            requested_rotation,
            scene_root,
            active,
            &parents,
            &grids,
            &spatial,
        )
        .expect("scene root is connected to active physics frame");

        assert!(
            (actual_position - expected_local).length() < 1e-7,
            "actual={actual_position:?} expected={expected_local:?} delta={:?}",
            actual_position - expected_local
        );
        assert!(actual_rotation.abs_diff_eq(expected_rotation, 1e-12));
    }

    // ── MoveEntity's frame contract ─────────────────────────────────────

    /// `MoveEntity::translation` is in the active physics frame, so the handler must split it
    /// into the `(CellCoord, Transform)` pair big_space stores — writing only
    /// `Transform` would leave the stale cell in place and land the body
    /// `cell × edge` from the requested spot.
    ///
    /// Pinned at a NON-zero cell: in cell 0 the active-frame position and the
    /// local `Transform` are identical, which is why the sandbox never showed this
    /// and the moonbase (2 km cells) teleported a dragged prim out of sight.
    #[test]
    fn move_entity_splits_a_grid_absolute_target_across_cells() {
        use super::*;
        use big_space::prelude::{CellCoord, Grid};

        const EDGE: f32 = 2000.0;
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<lunco_api::registry::ApiEntityRegistry>();
        app.add_observer(on_move_entity_command);

        let grid = app
            .world_mut()
            .spawn((
                Grid::new(EDGE, 0.0),
                CellCoord::ZERO,
                Transform::default(),
                GlobalTransform::default(),
            ))
            .id();
        app.insert_resource(lunco_core::ActivePhysicsFrame(grid));
        // Starts at grid-absolute (0, 3947, 0) = cell y=2 + local y=-53.
        let body = app
            .world_mut()
            .spawn((
                CellCoord::new(0, 2, 0),
                Transform::from_translation(Vec3::new(0.0, -53.0, 0.0)),
                GlobalTransform::default(),
                ChildOf(grid),
            ))
            .id();
        let gid = lunco_core::GlobalEntityId::from_raw(7);
        app.world_mut()
            .resource_mut::<lunco_api::registry::ApiEntityRegistry>()
            .assign(body, gid);

        // Move it 100 m up, in grid-absolute terms: 3947 → 4047.
        app.world_mut().trigger(MoveEntity {
            entity_id: 7,
            translation: [0.0, 4047.0, 0.0],
        });
        app.update();

        let cell = app.world().get::<CellCoord>(body).copied().unwrap();
        let tf = app.world().get::<Transform>(body).copied().unwrap();
        let landed = cell.y as f32 * EDGE + tf.translation.y;
        assert!(
            (landed - 4047.0).abs() < 1e-2,
            "reassembled position {landed} != requested 4047 (cell {cell:?}, local {:?})",
            tf.translation
        );
        // The whole point: the request must NOT have been written raw into the
        // local transform, which is what threw the object a cell away.
        assert!(
            tf.translation.y.abs() < EDGE,
            "local translation {} must be a cell remainder, not the absolute",
            tf.translation.y
        );
    }

    /// A body below a plain parent has no cell. The public active-frame target
    /// must be converted through that parent exactly once, and no cell may be
    /// invented for it.
    #[test]
    fn move_entity_leaves_a_cell_less_entity_alone() {
        use super::*;
        use big_space::prelude::{CellCoord, Grid};

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<lunco_api::registry::ApiEntityRegistry>();
        app.add_observer(on_move_entity_command);

        let grid = app
            .world_mut()
            .spawn((Grid::new(2_000.0, 0.0), GlobalTransform::default()))
            .id();
        app.insert_resource(lunco_core::ActivePhysicsFrame(grid));
        let parent = app
            .world_mut()
            .spawn((Transform::from_xyz(10.0, 0.0, 0.0), ChildOf(grid)))
            .id();
        let loose = app
            .world_mut()
            .spawn((
                Transform::default(),
                GlobalTransform::default(),
                ChildOf(parent),
            ))
            .id();
        app.world_mut()
            .resource_mut::<lunco_api::registry::ApiEntityRegistry>()
            .assign(loose, lunco_core::GlobalEntityId::from_raw(9));

        app.world_mut().trigger(MoveEntity {
            entity_id: 9,
            translation: [1.0, 2.0, 3.0],
        });
        app.update();

        assert_eq!(
            app.world().get::<Transform>(loose).unwrap().translation,
            Vec3::new(-9.0, 2.0, 3.0)
        );
        assert!(app.world().get::<CellCoord>(loose).is_none());
    }

    #[test]
    fn move_entity_rejects_a_large_physics_body_jump() {
        use super::*;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<lunco_api::registry::ApiEntityRegistry>();
        app.add_observer(on_move_entity_command);

        let grid = app
            .world_mut()
            .spawn((
                big_space::prelude::Grid::new(2_000.0, 0.0),
                GlobalTransform::default(),
            ))
            .id();
        app.insert_resource(lunco_core::ActivePhysicsFrame(grid));

        let body = app
            .world_mut()
            .spawn((
                RigidBody::Dynamic,
                Transform::default(),
                GlobalTransform::default(),
                ChildOf(grid),
            ))
            .id();
        app.world_mut()
            .resource_mut::<lunco_api::registry::ApiEntityRegistry>()
            .assign(body, lunco_core::GlobalEntityId::from_raw(10));

        app.world_mut().trigger(MoveEntity {
            entity_id: 10,
            translation: [MAX_MOVE_ENTITY_DISPLACEMENT + 1.0, 0.0, 0.0],
        });
        app.update();

        assert_eq!(
            app.world().get::<Transform>(body).unwrap().translation,
            Vec3::ZERO,
            "a pointer discontinuity must not teleport a physics body"
        );
    }

    #[test]
    fn move_entity_rejects_a_physics_body_target_outside_world_bounds() {
        use super::*;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<lunco_api::registry::ApiEntityRegistry>();
        app.insert_resource(lunco_physics::WorldBounds::Some {
            min: DVec3::splat(-100.0),
            max: DVec3::splat(100.0),
        });
        app.add_observer(on_move_entity_command);

        let grid = app
            .world_mut()
            .spawn((
                big_space::prelude::Grid::new(2_000.0, 0.0),
                GlobalTransform::default(),
            ))
            .id();
        app.insert_resource(lunco_core::ActivePhysicsFrame(grid));

        let body = app
            .world_mut()
            .spawn((
                RigidBody::Dynamic,
                Transform::default(),
                GlobalTransform::default(),
                ChildOf(grid),
            ))
            .id();
        app.world_mut()
            .resource_mut::<lunco_api::registry::ApiEntityRegistry>()
            .assign(body, lunco_core::GlobalEntityId::from_raw(11));

        app.world_mut().trigger(MoveEntity {
            entity_id: 11,
            translation: [10.0, 0.0, 101.0],
        });
        app.update();

        assert_eq!(
            app.world().get::<Transform>(body).unwrap().translation,
            Vec3::ZERO,
            "a physics body must remain inside the authored local world"
        );
    }

    // ── C4b: move-transform → runtime-layer persistence ─────────────────

    #[test]
    fn rotate_entity_stores_parent_local_but_round_trips_in_active_frame() {
        use super::*;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<lunco_api::registry::ApiEntityRegistry>();
        app.add_observer(on_rotate_entity_command);

        let active = app
            .world_mut()
            .spawn((
                lunco_core::WorldGridConfig::default().grid(),
                GlobalTransform::default(),
            ))
            .id();
        app.insert_resource(lunco_core::ActivePhysicsFrame(active));
        let parent_rotation = Quat::from_rotation_y(0.7);
        let parent = app
            .world_mut()
            .spawn((Transform::from_rotation(parent_rotation), ChildOf(active)))
            .id();
        let body = app
            .world_mut()
            .spawn((Transform::default(), ChildOf(parent)))
            .id();
        app.world_mut()
            .resource_mut::<lunco_api::registry::ApiEntityRegistry>()
            .assign(body, lunco_core::GlobalEntityId::from_raw(12));
        let desired = DQuat::from_rotation_x(-0.35);

        app.world_mut().trigger(RotateEntity {
            entity_id: 12,
            rotation: desired.to_array(),
        });
        app.update();

        let stored = app.world().get::<Transform>(body).unwrap().rotation;
        let expected_local = parent_rotation.inverse().as_dquat() * desired;
        assert!(stored.dot(expected_local.as_quat()).abs() > 1.0 - 1.0e-6);

        let mut state: bevy::ecs::system::SystemState<(
            Query<&ChildOf>,
            Query<&Grid>,
            Query<(Option<&CellCoord>, &Transform)>,
        )> = bevy::ecs::system::SystemState::new(app.world_mut());
        let (parents, grids, spatial) = state.get(app.world()).unwrap();
        let (_, round_trip) =
            lunco_core::coords::pose_in_grid(body, active, &parents, &grids, &spatial)
                .expect("body remains connected to active frame");
        assert!(round_trip.as_quat().dot(desired.as_quat()).abs() > 1.0 - 1.0e-6);
    }

    #[test]
    fn rotate_entity_publishes_a_shortest_angular_pulse() {
        use super::*;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<lunco_api::registry::ApiEntityRegistry>();
        app.add_observer(on_rotate_entity_command);

        let active = app
            .world_mut()
            .spawn((
                lunco_core::WorldGridConfig::default().grid(),
                GlobalTransform::default(),
            ))
            .id();
        app.insert_resource(lunco_core::ActivePhysicsFrame(active));
        let body = app
            .world_mut()
            .spawn((
                RigidBody::Dynamic,
                Transform::default(),
                GlobalTransform::default(),
                AngularVelocity::default(),
                ChildOf(active),
            ))
            .id();
        app.world_mut()
            .resource_mut::<lunco_api::registry::ApiEntityRegistry>()
            .assign(body, lunco_core::GlobalEntityId::from_raw(13));

        // The negative quaternion names the same quarter-turn as the positive
        // one. The pulse must follow the short +Y arc, not a nearly full turn.
        let desired = -DQuat::from_rotation_y(0.25);
        app.world_mut().trigger(RotateEntity {
            entity_id: 13,
            rotation: desired.to_array(),
        });
        app.update();

        let angular = app.world().get::<AngularVelocity>(body).unwrap().0;
        assert!(
            (angular.y - 60.0).abs() < 1.0e-5,
            "angular pulse={angular:?}"
        );
        assert!(angular.x.abs() < 1.0e-9 && angular.z.abs() < 1.0e-9);
        assert!(app
            .world()
            .get::<JustMovedKinematic>(body)
            .is_some_and(|marker| marker.angular_pulse));
    }

    #[test]
    fn angular_pulse_cleanup_restores_bodies_without_linear_velocity() {
        use super::*;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_systems(Update, clear_kinematic_pulse_velocity);
        let body = app
            .world_mut()
            .spawn((
                RigidBody::Kinematic,
                AngularVelocity(DVec3::new(1.0, 2.0, 3.0)),
                JustMovedKinematic {
                    restore: Some(RigidBody::Dynamic),
                    angular_pulse: true,
                },
            ))
            .id();

        app.update();

        assert_eq!(
            app.world().get::<RigidBody>(body).copied(),
            Some(RigidBody::Dynamic)
        );
        assert_eq!(
            app.world().get::<AngularVelocity>(body).unwrap().0,
            DVec3::ZERO
        );
        assert!(app.world().get::<JustMovedKinematic>(body).is_none());
    }

    /// Build a headless app with the runtime-move producer wired and an active
    /// USD document containing `/World`, plus a sim entity bound to `prim_path`
    /// under api id `api_id`. Returns `(app, doc_id)`.
    fn app_with_runtime_producer(
        prim_path: &str,
        api_id: u64,
    ) -> (bevy::prelude::App, lunco_doc::DocumentId) {
        use super::*;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        // UsdCommandsPlugin inserts DocumentRegistry<UsdDocument> + the `on_apply_usd_op`
        // observer that processes the `ApplyUsdOp` our producer dispatches.
        app.add_plugins(lunco_usd::commands::UsdCommandsPlugin);
        app.init_resource::<lunco_api::registry::ApiEntityRegistry>();
        app.add_observer(persist_move_to_runtime_layer);
        app.add_observer(persist_rotation_to_runtime_layer);

        let doc = {
            let mut reg = app
                .world_mut()
                .resource_mut::<DocumentRegistry<UsdDocument>>();
            reg.allocate(
                "#usda 1.0\ndef Xform \"World\"\n{\n}\n".to_string(),
                lunco_doc::PathlessOrigin::untitled("Scene.usda"),
            )
        };
        let mut ws = lunco_workspace::Workspace::default();
        ws.active_document = Some(doc);
        app.insert_resource(lunco_workspace::WorkspaceResource(ws));

        let grid = app
            .world_mut()
            .spawn((
                lunco_core::WorldGridConfig::default().grid(),
                GlobalTransform::default(),
            ))
            .id();
        app.insert_resource(lunco_core::ActivePhysicsFrame(grid));
        let ent = app
            .world_mut()
            .spawn((
                UsdPrimPath {
                    stage_handle: Handle::default(),
                    path: prim_path.to_string(),
                },
                CellCoord::ZERO,
                Transform::default(),
                ChildOf(grid),
            ))
            .id();
        app.world_mut()
            .resource_mut::<lunco_api::registry::ApiEntityRegistry>()
            .assign(ent, lunco_core::GlobalEntityId::from_raw(api_id));
        app.update();
        (app, doc)
    }

    #[test]
    fn move_of_authored_prim_persists_to_runtime_layer() {
        use super::*;
        use lunco_usd_bevy::usd_data::UsdDataExt;

        let (mut app, doc) = app_with_runtime_producer("/World", 42);
        app.world_mut().trigger(MoveEntity {
            entity_id: 42,
            translation: [3.0, 4.0, 5.0],
        });
        for _ in 0..3 {
            app.update();
        }

        let reg = app.world().resource::<DocumentRegistry<UsdDocument>>();
        let docu = reg.host(doc).expect("doc alive").document();
        let world = lunco_usd_bevy::SdfPath::new("/World").unwrap();
        // The move landed in the RUNTIME layer... (read via `UsdDataExt` on
        // purpose — WHICH LAYER holds the opinion is the whole assertion, and a
        // composed read cannot see that distinction.)
        assert_eq!(
            docu.runtime_data()
                .prim_attribute_value::<[f64; 3]>(&world, "xformOp:translate"),
            Some([3.0, 4.0, 5.0]),
            "authored-scene move persists to the runtime layer"
        );
        // ...and the base layer (what Save writes) stays clean.
        let attr = lunco_usd_bevy::SdfPath::new("/World.xformOp:translate").unwrap();
        assert!(docu.data().spec(&attr).is_none(), "base layer untouched");
        assert!(
            !docu.source().contains("xformOp:translate"),
            "save excludes runtime move"
        );
    }

    #[test]
    fn rotation_of_authored_prim_persists_parent_local_orientation() {
        use super::*;
        use lunco_usd_bevy::usd_data::UsdDataExt;

        let (mut app, doc) = app_with_runtime_producer("/World", 43);
        let requested = DQuat::from_rotation_x(0.2);
        app.world_mut().trigger(RotateEntity {
            entity_id: 43,
            rotation: requested.to_array(),
        });
        for _ in 0..3 {
            app.update();
        }

        let reg = app.world().resource::<DocumentRegistry<UsdDocument>>();
        let docu = reg.host(doc).expect("doc alive").document();
        let world = lunco_usd_bevy::SdfPath::new("/World").unwrap();
        let authored = docu
            .runtime_data()
            .prim_attribute_value::<[f64; 3]>(&world, "xformOp:rotateXYZ")
            .expect("rotation authored in runtime layer");
        assert!((authored[0] - 0.2_f64.to_degrees()).abs() < 1.0e-9);
        assert!(authored[1].abs() < 1.0e-9 && authored[2].abs() < 1.0e-9);
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
        let mut seen_name: HashSet<&str> = HashSet::new();
        let mut seen_attr: HashSet<&str> = HashSet::new();
        for p in WHEEL_PARAMS {
            assert!(!p.name.is_empty(), "a param with no name is unreachable");
            assert!(
                seen_name.insert(p.name),
                "duplicate property name {}",
                p.name
            );
            assert!(
                !p.usd_attr.is_empty(),
                "every param must round-trip through USD"
            );
            assert!(
                seen_attr.insert(p.usd_attr),
                "duplicate USD attr {}",
                p.usd_attr
            );
            // Both consumers (live setter + USD persister) resolve through the
            // same canonical lookup, so a name that sets a field always has an attr.
            let row = wheel_param(p.name).expect("wheel name resolves");
            assert_eq!(row.usd_attr, p.usd_attr);
        }

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
        use super::*;
        use lunco_doc::Document;
        use lunco_usd_bevy::usd_data::UsdDataExt;

        let (mut app, doc) = app_with_runtime_producer("/World", 42);
        // USD's half of the generic verb now lives in `lunco-usd` (see the note above
        // `handle_undo_input`), so the test wires the real observer from there.
        app.add_observer(lunco_usd::commands::on_undo_usd_document);
        app.world_mut().trigger(MoveEntity {
            entity_id: 42,
            translation: [3.0, 4.0, 5.0],
        });
        for _ in 0..3 {
            app.update();
        }
        let world_path = lunco_usd_bevy::SdfPath::new("/World").unwrap();
        let gen_after_move = {
            let reg = app.world().resource::<DocumentRegistry<UsdDocument>>();
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

        let reg = app.world().resource::<DocumentRegistry<UsdDocument>>();
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
        use super::*;
        use lunco_doc::Document;

        // Entity bound to a prim the document does NOT contain (e.g. a palette
        // spawn referencing an external asset).
        let (mut app, doc) = app_with_runtime_producer("/PaletteSpawn", 7);
        app.world_mut().trigger(MoveEntity {
            entity_id: 7,
            translation: [1.0, 2.0, 3.0],
        });
        for _ in 0..3 {
            app.update();
        }

        let reg = app.world().resource::<DocumentRegistry<UsdDocument>>();
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
    fn document_backed_spawn_is_one_atomic_usd_change() {
        use super::*;
        use lunco_usd_bevy::usd_data::UsdDataExt;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(lunco_usd::commands::UsdCommandsPlugin);
        let doc = {
            let mut reg = app
                .world_mut()
                .resource_mut::<DocumentRegistry<UsdDocument>>();
            reg.allocate(
                "#usda 1.0\n(\n    defaultPrim = \"World\"\n)\ndef Xform \"World\"\n{\n}\n"
                    .to_string(),
                lunco_doc::PathlessOrigin::untitled("Scene.usda"),
            )
        };
        let (prim_path, ops) = runtime_spawn_ops(
            "test_rover",
            "vessels/rovers/test_rover.usda",
            "/World",
            DVec3::new(2.0, 0.0, 7.0),
            DQuat::IDENTITY,
        );
        assert_eq!(ops.len(), 4, "spawn lowers to one complete change set");
        app.world_mut().trigger(ApplyUsdOps {
            doc,
            label: "Spawn Test Rover".into(),
            ops,
        });
        for _ in 0..3 {
            app.update();
        }

        let reg = app.world().resource::<DocumentRegistry<UsdDocument>>();
        let docu = reg.host(doc).expect("doc alive").document();
        let prim = lunco_usd_bevy::SdfPath::new(&prim_path).unwrap();
        // The referenced spawn prim landed under the default prim, in RUNTIME...
        assert!(
            docu.runtime_data().spec(&prim).is_some(),
            "spawn prim authored in runtime layer"
        );
        assert!(
            docu.data().spec(&prim).is_none(),
            "base layer untouched by spawn"
        );
        // `UsdDataExt` on purpose — see the layer-targeting note above.
        assert_eq!(
            docu.runtime_data()
                .prim_attribute_value::<[f64; 3]>(&prim, "xformOp:translate"),
            Some([2.0, 0.0, 7.0]),
            "spawn drop position recorded in runtime layer"
        );
        assert_eq!(
            docu.runtime_data()
                .prim_attribute_value::<String>(&prim, "lunco:catalogId"),
            Some("test_rover".to_string()),
            "spawn catalog identity must be authored with the runtime prim"
        );
        // ...rides into the composed view as a resolvable reference...
        let composed = docu.composed_source();
        assert!(
            composed.contains("@lunco://vessels/rovers/test_rover.usda@"),
            "composed view must carry the spawn reference:\n{composed}"
        );
        // ...and is excluded from Save (base only).
        assert!(
            !docu.source().contains("test_rover"),
            "spawn leaked into save:\n{}",
            docu.source()
        );
    }

    #[test]
    fn document_backed_spawn_normalizes_the_pseudo_root_parent() {
        use super::*;

        let (prim_path, ops) = runtime_spawn_ops(
            "test_rover",
            "vessels/rovers/test_rover.usda",
            "/",
            DVec3::ZERO,
            DQuat::IDENTITY,
        );

        assert!(prim_path.starts_with("/test_rover_"));
        assert!(matches!(
            &ops[0],
            UsdOp::AddPrim { parent_path, .. } if parent_path == "/"
        ));
    }
}
