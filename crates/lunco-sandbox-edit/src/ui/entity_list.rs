//! Entity list panel — WorkbenchPanel implementation.
//!
//! A hierarchy tree of scene objects: top-level objects (rovers, props,
//! terrain, cosim blocks) with their sub-parts (wheels, body) nested beneath,
//! so you can drill in and select a single wheel. Internal plumbing (cosim
//! wires, ports, empty transform wrappers) is hidden — only entities that are
//! selectable or mesh-bearing, plus their ancestors, appear. Clicking a node
//! selects it.

use bevy::prelude::*;
use bevy_egui::egui;
use lunco_workbench::{Panel, PanelId, PanelSlot};
use std::collections::{HashMap, HashSet};

// Removed SelectedEntity import

/// Entity list panel — hierarchy tree of scene entities.
pub struct EntityList;

impl Panel for EntityList {
    fn id(&self) -> PanelId { PanelId("entity_list") }
    fn title(&self) -> String { "Entities".into() }
    fn default_slot(&self) -> PanelSlot { PanelSlot::SideBrowser }
    fn transparent_background(&self) -> bool { true }

    fn render(&mut self, ui: &mut egui::Ui, world: &mut World) {
        let (mantle, tokens) = {
            let theme = world.resource::<lunco_theme::Theme>();
            (theme.colors.mantle, theme.tokens.clone())
        };
        egui::Frame::new()
            .fill(mantle)
            .inner_margin(8.0)
            .corner_radius(4)
            .show(ui, |ui| entity_list_content(self, ui, world, &tokens));
    }
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

/// Render one tree node and its visible descendants. Leaf nodes are a
/// selectable label; branch nodes get an expander (`CollapsingState`) whose
/// header is itself selectable, so a click on the rover selects the rover and
/// the triangle drills into its wheels.
#[allow(clippy::too_many_arguments)]
fn render_node(
    ui: &mut egui::Ui,
    entity: Entity,
    kids: &HashMap<Entity, Vec<Entity>>,
    names: &HashMap<Entity, String>,
    shown: &HashMap<Entity, bool>,
    selected: &crate::SelectedEntities,
    to_select: &mut Option<(Entity, bool)>,
    to_focus: &mut Option<Entity>,
) {
    let label = names
        .get(&entity)
        .map(|s| leaf(s))
        .unwrap_or_else(|| format!("{entity:?}"));
    let visible_kids: Vec<Entity> = kids
        .get(&entity)
        .map(|v| v.iter().copied().filter(|c| *shown.get(c).unwrap_or(&false)).collect())
        .unwrap_or_default();

    if visible_kids.is_empty() {
        select_label(ui, entity, &label, selected, to_select, to_focus);
        return;
    }

    let id = ui.make_persistent_id(("entity_tree", entity));
    egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, false)
        .show_header(ui, |ui| {
            select_label(ui, entity, &label, selected, to_select, to_focus);
        })
        .body(|ui| {
            for child in visible_kids {
                render_node(ui, child, kids, names, shown, selected, to_select, to_focus);
            }
        });
}

/// A selectable entity label: single click selects, double click also flags it
/// for camera focus. Shared by the tree nodes and the flat shader group so the
/// click/double-click behaviour stays identical everywhere.
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

fn entity_list_content(_panel: &mut EntityList, ui: &mut egui::Ui, world: &mut World, tokens: &lunco_theme::DesignTokens) {
    let _ = tokens;
    ui.label("Click to select. Expand ▸ to reach sub-parts (wheels, body).");
    ui.separator();

    // ── Gather the scene graph (read-only; no world borrow held while drawing).
    let named: Vec<(Entity, String)> = world
        .query::<(Entity, &Name)>()
        .iter(world)
        .map(|(e, n)| (e, n.as_str().to_string()))
        .collect();
    let names: HashMap<Entity, String> = named.iter().cloned().collect();
    let named_set: HashSet<Entity> = named.iter().map(|(e, _)| *e).collect();

    // Parent of each entity (full graph, not just named) so unnamed grid/wrapper
    // entities can be skipped over when finding an entity's display parent.
    let child_of: HashMap<Entity, Entity> = world
        .query::<(Entity, &ChildOf)>()
        .iter(world)
        .map(|(e, c)| (e, c.parent()))
        .collect();

    // "Interesting" = something a user would edit: a selectable object or any
    // mesh-bearing part. Everything else (cosim wires, ports, empty transform
    // wrappers) is plumbing — hidden unless it's an ancestor of an interesting
    // entity. (Cosim model blocks ARE selectable, so they stay.)
    let selectable: HashSet<Entity> = world
        .query_filtered::<Entity, With<lunco_core::SelectableRoot>>()
        .iter(world)
        .collect();
    let has_mesh: HashSet<Entity> = world
        .query_filtered::<Entity, With<Mesh3d>>()
        .iter(world)
        .collect();

    // Shader-editable subset (terrain + props with a custom ShaderMaterial),
    // pinned at the top for quick access to the shader params.
    let mut shader_sorted: Vec<(Entity, String)> = world
        .query_filtered::<(Entity, &Name), With<MeshMaterial3d<lunco_materials::ShaderMaterial>>>()
        .iter(world)
        .map(|(e, n)| (e, n.as_str().to_string()))
        .collect();
    shader_sorted.sort_by(|a, b| a.1.cmp(&b.1));

    let selected_resource = world.get_resource::<crate::SelectedEntities>()
        .cloned()
        .unwrap_or_default();

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

    // Stable alphabetical order by leaf label, at every level.
    let by_leaf = |a: &Entity, b: &Entity| {
        let la = names.get(a).map(|s| leaf(s)).unwrap_or_default();
        let lb = names.get(b).map(|s| leaf(s)).unwrap_or_default();
        la.cmp(&lb)
    };
    for v in kids.values_mut() {
        v.sort_by(by_leaf);
    }
    roots.retain(|e| *shown.get(e).unwrap_or(&false));
    roots.sort_by(by_leaf);

    let mut to_select: Option<(Entity, bool)> = None;
    let mut to_focus: Option<Entity> = None;

    // Pinned shader-materials group.
    if !shader_sorted.is_empty() {
        egui::CollapsingHeader::new("🎨 Shader materials")
            .default_open(true)
            .show(ui, |ui| {
                ui.label(egui::RichText::new("Edit params in the Inspector").weak());
                for (e, name) in &shader_sorted {
                    select_label(ui, *e, &leaf(name), &selected_resource, &mut to_select, &mut to_focus);
                }
            });
        ui.separator();
    }

    // The hierarchy.
    egui::ScrollArea::vertical().show(ui, |ui| {
        for root in &roots {
            render_node(ui, *root, &kids, &names, &shown, &selected_resource, &mut to_select, &mut to_focus);
        }
    });

    // Route selection through the single mutation path (the `SelectEntity`
    // command the viewport click + API also use), so it clears the previous
    // `Selected`/`GizmoTarget` and updates `SelectedEntity` before the Inspector
    // renders later this same egui pass. Sub-parts (wheels) have no API id, so
    // they fall back to a direct `SelectedEntity` write — enough for the
    // Inspector to retarget, without the highlight/gizmo bookkeeping.
    if let Some((entity, shift_held)) = to_select {
        let is_selected = world.get_resource::<crate::SelectedEntities>().unwrap().entities.contains(&entity);

        if !shift_held {
            // Clear other selections
            let old: Vec<Entity> = world
                .query_filtered::<Entity, With<crate::selection::Selected>>()
                .iter(world)
                .collect();
            for o in old {
                if o != entity {
                    world
                        .entity_mut(o)
                        .remove::<crate::selection::Selected>()
                        .remove::<transform_gizmo_bevy::GizmoTarget>();
                }
            }
            world.get_resource_mut::<crate::SelectedEntities>().unwrap().entities.clear();
        }

        if shift_held && is_selected {
            world.entity_mut(entity).remove::<crate::selection::Selected>().remove::<transform_gizmo_bevy::GizmoTarget>();
            world.get_resource_mut::<crate::SelectedEntities>().unwrap().entities.retain(|e| *e != entity);
        } else {
            world.entity_mut(entity).insert((crate::selection::Selected, transform_gizmo_bevy::GizmoTarget::default()));
            let mut selected = world.get_resource_mut::<crate::SelectedEntities>().unwrap();
            if !selected.entities.contains(&entity) {
                selected.entities.push(entity);
            }
        }
        
        let is_empty = world.get_resource::<crate::SelectedEntities>().unwrap().entities.is_empty();
        world.get_resource_mut::<lunco_core::DragModeActive>().unwrap().active = !is_empty;
    }

    // Double-click flies the camera to the entity via the same `FocusEntityById`
    // command the API exposes. Works for anything with an API id — no collider
    // required (this is list-driven, not a viewport raycast).
    if let Some(entity) = to_focus {
        if let Some(id) = world
            .get_resource::<lunco_api::registry::ApiEntityRegistry>()
            .and_then(|r| r.api_id_for(entity))
            .map(|g| g.get())
        {
            world.trigger(crate::commands::FocusEntityById { entity_id: id, distance: 0.0 });
        }
    }
}
