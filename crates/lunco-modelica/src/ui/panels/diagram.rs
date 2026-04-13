//! Diagram panel — renders Modelica component graphs as interactive node diagrams.
//!
//! ## Entity Viewer Pattern
//!
//! This panel watches `WorkbenchState.selected_entity` and renders a
//! `ComponentGraph` (built from the Modelica AST) as an egui-snarl node graph.
//! It doesn't care if it's in a workbench, 3D overlay, or mission dashboard.
//!
//! ## Diagram Types
//!
//! - **Block Diagram**: Components as nodes, `connect()` as edges
//! - **Connection Diagram**: Like block diagram but connector instances expanded
//! - **Package Hierarchy**: Packages as subsystem nodes with containment edges
//!
//! ## Layout Convention
//!
//! Panel ID `modelica_diagram_preview` → auto-slots to **Center** (contains "preview").
//! Tabs with Code Editor by default. Users can drag to split vertically/horizontally.

use bevy::prelude::*;
use bevy_egui::egui;
use bevy_egui::egui::{Pos2, Ui};
use bevy_workbench::dock::WorkbenchPanel;
use egui_snarl::{InPin, InPinId, NodeId, OutPin, OutPinId, Snarl};
use egui_snarl::ui::{SnarlViewer, SnarlPin, PinInfo, SnarlStyle};
use lunco_core::diagram::{ComponentGraph, ComponentNode, NodeKind, PortDirection};

use crate::diagram::ModelicaComponentBuilder;
use crate::diagram::DiagramType;
use crate::ui::WorkbenchState;

/// Resource holding the current diagram state.
#[derive(Resource)]
pub struct DiagramState {
    /// The Snarl graph data. Lazily rebuilt when the source model changes.
    pub snarl: Option<Snarl<ModelicaNode>>,
    /// Hash of the source code that produced the current snarl.
    pub source_hash: u64,
    /// The type of diagram to render.
    pub diagram_type: DiagramType,
}

impl Default for DiagramState {
    fn default() -> Self {
        Self {
            snarl: None,
            source_hash: 0,
            diagram_type: DiagramType::BlockDiagram,
        }
    }
}

/// A node in the diagram Snarl.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum ModelicaNode {
    /// A component instance (e.g., `Resistor R1`).
    Component {
        name: String,
        type_name: String,
        input_ports: Vec<String>,
        output_ports: Vec<String>,
        qualified_name: String,
    },
    /// A connector instance (e.g., `Pin p` on R1).
    Connector {
        name: String,
        port_type: String,
    },
    /// A subsystem or package.
    Subsystem {
        name: String,
        qualified_name: String,
    },
    /// A Modelica class (model, block, function).
    Class {
        name: String,
        class_type: String,
        qualified_name: String,
    },
}

impl ModelicaNode {
    fn title(&self) -> &str {
        match self {
            ModelicaNode::Component { name, .. } => name,
            ModelicaNode::Connector { name, .. } => name,
            ModelicaNode::Subsystem { name, .. } => name,
            ModelicaNode::Class { name, .. } => name,
        }
    }

    fn subtitle(&self) -> String {
        match self {
            ModelicaNode::Component { type_name, .. } => format!("({})", type_name),
            ModelicaNode::Connector { port_type, .. } => format!("[{}]", port_type),
            ModelicaNode::Subsystem { .. } => String::new(),
            ModelicaNode::Class { class_type, .. } => format!("({})", class_type),
        }
    }

    fn input_label(&self, idx: usize) -> Option<&str> {
        match self {
            ModelicaNode::Component { input_ports, .. } => input_ports.get(idx).map(|s| s.as_str()),
            ModelicaNode::Connector { .. } => Some("in"),
            _ => None,
        }
    }

    fn output_label(&self, idx: usize) -> Option<&str> {
        match self {
            ModelicaNode::Component { output_ports, .. } => output_ports.get(idx).map(|s| s.as_str()),
            _ => None,
        }
    }
}

/// The viewer that renders `ModelicaNode` in egui-snarl.
pub struct ModelicaDiagramViewer;

impl SnarlViewer<ModelicaNode> for ModelicaDiagramViewer {
    fn title(&mut self, node: &ModelicaNode) -> String {
        node.title().to_string()
    }

    fn inputs(&mut self, node: &ModelicaNode) -> usize {
        match node {
            ModelicaNode::Component { input_ports, .. } => input_ports.len(),
            ModelicaNode::Connector { .. } => 1,
            _ => 0,
        }
    }

    fn outputs(&mut self, node: &ModelicaNode) -> usize {
        match node {
            ModelicaNode::Component { output_ports, .. } => output_ports.len(),
            _ => 0,
        }
    }

    fn show_input(
        &mut self,
        pin: &InPin,
        ui: &mut Ui,
        snarl: &mut Snarl<ModelicaNode>,
    ) -> impl SnarlPin + 'static {
        let label = snarl[pin.id.node]
            .input_label(pin.id.input)
            .unwrap_or("?");
        ui.label(format!("◀ {}", label));
        PinInfo::circle()
    }

    fn show_output(
        &mut self,
        pin: &OutPin,
        ui: &mut Ui,
        snarl: &mut Snarl<ModelicaNode>,
    ) -> impl SnarlPin + 'static {
        let label = snarl[pin.id.node]
            .output_label(pin.id.output)
            .unwrap_or("?");
        ui.label(format!("{} ▶", label));
        PinInfo::circle()
    }

    fn has_body(&mut self, node: &ModelicaNode) -> bool {
        !node.subtitle().is_empty()
    }

    fn show_body(
        &mut self,
        _node: NodeId,
        _inputs: &[InPin],
        _outputs: &[OutPin],
        ui: &mut Ui,
        snarl: &mut Snarl<ModelicaNode>,
    ) {
        // Body shows the qualified name
        if let Some(node) = snarl.nodes().next() {
            let subtitle = node.subtitle();
            if !subtitle.is_empty() {
                ui.label(
                    egui::RichText::new(&subtitle)
                        .size(9.0)
                        .color(egui::Color32::GRAY),
                );
            }
        }
    }

    fn has_footer(&mut self, node: &ModelicaNode) -> bool {
        matches!(node, ModelicaNode::Component { qualified_name, .. }
            | ModelicaNode::Subsystem { qualified_name, .. }
            | ModelicaNode::Class { qualified_name, .. } if !qualified_name.is_empty())
    }

    fn show_footer(
        &mut self,
        node: NodeId,
        _inputs: &[InPin],
        _outputs: &[OutPin],
        ui: &mut Ui,
        snarl: &mut Snarl<ModelicaNode>,
    ) {
        if let Some(n) = snarl.get_node(node) {
            let qname = match n {
                ModelicaNode::Component { qualified_name, .. }
                | ModelicaNode::Subsystem { qualified_name, .. }
                | ModelicaNode::Class { qualified_name, .. } => qualified_name,
                _ => return,
            };
            ui.label(
                egui::RichText::new(qname)
                    .size(8.0)
                    .color(egui::Color32::DARK_GRAY),
            );
        }
    }
}

/// Diagram panel — renders the Modelica component graph.
pub struct DiagramPanel;

impl WorkbenchPanel for DiagramPanel {
    fn id(&self) -> &str { "modelica_diagram_preview" }
    fn title(&self) -> String { "🔗 Diagram".into() }
    fn closable(&self) -> bool { true }
    fn default_visible(&self) -> bool { true }
    fn needs_world(&self) -> bool { true }

    fn ui(&mut self, _ui: &mut egui::Ui) {}

    fn ui_world(&mut self, ui: &mut egui::Ui, world: &mut World) {
        // Read state and check if rebuild needed
        let (editor_buffer, current_hash) = {
            let state = world.get_resource::<WorkbenchState>();
            match state {
                Some(s) => (Some(s.editor_buffer.clone()), crate::ast_extract::hash_content(&s.editor_buffer)),
                None => (None, 0),
            }
        };

        let (needs_rebuild, diagram_type) = world
            .get_resource::<DiagramState>()
            .map(|s| {
                let rebuild = editor_buffer.is_some() && current_hash != s.source_hash;
                (rebuild, s.diagram_type)
            })
            .unwrap_or((true, DiagramType::BlockDiagram));

        // Rebuild if needed
        if needs_rebuild {
            if let Some(source) = editor_buffer {
                if let Some(builder) = ModelicaComponentBuilder::from_source(&source) {
                    let graph = builder.diagram_type(diagram_type).build();
                    let new_snarl = build_snarl_from_graph(&graph);
                    if let Some(mut state) = world.get_resource_mut::<DiagramState>() {
                        state.snarl = Some(new_snarl);
                        state.source_hash = current_hash;
                    }
                }
            }
        }

        // Render toolbar
        ui.horizontal(|ui| {
            // Diagram type selector
            if let Some(mut state) = world.get_resource_mut::<DiagramState>() {
                let prev = state.diagram_type;
                egui::ComboBox::from_label("")
                    .selected_text(format!("{:?}", prev))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut state.diagram_type, DiagramType::BlockDiagram, "Block Diagram");
                        ui.selectable_value(&mut state.diagram_type, DiagramType::ConnectionDiagram, "Connection Diagram");
                        ui.selectable_value(&mut state.diagram_type, DiagramType::PackageHierarchy, "Package Hierarchy");
                    });
                if prev != state.diagram_type {
                    state.source_hash = 0; // Force rebuild
                }
            }

            ui.separator();

            if ui.button("↻ Rebuild").clicked() {
                if let Some(mut state) = world.get_resource_mut::<DiagramState>() {
                    state.source_hash = 0;
                }
            }
            ui.separator();
            let node_count = world
                .get_resource::<DiagramState>()
                .and_then(|s| s.snarl.as_ref().map(|sn| sn.nodes().count()))
                .unwrap_or(0);
            if node_count > 0 {
                ui.label(format!("{} nodes", node_count));
            } else {
                ui.label("No diagram — compile a model first");
            }
        });
        ui.separator();

        // Render the Snarl graph
        let has_snarl = world
            .get_resource::<DiagramState>()
            .and_then(|s| s.snarl.as_ref().map(|_| true))
            .unwrap_or(false);

        if has_snarl {
            let mut state = world.resource_mut::<DiagramState>();
            if let Some(snarl) = &mut state.snarl {
                let mut viewer = ModelicaDiagramViewer;
                let style = SnarlStyle::default();
                snarl.show(&mut viewer, &style, "diagram", ui);
            }
        } else {
            ui.centered_and_justified(|ui| {
                ui.label(
                    egui::RichText::new("Load and compile a .mo file to see the diagram")
                        .color(egui::Color32::GRAY),
                );
            });
        }
    }
}

/// Convert a `ComponentGraph` into a `Snarl<ModelicaNode>`.
fn build_snarl_from_graph(graph: &ComponentGraph) -> Snarl<ModelicaNode> {
    let mut snarl = Snarl::default();
    let mut id_map = std::collections::HashMap::new();

    // Add all nodes with positions
    for (i, node) in graph.nodes.iter().enumerate() {
        let snarl_node = component_node_to_snarl(node);
        // Arrange in a grid layout
        let cols = 4;
        let row = i / cols;
        let col = i % cols;
        let pos = Pos2::new(col as f32 * 200.0, row as f32 * 150.0);
        let id = snarl.insert_node(pos, snarl_node);
        id_map.insert(node.id, id);
    }

    // Add all connections
    for edge in &graph.edges {
        if let (Some(&src), Some(&tgt)) = (id_map.get(&edge.source), id_map.get(&edge.target)) {
            snarl.connect(
                OutPinId { node: src, output: edge.source_port },
                InPinId { node: tgt, input: edge.target_port },
            );
        }
    }

    snarl
}

/// Convert a `ComponentNode` to a `ModelicaNode` for snarl rendering.
fn component_node_to_snarl(node: &ComponentNode) -> ModelicaNode {
    let input_ports: Vec<String> = node.ports.iter()
        .filter(|p| matches!(p.direction, PortDirection::Input | PortDirection::Bidir))
        .map(|p| p.name.clone())
        .collect();

    let output_ports: Vec<String> = node.ports.iter()
        .filter(|p| matches!(p.direction, PortDirection::Output | PortDirection::Bidir))
        .map(|p| p.name.clone())
        .collect();

    match node.kind {
        NodeKind::Component => {
            ModelicaNode::Component {
                name: node.label.clone(),
                type_name: node.meta.get("type_name").cloned().unwrap_or_default(),
                input_ports,
                output_ports,
                qualified_name: node.qualified_name.clone(),
            }
        }
        NodeKind::Connector => {
            ModelicaNode::Connector {
                name: node.label.clone(),
                port_type: node.ports.first()
                    .and_then(|p| p.port_type.clone())
                    .unwrap_or_default(),
            }
        }
        NodeKind::Subsystem => {
            ModelicaNode::Subsystem {
                name: node.label.clone(),
                qualified_name: node.qualified_name.clone(),
            }
        }
        NodeKind::Class => {
            ModelicaNode::Class {
                name: node.label.clone(),
                class_type: node.meta.get("class_type").cloned().unwrap_or("model".to_string()),
                qualified_name: node.qualified_name.clone(),
            }
        }
        _ => ModelicaNode::Subsystem {
            name: node.label.clone(),
            qualified_name: node.qualified_name.clone(),
        },
    }
}
