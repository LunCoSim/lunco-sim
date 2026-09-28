//! Entity list panel — `lunco-workbench::Panel` implementation.
//!
//! A hierarchy tree of scene objects: top-level objects (rovers, props,
//! terrain, cosim blocks) with their sub-parts (wheels, body) nested beneath,
//! so you can drill in and select a single wheel. Internal plumbing (cosim
//! wires, ports, empty transform wrappers) is hidden — only entities that are
//! selectable or mesh-bearing, plus their ancestors, appear. Clicking a node
//! selects it.
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

/// Generic Twin setting key for the entity tree's grid visibility.
pub const ENTITY_LIST_GRID_SCOPE_SETTING: &str = "ui.entity_list.grid_scope";

/// Which BigSpace grid the entity tree includes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum EntityGridScope {
    /// Only entities under the live [`lunco_spatial::ActivePhysicsFrame`].
    #[default]
    Current,
    /// Entities from every grid in the mounted scene.
    All,
}

impl EntityGridScope {
    fn setting_value(self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::All => "all",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Current => "Current grid only",
            Self::All => "All grids",
        }
    }
}

fn parse_entity_grid_scope(
    value: &lunco_workspace::TwinSettingValue,
) -> Result<EntityGridScope, String> {
    match value {
        lunco_workspace::TwinSettingValue::Text(value) => match value.as_str() {
            "current" => Ok(EntityGridScope::Current),
            "all" => Ok(EntityGridScope::All),
            _ => Err(format!(
                "`{ENTITY_LIST_GRID_SCOPE_SETTING}` must be `current` or `all`, got `{value}`"
            )),
        },
        other => Err(format!(
            "`{ENTITY_LIST_GRID_SCOPE_SETTING}` must be text (`current` or `all`), got {other:?}"
        )),
    }
}

fn entity_grid_scope(workspace: Option<&WorkspaceResource>) -> Result<EntityGridScope, String> {
    let Some(workspace) = workspace else {
        return Ok(EntityGridScope::Current);
    };
    let Some(twin_id) = workspace.active_twin else {
        return Ok(EntityGridScope::Current);
    };
    let Some(twin) = workspace.twin(twin_id) else {
        return Err(format!("active Twin {twin_id:?} is no longer registered"));
    };
    let Some(manifest) = twin.manifest.as_ref() else {
        return Ok(EntityGridScope::Current);
    };
    manifest
        .setting(ENTITY_LIST_GRID_SCOPE_SETTING)
        .map_or(Ok(EntityGridScope::Current), parse_entity_grid_scope)
}

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
        ui.label(egui::RichText::new("Entity list").weak().small());
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
            egui::RichText::new("Grid scope (active Twin)")
                .weak()
                .small(),
        );
        let Some((scope_result, can_persist)) =
            ctx.resource::<WorkspaceResource>().map(|workspace| {
                let can_persist = workspace
                    .active_twin
                    .and_then(|id| workspace.twin(id))
                    .is_some_and(|twin| twin.manifest.is_some());
                (entity_grid_scope(Some(workspace)), can_persist)
            })
        else {
            ui.label("No workspace session is available.");
            return;
        };
        match scope_result {
            Ok(current_scope) => {
                let mut next_scope = current_scope;
                ui.add_enabled_ui(can_persist, |ui| {
                    ui.radio_value(
                        &mut next_scope,
                        EntityGridScope::Current,
                        EntityGridScope::Current.label(),
                    );
                    ui.radio_value(
                        &mut next_scope,
                        EntityGridScope::All,
                        EntityGridScope::All.label(),
                    );
                });
                if !can_persist {
                    ui.weak("Open a manifest-backed Twin to persist this choice.");
                } else if next_scope != current_scope {
                    ctx.trigger(SetTwinSetting {
                        key: ENTITY_LIST_GRID_SCOPE_SETTING.into(),
                        value: TwinSettingInput::Text(next_scope.setting_value().into()),
                    });
                }
            }
            Err(error) => {
                ui.label(format!("Grid scope error: {error}"));
            }
        }

        ui.separator();
        ui.label(
            egui::RichText::new("Runtime scene edits (active Twin)")
                .weak()
                .small(),
        );
        ui.small("Route points, runtime spawns, and gizmo edits use Twin Runtime (@runtime@).");
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
    /// this value rather than trusting a `Changed<ChildOf>` tick: grid and
    /// celestial systems may re-stamp an identical parent every frame.
    parents: HashMap<Entity, Entity>,
    /// The filter value used for the cached tree.
    show_system: bool,
    /// The active Twin's grid-scope choice used for the cached tree.
    grid_scope: EntityGridScope,
    /// The active Twin that supplied [`grid_scope`](Self::grid_scope).
    active_twin: Option<lunco_workspace::TwinId>,
    /// The live physics grid when one was available during the build.
    current_grid: Option<Entity>,
    /// An invalid setting or missing current frame prevents a misleading tree.
    scope_error: Option<String>,
    /// Named entities eligible for the current system-entity preference. This
    /// lets the gate notice a hidden entity moving into the current grid.
    scope_entities: HashSet<Entity>,
    /// Unnamed ancestors whose reparenting can change an entity's grid scope.
    scope_ancestors: HashSet<Entity>,
    /// Set once the first build runs, so the change-gate forces an initial fill.
    built: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct GridScopeState {
    scope: EntityGridScope,
    active_twin: Option<lunco_workspace::TwinId>,
    current_grid: Option<Entity>,
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
    scope_state: GridScopeState,
    scope_error: Option<String>,
    child_of: HashMap<Entity, Entity>,
    grids: HashSet<Entity>,
    named_candidates: Vec<NamedTreeCandidate>,
}

#[derive(Resource, Default)]
pub(crate) struct EntityTreeBuildState {
    revision: u64,
    dirty: bool,
    task: Option<Task<(u64, EntityTreeView)>>,
}

impl EntityTreeBuildState {
    fn invalidate(&mut self) {
        self.revision = self.revision.wrapping_add(1);
        self.dirty = true;
    }
}

fn grid_scope_state(
    workspace: Option<&WorkspaceResource>,
    current_grid: Option<Entity>,
    active_frame_bound: bool,
) -> (GridScopeState, Option<String>) {
    let scope_result = entity_grid_scope(workspace);
    let scope = scope_result.as_ref().copied().unwrap_or_default();
    let active_twin = workspace.and_then(|workspace| workspace.active_twin);
    let error = scope_result.err().or_else(|| {
        if scope == EntityGridScope::Current {
            match current_grid {
                None if !active_frame_bound => {
                    Some("current grid is unavailable: no ActivePhysicsFrame is bound".into())
                }
                None => Some(
                    "current grid is unavailable: ActivePhysicsFrame is not a live Grid".into(),
                ),
                Some(_) => None,
            }
        } else {
            None
        }
    });
    (
        GridScopeState {
            scope,
            active_twin,
            current_grid,
        },
        error,
    )
}

fn nearest_grid(
    entity: Entity,
    child_of: &HashMap<Entity, Entity>,
    grids: &HashSet<Entity>,
) -> Option<Entity> {
    let mut current = entity;
    for _ in 0..64 {
        if grids.contains(&current) {
            return Some(current);
        }
        let parent = child_of.get(&current).copied()?;
        current = parent;
    }
    None
}

fn collect_scope_ancestors(
    entity: Entity,
    child_of: &HashMap<Entity, Entity>,
    ancestors: &mut HashSet<Entity>,
) {
    let mut current = entity;
    for _ in 0..64 {
        if !ancestors.insert(current) {
            return;
        }
        let Some(parent) = child_of.get(&current).copied() else {
            return;
        };
        current = parent;
    }
}

fn in_scope(
    entity: Entity,
    state: GridScopeState,
    child_of: &HashMap<Entity, Entity>,
    grids: &HashSet<Entity>,
) -> bool {
    match state.scope {
        EntityGridScope::All => true,
        EntityGridScope::Current => nearest_grid(entity, child_of, grids) == state.current_grid,
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

/// Change-driven producer for [`EntityTreeView`]. A **normal** Bevy system with
/// cached `Query` params — no per-frame `QueryState` rebuild — gated by
/// [`scene_topology_changed`] so the whole harvest only runs when the scene
/// actually changes. This is the entire cost the old per-frame `render` paid; it
/// now runs ~once per topology change instead of every frame.
pub(crate) fn populate_entity_tree_view(
    mut build: ResMut<EntityTreeBuildState>,
    settings: Res<EntityListSettings>,
    workspace: Option<Res<WorkspaceResource>>,
    active_frame: Option<Res<lunco_spatial::ActivePhysicsFrame>>,
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
    child_q: Query<(Entity, &ChildOf)>,
    grid_q: Query<Entity, With<big_space::prelude::Grid>>,
) {
    if build.task.is_some() {
        build.invalidate();
        return;
    }
    build.invalidate();

    let input = {
        let _span = bevy::log::info_span!("entity_tree_view_snapshot").entered();
        // System-owned churn is excluded before the immutable snapshot leaves the
        // ECS thread. The hierarchy derivation below uses only owned values.
        let grids: HashSet<Entity> = grid_q.iter().collect();
        let (scope_state, scope_error) = grid_scope_state(
            workspace.as_deref(),
            active_frame
                .as_ref()
                .map(|frame| frame.0)
                .filter(|grid| grids.contains(grid)),
            active_frame.is_some(),
        );
        let child_of: HashMap<Entity, Entity> = child_q
            .iter()
            .map(|(entity, child)| (entity, child.parent()))
            .collect();
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
                    if !settings.show_system && is_system {
                        return None;
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
        EntityTreeBuildInput {
            show_system: settings.show_system,
            scope_state,
            scope_error,
            child_of,
            grids,
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
    let Some((revision, result)) = completed else {
        return;
    };
    build.task = None;
    if revision == build.revision {
        *view = result;
        build.dirty = false;
    } else {
        build.dirty = true;
    }
}

fn derive_entity_tree_view(input: EntityTreeBuildInput) -> EntityTreeView {
    let EntityTreeBuildInput {
        show_system,
        scope_state,
        scope_error,
        child_of,
        grids,
        named_candidates,
    } = input;
    let mut scope_entities = HashSet::new();
    let mut scope_ancestors = HashSet::new();
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
        scope_entities.insert(entity);
        collect_scope_ancestors(entity, &child_of, &mut scope_ancestors);
        if scope_error.is_some() || !in_scope(entity, scope_state, &child_of, &grids) {
            continue;
        }
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
        for _ in 0..64 {
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
    view.grid_scope = scope_state.scope;
    view.active_twin = scope_state.active_twin;
    view.current_grid = scope_state.current_grid;
    view.scope_error = scope_error;
    view.scope_entities = scope_entities;
    view.scope_ancestors = scope_ancestors;
    view.built = true;
    view
}

/// Run condition for [`populate_entity_tree_view`]: rebuild only when the scene
/// topology that the tree depends on changes — a **named** node's hierarchy is
/// added or modified (`Changed` includes `Added`), the interesting marker sets
/// gain members, or any of those components are removed (covers despawns). The
/// `Local` flag forces one initial build (a freshly-added system does not see
/// pre-existing entities as `Changed`). On a quiescent scene this returns
/// `false` and the harvest is skipped entirely.
/// The harvest renders only named nodes. Its gate must therefore ignore changes
/// to unnamed internal wrappers as well as system-owned entities (unless shown):
/// terrain streaming and render extraction create both continuously, and neither
/// can change the visible tree by itself.
/// Tracked automatically by `add_view_model` — see [`lunco_core_runtime::gate::tracked`].
pub(crate) fn entity_tree_build_due(build: Res<EntityTreeBuildState>) -> bool {
    build.dirty && build.task.is_none()
}

pub(crate) fn scene_topology_changed(
    mut first: Local<bool>,
    settings: Res<EntityListSettings>,
    view: Res<EntityTreeView>,
    workspace: Option<Res<WorkspaceResource>>,
    active_frame: Option<Res<lunco_spatial::ActivePhysicsFrame>>,
    grids: Query<Entity, With<big_space::prelude::Grid>>,
    changed_unnamed_parents: Query<Entity, (Changed<ChildOf>, Without<Name>)>,
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
                Changed<ChildOf>,
                Changed<lunco_core::markers::Callsign>,
                Changed<lunco_core::CatalogEntryId>,
                Changed<lunco_usd_bevy_scene::UsdPrimPath>,
                Added<Mesh3d>,
                Added<lunco_core::SelectableRoot>,
                Added<SceneCamera>,
            )>,
        ),
    >,
    mut rm_name: RemovedComponents<Name>,
    mut rm_child: RemovedComponents<ChildOf>,
    mut rm_mesh: RemovedComponents<Mesh3d>,
    mut rm_sel: RemovedComponents<lunco_core::SelectableRoot>,
    mut rm_callsign: RemovedComponents<lunco_core::markers::Callsign>,
    mut rm_catalog: RemovedComponents<lunco_core::CatalogEntryId>,
    mut rm_usd_path: RemovedComponents<lunco_usd_bevy_scene::UsdPrimPath>,
    mut rm_camera: RemovedComponents<SceneCamera>,
) -> bool {
    let current_grid = active_frame
        .as_ref()
        .map(|frame| frame.0)
        .filter(|grid| grids.get(*grid).is_ok());
    let (scope_state, scope_error) =
        grid_scope_state(workspace.as_deref(), current_grid, active_frame.is_some());
    let scope_changed = view.grid_scope != scope_state.scope
        || view.active_twin != scope_state.active_twin
        || view.current_grid != scope_state.current_grid
        || view.scope_error != scope_error;

    // Drain removal buffers every frame (keeps them from accumulating) and note
    // whether anything relevant was removed. A removed entity can no longer be
    // queried, so "was it system-owned?" is answered by the view itself: if the
    // tree never showed it, its death cannot change the tree.
    // `fold`, not `any` — `any` short-circuits and would leave the rest of the
    // buffer undrained.
    let drained = |it: &mut dyn Iterator<Item = Entity>| {
        it.fold(false, |acc, e| acc | view.labels.contains_key(&e))
    };
    let removed = drained(&mut rm_name.read())
        | drained(&mut rm_child.read())
        | drained(&mut rm_mesh.read())
        | drained(&mut rm_sel.read())
        | drained(&mut rm_callsign.read())
        | drained(&mut rm_catalog.read())
        | drained(&mut rm_usd_path.read())
        | rm_camera.read().fold(false, |acc, entity| {
            acc | view.camera_entities.contains(&entity)
        });
    // The raw ECS graph contains many named but visibility-pruned implementation
    // entities (telemetry channel holders, transform wrappers, etc.). A change
    // to an entity outside the cached scope is ignored unless it is a newly
    // eligible named entity. Unnamed ancestors are handled separately below,
    // because their reparenting can change a descendant's grid membership.
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
            let eligible =
                view.scope_entities.contains(&entity) || system.is_none() || settings.show_system;
            let newly_visible = !view.base_labels.contains_key(&entity)
                && (has_mesh || has_selectable || has_camera);
            eligible
                && (newly_visible
                    || view.scope_entities.contains(&entity)
                    || value_changed((entity, name, callsign, catalog_id, path, parent)))
        },
    );
    let unnamed_parent_changed = changed_unnamed_parents
        .iter()
        .any(|entity| view.scope_ancestors.contains(&entity));
    let invalidated = !*first
        || view.show_system != settings.show_system
        || scope_changed
        || named_changed
        || unnamed_parent_changed
        || removed;
    *first = true;
    invalidated
}

/// Retire the derived tree as soon as its active Twin closes. The next Twin's
/// scene repopulates it from its own manifest and grid frame; no old scope or
/// rows remain visible during the transition.
pub(crate) fn on_twin_closed(
    trigger: On<TwinClosed>,
    mut view: ResMut<EntityTreeView>,
    mut build: ResMut<EntityTreeBuildState>,
) {
    if trigger.event().was_active {
        *view = EntityTreeView::default();
        build.invalidate();
    }
}

/// Entity list panel — hierarchy tree of scene entities.
pub struct EntityList;

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
        ctx.panel_content_frame()
            .show(ui, |ui| entity_list_content(ui, ctx));
    }
}

/// Render one tree node and its descendants. Children in the view are already
/// visibility-pruned and sorted, so this is pure paint — leaf nodes are a
/// selectable label; branch nodes use the shared workbench tree renderer whose
/// header is itself selectable, so a click on the rover selects the rover and
/// the triangle drills into its wheels.
fn render_node(
    ui: &mut egui::Ui,
    entity: Entity,
    depth: usize,
    view: &EntityTreeView,
    selected: &lunco_scene_selection::SelectedEntities,
    to_select: &mut Option<(Entity, bool)>,
    to_focus: &mut Option<Entity>,
) {
    let label = view
        .labels
        .get(&entity)
        .cloned()
        .unwrap_or_else(|| "Unnamed entity".to_string());

    match view.kids.get(&entity) {
        None => {
            let _ = lunco_workbench_widgets::tree::leaf(ui, |ui| {
                select_label(
                    ui,
                    entity,
                    &label,
                    view.camera_identities.get(&entity).map(String::as_str),
                    selected,
                    to_select,
                    to_focus,
                )
            });
        }
        Some(children) => {
            let id = ui.make_persistent_id(("entity_tree", entity));
            let mut header_select = None;
            let mut header_focus = None;
            // Match Telemetry's initial presentation: reveal two levels while
            // leaving deeper sub-part detail collapsed.
            lunco_workbench_widgets::tree::branch(
                ui,
                id,
                lunco_workbench_widgets::tree::default_open_at_depth(depth),
                None,
                |ui| {
                    select_label(
                        ui,
                        entity,
                        &label,
                        view.camera_identities.get(&entity).map(String::as_str),
                        selected,
                        &mut header_select,
                        &mut header_focus,
                    )
                },
                |ui| {
                    for &child in children {
                        render_node(ui, child, depth + 1, view, selected, to_select, to_focus);
                    }
                },
            );
            if header_select.is_some() {
                *to_select = header_select;
            }
            if header_focus.is_some() {
                *to_focus = header_focus;
            }
        }
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
    to_select: &mut Option<(Entity, bool)>,
    to_focus: &mut Option<Entity>,
) -> bool {
    let hint = match full_identity {
        Some(identity) => format!(
            "{identity}  ·  click to select · Shift+Click to multiselect · double-click to focus"
        ),
        None => "Click to select · Shift+Click to multiselect · double-click to focus".to_owned(),
    };
    let width = ui.available_width();
    let resp = lunco_workbench_widgets::tree::selectable_label(
        ui,
        selected.entities.contains(&entity),
        label,
        width,
    )
    .on_hover_text(hint);

    let shift_held = ui.input(|i| i.modifiers.shift);

    if resp.clicked() {
        *to_select = Some((entity, shift_held));
    }
    if resp.double_clicked() {
        *to_select = Some((entity, shift_held));
        *to_focus = Some(entity);
    }
    resp.clicked()
}

fn entity_list_content(ui: &mut egui::Ui, ctx: &mut PanelCtx) {
    ui.label("Click to select. Expand > to reach sub-parts (wheels, body).");
    if let Some((scope, error)) = ctx
        .resource::<EntityTreeView>()
        .map(|view| (view.grid_scope, view.scope_error.clone()))
    {
        match error {
            Some(error) => ui.label(format!("Grid scope error: {error}")),
            None => ui.label(format!("Grid scope: {}", scope.label())),
        };
    }
    ui.separator();

    // Authoritative selection — read directly (small, cheap); never shadowed.
    let selected = ctx
        .resource::<lunco_scene_selection::SelectedEntities>()
        .cloned()
        .unwrap_or_default();

    let mut to_select: Option<(Entity, bool)> = None;
    let mut to_focus: Option<Entity> = None;

    // Borrow the precomputed view for the duration of painting only, then drop
    // it so `ctx` is free for the selection/focus mutations below.
    {
        let Some(view) = ctx.resource::<EntityTreeView>() else {
            return;
        };

        // ONE panel-level ScrollArea owning every row. `auto_shrink([false; 2])`
        // makes the area claim the panel's full height instead of shrinking to
        // content — a shrunk area never scrolls, which is why a long tree ran off
        // the bottom of the panel with no way to reach it.
        egui::ScrollArea::vertical()
            .id_salt("entity_list_scroll")
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                for &root in &view.roots {
                    render_node(ui, root, 0, view, &selected, &mut to_select, &mut to_focus);
                }
            });
        if selected
            .entities
            .iter()
            .any(|entity| !view.labels.contains_key(entity))
        {
            ui.label("A selected entity is outside the current tree scope.");
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
    fn topology_gate_rebuilds_when_a_named_selectable_arrives_after_initial_fill() {
        let mut app = App::new();
        app.init_resource::<EntityListSettings>()
            .init_resource::<EntityTreeView>()
            .init_resource::<GateRuns>()
            .add_systems(Update, count_gate_run.run_if(scene_topology_changed));

        app.update();
        assert_eq!(app.world().resource::<GateRuns>().0, 1);

        app.world_mut()
            .spawn((Name::new("Rover"), lunco_core::SelectableRoot));
        app.update();

        assert_eq!(app.world().resource::<GateRuns>().0, 2);
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
    fn missing_grid_scope_uses_the_documented_current_default() {
        assert_eq!(entity_grid_scope(None), Ok(EntityGridScope::Current));
    }

    #[test]
    fn grid_scope_accepts_only_the_canonical_values() {
        assert_eq!(
            parse_entity_grid_scope(&lunco_workspace::TwinSettingValue::Text("current".into())),
            Ok(EntityGridScope::Current)
        );
        assert_eq!(
            parse_entity_grid_scope(&lunco_workspace::TwinSettingValue::Text("all".into())),
            Ok(EntityGridScope::All)
        );
        assert!(parse_entity_grid_scope(&lunco_workspace::TwinSettingValue::Bool(true)).is_err());
        assert!(
            parse_entity_grid_scope(&lunco_workspace::TwinSettingValue::Text("other".into()))
                .is_err()
        );
    }

    #[test]
    fn nearest_grid_walks_through_unnamed_wrappers() {
        let grid = Entity::from_raw_u32(1).unwrap();
        let other_grid = Entity::from_raw_u32(4).unwrap();
        let wrapper = Entity::from_raw_u32(2).unwrap();
        let entity = Entity::from_raw_u32(3).unwrap();
        let other_entity = Entity::from_raw_u32(5).unwrap();
        let parents = HashMap::from([(wrapper, grid), (entity, wrapper)]);
        let other_parents = HashMap::from([(other_entity, other_grid)]);
        let grids = HashSet::from([grid, other_grid]);

        assert_eq!(nearest_grid(entity, &parents, &grids), Some(grid));
        assert!(in_scope(
            entity,
            GridScopeState {
                scope: EntityGridScope::Current,
                active_twin: None,
                current_grid: Some(grid),
            },
            &parents,
            &grids,
        ));
        assert!(!in_scope(
            other_entity,
            GridScopeState {
                scope: EntityGridScope::Current,
                active_twin: None,
                current_grid: Some(grid),
            },
            &other_parents,
            &grids,
        ));
        assert!(in_scope(
            other_entity,
            GridScopeState {
                scope: EntityGridScope::All,
                active_twin: None,
                current_grid: Some(grid),
            },
            &other_parents,
            &grids,
        ));
    }

    #[test]
    fn active_twin_close_clears_derived_scope_state() {
        let mut app = App::new();
        app.init_resource::<EntityTreeView>()
            .add_observer(on_twin_closed);
        {
            let mut view = app.world_mut().resource_mut::<EntityTreeView>();
            view.built = true;
            view.active_twin = Some(lunco_workspace::TwinId::new(7));
            view.scope_error = Some("stale".into());
        }

        app.world_mut().trigger(TwinClosed {
            twin: lunco_workspace::TwinId::new(7),
            root: std::path::PathBuf::from("/outgoing"),
            was_active: true,
        });

        let view = app.world().resource::<EntityTreeView>();
        assert!(!view.built);
        assert_eq!(view.active_twin, None);
        assert_eq!(view.scope_error, None);
    }
}
