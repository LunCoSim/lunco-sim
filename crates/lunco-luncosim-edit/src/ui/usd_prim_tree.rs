//! USD **prim tree** panel — the scene's authoring hierarchy.
//!
//! Where the Entity list (`entity_list.rs`) shows the ECS objects a user can
//! click, this shows the faithful **USD prim hierarchy** —
//! `/SandboxScene → Rover → Rocker → Bogie → Wheel_FL` — reconstructed from the
//! `UsdPrimPath` of every spawned prim (intermediate xforms that carry no entity
//! of their own are synthesized from the path so the structure is complete).
//! That is the structure you navigate when *building* an object: drill the
//! hierarchy, select a part, tune it in the Inspector.
//!
//! Clicking a node that maps to an entity selects it through the same
//! `apply_selection` path the viewport and Entity list use; intermediate nodes
//! are pure expanders.
//!
//! # Reactive shape (WP-8)
//!
//! [`produce_usd_prim_tree`] is the view-model producer: it runs on the main
//! thread (the stage is `!Send`), reads the composed stage for each prim's type
//! + body flag, and rebuilds the [`UsdPrimTreeView`] only when the set of prim
//! paths changes (hash-gated). The panel is pure paint over that resource.

use std::collections::{BTreeSet, HashMap};

use bevy::prelude::*;
use bevy_egui::egui;
use lunco_usd_bevy::{
    instance_key, CanonicalStages, SdfPath, UsdInstanceRoot, UsdPrimPath, UsdRead, UsdStageAsset,
};
use lunco_workbench::{Panel, PanelCtx, PanelId, PanelSlot};

pub const USD_PRIM_TREE_PANEL_ID: PanelId = PanelId("usd_prim_tree");

/// Tree-node identity: the prim's owning **instance** (its instance-root GID,
/// `None` for authored scene prims) plus its stage-relative path. Two runtime
/// spawns of one asset compose IDENTICAL paths, so the instance is what keeps
/// them from collapsing into a single node — each spawn is its own root subtree.
type NodeKey = (Option<u64>, String);

/// One node in the prim tree.
struct PrimTreeNode {
    /// Leaf name (last path segment).
    name: String,
    /// Composed `typeName`, empty for a synthesized intermediate.
    type_name: String,
    /// The ECS entity for this prim, if one was spawned (selectable).
    entity: Option<Entity>,
    /// Applies `PhysicsRigidBodyAPI`.
    is_body: bool,
    /// Child node keys, sorted by name.
    children: Vec<NodeKey>,
}

/// Render-ready USD prim hierarchy. Derived, never authoritative.
#[derive(Resource, Default)]
pub struct UsdPrimTreeView {
    nodes: HashMap<NodeKey, PrimTreeNode>,
    /// Top-level node keys, sorted by name.
    roots: Vec<NodeKey>,
    /// Hash of the last projected path set; a rebuild is skipped while it holds.
    hash: u64,
    built: bool,
}

/// View-model producer: rebuild [`UsdPrimTreeView`] from the composed stage when
/// the prim-path set changes.
pub fn produce_usd_prim_tree(
    q: Query<(Entity, &UsdPrimPath)>,
    q_provenance: Query<&lunco_core::Provenance>,
    q_gid: Query<&lunco_core::GlobalEntityId>,
    q_instance_root: Query<(), With<UsdInstanceRoot>>,
    stages: Res<Assets<UsdStageAsset>>,
    mut canonical: NonSendMut<CanonicalStages>,
    mut view: ResMut<UsdPrimTreeView>,
) {
    // Pick the scene stage = the stage id with the most prim entities.
    let mut counts: HashMap<AssetId<UsdStageAsset>, (usize, Handle<UsdStageAsset>)> =
        HashMap::new();
    for (_, p) in q.iter() {
        counts
            .entry(p.stage_handle.id())
            .or_insert((0, p.stage_handle.clone()))
            .0 += 1;
    }
    let Some((stage_id, handle)) = counts
        .into_iter()
        .max_by_key(|(_, (c, _))| *c)
        .map(|(id, (_, h))| (id, h))
    else {
        return;
    };

    // ((instance, path) → entity) for this stage; the set of keys drives the
    // change gate. Keying on the instance is what gives two spawns of one asset
    // two subtrees instead of one collapsed node (their paths are identical).
    let mut entity_of: HashMap<NodeKey, Entity> = HashMap::new();
    for (e, p) in q.iter() {
        if p.stage_handle.id() == stage_id {
            let inst = instance_key(e, &q_provenance, &q_gid, &q_instance_root);
            entity_of.insert((inst, p.path.clone()), e);
        }
    }

    // Every path + all of its ancestor prefixes (within the SAME instance), so
    // intermediate xforms appear under the right spawn.
    let mut all_paths: BTreeSet<NodeKey> = BTreeSet::new();
    for (inst, path) in entity_of.keys() {
        let mut acc = String::new();
        for seg in path.split('/').filter(|s| !s.is_empty()) {
            acc.push('/');
            acc.push_str(seg);
            all_paths.insert((*inst, acc.clone()));
        }
    }

    let hash = {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        for key in &all_paths {
            key.hash(&mut h);
            entity_of.contains_key(key).hash(&mut h);
        }
        h.finish()
    };
    if view.built && view.hash == hash {
        return;
    }

    // Ensure the canonical stage is built so we can read type/body per prim.
    if canonical.get(stage_id).is_none() {
        if let Some(recipe) = stages.get(&handle).and_then(|a| a.recipe.clone()) {
            canonical.get_or_build(stage_id, &recipe);
        }
    }
    let stage_view = canonical.get(stage_id).map(|cs| cs.view());

    let mut nodes: HashMap<NodeKey, PrimTreeNode> = HashMap::new();
    let mut roots: Vec<NodeKey> = Vec::new();

    for key in &all_paths {
        let (_, path) = key;
        let name = path.rsplit('/').next().unwrap_or(path).to_string();
        let (type_name, is_body) = match &stage_view {
            Some(v) => match SdfPath::new(path) {
                Ok(sdf) => (
                    v.type_name(&sdf).unwrap_or_default(),
                    v.has_api_schema(&sdf, "PhysicsRigidBodyAPI"),
                ),
                Err(_) => (String::new(), false),
            },
            None => (String::new(), false),
        };
        nodes.insert(
            key.clone(),
            PrimTreeNode {
                name,
                type_name,
                entity: entity_of.get(key).copied(),
                is_body,
                children: Vec::new(),
            },
        );
    }

    // Wire parent → children (and collect roots). The parent shares this node's
    // instance, so the prefix resolves within the same spawn.
    for key in &all_paths {
        let (inst, path) = key;
        match path.rsplit_once('/') {
            Some(("", _)) | None => roots.push(key.clone()),
            Some((parent, _)) => {
                let parent_key = (*inst, parent.to_string());
                if let Some(p) = nodes.get_mut(&parent_key) {
                    p.children.push(key.clone());
                } else {
                    // Parent prefix wasn't itself a prim (shouldn't happen — we
                    // inserted every prefix — but stay total).
                    roots.push(key.clone());
                }
            }
        }
    }

    // Sort children + roots by leaf name for a stable tree (leaf == node name,
    // so sorting by the path's last segment avoids a borrow of `nodes`).
    for node in nodes.values_mut() {
        node.children
            .sort_by_key(|(_, c)| c.rsplit('/').next().unwrap_or(c).to_string());
    }
    roots.sort_by_key(|(_, p)| p.rsplit('/').next().unwrap_or(p).to_string());

    view.nodes = nodes;
    view.roots = roots;
    view.hash = hash;
    view.built = true;
}

/// USD prim tree panel.
pub struct UsdPrimTreePanel;

impl Panel for UsdPrimTreePanel {
    fn id(&self) -> PanelId {
        USD_PRIM_TREE_PANEL_ID
    }
    fn title(&self) -> String {
        "🌲 Prims".into()
    }
    fn menu_group(&self) -> lunco_workbench::PanelMenuGroup {
        lunco_workbench::PanelMenuGroup::Scene
    }

    fn default_slot(&self) -> PanelSlot {
        PanelSlot::SideBrowser
    }
    fn transparent_background(&self) -> bool {
        true
    }

    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut PanelCtx) {
        let mantle = ctx.resource_expect::<lunco_theme::Theme>().colors.mantle;
        egui::Frame::new()
            .fill(mantle)
            .inner_margin(8.0)
            .corner_radius(4)
            .show(ui, |ui| prim_tree_content(ui, ctx));
    }
}

fn prim_tree_content(ui: &mut egui::Ui, ctx: &mut PanelCtx) {
    ui.label("The scene's USD structure. Expand ▸ to reach sub-parts; click a part to select it.");
    ui.separator();

    let selected = ctx
        .resource::<crate::SelectedEntities>()
        .cloned()
        .unwrap_or_default();

    let mut to_select: Option<Entity> = None;

    {
        let Some(view) = ctx.resource::<UsdPrimTreeView>() else {
            return;
        };
        if !view.built || view.roots.is_empty() {
            ui.label(egui::RichText::new("No USD scene loaded.").weak());
            return;
        }
        egui::ScrollArea::vertical().show(ui, |ui| {
            for root in &view.roots {
                render_prim_node(ui, root, view, &selected, &mut to_select, 0);
            }
        });
    }

    // Route selection through the shared `apply_selection` (keyed by Entity).
    if let Some(entity) = to_select {
        ctx.defer(move |world| {
            let old: Vec<Entity> = world
                .query_filtered::<Entity, With<crate::selection::Selected>>()
                .iter(world)
                .collect();
            world.resource_scope(|world, mut selected: Mut<crate::SelectedEntities>| {
                let mut commands = world.commands();
                crate::selection::apply_selection(
                    &mut commands,
                    &mut selected,
                    old,
                    entity,
                    false,
                    false,
                );
            });
            world.flush();
        });
    }
}

/// Render one prim node + its descendants. A node that maps to an entity is a
/// selectable label; a node with children gets an expander whose header is the
/// (possibly selectable) label; a childless intermediate is a dim, inert label.
fn render_prim_node(
    ui: &mut egui::Ui,
    key: &NodeKey,
    view: &UsdPrimTreeView,
    selected: &crate::SelectedEntities,
    to_select: &mut Option<Entity>,
    depth: usize,
) {
    let Some(node) = view.nodes.get(key) else {
        return;
    };
    let label = prim_label(node);

    if node.children.is_empty() {
        prim_select_label(ui, node, &label, selected, to_select);
        return;
    }
    // Top two levels open by default so the scene structure is visible without
    // drilling; deeper subtrees (a rover's per-wheel joints) start collapsed.
    let default_open = depth < 2;
    // Key includes the instance, so two spawns of one asset get DISTINCT egui
    // ids (identical paths would otherwise share collapse state).
    let id = ui.make_persistent_id(("usd_prim_tree", key));
    egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, default_open)
        .show_header(ui, |ui| {
            prim_select_label(ui, node, &label, selected, to_select);
        })
        .body(|ui| {
            for child in &node.children {
                render_prim_node(ui, child, view, selected, to_select, depth + 1);
            }
        });
}

/// The row for one prim: selectable when it has an entity, otherwise a dim inert
/// label (an intermediate xform the user can expand but not select).
fn prim_select_label(
    ui: &mut egui::Ui,
    node: &PrimTreeNode,
    label: &str,
    selected: &crate::SelectedEntities,
    to_select: &mut Option<Entity>,
) {
    match node.entity {
        Some(entity) => {
            let resp = ui
                .selectable_label(selected.entities.contains(&entity), label)
                .on_hover_text(if node.type_name.is_empty() {
                    "Click to select".to_string()
                } else {
                    format!("{}  ·  click to select", node.type_name)
                });
            if resp.clicked() {
                *to_select = Some(entity);
            }
        }
        None => {
            ui.add(egui::Label::new(egui::RichText::new(label).weak()));
        }
    }
}

/// `<glyph> <name>` — a wrench for a rigid body, else a folder/dot.
fn prim_label(node: &PrimTreeNode) -> String {
    let glyph = if node.is_body {
        "🔩"
    } else if node.children.is_empty() {
        "·"
    } else {
        "▪"
    };
    format!("{glyph} {}", node.name)
}
