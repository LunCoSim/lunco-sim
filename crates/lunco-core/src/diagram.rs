//! Unified diagram data model — the canonical graph representation.
//!
//! This module provides pure-Rust graph types that represent connected components
//! with typed ports. No Bevy, no ECS, no rendering — just data that any domain
//! (Modelica AST, ECS wires, FSW architecture) can build and share.
//!
//! ## Architecture
//!
//! ```text
//!   ComponentGraph (canonical, this module)
//!       ▲
//!       │ built by
//!   ┌───┴────┬─────────────┐
//!   │        │             │
//!   │   ┌────┴────┐   ┌────┴────┐
//!   │   │Modelica │   │  ECS    │
//!   │   │GraphBld │   │WireGraph│
//!   │   └─────────┘   └─────────┘
//!   │
//!   ▼ (converted to)
//!   egui-snarl Snarl (rendered in lunco-ui)
//! ```
//!
//! ## Ontology Alignment (specs/ontology.md, Article IX)
//!
//! This module implements the graph-level concepts from the Engineering Ontology:
//!
//! | Ontology Concept | ComponentGraph Type | Notes |
//! |-----------------|---------------------|-------|
//! | **Port**        | `ComponentPort`       | Named, typed interface point. Maps 1:1 to SysML Proxy Port. |
//! | **Wire**        | `EdgeKind::Wire`    | Signal/power link between ports. Maps to SysML connection. |
//! | **Connection**  | `EdgeKind::Connect` | Modelica `connect()` equation. Maps to Modelica connector. |
//! | **Component**   | `NodeKind::Component` | A functional unit (resistor, motor, sensor). Maps to SysML part. |
//! | **Subsystem**   | `NodeKind::Subsystem` | A containing unit (package, assembly). Maps to Space System. |
//! | **Space System**| `NodeKind::Class`   | Top-level container. Maps to Space System / Vehicle. |
//! | **Link**        | `NodeKind::Signal`  | Structural/data link. Maps to URDF link. |
//! | **Attribute**   | `ComponentNode::meta` | Measurable property stored as key-value metadata. |
//!
//! ## Usage
//!
//! ```ignore
//! use lunco_core::diagram::{ComponentGraph, ComponentBuilder, NodeKind};
//!
//! let graph = ComponentGraph::titled("RC Circuit")
//!     .node("R1", NodeKind::Component)
//!         .port(ComponentPort::output("p").with_type("Pin"))
//!         .port(ComponentPort::output("n").with_type("Pin"))
//!     .end()
//!     .node("C1", NodeKind::Component)
//!         .port(ComponentPort::input("p").with_type("Pin"))
//!         .port(ComponentPort::input("n").with_type("Pin"))
//!     .end()
//!     .connect("R1", "p", "C1", "p", EdgeKind::Connect)
//!     .build();
//! ```

use std::collections::HashMap;

/// Unique identifier for a node within a [`ComponentGraph`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(pub u32);

/// Unique identifier for a port index within a node.
pub type PortIdx = usize;

/// A named port on a diagram node.
///
/// Ports are the connection points on nodes. Each port has a direction
/// (input or output) and an optional type string (e.g., "Real", "Pin",
/// "Flange_a") for type-aware visualization and validation.
#[derive(Debug, Clone, PartialEq)]
pub struct ComponentPort {
    /// Port name (e.g., "p", "n", "u", "y", "voltage").
    pub name: String,
    /// Port direction.
    pub direction: PortDirection,
    /// Optional type tag (e.g., "Real", "Pin", "Flange_a", "i16").
    pub port_type: Option<String>,
    /// Optional description for tooltip display.
    pub description: Option<String>,
}

impl ComponentPort {
    /// Create an input port.
    pub fn input(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            direction: PortDirection::Input,
            port_type: None,
            description: None,
        }
    }

    /// Create an output port.
    pub fn output(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            direction: PortDirection::Output,
            port_type: None,
            description: None,
        }
    }

    /// Set the port type tag.
    pub fn with_type(mut self, ty: impl Into<String>) -> Self {
        self.port_type = Some(ty.into());
        self
    }

    /// Set the description text.
    pub fn with_description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }
}

/// Port direction — input (receives data) or output (sends data).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PortDirection {
    /// Input port (receives connections).
    #[default]
    Input,
    /// Output port (sends connections).
    Output,
    /// Bidirectional port (e.g., Modelica acausal connectors).
    Bidir,
}

/// A node in the diagram.
///
/// Nodes represent components, subsystems, ports, or packages depending on
/// the [`NodeKind`]. Each node has a set of connection ports.
#[derive(Debug, Clone, PartialEq)]
pub struct ComponentNode {
    /// Unique ID within the graph.
    pub id: NodeId,
    /// What kind of thing this node represents.
    pub kind: NodeKind,
    /// Display label (short name shown on the node).
    pub label: String,
    /// Fully qualified name (for navigation/debugging).
    pub qualified_name: String,
    /// Connection points on this node.
    pub ports: Vec<ComponentPort>,
    /// Optional metadata (entity ID, source file line, etc.).
    pub meta: HashMap<String, String>,
}

impl ComponentNode {
    /// Find a port by name, returning its index.
    pub fn port_index(&self, name: &str) -> Option<PortIdx> {
        self.ports.iter().position(|p| p.name == name)
    }
}

/// What a diagram node represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NodeKind {
    /// A component instance (e.g., `Resistor R1`).
    #[default]
    Component,
    /// A connector instance (e.g., `Pin p`).
    Connector,
    /// A subsystem / package (containment node).
    Subsystem,
    /// A FSW digital port.
    DigitalPort,
    /// A physical port.
    PhysicalPort,
    /// A wire/signal node (for signal flow diagrams).
    Signal,
    /// A Modelica class (model, block, function).
    Class,
}

/// An edge between two nodes.
///
/// Edges represent connections between ports on nodes. The edge kind
/// distinguishes between different connection types (Modelica equations,
/// ECS wires, inheritance, containment, etc.).
#[derive(Debug, Clone, PartialEq)]
pub struct ComponentEdge {
    /// Source node ID.
    pub source: NodeId,
    /// Index of the output port on the source node.
    pub source_port: PortIdx,
    /// Target node ID.
    pub target: NodeId,
    /// Index of the input port on the target node.
    pub target_port: PortIdx,
    /// What this edge represents (connection, wire, inheritance, etc.).
    pub kind: EdgeKind,
    /// Optional label shown on the edge (e.g., equation text, signal name).
    pub label: Option<String>,
    /// Optional metadata.
    pub meta: HashMap<String, String>,
}

/// The semantic meaning of an edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EdgeKind {
    /// Modelica `connect(a, b)` equation.
    #[default]
    Connect,
    /// ECS `Wire` component (digital ↔ physical bridge).
    Wire,
    /// FSW signal path.
    Signal,
    /// `extends` inheritance.
    Extends,
    /// `import` reference.
    Import,
    /// Package containment.
    Contains,
    /// General association (for non-causal diagrams).
    Association,
}

/// A complete diagram graph — nodes with typed ports, connected by edges.
///
/// This is the core data structure for all diagram visualization. It is
/// domain-agnostic: Modelica AST, ECS queries, and FSW configurations can
/// all produce `ComponentGraph` instances that share a single viewer.
#[derive(Debug, Clone, Default)]
pub struct ComponentGraph {
    /// All nodes in the graph.
    pub nodes: Vec<ComponentNode>,
    /// All edges in the graph.
    pub edges: Vec<ComponentEdge>,
    /// Optional title for the diagram.
    pub title: Option<String>,
    /// Metadata about the graph source.
    pub meta: HashMap<String, String>,
}

impl ComponentGraph {
    /// Create a new empty graph.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a new graph with a title.
    pub fn titled(title: impl Into<String>) -> Self {
        Self {
            title: Some(title.into()),
            ..Default::default()
        }
    }

    /// Add a node to the graph and return its ID.
    pub fn add_node(
        &mut self,
        kind: NodeKind,
        label: impl Into<String>,
        ports: Vec<ComponentPort>,
    ) -> NodeId {
        let label = label.into();
        let id = NodeId(self.nodes.len() as u32);
        self.nodes.push(ComponentNode {
            id,
            kind,
            label: label.clone(),
            qualified_name: label,
            ports,
            meta: HashMap::new(),
        });
        id
    }

    /// Add a node with a fully qualified name.
    pub fn add_node_named(
        &mut self,
        kind: NodeKind,
        label: impl Into<String>,
        qualified_name: impl Into<String>,
        ports: Vec<ComponentPort>,
    ) -> NodeId {
        let label = label.into();
        let qualified_name = qualified_name.into();
        let id = NodeId(self.nodes.len() as u32);
        self.nodes.push(ComponentNode {
            id,
            kind,
            label,
            qualified_name,
            ports,
            meta: HashMap::new(),
        });
        id
    }

    /// Connect two nodes by port index.
    pub fn connect(
        &mut self,
        source: NodeId,
        source_port: PortIdx,
        target: NodeId,
        target_port: PortIdx,
        kind: EdgeKind,
    ) {
        self.edges.push(ComponentEdge {
            source,
            source_port,
            target,
            target_port,
            kind,
            label: None,
            meta: HashMap::new(),
        });
    }

    /// Connect two nodes with a labeled edge.
    pub fn connect_labeled(
        &mut self,
        source: NodeId,
        source_port: PortIdx,
        target: NodeId,
        target_port: PortIdx,
        kind: EdgeKind,
        label: impl Into<String>,
    ) {
        self.edges.push(ComponentEdge {
            source,
            source_port,
            target,
            target_port,
            kind,
            label: Some(label.into()),
            meta: HashMap::new(),
        });
    }

    /// Find a node by ID.
    pub fn get_node(&self, id: NodeId) -> Option<&ComponentNode> {
        self.nodes.get(id.0 as usize)
    }

    /// Find a node by qualified name.
    pub fn find_node(&self, qualified_name: &str) -> Option<&ComponentNode> {
        self.nodes.iter().find(|n| n.qualified_name == qualified_name)
    }

    /// Get all edges connected to a node.
    pub fn edges_for_node(&self, id: NodeId) -> impl Iterator<Item = &ComponentEdge> {
        self.edges
            .iter()
            .filter(move |e| e.source == id || e.target == id)
    }

    /// Node count.
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Edge count.
    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }

    /// Clear all nodes and edges.
    pub fn clear(&mut self) {
        self.nodes.clear();
        self.edges.clear();
    }
}

/// Fluent builder for constructing a [`ComponentGraph`].
///
/// Provides a more ergonomic API for building graphs programmatically:
///
/// ```ignore
/// let graph = ComponentBuilder::new("RC Circuit")
///     .node("R1", NodeKind::Component)
///         .port(ComponentPort::output("p").with_type("Pin"))
///         .port(ComponentPort::output("n").with_type("Pin"))
///     .node("C1", NodeKind::Component)
///         .port(ComponentPort::input("p").with_type("Pin"))
///         .port(ComponentPort::input("n").with_type("Pin"))
///     .connect("R1", "p", "C1", "p", EdgeKind::Connect)
///     .build();
/// ```
pub struct ComponentBuilder {
    graph: ComponentGraph,
    current_node: Option<NodeId>,
    name_to_id: HashMap<String, NodeId>,
}

impl ComponentBuilder {
    /// Create a new builder with a title.
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            graph: ComponentGraph::titled(title),
            current_node: None,
            name_to_id: HashMap::new(),
        }
    }

    /// Start defining a new node by its short name.
    pub fn node(mut self, name: impl Into<String>, kind: NodeKind) -> ComponentNodeBuilder {
        let name = name.into();
        let id = NodeId(self.graph.nodes.len() as u32);
        self.name_to_id.insert(name.clone(), id);
        self.current_node = Some(id);
        self.graph.nodes.push(ComponentNode {
            id,
            kind,
            label: name.clone(),
            qualified_name: name,
            ports: Vec::new(),
            meta: HashMap::new(),
        });
        ComponentNodeBuilder { builder: self }
    }

    /// Start defining a node with a separate display label.
    pub fn node_labeled(
        mut self,
        name: impl Into<String>,
        label: impl Into<String>,
        kind: NodeKind,
    ) -> ComponentNodeBuilder {
        let name = name.into();
        let label = label.into();
        let id = NodeId(self.graph.nodes.len() as u32);
        self.name_to_id.insert(name.clone(), id);
        self.current_node = Some(id);
        self.graph.nodes.push(ComponentNode {
            id,
            kind,
            label,
            qualified_name: name.clone(),
            ports: Vec::new(),
            meta: HashMap::new(),
        });
        ComponentNodeBuilder { builder: self }
    }

    /// Connect two nodes by name and port name.
    ///
    /// Looks up nodes by their registration name and ports by port name.
    /// Panics if the node or port is not found (programmer error).
    pub fn connect(
        mut self,
        source_name: &str,
        source_port: &str,
        target_name: &str,
        target_port: &str,
        kind: EdgeKind,
    ) -> Self {
        let source = self.name_to_id[source_name];
        let target = self.name_to_id[target_name];
        let source_node = self.graph.get_node(source).unwrap();
        let target_node = self.graph.get_node(target).unwrap();
        let sp = source_node.port_index(source_port).unwrap_or_else(|| {
            panic!(
                "Node '{}' has no output port '{}'",
                source_node.label, source_port
            )
        });
        let tp = target_node.port_index(target_port).unwrap_or_else(|| {
            panic!(
                "Node '{}' has no input port '{}'",
                target_node.label, target_port
            )
        });
        self.graph.connect(source, sp, target, tp, kind);
        self
    }

    /// Connect two nodes with a label on the edge.
    pub fn connect_labeled(
        mut self,
        source_name: &str,
        source_port: &str,
        target_name: &str,
        target_port: &str,
        kind: EdgeKind,
        label: impl Into<String>,
    ) -> Self {
        let source = self.name_to_id[source_name];
        let target = self.name_to_id[target_name];
        let source_node = self.graph.get_node(source).unwrap();
        let target_node = self.graph.get_node(target).unwrap();
        let sp = source_node.port_index(source_port).unwrap_or_else(|| {
            panic!(
                "Node '{}' has no output port '{}'",
                source_node.label, source_port
            )
        });
        let tp = target_node.port_index(target_port).unwrap_or_else(|| {
            panic!(
                "Node '{}' has no input port '{}'",
                target_node.label, target_port
            )
        });
        self.graph.connect_labeled(source, sp, target, tp, kind, label);
        self
    }

    /// Add metadata to the graph.
    pub fn meta(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.graph.meta.insert(key.into(), value.into());
        self
    }

    /// Finalize and return the built graph.
    pub fn build(self) -> ComponentGraph {
        self.graph
    }
}

/// Fluent builder for adding ports to a node.
pub struct ComponentNodeBuilder {
    builder: ComponentBuilder,
}

impl ComponentNodeBuilder {
    /// Add a port to the current node.
    pub fn port(mut self, port: ComponentPort) -> Self {
        let id = self.builder.current_node.unwrap();
        let node = self.builder.graph.nodes.get_mut(id.0 as usize).unwrap();
        node.ports.push(port);
        self
    }

    /// Add multiple ports at once.
    pub fn ports(mut self, ports: impl IntoIterator<Item = ComponentPort>) -> Self {
        let id = self.builder.current_node.unwrap();
        let node = self.builder.graph.nodes.get_mut(id.0 as usize).unwrap();
        node.ports.extend(ports);
        self
    }

    /// Add metadata to the current node.
    pub fn meta(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        let id = self.builder.current_node.unwrap();
        let node = self.builder.graph.nodes.get_mut(id.0 as usize).unwrap();
        node.meta.insert(key.into(), value.into());
        self
    }

    /// Finish this node and return to the builder.
    pub fn end(self) -> ComponentBuilder {
        self.builder
    }
}

// ---------------------------------------------------------------------------
// ECS wire-to-graph conversion (lunco-core native domain)
// ---------------------------------------------------------------------------

/// Trait for wire-like data that can be converted to a diagram.
///
/// This allows both real `Wire` components and mock/test data to produce
/// diagram graphs without depending on Bevy.
pub trait WireLikeSource {
    /// Source entity ID.
    fn source(&self) -> u64;
    /// Target entity ID.
    fn target(&self) -> u64;
    /// Scale factor applied during signal conversion.
    fn scale(&self) -> f32;
    /// Whether this wire connects digital ports (true) or physical ports (false).
    fn is_digital(&self) -> bool;
}

impl ComponentGraph {
    /// Convert ECS wire/port configuration into a [`ComponentGraph`].
    ///
    /// This function takes the implicit graph formed by `DigitalPort`, `PhysicalPort`,
    /// and `Wire` components and materializes it as an explicit `ComponentGraph` for
    /// visualization.
    ///
    /// Note: This function operates on raw entity/port data to remain Bevy-independent.
    /// The Bevy-specific conversion lives in the ECS query system that calls this.
    pub fn from_wires<W>(wires: impl IntoIterator<Item = W>) -> Self
    where
        W: WireLikeSource,
    {
        // Collect into Vec so we can iterate multiple times
        let wires: Vec<W> = wires.into_iter().collect();
        let mut builder = ComponentBuilder::new("Signal Flow");
        let mut entity_names: HashMap<u64, String> = HashMap::new();
        let mut entity_is_digital: HashMap<u64, bool> = HashMap::new();

        // First pass: collect all entities and their digital/physical nature
        for wire in &wires {
            entity_names.entry(wire.source()).or_insert_with(|| format!("entity_{}", wire.source()));
            entity_names.entry(wire.target()).or_insert_with(|| format!("entity_{}", wire.target()));
            entity_is_digital.entry(wire.source()).or_insert(wire.is_digital());
            entity_is_digital.entry(wire.target()).or_insert(wire.is_digital());
        }

        // Second pass: create all nodes with both input and output ports
        let mut nodes_created: HashMap<u64, String> = HashMap::new();
        for (&entity_id, name) in &entity_names {
            let digital = entity_is_digital[&entity_id];
            let type_tag = if digital { "i16" } else { "f32" };
            let kind = if digital { NodeKind::DigitalPort } else { NodeKind::PhysicalPort };
            builder = builder
                .node(name, kind)
                    .port(ComponentPort::output("out").with_type(type_tag))
                    .port(ComponentPort::input("in").with_type(type_tag))
                .end();
            nodes_created.insert(entity_id, name.clone());
        }

        // Third pass: create edges
        for wire in &wires {
            let src_name = &nodes_created[&wire.source()];
            let tgt_name = &nodes_created[&wire.target()];

            let scale_label = if (wire.scale() - 1.0).abs() > 1e-6 {
                Some(format!("×{}", wire.scale()))
            } else {
                None
            };

            builder = match scale_label {
                Some(label) => builder.connect_labeled(src_name, "out", tgt_name, "in", EdgeKind::Wire, label),
                None => builder.connect(src_name, "out", tgt_name, "in", EdgeKind::Wire),
            };
        }

        builder.build()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builder_simple() {
        let graph = ComponentBuilder::new("Test")
            .node("R1", NodeKind::Component)
                .port(ComponentPort::output("p").with_type("Pin"))
                .port(ComponentPort::output("n").with_type("Pin"))
            .end()
            .node("C1", NodeKind::Component)
                .port(ComponentPort::input("p").with_type("Pin"))
                .port(ComponentPort::input("n").with_type("Pin"))
            .end()
            .connect("R1", "p", "C1", "p", EdgeKind::Connect)
            .build();

        assert_eq!(graph.node_count(), 2);
        assert_eq!(graph.edge_count(), 1);

        let r1 = graph.find_node("R1").unwrap();
        assert_eq!(r1.ports.len(), 2);
        assert_eq!(r1.ports[0].name, "p");
        assert_eq!(r1.ports[0].port_type.as_deref(), Some("Pin"));

        let edge = &graph.edges[0];
        assert_eq!(edge.kind, EdgeKind::Connect);
        assert_eq!(edge.source_port, 0);
        assert_eq!(edge.target_port, 0);
    }

    #[test]
    fn test_direct_api() {
        let mut graph = ComponentGraph::new();
        let a = graph.add_node(NodeKind::Component, "Sensor", vec![
            ComponentPort::output("voltage").with_type("Real"),
        ]);
        let b = graph.add_node(NodeKind::Component, "ADC", vec![
            ComponentPort::input("analog").with_type("Real"),
        ]);
        graph.connect(a, 0, b, 0, EdgeKind::Signal);

        assert_eq!(graph.node_count(), 2);
        assert_eq!(graph.edge_count(), 1);
        assert_eq!(graph.get_node(a).unwrap().label, "Sensor");
        assert_eq!(graph.get_node(b).unwrap().label, "ADC");
    }

    #[test]
    fn test_wire_graph_conversion() {
        struct MockWire {
            src: u64,
            tgt: u64,
            scl: f32,
            digital: bool,
        }
        impl WireLikeSource for MockWire {
            fn source(&self) -> u64 { self.src }
            fn target(&self) -> u64 { self.tgt }
            fn scale(&self) -> f32 { self.scl }
            fn is_digital(&self) -> bool { self.digital }
        }

        let wires = vec![
            MockWire { src: 1, tgt: 2, scl: 1.0, digital: true },
            MockWire { src: 2, tgt: 3, scl: 2.5, digital: false },
        ];

        let graph = ComponentGraph::from_wires(wires);
        assert_eq!(graph.node_count(), 3);
        assert_eq!(graph.edge_count(), 2);
        assert_eq!(graph.edges[1].label.as_deref(), Some("×2.5"));
    }

    #[test]
    fn test_graph_clear() {
        let mut graph = ComponentBuilder::new("Test")
            .node("A", NodeKind::Component)
                .port(ComponentPort::output("x"))
            .end()
            .node("B", NodeKind::Component)
                .port(ComponentPort::input("y"))
            .end()
            .connect("A", "x", "B", "y", EdgeKind::Connect)
            .build();

        assert_eq!(graph.node_count(), 2);
        graph.clear();
        assert_eq!(graph.node_count(), 0);
        assert_eq!(graph.edge_count(), 0);
    }

    #[test]
    fn test_edges_for_node() {
        let graph = ComponentBuilder::new("Test")
            .node("A", NodeKind::Component)
                .port(ComponentPort::output("x"))
            .end()
            .node("B", NodeKind::Component)
                .port(ComponentPort::input("y"))
                .port(ComponentPort::output("z"))
            .end()
            .node("C", NodeKind::Component)
                .port(ComponentPort::input("w"))
            .end()
            .connect("A", "x", "B", "y", EdgeKind::Connect)
            .connect("B", "z", "C", "w", EdgeKind::Connect)
            .build();

        let b = graph.find_node("B").unwrap();
        let edges: Vec<_> = graph.edges_for_node(b.id).collect();
        assert_eq!(edges.len(), 2); // one incoming, one outgoing
    }

    #[test]
    fn test_node_kinds() {
        // Verify all NodeKind variants are usable
        let mut graph = ComponentGraph::new();
        graph.add_node(NodeKind::Component, "c", vec![]);
        graph.add_node(NodeKind::Connector, "cn", vec![]);
        graph.add_node(NodeKind::Subsystem, "s", vec![]);
        graph.add_node(NodeKind::DigitalPort, "dp", vec![]);
        graph.add_node(NodeKind::PhysicalPort, "pp", vec![]);
        graph.add_node(NodeKind::Signal, "sig", vec![]);
        graph.add_node(NodeKind::Class, "cls", vec![]);
        assert_eq!(graph.node_count(), 7);
    }

    #[test]
    fn test_edge_kinds() {
        // Verify all EdgeKind variants are usable
        let kinds = [
            EdgeKind::Connect,
            EdgeKind::Wire,
            EdgeKind::Signal,
            EdgeKind::Extends,
            EdgeKind::Import,
            EdgeKind::Contains,
            EdgeKind::Association,
        ];
        let mut graph = ComponentGraph::new();
        let a = graph.add_node(NodeKind::Component, "a", vec![ComponentPort::output("x")]);
        for (i, kind) in kinds.iter().enumerate() {
            let b = graph.add_node(NodeKind::Component, &format!("b{i}"), vec![ComponentPort::input("y")]);
            graph.connect(a, 0, b, 0, *kind);
        }
        assert_eq!(graph.edge_count(), kinds.len());
    }
}
