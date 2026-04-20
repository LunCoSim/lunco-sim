//! Visual diagram editor — drag-and-drop component composition.
//!
//! ## Architecture
//!
//! Users build models visually by dragging components from a palette onto a canvas,
//! then connecting ports with edges. The diagram is compiled into a composite
//! Modelica model, written to a temp file, and executed.
//!
//! ```text
//! ┌──────────────┐     ┌─────────────────┐     ┌──────────────────┐
//! │ MSL Palette  │──▶  │ Visual Canvas   │──▶  │ Code Generator   │
//! │ (components) │     │ (nodes + edges) │     │ (.mo temp file)  │
//! └──────────────┘     └─────────────────┘     └────────┬─────────┘
//!                                                       │
//!                                              ┌────────▼─────────┐
//!                                              │ Compiler + Run   │
//!                                              └──────────────────┘
//! ```
//!
//! ## Generated Modelica Example
//!
//! A visual diagram with a voltage source, resistor, capacitor, and ground
//! connected together generates:
//!
//! ```modelica
//! model VisualDiagram
//!   import Modelica.Electrical.Analog.Basic.Resistor;
//!   import Modelica.Electrical.Analog.Basic.Capacitor;
//!   import Modelica.Electrical.Analog.Sources.ConstantVoltage;
//!   import Modelica.Electrical.Analog.Basic.Ground;
//!
//!   ConstantVoltage V1(V=10) annotation(...);
//!   Resistor R1(R=100) annotation(...);
//!   Capacitor C1(C=0.001) annotation(...);
//!   Ground GND annotation(...);
//!
//! equation
//!   connect(V1.p, R1.p);
//!   connect(R1.n, C1.p);
//!   connect(C1.n, GND.p);
//!   connect(V1.n, GND.p);
//! end VisualDiagram;
//! ```

use bevy::prelude::*;
use bevy_egui::egui::Pos2;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Diagram Data Model
// ---------------------------------------------------------------------------

/// Unique identifier for a diagram node instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DiagramNodeId(Uuid);

impl DiagramNodeId {
    pub fn new() -> Self { Self(Uuid::new_v4()) }
}

impl Default for DiagramNodeId {
    fn default() -> Self { Self::new() }
}

/// A port definition for an MSL component.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortDef {
    /// Port name (e.g., "p", "n", "flange_a").
    pub name: String,
    /// Connector type (e.g., "Pin", "Flange_a").
    pub connector_type: String,
    /// MSL path of the connector type.
    pub msl_path: String,
    /// Whether this port is a flow connector.
    pub is_flow: bool,
    /// Port position in Modelica diagram coordinates (-100..100).
    /// Extracted from the `annotation(Placement(...))` on the connector declaration.
    /// x < 0 = left side, x > 0 = right side, y > 0 = top, y < 0 = bottom.
    /// (0, 0) means the position is unknown (no annotation + no causality hint).
    #[serde(default)]
    pub x: f32,
    #[serde(default)]
    pub y: f32,
}

/// A parameter definition for an MSL component.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParamDef {
    /// Parameter name (e.g., "R", "C", "V").
    pub name: String,
    /// Parameter type (e.g., "Real", "Integer").
    pub param_type: String,
    /// Default value.
    pub default: String,
    /// Unit (e.g., "Ohm", "F", "V").
    pub unit: Option<String>,
}

/// An MSL component available in the palette.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MSLComponentDef {
    /// Short name (e.g., "Resistor").
    pub name: String,
    /// Full MSL path (e.g., "Modelica.Electrical.Analog.Basic.Resistor").
    pub msl_path: String,
    /// Category for grouping (e.g., "Electrical/Analog/Basic").
    pub category: String,
    /// Icon/display name.
    pub display_name: String,
    /// Detailed description.
    pub description: Option<String>,
    /// Schematic text (e.g. "cosh").
    pub icon_text: Option<String>,
    /// Path to SVG icon asset, relative to assets root.
    pub icon_asset: Option<String>,
    /// Ports defined by this component.
    pub ports: Vec<PortDef>,
    /// Parameters that can be configured.
    pub parameters: Vec<ParamDef>,
    /// Decoded `Icon(graphics={...})` annotation for the class. When
    /// `Some`, the canvas renders these primitives via
    /// [`crate::icon_paint::paint_graphics`] instead of falling back to
    /// the SVG icon path. Populated by the diagram projector for
    /// user-defined classes parsed from the open document; MSL
    /// palette entries leave it `None` and continue to use their
    /// pre-rasterised SVG icons.
    #[serde(default)]
    pub icon_graphics: Option<crate::annotations::Icon>,
}

/// A node instance placed on the visual canvas.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagramNode {
    pub id: DiagramNodeId,
    /// Instance name (e.g., "R1", "C1").
    pub instance_name: String,
    /// Component definition reference.
    pub component_def: MSLComponentDef,
    /// Parameter values (name → value).
    pub parameter_values: HashMap<String, String>,
    /// Canvas position.
    pub position: Pos2,
    /// Whether the node is selected.
    pub selected: bool,
}

/// A connection between two component ports.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagramEdge {
    pub id: Uuid,
    pub source_node: DiagramNodeId,
    pub source_port: String,
    pub target_node: DiagramNodeId,
    pub target_port: String,
}

/// The complete visual diagram.
#[derive(Resource, Default, Serialize, Deserialize, Clone)]
pub struct VisualDiagram {
    pub nodes: Vec<DiagramNode>,
    pub edges: Vec<DiagramEdge>,
    /// Counter for auto-naming instances (R1, R2, C1, ...).
    pub name_counters: HashMap<String, u32>,
}

impl VisualDiagram {
    /// Generate a unique instance name for a component type.
    pub fn next_instance_name(&mut self, component_name: &str) -> String {
        let counter = self.name_counters.entry(component_name.to_string()).or_insert(0);
        *counter += 1;
        // Use first letter as prefix: Resistor → R1, Capacitor → C1
        let prefix = component_name.chars().next().unwrap_or('X').to_uppercase().to_string();
        format!("{}{}", prefix, counter)
    }

    /// Add a node to the diagram.
    pub fn add_node(&mut self, def: MSLComponentDef, position: Pos2) -> DiagramNodeId {
        self.add_node_with_id(DiagramNodeId::new(), def, position)
    }

    /// Add a node with a specific ID.
    pub fn add_node_with_id(&mut self, id: DiagramNodeId, def: MSLComponentDef, position: Pos2) -> DiagramNodeId {
        let instance_name = self.next_instance_name(&def.name);
        let mut parameter_values = HashMap::new();
        for param in &def.parameters {
            parameter_values.insert(param.name.clone(), param.default.clone());
        }
        self.nodes.push(DiagramNode {
            id,
            instance_name,
            component_def: def,
            parameter_values,
            position,
            selected: false,
        });
        id
    }

    /// Remove a node and its connected edges.
    pub fn remove_node(&mut self, id: DiagramNodeId) {
        self.nodes.retain(|n| n.id != id);
        self.edges.retain(|e| e.source_node != id && e.target_node != id);
    }

    /// Add an edge between two ports.
    pub fn add_edge(
        &mut self,
        source_node: DiagramNodeId,
        source_port: String,
        target_node: DiagramNodeId,
        target_port: String,
    ) {
        // Check for duplicate
        let exists = self.edges.iter().any(|e| {
            (e.source_node == source_node && e.source_port == source_port
                && e.target_node == target_node && e.target_port == target_port)
            || (e.source_node == target_node && e.source_port == target_port
                && e.target_node == source_node && e.target_port == source_port)
        });
        if !exists {
            self.edges.push(DiagramEdge {
                id: Uuid::new_v4(),
                source_node,
                source_port,
                target_node,
                target_port,
            });
        }
    }

    /// Remove an edge.
    pub fn remove_edge(&mut self, id: Uuid) {
        self.edges.retain(|e| e.id != id);
    }

    /// Find a node by ID.
    pub fn get_node(&self, id: DiagramNodeId) -> Option<&DiagramNode> {
        self.nodes.iter().find(|n| n.id == id)
    }

    /// Find a node by ID (mutable).
    pub fn get_node_mut(&mut self, id: DiagramNodeId) -> Option<&mut DiagramNode> {
        self.nodes.iter_mut().find(|n| n.id == id)
    }
}

// ---------------------------------------------------------------------------
// MSL Component Library
// ---------------------------------------------------------------------------

use std::sync::OnceLock;

static MSL_LIBRARY: OnceLock<Vec<MSLComponentDef>> = OnceLock::new();

/// Returns the MSL component definitions available in the palette.
/// Loaded from the preprocessed MSL index.
pub fn msl_component_library() -> &'static [MSLComponentDef] {
    MSL_LIBRARY.get_or_init(|| {
        let index_path = lunco_assets::msl_dir().join("msl_index.json");
        if let Ok(content) = std::fs::read_to_string(index_path) {
            if let Ok(lib) = serde_json::from_str(&content) {
                return lib;
            }
        }
        Vec::new()
    })
}

/// Get unique categories from the MSL library.
pub fn msl_categories() -> Vec<String> {
    let mut cats: Vec<String> = msl_component_library()
        .iter()
        .map(|c| c.category.clone())
        .collect();
    cats.sort();
    cats.dedup();
    cats
}

/// Get components in a category.
pub fn msl_components_in_category(category: &str) -> Vec<MSLComponentDef> {
    msl_component_library()
        .iter()
        .filter(|c| c.category == category)
        .cloned()
        .collect()
}

/// Lookup a component definition by its MSL path.
pub fn msl_component_by_path(path: &str) -> Option<MSLComponentDef> {
    msl_component_library()
        .iter()
        .find(|c| c.msl_path == path)
        .cloned()
}

// ---------------------------------------------------------------------------
// Code Generator — Diagram → Modelica Source
// ---------------------------------------------------------------------------

/// Generate Modelica source code from a visual diagram.
///
/// Returns the complete `.mo` file content as a string.
pub fn generate_modelica_source(diagram: &VisualDiagram, model_name: &str) -> String {
    let mut source = String::new();

    // Model header
    source.push_str(&format!("model {}\n", model_name));
    source.push_str("  // Auto-generated from visual diagram\n\n");

    // Imports — collect unique MSL paths needed
    let mut imports: Vec<String> = diagram
        .nodes
        .iter()
        .map(|n| n.component_def.msl_path.clone())
        .collect();
    imports.sort();
    imports.dedup();

    for import_path in &imports {
        source.push_str(&format!("  import {};\n", import_path));
    }
    if !imports.is_empty() {
        source.push('\n');
    }

    // Component declarations
    for node in &diagram.nodes {
        let short_name = node.component_def.name.clone();
        let params: Vec<String> = node.parameter_values
            .iter()
            .map(|(k, v)| format!("{}={}", k, v))
            .collect();
        let param_str = if params.is_empty() {
            String::new()
        } else {
            format!("({})", params.join(", "))
        };
        
        let x = node.position.x;
        let y = node.position.y;
        // Note: Modelica coordinate space is typically +Y up, Snarl is +Y down.
        // We invert Y here, mapping 1:1 pixels to coordinate space.
        let annotation = format!(
            " annotation(Placement(transformation(extent={{{{ {}, {} }}, {{ {}, {} }}}})))",
            x - 20.0, -y - 20.0, x + 20.0, -y + 20.0
        );

        source.push_str(&format!(
            "  {} {}{}{};\n",
            short_name, node.instance_name, param_str, annotation
        ));
    }
    source.push_str("\nequation\n");

    // Connection equations
    for edge in &diagram.edges {
        let src_node = diagram.get_node(edge.source_node);
        let tgt_node = diagram.get_node(edge.target_node);
        if let (Some(src), Some(tgt)) = (src_node, tgt_node) {
            source.push_str(&format!(
                "  connect({}.{}, {}.{});\n",
                src.instance_name, edge.source_port,
                tgt.instance_name, edge.target_port
            ));
        }
    }

    // If no connections, add a dummy equation to make it valid
    if diagram.edges.is_empty() && !diagram.nodes.is_empty() {
        source.push_str("  // No connections yet\n");
    }

    source.push_str(&format!("end {};\n", model_name));
    source
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_rc_circuit() {
        let mut diagram = VisualDiagram::default();

        let lib = msl_component_library();
        let v1_def = lib.iter().find(|c| c.name == "ConstantVoltage").unwrap().clone();
        let r1_def = lib.iter().find(|c| c.name == "Resistor").unwrap().clone();
        let c1_def = lib.iter().find(|c| c.name == "Capacitor").unwrap().clone();
        let gnd_def = lib.iter().find(|c| c.name == "Ground").unwrap().clone();

        let v1 = diagram.add_node(v1_def, Pos2::new(0.0, 0.0));
        let r1 = diagram.add_node(r1_def, Pos2::new(200.0, 0.0));
        let c1 = diagram.add_node(c1_def, Pos2::new(400.0, 0.0));
        let gnd = diagram.add_node(gnd_def, Pos2::new(200.0, 200.0));
        diagram.get_node_mut(gnd).unwrap().instance_name = "GND".into();

        diagram.add_edge(v1, "p".into(), r1, "p".into());
        diagram.add_edge(r1, "n".into(), c1, "p".into());
        diagram.add_edge(c1, "n".into(), gnd, "p".into());
        diagram.add_edge(v1, "n".into(), gnd, "p".into());

        let source = generate_modelica_source(&diagram, "TestRC");
        assert!(source.contains("model TestRC"));
        assert!(source.contains("connect(C1.n, GND.p)"));
        assert!(source.contains("Resistor R1(R=100)"));
        assert!(source.contains("Capacitor C1(C=0.001)"));
        assert!(source.contains("end TestRC"));
    }

    #[test]
    fn test_msl_library_not_empty() {
        let lib = msl_component_library();
        assert!(!lib.is_empty());
        assert!(lib.iter().any(|c| c.name == "Resistor"));
        assert!(lib.iter().any(|c| c.name == "Ground"));
    }
}
