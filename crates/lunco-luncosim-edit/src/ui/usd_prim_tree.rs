//! USD **prim tree** panel — the scene's authoring hierarchy.
//!
//! Where the Entity list (`entity_list.rs`) shows the live simulation ECS, this
//! shows the faithful **USD prim hierarchy** of each open Editor preview
//! — `/Assembly → Chassis → Wheel_FL`. It is reconstructed from each
//! document's isolated preview projection; intermediate xforms that carry no
//! entity of their own are synthesized from the path so the structure is complete.
//! That is the structure you navigate when editing an assembly: drill the
//! hierarchy, select a part, tune it in the Inspector.
//!
//! Clicking a node that maps to a preview entity selects it through the same
//! typed selection path used by the viewport; intermediate nodes are pure
//! expanders. The live simulation projection is never selected by this tree.
//!
//! # Reactive shape (WP-8)
//!
//! [`produce_usd_prim_tree`] is the view-model producer: it runs on the main
//! thread (the stage is `!Send`), reads the composed stage for each prim's type
//! and body flag, and rebuilds the [`UsdPrimTreeView`] only when the set of prim
//! paths changes (hash-gated). The panel is pure paint over that resource.

use std::collections::{BTreeSet, HashMap};

use bevy::prelude::*;
use bevy_egui::egui;
use lunco_render::SceneCamera;
use lunco_usd::ui::viewport::{UsdPreviewId, UsdViewportState};
use lunco_usd_bevy::{
    camera_switch::camera_display_labels, CanonicalStages, SdfPath, UsdPrimPath, UsdRead,
    UsdStageAsset,
};
use lunco_workbench_core::{Panel, PanelCtx, PanelId, PanelSlot};

pub const USD_PRIM_TREE_PANEL_ID: PanelId = PanelId("usd_prim_tree");

/// Tree-node identity: the composed USD prim path in one preview lease.
/// Document scope comes from [`UsdViewportState`], so two open files can
/// contain the same paths without colliding.
type NodeKey = String;

/// One node in the prim tree.
struct PrimTreeNode {
    /// Leaf name or readable camera label when this node is a scene camera.
    display_name: String,
    /// Full camera identity retained for the row tooltip, if this is a camera.
    camera_identity: Option<String>,
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
#[derive(Default)]
pub struct UsdPrimTreeSessionView {
    nodes: HashMap<NodeKey, PrimTreeNode>,
    /// Top-level node keys, sorted by name.
    roots: Vec<NodeKey>,
    /// Hash of the last projected path set; a rebuild is skipped while it holds.
    hash: u64,
    built: bool,
}

/// Session-keyed USD hierarchy views. Every open preview retains its own
/// composed tree; the panel only paints the focused lease.
#[derive(Resource, Default)]
pub struct UsdPrimTreeView {
    sessions: HashMap<UsdPreviewId, UsdPrimTreeSessionView>,
}

impl UsdPrimTreeView {
    pub(crate) fn focused(&self, viewport: &UsdViewportState) -> Option<&UsdPrimTreeSessionView> {
        viewport
            .focused_preview_id()
            .and_then(|preview| self.sessions.get(&preview))
    }
}

/// Wake the Editor tree only when its explicit document projection or the
/// composed USD stage changes. There is no meaningful tree when the preview
/// has no selected document.
pub fn editor_prim_tree_changed(
    viewport: Option<Res<UsdViewportState>>,
    revision: Res<lunco_usd_bevy::UsdStageRevision>,
) -> bool {
    viewport.is_some_and(|state| state.is_changed()) || revision.is_changed()
}

/// View-model producer: rebuild [`UsdPrimTreeView`] from the composed stage when
/// the prim-path set changes.
pub fn produce_usd_prim_tree(
    q: Query<(Entity, &UsdPrimPath, Has<SceneCamera>)>,
    q_parents: Query<&ChildOf>,
    q_callsign: Query<&lunco_core::markers::Callsign>,
    q_catalog_id: Query<&lunco_core::CatalogEntryId>,
    stages: Res<Assets<UsdStageAsset>>,
    mut canonical: NonSendMut<CanonicalStages>,
    mut view: ResMut<UsdPrimTreeView>,
    viewport: Option<Res<UsdViewportState>>,
) {
    let Some(viewport) = viewport else {
        view.sessions.clear();
        return;
    };
    let open: std::collections::HashSet<_> = viewport.sessions().map(|s| s.id()).collect();
    view.sessions.retain(|preview, _| open.contains(preview));

    for session in viewport.sessions() {
        let session_view = view.sessions.entry(session.id()).or_default();
        let preview_root = session.scene_root();
        let handle = session.stage_handle().clone();
        let stage_id = handle.id();

        // The set of explicit document paths drives the change gate. The
        // preview root is the document scope; the live simulation is absent.
        let mut entity_of: HashMap<NodeKey, Entity> = HashMap::new();
        for (e, p, _) in q.iter() {
            if p.stage_handle.id() == stage_id
                && crate::ui::is_editor_preview_entity(e, preview_root, &q_parents)
            {
                entity_of.insert(p.path.clone(), e);
            }
        }

        let camera_identities: Vec<(Entity, String)> = q
            .iter()
            .filter(|(entity, path, is_camera)| {
                *is_camera
                    && path.stage_handle.id() == stage_id
                    && crate::ui::is_editor_preview_entity(*entity, preview_root, &q_parents)
            })
            .map(|(entity, path, _)| (entity, path.path.clone()))
            .collect();
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

        // Every path plus all ancestor prefixes, so intermediate xforms appear
        // under the correct authored hierarchy.
        let mut all_paths: BTreeSet<NodeKey> = BTreeSet::new();
        for path in entity_of.keys() {
            let mut acc = String::new();
            for seg in path.split('/').filter(|s| !s.is_empty()) {
                acc.push('/');
                acc.push_str(seg);
                all_paths.insert(acc.clone());
            }
        }

        let hash = {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            for key in &all_paths {
                key.hash(&mut h);
                if let Some(entity) = entity_of.get(key) {
                    true.hash(&mut h);
                    q_callsign
                        .get(*entity)
                        .ok()
                        .map(|value| value.0.as_str())
                        .hash(&mut h);
                    q_catalog_id
                        .get(*entity)
                        .ok()
                        .map(|value| value.0.as_str())
                        .hash(&mut h);
                } else {
                    false.hash(&mut h);
                }
            }
            h.finish()
        };
        if session_view.built && session_view.hash == hash {
            continue;
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
            let path = key;
            let name = path.rsplit('/').next().unwrap_or(path);
            let default_display_name = entity_of
                .get(key)
                .map(|entity| {
                    let path_name = Name::new(path.clone());
                    lunco_core::entity_display_name(
                        Some(&path_name),
                        q_callsign.get(*entity).ok(),
                        q_catalog_id.get(*entity).ok(),
                    )
                })
                .filter(|label| !label.is_empty())
                .unwrap_or_else(|| lunco_core::humanize_identifier(name));
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
            let display_name = entity_of
                .get(key)
                .and_then(|entity| camera_labels.get(entity))
                .cloned()
                .unwrap_or(default_display_name);
            nodes.insert(
                key.clone(),
                PrimTreeNode {
                    display_name,
                    camera_identity: entity_of
                        .get(key)
                        .and_then(|entity| camera_identity_by_entity.get(entity))
                        .cloned(),
                    type_name,
                    entity: entity_of.get(key).copied(),
                    is_body,
                    children: Vec::new(),
                },
            );
        }

        // Wire parent → children and collect roots from the explicit USD paths.
        for key in &all_paths {
            match key.rsplit_once('/') {
                Some(("", _)) | None => roots.push(key.clone()),
                Some((parent, _)) => {
                    let parent_key = parent.to_string();
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
                .sort_by_key(|c| c.rsplit('/').next().unwrap_or(c).to_string());
        }
        roots.sort_by_key(|p| p.rsplit('/').next().unwrap_or(p).to_string());

        session_view.nodes = nodes;
        session_view.roots = roots;
        session_view.hash = hash;
        session_view.built = true;
    }
}

/// USD prim tree panel.
pub struct UsdPrimTreePanel;

impl Panel for UsdPrimTreePanel {
    fn id(&self) -> PanelId {
        USD_PRIM_TREE_PANEL_ID
    }
    fn title(&self) -> String {
        "Prims".into()
    }
    fn menu_group(&self) -> lunco_workbench_core::PanelMenuGroup {
        lunco_workbench_core::PanelMenuGroup::Scene
    }

    fn default_slot(&self) -> PanelSlot {
        PanelSlot::SideBrowser
    }
    fn transparent_background(&self) -> bool {
        true
    }

    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut PanelCtx) {
        ctx.panel_content_frame()
            .show(ui, |ui| prim_tree_content(ui, ctx));
    }
}

fn prim_tree_content(ui: &mut egui::Ui, ctx: &mut PanelCtx) {
    let selected = ctx
        .resource::<lunco_scene_commands::SelectedEntities>()
        .cloned()
        .unwrap_or_default();

    let mut to_select: Option<Entity> = None;
    let primary = selected.primary();

    {
        let Some(viewport) = ctx.resource::<UsdViewportState>() else {
            return;
        };
        let Some(focused_preview) = viewport.focused_preview_id() else {
            return;
        };
        let Some(view) = ctx
            .resource::<UsdPrimTreeView>()
            .and_then(|views| views.focused(viewport))
        else {
            return;
        };
        if !view.built || view.roots.is_empty() {
            ui.label(egui::RichText::new("No USD scene loaded.").weak());
            return;
        }
        let scroll_id = ui.make_persistent_id(("usd_prim_tree_scroll", focused_preview.0));
        let selection_id = ui.make_persistent_id(("usd_prim_tree_selection", focused_preview.0));
        let selection_changed = ui
            .ctx()
            .data(|data| data.get_temp::<Vec<Entity>>(selection_id))
            .as_ref()
            != Some(&selected.entities);
        let reveal_path =
            primary.and_then(|entity| ctx.get::<UsdPrimPath>(entity).map(|path| path.path.clone()));

        // The dock deliberately disables its generic tab scroll wrapper, so
        // this panel owns one full-width scroll area. On a new selection, open
        // the selected path's ancestors and ask the selected row to reveal
        // itself; stable selections do not fight the user's manual scrolling.
        egui::ScrollArea::vertical()
            .id_salt(scroll_id)
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                for root in &view.roots {
                    render_prim_node(
                        ui,
                        root,
                        view,
                        &selected,
                        primary,
                        reveal_path.as_deref(),
                        selection_changed,
                        &mut to_select,
                        0,
                    );
                }
            });
        ui.ctx()
            .data_mut(|data| data.insert_temp(selection_id, selected.entities.clone()));
    }

    // Route selection through the shared `apply_selection` (keyed by Entity).
    if let Some(entity) = to_select {
        ctx.trigger(crate::selection::SelectEntityTarget {
            target: entity,
            intent: crate::selection::SelectionIntent::Replace,
        });
    }
}

/// Render one prim node + its descendants. A node that maps to an entity is a
/// selectable label; a node with children gets an expander whose header is the
/// (possibly selectable) label; a childless intermediate is a dim, inert label.
fn render_prim_node(
    ui: &mut egui::Ui,
    key: &NodeKey,
    view: &UsdPrimTreeSessionView,
    selected: &lunco_scene_commands::SelectedEntities,
    primary: Option<Entity>,
    reveal_path: Option<&str>,
    selection_changed: bool,
    to_select: &mut Option<Entity>,
    depth: usize,
) {
    let Some(node) = view.nodes.get(key) else {
        return;
    };
    let label = prim_label(node);

    if node.children.is_empty() {
        let _ = lunco_workbench::tree::leaf(ui, |ui| {
            prim_select_label(
                ui,
                node,
                &label,
                selected,
                primary,
                selection_changed,
                to_select,
            )
        });
        return;
    }
    // Top two levels open by default so the scene structure is visible without
    // drilling; deeper subtrees (a rover's per-wheel joints) start collapsed.
    let default_open = depth < 2;
    // The document path is stable and already scoped by this panel's active
    // document, so it is sufficient for collapse-state identity.
    let id = ui.make_persistent_id(("usd_prim_tree", key));
    let open = reveal_path.is_some_and(|path| is_path_or_descendant(path, key));
    let mut header_select = None;
    lunco_workbench::tree::branch(
        ui,
        id,
        default_open,
        open.then_some(true),
        |ui| {
            prim_select_label(
                ui,
                node,
                &label,
                selected,
                primary,
                selection_changed,
                &mut header_select,
            )
        },
        |ui| {
            for child in &node.children {
                render_prim_node(
                    ui,
                    child,
                    view,
                    selected,
                    primary,
                    reveal_path,
                    selection_changed,
                    to_select,
                    depth + 1,
                );
            }
        },
    );
    if header_select.is_some() {
        *to_select = header_select;
    }
}

/// The row for one prim: selectable when it has an entity, otherwise a dim inert
/// label (an intermediate xform the user can expand but not select).
fn prim_select_label(
    ui: &mut egui::Ui,
    node: &PrimTreeNode,
    label: &str,
    selected: &lunco_scene_commands::SelectedEntities,
    primary: Option<Entity>,
    selection_changed: bool,
    to_select: &mut Option<Entity>,
) -> bool {
    match node.entity {
        Some(entity) => {
            let hint = if node.type_name.is_empty() {
                "Click to select".to_string()
            } else {
                format!("{}  ·  click to select", node.type_name)
            };
            let hint = node
                .camera_identity
                .as_deref()
                .map(|identity| format!("{identity}  ·  {hint}"))
                .unwrap_or(hint);
            let resp = ui
                .add_sized(
                    [ui.available_width(), ui.spacing().interact_size.y],
                    egui::Button::selectable(selected.entities.contains(&entity), label),
                )
                .on_hover_text(hint);
            if selection_changed && primary == Some(entity) {
                resp.scroll_to_me(Some(egui::Align::Center));
            }
            if resp.clicked() {
                *to_select = Some(entity);
            }
            resp.clicked()
        }
        None => {
            let resp = ui.add_sized(
                [ui.available_width(), ui.spacing().interact_size.y],
                egui::Label::new(egui::RichText::new(label).weak()),
            );
            resp.clicked()
        }
    }
}

/// Return whether `path` is the node or a descendant of `node`.
fn is_path_or_descendant(path: &str, node: &str) -> bool {
    path == node
        || path
            .strip_prefix(node)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

/// `<marker> <name>` — a body marker for a rigid body, else a folder/dot.
fn prim_label(node: &PrimTreeNode) -> String {
    let glyph = if node.is_body {
        "[body]"
    } else if node.children.is_empty() {
        "·"
    } else {
        "▪"
    };
    format!("{glyph} {}", node.display_name)
}
