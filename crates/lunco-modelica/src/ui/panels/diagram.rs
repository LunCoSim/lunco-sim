//! Diagram panel — clean visual canvas for building models.
//!
//! ## Architecture (Dymola-style)
//!
//! The diagram panel owns a **VisualModel** — a clean, editable Modelica model
//! that the user builds by clicking components in the MSL palette.
//!
//! - Click component in palette → appended to the VisualModel
//! - Diagram renders from the VisualModel's current state
//! - COMPILE & RUN → generates a temp `.mo` file from the VisualModel
//!
//! This is separate from the "parsed model view" which shows existing `.mo` files.

use bevy::prelude::*;
use bevy_egui::egui;
use bevy_workbench::dock::WorkbenchPanel;
use egui_snarl::{InPin, InPinId, OutPin, OutPinId, Snarl};
use egui_snarl::ui::{SnarlViewer, SnarlPin, PinInfo, SnarlStyle};
use std::collections::HashMap;
use std::sync::Arc;

use crate::visual_diagram::{
    DiagramNodeId, VisualDiagram, MSLComponentDef,
    generate_modelica_source, msl_component_library,
};
use crate::ui::WorkbenchState;
use crate::{ModelicaModel, ModelicaChannels, ModelicaCommand};

// ---------------------------------------------------------------------------
// Diagram State
// ---------------------------------------------------------------------------

/// Resource holding the visual diagram being built on the canvas.
#[derive(Resource)]
pub struct DiagramState {
    /// The visual model being built.
    pub diagram: VisualDiagram,
    /// The egui-snarl state for the canvas.
    pub snarl: Snarl<DiagramNode>,
    /// Generated source from last compile.
    pub last_source: Option<String>,
    /// Compile status message.
    pub compile_status: Option<String>,
    /// Whether last compile succeeded.
    pub compile_ok: bool,
    /// Counter for model names.
    pub model_counter: u32,
    /// Counter for component placement positions.
    pub placement_counter: u32,
}

impl DiagramState {
    /// Add a component to both the diagram data and the snarl UI.
    pub fn add_component(&mut self, def: MSLComponentDef, pos: egui::Pos2) {
        let node_id = self.diagram.add_node(def.clone(), pos);
        let ports: Vec<String> = def.ports.iter().map(|p| p.name.clone()).collect();
        let connector_types: Vec<String> = def.ports.iter().map(|p| p.connector_type.clone()).collect();

        let snarl_node = DiagramNode::Component {
            id: node_id,
            instance_name: self.diagram.get_node(node_id).unwrap().instance_name.clone(),
            type_name: def.name,
            ports,
            connector_types,
        };
        self.snarl.insert_node(pos, snarl_node);
    }

    /// Rebuild the snarl from the current diagram.
    pub fn rebuild_snarl(&mut self) {
        self.snarl = build_snarl(&self.diagram);
    }
}

impl Default for DiagramState {
    fn default() -> Self {
        Self {
            diagram: VisualDiagram::default(),
            snarl: Snarl::default(),
            last_source: None,
            compile_status: None,
            compile_ok: false,
            model_counter: 0,
            placement_counter: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Snarl Node Type
// ---------------------------------------------------------------------------

/// A visual node on the diagram canvas.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum DiagramNode {
    Component {
        id: DiagramNodeId,
        instance_name: String,
        type_name: String,
        ports: Vec<String>,
        connector_types: Vec<String>,
    },
}

impl DiagramNode {
    fn title(&self) -> &str {
        match self {
            DiagramNode::Component { instance_name, .. } => instance_name,
        }
    }

    fn subtitle(&self) -> String {
        match self {
            DiagramNode::Component { type_name, .. } => type_name.clone(),
        }
    }

    fn port_count(&self) -> usize {
        match self {
            DiagramNode::Component { ports, .. } => ports.len(),
        }
    }

    fn port_label(&self, idx: usize) -> Option<&str> {
        match self {
            DiagramNode::Component { ports, .. } => ports.get(idx).map(|s| s.as_str()),
        }
    }

    fn connector_type(&self, idx: usize) -> Option<&str> {
        match self {
            DiagramNode::Component { connector_types, .. } => connector_types.get(idx).map(|s| s.as_str()),
        }
    }
}

// ---------------------------------------------------------------------------
// Snarl Viewer
// ---------------------------------------------------------------------------

pub struct DiagramViewer;

impl SnarlViewer<DiagramNode> for DiagramViewer {
    fn title(&mut self, node: &DiagramNode) -> String {
        node.title().to_string()
    }

    fn inputs(&mut self, node: &DiagramNode) -> usize {
        node.port_count()
    }

    fn outputs(&mut self, node: &DiagramNode) -> usize {
        node.port_count()
    }

    fn show_input(
        &mut self,
        _pin: &InPin,
        ui: &mut egui::Ui,
        _snarl: &mut Snarl<DiagramNode>,
    ) -> impl SnarlPin + 'static {
        // Tiny colored dot — no label text
        let size = 6.0;
        let resp = ui.allocate_response(egui::vec2(size, size), egui::Sense::click_and_drag());
        ui.painter().circle_filled(resp.rect.center(), size / 2.0, egui::Color32::from_rgb(80, 150, 255));
        PinInfo::circle()
    }

    fn show_output(
        &mut self,
        _pin: &OutPin,
        ui: &mut egui::Ui,
        _snarl: &mut Snarl<DiagramNode>,
    ) -> impl SnarlPin + 'static {
        let size = 6.0;
        let resp = ui.allocate_response(egui::vec2(size, size), egui::Sense::click_and_drag());
        ui.painter().circle_filled(resp.rect.center(), size / 2.0, egui::Color32::from_rgb(80, 150, 255));
        PinInfo::circle()
    }

    fn show_header(
        &mut self,
        node_id: egui_snarl::NodeId,
        _inputs: &[InPin],
        _outputs: &[OutPin],
        ui: &mut egui::Ui,
        snarl: &mut Snarl<DiagramNode>,
    ) {
        let node = &snarl[node_id];
        ui.label(egui::RichText::new(node.title()).size(11.0).strong());
        let sub = node.subtitle();
        if !sub.is_empty() {
            ui.label(egui::RichText::new(&sub).size(8.0).color(egui::Color32::GRAY));
        }
    }

    fn has_body(&mut self, _node: &DiagramNode) -> bool {
        false
    }

    fn has_footer(&mut self, _node: &DiagramNode) -> bool {
        false
    }

    fn node_frame(
        &mut self,
        default: egui::Frame,
        _node_id: egui_snarl::NodeId,
        _inputs: &[InPin],
        _outputs: &[OutPin],
        _snarl: &Snarl<DiagramNode>,
    ) -> egui::Frame {
        // Minimal frame — thin border, no header background
        let mut frame = default;
        frame.fill = egui::Color32::from_rgb(40, 40, 45);
        frame.stroke = egui::Stroke::new(1.0, egui::Color32::from_rgb(80, 80, 80));
        frame.inner_margin = egui::Margin::same(4);
        frame
    }
}

// ---------------------------------------------------------------------------
// Quick Place (for empty diagram hints)
// ---------------------------------------------------------------------------

fn auto_place_component(world: &mut World, component_name: &str) {
    let lib = crate::visual_diagram::msl_component_library();
    let comp = lib.iter().find(|c| c.name == component_name).cloned();
    if let Some(def) = comp {
        if let Some(mut ds) = world.get_resource_mut::<DiagramState>() {
            ds.placement_counter += 1;
            let x = 100.0 + (ds.placement_counter % 3) as f32 * 200.0;
            let y = 80.0 + (ds.placement_counter / 3) as f32 * 160.0;
            ds.add_component(def, egui::Pos2::new(x, y));
        }
    }
}

/// Parse Modelica source and build a `VisualDiagram` from component
/// instantiations and `connect()` equations.
///
/// Returns `None` if the model has no component instantiations
/// (e.g., equation-based models like Battery.mo, SpringMass.mo).
fn import_model_to_diagram(source: &str) -> Option<VisualDiagram> {
    use crate::diagram::ModelicaComponentBuilder;

    // Try to build a component graph from the source
    let builder = ModelicaComponentBuilder::from_source(source)?;
    let graph = builder.build();

    // If no components found, this is an equation-based model
    if graph.node_count() == 0 {
        return None;
    }

    // Convert ComponentGraph → VisualDiagram
    let mut diagram = VisualDiagram::default();

    // Build a lookup from component type name → MSLComponentDef
    let msl_lib = msl_component_library();
    let msl_lookup: HashMap<&str, &MSLComponentDef> = msl_lib.iter()
        .map(|c| (c.name.as_str(), c))
        .collect();

    // Place nodes in a grid layout
    let node_spacing_x = 200.0;
    let node_spacing_y = 150.0;
    let cols = 3;

    for (idx, node) in graph.nodes.iter().enumerate() {
        if node.qualified_name.is_empty() {
            continue;
        }

        // Extract short name from qualified_name (e.g., "RC_Circuit.R1" → "R1")
        let short_name = node.qualified_name.split('.').last().unwrap_or(&node.qualified_name);

        // Try to find matching MSL component definition
        let type_name = node.meta.get("type_name").map(|s| s.as_str()).unwrap_or("");
        let component_def = msl_lookup.get(type_name)
            .or_else(|| msl_lookup.get(short_name))
            .cloned();

        if let Some(def) = component_def {
            let row = idx / cols;
            let col = idx % cols;
            let pos = egui::Pos2::new(col as f32 * node_spacing_x, row as f32 * node_spacing_y);

            let node_id = diagram.add_node(def.clone(), pos);

            // Override the auto-generated instance name with the one from the source
            if let Some(diagram_node) = diagram.get_node_mut(node_id) {
                diagram_node.instance_name = short_name.to_string();
            }
        }
    }

    // Add edges from graph connections
    for edge in &graph.edges {
        let src_node = &graph.nodes[edge.source.0 as usize];
        let tgt_node = &graph.nodes[edge.target.0 as usize];

        let src_short = src_node.qualified_name.split('.').last().unwrap_or("");
        let tgt_short = tgt_node.qualified_name.split('.').last().unwrap_or("");

        // Find matching diagram nodes
        let src_diagram_id = diagram.nodes.iter()
            .find(|n| n.instance_name == src_short)
            .map(|n| n.id);
        let tgt_diagram_id = diagram.nodes.iter()
            .find(|n| n.instance_name == tgt_short)
            .map(|n| n.id);

        if let (Some(src_id), Some(tgt_id)) = (src_diagram_id, tgt_diagram_id) {
            // Port names from graph node ports
            let src_port = src_node.ports.get(edge.source_port).map(|p| p.name.clone()).unwrap_or_default();
            let tgt_port = tgt_node.ports.get(edge.target_port).map(|p| p.name.clone()).unwrap_or_default();
            diagram.add_edge(src_id, src_port, tgt_id, tgt_port);
        }
    }

    if diagram.nodes.is_empty() {
        None
    } else {
        Some(diagram)
    }
}

// ---------------------------------------------------------------------------
// Diagram ↔ Snarl Sync
// ---------------------------------------------------------------------------

fn build_snarl(diagram: &VisualDiagram) -> Snarl<DiagramNode> {
    let mut snarl = Snarl::default();
    let mut id_map: HashMap<DiagramNodeId, egui_snarl::NodeId> = HashMap::new();

    for node in &diagram.nodes {
        let ports: Vec<String> = node.component_def.ports.iter().map(|p| p.name.clone()).collect();
        let connector_types: Vec<String> = node.component_def.ports.iter().map(|p| p.connector_type.clone()).collect();
        let snarl_node = DiagramNode::Component {
            id: node.id,
            instance_name: node.instance_name.clone(),
            type_name: node.component_def.name.clone(),
            ports,
            connector_types,
        };
        let pos = egui::Pos2::new(node.position.x, node.position.y);
        let sid = snarl.insert_node(pos, snarl_node);
        id_map.insert(node.id, sid);
    }

    for edge in &diagram.edges {
        if let (Some(&src_sid), Some(&tgt_sid)) = (id_map.get(&edge.source_node), id_map.get(&edge.target_node)) {
            let src_node = diagram.get_node(edge.source_node);
            let tgt_node = diagram.get_node(edge.target_node);
            if let (Some(src), Some(tgt)) = (src_node, tgt_node) {
                let src_idx = src.component_def.ports.iter().position(|p| p.name == edge.source_port).unwrap_or(0);
                let tgt_idx = tgt.component_def.ports.iter().position(|p| p.name == edge.target_port).unwrap_or(0);
                snarl.connect(
                    OutPinId { node: src_sid, output: src_idx },
                    InPinId { node: tgt_sid, input: tgt_idx },
                );
            }
        }
    }

    snarl
}

fn sync_connections(snarl: &Snarl<DiagramNode>, diagram: &mut VisualDiagram) {
    diagram.edges.clear();
    for (out_pin, in_pin) in snarl.wires() {
        let src_sid = out_pin.node;
        let tgt_sid = in_pin.node;
        let src_pidx = out_pin.output;
        let tgt_pidx = in_pin.input;

        let DiagramNode::Component { id: src_id, ports: src_ports, .. } = &snarl[src_sid];
        let DiagramNode::Component { id: tgt_id, ports: tgt_ports, .. } = &snarl[tgt_sid];
        let src_port = src_ports.get(src_pidx).cloned().unwrap_or_default();
        let tgt_port = tgt_ports.get(tgt_pidx).cloned().unwrap_or_default();
        diagram.add_edge(*src_id, src_port, *tgt_id, tgt_port);
    }
}

// ---------------------------------------------------------------------------
// Diagram Panel
// ---------------------------------------------------------------------------

/// Diagram canvas panel — shows the visual model being built.
pub struct DiagramPanel;

impl WorkbenchPanel for DiagramPanel {
    fn id(&self) -> &str { "modelica_diagram_preview" }
    fn title(&self) -> String { "🔗 Diagram".into() }
    fn closable(&self) -> bool { true }
    fn default_visible(&self) -> bool { true }
    fn needs_world(&self) -> bool { true }
    fn ui(&mut self, _ui: &mut egui::Ui) {}

    fn ui_world(&mut self, ui: &mut egui::Ui, world: &mut World) {
        if world.get_resource::<DiagramState>().is_none() {
            world.insert_resource(DiagramState::default());
        }

        // ── Check if open_model changed → import diagram ──
        {
            let dirty = world.get_resource::<WorkbenchState>()
                .map(|s| s.diagram_dirty)
                .unwrap_or(false);
            if dirty {
                if let Some(state) = world.get_resource::<WorkbenchState>() {
                    if let Some(ref model) = state.open_model {
                        // Parse source → build VisualDiagram
                        if let Some(diagram) = import_model_to_diagram(&model.source) {
                            if let Some(mut ds) = world.get_resource_mut::<DiagramState>() {
                                ds.diagram = diagram;
                                ds.rebuild_snarl();
                            }
                        }
                    }
                }
                if let Some(mut state) = world.get_resource_mut::<WorkbenchState>() {
                    state.diagram_dirty = false;
                }
            }
        }

        // ── Breadcrumb bar ──
        let (has_model, display_name, is_read_only, has_back) = {
            let state = world.get_resource::<WorkbenchState>();
            state.map(|s| {
                s.open_model.as_ref().map(|m| {
                    (true, m.display_name.clone(), m.read_only, !s.navigation_stack.is_empty())
                }).unwrap_or((false, String::new(), false, false))
            }).unwrap_or((false, String::new(), false, false))
        };

        if has_model {
            ui.horizontal(|ui| {
                // Read-only badge
                if is_read_only {
                    ui.colored_label(egui::Color32::from_rgb(200, 150, 50), "👁 Read-only");
                } else {
                    ui.colored_label(egui::Color32::GREEN, "✏️ Editing");
                }
                ui.label(format!("• {}", display_name));

                // Back button
                if has_back {
                    if ui.small_button("← Back").clicked() {
                        if let Some(mut state) = world.get_resource_mut::<WorkbenchState>() {
                            if let Some(prev_path) = state.navigation_stack.pop() {
                                // Navigate back: find the model by path and re-open it
                                // For now, just clear the open_model (breadcrumb nav is future work)
                                state.open_model = None;
                                state.diagram_dirty = true;
                                let _ = prev_path; // used when we implement full nav
                            }
                        }
                    }
                }
            });
            ui.separator();
        }

        // ── Toolbar ──
        let compile_clicked = ui.button("🚀 COMPILE & RUN").clicked();
        ui.separator();

        {
            let s = world.get_resource::<DiagramState>();
            if let Some(st) = s {
                ui.label(format!("{} components, {} wires", st.diagram.nodes.len(), st.diagram.edges.len()));
                if let Some(status) = &st.compile_status {
                    ui.separator();
                    let color = if st.compile_ok { egui::Color32::GREEN } else { egui::Color32::LIGHT_RED };
                    ui.colored_label(color, status);
                }
            }
        }
        ui.separator();

        // Clear button
        if ui.small_button("🗑 Clear").clicked() {
            if let Some(mut s) = world.get_resource_mut::<DiagramState>() {
                s.diagram = VisualDiagram::default();
                s.snarl = Snarl::default();
                s.compile_status = None;
            }
        }
        ui.separator();

        // ── Canvas ──
        let has_nodes = {
            world.get_resource::<DiagramState>()
                .map(|s| !s.diagram.nodes.is_empty())
                .unwrap_or(false)
        };

        if !has_nodes {
            // Show workflow hints when diagram is empty
            ui.vertical_centered(|ui| {
                ui.add_space(40.0);
                ui.heading("Build Your Model");
                ui.add_space(16.0);

                ui.label("1. Open \"📦 MSL Library\" panel");
                ui.label("   (tab in center, next to this Diagram tab)");
                ui.label("   Click a component to place it on this canvas");
                ui.add_space(8.0);
                ui.label("2. Drag wires between the small port dots");
                ui.add_space(8.0);
                ui.label("3. Click 🚀 COMPILE & RUN to simulate");

                ui.add_space(20.0);
                ui.separator();
                ui.label("Quick start — click to place:");

                if ui.button("⚡ Resistor").clicked() {
                    auto_place_component(world, "Resistor");
                }
                if ui.button("🔋 Constant Voltage").clicked() {
                    auto_place_component(world, "ConstantVoltage");
                }
                if ui.button("⏚ Ground").clicked() {
                    auto_place_component(world, "Ground");
                }
                if ui.button("|| Capacitor").clicked() {
                    auto_place_component(world, "Capacitor");
                }
            });
        }

        // Allocate space for snarl
        ui.allocate_space(egui::vec2(500.0, 400.0));
        let rect = ui.max_rect();
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
        
        // Use persisted snarl
        if let Some(mut ds) = world.get_resource_mut::<DiagramState>() {
            let mut viewer = DiagramViewer;
            ds.snarl.show(&mut viewer, &SnarlStyle::default(), "diagram", &mut child);

            // Split the borrow explicitly to avoid borrow checker error
            let DiagramState { snarl, diagram, .. } = &mut *ds;
            sync_connections(snarl, diagram);
        }

        // ── Compile ──
        if compile_clicked {
            // Extract data first
            let (model_counter, _diagram, source, temp_path) = {
                let Some(s) = world.get_resource::<DiagramState>() else { return };
                let mc = s.model_counter + 1;
                let model_name = format!("VisualModel{}", mc);
                let _diagram = &s.diagram; let source = generate_modelica_source(&s.diagram, &model_name);
                let temp_dir = std::env::temp_dir().join("luncosim");
                let _ = std::fs::create_dir_all(&temp_dir);
                let temp_path = temp_dir.join(format!("{}.mo", model_name));
                (mc, s.diagram.clone(), source.clone(), temp_path)
            };

            // Write file
            if let Err(e) = std::fs::write(&temp_path, &source) {
                if let Some(mut s) = world.get_resource_mut::<DiagramState>() {
                    s.compile_status = Some(format!("Write error: {}", e));
                    s.compile_ok = false;
                }
                return;
            }

            // Spawn entity
            let session_id = model_counter as u64;
            let model_name = format!("VisualModel{}", model_counter);
            let entity = world.spawn((
                Name::new(model_name.clone()),
                ModelicaModel {
                    model_path: temp_path,
                    model_name: model_name.clone(),
                    original_source: source.clone().into(),
                    current_time: 0.0,
                    last_step_time: 0.0,
                    session_id,
                    paused: false,
                    parameters: HashMap::new(),
                    inputs: HashMap::new(),
                    variables: HashMap::new(),
                    is_stepping: true,
                },
            )).id();

            // Send command
            if let Some(channels) = world.get_resource::<ModelicaChannels>() {
                let _ = channels.tx.send(ModelicaCommand::Compile {
                    entity,
                    session_id,
                    model_name,
                    source,
                });
            }
            if let Some(mut s) = world.get_resource_mut::<DiagramState>() {
                s.model_counter = model_counter;
                s.compile_status = Some("Compiling...".into());
            }
        }
    }
}
