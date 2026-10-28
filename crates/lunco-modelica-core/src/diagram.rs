//! Modelica-to-diagram graph builder.
//!
//! Projects an immutable Rumoca declaration snapshot into a `ComponentGraph`.
//! Package hierarchy reads the document AST; component semantics come from the
//! shared session.
//!
//! ## Supported diagram types
//!
//! - **Block Diagram**: Components as nodes, `connect()` as edges
//! - **Connection Diagram**: Like block diagram but connector nodes expanded
//! - **Package Hierarchy**: Packages as subsystem nodes with containment edges
//!
use lunco_modelica_ast::diagram_model::{
    ComponentGraph, ComponentPort, EdgeKind, NodeId, NodeKind, PortDirection,
};
use lunco_modelica_ast::{Causality, ClassType};
use rumoca_compile::parsing::ast::{ClassDef, Equation, Expression, StoredDefinition};
use rumoca_compile::{Session, compile::ClassComponentDeclaration};
use std::collections::HashMap;
use std::sync::Arc;

/// The declaration facts required by the consumer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagramScope {
    /// Target ports, parameters and icon; no internal component graph.
    ClassInterface,
    /// Target and instantiated components, including their connector ports.
    ComponentGraph,
}

/// Immutable presentation facts captured from the shared Rumoca session.
/// Inheritance and type lookup are completed before graph projection; missing
/// or cyclic declarations reject capture instead of inventing ports.
#[derive(Debug, Clone)]
pub struct DiagramSnapshot {
    pub target: String,
    pub members: Arc<Vec<ClassComponentDeclaration>>,
    /// Authored target equations for component-graph capture; omitted from interface capture.
    pub equations: Vec<Equation>,
    classes: HashMap<String, DiagramClass>,
}

#[derive(Debug, Clone)]
pub struct DiagramClass {
    pub class_type: ClassType,
    pub expandable: bool,
    pub partial: bool,
    pub annotation: Vec<Expression>,
    pub icon: Option<lunco_modelica_ast::annotations::Icon>,
    pub members: Arc<Vec<ClassComponentDeclaration>>,
}

/// Resolve a class's source owner through the shared session symbol index.
pub fn class_source_uri(session: &mut Session, target: &str) -> Result<String, String> {
    session
        .workspace_symbol_query(target.rsplit('.').next().unwrap_or(target))
        .into_iter()
        .find(|symbol| {
            let full = symbol
                .container_name
                .as_deref()
                .filter(|s| !s.is_empty())
                .map(|p| format!("{p}.{}", symbol.name))
                .unwrap_or_else(|| symbol.name.clone());
            full == target
                && matches!(
                    symbol.kind,
                    rumoca_compile::compile::WorkspaceSymbolKind::Class(_)
                )
        })
        .map(|symbol| symbol.uri)
        .ok_or_else(|| format!("Modelica source for `{target}` is unavailable"))
}

// Component declarations already include inherited members. Connections must
// follow the same inheritance, using Rumoca navigation to resolve each base.
fn inherited_equations(
    session: &mut Session,
    target: &str,
    ancestry: &mut Vec<String>,
) -> Result<Vec<Equation>, String> {
    if ancestry.iter().any(|name| name == target) {
        return Err(format!("Cyclic Modelica diagram inheritance at `{target}`"));
    }
    let definition = session
        .class_definition_query(target)
        .ok_or_else(|| format!("Modelica definition `{target}` is unavailable"))?
        .clone();
    ancestry.push(target.to_string());
    let mut equations = Vec::new();
    if !definition.extends.is_empty() {
        let uri = class_source_uri(session, target)?;
        for base in &definition.extends {
            let token = base
                .base_name
                .name
                .last()
                .ok_or_else(|| format!("Empty Modelica base name in `{target}`"))?;
            let location = &token.location;
            let resolved = session
                .navigation_class_target_query(
                    &uri,
                    location.start_line.saturating_sub(1),
                    location.start_column.saturating_sub(1),
                )
                .ok_or_else(|| {
                    format!(
                        "Unresolved Modelica diagram base `{}` in `{target}`",
                        base.base_name
                    )
                })?;
            equations.extend(inherited_equations(
                session,
                &resolved.qualified_name,
                ancestry,
            )?);
        }
    }
    ancestry.pop();
    equations.extend(definition.equations);
    Ok(equations)
}

impl DiagramSnapshot {
    /// Capture the selected interface or its component graph. Rumoca owns
    /// declaration resolution and caches member results by session revision.
    pub fn capture(
        session: &mut Session,
        target: &str,
        scope: DiagramScope,
    ) -> Result<Self, String> {
        let equations = match scope {
            DiagramScope::ClassInterface => Vec::new(),
            DiagramScope::ComponentGraph => inherited_equations(session, target, &mut Vec::new())?,
        };
        let members = session
            .class_component_declarations_query(target)
            .ok_or_else(|| {
                format!("Modelica diagram declarations for `{target}` are unresolved or cyclic")
            })?;
        let mut snapshot = Self {
            target: target.to_owned(),
            members,
            equations,
            classes: HashMap::new(),
        };
        snapshot.capture_class(session, target)?;
        let types: Vec<String> = snapshot
            .members
            .iter()
            .filter(|member| {
                scope == DiagramScope::ComponentGraph
                    || (!member.component.is_protected
                        && (!matches!(member.effective_causality, Causality::Empty)
                            || session
                                .class_definition_query(&member.type_name)
                                .is_some_and(|class| class.class_type == ClassType::Connector)))
            })
            .map(|member| member.type_name.clone())
            .collect();
        for name in types {
            snapshot.capture_class(session, &name)?;
        }
        if scope == DiagramScope::ComponentGraph {
            let port_types: Vec<String> = snapshot
                .classes
                .values()
                .flat_map(|class| class.members.iter().map(|member| member.type_name.clone()))
                .collect();
            for name in port_types {
                if session
                    .class_definition_query(&name)
                    .is_some_and(|class| class.class_type == ClassType::Connector)
                {
                    snapshot.capture_class(session, &name)?;
                }
            }
        }
        Ok(snapshot)
    }

    fn capture_class(&mut self, session: &mut Session, name: &str) -> Result<(), String> {
        if rumoca_core::is_builtin_type(name) || self.classes.contains_key(name) {
            return Ok(());
        }
        let class = session
            .class_definition_query(name)
            .ok_or_else(|| format!("Modelica diagram type `{name}` is unavailable"))?;
        let (class_type, expandable, partial, annotation, icon) = (
            class.class_type.clone(),
            class.expandable,
            class.partial,
            class.annotation.clone(),
            lunco_modelica_ast::annotations::extract_icon(&class.annotation),
        );
        let members = session
            .class_component_declarations_query(name)
            .ok_or_else(|| {
                format!("Modelica diagram declarations for `{name}` are unresolved or cyclic")
            })?;
        self.classes.insert(
            name.to_owned(),
            DiagramClass {
                class_type,
                expandable,
                partial,
                annotation,
                icon,
                members,
            },
        );
        Ok(())
    }

    pub fn class_ports(
        &self,
        class: &DiagramClass,
    ) -> Vec<lunco_modelica_index::visual_diagram::PortDef> {
        class.members.iter().filter(|member| self.is_port(member)).map(|member| {
        let sub = &member.component;
        let (x, y, size_x, size_y, rotation_deg) = lunco_modelica_ast::annotations::extract_placement(&sub.annotation).map(|placement| {
            let (x, y, width, height) = placement.transformation.centre_size();
            (x as f32, y as f32, width as f32, height as f32, placement.transformation.rotation as f32)
        }).unwrap_or((0.0, 0.0, 20.0, 20.0, 0.0));
        let connector = self.class(&member.type_name);
        let flow_vars = connector.map(|class| class.members.iter().filter_map(|member| {
            matches!(member.component.connection, rumoca_compile::parsing::ast::Connection::Flow(_)).then(|| {
                lunco_modelica_index::visual_diagram::FlowVarMeta {
                    name: member.component.name.clone(),
                    unit: member.component.modifications.get("unit")
                        .and_then(lunco_modelica_ast::ast_extract::string_literal_value).unwrap_or_default(),
                }
            })
        }).collect::<Vec<_>>()).unwrap_or_default();
        use lunco_modelica_index::visual_diagram::PortKind;
        let kind = if !flow_vars.is_empty() { PortKind::Acausal } else {
            match member.effective_causality {
                lunco_modelica_ast::Causality::Input(_) => PortKind::Input,
                lunco_modelica_ast::Causality::Output(_) => PortKind::Output,
                lunco_modelica_ast::Causality::Empty => {
                    let (n_in, n_out) = connector.map(|class| class.members.iter().fold((0,0), |(i,o), member| match member.effective_causality {
                        lunco_modelica_ast::Causality::Input(_) => (i+1,o),
                        lunco_modelica_ast::Causality::Output(_) => (i,o+1),
                        lunco_modelica_ast::Causality::Empty => (i,o),
                    })).unwrap_or((0,0));
                    match (n_in,n_out) { (1,0) => PortKind::Input, (0,1) => PortKind::Output, _ => PortKind::Acausal }
                }
            }
        };
        lunco_modelica_index::visual_diagram::PortDef {
            name: sub.name.clone(), connector_type: member.type_name.clone(), library_path: member.type_name.clone(),
            is_flow: !flow_vars.is_empty(), x, y, size_x, size_y,
            rotation_deg, color: connector.and_then(|class| connector_icon_color(&class.annotation)), kind, flow_vars,
        }
    }).collect()
    }

    pub(crate) fn classes_mut(&mut self) -> impl Iterator<Item = (&String, &mut DiagramClass)> {
        self.classes.iter_mut()
    }

    pub fn class(&self, name: &str) -> Option<&DiagramClass> {
        self.classes.get(name)
    }

    pub fn is_port(&self, member: &ClassComponentDeclaration) -> bool {
        !member.component.is_protected
            && (!matches!(member.effective_causality, Causality::Empty)
                || self
                    .class(&member.type_name)
                    .is_some_and(|class| class.class_type == ClassType::Connector))
    }

    fn ports_for_component(&self, member: &ClassComponentDeclaration) -> Vec<ComponentPort> {
        if self
            .class(&member.type_name)
            .is_some_and(|class| class.class_type == ClassType::Connector)
            || !matches!(member.effective_causality, Causality::Empty)
        {
            return vec![declaration_port(member, &member.component.name)];
        }
        // Only built-in primitives lack a class in this validated snapshot.
        self.class(&member.type_name)
            .map(|class| {
                class
                    .members
                    .iter()
                    .filter(|sub| self.is_port(sub))
                    .map(|sub| declaration_port(sub, &sub.component.name))
                    .collect()
            })
            .unwrap_or_default()
    }
}

// The first authored graphic color supplies connector presentation.
fn connector_icon_color(
    annotation: &[rumoca_compile::parsing::ast::Expression],
) -> Option<[u8; 3]> {
    use lunco_modelica_ast::annotations::{GraphicItem, extract_icon};
    let icon = extract_icon(annotation)?;
    for g in &icon.graphics {
        let (line, fill) = match g {
            GraphicItem::Rectangle(r) => (r.shape.line_color, r.shape.fill_color),
            GraphicItem::Polygon(p) => (p.shape.line_color, p.shape.fill_color),
            GraphicItem::Ellipse(e) => (e.shape.line_color, e.shape.fill_color),
            GraphicItem::Line(l) => (l.color, None),
            GraphicItem::Text(_) | GraphicItem::Bitmap(_) => (None, None),
        };
        if let Some(c) = line.or(fill) {
            return Some([c.r, c.g, c.b]);
        }
    }
    None
}

impl DiagramClass {
    pub fn parameters(&self) -> Vec<lunco_modelica_index::visual_diagram::ParamDef> {
        self.members
            .iter()
            .filter(|member| {
                matches!(
                    member.component.variability,
                    lunco_modelica_ast::Variability::Parameter(_)
                )
            })
            .map(|member| lunco_modelica_index::visual_diagram::ParamDef {
                name: member.component.name.clone(),
                param_type: member.type_name.clone(),
                default: member
                    .component
                    .binding
                    .as_ref()
                    .map(ToString::to_string)
                    .unwrap_or_default(),
                unit: member
                    .component
                    .modifications
                    .get("unit")
                    .and_then(lunco_modelica_ast::ast_extract::string_literal_value),
            })
            .collect()
    }
}

fn declaration_port(member: &ClassComponentDeclaration, name: &str) -> ComponentPort {
    let mut port = ComponentPort::output(name).with_type(&member.type_name);
    port.direction = match member.effective_causality {
        Causality::Input(_) => PortDirection::Input,
        Causality::Output(_) => PortDirection::Output,
        Causality::Empty => PortDirection::Bidir,
    };
    port
}

/// The type of diagram to generate from a Modelica model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DiagramType {
    /// Component block diagram — components as nodes, `connect()` as edges.
    #[default]
    BlockDiagram,
    /// Connection diagram — like block diagram but connector instances
    /// (e.g., `R1.p`, `R1.n`) are expanded as separate nodes.
    ConnectionDiagram,
    /// Package hierarchy — packages as subsystem nodes.
    PackageHierarchy,
}

/// Project already-resolved Modelica declarations into a component graph.
pub struct ModelicaComponentBuilder {
    ast: Arc<StoredDefinition>,
    snapshot: Arc<DiagramSnapshot>,
    diagram_type: DiagramType,
}

impl ModelicaComponentBuilder {
    pub fn from_snapshot(ast: Arc<StoredDefinition>, snapshot: Arc<DiagramSnapshot>) -> Self {
        Self {
            ast,
            snapshot,
            diagram_type: DiagramType::BlockDiagram,
        }
    }

    /// Set the diagram type to generate.
    pub fn diagram_type(mut self, ty: DiagramType) -> Self {
        self.diagram_type = ty;
        self
    }

    /// Build the diagram graph.
    pub fn build(self) -> ComponentGraph {
        match self.diagram_type {
            DiagramType::BlockDiagram => self.build_block_diagram(),
            DiagramType::ConnectionDiagram => self.build_connection_diagram(),
            DiagramType::PackageHierarchy => self.build_package_hierarchy(),
        }
    }

    // -----------------------------------------------------------------------
    // Block Diagram
    // -----------------------------------------------------------------------

    fn build_block_diagram(self) -> ComponentGraph {
        let target = &self.snapshot.target;
        let mut graph = ComponentGraph::titled(target);

        {
            let mut name_to_id: HashMap<String, NodeId> = HashMap::new();

            let merged: Vec<_> = self
                .snapshot
                .members
                .iter()
                .map(|member| (member.component.name.as_str(), member))
                .collect();

            // Add components as nodes
            for &(comp_name, member) in &merged {
                let ports = self.snapshot.ports_for_component(member);
                let qualified = format!("{}.{}", target, comp_name);
                let node_id =
                    graph.add_node_named(NodeKind::Component, comp_name, qualified, ports);
                // Store component type name in meta for display
                graph.nodes[node_id.0 as usize]
                    .meta
                    .insert("type_name".to_string(), member.type_name.clone());
                name_to_id.insert(comp_name.to_string(), node_id);
            }

            // Add connections as edges
            for eq in &self.snapshot.equations {
                if let Equation::Connect { lhs, rhs, .. } = eq {
                    let (src_node, src_port) = parse_connect_reference(lhs);
                    let (tgt_node, tgt_port) = parse_connect_reference(rhs);

                    if let (Some(&src_id), Some(&tgt_id)) =
                        (name_to_id.get(&src_node), name_to_id.get(&tgt_node))
                    {
                        let src_node_ref = graph.get_node(src_id).unwrap();
                        let tgt_node_ref = graph.get_node(tgt_id).unwrap();

                        // `connect(u, P.u)` where `u` is the enclosing
                        // class's own connector: parse_connect_reference
                        // returns ("u", "") for that side because the
                        // bare identifier is the entire reference. Our
                        // graph node for `u` has its single port (named
                        // "u" / "p" / whatever causality dictated) at
                        // index 0; treat the empty port-name as port 0
                        // so the wire actually gets built. Without
                        // this, every source library connect from a model-level
                        // connector silently drops out of the diagram.
                        let resolve_port =
                            |n: &lunco_modelica_ast::diagram_model::ComponentNode,
                             port: &str|
                             -> Option<usize> {
                                if port.is_empty() && !n.ports.is_empty() {
                                    Some(0)
                                } else {
                                    n.port_index(port)
                                }
                            };
                        if let (Some(sp), Some(tp)) = (
                            resolve_port(src_node_ref, &src_port),
                            resolve_port(tgt_node_ref, &tgt_port),
                        ) {
                            graph.connect(src_id, sp, tgt_id, tp, EdgeKind::Connect);
                        }
                    }
                }
            }

            // Generated USD networks use causal scalar ports. Their topology
            // is represented by direct equalities rather than `connect()`;
            // preserve those signal links in the shared graph as directed
            // edges while leaving component-internal equations private.
            for eq in &self.snapshot.equations {
                let Equation::Simple { lhs, rhs } = eq else {
                    continue;
                };
                let Some((lhs_node, lhs_port)) = simple_signal_reference(lhs) else {
                    continue;
                };
                let Some((rhs_node, rhs_port)) = simple_signal_reference(rhs) else {
                    continue;
                };
                let (Some(&lhs_id), Some(&rhs_id)) =
                    (name_to_id.get(&lhs_node), name_to_id.get(&rhs_node))
                else {
                    continue;
                };
                let lhs_kind = graph
                    .get_node(lhs_id)
                    .and_then(|node| node.ports.iter().find(|port| port.name == lhs_port))
                    .map(|port| port.direction);
                let rhs_kind = graph
                    .get_node(rhs_id)
                    .and_then(|node| node.ports.iter().find(|port| port.name == rhs_port))
                    .map(|port| port.direction);
                let (source_id, source_port, target_id, target_port) = match (lhs_kind, rhs_kind) {
                    (
                        Some(lunco_modelica_ast::diagram_model::PortDirection::Input),
                        Some(lunco_modelica_ast::diagram_model::PortDirection::Output),
                    ) => (rhs_id, rhs_port, lhs_id, lhs_port),
                    (
                        Some(lunco_modelica_ast::diagram_model::PortDirection::Output),
                        Some(lunco_modelica_ast::diagram_model::PortDirection::Input),
                    ) => (lhs_id, lhs_port, rhs_id, rhs_port),
                    _ => continue,
                };
                let Some(source_index) = graph
                    .get_node(source_id)
                    .and_then(|node| node.port_index(&source_port))
                else {
                    continue;
                };
                let Some(target_index) = graph
                    .get_node(target_id)
                    .and_then(|node| node.port_index(&target_port))
                else {
                    continue;
                };
                graph.connect(
                    source_id,
                    source_index,
                    target_id,
                    target_index,
                    EdgeKind::Signal,
                );
            }
        }

        graph
    }

    // -----------------------------------------------------------------------
    // Connection Diagram (expanded connectors as nodes)
    // -----------------------------------------------------------------------

    fn build_connection_diagram(self) -> ComponentGraph {
        let target = &self.snapshot.target;
        let mut graph = ComponentGraph::titled(target);

        {
            let mut name_to_id: HashMap<String, NodeId> = HashMap::new();

            // Track connector instances: "comp.port" → info
            let mut connector_registry: HashMap<String, ConnectorInfo> = HashMap::new();

            let merged: Vec<_> = self
                .snapshot
                .members
                .iter()
                .map(|member| (member.component.name.as_str(), member))
                .collect();

            // First pass: identify all connector ports on components
            for &(comp_name, member) in &merged {
                let conn_ports = self.snapshot.ports_for_component(member);
                for conn_name in &conn_ports {
                    let key = format!("{}.{}", comp_name, conn_name.name);
                    connector_registry
                        .entry(key.clone())
                        .or_insert_with(|| ConnectorInfo {
                            comp_name: comp_name.to_string(),
                            port_name: conn_name.name.clone(),
                            port_type: member.type_name.clone(),
                        });
                }
            }

            // Add component nodes (only those with connector ports)
            for &(comp_name, member) in &merged {
                let conn_ports = self.snapshot.ports_for_component(member);
                let ports = conn_ports;

                if !ports.is_empty() {
                    let node_id = graph.add_node_named(
                        NodeKind::Component,
                        comp_name,
                        format!("{}.{}", target, comp_name),
                        ports,
                    );
                    name_to_id.insert(comp_name.to_string(), node_id);
                }
            }

            // Add connector nodes
            for (conn_key, conn) in &connector_registry {
                let node_id = graph.add_node_named(
                    NodeKind::Connector,
                    conn_key,
                    format!("{}.{}", target, conn_key),
                    vec![ComponentPort::input("signal").with_type(&conn.port_type)],
                );
                name_to_id.insert(conn_key.clone(), node_id);
            }

            // Connect component ports → connector nodes
            for (conn_key, conn) in &connector_registry {
                if let (Some(&comp_id), Some(&conn_id)) =
                    (name_to_id.get(&conn.comp_name), name_to_id.get(conn_key))
                {
                    let comp_node = graph.get_node(comp_id).unwrap();
                    if let Some(sp) = comp_node.port_index(&conn.port_name) {
                        graph.connect(comp_id, sp, conn_id, 0, EdgeKind::Connect);
                    }
                }
            }

            // Connect connector nodes via `connect()` equations
            for eq in &self.snapshot.equations {
                if let Equation::Connect { lhs, rhs, .. } = eq {
                    let (src_node, src_port) = parse_connect_reference(lhs);
                    let (tgt_node, tgt_port) = parse_connect_reference(rhs);

                    let src_key = format!("{}.{}", src_node, src_port);
                    let tgt_key = format!("{}.{}", tgt_node, tgt_port);

                    if let (Some(&src_id), Some(&tgt_id)) =
                        (name_to_id.get(&src_key), name_to_id.get(&tgt_key))
                    {
                        graph.connect_labeled(
                            src_id,
                            0,
                            tgt_id,
                            0,
                            EdgeKind::Connect,
                            format!("{} ↔ {}", src_key, tgt_key),
                        );
                    }
                }
            }
        }

        graph
    }

    // -----------------------------------------------------------------------
    // Package Hierarchy
    // -----------------------------------------------------------------------

    fn build_package_hierarchy(self) -> ComponentGraph {
        let mut graph = ComponentGraph::titled("Package Hierarchy");
        let mut name_to_id: HashMap<String, NodeId> = HashMap::new();

        // Collect all class paths
        let mut all_classes: Vec<(String, ClassType)> = Vec::new();
        for (name, class) in &self.ast.classes {
            collect_class_names(class, name, &mut all_classes);
        }

        // Create nodes
        for (qualified_name, class_type) in &all_classes {
            let short_name = rumoca_core::top_level_last_segment(qualified_name);

            let kind = match class_type {
                ClassType::Package | ClassType::Class => NodeKind::Subsystem,
                ClassType::Model => NodeKind::Class,
                ClassType::Block => NodeKind::Class,
                ClassType::Function => NodeKind::Class,
                _ => NodeKind::Component,
            };

            let node_id = graph.add_node(kind, short_name, vec![]);
            graph.nodes[node_id.0 as usize].qualified_name = qualified_name.clone();
            name_to_id.insert(qualified_name.clone(), node_id);
        }

        // Add containment edges
        for (qualified_name, _) in &all_classes {
            if let Some((parent_name, _child)) = rumoca_core::split_last_top_level(qualified_name) {
                if let Some(&parent_id) = name_to_id.get(parent_name) {
                    if let Some(&child_id) = name_to_id.get(qualified_name) {
                        // Find port indices (create if needed)
                        let parent = graph.get_node(parent_id).unwrap();
                        let port_name = qualified_name.to_string();
                        let source_port = parent.port_index(&port_name).unwrap_or_else(|| {
                            let idx = graph.nodes[parent_id.0 as usize].ports.len();
                            graph.nodes[parent_id.0 as usize].ports.push(
                                ComponentPort::output(&port_name).with_description("Contains"),
                            );
                            idx
                        });

                        let child = graph.get_node(child_id).unwrap();
                        let target_port = child.port_index(&port_name).unwrap_or_else(|| {
                            let idx = graph.nodes[child_id.0 as usize].ports.len();
                            graph.nodes[child_id.0 as usize].ports.push(
                                ComponentPort::input(&port_name).with_description("Contained in"),
                            );
                            idx
                        });

                        graph.connect(
                            parent_id,
                            source_port,
                            child_id,
                            target_port,
                            EdgeKind::Contains,
                        );
                    }
                }
            }
        }

        graph
    }

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------
}

/// Information about a connector instance in a Modelica model.
#[derive(Debug, Clone)]
struct ConnectorInfo {
    comp_name: String,
    port_name: String,
    port_type: String,
}

/// Parse a Modelica component reference like `R1.p` into `(component, port)`.
///
/// The first segment identifies the graph node; the remaining path identifies
/// its nested connector.
fn parse_connect_reference(
    comp_ref: &rumoca_compile::parsing::ast::ComponentReference,
) -> (String, String) {
    let parts: Vec<String> = comp_ref
        .parts
        .iter()
        .map(|p| p.ident.text.to_string())
        .collect();

    match parts.len() {
        0 => (String::new(), String::new()),
        1 => (parts[0].clone(), String::new()),
        _ => {
            let port = parts[1..].join(".");
            let comp = parts[0].clone();
            (comp, port)
        }
    }
}

fn simple_signal_reference(expression: &Expression) -> Option<(String, String)> {
    let Expression::ComponentReference(reference) = expression else {
        return None;
    };
    let mut parts = reference
        .parts
        .iter()
        .map(|part| part.ident.text.to_string());
    let component = parts.next()?;
    let port = parts.collect::<Vec<_>>().join(".");
    (!port.is_empty()).then_some((component, port))
}

/// Extract all top-level class names from an AST, including nested classes.
pub fn list_class_names(ast: &StoredDefinition) -> Vec<String> {
    let mut names: Vec<(String, ClassType)> = Vec::new();
    for (name, class) in &ast.classes {
        collect_class_names(class, name, &mut names);
    }
    names.into_iter().map(|(n, _)| n).collect()
}

fn collect_class_names(class: &ClassDef, prefix: &str, names: &mut Vec<(String, ClassType)>) {
    names.push((prefix.to_string(), class.class_type.clone()));
    for (nested_name, nested) in &class.classes {
        collect_class_names(nested, &format!("{}.{}", prefix, nested_name), names);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interface_capture_retains_ports_without_projecting_internal_bodies() {
        let mut session = Session::default();
        session
            .add_document(
                "interface.mo",
                r#"
package Library
  connector Pin
    flow Real current(unit="A");
    Real voltage(unit="V");
  end Pin;
  model Internal Missing unresolved; end Internal;
  model Root
    Pin terminal;
    Internal internal;
    parameter Real gain=2;
  end Root;
end Library;
"#,
            )
            .unwrap();
        let interface =
            DiagramSnapshot::capture(&mut session, "Library.Root", DiagramScope::ClassInterface)
                .unwrap();
        assert!(interface.class("Library.Internal").is_none());
        assert!(interface.equations.is_empty());
        let root = interface.class("Library.Root").unwrap();
        let ports = interface.class_ports(root);
        assert_eq!(ports.len(), 1);
        assert_eq!(ports[0].name, "terminal");
        assert_eq!(ports[0].flow_vars[0].unit, "A");
        assert_eq!(root.parameters()[0].name, "gain");
        assert!(
            DiagramSnapshot::capture(&mut session, "Library.Root", DiagramScope::ComponentGraph)
                .is_err()
        );
        assert!(
            DiagramSnapshot::capture(
                &mut session,
                "Library.Internal",
                DiagramScope::ClassInterface
            )
            .is_err()
        );
    }

    fn source_builder(source: &str, target: Option<&str>) -> ModelicaComponentBuilder {
        let ast = rumoca_phase_parse::parse_to_ast(source, "diagram-fixture.mo")
            .expect("valid diagram source fixture");
        let target = target
            .map(str::to_owned)
            .or_else(|| lunco_modelica_ast::ast_extract::extract_model_name_from_ast(&ast))
            .expect("fixture target");
        let mut session = Session::default();
        session
            .add_document("diagram-fixture.mo", source)
            .expect("fixture admission");
        let snapshot =
            DiagramSnapshot::capture(&mut session, &target, DiagramScope::ComponentGraph)
                .expect("resolved fixture");
        ModelicaComponentBuilder::from_snapshot(Arc::new(ast), Arc::new(snapshot))
    }

    #[test]
    fn test_block_diagram_rc_circuit() {
        let source = r#"
model RC_Circuit
  Resistor R1(R=100.0);
  Capacitor C1(C=1e-3);
  VoltageSource V1(V=5.0);
equation
  connect(V1.p, R1.p);
  connect(R1.n, C1.p);
  connect(C1.n, V1.n);
end RC_Circuit;

connector Pin
  flow Real i;
  Real v;
end Pin;

model Resistor
  Pin p;
  Pin n;
  parameter Real R;
equation
  R * p.i = p.v - n.v;
  p.i + n.i = 0;
end Resistor;

model Capacitor
  Pin p;
  Pin n;
  parameter Real C;
equation
  C * der(p.v - n.v) = p.i;
end Capacitor;

model VoltageSource
  Pin p;
  Pin n;
  parameter Real V;
equation
  p.v - n.v = V;
end VoltageSource;
"#;
        let builder = source_builder(source, Some("RC_Circuit"));
        let graph = builder.diagram_type(DiagramType::BlockDiagram).build();

        assert_eq!(graph.node_count(), 3, "Should have R1, C1, V1");
        assert_eq!(graph.edge_count(), 3, "Should have 3 connect() equations");

        // find_node searches by qualified_name
        assert!(
            graph.find_node("RC_Circuit.R1").is_some(),
            "Should have R1 node"
        );
        assert!(
            graph.find_node("RC_Circuit.C1").is_some(),
            "Should have C1 node"
        );
        assert!(
            graph.find_node("RC_Circuit.V1").is_some(),
            "Should have V1 node"
        );
    }

    #[test]
    fn test_block_diagram_simple_model() {
        let source = r#"
model SpringMass
  parameter Real k = 100.0;
  parameter Real m = 1.0;
  Real x;
  Real v;
equation
  v = der(x);
  m * der(v) = -k * x;
end SpringMass;
"#;
        let builder = source_builder(source, None);
        let graph = builder.build();

        // SpringMass has no connect() equations, so no edges
        assert_eq!(graph.edge_count(), 0);
    }

    #[test]
    fn test_connection_diagram_expands_connectors() {
        let source = r#"
model RC_Circuit
  Resistor R1(R=100.0);
  Capacitor C1(C=1e-3);
equation
  connect(R1.p, C1.p);
  connect(R1.n, C1.n);
end RC_Circuit;

model Resistor
  Pin p;
  Pin n;
end Resistor;

model Capacitor
  Pin p;
  Pin n;
end Capacitor;
connector Pin
  Real v;
  flow Real i;
end Pin;
"#;
        let builder = source_builder(source, Some("RC_Circuit"));
        let graph = builder.diagram_type(DiagramType::ConnectionDiagram).build();

        // Should have R1, C1, plus connector nodes (R1.p, R1.n, C1.p, C1.n)
        assert!(graph.node_count() >= 2, "Should have at least R1 and C1");
    }

    #[test]
    fn connection_diagram_uses_resolved_port_names() {
        let source = r#"
model Circuit
  VoltageWidget a;
  VoltageWidget b;
equation
  connect(a.outlet, b.inlet);
end Circuit;
model VoltageWidget
  Socket inlet;
  Socket outlet;
end VoltageWidget;
connector Socket
  Real potential;
  flow Real transfer;
end Socket;
"#;
        let graph = source_builder(source, Some("Circuit"))
            .diagram_type(DiagramType::ConnectionDiagram)
            .build();
        let node = graph.find_node("Circuit.a").expect("component node");
        assert_eq!(
            node.ports
                .iter()
                .map(|port| port.name.as_str())
                .collect::<Vec<_>>(),
            vec!["inlet", "outlet"]
        );
        assert!(graph.find_node("Circuit.a.outlet").is_some());
        assert!(graph.find_node("Circuit.a.p").is_none());
    }

    #[test]
    fn test_package_hierarchy() {
        let source = r#"
package MyLib
  model Components
    model Resistor
      Real p, n;
    end Resistor;
  end Components;

  model Circuits
    import MyLib.Components.Resistor;
    model RC
      Resistor R1;
    end RC;
  end Circuits;
end MyLib;
"#;
        let builder = source_builder(source, None);
        let graph = builder.diagram_type(DiagramType::PackageHierarchy).build();

        // Should have MyLib, Components, Resistor, Circuits, RC
        assert!(graph.node_count() >= 3);
    }

    #[test]
    fn test_parse_connect_reference() {
        use lunco_modelica_ast::{Span, Token as AstToken};
        use rumoca_compile::parsing::ast::ComponentRefPart;
        use std::sync::Arc;

        fn make_ref(parts: &[(&str, &str)]) -> rumoca_compile::parsing::ast::ComponentReference {
            rumoca_compile::parsing::ast::ComponentReference {
                local: false,
                parts: parts
                    .iter()
                    .map(|(_name, text)| ComponentRefPart {
                        ident: AstToken {
                            text: Arc::from(*text),
                            ..Default::default()
                        },
                        subs: None,
                        def_id: None,
                    })
                    .collect(),
                qualified_display_name: None,
                span: Span::DUMMY,
            }
        }

        let r = make_ref(&[("R1", "R1"), ("p", "p")]);
        let (node, port) = parse_connect_reference(&r);
        assert_eq!(node, "R1");
        assert_eq!(port, "p");

        let r = make_ref(&[("A", "A"), ("B", "B"), ("p", "p")]);
        let (node, port) = parse_connect_reference(&r);
        assert_eq!(node, "A");
        assert_eq!(port, "B.p");
    }

    #[test]
    fn test_block_diagram_includes_inherited_connectors() {
        // PID-style: derived class extends a base that owns u/y.
        // Without extends-walking, wires to `u`/`y` silently drop.
        let source = r#"
partial model SISO
  RealInput u;
  RealOutput y;
end SISO;

model PID
  extends SISO;
  Gain k;
equation
  connect(u, k.u);
  connect(k.y, y);
end PID;

connector RealInput
  input Real signal;
end RealInput;

connector RealOutput
  output Real signal;
end RealOutput;

block Gain
  RealInput u;
  RealOutput y;
end Gain;
"#;
        let builder = source_builder(source, Some("PID"));
        let graph = builder.diagram_type(DiagramType::BlockDiagram).build();

        assert_eq!(graph.edge_count(), 2);
        assert!(
            graph.find_node("PID.u").is_some(),
            "u (inherited from SISO) must be a node"
        );
        assert!(
            graph.find_node("PID.y").is_some(),
            "y (inherited from SISO) must be a node"
        );
        assert!(
            graph.find_node("PID.k").is_some(),
            "k (direct) must be a node"
        );
    }

    #[test]
    fn snapshot_rejects_invalid_types_and_tracks_session_revision() {
        let mut session = Session::default();
        session
            .add_document("facts.mo", "model Root MissingType child; end Root;")
            .unwrap();
        assert!(
            DiagramSnapshot::capture(&mut session, "Root", DiagramScope::ComponentGraph).is_err()
        );
        session
            .add_document(
                "facts.mo",
                "model Root extends Base; end Root; model Base extends Root; end Base;",
            )
            .unwrap();
        assert!(
            DiagramSnapshot::capture(&mut session, "Root", DiagramScope::ComponentGraph).is_err()
        );
        session
            .add_document("facts.mo", "model Root input Real demand; end Root;")
            .unwrap();
        let first =
            DiagramSnapshot::capture(&mut session, "Root", DiagramScope::ComponentGraph).unwrap();
        let repeated =
            DiagramSnapshot::capture(&mut session, "Root", DiagramScope::ComponentGraph).unwrap();
        assert!(Arc::ptr_eq(&first.members, &repeated.members));
        session
            .add_document("facts.mo", "model Root output Real delivered; end Root;")
            .unwrap();
        let edited =
            DiagramSnapshot::capture(&mut session, "Root", DiagramScope::ComponentGraph).unwrap();
        assert_eq!(edited.members[0].component.name, "delivered");
        assert_eq!(first.members[0].component.name, "demand");
        assert!(!Arc::ptr_eq(&first.members, &edited.members));
    }

    #[test]
    fn test_list_class_names() {
        let source = r#"
package MyLib
  model A
    model B
    end B;
  end A;
  model C
  end C;
end MyLib;
"#;
        let ast = rumoca_phase_parse::parse_to_ast(source, "test.mo").unwrap();
        let names = list_class_names(&ast);
        assert!(names.contains(&"MyLib".to_string()));
        assert!(names.contains(&"MyLib.A".to_string()));
        assert!(names.contains(&"MyLib.A.B".to_string()));
        assert!(names.contains(&"MyLib.C".to_string()));
    }
}
