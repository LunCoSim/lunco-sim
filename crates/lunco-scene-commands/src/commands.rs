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
use lunco_core::{on_command, register_commands, Ack, Command, OpId, SpawnEntity};
use lunco_doc_bevy::DocumentRegistry;
use lunco_doc_bevy::{RedoDocument, UndoDocument};
use lunco_scene_catalog::catalog::{spawn_usd_entry, SpawnAnchor, SpawnCatalog, SpawnSource};
use lunco_scene_selection::SelectedEntities;
use lunco_usd_bevy_scene::{UsdPrimPath, UsdSceneRoot};
use lunco_usd_core::commands::{ApplyUsdOp, ApplyUsdOps};
use lunco_usd_document::document::UsdDocument;
use lunco_usd_document::document::{LayerId, UsdOp};
use openusd::schemas::lux::tokens as ltok;

/// Select one live scene entity through the render-free shared selection
/// resource. Editor packages may add highlights or gizmos, but authored tools
/// only need this canonical entity selection so a later pointer context can
/// carry the selected USD path in both interactive and headless runs.
#[Command(default)]
pub struct SelectSceneEntity {
    /// API-stable global entity ID from the live entity registry. `0` clears.
    pub entity_id: u64,
    /// Retain the current selection and add this entity.
    pub extend: bool,
    /// Toggle this entity in the current selection.
    pub toggle: bool,
    /// Remove this entity without adding it.
    pub remove_only: bool,
}

#[on_command(SelectSceneEntity)]
pub fn on_select_scene_entity(
    trigger: On<SelectSceneEntity>,
    registry: Res<lunco_api::registry::ApiEntityRegistry>,
    selected: Option<ResMut<SelectedEntities>>,
    q_paths: Query<&UsdPrimPath>,
) {
    let Some(mut selected) = selected else {
        return;
    };
    let command = trigger.event();
    if command.entity_id == 0 {
        selected.entities.clear();
        selected.stable_paths.clear();
        return;
    }
    let Some(target) = registry.resolve(&lunco_core::GlobalEntityId::from_raw(command.entity_id))
    else {
        return;
    };
    let path = q_paths.get(target).ok().map(|prim| prim.path.clone());
    if command.remove_only {
        selected.entities.retain(|entity| *entity != target);
        if let Some(path) = path {
            selected.stable_paths.retain(|selected| selected != &path);
        }
    } else if command.toggle {
        if selected.entities.contains(&target) {
            selected.entities.retain(|entity| *entity != target);
            if let Some(path) = path {
                selected.stable_paths.retain(|selected| selected != &path);
            }
        } else {
            selected.entities.push(target);
            if let Some(path) = path {
                if !selected
                    .stable_paths
                    .iter()
                    .any(|selected| selected == &path)
                {
                    selected.stable_paths.push(path);
                }
            }
        }
    } else if command.extend {
        if !selected.entities.contains(&target) {
            selected.entities.push(target);
        }
        if let Some(path) = path {
            if !selected
                .stable_paths
                .iter()
                .any(|selected| selected == &path)
            {
                selected.stable_paths.push(path);
            }
        }
    } else {
        selected.entities.clear();
        selected.entities.push(target);
        selected.stable_paths.clear();
        if let Some(path) = path {
            selected.stable_paths.push(path);
        }
    }
}

/// Re-resolve stable scene selection paths after USD replaces their ECS
/// projections. The selection command owns the requested paths; this generic
/// bridge owns only the disposable entity realization.
fn reconcile_stable_scene_selection(
    mut selected: ResMut<SelectedEntities>,
    stage_revision: Res<lunco_usd_bevy_scene::UsdStageRevision>,
    q_paths: Query<(Entity, &UsdPrimPath)>,
) {
    if !selected.is_changed() && !stage_revision.is_changed() {
        return;
    }
    let requested = selected.stable_paths.clone();
    if requested.is_empty() {
        return;
    }
    let mut resolved = Vec::with_capacity(requested.len());
    for path in requested {
        if let Some((entity, _)) = q_paths.iter().find(|(_, prim)| prim.path == path) {
            resolved.push(entity);
        }
    }
    selected.entities = resolved;
}

/// Request an entity detachment (joint-aware when joint state is present).
#[Command(reflect_default)]
pub struct DetachJoint {
    /// Entity to detach. Joint state, when present, is retired through the
    /// physics lifecycle; ordinary entities use normal removal.
    pub target: Entity,
    /// Persistent (default) authors the joint's removal into the scene's runtime
    /// layer — removing runtime-only prims and deactivating base/composed prims
    /// — so it journals, syncs, and survives reload. Interactive is a throwaway
    /// live operation with no journal. See [`lunco_core::EditIntent`]. Omitted
    /// by API callers → `Persistent`.
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

/// Observer that handles `DetachJoint` commands.
///
/// This is a generic entity-detach verb, not a `PhysicsJointLink` allow-list:
/// an ordinary entity is removed directly, while an entity carrying any joint
/// state is routed through the physics lifecycle marker.  The USD/Avian bridge
/// owns that joint teardown path.  Keeping native-joint despawn out of this
/// command layer is important: Bevy's recursive despawn removes joint
/// components in an unspecified order, while Avian requires graph/island
/// retirement first.
#[on_command(DetachJoint)]
pub fn on_detach_joint(
    trigger: On<DetachJoint>,
    mut commands: Commands,
    q_joint: Query<(
        Option<&lunco_physics::PhysicsJointLink>,
        Option<&UsdPrimPath>,
        Option<&lunco_physics::PhysicsJointDetachRequested>,
        Option<&lunco_physics::PhysicsJointPending>,
        Option<&avian3d::dynamics::solver::joint_graph::JointComponentId>,
    )>,
    q_detached: Query<Option<&lunco_physics::PhysicsJointDetachSet>>,
) -> Result<Ack, String> {
    let cmd = trigger.event();
    let (link, path, already_requested, pending, native) = q_joint
        .get(cmd.target)
        .ok()
        .map(|(link, path, requested, pending, native)| {
            (
                link.copied(),
                path.map(|path| path.path.clone()),
                requested.is_some(),
                pending.is_some(),
                native.is_some(),
            )
        })
        .unwrap_or((None, None, false, false, false));
    let Some(mut entity) = commands.get_entity(cmd.target).ok() else {
        warn!(
            "DETACH_JOINT rejected: target {:?} does not exist",
            cmd.target
        );
        return Err(format!("target {:?} does not exist", cmd.target));
    };
    if already_requested {
        warn!(
            "DETACH_JOINT ignored: target {:?} already has a pending detach",
            cmd.target
        );
        return Ok(Ack::new(OpId::new()));
    }
    let path = path.unwrap_or_default();
    let Some(link) = link else {
        if pending || native {
            warn!(
                "DETACH_JOINT rejected: target {:?} has joint state but no PhysicsJointLink; repair attachment topology before detaching",
                cmd.target
            );
            return Err(format!(
                "target {:?} has joint state but no PhysicsJointLink; repair attachment topology before detaching",
                cmd.target
            ));
        }
        entity.try_despawn();
        info!(
            "DETACH_JOINT: removed ordinary non-joint entity {:?} ({:?})",
            cmd.target, cmd.intent
        );
        return Ok(Ack::new(OpId::new()));
    };
    {
        entity.try_insert(lunco_physics::PhysicsJointDetachRequested);
        info!(
            "DETACH_JOINT: queued solver-safe retirement for {:?} ({:?})",
            cmd.target, cmd.intent
        );
    }
    if !path.is_empty() {
        // The marker is written to both endpoints before the lifecycle owner
        // retires the joint. Admission can then release exactly this authored
        // topology edge without promoting a body that still has another
        // unresolved joint. Keep the existing endpoint list when several
        // joints release.
        for body in [link.body0, link.body1] {
            let mut detached = q_detached
                .get(body)
                .ok()
                .flatten()
                .cloned()
                .unwrap_or_default();
            detached.record(path.clone());
            if let Ok(mut entity) = commands.get_entity(body) {
                entity.try_insert(detached);
            }
        }
    }
    Ok(Ack::new(OpId::new()))
}

/// Persist a **`Persistent`** `DetachJoint` into the active USD document's
/// runtime overlay. Runtime-only joints are removed; a joint supplied by the
/// base scene or a composition arc receives the same standard active=false
/// override used by generic entity deletion. `Interactive` detaches are
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
    let Some((doc, path, target)) = lunco_scene_authoring::doc_resolve::delete_target(
        cmd.target,
        &q_prim,
        &usd_registry,
        workspace.as_deref(),
    ) else {
        return;
    };

    let op = match target {
        lunco_scene_authoring::doc_resolve::DeleteTarget::RemoveRuntime => UsdOp::RemovePrim {
            edit_target: LayerId::runtime(),
            path,
        },
        lunco_scene_authoring::doc_resolve::DeleteTarget::DeactivateRuntime => UsdOp::SetActive {
            edit_target: LayerId::runtime(),
            path,
            active: false,
        },
    };
    commands.trigger(ApplyUsdOp {
        doc_id: doc,
        parent_gen: None,
        op,
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
            reference: Some(lunco_assets_core::engine_asset_uri(asset_path)),
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
    active_frame: Res<lunco_spatial::ActivePhysicsFrame>,
    q_scene_root: Query<(Entity, &UsdPrimPath), With<UsdSceneRoot>>,
    q_parents: Query<&ChildOf>,
    q_grids: Query<&Grid>,
    q_spatial: Query<(Option<&CellCoord>, &Transform)>,
    role: Res<lunco_core_session::NetworkRole>,
    backed: Res<lunco_usd_bevy_twin::DocBackedTwinScenes>,
) {
    let cmd = trigger.event();

    // On a pure client, spawning is the host's job: the command is captured and
    // sent to the host, which spawns the authoritative rover and replicates it
    // back (arriving via `apply_replicated_spawns`). Don't spawn locally, or the
    // client would get a duplicate with no server identity.
    if matches!(*role, lunco_core_session::NetworkRole::Client) {
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
    let Some((position, rotation)) = lunco_spatial::coords::pose_in_parent_local(
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
    if let Some(doc) = lunco_usd_bevy_twin::scene_document_for(
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
        lunco_core_session::NetReplicate,
        lunco_core_session::NetSpawn {
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
    mut pending: ResMut<lunco_core_session::PendingReplicatedSpawns>,
    mut commands: Commands,
    catalog: Res<SpawnCatalog>,
    asset_server: Res<AssetServer>,
    active_frame: Res<lunco_spatial::ActivePhysicsFrame>,
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
            lunco_spatial::coords::pose_in_grid_to_parent_storage(
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
            lunco_core_session::NetReplicate,
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
    /// Target translation in the semantic [`lunco_spatial::ActivePhysicsFrame`].
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
    active_frame: Res<lunco_spatial::ActivePhysicsFrame>,
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
        let Some((prev_abs, _)) = lunco_spatial::coords::pose_in_grid(
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
        let Some((new_cell, new_local)) = lunco_spatial::coords::position_in_grid_to_parent_local(
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
/// The public quaternion is expressed in [`lunco_spatial::ActivePhysicsFrame`], the
/// same semantic frame as `MoveEntity`. Rotation is not frame-invariant: a
/// rotating body Grid and a rotated assembly parent both change the local
/// quaternion that must be stored on the entity. The observer performs that
/// hierarchy conversion once.
///
/// Written through `Transform`, never through avian's `Rotation`, for exactly
/// the reason `MoveEntity` never hand-writes `Position`:
/// `lunco-usd_avian_core::PhysicsBridgeSystems::Read` detects the external
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
    active_frame: Res<lunco_spatial::ActivePhysicsFrame>,
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
        let Some((_, previous_rotation)) = lunco_spatial::coords::pose_in_grid(
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
        let Some(local_rotation) = lunco_spatial::coords::rotation_in_grid_to_parent_local(
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
    active_frame: Res<lunco_spatial::ActivePhysicsFrame>,
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
        let Some((previous_position, previous_rotation)) = lunco_spatial::coords::pose_in_grid(
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
            lunco_spatial::coords::position_in_grid_to_parent_local(
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
        let Some(new_rotation) = lunco_spatial::coords::rotation_in_grid_to_parent_local(
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
    active_frame: Res<lunco_spatial::ActivePhysicsFrame>,
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
    let Some((cell, local_translation)) = lunco_spatial::coords::position_in_grid_to_parent_local(
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
    let Some(local_rotation) = lunco_spatial::coords::rotation_in_grid_to_parent_local(
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
    active_frame: Res<lunco_spatial::ActivePhysicsFrame>,
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

    let Some((cell, local)) = lunco_spatial::coords::position_in_grid_to_parent_local(
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
    active_frame: Res<lunco_spatial::ActivePhysicsFrame>,
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
    let Some(local) = lunco_spatial::coords::rotation_in_grid_to_parent_local(
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
        lunco_usd_data::usd_data::has_authored_api_schema(
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
/// The typed verb for "remove this" authors a journaled, replicated, undoable
/// runtime-layer edit. Runtime-only prims use `RemovePrim`; base-authored and
/// referenced prims use a stronger `active = false` override so the base scene
/// and referenced asset remain intact.
///
/// This despawns AND (via [`persist_delete_to_runtime_layer`]) authors the
/// corresponding USD edit, which is what makes deletion journaled and undoable.
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
    selected: Option<ResMut<SelectedEntities>>,
    usd_registry: Option<Res<DocumentRegistry<UsdDocument>>>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    q_prim: Query<&UsdPrimPath>,
    q_joint_state: Query<
        (),
        Or<(
            With<lunco_physics::PhysicsJointLink>,
            With<lunco_physics::PhysicsJointPending>,
            With<avian3d::dynamics::solver::joint_graph::JointComponentId>,
        )>,
    >,
    mut commands: Commands,
) -> Result<Ack, String> {
    let cmd = trigger.event();
    if q_joint_state.contains(cmd.target) {
        warn!(
            "DELETE_ENTITY rejected for joint {:?}; use DetachJoint so the physics bridge retires the solver edge",
            cmd.target
        );
        return Err(format!(
            "target {:?} is a joint; use DetachJoint so the physics bridge retires the solver edge",
            cmd.target
        ));
    }
    let Some(mut entity) = commands.get_entity(cmd.target).ok() else {
        warn!(
            "DELETE_ENTITY rejected: target {:?} does not exist",
            cmd.target
        );
        return Err(format!("target {:?} does not exist", cmd.target));
    };
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
                return Err(format!(
                    "target {path} is an attached component; use DetachComponent"
                ));
            }
        }
    }
    let deleted_path = q_prim.get(cmd.target).ok().map(|prim| prim.path.clone());
    entity.try_despawn();
    if let Some(mut selected) = selected {
        selected.entities.retain(|e| *e != cmd.target);
        if let Some(path) = deleted_path {
            selected.stable_paths.retain(|selected| selected != &path);
        }
    }
    Ok(Ack::new(OpId::new()))
}

/// Authoring leg: apply the generic USD delete edit, so the deletion persists,
/// journals, replicates, and undoes. Same shape as every other `persist_*`
/// observer.
pub fn persist_delete_to_runtime_layer(
    trigger: On<DeleteEntity>,
    usd_registry: Res<DocumentRegistry<UsdDocument>>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    q_prim: Query<&UsdPrimPath>,
    q_joint_state: Query<
        (),
        Or<(
            With<lunco_physics::PhysicsJointLink>,
            With<lunco_physics::PhysicsJointPending>,
            With<avian3d::dynamics::solver::joint_graph::JointComponentId>,
        )>,
    >,
    mut commands: Commands,
) {
    let cmd = trigger.event();
    if q_joint_state.contains(cmd.target) {
        warn!(
            "DELETE_ENTITY persistence rejected for joint {:?}; use DetachJoint",
            cmd.target
        );
        return;
    }
    if !cmd.intent.is_persistent() {
        return;
    }
    let Some((doc, path, target)) = lunco_scene_authoring::doc_resolve::delete_target(
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
    let op = match target {
        lunco_scene_authoring::doc_resolve::DeleteTarget::RemoveRuntime => UsdOp::RemovePrim {
            edit_target: LayerId::runtime(),
            path,
        },
        lunco_scene_authoring::doc_resolve::DeleteTarget::DeactivateRuntime => UsdOp::SetActive {
            edit_target: LayerId::runtime(),
            path,
            active: false,
        },
    };
    commands.trigger(ApplyUsdOp {
        doc_id: doc,
        parent_gen: None,
        op,
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

// Property and shader authoring is owned by lunco-scene-authoring. Keeping
// that contract in one package prevents duplicate command types and readers.
/// Plugin that registers scene mutation and shader authoring command
/// observers. Catalog resources, discovery, and catalog-only rescan commands
/// are installed by [`lunco_scene_catalog::SceneCatalogPlugin`]. Camera
/// commands are installed by the sibling `lunco-scene-camera` package.
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
    on_move_entity_command,
    on_rotate_entity_command,
    on_set_usd_connection,
    on_spawn_entity_command,
    on_step_physics,
    on_select_scene_entity,
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
        app.add_systems(Update, reconcile_stable_scene_selection);
        // The read-only scene surface is a separate production package, so query
        // changes do not rebuild this much larger mutation layer.
        app.add_plugins(lunco_scene_queries::SceneQueryPlugin);
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
        // NOTE: `SelectEntity`/`on_select_entity` are editor-only (they drive the
        // Inspector highlight + gizmo) and live in the `ui`-gated `selection`
        // module; `SceneEditPlugin` registers them. The headless server has no
        // selection, so they're absent here by design.
        app.add_systems(FixedPostUpdate, clear_kinematic_pulse_velocity);
        // Resources this plugin's OWN systems read, so it stands alone without the
        // UI-layer `SceneEditPlugin` / the render-layer `ShaderMaterialPlugin`
        // (e.g. a headless `--no-ui` server that adds only `SpawnCommandPlugin`).
        // The host must install `lunco_assets_core::register_lunco_asset_sources`
        // before Bevy's asset plugin; that shared asset boundary owns the
        // `AssetManifest` and `TwinRoots` resources consumed here.
        // `init_resource` is idempotent, so when those plugins also init these it's
        // a harmless no-op:
        //   - `SpawnCatalog`   — read by the catalog and `apply_replicated_spawns`;
        //   - `ShaderCatalog`  — owned and populated by `SceneCatalogPlugin`;
        //     shader command observers consume it but do not scan it themselves.
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
    fn detach_joint_remains_generic_for_ordinary_entities() {
        use super::*;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<lunco_core::CommandResults>()
            .init_resource::<lunco_core::ActiveCommandId>();
        app.add_observer(on_detach_joint);

        let entity = app.world_mut().spawn_empty().id();
        app.world_mut().trigger(DetachJoint {
            target: entity,
            intent: lunco_core::EditIntent::Interactive,
        });
        app.update();

        assert!(
            app.world().get_entity(entity).is_err(),
            "generic DetachJoint must not require PhysicsJointLink for an ordinary entity"
        );
    }

    #[test]
    fn malformed_joint_detach_is_rejected_without_despawn_or_panic() {
        use super::*;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<lunco_core::CommandResults>()
            .init_resource::<lunco_core::ActiveCommandId>();
        app.add_observer(on_detach_joint);

        // A pending/native joint without the link is malformed topology. The
        // command must leave it intact for the authored linter to diagnose;
        // direct despawn would bypass the solver's graph lifecycle.
        let malformed = app
            .world_mut()
            .spawn(lunco_physics::PhysicsJointPending)
            .id();
        app.world_mut()
            .resource_mut::<lunco_core::ActiveCommandId>()
            .set(Some(41));
        app.world_mut().trigger(DetachJoint {
            target: malformed,
            intent: lunco_core::EditIntent::Interactive,
        });
        app.update();

        assert!(app.world().get_entity(malformed).is_ok());
        assert!(matches!(
            app.world()
                .resource::<lunco_core::CommandResults>()
                .get(41),
            Some(lunco_core::CommandOutcome::Failed(message))
                if message.contains("PhysicsJointLink")
        ));
    }

    #[test]
    fn spawn_pose_is_converted_to_scene_root_axes_once() {
        use super::*;
        use bevy::ecs::system::SystemState;
        use big_space::prelude::{CellCoord, Grid};

        let mut world = World::new();
        let active_grid = lunco_spatial::WorldGridConfig::default().grid();
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
        let (root_position, stored_root_rotation) = lunco_spatial::coords::grid_relative_pose(
            scene_root, active, &parents, &grids, &spatial,
        )
        .expect("scene root pose is available in the active physics frame");
        let stored_root_rotation = stored_root_rotation.normalize();
        assert!(
            root_position.length() > 1.0e5,
            "cell translation was not composed"
        );
        let requested_position = root_position + stored_root_rotation * expected_local;
        let requested_rotation = stored_root_rotation * expected_rotation;
        let (actual_position, actual_rotation) = lunco_spatial::coords::pose_in_parent_local(
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
        app.insert_resource(lunco_spatial::ActivePhysicsFrame(grid));
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
        app.insert_resource(lunco_spatial::ActivePhysicsFrame(grid));
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
        app.insert_resource(lunco_spatial::ActivePhysicsFrame(grid));

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
        app.insert_resource(lunco_spatial::ActivePhysicsFrame(grid));

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
                lunco_spatial::WorldGridConfig::default().grid(),
                GlobalTransform::default(),
            ))
            .id();
        app.insert_resource(lunco_spatial::ActivePhysicsFrame(active));
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
            lunco_spatial::coords::pose_in_grid(body, active, &parents, &grids, &spatial)
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
                lunco_spatial::WorldGridConfig::default().grid(),
                GlobalTransform::default(),
            ))
            .id();
        app.insert_resource(lunco_spatial::ActivePhysicsFrame(active));
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
