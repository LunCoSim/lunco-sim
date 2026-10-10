//! Entity list panel — `lunco-workbench::Panel` implementation.
//!
//! A hierarchy tree of scene objects: top-level objects (rovers, props,
//! terrain, cosim blocks) with their sub-parts (wheels, body) nested beneath,
//! so you can drill in and select a single wheel. Internal plumbing (cosim
//! wires, ports, empty transform wrappers) is hidden — only entities that are
//! selectable or mesh-bearing, plus their ancestors, appear. Clicking a node
//! selects it.
//! Membership follows the active scene mount's `UsdSceneRoot` hierarchy, so
//! nested BigSpace grids do not split one scene into separate browser scopes.
//!
//! **Reactive shape (WP-8):** the panel is a pure *view*. The scene-graph
//! harvest — flatten, parent-collapse, visibility prune, sort — runs in
//! [`populate_entity_tree_view`], a change-driven system that only re-derives
//! when the scene topology actually changes (see [`scene_topology_changed`]),
//! and stores the render-ready result in the [`EntityTreeView`] resource.
//! `render` reads that resource and the authoritative [`lunco_scene_selection::SelectedEntities`]
//! directly, and routes clicks through the same `apply_selection` path as
//! before. Nothing is scanned, walked, or sorted while painting.

use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task, futures_lite::future};
use bevy_egui::egui;
use lunco_camera_core::camera_display_labels;
use lunco_render::SceneCamera;
use lunco_scene_selection::{SelectEntityTarget, SelectionIntent};
use lunco_settings::SettingsSection;
use lunco_usd_core::runtime::{RUNTIME_PERSISTENCE_SETTING, runtime_persistence_for_twin};
use lunco_workbench_core::{Panel, PanelCtx, PanelId, PanelSlot};
use lunco_workspace::{SetTwinSetting, TwinClosed, TwinSettingInput, WorkspaceResource};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// Persisted view prefs for the Entity list.
#[derive(Resource, Serialize, Deserialize, Clone, Copy, PartialEq, Debug, Default)]
pub struct EntityListSettings {
    /// Show entities a system owns and churns ([`lunco_core::SystemManaged`]:
    /// streamed LOD tiles, globe tiles, scattered rocks). Off by default — with
    /// terrain streaming there are hundreds live and they bury the handful of
    /// authored objects the list exists to show.
    pub show_system: bool,
}

impl SettingsSection for EntityListSettings {
    const KEY: &'static str = "entity_list";
}

/// Push the Entity-list filter into the workbench **Settings** menu — where every
/// other persisted view pref lives (theme, perf HUD, terrain). The panel stays a
/// pure view; it grows no toolbar of its own.
pub(crate) fn register_settings_submenu(world: &mut World) {
    let Some(mut menus) = world.get_resource_mut::<lunco_workbench_core::WorkbenchMenuRegistry>()
    else {
        return;
    };
    menus.register_settings_submenu("Entity list", |ui, ctx| {
        ui.label(
            egui::RichText::new("Entity list")
                .text_style(lunco_theme::TypographyRole::Label.text_style())
                .weak(),
        );
        let current = ctx
            .resource::<EntityListSettings>()
            .is_some_and(|s| s.show_system);
        let mut next = current;
        ui.checkbox(&mut next, "Show system entities")
            .on_hover_text(
                "Streamed terrain LOD tiles, globe tiles and scattered rocks — spawned \
                 and despawned continuously as the camera moves. Hidden by default so \
                 the list shows authored scene objects only.",
            );
        if next != current {
            ctx.set_resource(EntityListSettings { show_system: next });
        }

        ui.separator();
        ui.label(
            egui::RichText::new("Runtime scene edits (active Twin)")
                .text_style(lunco_theme::TypographyRole::Label.text_style())
                .weak()
        );
        ui.label("Route points, runtime spawns, and gizmo edits use Twin Runtime (@runtime@).");
        let persistence_state = ctx.resource::<WorkspaceResource>().map(|workspace| {
            let Some(twin_id) = workspace.active_twin else {
                return (false, Ok(false));
            };
            let Some(twin) = workspace.twin(twin_id) else {
                return (false, Ok(false));
            };
            (twin.manifest.is_some(), runtime_persistence_for_twin(twin))
        });
        let Some((can_persist, persistence_result)) = persistence_state else {
            ui.label("No workspace session is available.");
            return;
        };
        match persistence_result {
            Ok(current) => {
                let mut next = current;
                ui.add_enabled_ui(can_persist, |ui| {
                    ui.checkbox(&mut next, "Persist runtime scene edits")
                        .on_hover_text(
                            "Route points, generated spawns, and gizmo moves are authored in this Twin's Runtime layer. When enabled, that layer loads from and saves to .lunco/runtime; off keeps edits for this session only.",
                        );
                });
                if !can_persist {
                    ui.weak("Open a manifest-backed Twin to opt in to runtime persistence.");
                } else if next != current {
                    ctx.trigger(SetTwinSetting {
                        key: RUNTIME_PERSISTENCE_SETTING.into(),
                        value: TwinSettingInput::Bool(next),
                    });
                }
            }
            Err(error) => {
                ui.label(format!("Runtime persistence error: {error}"));
            }
        }
    });
}

/// Render-ready, flattened scene tree for the Entity list panel.
///
/// Derived, disposable state — **never** authoritative. Populated only by
/// [`populate_entity_tree_view`]; panels read it, never write it. Children in
/// [`kids`](Self::kids) are already visibility-pruned and sorted, so the panel
/// can paint without filtering, and [`roots`](Self::roots) holds only shown
/// top-level entities.
#[derive(Resource, Default)]
pub struct EntityTreeView {
    /// Monotonic source revision used to invalidate panel-owned row indexes.
    pub revision: u64,
    /// Shown top-level entities, sorted by leaf label.
    pub roots: Vec<Entity>,
    /// Shown children per parent, sorted by leaf label. A parent with no shown
    /// children has no entry (so the panel treats it as a leaf).
    pub kids: HashMap<Entity, Vec<Entity>>,
    /// Display label per visible named entity.
    pub labels: HashMap<Entity, String>,
    /// Unqualified semantic label per visible named entity. Kept separately so
    /// the topology gate can compare source labels without treating a stable
    /// duplicate suffix as a source change.
    base_labels: HashMap<Entity, String>,
    /// Stable source address used to order duplicate labels independently of
    /// Bevy allocation order.
    stable_keys: HashMap<Entity, String>,
    /// Visible camera entities whose labels use the shared camera policy.
    camera_entities: HashSet<Entity>,
    /// Full camera identities retained for row tooltips and diagnostics.
    camera_identities: HashMap<Entity, String>,
    /// Direct parent snapshot for visible named entities. The gate compares
    /// actual edge values: grid and celestial systems may reinsert an identical
    /// parent every frame.
    parents: HashMap<Entity, Entity>,
    show_system: bool,
    /// Primary scene mount used for the cached tree.
    active_scene_root: Option<Entity>,
    /// Invalid scene ownership prevents a misleading tree.
    scene_error: Option<String>,
    /// Active-scene ancestry whose parent or scene-root changes can alter the tree.
    candidate_hierarchy_entities: HashSet<Entity>,
    /// Parent links at the time the active-scene ancestry was harvested.
    candidate_hierarchy_parents: HashMap<Entity, Entity>,
    /// Set once the first build runs, so the change-gate forces an initial fill.
    built: bool,
}

struct NamedTreeCandidate {
    entity: Entity,
    label: String,
    stable_key: String,
    camera_identity: Option<String>,
    selectable: bool,
    has_mesh: bool,
}

struct EntityTreeBuildInput {
    show_system: bool,
    active_scene_root: Option<Entity>,
    scene_error: Option<String>,
    child_of: HashMap<Entity, Entity>,
    named_candidates: Vec<NamedTreeCandidate>,
}

#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct EntityTreeSceneQueries<'w, 's> {
    mount: Option<Res<'w, lunco_core::SceneMountState>>,
    roots: Query<'w, 's, (), With<lunco_usd_bevy_scene::UsdSceneRoot>>,
    preview_roots: Query<'w, 's, (), With<lunco_usd_bevy_scene::UsdPreviewOnly>>,
    parents: Query<'w, 's, &'static ChildOf>,
    boundary_changes: Query<
        'w,
        's,
        Entity,
        Or<(
            Changed<lunco_usd_bevy_scene::UsdSceneRoot>,
            Changed<lunco_usd_bevy_scene::UsdPreviewOnly>,
        )>,
    >,
    entities: Query<'w, 's, Entity>,
}

#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct EntityTreeRemovals<'w, 's> {
    names: RemovedComponents<'w, 's, Name>,
    children: RemovedComponents<'w, 's, ChildOf>,
    meshes: RemovedComponents<'w, 's, Mesh3d>,
    selectables: RemovedComponents<'w, 's, lunco_core::SelectableRoot>,
    callsigns: RemovedComponents<'w, 's, lunco_core::markers::Callsign>,
    catalog_ids: RemovedComponents<'w, 's, lunco_core::CatalogEntryId>,
    usd_paths: RemovedComponents<'w, 's, lunco_usd_bevy_scene::UsdPrimPath>,
    scene_roots: RemovedComponents<'w, 's, lunco_usd_bevy_scene::UsdSceneRoot>,
    preview_roots: RemovedComponents<'w, 's, lunco_usd_bevy_scene::UsdPreviewOnly>,
    system_managed: RemovedComponents<'w, 's, lunco_core::SystemManaged>,
    cameras: RemovedComponents<'w, 's, SceneCamera>,
}

#[derive(Resource, Default)]
pub(crate) struct EntityTreeBuildState {
    revision: u64,
    dirty: bool,
    task: Option<Task<(u64, EntityTreeView)>>,
}

/// Native hierarchy insertions delivered to the tree's Update gate.
#[derive(Message)]
pub(crate) struct EntityTreeParentInserted(Entity);

fn record_entity_tree_parent_insert(
    trigger: On<Insert, ChildOf>,
    mut insertions: MessageWriter<EntityTreeParentInserted>,
) {
    insertions.write(EntityTreeParentInserted(trigger.entity));
}

pub(crate) fn install_entity_tree_hierarchy_tracking(app: &mut App) {
    app.add_message::<EntityTreeParentInserted>()
        .add_observer(record_entity_tree_parent_insert);
}

impl EntityTreeBuildState {
    fn invalidate(&mut self) {
        self.revision = self.revision.wrapping_add(1);
        self.dirty = true;
    }
}

fn active_scene_root_state(
    scene_mount: Option<&lunco_core::SceneMountState>,
    scene_roots: &Query<(), With<lunco_usd_bevy_scene::UsdSceneRoot>>,
) -> (Option<Entity>, Option<String>) {
    let Some(scene_mount) = scene_mount else {
        return (None, Some("scene mount state is unavailable".into()));
    };
    match scene_mount.active_root() {
        None => (None, None),
        Some(root) if scene_roots.contains(root) => (Some(root), None),
        Some(root) => (
            None,
            Some(format!(
                "active scene root {root:?} is not a live USD scene root"
            )),
        ),
    }
}

fn belongs_to_active_scene(
    entity: Entity,
    active_scene_root: Option<Entity>,
    scene_roots: &Query<(), With<lunco_usd_bevy_scene::UsdSceneRoot>>,
    preview_roots: &Query<(), With<lunco_usd_bevy_scene::UsdPreviewOnly>>,
    parents: &Query<&ChildOf>,
    entities: &Query<Entity>,
) -> Result<bool, String> {
    let Some(active_root) = active_scene_root else {
        return Ok(false);
    };
    match lunco_usd_bevy_scene::scene_root_ancestor(entity, scene_roots, parents, entities) {
        Ok(root) if root == Some(active_root) => Ok(!lunco_usd_bevy_scene::is_preview_only(
            entity,
            parents,
            preview_roots,
        )),
        Ok(_) => Ok(false),
        Err(lunco_usd_bevy_scene::SceneRootAncestorError::MissingParentEntity) => Err(format!(
            "entity {entity:?} has a missing parent while resolving scene ownership"
        )),
        Err(lunco_usd_bevy_scene::SceneRootAncestorError::DepthExceeded) => Err(format!(
            "entity {entity:?} scene hierarchy exceeds {} ancestors",
            lunco_usd_bevy_scene::MAX_SCENE_HIERARCHY_DEPTH
        )),
    }
}

fn collect_candidate_hierarchy(
    entity: Entity,
    active_scene_root: Entity,
    child_of: &HashMap<Entity, Entity>,
    ancestors: &mut HashSet<Entity>,
    hierarchy_parents: &mut HashMap<Entity, Entity>,
) {
    let mut current = entity;
    for _ in 0..lunco_usd_bevy_scene::MAX_SCENE_HIERARCHY_DEPTH {
        if !ancestors.insert(current) {
            return;
        }
        if current == active_scene_root {
            return;
        }
        let Some(parent) = child_of.get(&current).copied() else {
            return;
        };
        hierarchy_parents.insert(current, parent);
        current = parent;
    }
}

fn stable_key(name: &Name, path: Option<&lunco_usd_bevy_scene::UsdPrimPath>) -> String {
    path.map(|path| path.path.as_str())
        .filter(|path| !path.is_empty() && *path != "/")
        .unwrap_or_else(|| name.as_str())
        .to_string()
}

/// Add a deterministic ordinal only where visible entities share a semantic
/// label. USD paths are stable presentation-order keys; the live ECS entity is
/// never used to decide which duplicate gets which suffix.
fn disambiguate_labels(
    named: &[(Entity, String, String)],
    shown: &HashMap<Entity, bool>,
) -> HashMap<Entity, String> {
    let mut groups: HashMap<&str, Vec<(Entity, &str)>> = HashMap::new();
    for (entity, label, key) in named {
        if shown.get(entity).copied().unwrap_or(false) {
            groups.entry(label).or_default().push((*entity, key));
        }
    }

    let mut labels = HashMap::new();
    for (base, mut members) in groups {
        members.sort_by_key(|(_, a)| *a);
        if members.len() == 1 {
            labels.insert(members[0].0, base.to_string());
        } else {
            for (ordinal, (entity, _)) in members.into_iter().enumerate() {
                labels.insert(entity, format!("{base} ({})", ordinal + 1));
            }
        }
    }
    labels
}

/// `true` if `e` is shown (interesting itself, or an ancestor of something
/// interesting). Memoized post-order walk; the pre-insert of `false` guards
/// against malformed cycles in the parent graph.
fn compute_shown(
    e: Entity,
    kids: &HashMap<Entity, Vec<Entity>>,
    interesting: &dyn Fn(Entity) -> bool,
    shown: &mut HashMap<Entity, bool>,
) -> bool {
    if let Some(&v) = shown.get(&e) {
        return v;
    }
    shown.insert(e, false);
    let mut vis = interesting(e);
    if let Some(cs) = kids.get(&e) {
        for &c in cs {
            vis |= compute_shown(c, kids, interesting, shown);
        }
    }
    shown.insert(e, vis);
    vis
}

/// Snapshot producer for [`EntityTreeView`]. A **normal** Bevy system with
/// cached `Query` params — no per-frame `QueryState` rebuild — gated by
/// [`entity_tree_build_due`]. Topology changes mark the build dirty separately,
/// so a pending worker never re-enters this query-heavy system just to discard
/// its result.
pub(crate) fn populate_entity_tree_view(
    mut build: ResMut<EntityTreeBuildState>,
    settings: Res<EntityListSettings>,
    scene_mount: Option<Res<lunco_core::SceneMountState>>,
    scene_roots: Query<(), With<lunco_usd_bevy_scene::UsdSceneRoot>>,
    preview_roots: Query<(), With<lunco_usd_bevy_scene::UsdPreviewOnly>>,
    parents: Query<&ChildOf>,
    entities: Query<Entity>,
    named_q: Query<(
        Entity,
        &Name,
        Option<&lunco_core::markers::Callsign>,
        Option<&lunco_core::CatalogEntryId>,
        Option<&lunco_usd_bevy_scene::UsdPrimPath>,
        Has<SceneCamera>,
        Has<lunco_core::SystemManaged>,
        Has<lunco_core::SelectableRoot>,
        Has<Mesh3d>,
    )>,
) {
    if build.task.is_some() {
        return;
    }

    let input = {
        let _span = bevy::log::info_span!("entity_tree_view_snapshot").entered();
        // System-owned churn is excluded before the immutable snapshot leaves the
        // ECS thread. The hierarchy derivation below uses only owned values.
        let (active_scene_root, mut scene_error) =
            active_scene_root_state(scene_mount.as_deref(), &scene_roots);
        let named_candidates: Vec<_> = named_q
            .iter()
            .filter_map(
                |(
                    entity,
                    name,
                    callsign,
                    catalog_id,
                    path,
                    is_camera,
                    is_system,
                    selectable,
                    has_mesh,
                )| {
                    if Some(entity) == active_scene_root || (!settings.show_system && is_system) {
                        return None;
                    }
                    match belongs_to_active_scene(
                        entity,
                        active_scene_root,
                        &scene_roots,
                        &preview_roots,
                        &parents,
                        &entities,
                    ) {
                        Ok(true) => {}
                        Ok(false) => return None,
                        Err(error) => {
                            scene_error.get_or_insert(error);
                            return None;
                        }
                    }
                    Some(NamedTreeCandidate {
                        entity,
                        label: lunco_core::entity_display_name(Some(name), callsign, catalog_id),
                        stable_key: stable_key(name, path),
                        camera_identity: is_camera.then(|| {
                            path.map(|path| path.path.clone())
                                .unwrap_or_else(|| name.as_str().to_string())
                        }),
                        selectable,
                        has_mesh,
                    })
                },
            )
            .collect();
        // The tree only reads hierarchy along named entities' ancestry. Harvest
        // those paths through indexed Query::get lookups
        // instead of scanning every internal wrapper in the scene.
        let mut child_of = HashMap::new();
        let mut visited = HashSet::new();
        for candidate in &named_candidates {
            let mut current = candidate.entity;
            for _ in 0..lunco_usd_bevy_scene::MAX_SCENE_HIERARCHY_DEPTH {
                if Some(current) == active_scene_root {
                    break;
                }
                if visited.contains(&current) {
                    break;
                }
                visited.insert(current);
                let Ok(parent) = parents.get(current) else {
                    break;
                };
                let parent = parent.parent();
                child_of.insert(current, parent);
                current = parent;
            }
        }
        EntityTreeBuildInput {
            show_system: settings.show_system,
            active_scene_root,
            scene_error,
            child_of,
            named_candidates,
        }
    };

    let revision = build.revision;
    build.dirty = false;
    let span = bevy::log::info_span!("entity_tree_view_derive_worker");
    build.task = Some(AsyncComputeTaskPool::get().spawn(async move {
        let _span = span.enter();
        (revision, derive_entity_tree_view(input))
    }));
}

pub(crate) fn entity_tree_task_pending(build: Res<EntityTreeBuildState>) -> bool {
    build.task.is_some()
}

pub(crate) fn poll_entity_tree_view_build(
    mut build: ResMut<EntityTreeBuildState>,
    mut view: ResMut<EntityTreeView>,
) {
    let completed = build
        .task
        .as_mut()
        .and_then(|task| future::block_on(future::poll_once(task)));
    let Some((revision, mut result)) = completed else {
        return;
    };
    build.task = None;
    if revision == build.revision {
        result.revision = revision;
        *view = result;
        build.dirty = false;
    } else {
        build.dirty = true;
    }
}

fn derive_entity_tree_view(input: EntityTreeBuildInput) -> EntityTreeView {
    let EntityTreeBuildInput {
        show_system,
        active_scene_root,
        scene_error,
        child_of,
        named_candidates,
    } = input;
    let mut candidate_hierarchy_entities = HashSet::new();
    let mut candidate_hierarchy_parents = HashMap::new();
    let mut named: Vec<(Entity, String, String)> = Vec::with_capacity(named_candidates.len());
    let mut camera_identities = Vec::new();
    let mut selectable = HashSet::new();
    let mut has_mesh = HashSet::new();
    for candidate in named_candidates {
        let NamedTreeCandidate {
            entity,
            label,
            stable_key,
            camera_identity,
            selectable: is_selectable,
            has_mesh: has_mesh_marker,
        } = candidate;
        if is_selectable {
            selectable.insert(entity);
        }
        if has_mesh_marker {
            has_mesh.insert(entity);
        }
        let Some(active_scene_root) = active_scene_root.filter(|_| scene_error.is_none()) else {
            continue;
        };
        collect_candidate_hierarchy(
            entity,
            active_scene_root,
            &child_of,
            &mut candidate_hierarchy_entities,
            &mut candidate_hierarchy_parents,
        );
        if let Some(identity) = camera_identity {
            camera_identities.push((entity, identity));
        }
        named.push((entity, label, stable_key));
    }
    let named_set: HashSet<Entity> = named.iter().map(|(entity, _, _)| *entity).collect();
    let camera_names: Vec<String> = camera_identities
        .iter()
        .map(|(_, identity)| identity.clone())
        .collect();
    let camera_identity_by_entity: HashMap<Entity, String> =
        camera_identities.iter().cloned().collect();
    let camera_labels: HashMap<Entity, String> = camera_identities
        .into_iter()
        .zip(camera_display_labels(&camera_names))
        .map(|((entity, _), label)| (entity, label))
        .collect();
    let camera_entities: HashSet<Entity> = camera_labels.keys().copied().collect();

    let display_parent = |entity: Entity| -> Option<Entity> {
        let mut current = entity;
        for _ in 0..lunco_usd_bevy_scene::MAX_SCENE_HIERARCHY_DEPTH {
            let parent = *child_of.get(&current)?;
            if named_set.contains(&parent) {
                return Some(parent);
            }
            current = parent;
        }
        None
    };
    let mut kids: HashMap<Entity, Vec<Entity>> = HashMap::new();
    let mut roots = Vec::new();
    for (entity, _, _) in &named {
        match display_parent(*entity) {
            Some(parent) => kids.entry(parent).or_default().push(*entity),
            None => roots.push(*entity),
        }
    }

    let interesting = |entity| {
        selectable.contains(&entity)
            || has_mesh.contains(&entity)
            || camera_entities.contains(&entity)
    };
    let mut shown = HashMap::new();
    for (entity, _, _) in &named {
        compute_shown(*entity, &kids, &interesting, &mut shown);
    }

    let base_labels: HashMap<Entity, String> = named
        .iter()
        .filter(|(entity, _, _)| shown.get(entity).copied().unwrap_or(false))
        .map(|(entity, label, _)| (*entity, label.clone()))
        .collect();
    let mut labels = disambiguate_labels(&named, &shown);
    for (entity, label) in &camera_labels {
        if shown.get(entity).copied().unwrap_or(false) {
            labels.insert(*entity, label.clone());
        }
    }
    let by_leaf = |left: &Entity, right: &Entity| {
        let left_label = labels.get(left).map(String::as_str).unwrap_or("");
        let right_label = labels.get(right).map(String::as_str).unwrap_or("");
        left_label.cmp(right_label)
    };
    let mut pruned: HashMap<Entity, Vec<Entity>> = HashMap::new();
    for (parent, children) in &kids {
        let mut visible_children: Vec<Entity> = children
            .iter()
            .copied()
            .filter(|child| *shown.get(child).unwrap_or(&false))
            .collect();
        if visible_children.is_empty() {
            continue;
        }
        visible_children.sort_by(by_leaf);
        pruned.insert(*parent, visible_children);
    }
    roots.retain(|entity| *shown.get(entity).unwrap_or(&false));
    roots.sort_by(by_leaf);

    let mut view = EntityTreeView::default();
    view.roots = roots;
    view.kids = pruned;
    view.labels = labels
        .into_iter()
        .filter(|(entity, _)| shown.get(entity).copied().unwrap_or(false))
        .collect();
    view.base_labels = base_labels;
    view.stable_keys = named
        .iter()
        .filter(|(entity, _, _)| shown.get(entity).copied().unwrap_or(false))
        .map(|(entity, _, key)| (*entity, key.clone()))
        .collect();
    view.camera_entities = camera_entities
        .into_iter()
        .filter(|entity| view.labels.contains_key(entity))
        .collect();
    view.camera_identities = camera_identity_by_entity
        .into_iter()
        .filter(|(entity, _)| view.labels.contains_key(entity))
        .collect();
    view.parents = view
        .labels
        .keys()
        .filter_map(|entity| {
            child_of
                .get(entity)
                .copied()
                .map(|parent| (*entity, parent))
        })
        .collect();
    view.show_system = show_system;
    view.active_scene_root = active_scene_root;
    view.scene_error = scene_error;
    view.candidate_hierarchy_entities = candidate_hierarchy_entities;
    view.candidate_hierarchy_parents = candidate_hierarchy_parents;
    view.built = true;
    view
}

/// Run condition for [`mark_entity_tree_view_dirty`]: report when the scene
/// topology that the tree depends on changes: candidate labels, markers,
/// hierarchy edges, scene boundaries or removals (including despawns). The
/// `Local` flag forces one initial build (a freshly-added system does not see
/// pre-existing entities as `Changed`). On a quiescent scene this returns
/// `false` and the harvest is skipped entirely.
/// Parent insertions arrive through native lifecycle messages, coalesced by
/// entity before comparing current edges. Unnamed candidate ancestors can
/// invalidate descendants; unrelated scene hierarchies remain outside this view.
/// Tracked automatically by `add_view_model` — see [`lunco_core_runtime::gate::tracked`].
pub(crate) fn entity_tree_build_due(build: Res<EntityTreeBuildState>) -> bool {
    build.dirty && build.task.is_none()
}

/// Record a topology invalidation without entering the query-heavy snapshot
/// producer. The worker revision changes immediately, so any in-flight result
/// is rejected when it completes.
pub(crate) fn mark_entity_tree_view_dirty(mut build: ResMut<EntityTreeBuildState>) {
    build.invalidate();
}

pub(crate) fn scene_topology_changed(
    mut first: Local<bool>,
    settings: Res<EntityListSettings>,
    view: Res<EntityTreeView>,
    scene: EntityTreeSceneQueries,
    mut parent_insertions: MessageReader<EntityTreeParentInserted>,
    mut inserted_parents: Local<HashSet<Entity>>,
    changed: Query<
        (
            Entity,
            &Name,
            Option<&lunco_core::markers::Callsign>,
            Option<&lunco_core::CatalogEntryId>,
            Option<&lunco_usd_bevy_scene::UsdPrimPath>,
            Option<&ChildOf>,
            Option<&lunco_core::SystemManaged>,
            Has<Mesh3d>,
            Has<lunco_core::SelectableRoot>,
            Has<SceneCamera>,
        ),
        (
            With<Name>,
            Or<(
                Changed<Name>,
                Changed<lunco_core::markers::Callsign>,
                Changed<lunco_core::CatalogEntryId>,
                Changed<lunco_usd_bevy_scene::UsdPrimPath>,
                Changed<lunco_core::SystemManaged>,
                Added<Mesh3d>,
                Added<lunco_core::SelectableRoot>,
                Added<SceneCamera>,
            )>,
        ),
    >,
    mut removals: EntityTreeRemovals,
) -> bool {
    let (active_scene_root, scene_error) =
        active_scene_root_state(scene.mount.as_deref(), &scene.roots);
    let active_scene_changed =
        view.active_scene_root != active_scene_root || view.scene_error != scene_error;
    let removed_scene_boundary =
        removals.scene_roots.read().count() > 0 || removals.preview_roots.read().count() > 0;

    // Drain removal buffers every frame (keeps them from accumulating) and note
    // whether anything relevant was removed. A removed entity can no longer be
    // queried, so "was it system-owned?" is answered by the view itself: if the
    // tree never showed it, its death cannot change the tree.
    // `fold`, not `any` — `any` short-circuits and would leave the rest of the
    // buffer undrained.
    let drained = |it: &mut dyn Iterator<Item = Entity>| {
        it.fold(false, |acc, e| acc | view.labels.contains_key(&e))
    };
    let removed_child = removals.children.read().fold(false, |acc, entity| {
        acc | (Some(entity) != active_scene_root
            && view.candidate_hierarchy_entities.contains(&entity))
    });
    let removed_system_managed = removals.system_managed.read().fold(false, |acc, entity| {
        acc | (!settings.show_system
            && match belongs_to_active_scene(
                entity,
                active_scene_root,
                &scene.roots,
                &scene.preview_roots,
                &scene.parents,
                &scene.entities,
            ) {
                Ok(belongs) => belongs,
                Err(_) => true,
            })
    });
    let removed = drained(&mut removals.names.read())
        | removed_child
        | drained(&mut removals.meshes.read())
        | drained(&mut removals.selectables.read())
        | drained(&mut removals.callsigns.read())
        | drained(&mut removals.catalog_ids.read())
        | drained(&mut removals.usd_paths.read())
        | removed_scene_boundary
        | removed_system_managed
        | removals.cameras.read().fold(false, |acc, entity| {
            acc | view.camera_entities.contains(&entity)
        });
    // The raw ECS graph contains many named but visibility-pruned implementation
    // entities (telemetry channel holders, transform wrappers, etc.). A change
    // outside the active scene is ignored unless it moves a candidate into that
    // scene or changes a visible candidate's source data.
    let value_changed = |(entity, name, callsign, catalog_id, path, parent): (
        Entity,
        &Name,
        Option<&lunco_core::markers::Callsign>,
        Option<&lunco_core::CatalogEntryId>,
        Option<&lunco_usd_bevy_scene::UsdPrimPath>,
        Option<&ChildOf>,
    )| {
        let Some(cached_label) = view.base_labels.get(&entity) else {
            return false;
        };
        lunco_core::entity_display_name(Some(name), callsign, catalog_id) != *cached_label
            || view
                .stable_keys
                .get(&entity)
                .is_none_or(|cached| stable_key(name, path) != *cached)
            || view.parents.get(&entity).copied() != parent.map(|p| p.parent())
    };
    let named_changed = changed.iter().any(
        |(
            entity,
            name,
            callsign,
            catalog_id,
            path,
            parent,
            system,
            has_mesh,
            has_selectable,
            has_camera,
        )| {
            let was_visible = view.base_labels.contains_key(&entity);
            let newly_visible = !was_visible
                && (settings.show_system || system.is_none())
                && (has_mesh || has_selectable || has_camera);
            let entered_active_scene = newly_visible
                && match belongs_to_active_scene(
                    entity,
                    active_scene_root,
                    &scene.roots,
                    &scene.preview_roots,
                    &scene.parents,
                    &scene.entities,
                ) {
                    Ok(belongs) => belongs,
                    Err(_) => true,
                };
            let system_filter_changed = was_visible && !settings.show_system && system.is_some();
            let camera_changed =
                was_visible && has_camera != view.camera_entities.contains(&entity);
            entered_active_scene
                || system_filter_changed
                || camera_changed
                || value_changed((entity, name, callsign, catalog_id, path, parent))
        },
    );
    // ChildOf is immutable: native insert events cover both initial edges and
    // reparenting. Drain coalesced identities, then compare their current edge
    // with the snapshot; identical parent insertions do not rebuild the tree.
    inserted_parents.extend(parent_insertions.read().map(|insertion| insertion.0));
    let scene_hierarchy_changed = inserted_parents.drain().any(|entity| {
        if Some(entity) == active_scene_root {
            return false;
        }
        let Ok(parent) = scene.parents.get(entity) else {
            return false;
        };
        if view.candidate_hierarchy_entities.contains(&entity) {
            view.candidate_hierarchy_parents.get(&entity).copied() != Some(parent.parent())
        } else {
            match belongs_to_active_scene(
                entity,
                active_scene_root,
                &scene.roots,
                &scene.preview_roots,
                &scene.parents,
                &scene.entities,
            ) {
                Ok(belongs) => belongs,
                Err(_) => true,
            }
        }
    });
    let scene_boundary_changed = scene
        .boundary_changes
        .iter()
        .any(|entity| view.candidate_hierarchy_entities.contains(&entity));
    let invalidated = !*first
        || view.show_system != settings.show_system
        || active_scene_changed
        || named_changed
        || scene_hierarchy_changed
        || scene_boundary_changed
        || removed;
    *first = true;
    invalidated
}

/// Retire the derived tree as soon as its active Twin closes. The next scene
/// mount repopulates it from the new active root; outgoing rows do not linger.
pub(crate) fn on_twin_closed(
    trigger: On<TwinClosed>,
    mut view: ResMut<EntityTreeView>,
    mut build: ResMut<EntityTreeBuildState>,
    mut parent_insertions: ResMut<Messages<EntityTreeParentInserted>>,
) {
    if trigger.event().was_active {
        *view = EntityTreeView::default();
        build.invalidate();
        parent_insertions.clear();
    }
}

/// Entity list panel — hierarchy tree of scene entities.
pub struct EntityList {
    visible_rows: Vec<VisibleEntityRow>,
    visible_rows_revision: Option<u64>,
    visible_rows_dirty: bool,
}

impl Default for EntityList {
    fn default() -> Self {
        Self {
            visible_rows: Vec::new(),
            visible_rows_revision: None,
            visible_rows_dirty: true,
        }
    }
}

struct VisibleEntityRow {
    entity: Entity,
    depth: usize,
    branch_id: Option<egui::Id>,
}

impl Panel for EntityList {
    fn id(&self) -> PanelId {
        PanelId("entity_list")
    }
    fn title(&self) -> String {
        "Entities".into()
    }
    fn default_slot(&self) -> PanelSlot {
        PanelSlot::SideBrowser
    }
    fn menu_group(&self) -> lunco_workbench_core::PanelMenuGroup {
        lunco_workbench_core::PanelMenuGroup::Builder
    }
    fn transparent_background(&self) -> bool {
        true
    }

    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut PanelCtx) {
        ctx.panel_content_frame().show(ui, |ui| {
            entity_list_content(
                ui,
                ctx,
                &mut self.visible_rows,
                &mut self.visible_rows_revision,
                &mut self.visible_rows_dirty,
            )
        });
    }
}

/// Rebuild the open portion of the cached hierarchy with stable entity-keyed
/// branch IDs. The resulting rows are painted through `ScrollArea::show_rows`,
/// so offscreen descendants do not allocate widgets or text layouts.
fn collect_visible_rows(
    ctx: &egui::Context,
    entity: Entity,
    depth: usize,
    view: &EntityTreeView,
    rows: &mut Vec<VisibleEntityRow>,
) {
    let branch_id = view
        .kids
        .contains_key(&entity)
        .then(|| egui::Id::new(("entity_tree", entity)));
    rows.push(VisibleEntityRow {
        entity,
        depth,
        branch_id,
    });

    let (Some(branch_id), Some(children)) = (branch_id, view.kids.get(&entity)) else {
        return;
    };
    let default_open = lunco_workbench_widgets::tree::default_open_at_depth(depth);
    let state = egui::collapsing_header::CollapsingState::load_with_default_open(
        ctx,
        branch_id,
        default_open,
    );
    if !state.is_open() {
        return;
    }

    for &child in children {
        collect_visible_rows(ctx, child, depth + 1, view, rows);
    }
}

/// Render one flattened hierarchy row. The branch body is painted separately
/// by the virtualized list, while its persistent disclosure state remains the
/// same as the ordinary shared tree branch.
fn render_node_row(
    ui: &mut egui::Ui,
    row: &VisibleEntityRow,
    view: &EntityTreeView,
    selected: &lunco_scene_selection::SelectedEntities,
    shift_held: bool,
    to_select: &mut Option<(Entity, bool)>,
    to_focus: &mut Option<Entity>,
    tree_changed: &mut bool,
) {
    let entity = row.entity;
    let label = view
        .labels
        .get(&entity)
        .map(String::as_str)
        .unwrap_or("Unnamed entity");

    let Some(id) = row.branch_id else {
        let _ = lunco_workbench_widgets::tree::leaf(ui, |ui| {
            select_label(
                ui,
                entity,
                label,
                view.camera_identities.get(&entity).map(String::as_str),
                selected,
                shift_held,
                to_select,
                to_focus,
            )
        });
        return;
    };

    let mut header_select = None;
    let mut header_focus = None;
    let branch_state = lunco_workbench_widgets::tree::branch_header(
        ui,
        id,
        lunco_workbench_widgets::tree::default_open_at_depth(row.depth),
        None,
        |ui| {
            select_label(
                ui,
                entity,
                label,
                view.camera_identities.get(&entity).map(String::as_str),
                selected,
                shift_held,
                &mut header_select,
                &mut header_focus,
            )
        },
    );
    *tree_changed |= branch_state.changed;
    if header_select.is_some() {
        *to_select = header_select;
    }
    if header_focus.is_some() {
        *to_focus = header_focus;
    }
}

/// A selectable entity label: single click selects, double click also flags it
/// for camera focus. Shared by every row in the tree so the click/double-click
/// behaviour stays identical at every depth.
fn select_label(
    ui: &mut egui::Ui,
    entity: Entity,
    label: &str,
    full_identity: Option<&str>,
    selected: &lunco_scene_selection::SelectedEntities,
    shift_held: bool,
    to_select: &mut Option<(Entity, bool)>,
    to_focus: &mut Option<Entity>,
) -> bool {
    let width = ui.available_width();
    let response = lunco_workbench_widgets::tree::selectable_label(
        ui,
        selected.entities.contains(&entity),
        label,
        width,
    );
    let resp = if response.hovered() {
        match full_identity {
            Some(identity) => response.on_hover_text(format!(
                "{identity}  ·  click to select · Shift+Click to multiselect · double-click to focus"
            )),
            None => response.on_hover_text(
                "Click to select · Shift+Click to multiselect · double-click to focus",
            ),
        }
    } else {
        response
    };

    if resp.clicked() {
        *to_select = Some((entity, shift_held));
    }
    if resp.double_clicked() {
        *to_select = Some((entity, shift_held));
        *to_focus = Some(entity);
    }
    resp.clicked()
}

fn entity_list_content(
    ui: &mut egui::Ui,
    ctx: &mut PanelCtx,
    visible_rows: &mut Vec<VisibleEntityRow>,
    visible_rows_revision: &mut Option<u64>,
    visible_rows_dirty: &mut bool,
) {
    ui.label("Click to select. Expand > to reach sub-parts (wheels, body).");
    if let Some(view) = ctx.resource::<EntityTreeView>() {
        match view.scene_error.as_deref() {
            Some(error) => {
                ui.horizontal(|ui| {
                    ui.label("Scene ownership error:");
                    ui.label(error);
                });
            }
            None if view.active_scene_root.is_some() => {
                ui.label("Scene scope: Active scene");
            }
            None => {
                ui.label("No active scene mounted.");
            }
        };
    }
    ui.separator();

    // Authoritative selection — read directly (small, cheap); never shadowed.
    let empty_selection = lunco_scene_selection::SelectedEntities::default();
    let selected = ctx
        .resource::<lunco_scene_selection::SelectedEntities>()
        .unwrap_or(&empty_selection);
    let shift_held = ui.input(|i| i.modifiers.shift);

    let mut to_select: Option<(Entity, bool)> = None;
    let mut to_focus: Option<Entity> = None;

    // Borrow the precomputed view for the duration of painting only, then drop
    // it so `ctx` is free for the selection/focus mutations below.
    {
        let Some(view) = ctx.resource::<EntityTreeView>() else {
            return;
        };

        // One panel-level scroll area owns the open hierarchy. `show_rows`
        // reserves its full extent while constructing widgets only for rows
        // that intersect the viewport.
        if *visible_rows_dirty || *visible_rows_revision != Some(view.revision) {
            visible_rows.clear();
            for &root in &view.roots {
                collect_visible_rows(ui.ctx(), root, 0, view, visible_rows);
            }
            *visible_rows_revision = Some(view.revision);
            *visible_rows_dirty = false;
        }
        let mut tree_changed = false;
        let row_height = ui.spacing().interact_size.y;
        egui::ScrollArea::vertical()
            .id_salt("entity_list_scroll")
            .auto_shrink([false; 2])
            .show_rows(ui, row_height, visible_rows.len(), |ui, range| {
                for row_index in range {
                    let row = &visible_rows[row_index];
                    ui.push_id(("entity_tree_row", row.entity), |ui| {
                        let indent = ui.spacing().indent * row.depth as f32;
                        ui.horizontal(|ui| {
                            ui.add_space(indent);
                            ui.vertical(|ui| {
                                render_node_row(
                                    ui,
                                    row,
                                    view,
                                    selected,
                                    shift_held,
                                    &mut to_select,
                                    &mut to_focus,
                                    &mut tree_changed,
                                );
                            });
                        });
                    });
                }
            });
        if selected
            .entities
            .iter()
            .any(|entity| !view.labels.contains_key(entity))
        {
            ui.label("A selected entity is outside the active scene tree.");
        }
        if tree_changed {
            *visible_rows_dirty = true;
        }
    }

    // Route selection through the same selection owner the viewport click and
    // `SelectEntity` API use — keyed by `Entity` (sub-parts share api_ids, so id
    // round-trips select the wrong instance). Explorer Shift-click retains its
    // established toggle behavior; viewport Shift-click uses Extend.
    if let Some((entity, shift_held)) = to_select {
        ctx.trigger(SelectEntityTarget {
            target: entity,
            intent: if shift_held {
                SelectionIntent::Toggle
            } else {
                SelectionIntent::Replace
            },
        });
    }

    // Double-click flies the camera to the entity via the same `FocusEntityById`
    // command the API exposes. Works for anything with an API id — no collider
    // required (this is list-driven, not a viewport raycast).
    if let Some(entity) = to_focus {
        let id = ctx
            .resource::<lunco_api::registry::ApiEntityRegistry>()
            .and_then(|r| r.api_id_for(entity))
            .map(|g| g.get());
        if let Some(id) = id {
            ctx.trigger(lunco_scene_camera::FocusEntityById {
                entity_id: id,
                distance: 0.0,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Resource, Default)]
    struct GateRuns(u32);

    fn count_gate_run(mut runs: ResMut<GateRuns>) {
        runs.0 += 1;
    }

    #[test]
    fn topology_gate_tracks_only_entities_in_the_active_scene() {
        let mut app = App::new();
        install_entity_tree_hierarchy_tracking(&mut app);
        app.init_resource::<EntityListSettings>()
            .init_resource::<EntityTreeView>()
            .init_resource::<lunco_core::SceneMountState>()
            .init_resource::<GateRuns>()
            .add_systems(Update, count_gate_run.run_if(scene_topology_changed));
        let active_root = app
            .world_mut()
            .spawn(lunco_usd_bevy_scene::UsdSceneRoot)
            .id();
        app.world_mut()
            .resource_mut::<lunco_core::SceneMountState>()
            .register_root(active_root, true);

        app.update();
        assert_eq!(app.world().resource::<GateRuns>().0, 1);
        app.world_mut()
            .resource_mut::<EntityTreeView>()
            .active_scene_root = Some(active_root);

        let additive_root = app
            .world_mut()
            .spawn(lunco_usd_bevy_scene::UsdSceneRoot)
            .id();
        app.world_mut().spawn((
            Name::new("Additive Entity"),
            lunco_core::SelectableRoot,
            ChildOf(additive_root),
        ));
        app.update();
        assert_eq!(app.world().resource::<GateRuns>().0, 1);

        app.world_mut().spawn((
            Name::new("Rover"),
            lunco_core::SelectableRoot,
            ChildOf(active_root),
        ));
        app.update();

        assert_eq!(app.world().resource::<GateRuns>().0, 2);
    }

    #[test]
    fn unchanged_parent_ticks_do_not_rebuild_the_active_tree() {
        let mut app = App::new();
        install_entity_tree_hierarchy_tracking(&mut app);
        app.init_resource::<EntityListSettings>()
            .init_resource::<EntityTreeView>()
            .init_resource::<lunco_core::SceneMountState>()
            .init_resource::<GateRuns>()
            .add_systems(Update, count_gate_run.run_if(scene_topology_changed));
        let active_root = app
            .world_mut()
            .spawn(lunco_usd_bevy_scene::UsdSceneRoot)
            .id();
        let rover = app
            .world_mut()
            .spawn((
                Name::new("Rover"),
                lunco_core::SelectableRoot,
                ChildOf(active_root),
            ))
            .id();
        app.world_mut()
            .resource_mut::<lunco_core::SceneMountState>()
            .register_root(active_root, true);
        {
            let mut view = app.world_mut().resource_mut::<EntityTreeView>();
            view.active_scene_root = Some(active_root);
            view.labels.insert(rover, "Rover".into());
            view.base_labels.insert(rover, "Rover".into());
            view.stable_keys.insert(rover, "Rover".into());
            view.parents.insert(rover, active_root);
            view.candidate_hierarchy_entities = HashSet::from([active_root, rover]);
            view.candidate_hierarchy_parents.insert(rover, active_root);
        }

        app.update();
        assert_eq!(app.world().resource::<GateRuns>().0, 1);

        app.world_mut()
            .entity_mut(rover)
            .insert(ChildOf(active_root));
        app.update();

        assert_eq!(app.world().resource::<GateRuns>().0, 1);
        app.update();
        assert_eq!(app.world().resource::<GateRuns>().0, 1);

        let additive_root = app
            .world_mut()
            .spawn(lunco_usd_bevy_scene::UsdSceneRoot)
            .id();
        app.world_mut()
            .entity_mut(rover)
            .insert(ChildOf(additive_root));
        app.update();
        assert_eq!(app.world().resource::<GateRuns>().0, 2);
        app.world_mut()
            .resource_mut::<EntityTreeView>()
            .parents
            .insert(rover, additive_root);
        app.world_mut()
            .resource_mut::<EntityTreeView>()
            .candidate_hierarchy_parents
            .insert(rover, additive_root);

        app.world_mut()
            .entity_mut(rover)
            .insert(ChildOf(active_root));
        app.update();
        assert_eq!(app.world().resource::<GateRuns>().0, 3);
        app.world_mut()
            .resource_mut::<EntityTreeView>()
            .parents
            .insert(rover, active_root);
        app.world_mut()
            .resource_mut::<EntityTreeView>()
            .candidate_hierarchy_parents
            .insert(rover, active_root);

        app.world_mut().entity_mut(rover).remove::<ChildOf>();
        app.update();
        assert_eq!(app.world().resource::<GateRuns>().0, 4);
        app.world_mut().despawn(rover);
        app.update();
        assert_eq!(app.world().resource::<GateRuns>().0, 5);
        app.update();
        assert_eq!(app.world().resource::<GateRuns>().0, 5);
    }

    #[test]
    fn hierarchy_insertions_track_unnamed_candidate_ancestors() {
        let mut app = App::new();
        install_entity_tree_hierarchy_tracking(&mut app);
        app.init_resource::<EntityListSettings>()
            .init_resource::<EntityTreeView>()
            .init_resource::<lunco_core::SceneMountState>()
            .init_resource::<GateRuns>()
            .add_systems(Update, count_gate_run.run_if(scene_topology_changed));
        let active_root = app
            .world_mut()
            .spawn(lunco_usd_bevy_scene::UsdSceneRoot)
            .id();
        let additive_root = app
            .world_mut()
            .spawn(lunco_usd_bevy_scene::UsdSceneRoot)
            .id();
        let wrapper = app.world_mut().spawn(ChildOf(active_root)).id();
        let rover = app
            .world_mut()
            .spawn((
                Name::new("Rover"),
                lunco_core::SelectableRoot,
                ChildOf(wrapper),
            ))
            .id();
        app.world_mut()
            .resource_mut::<lunco_core::SceneMountState>()
            .register_root(active_root, true);
        {
            let mut view = app.world_mut().resource_mut::<EntityTreeView>();
            view.active_scene_root = Some(active_root);
            view.labels.insert(rover, "Rover".into());
            view.base_labels.insert(rover, "Rover".into());
            view.stable_keys.insert(rover, "Rover".into());
            view.parents.insert(rover, wrapper);
            view.candidate_hierarchy_entities = HashSet::from([active_root, wrapper, rover]);
            view.candidate_hierarchy_parents =
                HashMap::from([(wrapper, active_root), (rover, wrapper)]);
        }
        app.update();
        app.world_mut()
            .entity_mut(wrapper)
            .insert(ChildOf(active_root));
        app.update();
        assert_eq!(app.world().resource::<GateRuns>().0, 1);

        app.world_mut()
            .entity_mut(wrapper)
            .insert(ChildOf(additive_root));
        app.update();
        assert_eq!(app.world().resource::<GateRuns>().0, 2);
        app.update();
        assert_eq!(app.world().resource::<GateRuns>().0, 2);

        app.world_mut().entity_mut(wrapper).remove::<ChildOf>();
        app.update();
        assert_eq!(app.world().resource::<GateRuns>().0, 3);
    }

    #[test]
    fn duplicate_labels_are_ordered_by_stable_source_key() {
        let first = Entity::from_raw_u32(1).unwrap();
        let second = Entity::from_raw_u32(2).unwrap();
        let named = vec![
            (second, "Rocker Bogie".to_string(), "/Scene/B".to_string()),
            (first, "Rocker Bogie".to_string(), "/Scene/A".to_string()),
        ];
        let shown = HashMap::from([(first, true), (second, true)]);

        let labels = disambiguate_labels(&named, &shown);

        assert_eq!(labels[&first], "Rocker Bogie (1)");
        assert_eq!(labels[&second], "Rocker Bogie (2)");
    }

    #[test]
    fn hidden_duplicates_do_not_change_visible_labels() {
        let visible = Entity::from_raw_u32(1).unwrap();
        let hidden = Entity::from_raw_u32(2).unwrap();
        let named = vec![
            (visible, "Antenna".to_string(), "/Scene/A".to_string()),
            (hidden, "Antenna".to_string(), "/Scene/B".to_string()),
        ];
        let shown = HashMap::from([(visible, true), (hidden, false)]);

        let labels = disambiguate_labels(&named, &shown);

        assert_eq!(labels[&visible], "Antenna");
        assert!(!labels.contains_key(&hidden));
    }

    #[test]
    fn active_scene_membership_follows_the_mount_through_nested_hierarchy() {
        let mut world = World::new();
        let outer_grid = world.spawn(big_space::prelude::Grid::new(2000.0, 0.0)).id();
        let active_root = world
            .spawn((
                lunco_usd_bevy_scene::UsdSceneRoot,
                big_space::prelude::Grid::new(2000.0, 0.0),
                ChildOf(outer_grid),
            ))
            .id();
        let wrapper = world.spawn(ChildOf(active_root)).id();
        let rover = world.spawn(ChildOf(wrapper)).id();
        let preview_root = world
            .spawn((lunco_usd_bevy_scene::UsdPreviewOnly, ChildOf(active_root)))
            .id();
        let preview_prim = world.spawn(ChildOf(preview_root)).id();
        let additive_root = world.spawn(lunco_usd_bevy_scene::UsdSceneRoot).id();
        let additive_prim = world.spawn(ChildOf(additive_root)).id();
        let mut queries = bevy::ecs::system::SystemState::<(
            Query<(), With<lunco_usd_bevy_scene::UsdSceneRoot>>,
            Query<(), With<lunco_usd_bevy_scene::UsdPreviewOnly>>,
            Query<&ChildOf>,
            Query<Entity>,
        )>::new(&mut world);
        let (scene_roots, preview_roots, parents, entities) = queries.get(&world).unwrap();

        assert_eq!(
            belongs_to_active_scene(
                rover,
                Some(active_root),
                &scene_roots,
                &preview_roots,
                &parents,
                &entities,
            ),
            Ok(true)
        );
        assert_eq!(
            belongs_to_active_scene(
                additive_prim,
                Some(active_root),
                &scene_roots,
                &preview_roots,
                &parents,
                &entities,
            ),
            Ok(false)
        );
        assert_eq!(
            belongs_to_active_scene(
                preview_prim,
                Some(active_root),
                &scene_roots,
                &preview_roots,
                &parents,
                &entities,
            ),
            Ok(false)
        );
    }

    #[test]
    fn candidate_hierarchy_snapshot_reaches_the_active_scene_root() {
        let root = Entity::from_raw_u32(1).unwrap();
        let mut child_of = HashMap::new();
        let mut parent = root;
        for raw in 2..=80 {
            let child = Entity::from_raw_u32(raw).unwrap();
            child_of.insert(child, parent);
            parent = child;
        }
        let mut ancestors = HashSet::new();
        let mut hierarchy_parents = HashMap::new();

        collect_candidate_hierarchy(
            parent,
            root,
            &child_of,
            &mut ancestors,
            &mut hierarchy_parents,
        );

        assert_eq!(ancestors.len(), 80);
        assert_eq!(hierarchy_parents.len(), 79);
        assert!(ancestors.contains(&root));
    }

    #[test]
    fn active_twin_close_clears_the_derived_scene_tree() {
        let mut app = App::new();
        install_entity_tree_hierarchy_tracking(&mut app);
        app.init_resource::<EntityTreeView>()
            .init_resource::<EntityTreeBuildState>()
            .add_observer(on_twin_closed);
        {
            let mut view = app.world_mut().resource_mut::<EntityTreeView>();
            view.built = true;
            view.active_scene_root = Some(Entity::from_raw_u32(1).unwrap());
            view.scene_error = Some("stale".into());
        }
        let parent = app.world_mut().spawn_empty().id();
        app.world_mut().spawn(ChildOf(parent));
        assert!(
            !app.world()
                .resource::<Messages<EntityTreeParentInserted>>()
                .is_empty()
        );

        app.world_mut().trigger(TwinClosed {
            twin: lunco_workspace::TwinId::new(7),
            root: std::path::PathBuf::from("/outgoing"),
            was_active: true,
        });

        let view = app.world().resource::<EntityTreeView>();
        assert!(!view.built);
        assert_eq!(view.active_scene_root, None);
        assert_eq!(view.scene_error, None);
        assert!(
            app.world()
                .resource::<Messages<EntityTreeParentInserted>>()
                .is_empty()
        );
    }
}
