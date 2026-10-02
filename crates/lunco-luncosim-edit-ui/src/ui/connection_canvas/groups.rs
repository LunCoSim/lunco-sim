//! View-only group plans, exact collapsed endpoints, and journaled group gestures.
use super::{UsdCanvasSessionState, UsdCanvasState, projection, view_files::state_for};
use bevy::prelude::*;
use bevy_egui::egui;
use lunco_canvas::{Node, NodeId, Port, PortId, PortRef, Pos, Rect, Scene};
use lunco_core::{Command, on_command, register_commands};
use lunco_doc::diagram_view::{DiagramGroup, DiagramPosition, DiagramViewOp};
use lunco_doc::{Ack, Document, Mutation, OpId};
use lunco_hooks::{HookValue, RuntimeExecutionContext};
use lunco_workbench_core::PanelCtx;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

lunco_hooks::declare_hook! {
    id: "diagram.group.plan", owner: "lunco-luncosim-edit-ui",
    description: "Choose view-only groups from composed component contracts, collections and topology.",
    signature: [facts: Map], output: ArrayOfMap, deterministic: true, required: false, installable: true,
}

/// Endpoint identity carried by one summary port; it always resolves to a real card.
#[derive(Clone, Debug)]
pub(super) struct GroupEndpoint {
    pub node: Arc<Node>,
    pub port: PortId,
}

#[derive(Clone, Debug)]
pub(super) struct Group {
    pub id: String,
    pub label: String,
    pub members: BTreeSet<String>,
    pub collapsed: bool,
    pub rect: Option<Rect>,
    pub internal_links: usize,
    pub feedback_links: usize,
}
impl Group {
    pub(super) fn key(&self) -> String {
        format!("#group:{}", self.id)
    }
}

/// Merge authored overrides with a validated disjoint policy partition on the worker.
pub(super) fn evaluate(
    facts: &HookValue,
    context: RuntimeExecutionContext,
) -> (Vec<Group>, Vec<String>) {
    let mut warnings = Vec::new();
    let mut groups = Vec::new();
    let mut used = BTreeSet::new();
    let nodes: BTreeSet<_> = match facts.get("nodes") {
        Some(HookValue::Array(nodes)) => nodes
            .iter()
            .filter(|n| n.get("role").and_then(HookValue::as_str) == Some("prim"))
            .filter_map(|n| n.get("path").and_then(HookValue::as_str))
            .collect(),
        _ => return (groups, vec!["Grouping facts lack nodes".into()]),
    };
    let enabled = facts
        .get("grouping")
        .and_then(HookValue::as_bool)
        .unwrap_or(true);
    if !enabled {
        return (groups, warnings);
    }
    let strings = |key| -> BTreeSet<String> {
        match facts.get(key) {
            Some(HookValue::Array(values)) => values
                .iter()
                .filter_map(HookValue::as_str)
                .map(str::to_string)
                .collect(),
            _ => Default::default(),
        }
    };
    let collapsed = strings("collapsed_groups");
    let excluded = strings("excluded_groups");
    let manual = match facts.get("manual_groups") {
        Some(HookValue::Array(values)) => values.clone(),
        _ => Vec::new(),
    };
    let policy = lunco_hooks::invoke_with_context("diagram.group.plan", &[facts.clone()], context);
    let automatic = match policy {
        Some(Ok(HookValue::Array(groups))) => groups,
        Some(Ok(_)) => {
            warnings.push(
                "Grouping policy must return an array of groups; showing ungrouped cards".into(),
            );
            Vec::new()
        }
        Some(Err(error)) => {
            warnings.push(format!(
                "Grouping policy failed: {error}; showing ungrouped cards"
            ));
            Vec::new()
        }
        None => {
            warnings.push("Grouping policy is unavailable; showing ungrouped cards".into());
            Vec::new()
        }
    };
    for (authored, entry) in manual
        .into_iter()
        .map(|e| (true, e))
        .chain(automatic.into_iter().map(|e| (false, e)))
    {
        let Some(id) = entry
            .get("id")
            .and_then(HookValue::as_str)
            .filter(|id| !id.trim().is_empty())
        else {
            warnings.push("Skipped group without an identity".into());
            continue;
        };
        if excluded.contains(id) {
            continue;
        }
        let Some(label) = entry
            .get("label")
            .and_then(HookValue::as_str)
            .filter(|s| !s.trim().is_empty() && s.len() <= 256)
        else {
            warnings.push(format!("Skipped group {id}: invalid label"));
            continue;
        };
        let Some(HookValue::Array(entries)) = entry.get("members") else {
            warnings.push(format!("Skipped group {id}: missing membership"));
            continue;
        };
        let mut members = BTreeSet::new();
        let mut malformed = false;
        for member in entries {
            let Some(path) = member.as_str() else {
                malformed = true;
                continue;
            };
            if !nodes.contains(path) || used.contains(path) {
                if !authored && !used.contains(path) {
                    malformed = true;
                }
                continue;
            }
            if !members.insert(path.to_string()) {
                malformed = true;
            }
        }
        if malformed {
            warnings.push(format!(
                "Group {id} contains duplicate or missing members; recovered usable membership"
            ));
        }
        if members.len() < 2 {
            continue;
        }
        if groups.iter().any(|g: &Group| g.id == id) {
            warnings.push(format!("Skipped duplicate group {id}"));
            continue;
        }
        used.extend(members.iter().cloned());
        groups.push(Group {
            id: id.into(),
            label: label.into(),
            members,
            collapsed: collapsed.contains(id),
            rect: None,
            internal_links: 0,
            feedback_links: 0,
        });
    }
    (groups, warnings)
}

/// Resolve a summary endpoint once at the generic authoring/inspection boundary.
pub(super) fn endpoint<'a>(
    scene: &'a Scene,
    port: &'a PortRef,
) -> Result<(&'a Node, &'a str), String> {
    let node = scene
        .node(port.node)
        .ok_or("Connection endpoint card is absent")?;
    if let Some(endpoint) = node
        .data
        .downcast_ref::<projection::UsdPrimNodeData>()
        .and_then(|data| data.group_ports.get(port.port.as_str()))
    {
        Ok((&endpoint.node, endpoint.port.as_str()))
    } else {
        Ok((node, port.port.as_str()))
    }
}

/// Rebuild the visible quotient graph. The full scene retains every original edge.
pub(super) fn project(state: &mut UsdCanvasSessionState) {
    let full = state.canvas.scene.clone();
    state.expanded_scene = Some(full.clone());
    let by_key: HashMap<_, _> = full
        .nodes()
        .filter_map(|(id, n)| projection::diagram_key(n).map(|key| (key.to_string(), *id)))
        .collect();
    let mut membership = HashMap::new();
    for (index, group) in state.group_plan.iter_mut().enumerate() {
        let members: Vec<_> = group
            .members
            .iter()
            .filter_map(|key| by_key.get(key).copied())
            .collect();
        group.rect = members
            .iter()
            .filter_map(|id| full.node(*id).map(|n| n.rect))
            .reduce(|a, b| {
                Rect::from_min_max(
                    Pos::new(a.min.x.min(b.min.x), a.min.y.min(b.min.y)),
                    Pos::new(a.max.x.max(b.max.x), a.max.y.max(b.max.y)),
                )
            });
        group.internal_links = 0;
        group.feedback_links = 0;
        for id in members {
            membership.insert(id, index);
        }
    }
    for (_, edge) in full.edges() {
        if let (Some(a), Some(b)) = (
            membership.get(&edge.from.node),
            membership.get(&edge.to.node),
        ) {
            if a == b {
                state.group_plan[*a].internal_links += 1;
                if edge.from.node == edge.to.node {
                    state.group_plan[*a].feedback_links += 1;
                }
            }
        }
    }
    if !state.group_plan.iter().any(|g| g.collapsed) {
        return;
    }
    let mut visible = full.clone();
    let mut proxies = HashMap::new();
    let mut portals: HashMap<(NodeId, PortId), PortRef> = HashMap::new();
    for (index, group) in state
        .group_plan
        .iter_mut()
        .enumerate()
        .filter(|(_, g)| g.collapsed)
    {
        let Some(bounds) = group.rect else {
            continue;
        };
        let id = visible.alloc_node_id();
        proxies.insert(index, id);
        let mut endpoints = BTreeMap::new();
        for (_, edge) in full.edges() {
            let from = membership.get(&edge.from.node) == Some(&index);
            let to = membership.get(&edge.to.node) == Some(&index);
            if from == to {
                continue;
            }
            let endpoint = if from { &edge.from } else { &edge.to };
            if let Some(node) = full.node(endpoint.node) {
                let key = format!(
                    "{}.{}",
                    projection::diagram_key(node).unwrap_or(""),
                    endpoint.port.as_str()
                );
                endpoints.entry(key).or_insert_with(|| GroupEndpoint {
                    node: Arc::new(node.clone()),
                    port: endpoint.port.clone(),
                });
            }
        }
        let height = (projection::PORT_ROW_H * endpoints.len() as f32 + 70.0).max(96.0);
        let rect = Rect::from_min_size(bounds.min, 250.0, height);
        group.rect = Some(rect);
        let mut ports = Vec::new();
        let mut types = BTreeMap::new();
        let mut left = 0;
        let mut right = 0;
        for (key, endpoint) in &endpoints {
            let Some(original) = endpoint.node.ports.iter().find(|p| p.id == endpoint.port) else {
                continue;
            };
            let output = original.kind.as_str() == "output";
            let row = if output {
                let row = right;
                right += 1;
                row
            } else {
                let row = left;
                left += 1;
                row
            };
            ports.push(Port {
                id: PortId::new(key.as_str()),
                kind: original.kind.clone(),
                local_offset: Pos::new(
                    if output { 250.0 } else { 0.0 },
                    49.5 + row as f32 * projection::PORT_ROW_H,
                ),
            });
            if let Some(data) = endpoint
                .node
                .data
                .downcast_ref::<projection::UsdPrimNodeData>()
            {
                if let Some(ty) = data.port_types.get(endpoint.port.as_str()) {
                    types.insert(key.clone(), ty.clone());
                }
            }
            portals.insert(
                (endpoint.node.id, endpoint.port.clone()),
                PortRef {
                    node: id,
                    port: PortId::new(key.as_str()),
                },
            );
        }
        visible.insert_node(Node {
            id,
            rect,
            kind: projection::NODE_KIND.into(),
            data: Arc::new(projection::UsdPrimNodeData {
                group_id: Some(group.id.clone()),
                group_ports: endpoints,
                programs: Vec::new(),
                accent: Some(projection::DiagramAccent::Package),
                type_name: format!(
                    "{} members · {} internal · {} feedback",
                    group.members.len(),
                    group.internal_links,
                    group.feedback_links
                ),
                is_body: false,
                port_types: types,
                port_sources: Default::default(),
                view_key: group.key(),
                boundary: None,
            }),
            ports,
            label: format!("{} · {}", group.label, group.members.len()),
            origin: None,
            resizable: false,
            visual_rect: None,
        });
    }
    let edges: Vec<_> = full.edges().map(|(_, edge)| edge.clone()).collect();
    for (node, index) in &membership {
        if proxies.contains_key(index) {
            visible.remove_node(*node);
        }
    }
    for mut edge in edges {
        let a = membership
            .get(&edge.from.node)
            .filter(|i| proxies.contains_key(i));
        let b = membership
            .get(&edge.to.node)
            .filter(|i| proxies.contains_key(i));
        if a.is_some() && a == b {
            continue;
        }
        if let Some(endpoint) = portals.get(&(edge.from.node, edge.from.port.clone())) {
            edge.from = endpoint.clone();
        }
        if let Some(endpoint) = portals.get(&(edge.to.node, edge.to.port.clone())) {
            edge.to = endpoint.clone();
        }
        edge.waypoints.clear();
        visible.insert_edge(edge);
    }
    projection::route_edges(&mut visible);
    state.canvas.scene = visible;
}

/// Author a manual view group; the document validates disjoint membership.
#[Command(default)]
pub struct SetConnectionGroup {
    pub view_id: u64,
    pub group_id: String,
    pub label: String,
    pub members: Vec<String>,
}
/// Persist disclosure independently for this named view.
#[Command(default)]
pub struct SetConnectionGroupCollapsed {
    pub view_id: u64,
    pub group_id: String,
    pub collapsed: bool,
}
/// Remove a manual group, or suppress one policy-generated group in this view.
#[Command(default)]
pub struct RemoveConnectionGroup {
    pub view_id: u64,
    pub group_id: String,
}
/// Move all member placements in one reversible view-document transaction.
#[Command(default)]
pub struct MoveConnectionGroup {
    pub view_id: u64,
    pub group_id: String,
    pub dx: f64,
    pub dy: f64,
}
/// Enable or disable automatic and manual group presentation for one view.
#[Command(default)]
pub struct SetConnectionGrouping {
    pub view_id: u64,
    pub enabled: bool,
}

pub(super) fn edit(
    state: &mut UsdCanvasSessionState,
    mutate: impl FnOnce(&mut lunco_doc::diagram_view::DiagramView) -> Result<(), String>,
) -> Result<Ack, String> {
    let host = state
        .view_document
        .as_mut()
        .ok_or("View document is unavailable")?;
    let mut view = host
        .document()
        .data()
        .views
        .get(&state.selected_view)
        .ok_or("Named view is absent")?
        .clone();
    mutate(&mut view)?;
    let ack = host
        .apply(Mutation::local(DiagramViewOp::SetView {
            name: state.selected_view.clone(),
            view: Some(view),
        }))
        .map_err(|e| e.to_string())?;
    state.navigation_restore = Some((state.canvas.viewport.center, state.canvas.viewport.zoom));
    state.rebuild_view();
    Ok(ack)
}
#[on_command(SetConnectionGroup)]
fn set_group(
    trigger: On<SetConnectionGroup>,
    mut views: ResMut<UsdCanvasState>,
) -> Result<Ack, String> {
    let cmd = trigger.event();
    let state = state_for(&mut views, cmd.view_id)?;
    if cmd.members.iter().any(|path| {
        !state.canvas.scene.nodes().any(|(_, n)| {
            projection::diagram_key(n) == Some(path)
                && n.data
                    .downcast_ref::<projection::UsdPrimNodeData>()
                    .is_some_and(|d| d.boundary.is_none() && d.group_id.is_none())
        })
    }) {
        return Err("Manual grouping needs visible USD prim cards".into());
    }
    let id = if cmd.group_id.is_empty() {
        format!("manual:{}", OpId::new().0)
    } else {
        cmd.group_id.clone()
    };
    edit(state, |view| {
        view.groups.insert(
            id,
            DiagramGroup {
                label: cmd.label.clone(),
                members: cmd.members.iter().cloned().collect(),
            },
        );
        Ok(())
    })
}
#[on_command(SetConnectionGroupCollapsed)]
fn collapse_group(
    trigger: On<SetConnectionGroupCollapsed>,
    mut views: ResMut<UsdCanvasState>,
) -> Result<Ack, String> {
    let cmd = trigger.event();
    let state = state_for(&mut views, cmd.view_id)?;
    if !state.group_plan.iter().any(|g| g.id == cmd.group_id) {
        return Err("Group is absent from the current view".into());
    }
    edit(state, |view| {
        if cmd.collapsed {
            view.collapsed_groups.insert(cmd.group_id.clone());
        } else {
            view.collapsed_groups.remove(&cmd.group_id);
        }
        Ok(())
    })
}
#[on_command(RemoveConnectionGroup)]
fn remove_group(
    trigger: On<RemoveConnectionGroup>,
    mut views: ResMut<UsdCanvasState>,
) -> Result<Ack, String> {
    let cmd = trigger.event();
    let state = state_for(&mut views, cmd.view_id)?;
    if !state.group_plan.iter().any(|g| g.id == cmd.group_id) {
        return Err("Group is absent from the current view".into());
    }
    edit(state, |view| {
        view.groups.remove(&cmd.group_id);
        view.collapsed_groups.remove(&cmd.group_id);
        view.excluded_groups.insert(cmd.group_id.clone());
        Ok(())
    })
}
#[on_command(MoveConnectionGroup)]
fn move_group(
    trigger: On<MoveConnectionGroup>,
    mut views: ResMut<UsdCanvasState>,
) -> Result<Ack, String> {
    let cmd = trigger.event();
    let state = state_for(&mut views, cmd.view_id)?;
    if !cmd.dx.is_finite() || !cmd.dy.is_finite() {
        return Err("Group displacement must be finite".into());
    }
    let group = state
        .group_plan
        .iter()
        .find(|g| g.id == cmd.group_id)
        .ok_or("Group is absent from the current view")?;
    let full = state.expanded_scene.as_ref().unwrap_or(&state.canvas.scene);
    let mut positions = Vec::new();
    for (_, node) in full.nodes() {
        if projection::diagram_key(node).is_some_and(|key| group.members.contains(key)) {
            let stored = state
                .view_document
                .as_ref()
                .and_then(|host| host.document().data().views.get(&state.selected_view))
                .and_then(|view| {
                    projection::diagram_key(node).and_then(|key| view.positions.get(key))
                });
            let x = stored
                .map(|p| p.x)
                .unwrap_or_else(|| f64::from(node.rect.min.x))
                + cmd.dx;
            let y = stored
                .map(|p| p.y)
                .unwrap_or_else(|| f64::from(node.rect.min.y))
                + cmd.dy;
            if x.abs() > f64::from(f32::MAX) || y.abs() > f64::from(f32::MAX) {
                return Err("Group displacement exceeds the rendering range".into());
            }
            positions.push((
                projection::diagram_key(node).unwrap().to_string(),
                DiagramPosition { x, y },
            ));
        }
    }
    edit(state, |view| {
        view.positions.extend(positions);
        Ok(())
    })
}
#[on_command(SetConnectionGrouping)]
fn grouping(
    trigger: On<SetConnectionGrouping>,
    mut views: ResMut<UsdCanvasState>,
) -> Result<Ack, String> {
    let cmd = trigger.event();
    edit(state_for(&mut views, cmd.view_id)?, |view| {
        view.grouping = cmd.enabled;
        Ok(())
    })
}
register_commands!(
    set_group,
    collapse_group,
    remove_group,
    move_group,
    grouping
);

/// Paint cached group bounds; interactions dispatch the same typed view commands.
pub(super) fn frames(
    ui: &mut egui::Ui,
    ctx: &mut PanelCtx,
    state: &UsdCanvasSessionState,
    graph: egui::Rect,
) {
    let Some(host) = &state.view_document else {
        return;
    };
    let view_id = host.document().id().raw();
    let theme = lunco_theme::active(ui.ctx());
    let viewport_rect = Rect::from_min_max(
        Pos::new(graph.min.x, graph.min.y),
        Pos::new(graph.max.x, graph.max.y),
    );
    for group in state.group_plan.iter().filter(|g| !g.collapsed) {
        let Some(bounds) = group.rect else {
            continue;
        };
        let a = state.canvas.viewport.world_to_screen(
            Pos::new(bounds.min.x - 24.0, bounds.min.y - 48.0),
            viewport_rect,
        );
        let b = state.canvas.viewport.world_to_screen(
            Pos::new(bounds.max.x + 24.0, bounds.max.y + 24.0),
            viewport_rect,
        );
        let frame = egui::Rect::from_min_max(egui::pos2(a.x, a.y), egui::pos2(b.x, b.y));
        if !frame.intersects(graph) {
            continue;
        }
        let header = egui::Rect::from_min_size(
            frame.min,
            egui::vec2(frame.width(), ui.spacing().interact_size.y),
        );
        let response = ui.interact(
            header.intersect(graph),
            egui::Id::new(("diagram_group", view_id, &group.id)),
            egui::Sense::click_and_drag(),
        );
        let drag_id = response.id.with("displacement");
        let mut displacement = ui
            .ctx()
            .data_mut(|data| data.get_temp::<egui::Vec2>(drag_id).unwrap_or_default());
        if response.drag_started() {
            displacement = egui::Vec2::ZERO;
        }
        if response.dragged() {
            displacement += response.drag_delta();
            ui.ctx()
                .data_mut(|data| data.insert_temp(drag_id, displacement));
        }
        let painter = ui.painter().with_clip_rect(graph);
        painter.rect_stroke(
            frame.translate(displacement),
            egui::CornerRadius::same(theme.rounding.panel as u8),
            egui::Stroke::new(1.0, theme.tokens.accent),
            egui::StrokeKind::Outside,
        );
        painter.text(
            header.left_center(),
            egui::Align2::LEFT_CENTER,
            format!("{} · {} members", group.label, group.members.len()),
            egui::TextStyle::Body.resolve(ui.style()),
            theme.tokens.text,
        );
        if response.double_clicked() {
            ctx.trigger(SetConnectionGroupCollapsed {
                view_id,
                group_id: group.id.clone(),
                collapsed: true,
            });
        }
        if response.drag_stopped() {
            let d = displacement / state.canvas.viewport.zoom;
            ui.ctx().data_mut(|data| data.remove::<egui::Vec2>(drag_id));
            ctx.trigger(MoveConnectionGroup {
                view_id,
                group_id: group.id.clone(),
                dx: f64::from(d.x),
                dy: f64::from(d.y),
            });
        }
        response.context_menu(|ui| {
            if ui.button("Collapse group").clicked() {
                ctx.trigger(SetConnectionGroupCollapsed {
                    view_id,
                    group_id: group.id.clone(),
                    collapsed: true,
                });
                ui.close();
            }
            if ui.button("Ungroup").clicked() {
                ctx.trigger(RemoveConnectionGroup {
                    view_id,
                    group_id: group.id.clone(),
                });
                ui.close();
            }
        });
    }
}

pub(super) fn init(app: &mut App) {
    register_all_commands(app);
}
