//! Entity list panel — WorkbenchPanel implementation.
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
//! `render` reads that resource and the authoritative [`crate::SelectedEntities`]
//! directly, and routes clicks through the same `apply_selection` path as
//! before. Nothing is scanned, walked, or sorted while painting.

use bevy::prelude::*;
use bevy_egui::egui;
use lunco_settings::SettingsSection;
use lunco_workbench::{Panel, PanelCtx, PanelId, PanelSlot};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// Persisted view prefs for the Entity list.
#[derive(Resource, Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
pub struct EntityListSettings {
    /// Show entities a system owns and churns ([`lunco_core::SystemManaged`]:
    /// streamed LOD tiles, globe tiles, scattered rocks). Off by default — with
    /// terrain streaming there are hundreds live and they bury the handful of
    /// authored objects the list exists to show.
    pub show_system: bool,
}

impl Default for EntityListSettings {
    fn default() -> Self {
        Self { show_system: false }
    }
}

impl SettingsSection for EntityListSettings {
    const KEY: &'static str = "entity_list";
}

/// Push the Entity-list filter into the workbench **Settings** menu — where every
/// other persisted view pref lives (theme, perf HUD, terrain). The panel stays a
/// pure view; it grows no toolbar of its own.
pub(crate) fn register_settings_menu(world: &mut World) {
    let Some(mut layout) = world.get_resource_mut::<lunco_workbench::WorkbenchLayout>() else {
        return;
    };
    layout.register_settings(|ui, world| {
        ui.label(egui::RichText::new("Entity list").weak().small());
        let mut settings = world.resource_mut::<EntityListSettings>();
        ui.checkbox(&mut settings.show_system, "Show system entities")
            .on_hover_text(
                "Streamed terrain LOD tiles, globe tiles and scattered rocks — spawned \
                 and despawned continuously as the camera moves. Hidden by default so \
                 the list shows authored scene objects only.",
            );
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
    /// Leaf display label per named entity.
    pub labels: HashMap<Entity, String>,
    /// Set once the first build runs, so the change-gate forces an initial fill.
    built: bool,
}

/// Last path segment of a USD prim name (`/SandboxScene/Rover/Wheel_FL` →
/// `Wheel_FL`); plain names (`Dynamic Ball`) pass through unchanged.
fn leaf(full: &str) -> String {
    full.rsplit(['/', '\\']).next().unwrap_or(full).to_string()
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
    mut view: ResMut<EntityTreeView>,
    settings: Res<EntityListSettings>,
    named_q: Query<(Entity, &Name)>,
    system_q: Query<Entity, With<lunco_core::SystemManaged>>,
    child_q: Query<(Entity, &ChildOf)>,
    selectable_q: Query<Entity, With<lunco_core::SelectableRoot>>,
    mesh_q: Query<Entity, With<Mesh3d>>,
) {
    // ── Harvest (read-only).
    // System-owned churn (streamed LOD tiles, globe tiles, scatter) is dropped
    // right here unless the user opted in, so nothing downstream — parenting,
    // visibility, sort — even sees it. Their children (none today) would simply
    // re-parent to the nearest surviving named ancestor.
    let system: HashSet<Entity> = if settings.show_system {
        HashSet::new()
    } else {
        system_q.iter().collect()
    };
    let named: Vec<(Entity, String)> = named_q
        .iter()
        .filter(|(e, _)| !system.contains(e))
        .map(|(e, n)| (e, n.as_str().to_string()))
        .collect();
    let named_set: HashSet<Entity> = named.iter().map(|(e, _)| *e).collect();
    let labels: HashMap<Entity, String> =
        named.iter().map(|(e, full)| (*e, leaf(full))).collect();

    // Parent of each entity (full graph, not just named) so unnamed grid/wrapper
    // entities can be skipped over when finding an entity's display parent.
    let child_of: HashMap<Entity, Entity> =
        child_q.iter().map(|(e, c)| (e, c.parent())).collect();

    // "Interesting" = something a user would edit: a selectable object or any
    // mesh-bearing part. Everything else (cosim wires, ports, empty transform
    // wrappers) is plumbing — hidden unless it's an ancestor of an interesting
    // entity. (Cosim model blocks ARE selectable, so they stay.)
    let selectable: HashSet<Entity> = selectable_q.iter().collect();
    let has_mesh: HashSet<Entity> = mesh_q.iter().collect();

    // NOTE: there is deliberately no separate "shader materials" group any more.
    // Every `ShaderLook` entity is already a mesh in the tree below, so the pinned
    // group was the same objects listed twice — and since every streamed terrain
    // tile carries a `ShaderLook`, it was mostly LOD churn. Select the object in
    // the tree; its shader params are in the Inspector as before.
    //
    // Build the display tree: each named entity's parent is its nearest named
    // ancestor (unnamed wrappers collapse away), giving rover→wheel nesting
    // instead of a flat alphabetical dump.
    let display_parent = |e: Entity| -> Option<Entity> {
        let mut cur = e;
        for _ in 0..64 {
            let p = *child_of.get(&cur)?;
            if named_set.contains(&p) {
                return Some(p);
            }
            cur = p;
        }
        None
    };
    let mut kids: HashMap<Entity, Vec<Entity>> = HashMap::new();
    let mut roots: Vec<Entity> = Vec::new();
    for (e, _) in &named {
        match display_parent(*e) {
            Some(p) => kids.entry(p).or_default().push(*e),
            None => roots.push(*e),
        }
    }

    // Visibility: an entity shows if it or any descendant is interesting.
    let interesting = |e: Entity| selectable.contains(&e) || has_mesh.contains(&e);
    let mut shown: HashMap<Entity, bool> = HashMap::new();
    for (e, _) in &named {
        compute_shown(*e, &kids, &interesting, &mut shown);
    }

    // Prune children to shown-only + stable alphabetical order by leaf label, at
    // every level; drop empty entries so the panel treats them as leaves.
    let by_leaf = |a: &Entity, b: &Entity| {
        let la = labels.get(a).map(String::as_str).unwrap_or("");
        let lb = labels.get(b).map(String::as_str).unwrap_or("");
        la.cmp(lb)
    };
    let mut pruned: HashMap<Entity, Vec<Entity>> = HashMap::new();
    for (parent, cs) in &kids {
        let mut v: Vec<Entity> =
            cs.iter().copied().filter(|c| *shown.get(c).unwrap_or(&false)).collect();
        if v.is_empty() {
            continue;
        }
        v.sort_by(by_leaf);
        pruned.insert(*parent, v);
    }
    roots.retain(|e| *shown.get(e).unwrap_or(&false));
    roots.sort_by(by_leaf);

    view.roots = roots;
    view.kids = pruned;
    view.labels = labels;
    view.built = true;
}

/// Run condition for [`populate_entity_tree_view`]: rebuild only when the scene
/// topology that the tree depends on changes — names/hierarchy added or
/// modified (`Changed` includes `Added`), the interesting marker sets
/// gain members, or any of those components are removed (covers despawns). The
/// `Local` flag forces one initial build (a freshly-added system does not see
/// pre-existing entities as `Changed`). On a quiescent scene this returns
/// `false` and the harvest is skipped entirely.
/// System-owned entities are excluded from the gate exactly as they are from the
/// harvest (unless shown): terrain streaming spawns and despawns tiles every
/// frame, and counting those as "topology changed" would rebuild the whole tree
/// every frame to produce an identical result.
pub(crate) fn scene_topology_changed(
    mut first: Local<bool>,
    settings: Res<EntityListSettings>,
    view: Res<EntityTreeView>,
    changed: Query<(), (Or<(Changed<Name>, Changed<ChildOf>)>, Without<lunco_core::SystemManaged>)>,
    changed_system: Query<(), (Or<(Changed<Name>, Changed<ChildOf>)>, With<lunco_core::SystemManaged>)>,
    added: Query<
        (),
        (
            Or<(Added<Mesh3d>, Added<lunco_core::SelectableRoot>)>,
            Without<lunco_core::SystemManaged>,
        ),
    >,
    added_system: Query<
        (),
        (
            Or<(Added<Mesh3d>, Added<lunco_core::SelectableRoot>)>,
            With<lunco_core::SystemManaged>,
        ),
    >,
    mut rm_name: RemovedComponents<Name>,
    mut rm_child: RemovedComponents<ChildOf>,
    mut rm_mesh: RemovedComponents<Mesh3d>,
    mut rm_sel: RemovedComponents<lunco_core::SelectableRoot>,
) -> bool {
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
        | drained(&mut rm_sel.read());
    let system_churn =
        settings.show_system && (!changed_system.is_empty() || !added_system.is_empty());
    let run = !*first
        || settings.is_changed()
        || !changed.is_empty()
        || !added.is_empty()
        || system_churn
        || removed;
    *first = true;
    run
}

/// Entity list panel — hierarchy tree of scene entities.
pub struct EntityList;

impl Panel for EntityList {
    fn id(&self) -> PanelId { PanelId("entity_list") }
    fn title(&self) -> String { "Entities".into() }
    fn default_slot(&self) -> PanelSlot { PanelSlot::SideBrowser }
    fn menu_group(&self) -> lunco_workbench::PanelMenuGroup {
        lunco_workbench::PanelMenuGroup::Scene
    }
    fn transparent_background(&self) -> bool { true }

    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut PanelCtx) {
        let mantle = ctx.resource_expect::<lunco_theme::Theme>().colors.mantle;
        egui::Frame::new()
            .fill(mantle)
            .inner_margin(8.0)
            .corner_radius(4)
            .show(ui, |ui| entity_list_content(ui, ctx));
    }
}

/// Render one tree node and its descendants. Children in the view are already
/// visibility-pruned and sorted, so this is pure paint — leaf nodes are a
/// selectable label; branch nodes get an expander (`CollapsingState`) whose
/// header is itself selectable, so a click on the rover selects the rover and
/// the triangle drills into its wheels.
fn render_node(
    ui: &mut egui::Ui,
    entity: Entity,
    view: &EntityTreeView,
    selected: &crate::SelectedEntities,
    to_select: &mut Option<(Entity, bool)>,
    to_focus: &mut Option<Entity>,
) {
    let label = view
        .labels
        .get(&entity)
        .cloned()
        .unwrap_or_else(|| format!("{entity:?}"));

    match view.kids.get(&entity) {
        None => select_label(ui, entity, &label, selected, to_select, to_focus),
        Some(children) => {
            let id = ui.make_persistent_id(("entity_tree", entity));
            egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, false)
                .show_header(ui, |ui| {
                    select_label(ui, entity, &label, selected, to_select, to_focus);
                })
                .body(|ui| {
                    for &child in children {
                        render_node(ui, child, view, selected, to_select, to_focus);
                    }
                });
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
    selected: &crate::SelectedEntities,
    to_select: &mut Option<(Entity, bool)>,
    to_focus: &mut Option<Entity>,
) {
    let resp = ui
        .selectable_label(selected.entities.contains(&entity), label)
        .on_hover_text("Click to select · Shift+Click to multiselect · double-click to focus");

    let shift_held = ui.input(|i| i.modifiers.shift);

    if resp.clicked() {
        *to_select = Some((entity, shift_held));
    }
    if resp.double_clicked() {
        *to_select = Some((entity, shift_held));
        *to_focus = Some(entity);
    }
}

fn entity_list_content(ui: &mut egui::Ui, ctx: &mut PanelCtx) {
    ui.label("Click to select. Expand ▸ to reach sub-parts (wheels, body).");
    ui.separator();

    // Authoritative selection — read directly (small, cheap); never shadowed.
    let selected = ctx
        .resource::<crate::SelectedEntities>()
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
                    render_node(ui, root, view, &selected, &mut to_select, &mut to_focus);
                }
            });
    }

    // Route selection through the same `crate::selection::apply_selection` the
    // viewport-click and `SelectEntity` API use — keyed by `Entity` (sub-parts
    // share api_ids, so id round-trips select the wrong instance). Shift = extend
    // + toggle (multi-select), plain click = replace. The Inspector reads the
    // updated `SelectedEntities` later in this same egui pass.
    if let Some((entity, shift_held)) = to_select {
        ctx.defer(move |world| {
            let old: Vec<Entity> = world
                .query_filtered::<Entity, With<crate::selection::Selected>>()
                .iter(world)
                .collect();
            world.resource_scope(|world, mut selected: Mut<crate::SelectedEntities>| {
                let mut commands = world.commands();
                crate::selection::apply_selection(
                    &mut commands, &mut selected, old, entity, shift_held, shift_held,
                );
            });
            world.flush();
        });
    }

    // Double-click flies the camera to the entity via the same `FocusEntityById`
    // command the API exposes. Works for anything with an API id — no collider
    // required (this is list-driven, not a viewport raycast).
    if let Some(entity) = to_focus {
        ctx.defer(move |world| {
            if let Some(id) = world
                .get_resource::<lunco_api::registry::ApiEntityRegistry>()
                .and_then(|r| r.api_id_for(entity))
                .map(|g| g.get())
            {
                world.trigger(crate::commands::FocusEntityById { entity_id: id, distance: 0.0 });
            }
        });
    }
}
