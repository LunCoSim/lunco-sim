//! Source-scoped interface and binding views over the existing diagram projection.
use super::{UsdCanvasSessionState, UsdCanvasState, UsdPrimNodeData, navigation, projection};
use bevy::prelude::*;
use bevy_egui::egui;
use lunco_canvas::{EdgeId, NodeId, PortId, PortRef, Scene, SceneEvent};
use lunco_core::{Command, on_command, register_commands};
use lunco_doc::{Ack, Document, OpId};
use std::collections::{BTreeMap, HashMap, HashSet};

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum Presentation {
    #[default]
    Topology,
    Interface,
    Bindings,
}
impl Presentation {
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Topology => "topology",
            Self::Interface => "interface",
            Self::Bindings => "bindings",
        }
    }
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "topology" => Ok(Self::Topology),
            "interface" => Ok(Self::Interface),
            "bindings" => Ok(Self::Bindings),
            _ => Err("Presentation must be topology, interface or bindings".into()),
        }
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum InterfaceTab {
    #[default]
    Inputs,
    Outputs,
    Physical,
}
impl InterfaceTab {
    fn kind(self) -> &'static str {
        match self {
            Self::Inputs => "input",
            Self::Outputs => "output",
            Self::Physical => "acausal",
        }
    }
    fn title(self) -> &'static str {
        match self {
            Self::Inputs => "Inputs",
            Self::Outputs => "Outputs",
            Self::Physical => "Physical connectors",
        }
    }
}

/// Presentation preferences belong to the exact source lease, never to USD.
#[derive(Default)]
pub(super) struct Workspace {
    pub mode: Presentation,
    pub show_local_wires: bool,
    pub live_values: bool,
    pub animate_changes: bool,
    pub local_count: usize,
    pub hidden_local_count: usize,
    pub rows: Vec<BindingRow>,
    ports: Vec<InterfacePort>,
    model_ports: BTreeMap<String, Vec<usize>>,
    peers: HashMap<(NodeId, PortId), Vec<usize>>,
    filtered_rows: Vec<usize>,
    filter_key: Option<(String, bool)>,
    model: String,
    filter: String,
    local_only: bool,
    from: Option<PortRef>,
    to: Option<PortRef>,
    replace: bool,
    editor_open: bool,
    editing: Option<super::authoring::BindingPair>,
    interface_tab: InterfaceTab,
}

#[derive(Clone)]
pub(super) struct BindingRow {
    pub id: EdgeId,
    pub from: PortRef,
    pub to: PortRef,
    pub source: String,
    pub target: String,
    pub kind: projection::WireKind,
    pub local: bool,
}

/// Resolve rendered group portals once before tracing the full source graph.
pub(super) struct SelectionTrace {
    port: Option<String>,
    paths: HashSet<String>,
    edges: HashSet<EdgeId>,
}
impl SelectionTrace {
    pub(super) fn new(state: &UsdCanvasSessionState) -> Self {
        let port = state
            .canvas
            .selection
            .port()
            .and_then(|port| super::groups::endpoint(&state.canvas.scene, port).ok())
            .and_then(|(node, name)| node.origin.as_ref().map(|path| format!("{path}.{name}")));
        let paths = if port.is_some() {
            HashSet::new()
        } else {
            state
                .canvas
                .selection
                .nodes()
                .iter()
                .filter_map(|id| state.canvas.scene.node(*id))
                .flat_map(|node| {
                    let group = node
                        .data
                        .downcast_ref::<UsdPrimNodeData>()
                        .and_then(|data| data.group_id.as_ref())
                        .and_then(|id| state.group_plan.iter().find(|group| &group.id == id));
                    group
                        .map(|group| group.members.iter().cloned().collect::<Vec<_>>())
                        .unwrap_or_else(|| node.origin.iter().cloned().collect())
                })
                .collect()
        };
        Self {
            port,
            paths,
            edges: state.canvas.selection.edges().into_iter().collect(),
        }
    }
    pub(super) fn matches(&self, row: &BindingRow) -> bool {
        if let Some(port) = &self.port {
            return &row.source == port || &row.target == port;
        }
        self.edges.contains(&row.id)
            || [&row.source, &row.target]
                .iter()
                .filter_map(|key| key.rsplit_once('.').map(|(path, _)| path))
                .any(|path| self.paths.contains(path))
    }
}

#[derive(Clone)]
struct InterfacePort {
    endpoint: PortRef,
    key: String,
    path: String,
    name: String,
    kind: String,
    usd_type: Option<String>,
}

/// Select the live scene or the focused USD document as the Connections source.
#[Command(default)]
pub struct SetConnectionSource {
    /// True selects the live scene; false selects the focused USD document.
    pub active_scene: bool,
}
#[on_command(SetConnectionSource)]
fn select_source(
    trigger: On<SetConnectionSource>,
    mut views: ResMut<UsdCanvasState>,
) -> Result<Ack, String> {
    views.show_scene = trigger.active_scene;
    Ok(Ack::new(OpId::new()))
}

/// Configure one exact source-leased view.
#[Command(default)]
pub struct SetConnectionWorkspace {
    /// Exact Connections view-document identity returned by InspectConnectionDiagram.
    pub view_id: u64,
    /// Optional presentation: topology, interface or bindings.
    pub presentation: Option<String>,
    /// Reveal same-prim causal wires in the topology presentation.
    pub show_local_wires: Option<bool>,
    /// Request live port values in a scene view; document previews remain inert.
    pub live_values: Option<bool>,
    /// Highlight observed value changes while the simulation advances.
    pub animate_changes: Option<bool>,
}
#[on_command(SetConnectionWorkspace)]
fn configure(
    trigger: On<SetConnectionWorkspace>,
    mut views: ResMut<UsdCanvasState>,
) -> Result<Ack, String> {
    let request = trigger.event();
    let mode = request
        .presentation
        .as_deref()
        .map(Presentation::parse)
        .transpose()?;
    let state = super::view_files::state_for(&mut views, request.view_id)?;
    if let Some(mode) = mode {
        state.workspace.mode = mode;
    }
    if let Some(value) = request.live_values {
        state.workspace.live_values = value;
    }
    if let Some(value) = request.animate_changes {
        state.workspace.animate_changes = value;
    }
    if let Some(value) = request.show_local_wires {
        if state.workspace.show_local_wires != value {
            state.workspace.show_local_wires = value;
            state.restore_placements();
        }
    }
    if !state.workspace.live_values {
        state.activity.clear_samples();
    }
    Ok(Ack::new(OpId::new()))
}
register_commands!(configure, select_source);
pub(super) fn init(app: &mut App) {
    register_all_commands(app);
}

pub(super) fn full_scene(state: &UsdCanvasSessionState) -> &Scene {
    state.expanded_scene.as_ref().unwrap_or(&state.canvas.scene)
}

/// Called only at the projection boundary. Hidden wires leave the hit-test scene,
/// while the full graph retains authoring, tracing and table identities.
pub(super) fn refresh(state: &mut UsdCanvasSessionState) {
    let full = full_scene(state);
    let mut ports = Vec::new();
    for (id, node) in full.nodes() {
        let Some(data) = node.data.downcast_ref::<UsdPrimNodeData>() else {
            continue;
        };
        let Some(path) = node.origin.as_ref() else {
            continue;
        };
        for port in node
            .ports
            .iter()
            .filter(|p| !p.id.as_str().starts_with('~'))
        {
            ports.push(InterfacePort {
                endpoint: PortRef {
                    node: *id,
                    port: port.id.clone(),
                },
                key: data.view_key.clone(),
                path: path.clone(),
                name: port.id.as_str().into(),
                kind: port.kind.as_str().into(),
                usd_type: data.port_types.get(port.id.as_str()).cloned(),
            });
        }
    }
    ports.sort_by(|a, b| (&a.path, &a.kind, &a.name).cmp(&(&b.path, &b.kind, &b.name)));
    let rows: Vec<_> = full
        .edges()
        .filter_map(|(id, edge)| {
            let (source, source_port) = super::groups::endpoint(full, &edge.from).ok()?;
            let (target, target_port) = super::groups::endpoint(full, &edge.to).ok()?;
            let source_path = source.origin.as_ref()?.clone();
            let target_path = target.origin.as_ref()?.clone();
            let kind = edge.data.downcast_ref::<super::UsdWireData>()?.kind;
            Some(BindingRow {
                id: *id,
                from: edge.from.clone(),
                to: edge.to.clone(),
                source: format!("{source_path}.{source_port}"),
                target: format!("{target_path}.{target_port}"),
                local: projection::is_local_binding(full, edge),
                kind,
            })
        })
        .collect();
    let hidden: Vec<_> = rows
        .iter()
        .filter(|row| row.local && !state.workspace.show_local_wires)
        .filter(|row| {
            full.node(row.from.node)
                .and_then(|n| n.data.downcast_ref::<UsdPrimNodeData>())
                .is_some_and(|data| data.local_bindings_as_rows)
        })
        .map(|row| row.id)
        .collect();
    state.workspace.local_count = rows.iter().filter(|row| row.local).count();
    state.workspace.hidden_local_count = hidden.len();
    state.workspace.rows = rows;
    state.workspace.ports = ports;
    state.workspace.model_ports.clear();
    for (index, port) in state.workspace.ports.iter().enumerate() {
        state
            .workspace
            .model_ports
            .entry(port.path.clone())
            .or_default()
            .push(index);
    }
    state.workspace.peers.clear();
    for (index, row) in state.workspace.rows.iter().enumerate() {
        state
            .workspace
            .peers
            .entry((row.from.node, row.from.port.clone()))
            .or_default()
            .push(index);
        if row.from != row.to {
            state
                .workspace
                .peers
                .entry((row.to.node, row.to.port.clone()))
                .or_default()
                .push(index);
        }
    }
    state.workspace.filter_key = None;
    state.workspace.from = None;
    state.workspace.to = None;
    state.workspace.editing = None;
    state.workspace.replace = false;
    for id in hidden {
        state.canvas.scene.remove_edge(id);
        state
            .canvas
            .selection
            .remove(lunco_canvas::SelectItem::Edge(id));
    }
}

pub(super) fn controls(
    ui: &mut egui::Ui,
    ctx: &mut lunco_workbench_core::PanelCtx,
    state: &mut UsdCanvasSessionState,
) {
    let Some(host) = &state.view_document else {
        return;
    };
    let view_id = host.document().id().raw();
    ui.horizontal_wrapped(|ui| {
        for (mode, label) in [(Presentation::Topology,"Topology"),(Presentation::Interface,"Interface"),(Presentation::Bindings,"Bindings")] {
            if ui.selectable_label(state.workspace.mode == mode, label).clicked() {
                ctx.trigger(SetConnectionWorkspace { view_id, presentation: Some(mode.name().into()), ..default() });
            }
        }
        ui.separator();
        if ui.button(format!("{} local bindings", state.workspace.local_count)).on_hover_text("Inspect same-prim property mappings without routing loops around model cards").clicked() {
            state.workspace.local_only = true;
            ctx.trigger(SetConnectionWorkspace { view_id, presentation: Some("bindings".into()), ..default() });
        }
        let mut live = state.workspace.live_values;
        if ui.add_enabled(state.doc.is_none(), egui::Checkbox::new(&mut live, "Live values")).on_disabled_hover_text("Document previews are inert. Switch to Scene · Browse for runtime samples.").changed() {
            ctx.trigger(SetConnectionWorkspace { view_id, live_values: Some(live), ..default() });
        }
        if state.workspace.live_values && state.doc.is_none() {
            let mut animate = state.workspace.animate_changes;
            if ui.checkbox(&mut animate, "Highlight changes").on_hover_text("Briefly highlights observed source-value changes; does not represent physical flow or packet traffic").changed() {
                ctx.trigger(SetConnectionWorkspace { view_id, animate_changes: Some(animate), ..default() });
            }
            ui.label(if state.activity.running { "Running" } else { "Paused / held" });
        }
        if state.workspace.mode == Presentation::Topology {
            let mut show = state.workspace.show_local_wires;
            if ui.checkbox(&mut show, "Local wires").changed() { ctx.trigger(SetConnectionWorkspace { view_id, show_local_wires: Some(show), ..default() }); }
        }
    });
}

fn select(
    ui: &mut egui::Ui,
    ctx: &mut lunco_workbench_core::PanelCtx,
    state: &UsdCanvasSessionState,
    port: &InterfacePort,
) {
    let caption = super::visuals::endpoint_caption(&port.name, None);
    let caption = middle_elide(
        ui,
        &caption,
        ui.available_width() - ui.spacing().button_padding.x * 2.0,
    );
    if ui
        .add(
            egui::Button::selectable(
                state.canvas.selection.port() == Some(&port.endpoint),
                caption,
            )
            .truncate(),
        )
        .on_hover_text(format!("{}.{}", port.path, port.name))
        .clicked()
    {
        if let Some(host) = &state.view_document {
            ctx.trigger(navigation::SelectConnectionElement {
                view_id: host.document().id().raw(),
                key: port.key.clone(),
                port: Some(port.name.clone()),
                reveal: false,
            });
        }
    }
}

fn model_header(
    ui: &mut egui::Ui,
    ctx: &mut lunco_workbench_core::PanelCtx,
    state: &mut UsdCanvasSessionState,
) {
    let selected_path = super::inspection::selected_node(state)
        .and_then(|id| state.canvas.scene.node(id))
        .and_then(|node| node.origin.clone());
    if let Some(path) = selected_path {
        state.workspace.model = path;
    }
    if !state
        .workspace
        .model_ports
        .contains_key(&state.workspace.model)
    {
        state.workspace.model = if state
            .workspace
            .model_ports
            .contains_key(&state.diagram_root)
        {
            state.diagram_root.clone()
        } else {
            state
                .workspace
                .model_ports
                .keys()
                .next()
                .cloned()
                .unwrap_or_default()
        };
    }
    let before = state.workspace.model.clone();
    egui::ComboBox::from_id_salt("connection_interface_model")
        .selected_text(&state.workspace.model)
        .width(ui.available_width().min(500.0))
        .truncate()
        .show_ui(ui, |ui| {
            for path in state.workspace.model_ports.keys() {
                ui.selectable_value(&mut state.workspace.model, path.clone(), path);
            }
        });
    if before != state.workspace.model {
        state.canvas.selection.clear();
        if let Some(port) = state
            .workspace
            .model_ports
            .get(&state.workspace.model)
            .and_then(|indices| indices.first())
            .map(|index| &state.workspace.ports[*index])
        {
            if let Some(host) = &state.view_document {
                ctx.trigger(navigation::SelectConnectionElement {
                    view_id: host.document().id().raw(),
                    key: port.key.clone(),
                    port: None,
                    reveal: true,
                });
            }
        }
    }
    if let Some(node) = full_scene(state)
        .nodes()
        .map(|(_, node)| node)
        .find(|node| {
            node.origin.as_deref() == Some(state.workspace.model.as_str())
                && node
                    .data
                    .downcast_ref::<UsdPrimNodeData>()
                    .is_some_and(|data| !data.programs.is_empty())
        })
    {
        if let Some(data) = node.data.downcast_ref::<UsdPrimNodeData>() {
            let icon_size = (ui.available_height() * 0.25).clamp(32.0, 88.0);
            ui.horizontal(|ui| {
                if let Some(icon) = &data.modelica_icon {
                    let (rect, _) = ui.allocate_exact_size(
                        egui::vec2(icon_size, icon_size),
                        egui::Sense::hover(),
                    );
                    super::visuals::paint_modelica_icon(ui.painter(), rect, icon, &node.label);
                }
                ui.vertical(|ui| {
                    ui.strong(&node.label);
                    ui.horizontal_wrapped(|ui| {
                        for program in &data.programs {
                            if ui
                                .add_enabled(
                                    program.issue.is_none(),
                                    egui::Button::new(format!("Edit {} model", program.backend)),
                                )
                                .on_hover_text(&program.source)
                                .clicked()
                            {
                                if let Some(host) = &state.view_document {
                                    ctx.trigger(navigation::OpenConnectionNode {
                                        view_id: host.document().id().raw(),
                                        key: data.view_key.clone(),
                                        program_path: Some(program.path.clone()),
                                    });
                                }
                            }
                        }
                    });
                });
            });
        }
    }
}

fn port_column(
    ui: &mut egui::Ui,
    ctx: &mut lunco_workbench_core::PanelCtx,
    state: &mut UsdCanvasSessionState,
    kind: &str,
    title: &str,
) {
    if !title.is_empty() {
        ui.strong(title);
    }
    let ports: Vec<_> = state
        .workspace
        .model_ports
        .get(&state.workspace.model)
        .into_iter()
        .flatten()
        .copied()
        .filter(|index| state.workspace.ports[*index].kind == kind)
        .collect();
    if ports.is_empty() {
        ui.weak("None in this scope");
    }
    let height =
        ui.text_style_height(&egui::TextStyle::Body) * 6.0 + ui.spacing().item_spacing.y * 7.0;
    egui::ScrollArea::vertical()
        .id_salt(("interface_ports", kind))
        .max_height(ui.available_height().max(1.0))
        .show_rows(ui, height, ports.len(), |ui, range| {
            for index in &ports[range] {
                let row_top = ui.cursor().top();
                let port = state.workspace.ports[*index].clone();
                ui.separator();
                select(ui, ctx, state, &port);
                if let Some(usd_type) = &port.usd_type {
                    ui.weak(usd_type);
                }
                if state.workspace.live_values {
                    state
                        .activity
                        .requested
                        .insert(format!("{}.{}", port.path, port.name));
                }
                if let Some(sample) = state
                    .activity
                    .ports
                    .get(&format!("{}.{}", port.path, port.name))
                {
                    ui.label(sample.display());
                }
                let peers = state
                    .workspace
                    .peers
                    .get(&(port.endpoint.node, port.endpoint.port.clone()))
                    .map(Vec::as_slice)
                    .unwrap_or_default();
                if peers.is_empty() {
                    ui.weak(if kind == "input" {
                        "No authored connection"
                    } else {
                        "No authored consumer"
                    });
                }
                if let Some(index) = peers.first() {
                    let row = &state.workspace.rows[*index];
                    let label = if row.from == port.endpoint {
                        format!("→ {}", row.target)
                    } else {
                        format!("← {}", row.source)
                    };
                    endpoint_label(ui, &label);
                    ui.weak(format!(
                        "{} {}{}",
                        peers.len(),
                        if peers.len() == 1 {
                            "binding"
                        } else {
                            "bindings"
                        },
                        if row.local { " · same prim" } else { "" }
                    ));
                }
                let has_peers = !peers.is_empty();
                ui.horizontal_wrapped(|ui| {
                    if has_peers && ui.small_button("Inspect bindings").clicked() {
                        state.workspace.filter = format!("{}.{}", port.path, port.name);
                        state.workspace.local_only = false;
                        state.workspace.mode = Presentation::Bindings;
                    }
                    if state.doc.is_some() && ui.small_button("Connect…").clicked() {
                        state.workspace.editing = None;
                        state.workspace.replace = false;
                        state.workspace.from = Some(port.endpoint.clone());
                        state.workspace.to = None;
                        state.workspace.filter = format!("{}.{}", port.path, port.name);
                        state.workspace.local_only = false;
                        state.workspace.mode = Presentation::Bindings;
                        state.workspace.editor_open = true;
                    }
                });
                ui.add_space(
                    (height + ui.spacing().item_spacing.y - (ui.cursor().top() - row_top)).max(0.0),
                );
            }
        });
}

fn editor(
    ui: &mut egui::Ui,
    ctx: &mut lunco_workbench_core::PanelCtx,
    state: &mut UsdCanvasSessionState,
) {
    if state.doc.is_none() {
        ui.weak("Use Edit connections to author this source document.");
        return;
    }
    let caption = |value: &Option<PortRef>| {
        state
            .workspace
            .ports
            .iter()
            .find(|p| Some(&p.endpoint) == value.as_ref())
            .map(|p| format!("{} · {}", p.path, p.name))
            .unwrap_or_else(|| "Choose port…".into())
    };
    let from_caption = caption(&state.workspace.from);
    let to_caption = caption(&state.workspace.to);
    ui.columns(2, |columns| {
        columns[0].label("Port");
        egui::ComboBox::from_id_salt("binding_source")
            .selected_text(&from_caption)
            .width(columns[0].available_width())
            .truncate()
            .show_ui(&mut columns[0], |ui| {
                for port in &state.workspace.ports {
                    if ui
                        .selectable_value(
                            &mut state.workspace.from,
                            Some(port.endpoint.clone()),
                            format!("{} · {}", port.path, port.name),
                        )
                        .changed()
                    {
                        state.workspace.to = None;
                    }
                }
            })
            .response
            .on_hover_text(&from_caption);
        columns[1].label("Connect to");
        egui::ComboBox::from_id_salt("binding_target")
            .selected_text(&to_caption)
            .width(columns[1].available_width())
            .truncate()
            .show_ui(&mut columns[1], |ui| {
                for port in &state.workspace.ports {
                    if state.workspace.from.as_ref().is_some_and(|from| {
                        super::validate_connection(full_scene(state), from, &port.endpoint).is_ok()
                    }) {
                        ui.selectable_value(
                            &mut state.workspace.to,
                            Some(port.endpoint.clone()),
                            format!("{} · {}", port.path, port.name),
                        );
                    }
                }
            })
            .response
            .on_hover_text(&to_caption);
    });
    ui.horizontal_wrapped(|ui| {
        ui.checkbox(&mut state.workspace.replace, "Replace existing sources");
        let validation = state
            .workspace
            .from
            .as_ref()
            .zip(state.workspace.to.as_ref())
            .map(|(a, b)| super::validate_connection(full_scene(state), a, b));
        if ui
            .add_enabled(
                validation.as_ref().is_some_and(Result::is_ok),
                egui::Button::new(if state.workspace.editing.is_some() {
                    "Apply edit"
                } else {
                    "Connect"
                }),
            )
            .clicked()
        {
            if let (Some(from), Some(to), Some(host)) = (
                &state.workspace.from,
                &state.workspace.to,
                &state.view_document,
            ) {
                match super::authoring::BindingPair::capture(full_scene(state), from, to) {
                    Ok(pair) => ctx.trigger(super::authoring::ApplyConnectionBinding {
                        view_id: host.document().id().raw(),
                        parent_gen: state.generation,
                        from: pair.from,
                        to: pair.to,
                        previous: state.workspace.editing.clone(),
                        replace_sources: state.workspace.replace,
                    }),
                    Err(error) => state.last_error = Some(error),
                }
            }
        }
        if ui.small_button("Cancel").clicked() {
            state.workspace.editor_open = false;
            state.workspace.editing = None;
            state.workspace.replace = false;
            state.workspace.from = None;
            state.workspace.to = None;
        }
        if let Some(Err(error)) = validation {
            ui.colored_label(lunco_theme::active(ui.ctx()).tokens.warning, error);
        }
    });
}

/// Keep both the owner prefix and the distinguishing property suffix in view.
/// Full canonical identities remain available for copying in the tooltip.
fn endpoint_label(ui: &mut egui::Ui, identity: &str) {
    let label = middle_elide(ui, identity, ui.available_width());
    ui.add(egui::Label::new(label).truncate())
        .on_hover_ui(|ui| {
            ui.label(identity);
            if ui.small_button("Copy endpoint").clicked() {
                ui.ctx().copy_text(identity.into());
            }
        });
}

fn middle_elide(ui: &egui::Ui, identity: &str, available: f32) -> String {
    let font = egui::TextStyle::Body.resolve(ui.style());
    let full =
        ui.painter()
            .layout_no_wrap(identity.into(), font.clone(), ui.visuals().text_color());
    if full.size().x <= available {
        identity.to_owned()
    } else {
        ui.fonts_mut(|fonts| {
            let half = ((available - fonts.glyph_width(&font, '…')) * 0.5).max(0.0);
            let mut prefix = String::new();
            let mut suffix = Vec::new();
            let mut width = 0.0;
            for ch in identity.chars() {
                width += fonts.glyph_width(&font, ch);
                if width > half {
                    break;
                }
                prefix.push(ch);
            }
            width = 0.0;
            for ch in identity.chars().rev() {
                width += fonts.glyph_width(&font, ch);
                if width > half {
                    break;
                }
                suffix.push(ch);
            }
            format!("{prefix}…{}", suffix.into_iter().rev().collect::<String>())
        })
    }
}

fn bindings(
    ui: &mut egui::Ui,
    ctx: &mut lunco_workbench_core::PanelCtx,
    state: &mut UsdCanvasSessionState,
) {
    if state.doc.is_some() {
        if ui
            .toggle_value(
                &mut state.workspace.editor_open,
                if state.workspace.editing.is_some() {
                    "Edit binding"
                } else {
                    "Connect ports"
                },
            )
            .changed()
        {
            state.workspace.editing = None;
            state.workspace.replace = false;
            state.workspace.from = None;
            state.workspace.to = None;
        }
        if state.workspace.editor_open {
            editor(ui, ctx, state);
        }
    } else {
        ui.weak("Use Edit connections to author this source document.");
    }
    ui.separator();
    ui.horizontal_wrapped(|ui| {
        ui.add(
            lunco_workbench_widgets::text_editor::singleline(&mut state.workspace.filter)
                .hint_text("Find endpoint"),
        );
        ui.checkbox(&mut state.workspace.local_only, "Same prim only");
    });
    let needle = state.workspace.filter.to_lowercase();
    let filter_key = (needle.clone(), state.workspace.local_only);
    if state.workspace.filter_key.as_ref() != Some(&filter_key) {
        state.workspace.filtered_rows = state
            .workspace
            .rows
            .iter()
            .enumerate()
            .filter(|(_, row)| {
                (!state.workspace.local_only || row.local)
                    && (needle.is_empty()
                        || row.source.to_lowercase().contains(&needle)
                        || row.target.to_lowercase().contains(&needle))
            })
            .map(|(index, _)| index)
            .collect();
        state.workspace.filter_key = Some(filter_key);
    }
    let failures = state.workspace.filtered_rows.iter().any(|index| {
        state
            .activity
            .failure(&state.workspace.rows[*index])
            .is_some()
    });
    let extra_lines = usize::from(state.workspace.live_values) + usize::from(failures);
    let body = ui.text_style_height(&egui::TextStyle::Body);
    let row_height = ui.spacing().interact_size.y.max(body)
        + body * (2 + extra_lines) as f32
        + ui.spacing().item_spacing.y * (5 + extra_lines) as f32;
    let now = ctx
        .resource::<Time<Real>>()
        .map(|time| time.elapsed_secs_f64())
        .unwrap_or_default();
    egui::ScrollArea::vertical()
        .id_salt("connection_bindings")
        .show_rows(
            ui,
            row_height,
            state.workspace.filtered_rows.len(),
            |ui, range| {
                let indices = state.workspace.filtered_rows[range].to_vec();
                for index in indices {
                    let row_top = ui.cursor().top();
                    let row = state.workspace.rows[index].clone();
                    let row = &row;
                    ui.push_id(row.id, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.strong(if row.local {
                                "Same prim"
                            } else {
                                match row.kind {
                                    projection::WireKind::Dataflow => "Signal",
                                    projection::WireKind::Acausal => "Acausal",
                                    projection::WireKind::Joint => "Joint",
                                }
                            });
                            let status = state.activity.status(state.doc.is_some(), row);
                            ui.label(lunco_core::humanize_identifier(status));
                            if state.doc.is_some() && row.kind != projection::WireKind::Joint {
                                if ui.small_button("Edit").clicked() {
                                    match super::authoring::BindingPair::capture(
                                        full_scene(state),
                                        &row.from,
                                        &row.to,
                                    ) {
                                        Ok(pair) => {
                                            state.workspace.from = Some(row.from.clone());
                                            state.workspace.to = Some(row.to.clone());
                                            state.workspace.editing = Some(pair);
                                            state.workspace.replace = false;
                                            state.workspace.editor_open = true;
                                        }
                                        Err(error) => state.last_error = Some(error),
                                    }
                                }
                                if ui.small_button("Disconnect").clicked() {
                                    if let (Some(doc_id), Some(target), Some(sink)) = (
                                        state.doc,
                                        state.edit_target.as_ref(),
                                        super::edge_sink(full_scene(state), row.id),
                                    ) {
                                        let sinks = [(row.id, sink)].into();
                                        match super::build_ops(
                                            full_scene(state),
                                            &Default::default(),
                                            &sinks,
                                            &[SceneEvent::EdgeDeleted { id: row.id }],
                                            target,
                                        ) {
                                            Ok(ops) => {
                                                ctx.trigger(lunco_usd_core::commands::ApplyUsdOps {
                                                    doc_id,
                                                    parent_gen: Some(state.generation),
                                                    label: "Disconnect USD ports".into(),
                                                    ops,
                                                })
                                            }
                                            Err(error) => state.last_error = Some(error),
                                        }
                                    }
                                }
                            }
                        });
                        endpoint_label(ui, &row.source);
                        endpoint_label(ui, &row.target);
                        if let Some(failure) = state.activity.failure(row) {
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(failure)
                                        .color(lunco_theme::active(ui.ctx()).tokens.warning),
                                )
                                .truncate(),
                            )
                            .on_hover_text(failure);
                        }
                        if state.workspace.live_values {
                            state.activity.requested.insert(row.source.clone());
                            state.activity.requested.insert(row.target.clone());
                            let source = state
                                .activity
                                .ports
                                .get(&row.source)
                                .map(|sample| sample.display())
                                .unwrap_or_else(|| "No sample".into());
                            let target = state
                                .activity
                                .ports
                                .get(&row.target)
                                .map(|sample| sample.display())
                                .unwrap_or_else(|| "No sample".into());
                            let value = format!("{source} → {target}");
                            if state
                                .activity
                                .animating(row, now, state.workspace.animate_changes)
                            {
                                ui.colored_label(
                                    lunco_theme::active(ui.ctx()).tokens.port_output,
                                    value,
                                );
                            } else {
                                ui.label(value);
                            }
                        }
                        ui.separator();
                    });
                    ui.add_space(
                        (row_height + ui.spacing().item_spacing.y - (ui.cursor().top() - row_top))
                            .max(0.0),
                    );
                }
            },
        );
}

pub(super) fn render(
    ui: &mut egui::Ui,
    ctx: &mut lunco_workbench_core::PanelCtx,
    state: &mut UsdCanvasSessionState,
) {
    match state.workspace.mode {
        Presentation::Topology => {}
        Presentation::Interface => {
            model_header(ui, ctx, state);
            ui.separator();
            let physical = state
                .workspace
                .model_ports
                .get(&state.workspace.model)
                .into_iter()
                .flatten()
                .any(|index| state.workspace.ports[*index].kind == "acausal");
            let sections = if physical {
                vec![
                    InterfaceTab::Inputs,
                    InterfaceTab::Outputs,
                    InterfaceTab::Physical,
                ]
            } else {
                vec![InterfaceTab::Inputs, InterfaceTab::Outputs]
            };
            if !sections.contains(&state.workspace.interface_tab) {
                state.workspace.interface_tab = InterfaceTab::Inputs;
            }
            if ui.available_width() > ui.spacing().interact_size.x * 8.0 * sections.len() as f32 {
                ui.columns(sections.len(), |columns| {
                    for (column, section) in columns.iter_mut().zip(&sections) {
                        port_column(column, ctx, state, section.kind(), section.title());
                    }
                });
            } else {
                ui.horizontal_wrapped(|ui| {
                    for section in &sections {
                        let count = state
                            .workspace
                            .model_ports
                            .get(&state.workspace.model)
                            .into_iter()
                            .flatten()
                            .filter(|index| state.workspace.ports[**index].kind == section.kind())
                            .count();
                        ui.selectable_value(
                            &mut state.workspace.interface_tab,
                            *section,
                            format!("{} ({count})", section.title()),
                        );
                    }
                });
                let section = state.workspace.interface_tab;
                port_column(ui, ctx, state, section.kind(), "");
            }
        }
        Presentation::Bindings => bindings(ui, ctx, state),
    }
}
