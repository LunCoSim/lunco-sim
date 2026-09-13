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
use lunco_doc_bevy::DocumentRegistry;
use lunco_doc_bevy::{RedoDocument, UndoDocument};
use lunco_scene_catalog::catalog::{spawn_usd_entry, SpawnAnchor, SpawnCatalog, SpawnSource};
use lunco_usd::commands::{ApplyUsdOp, ApplyUsdOps};
use lunco_usd_bevy_scene::{UsdPrimPath, UsdSceneRoot};
use lunco_usd_core::document::UsdDocument;
use lunco_usd_core::document::{LayerId, UsdOp};
use openusd::schemas::lux::tokens as ltok;

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
    let Some((doc, path)) = lunco_scene_authoring::doc_resolve::authorable_prim(
        cmd.target,
        &q_prim,
        &usd_registry,
        workspace.as_deref(),
    ) else {
        return;
    };

    commands.trigger(ApplyUsdOp {
        doc_id: doc,
        parent_gen: None,
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
            reference_prim_path: None,
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
            doc_id: doc,
            parent_gen: None,
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

    // Networked identity (gap G2): `spawn_usd_entry` already carries the shared
    // runtime identity fence, so this caller only adds its replication contract
    // and the host's spawn journal. Keeping the fence in the constructor is
    // what makes palette spawns and authored runtime instances identical.
    commands.entity(result.root_entity).try_insert((
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
    // `asset_server`, never `pending`.
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
        // Pin the host id; the shared constructor already suppresses content
        // identity and marks the instance root. Forced Kinematic by
        // `force_kinematic_proxies` so snapshots drive it.
        commands.entity(result.root_entity).try_insert((
            lunco_core::GlobalEntityId::from_raw(job.gid),
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
    let Some((doc, path)) = lunco_scene_authoring::doc_resolve::authorable_prim(
        target,
        &q_prim,
        &usd_registry,
        workspace.as_deref(),
    ) else {
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
        doc_id: doc,
        parent_gen: None,
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
    let Some((doc, path)) = lunco_scene_authoring::doc_resolve::authorable_prim(
        target,
        &q_prim,
        &usd_registry,
        workspace.as_deref(),
    ) else {
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
        doc_id: doc,
        parent_gen: None,
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
    let Some((doc, path)) = lunco_scene_authoring::doc_resolve::authorable_prim(
        target,
        &q_prim,
        &usd_registry,
        workspace.as_deref(),
    ) else {
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
        doc_id: doc,
        parent_gen: None,
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
        commands.trigger(RedoDocument { doc_id: doc });
    } else {
        commands.trigger(UndoDocument { doc_id: doc });
    }
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
    let Ok(path) = openusd::sdf::Path::new(path) else {
        return false;
    };
    registry.host(doc).is_some_and(|host| {
        let composed = host.document().composed();
        lunco_usd_core::usd_data::has_authored_api_schema(
            &composed,
            &path,
            "LunCoMountAttachmentAPI",
        )
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
        let attachment = lunco_scene_authoring::doc_resolve::authorable_prim(
            cmd.target,
            &q_prim,
            registry,
            workspace.as_deref(),
        );
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
    let Some((doc, path)) = lunco_scene_authoring::doc_resolve::authorable_prim(
        cmd.target,
        &q_prim,
        &usd_registry,
        workspace.as_deref(),
    ) else {
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
        doc_id: doc,
        parent_gen: None,
        op: UsdOp::RemovePrim {
            edit_target: LayerId::runtime(),
            path,
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
    let Some((doc, path)) = lunco_scene_authoring::doc_resolve::authorable_prim(
        cmd.target,
        &q_prim,
        &usd_registry,
        workspace.as_deref(),
    ) else {
        return;
    };
    commands.trigger(ApplyUsdOp {
        doc_id: doc,
        parent_gen: None,
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

/// Persist a `SetEnvironmentLight` sun tweak into the active USD document's
/// runtime overlay — the environment twin of the shader-parameter authoring path.
///
/// [`lunco_environment::on_set_environment_light`] mutates the live
/// `DirectionalLight` for immediate feedback but writes nothing back to USD, so a
/// sun tweak is lost on reload. This decoupled observer authors the changed
/// fields as `SetAttribute`s onto the sun's `DistantLight` prim in
/// `LayerId::runtime()`, using the SAME attribute names the loader
/// (`lunco_usd_bevy_light::light`) already reads back — so illuminance / colour /
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
            With<lunco_usd_bevy_light::light::UsdAuthoredLight>,
            With<DirectionalLight>,
            Without<lunco_environment::Earthshine>,
        ),
    >,
    // The body fill, which now has a prim of its own to be written to — the
    // exact complement of `q_sun`, so no light is addressed twice.
    q_earthshine: Query<
        &UsdPrimPath,
        (
            With<lunco_usd_bevy_light::light::UsdAuthoredLight>,
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
        attrs.push((ltok::A_INTENSITY, "float", lux.to_string()));
    }
    if let Some([r, g, b]) = cmd.sun_color {
        attrs.push((ltok::A_COLOR, "color3f", format!("({r}, {g}, {b})")));
    }
    if let Some(v) = cmd.shadow_max_distance {
        // Standard `UsdLuxShadowAPI`, not an invented `lunco:` name.
        attrs.push((ltok::A_SHADOW_DISTANCE, "float", v.to_string()));
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

    let parent_path = lunco_usd_bevy_core::layer_default_prim(host.document().data())
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
        let fill_sdf = openusd::sdf::Path::new(&fill_path).ok();
        match lunco_usd_bevy_light::light::untextured_dome_intensity_sum(
            &composed,
            fill_sdf.as_ref(),
        ) {
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
        let Ok(prim_sdf) = openusd::sdf::Path::new(&prim.path) else {
            continue;
        };
        let owned = host.document().data().spec(&prim_sdf).is_some()
            || host.document().runtime_data().spec(&prim_sdf).is_some();
        if !owned {
            continue;
        }
        for (name, type_name, value) in &attrs {
            commands.trigger(ApplyUsdOp {
                doc_id: doc,
                parent_gen: None,
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
                doc_id: doc,
                parent_gen: None,
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
        fill_attrs.push((ltok::A_COLOR, "color3f", format!("({r}, {g}, {b})")));
    }
    if !fill_attrs.is_empty() {
        for prim in &q_earthshine {
            // Ownership guard, exactly as for the sun: only author onto fills the
            // active document actually holds.
            let Ok(prim_sdf) = openusd::sdf::Path::new(&prim.path) else {
                continue;
            };
            let owned = host.document().data().spec(&prim_sdf).is_some()
                || host.document().runtime_data().spec(&prim_sdf).is_some();
            if !owned {
                continue;
            }
            for (name, type_name, value) in &fill_attrs {
                commands.trigger(ApplyUsdOp {
                    doc_id: doc,
                    parent_gen: None,
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
        !openusd::sdf::Path::new(path)
            .ok()
            .map(|sdf| {
                host.document().data().spec(&sdf).is_some()
                    || host.document().runtime_data().spec(&sdf).is_some()
            })
            .unwrap_or(false)
    };
    if prim_missing(&env_path) {
        commands.trigger(ApplyUsdOp {
            doc_id: doc,
            parent_gen: None,
            op: UsdOp::AddPrim {
                edit_target: LayerId::runtime(),
                parent_path,
                name: "Environment".to_string(),
                type_name: Some(lunco_environment::LUNCO_ENVIRONMENT_PRIM_TYPE.to_string()),
                reference: None,
                reference_prim_path: None,
            },
        });
    }
    for (name, type_name, value) in &env_attrs {
        commands.trigger(ApplyUsdOp {
            doc_id: doc,
            parent_gen: None,
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
    // spelling, and `lunco_usd_bevy_light::light::on_usd_light_added` composes
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
        let intensity = lunco_usd_bevy_light::light::ambient_fill_intensity(requested, others);

        if lunco_usd_bevy_light::light::ambient_fill_saturates(requested, others) {
            warn!(
                "[scene-commands] ambient {requested} is below the {others} already \
                 contributed by other authored DomeLights; `{fill_path}` clamped to 0 \
                 and the scene will stay brighter than requested. Lower those domes' \
                 `inputs:intensity` instead."
            );
        }

        if prim_missing(&fill_path) {
            commands.trigger(ApplyUsdOp {
                doc_id: doc,
                parent_gen: None,
                op: UsdOp::AddPrim {
                    edit_target: LayerId::runtime(),
                    parent_path: env_path,
                    name: "AmbientFill".to_string(),
                    type_name: Some(ltok::T_DOME_LIGHT.to_string()),
                    reference: None,
                    reference_prim_path: None,
                },
            });
        }
        commands.trigger(ApplyUsdOp {
            doc_id: doc,
            parent_gen: None,
            op: UsdOp::SetAttribute {
                edit_target: LayerId::runtime(),
                path: fill_path,
                name: ltok::A_INTENSITY.to_string(),
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
/// (see [`lunco_usd_core::material::preview_surface_input`]) — it is the one knob a saved
/// scene will not carry, deliberately.
fn author_look_to_usd(commands: &mut Commands, target: Entity, key: &str, look: &PbrLook) {
    let look = look.clone();
    let key = key.to_string();
    commands.queue(move |world: &mut World| {
        let Some(doc) = lunco_scene_authoring::doc_resolve::resolve_doc_for_entity(world, target) else {
            return;
        };
        let Some(prim) = world.get::<UsdPrimPath>(target).cloned() else {
            return;
        };

        // `doubleSided` lives on the geometry, not the surface.
        if key == "double_sided" {
            world.trigger(ApplyUsdOp {
                doc_id: doc,
                parent_gen: None,
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
        if lunco_usd_core::material::preview_surface_input(&key).is_none() {
            return; // `unlit` — render-only intent, no USD surface input to write.
        }

        // An existing bound shader, else create the material.
        let existing = lunco_scene_authoring::doc_resolve::bound_shader_prim(world, &prim);
        let (mut ops, shader, fresh) = match existing {
            Some(sp) => (Vec::new(), sp, false),
            None => {
                let schemas = lunco_scene_authoring::doc_resolve::geom_api_schemas(world, &prim);
                match lunco_usd_core::material::ensure_preview_surface_ops(
                    LayerId::root(),
                    &prim.path,
                    &schemas,
                ) {
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
            if let Some((attr, _)) = lunco_usd_core::material::preview_surface_input(k) {
                set(attr, ty, v);
            }
        }
        for op in ops {
            world.trigger(ApplyUsdOp {
                doc_id: doc,
                parent_gen: None,
                op: op.clone(),
            });
        }
    });
}

/// Whether the edited look key names the same `UsdPreviewSurface` input as `slot`
/// (`roughness` and `alpha` are the canonical command keys).
fn key_matches(key: &str, slot: &str) -> bool {
    lunco_usd_core::material::preview_surface_input(key)
        == lunco_usd_core::material::preview_surface_input(slot)
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

/// Queue the authored leg of a reflected shader-parameter edit.
///
/// `SetObjectProperty` owns the public command and immediate intent update;
/// this deferred leg resolves the entity's explicit USD document and bound
/// Shader, then submits the same typed USD operation used by the Inspector.
/// Entities without a document remain session-only because they have no USD
/// owner for persistence.
fn author_shader_parameter_to_usd(
    commands: &mut Commands,
    target: Entity,
    name: String,
    value: ParamValue,
) {
    commands.queue(move |world: &mut World| {
        let Some(prim) = world.get::<UsdPrimPath>(target).cloned() else {
            return;
        };
        let Some(doc) = lunco_scene_authoring::doc_resolve::resolve_doc_for_entity(world, target) else {
            return;
        };
        let target = match lunco_scene_authoring::doc_resolve::resolve_shader_parameter_usd_target(
            world, &prim, &name, &value,
        ) {
            Ok(target) => target,
            Err(error) => {
                warn!(
                    "SET_PROPERTY: shader parameter '{}' was not authored: {error}",
                    name
                );
                return;
            }
        };
        world.trigger(ApplyUsdOp {
            doc_id: doc,
            parent_gen: None,
            op: UsdOp::SetAttribute {
                edit_target: LayerId::runtime(),
                path: target.shader_path,
                name: target.attribute_name,
                type_name: target.type_name,
                value: target.literal,
            },
        });
    });
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
            let authored = {
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
                        look.values.insert(name.clone(), v);
                        Some((name, v))
                    }
                    None => {
                        warn!("SET_PROPERTY: unknown property '{}'", key);
                        None
                    }
                }
            };
            if let Some((name, value)) = authored {
                author_shader_parameter_to_usd(&mut commands, target, name, value);
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
/// This is separate from the editor's `SelectUsdPrim`: a headless
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
    // view grid with current-region arrival). Local framing stays for
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

/// Plugin that registers scene mutation, shader authoring, and camera command
/// observers. Catalog resources, discovery, and catalog-only rescan commands
/// are installed by [`lunco_scene_catalog::SceneCatalogPlugin`].
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
    on_delete_entity,
    on_detach_joint,
    on_focus_entity_by_id,
    on_focus_entity_by_path,
    on_move_entity_command,
    on_rotate_entity_command,
    on_set_camera_look_at,
    on_set_usd_connection,
    on_spawn_entity_command,
    on_step_physics,
    on_transform_entity_command,
);

impl Plugin for SpawnCommandPlugin {
    fn build(&self, app: &mut App) {
        // Catalog discovery is a separate production package. Its plugin is
        // added here because every scene command host needs SpawnEntity's
        // authoritative catalog, but its systems/resources remain owned there.
        app.add_plugins(lunco_scene_catalog::SceneCatalogPlugin);
        // Property and shader authoring is a sibling production package. It
        // owns its command registration, persistence, and shader journal bridge.
        app.add_plugins(lunco_scene_authoring::SceneAuthoringPlugin);
        // Every `#[Command]` this crate owns — type + observer in one call, so a
        // verb is available consistently through the HTTP API, Rhai, and
        // `discover_schema`.
        register_all_commands(app);
        // The read-only scene surface is a separate production package, so query
        // changes do not rebuild this much larger mutation layer.
        app.add_plugins(lunco_scene_queries::SceneQueryPlugin);
        // Selection → telemetry focus, so every host that has the scene verbs has
        // scoped telemetry (the sandbox, the workbench, a headless server driven
        // by `SelectEntity`). Render-free: `lunco-signal` is a ring buffer of
        // f64s, not a UI. See `crate::mirror_selection_to_telemetry_focus`.
        app.init_resource::<crate::SelectedEntities>();
        app.init_resource::<lunco_signal::TelemetryFocus>();
        app.add_systems(Update, crate::mirror_selection_to_telemetry_focus);
        // Persist a Persistent DetachJoint into the active doc's runtime layer.
        app.add_observer(persist_detach_to_runtime_layer);
        app.add_observer(persist_delete_to_runtime_layer);
        // C4b: persist authored-scene moves into the active doc's runtime layer.
        app.add_observer(persist_move_to_runtime_layer);
        app.add_observer(persist_rotation_to_runtime_layer);
        app.add_observer(persist_transform_to_runtime_layer);
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
        app.add_systems(FixedPostUpdate, clear_kinematic_pulse_velocity);
        // Resources this plugin's OWN systems read, so it stands alone without the
        // UI-layer `SceneEditPlugin` / the render-layer `ShaderMaterialPlugin`
        // (e.g. a headless `--no-ui` server that adds only `SpawnCommandPlugin`).
        // The host must install `lunco_assets::register_lunco_asset_sources`
        // before Bevy's asset plugin; that shared asset boundary owns the
        // `AssetManifest` and `TwinRoots` resources consumed here.
        // `init_resource` is idempotent, so when those plugins also init these it's
        // a harmless no-op:
        //   - `SpawnCatalog`   — read by the catalog and `apply_replicated_spawns`;
        //   - `SelectedEntity` — read by `on_select_entity`;
        //   - `ShaderCatalog`  — owned and populated by `SceneCatalogPlugin`;
        //     shader command observers consume it but do not scan it themselves.
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
        // Client: instantiate host-replicated spawns before prediction consumes
        // the resulting entities. The shared `lunco_core::NetcodeSet` preserves
        // this ordering across the crate boundary.
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
        app.init_resource::<lunco_core::CommandResults>()
            .init_resource::<lunco_core::ActiveCommandId>();
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
        use lunco_usd_core::UsdDataExt;

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
        let world = openusd::sdf::Path::new("/World").unwrap();
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
        let attr = openusd::sdf::Path::new("/World.xformOp:translate").unwrap();
        assert!(docu.data().spec(&attr).is_none(), "base layer untouched");
        assert!(
            !docu.source().contains("xformOp:translate"),
            "save excludes runtime move"
        );
    }

    #[test]
    fn rotation_of_authored_prim_persists_parent_local_orientation() {
        use super::*;
        use lunco_usd_core::UsdDataExt;

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
        let world = openusd::sdf::Path::new("/World").unwrap();
        let authored = docu
            .runtime_data()
            .prim_attribute_value::<[f64; 3]>(&world, "xformOp:rotateXYZ")
            .expect("rotation authored in runtime layer");
        assert!((authored[0] - 0.2_f64.to_degrees()).abs() < 1.0e-9);
        assert!(authored[1].abs() < 1.0e-9 && authored[2].abs() < 1.0e-9);
    }

    // ── A10: ONE wheel-param table ──────────────────────────────────────

    /// The whole point of collapsing the two hand-synced tables: a wheel
    /// Ctrl+Z routes to `UndoDocument`, which pops the USD document's last op
    /// and applies its inverse. The editor keeps no private undo stack, so the
    /// journal and the editor can no longer disagree.
    #[test]
    fn undo_document_reverts_the_last_usd_op() {
        use super::*;
        use lunco_doc::Document;
        use lunco_usd_core::UsdDataExt;

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
        let world_path = openusd::sdf::Path::new("/World").unwrap();
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
        app.world_mut().trigger(UndoDocument { doc_id: doc });
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
            .spec(&openusd::sdf::Path::new("/PaletteSpawn").unwrap())
            .is_none());
    }

    // ── C4b: spawn → referenced runtime-layer prim ──────────────────────

    #[test]
    fn document_backed_spawn_is_one_atomic_usd_change() {
        use super::*;
        use lunco_usd_core::UsdDataExt;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(lunco_usd::commands::UsdCommandsPlugin);
        app.init_resource::<lunco_core::CommandResults>()
            .init_resource::<lunco_core::ActiveCommandId>();
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
            doc_id: doc,
            parent_gen: None,
            label: "Spawn Test Rover".into(),
            ops,
        });
        for _ in 0..3 {
            app.update();
        }

        let reg = app.world().resource::<DocumentRegistry<UsdDocument>>();
        let docu = reg.host(doc).expect("doc alive").document();
        let prim = openusd::sdf::Path::new(&prim_path).unwrap();
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
