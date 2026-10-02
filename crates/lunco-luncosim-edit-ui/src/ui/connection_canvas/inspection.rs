//! Cached endpoint inspection and presentation guidance over the current graph.
use super::{UsdCanvasSessionState, UsdPrimNodeData, navigation, projection};
use bevy_egui::egui;
use lunco_canvas::{EdgeId, NodeId, PortRef, Scene, SelectItem};
use lunco_doc::Document;
use lunco_workbench_core::PanelCtx;
use std::collections::HashMap;

/// Adjacency is rebuilt on graph changes, never by traversing USD in paint.
#[derive(Default)]
pub(super) struct ConnectionIndex {
    generation: Option<u64>,
    incident: HashMap<NodeId, Vec<EdgeId>>,
}
impl ConnectionIndex {
    pub(super) fn refresh(&mut self, scene: &Scene) {
        if self.generation == Some(scene.generation()) {
            return;
        }
        self.incident.clear();
        for (id, edge) in scene.edges() {
            self.incident.entry(edge.from.node).or_default().push(*id);
            if edge.to.node != edge.from.node {
                self.incident.entry(edge.to.node).or_default().push(*id);
            }
        }
        self.generation = Some(scene.generation());
    }
    pub(super) fn edges(&self, node: NodeId) -> &[EdgeId] {
        self.incident.get(&node).map(Vec::as_slice).unwrap_or(&[])
    }
}

pub(super) fn selected_node(state: &UsdCanvasSessionState) -> Option<NodeId> {
    state
        .canvas
        .selection
        .port()
        .map(|port| port.node)
        .or_else(|| match state.canvas.selection.primary() {
            Some(SelectItem::Node(id)) => Some(id),
            _ => None,
        })
}

fn endpoint_label(state: &UsdCanvasSessionState, port: &PortRef) -> String {
    super::groups::endpoint(&state.canvas.scene, port)
        .map(|(node, name)| format!("{}.{name}", node.origin.as_deref().unwrap_or(&node.label)))
        .unwrap_or_else(|_| port.port.as_str().to_string())
}

fn reveal(
    ui: &mut egui::Ui,
    ctx: &mut PanelCtx,
    state: &UsdCanvasSessionState,
    endpoint: &PortRef,
) {
    if ui.button("Go to").clicked() {
        if let (Some(host), Ok((node, name))) = (
            &state.view_document,
            super::groups::endpoint(&state.canvas.scene, endpoint),
        ) {
            if let Some(key) = projection::diagram_key(node) {
                ctx.trigger(navigation::SelectConnectionElement {
                    view_id: host.document().id().raw(),
                    key: key.into(),
                    port: Some(name.into()),
                    reveal: true,
                });
            }
        }
    }
}

fn wire(ui: &mut egui::Ui, ctx: &mut PanelCtx, state: &UsdCanvasSessionState, id: EdgeId) {
    let Some(edge) = state.canvas.scene.edge(id) else {
        return;
    };
    let kind = edge
        .data
        .downcast_ref::<projection::UsdWireData>()
        .map(|data| data.kind);
    ui.strong(match kind {
        Some(projection::WireKind::Dataflow) => "Causal →",
        Some(projection::WireKind::Acausal) => "Acausal ↔",
        _ => "Joint ↔",
    });
    for endpoint in [&edge.from, &edge.to] {
        ui.label(endpoint_label(state, endpoint));
        reveal(ui, ctx, state, endpoint);
    }
    ui.separator();
}

pub(super) fn render(ui: &mut egui::Ui, ctx: &mut PanelCtx, state: &mut UsdCanvasSessionState) {
    ui.set_max_width(ui.available_width());
    egui::ScrollArea::vertical().id_salt("connection_inspection").show(ui, |ui| {
        ui.heading("Connections");
        let Some(host) = &state.view_document else { return; };
        let view_id = host.document().id().raw();
        if let Some(id) = selected_node(state) {
            let Some(node) = state.canvas.scene.node(id) else { return; };
            let Some(data) = node.data.downcast_ref::<UsdPrimNodeData>() else { return; };
            ui.strong(&node.label);
            if let Some(group_id) = &data.group_id {
                if let Some(group) = state.group_plan.iter().find(|g| &g.id == group_id) {
                    ui.label(format!("{} members · {} internal links · {} feedback links",group.members.len(),group.internal_links,group.feedback_links));
                    ui.horizontal(|ui| {if ui.button("Expand group").clicked() {ctx.trigger(super::groups::SetConnectionGroupCollapsed {view_id,group_id:group_id.clone(),collapsed:false});} if ui.button("Ungroup").clicked() {ctx.trigger(super::groups::RemoveConnectionGroup {view_id,group_id:group_id.clone()});}});
                    ui.strong("Members");
                    for member in &group.members {if ui.button(member).clicked() {ctx.trigger(navigation::SelectConnectionElement {view_id,key:member.clone(),port:None,reveal:true});}}
                    ui.separator();
                    ui.label("External ports retain their exact USD endpoints. Expand to add connections to other member ports.");
                    for port in &node.ports {ui.label(endpoint_label(state,&PortRef {node:id,port:port.id.clone()}));}
                    for edge in state.inspection.edges(id) {wire(ui,ctx,state,*edge);}
                }
                return;
            }
            ui.label(node.origin.as_deref().unwrap_or(""));
            ui.label(&data.type_name);
            ui.horizontal_wrapped(|ui| {
                if ui.button("Focus").clicked() { ctx.trigger(navigation::FrameConnectionDiagram { view_id, key: Some(data.view_key.clone()) }); }
                let path = node.origin.as_deref().unwrap_or("");
                let has_children = state.source_nodes.iter().any(|n| n.path.strip_prefix(path).is_some_and(|tail| tail.starts_with('/')));
                if has_children && data.boundary.is_none() && ui.button("Enter system").clicked() {
                    ctx.trigger(navigation::NavigateConnectionDiagram { view_id, scope: Some(path.into()), back: false });
                }
                if !has_children && !data.programs.is_empty() && ui.button("Open source").clicked() {
                    ctx.trigger(navigation::OpenConnectionNode { view_id, key: data.view_key.clone(), program_path: None });
                }
            });
            if !data.programs.is_empty() {
                ui.separator(); ui.strong("Attached programs");
                for program in &data.programs {
                    let theme = lunco_theme::active(ui.ctx());
                    let color = match program.backend.as_str() { "Modelica" => theme.schematic.class_model_badge, "Rhai" => theme.schematic.class_block_badge, "Python" => theme.schematic.class_record_badge, _ => theme.tokens.text };
                    if ui.add_enabled(program.issue.is_none(), egui::Button::new(egui::RichText::new(format!("Open {}", program.backend)).color(color))).on_hover_text(&program.path).clicked() {
                        ctx.trigger(navigation::OpenConnectionNode { view_id, key: data.view_key.clone(), program_path: Some(program.path.clone()) });
                    }
                    ui.label(&program.source);
                    if let Some(issue) = &program.issue { ui.colored_label(theme.tokens.warning, issue); }
                }
            }
            ui.separator(); ui.strong("Ports");
            let ports: Vec<_> = node.ports.iter().filter(|p| !p.id.as_str().starts_with('~')).collect();
            if ports.is_empty() {
                ui.label("No authored ports");
                ui.label("Hierarchy and mounted geometry do not imply signal connections.");
            }
            for port in ports {
                let endpoint = PortRef { node: id, port: port.id.clone() };
                let count = state.inspection.edges(id).iter().filter(|edge| state.canvas.scene.edge(**edge).is_some_and(|edge| edge.from == endpoint || edge.to == endpoint)).count();
                let selected = state.canvas.selection.port() == Some(&endpoint);
                if ui.selectable_label(selected, format!("{} · {} · {} links", port.id.as_str(), port.kind, count)).clicked() {
                    ctx.trigger(navigation::SelectConnectionElement { view_id, key: data.view_key.clone(), port: Some(port.id.as_str().to_string()), reveal: false });
                }
                if selected {
                    if let Some(type_name) = data.port_types.get(port.id.as_str()) { ui.label(format!("USD type: {type_name}")); }
                    if state.doc.is_some() { connection_picker(ui, ctx, state, &endpoint); }
                }
            }
            ui.separator(); ui.strong("Connected endpoints");
            for edge in state.inspection.edges(id) {
                if state.canvas.selection.port().is_none_or(|port| state.canvas.scene.edge(*edge).is_some_and(|e| e.from == *port || e.to == *port)) { wire(ui, ctx, state, *edge); }
            }
        } else if let Some(SelectItem::Edge(id)) = state.canvas.selection.primary() {
            wire(ui, ctx, state, id);
        } else {
            ui.label("Select a prim, port or wire to trace its connections.");
            ui.label("Double-click a system to enter. Back restores your previous view.");
            ui.separator();
            ui.label(if state.doc.is_some() { "Drag ports to connect, or select a port and choose Connect to. Drop USD into space; drop a model or script onto its host prim." } else { "Scene browsing: node layout can move. Use Edit connections to author USD." });
        }
        if let Some(error) = &state.last_error { ui.separator(); ui.colored_label(lunco_theme::active(ui.ctx()).tokens.warning, error); }
    });
}

fn connection_picker(
    ui: &mut egui::Ui,
    ctx: &mut PanelCtx,
    state: &UsdCanvasSessionState,
    from: &PortRef,
) {
    ui.menu_button("Connect to…", |ui| {
        egui::ScrollArea::vertical()
            .max_height(260.0)
            .show(ui, |ui| {
                for (id, node) in state.canvas.scene.nodes() {
                    for port in &node.ports {
                        let to = PortRef {
                            node: *id,
                            port: port.id.clone(),
                        };
                        if super::validate_connection(&state.canvas.scene, from, &to).is_err() {
                            continue;
                        }
                        if ui
                            .button(format!("{} · {}", node.label, port.id.as_str()))
                            .clicked()
                        {
                            if let (Some(doc_id), Some(edit_target)) =
                                (state.doc, &state.edit_target)
                            {
                                if let Ok(op) =
                                    super::connect_op(&state.canvas.scene, from, &to, edit_target)
                                {
                                    ctx.trigger(lunco_usd_core::commands::ApplyUsdOps {
                                        doc_id,
                                        parent_gen: Some(state.generation),
                                        label: "Connect USD ports".into(),
                                        ops: vec![op],
                                    });
                                }
                            }
                            ui.close();
                        }
                    }
                }
            });
    });
}

pub(super) fn find(ui: &mut egui::Ui, ctx: &mut PanelCtx, state: &mut UsdCanvasSessionState) {
    ui.add(
        egui::TextEdit::singleline(&mut state.scope_search).hint_text("Find prim, port or program"),
    );
    let needle = state.scope_search.to_lowercase();
    let Some(host) = &state.view_document else {
        return;
    };
    let view_id = host.document().id().raw();
    egui::ScrollArea::vertical()
        .max_height(300.0)
        .show(ui, |ui| {
            for node in &state.source_nodes {
                if needle.is_empty() || node.path.to_lowercase().contains(&needle) {
                    if ui.button(&node.path).clicked() {
                        ctx.trigger(navigation::SelectConnectionElement {
                            view_id,
                            key: node.path.clone(),
                            port: None,
                            reveal: true,
                        });
                        ui.close();
                    }
                }
                if needle.is_empty() {
                    continue;
                }
                for port in node
                    .port_types
                    .keys()
                    .chain(node.referenced_ports.iter())
                    .collect::<std::collections::BTreeSet<_>>()
                {
                    if port.to_lowercase().contains(&needle)
                        && ui.button(format!("{}.{}", node.path, port)).clicked()
                    {
                        ctx.trigger(navigation::SelectConnectionElement {
                            view_id,
                            key: node.path.clone(),
                            port: Some(port.clone()),
                            reveal: true,
                        });
                        ui.close();
                    }
                }
                for program in &node.programs {
                    if (program.source.to_lowercase().contains(&needle)
                        || program.backend.to_lowercase().contains(&needle))
                        && ui
                            .button(format!("{} · {}", node.path, program.backend))
                            .on_hover_text(&program.source)
                            .clicked()
                    {
                        ctx.trigger(navigation::SelectConnectionElement {
                            view_id,
                            key: node.path.clone(),
                            port: None,
                            reveal: true,
                        });
                        ui.close();
                    }
                }
            }
        });
}

pub(super) fn context_menu(ui: &mut egui::Ui, ctx: &mut PanelCtx, state: &UsdCanvasSessionState) {
    if let (Some(id), Some(host)) = (selected_node(state), &state.view_document) {
        if let Some(key) = state
            .canvas
            .scene
            .node(id)
            .and_then(projection::diagram_key)
        {
            if ui.button("Focus selection").clicked() {
                ctx.trigger(navigation::FrameConnectionDiagram {
                    view_id: host.document().id().raw(),
                    key: Some(key.into()),
                });
                ui.close();
            }
            let node = state.canvas.scene.node(id).unwrap();
            let has_children = node.origin.as_deref().is_some_and(|path| {
                state.source_nodes.iter().any(|source| {
                    source
                        .path
                        .strip_prefix(path)
                        .is_some_and(|tail| tail.starts_with('/'))
                })
            });
            let has_program = node
                .data
                .downcast_ref::<UsdPrimNodeData>()
                .is_some_and(|data| !data.programs.is_empty());
            if (has_children || has_program) && ui.button("Open selection").clicked() {
                ctx.trigger(navigation::OpenConnectionNode {
                    view_id: host.document().id().raw(),
                    key: key.into(),
                    program_path: None,
                });
                ui.close();
            }
        }
    }
    if let Some(host) = &state.view_document {
        if ui.button("Fit system").clicked() {
            ctx.trigger(navigation::FrameConnectionDiagram {
                view_id: host.document().id().raw(),
                key: None,
            });
            ui.close();
        }
        if ui
            .add_enabled(
                !state.navigation_history.is_empty(),
                egui::Button::new("Back"),
            )
            .clicked()
        {
            ctx.trigger(navigation::NavigateConnectionDiagram {
                view_id: host.document().id().raw(),
                scope: None,
                back: true,
            });
            ui.close();
        }
    }
}

pub(super) fn connection_guidance(
    ui: &mut egui::Ui,
    response: &egui::Response,
    state: &UsdCanvasSessionState,
) {
    let Some(from) = state.canvas.tool.connection_origin() else {
        return;
    };
    let Some(rect) = state.canvas_rect else {
        return;
    };
    let theme = lunco_theme::active(ui.ctx());
    for (id, node) in state.canvas.scene.nodes() {
        for port in &node.ports {
            let to = PortRef {
                node: *id,
                port: port.id.clone(),
            };
            if super::validate_connection(&state.canvas.scene, &from, &to).is_ok() {
                let p = state
                    .canvas
                    .viewport
                    .world_to_screen(port.world_pos(node.rect), rect);
                ui.painter().circle_stroke(
                    egui::pos2(p.x, p.y),
                    ui.spacing().interact_size.y * 0.35,
                    egui::Stroke::new(2.0, theme.tokens.port_input),
                );
            }
        }
    }
    if let Some(pointer) = response.hover_pos() {
        let world = state
            .canvas
            .viewport
            .screen_to_world(lunco_canvas::Pos::new(pointer.x, pointer.y), rect);
        if let Some((node, lunco_canvas::NodeHitKind::Port(port))) =
            state.canvas.scene.hit_node(world, 6.0)
        {
            let to = PortRef { node, port };
            if let Err(reason) = super::validate_connection(&state.canvas.scene, &from, &to) {
                response.clone().on_hover_ui(|ui| {
                    ui.colored_label(theme.tokens.warning, reason);
                });
            }
        }
    }
}
