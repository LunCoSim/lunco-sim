//! Scene-transition and USD scene-mount mechanics.
//!
//! This module owns the scene lifecycle's entity and stage mechanics. The
//! parent plugin composes these systems with participant discovery, wiring, and
//! readiness; callers use the explicit scene API for mounting and teardown.

use super::*;
use lunco_core::{on_command, register_commands, Command};

/// Reload (or load) a USD scene at runtime via the API.
///
/// `curl … {"type":"ExecuteCommand","command":"LoadScene","params":{"path":"lunco://scenes/luncosim/sandbox_scene.usda"}}`
///
/// - `path`: root-qualified USD address (`lunco://…` or `twin://…`).
/// - `root_prim`: optional override for the SDF path of the prim to
///   spawn. Empty (default) reads the stage's `defaultPrim` metadata;
///   if absent, the scene load fails visibly; a whole-stage `/` mount is not a
///   valid scene root.
///
/// Despawns every existing entity carrying `UsdPrimPath` plus every
/// `SimConnection` (cosim wires are scene-derived in current code), then
/// reloads the asset from disk and spawns a fresh root entity. Existing
/// pipelines (`sync_usd_visuals`, `process_usd_cosim_prims`, the
/// avian/sim translators) take it from there. The canonical `WorldGrid`
/// is used as the parent — i.e. the `BigSpace` host stays put across
/// reloads. Invalid world-shell topology is reported rather than repaired
/// or resolved by entity order.
///
/// Cleans up worker-side state too: sends `ModelicaCommand::Despawn`
/// for every entity carrying a `ModelicaModel` (the Modelica worker
/// drops its `steppers` / `cached_models` / `sim_streams` entries). Scene-owned
/// Rhai documents are stopped and closed by the shared `SceneTeardown` owner;
/// independent API/editor documents remain open until their explicit close.
/// Without these ownership boundaries, repeated reloads accumulate stale
/// workers or make an unrelated interactive document disappear.
#[Command(default)]
pub struct LoadScene {
    /// Root-qualified USD address (`lunco://…` or `twin://…`). Filesystem paths
    /// are opened through `OpenFile`, not this scene-mount command.
    pub path: String,
    /// Optional override for the prim to spawn. Empty (default) reads
    /// `defaultPrim` from the stage's metadata header. A missing `defaultPrim`
    /// is a visible scene-load error; the runtime never mounts `/`.
    pub root_prim: String,
}

// The `LoadScene` OBSERVER lives in `lunco-usd-bevy-runtime`
// (`commands.rs::on_load_scene`), not here: mounting a scene has to resolve the
// requested path to its DOCUMENT first (a doc-backed scene must mount its
// composed `base ⊕ runtime`, never the base file), and the document registry
// lives one layer up. This crate owns the mount MECHANICS the observer drives —
// [`scene::validate_scene_address`], [`scene::resolve_root_prim`],
// [`scene::clear_scene_entities`], [`scene::spawn_scene_root_world`],
// [`SceneLoadInFlight`] — as its public mount API.

/// Reload the CURRENTLY-ACTIVE scene from disk — the "restart" verb.
///
/// [`LoadScene`] deliberately no-ops when asked to load the scene that is already
/// active (same path + root), so it cannot pick up on-disk edits to the LIVE
/// scene. `RestartScene` always clears the current scene's entities, force-reloads
/// its stage asset from disk (busting the asset cache), and respawns a single
/// fresh root — so editing a `.usda` then `restart_scene()` shows the change with
/// no duplicate instances. `reset_document` is interpreted by the document layer:
/// it is false for the normal preserve-edits restart and true only after the UI
/// has confirmed a full reset. The lifecycle mechanic still targets whichever
/// scene is loaded.
/// Paired with `pause()` this is the "reload-then-freeze" one-liner the workflow
/// wanted (`restart_scene(); pause();`).
#[Command(default)]
pub struct RestartScene {
    /// Discard the active file document's authored and runtime layers before
    /// remounting. Callers must obtain explicit user consent first.
    pub reset_document: bool,
}

#[on_command(RestartScene)]
fn on_restart_scene(
    trigger: On<RestartScene>,
    mut coordinator: ResMut<SceneTransitionCoordinator>,
) {
    match coordinator.admit(SceneTransitionRequest::restart(
        trigger.event().reset_document,
    )) {
        SceneTransitionAdmission::AlreadyActive => {
            info!("[restart-scene] restart is already in progress — no-op");
        }
        SceneTransitionAdmission::Queued => {
            info!("[restart-scene] queued behind the active scene transaction");
        }
        SceneTransitionAdmission::Admitted => {
            info!("[restart-scene] admitted for the next scene lifecycle phase");
        }
    }
}

pub(crate) fn execute_admitted_restart_scene(
    trigger: On<SceneTransitionAdmitted>,
    asset_server: Res<AssetServer>,
    mut coordinator: ResMut<SceneTransitionCoordinator>,
    mut commands: Commands,
    q_usd: Query<(Entity, &UsdPrimPath, Has<UsdSceneRoot>)>,
    scene: SceneEntities,
    mut mount_state: Option<ResMut<lunco_core::SceneMountState>>,
) {
    let SceneTransitionRequest::Restart { reset_document } = &trigger.event().request else {
        return;
    };

    // Every loaded prim shares the scene's stage handle. REUSE that handle (not a
    // freshly-resolved path) so the exact same asset — INCLUDING its source scheme
    // (`twin://…`, `lunco://…`) — is respawned. Resolving via `.path()` would
    // drop the scheme and load a *different* raw-file asset, breaking twin routing
    // (avatar/camera setup, composed runtime edits) and leaving a stale camera.
    let Some((_, upp, _)) = q_usd.iter().find(|(entity, _, is_scene_root)| {
        *is_scene_root
            && mount_state
                .as_deref()
                .is_none_or(|state| state.contains_root(*entity))
    }) else {
        warn!("[restart-scene] no scene is loaded — nothing to restart");
        coordinator.finish_noop();
        return;
    };
    let handle = upp.stage_handle.clone();
    // Full asset path WITH source scheme (owned, so `reload` doesn't need a
    // `'static` borrow), for the reload key + the scene-root label. `None` only
    // for a document-backed stage with no registered path — still respawnable
    // from the handle, just unlabelled.
    let asset_path = asset_server.get_path(handle.id()).map(|p| p.into_owned());
    let label = asset_path
        .as_ref()
        .map(|p| p.to_string())
        .unwrap_or_else(|| "restarted-scene".to_string());
    info!("[restart-scene] reloading `{}` from disk", label);

    // Reject late events and deferred projections from the outgoing root while
    // its recursive despawn is still queued.
    if let Some(state) = mount_state.as_deref_mut() {
        state.begin_replacement();
    }

    // Restart is an explicit replacement transaction. Cancel the old load
    // identity before reclaiming its parked entities.
    commands.remove_resource::<SceneLoadInFlight>();
    let transition = SceneTransition::Restart {
        path: label.clone(),
        root_prim: String::new(),
        reset_document: *reset_document,
    };
    coordinator.start(transition.clone());
    commands.insert_resource(SceneLoadInFlight {
        path: label.clone(),
        stage_id: handle.id(),
    });
    commands.trigger(lunco_core::SceneTransitionStarted { transition });
    let stage_id = handle.id();

    // Despawn the old scene + free worker-side state (shared with `ClearScene`).
    // Every scene-authored entity (incl. the Avatar camera) carries `UsdPrimPath`,
    // so `try_despawn` (hierarchy-recursive) tears the old camera down here — no
    // stale window camera survives into the fresh scene.
    clear_scene_entities(&mut commands, &scene);

    // Defer the asset reload itself as well as the respawn. Higher layers may
    // refresh a doc-backed Twin's composed overlay while handling this same
    // command; doing the read in this queued phase makes that refreshed source
    // the one this stage reload consumes, without making this sim-layer module
    // depend on the document registry.
    commands.queue(move |world: &mut World| {
        let reload_expected = asset_path.is_some();
        if let Some(ap) = asset_path {
            world.resource::<AssetServer>().reload(ap);
        }
        spawn_scene_root_with_stage(world, &label, "", handle);
        if !reload_expected {
            world.write_message(SceneStageAssetOutcome::Loaded { stage_id });
        }
    });
}

/// Clear the active scene — despawn every USD prim entity + cosim wire
/// and free the worker-side Modelica steppers / Python script docs they
/// referenced, leaving an empty viewport.
///
/// Fired when a Twin / folder opens with nothing to show — no
/// `[usd] default_scene`, or a plain folder with no USD content — so the
/// viewport reflects the newly opened folder instead of keeping the
/// previously loaded scene. (`LoadScene` does this same clear *before*
/// loading its new scene.) Also useful standalone over the API / MCP as
/// a "clear the world" verb.
#[Command(default)]
pub struct ClearScene {}

#[on_command(ClearScene)]
fn on_clear_scene(_trigger: On<ClearScene>, mut coordinator: ResMut<SceneTransitionCoordinator>) {
    match coordinator.admit(SceneTransitionRequest::clear()) {
        SceneTransitionAdmission::AlreadyActive => {
            info!("[clear-scene] clear is already in progress — no-op");
        }
        SceneTransitionAdmission::Queued => {
            info!("[clear-scene] queued behind the active scene transaction");
        }
        SceneTransitionAdmission::Admitted => {
            info!("[clear-scene] admitted for the next scene lifecycle phase");
        }
    }
}

pub(crate) fn execute_admitted_clear_scene(
    trigger: On<SceneTransitionAdmitted>,
    mut coordinator: ResMut<SceneTransitionCoordinator>,
    mut commands: Commands,
    scene: SceneEntities,
    mut mount_state: Option<ResMut<lunco_core::SceneMountState>>,
) {
    if trigger.event().request != SceneTransitionRequest::Clear {
        return;
    }

    info!("[clear-scene] clearing viewport");
    coordinator.start(SceneTransition::Clear);
    if let Some(state) = mount_state.as_deref_mut() {
        state.begin_replacement();
    }
    commands.trigger(lunco_core::SceneTransitionStarted {
        transition: SceneTransition::Clear,
    });
    // A clear invalidates a stage load that may still be waiting on an asset.
    // Without removing this identity, a late outcome from the outgoing stage
    // can close or assert against the replacement transaction.
    commands.remove_resource::<SceneLoadInFlight>();
    commands.remove_resource::<lunco_usd_bevy_scene::FailedSceneLoad>();
    clear_scene_entities(&mut commands, &scene);
    commands.queue(|world: &mut World| {
        world.trigger(SceneTransitionCompleted {
            transition: SceneTransition::Clear,
        });
    });
}

/// Route dependency-light scene requests to the typed command that owns each
/// transition. Every caller, including tutorials, enters the same transaction
/// coordinator as the public command/API surface.
pub(crate) fn on_scene_transition_intent(
    trigger: On<SceneTransitionIntent>,
    mut commands: Commands,
) {
    match &trigger.event().request {
        SceneTransitionRequest::Load { path, root_prim } => {
            commands.trigger(LoadScene {
                path: path.clone(),
                root_prim: root_prim.clone(),
            });
        }
        SceneTransitionRequest::Clear => commands.trigger(ClearScene {}),
        SceneTransitionRequest::Restart { reset_document } => {
            commands.trigger(RestartScene {
                reset_document: *reset_document,
            });
        }
    }
}

pub(crate) fn dispatch_admitted_scene_transition(
    mut coordinator: ResMut<SceneTransitionCoordinator>,
    mut commands: Commands,
) {
    let Some(request) = coordinator.take_admitted() else {
        return;
    };
    commands.trigger(SceneTransitionAdmitted { request });
}

pub(crate) fn has_admitted_scene_transition(coordinator: Res<SceneTransitionCoordinator>) -> bool {
    coordinator.has_admitted()
}

pub(crate) fn on_scene_transition_completed(
    trigger: On<SceneTransitionCompleted>,
    mut coordinator: ResMut<SceneTransitionCoordinator>,
) {
    coordinator.finish(&trigger.event().transition);
}

pub(crate) fn on_scene_transition_failed(
    trigger: On<SceneTransitionFailed>,
    mut coordinator: ResMut<SceneTransitionCoordinator>,
) {
    coordinator.finish(&trigger.event().transition);
}

/// Despawn the current scene's USD entities, synthesized physics entities, and
/// cosim wires.
///
/// The shared SceneTeardown schedule runs first. Its subsystem owners stop
/// scenario runtimes, retire Avian graph membership, and reset scene-derived
/// resources while the outgoing entities still exist. The deferred despawns
/// below then reclaim every entity under the same ownership boundary.
///
/// Commands touching this query use fallible forms because several teardown
/// owners may have already reclaimed a target in the same transaction.
/// The scene-owned entities a teardown touches, bundled as one `SystemParam`.
///
/// Every scene-lifecycle observer — `LoadScene` (in `lunco-usd-bevy-runtime`), `ClearScene`,
/// `RestartScene` — needs exactly this set. Bundling keeps the mount API honest:
/// a caller drives a teardown without naming `WorldGrid`, `OriginAnchor` or the
/// cosim `SimConnection` wire type, so `lunco-usd-bevy-runtime` needs no dependency on
/// `lunco-cosim` to orchestrate a scene swap.
#[derive(bevy::ecs::system::SystemParam)]
pub struct SceneEntities<'w, 's> {
    grid: Query<'w, 's, (Entity, &'static Children), With<WorldGrid>>,
    origin: Query<'w, 's, Entity, With<OriginAnchor>>,
    /// Every active scene root identifies the USD stage whose generated prims
    /// belong to that scene.  A camera mount is allowed to move a prim directly
    /// under the persistent grid, so hierarchy alone is not sufficient to find
    /// all of these on teardown.
    scene_roots: Query<'w, 's, &'static UsdPrimPath, With<UsdSceneRoot>>,
    prims: Query<'w, 's, (Entity, &'static UsdPrimPath)>,
    parents: Query<'w, 's, &'static ChildOf>,
    wires: Query<'w, 's, Entity, With<SimConnection>>,
    /// Physics-created joint entities and world-anchor bodies have no USD prim
    /// path, so their explicit scene-ownership marker is the authoritative
    /// reclamation key.
    physics_owned: Query<'w, 's, Entity, With<lunco_usd_avian::ScenePhysicsOwned>>,
}

pub fn clear_scene_entities(commands: &mut Commands, scene: &SceneEntities) {
    // A scene is its entities AND the resources derived from it. Resources are
    // restored through the registry rather than named here, so a subsystem that
    // adds scene-derived state does not also have to edit this function — see
    // `lunco_core::SceneTeardown`.
    commands.queue(lunco_core::run_scene_teardown);

    let (q_grid, q_origin, q_scene_roots, q_prims, q_wires, q_physics_owned) = (
        &scene.grid,
        &scene.origin,
        &scene.scene_roots,
        &scene.prims,
        &scene.wires,
        &scene.physics_owned,
    );
    let mut despawned = 0usize;

    // Despawn all children of the WorldGrid (recursively), except the persistent OriginAnchor
    let grid_entity = q_grid.single().ok().map(|(entity, _)| entity);
    if let Ok((_, children)) = q_grid.single() {
        for child in children.iter() {
            if !q_origin.contains(child) {
                commands.entity(child).try_despawn();
                despawned += 1;
            }
        }
    }

    // Some scene-owned prims intentionally leave their authored hierarchy. In
    // particular, `resolve_camera_mounts` moves a mounted USD camera directly
    // beneath the grid so it can host the floating origin at full precision.
    // It therefore survives a root-only clear and keeps rendering alongside the
    // next scene's avatar.  Reclaim every prim from an active scene-root stage
    // as a second, stage-scoped ownership sweep. Preview stages are not
    // `UsdSceneRoot`s, so editor previews remain outside this lifecycle.
    let active_stage_ids: std::collections::HashSet<_> = q_scene_roots
        .iter()
        .map(|root| root.stage_handle.id())
        .collect();
    let mut stage_prim_despawns = 0usize;
    for (entity, prim) in q_prims.iter() {
        let already_covered_by_grid = grid_entity.is_some_and(|grid| {
            let mut current = entity;
            for _ in 0..1024 {
                if current == grid {
                    return true;
                }
                let Ok(parent) = scene.parents.get(current) else {
                    return false;
                };
                current = parent.parent();
            }
            false
        });
        if active_stage_ids.contains(&prim.stage_handle.id()) && !already_covered_by_grid {
            commands.entity(entity).try_despawn();
            stage_prim_despawns += 1;
        }
    }

    // Despawn any root-level derived connection wires (which are spawned as root entities)
    for e in q_wires.iter() {
        commands.entity(e).try_despawn();
        despawned += 1;
    }

    for e in q_physics_owned.iter() {
        commands.entity(e).try_despawn();
        despawned += 1;
    }
    info!(
        "[scene] cleanup: {despawned} grid children and {stage_prim_despawns} stage prims queued for despawn"
    );
    // Every scene clear resets the whole clock tree to defaults (doc 19 §11b): a sky
    // left detached at 100 000×, a scrubbed animation, a paused transport — none of it
    // may survive into the next scene. This is the single choke point all three reload
    // paths funnel through, so the reset lives here, not at each call site.
    commands.trigger(lunco_time::ResetTime {});
}

/// End scene-owned script documents before their USD entities are removed.
///
/// The generic driver normally notices a detached entity on its next fixed
/// tick. That is deliberately too late for a scene boundary: the old
/// `on_stop` hook could then publish commands into the replacement scene, and
/// the driver's compiled `this` state would remain alive across the swap. The
/// marker is the ownership declaration; interactive/API documents are not
/// touched here.
pub(crate) fn stop_scene_owned_scripts(world: &mut World) {
    let targets: Vec<(Entity, Option<u64>)> = {
        let mut query =
            world.query_filtered::<(Entity, Option<&ScriptedModel>), With<SceneOwnedScript>>();
        query
            .iter(world)
            .map(|(entity, model)| (entity, model.and_then(|m| m.document_id)))
            .collect()
    };

    for (entity, document_id) in targets {
        ScenarioDriver::<RhaiScenarioRuntime>::stop_entity(world, entity);
        if let Ok(mut entity_mut) = world.get_entity_mut(entity) {
            entity_mut.remove::<ScriptedModel>();
        }
        if let Some(document_id) = document_id {
            if let Some(mut registry) = world.get_resource_mut::<ScriptRegistry>() {
                registry.documents.remove(&DocumentId::new(document_id));
            }
        }
    }
}

/// Despawn a single USD prim **subtree** (one runtime prim and its descendants).
///
/// Under Bevy 0.19's relationship system, despawning the root entity recursively
/// despawns all descendants. Component removal triggers (`On<Remove, T>`) fire automatically,
/// freeing any worker-side state (such as Modelica steppers or Python script documents)
/// via the reactive observers registered in `lunco-modelica-core` and `lunco-scripting`.
pub fn despawn_usd_subtree(world: &mut World, root: Entity) {
    if let Ok(em) = world.get_entity_mut(root) {
        em.despawn();
        info!("[scene] incremental despawn: entity {:?}", root);
    }
}

/// Spawn one new USD child prim into a live scene, mirroring the child branch of
/// the visual projector's per-prim analogue of a full scene-root mount, used
/// by the USD command runtime's E2 incremental-spawn path
/// when a `Resync` reports a prim added to the composed document.
///
/// The caller resolves the live parent from the canonical stage identity and
/// passes that entity through. This keeps the constructor scoped to the
/// authoritative USD hierarchy instead of making a second world-wide path
/// lookup (which cannot distinguish separate mounts of the same stage).
/// It spawns the stub child for `path` under the already-resolved
/// `parent_entity`, with a pre-read transform `tf`, inheriting grid-anchoring +
/// instance membership from that parent. The `on_usd_prim_added` observer then
/// builds the subtree from the canonical stage.
///
/// The live-stage projection bridge resolves the parent and checks the changed
/// path before calling this function. Passing the entity is intentional: the
/// parent hierarchy is the identity boundary, so this low-level constructor
/// neither resolves a parent by path nor scans the world for an existing child.
/// The stage itself cannot be held across the spawn because it aliases the
/// world; the observer reads it afresh from `CanonicalStages`.
pub fn spawn_usd_child_under_parent(
    world: &mut World,
    parent_entity: Entity,
    path: &str,
    tf: Transform,
) -> Option<Entity> {
    let stage_handle = world
        .get::<UsdPrimPath>(parent_entity)?
        .stage_handle
        .clone();
    let parent_path = world.get::<UsdPrimPath>(parent_entity)?.path.clone();
    let (parent_prefix, _) = path.rsplit_once('/')?;
    let expected_parent_path = if parent_prefix.is_empty() {
        "/"
    } else {
        parent_prefix
    };
    if parent_path != expected_parent_path {
        warn!(
            "[usd-cosim] incremental spawn rejected for {path}: resolved parent is {parent_path}, expected {expected_parent_path}"
        );
        return None;
    }

    // Inherit grid-anchoring + instance membership from the parent exactly as
    // `instantiate_usd_prim` derives them for its children.
    let parent_member = world.get::<UsdInstanceMember>(parent_entity).cloned();
    let parent_projection = world.get::<UsdInstanceProjection>(parent_entity).cloned();
    let parent_is_root = world.get::<UsdInstanceRoot>(parent_entity).is_some();
    let member = parent_member.or_else(|| {
        parent_is_root.then(|| UsdInstanceMember {
            root: parent_entity,
            root_path: parent_path.to_string(),
        })
    });

    let base = (
        Name::new(path.to_string()),
        UsdPrimPath {
            stage_handle,
            path: path.to_string(),
        },
        tf,
        GlobalTransform::default(),
        Visibility::Visible,
        InheritedVisibility::VISIBLE,
        ViewVisibility::default(),
    );
    // A top-level child of the nested scene Grid carries its own CellCoord.
    // Deeper USD descendants remain plain children of their authored parent.
    let parent_is_grid = world.get::<Grid>(parent_entity).is_some();
    let entity = match member {
        Some(m) if parent_is_grid => world
            .spawn((base, ChildOf(parent_entity), m, CellCoord::default()))
            .id(),
        Some(m) => world.spawn((base, ChildOf(parent_entity), m)).id(),
        None if parent_is_grid => world
            .spawn((base, ChildOf(parent_entity), CellCoord::default()))
            .id(),
        None => world.spawn((base, ChildOf(parent_entity))).id(),
    };
    if let Some(projection) = parent_projection {
        world.entity_mut(entity).insert(projection);
    }
    info!("[scene] incremental spawn: `{}` (entity {})", path, entity);
    Some(entity)
}

/// Validate a scene's address before it enters the shared scene lifecycle.
/// `LoadScene` accepts only the two registered, root-qualified asset schemes:
/// `lunco://` for the shipped library and `twin://` for an opened Twin.
/// Filesystem paths belong to `OpenFile` / startup root discovery and must not
/// be reinterpreted here.
pub fn validate_scene_address(path_in: &str) -> Option<String> {
    let valid_lunco = lunco_assets_core::parse_lunco_uri(path_in)
        .is_some_and(lunco_assets_core::asset_path::is_safe_relative_path);
    let valid_twin = lunco_assets_core::parse_twin_uri(path_in).is_some_and(|(name, rel)| {
        !name.is_empty() && lunco_assets_core::asset_path::is_safe_relative_path(rel)
    });
    if valid_lunco || valid_twin {
        return Some(path_in.to_string());
    }

    warn!(
        "[scene] `{path_in}` is not a root-qualified scene address — LoadScene takes \
         `lunco://…` or `twin://…`. Use OpenFile for a filesystem path."
    );
    None
}

/// Spawn a USD scene root directly under the canonical `WorldGrid` entity.
///
/// Shared by `LoadScene` (after its clear step) and `OpenFile` (additive
/// import). Blender-style no-op when the same `(asset, root_prim)` is
/// already mounted. Returns the spawned entity, or `None` on no-op /
/// missing or invalid `WorldGrid`.
pub fn spawn_scene_root_world(
    world: &mut World,
    path_in: &str,
    root_prim_in: &str,
) -> Option<Entity> {
    let asset_path = validate_scene_address(path_in)?;
    // File-backed source: the AssetServer reads + composes the on-disk
    // stage. The USD command runtime's E1 projection takes the other door
    // ([`spawn_scene_root_with_stage`]) to mount a document's *composed*
    // (base ⊕ runtime) stage instead.
    let handle = world
        .resource::<AssetServer>()
        .load::<UsdStageAsset>(asset_path.clone());
    spawn_scene_root_with_stage(world, &asset_path, root_prim_in, handle)
}

/// The mounted scene root — the entity a scene's whole prim subtree hangs from.
///
/// It is the only entity that knows **both** halves of "where does a scene-level
/// edit go?": its [`UsdPrimPath::stage_handle`] resolves to the editable document
/// (via `lunco_usd_bevy_twin::scene_document_for`), and its
/// [`UsdPrimPath::path`] is the *mounted root prim* — `/SandboxScene`, `/World`,
/// `/HdriTest`, whatever this scene's `defaultPrim` happens to be.
///
/// Before this marker existed, a command that wanted to author a new top-level
/// prim had to guess at both (count the document registry; hardcode `/World`) —
/// and a hardcoded `/World` authors under a parent that does not exist in a scene
/// rooted at `/SandboxScene`, so the prim composes into the layer and is then
/// never mounted. The scene root is the answer to both questions; ask it.
///
/// The preview viewport (`lunco_usd_viewport_ui`) mounts its own private root
/// the same way, so consumers that must act on the *running* scene should scope
/// their query rather than assume a single one exists.
/// Spawn a USD scene root from an **already-built** stage handle.
///
/// The handle-supplying sibling of [`spawn_scene_root_world`]: instead of
/// loading the stage from disk via the `AssetServer`, the caller hands in a
/// `Handle<UsdStageAsset>` it built itself. This is the seam E1 uses — lunco-usd-bevy-runtime
/// passes a handle holding a [`UsdDocument`](lunco_usd_document::document::UsdDocument)'s
/// *composed* (`base ⊕ runtime`) stage, so the live world projects the editable
/// document (with its persisted runtime spawns/moves) rather than the raw file.
///
/// `label` names the root (`Scene:{label}`) and feeds `defaultPrim` resolution;
/// `root_prim_in` empty defers the mount path to the stage's `defaultPrim`
/// (see [`resolve_root_prim`]). Missing `defaultPrim` is a terminal scene
/// error. Blender-style no-op when the same
/// `(handle, root_prim)` is already mounted. Returns the spawned entity, or
/// `None` on no-op.
pub fn spawn_scene_root_with_stage(
    world: &mut World,
    label: &str,
    root_prim_in: &str,
    handle: Handle<UsdStageAsset>,
) -> Option<Entity> {
    let asset_path = label.to_string();
    let root_prim = resolve_root_prim(&asset_path, root_prim_in);
    let new_id = handle.id();

    {
        let mut q = world.query::<&UsdPrimPath>();
        if q.iter(world)
            .any(|upp| upp.stage_handle.id() == new_id && upp.path == root_prim)
        {
            info!(
                "[scene] `{}` @ `{}` already loaded — no-op",
                asset_path, root_prim
            );
            return None;
        }
    }

    // Mount under the canonical world grid. `ensure_world_root` is create-or-get:
    // it builds the persistent shell (root + WorldGrid + persistent OriginAnchor)
    // the first scene load and returns the same grid on every reload — so the root
    // is never duplicated and never absent. Replaces the old "first `Grid` found"
    // heuristic, which was ambiguous once celestial / preview grids also existed.
    let grid = lunco_spatial::ensure_world_root(world);
    // Scene mounting owns the physics-frame binding. The canonical WorldGrid
    // is the scene frame; WorldRoot is only the persistent BigSpace shell and
    // must never become an implicit Avian frame.
    world.insert_resource(lunco_spatial::ActivePhysicsFrame(grid));

    // The scene root is the frame for its top-level USD prims as well as the
    // scene identity. Making it a nested Grid lets each top-level physical or
    // visual prim carry its own CellCoord, so a vehicle can cross a cell while
    // preserving the authored USD parentage and the same identity path.
    // Descendants remain plain children of their prim and use the low-precision
    // propagation path below that high-precision prim.
    // Register the mount before inserting `UsdPrimPath`. Adding that component
    // synchronously triggers the USD projection observer, which queues child
    // entities behind the scene-ownership fence. If the path were part of this
    // initial bundle, the observer could run before this root entered
    // `SceneMountState` and every queued child would be rejected as stale.
    //
    // The spatial components still land atomically with the root itself:
    // `ChildOf(grid)` + `CellCoord` + `Transform` are the same contract as
    // `migrate_to_grid`, avoiding the observer race that mis-tagged rover
    // chassis as `RigidBody::Static`.
    let scene_grid = world
        .get::<Grid>(grid)
        .cloned()
        .expect("ensure_world_root returned an entity without its Grid");
    let primary = world
        .get_resource::<SceneLoadInFlight>()
        .is_some_and(|load| load.stage_id == new_id && load.path == asset_path);
    let root = world
        .spawn((
            Name::new(format!("Scene:{}", asset_path)),
            UsdSceneRoot,
            scene_grid,
            Transform::default(),
            GlobalTransform::default(),
            Visibility::Visible,
            InheritedVisibility::default(),
            ViewVisibility::default(),
            CellCoord::default(),
            lunco_spatial::GridAnchor,
            ChildOf(grid),
        ))
        .id();
    if let Some(mut state) = world.get_resource_mut::<lunco_core::SceneMountState>() {
        state.register_root(root, primary);
    }
    world.entity_mut(root).insert(UsdPrimPath {
        stage_handle: handle,
        path: root_prim.clone(),
    });
    info!(
        "[scene] spawned `{}` @ `{}` (entity {})",
        asset_path, root_prim, root
    );
    Some(root)
}

/// Resolve the SDF mount path for a scene load.
///
/// Priority:
/// 1. explicit `override_in` (non-empty caller-supplied path) wins.
/// 2. otherwise return the empty *deferred-resolution sentinel* — the
///    scene-root entity is spawned with an empty path, and
///    visual projection resolves it from the
///    stage's `defaultPrim` metadata once the asset has parsed
///    (a missing `defaultPrim` is a terminal scene error).
///
/// The defaultPrim lookup is deliberately deferred rather than read
/// here: this runs synchronously at command time, before the stage
/// asset finishes loading. It is resolved from the parsed canonical `StageView`
/// at instantiate time instead — correct on both native and web, and
/// yielding the defaultPrim subtree rather than a whole-stage `/` mount.
///
/// Per USD spec, `defaultPrim` is only required for files that will be
/// *referenced* by other USD files (composition arcs need a target
/// prim). Opening a stage directly works fine without it.
pub fn resolve_root_prim(_asset_path: &str, override_in: &str) -> String {
    if !override_in.is_empty() {
        return override_in.to_string();
    }
    // Deferred sentinel — resolved against the parsed stage downstream.
    String::new()
}

register_commands!(on_clear_scene, on_restart_scene,);
