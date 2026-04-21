//! Modelica-to-diagram graph builder.
//!
//! Converts a parsed Modelica AST (`StoredDefinition`) into a [`ComponentGraph`]
//! that can be rendered by any diagram viewer.
//!
//! ## Supported diagram types
//!
//! - **Block Diagram**: Components as nodes, `connect()` as edges
//! - **Connection Diagram**: Like block diagram but connector nodes expanded
//! - **Package Hierarchy**: Packages as subsystem nodes with containment edges
//!
//! ## Usage
//!
//! ```ignore
//! use rumoca_phase_parse::parse_to_ast;
//! use lunco_modelica::diagram::ModelicaComponentBuilder;
//!
//! let ast = parse_to_ast(source, "model.mo").unwrap();
//! let graph = ModelicaComponentBuilder::from_ast(&ast)
//!     .diagram_type(DiagramType::BlockDiagram)
//!     .build();
//! ```

use lunco_core::diagram::{
    ComponentGraph, ComponentPort, EdgeKind, NodeId, NodeKind,
};
use rumoca_session::parsing::ast::{ClassDef, Component, Equation, Expression, StoredDefinition, Variability, Causality};
use rumoca_session::parsing::ClassType;
use std::collections::HashMap;

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

/// Builder that converts a Modelica AST into a [`ComponentGraph`].
pub struct ModelicaComponentBuilder {
    /// Shared reference to the parsed AST. Using `Arc` here instead
    /// of an owned `StoredDefinition` is load-bearing: MSL package
    /// files like `Modelica/Blocks/package.mo` parse into trees
    /// tens of megabytes deep, and a naïve `.clone()` of the
    /// whole tree on every projection is enough to push the OS
    /// into swap and freeze the whole device. Cloning the `Arc` is
    /// a single pointer bump regardless of tree size.
    ast: std::sync::Arc<StoredDefinition>,
    diagram_type: DiagramType,
    /// If set, only build the diagram for this specific class name.
    target_class: Option<String>,
}

impl ModelicaComponentBuilder {
    /// Create a new builder from a shared parsed AST. The `Arc`
    /// contract lets callers reuse the same AST across many
    /// builders without any deep clone; see the `ast` field
    /// comment for why this matters.
    pub fn from_ast(ast: std::sync::Arc<StoredDefinition>) -> Self {
        Self {
            ast,
            diagram_type: DiagramType::BlockDiagram,
            target_class: None,
        }
    }

    /// Create a new builder from raw Modelica source code.
    ///
    /// Uses rumoca's error-recovering `parse_to_syntax(...).best_effort()`
    /// so partial / semantically-invalid sources still produce a
    /// usable AST. The editor and diagram renderer should never go
    /// blank just because the user typed a duplicate name, a missing
    /// semicolon, or a half-finished construct — those are
    /// diagnostics, not "the file ceased to exist" signals, and they
    /// should match OMEdit / Dymola's "show everything, flag the
    /// problem" behaviour.
    ///
    /// Returns `None` only if rumoca couldn't even construct a
    /// recovery tree (i.e. catastrophically broken input). In
    /// practice `parse_to_syntax` always returns *something*, so the
    /// `None` branch is defensive.
    pub fn from_source(source: &str) -> Option<Self> {
        let syntax = rumoca_phase_parse::parse_to_syntax(source, "model.mo");
        let ast: StoredDefinition = syntax.best_effort().clone();
        Some(Self::from_ast(std::sync::Arc::new(ast)))
    }

    /// Set the diagram type to generate.
    pub fn diagram_type(mut self, ty: DiagramType) -> Self {
        self.diagram_type = ty;
        self
    }

    /// Set the target class to diagram (default: first non-package class).
    pub fn target_class(mut self, name: impl Into<String>) -> Self {
        self.target_class = Some(name.into());
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
        let target = self.resolve_target_class();
        let mut graph = ComponentGraph::titled(self.ast_title());

        if let Some(class) = self.get_target_class(&target) {
            let mut name_to_id: HashMap<String, NodeId> = HashMap::new();

            // Direct components first; then merge components inherited via
            // `extends` so wires connecting to parent-class connectors
            // (e.g. PID's u/y from `extends Modelica.Blocks.Interfaces.SISO`)
            // actually find a node to land on. Direct declarations win
            // on name collision (MLS §7 inheritance precedence).
            let mut merged: Vec<(String, Component)> = class
                .components
                .iter()
                .map(|(n, c)| (n.clone(), c.clone()))
                .collect();
            let direct_names: std::collections::HashSet<String> =
                merged.iter().map(|(n, _)| n.clone()).collect();
            for (name, comp) in collect_inherited_components(class, Some(&target), &self.ast, 0) {
                if !direct_names.contains(&name) {
                    merged.push((name, comp));
                }
            }

            // Add components as nodes
            for (comp_name, comp) in &merged {
                let ports = extract_component_ports(comp);
                let qualified = format!("{}.{}", target, comp_name);
                let node_id = graph.add_node_named(
                    NodeKind::Component,
                    comp_name,
                    qualified,
                    ports,
                );
                // Store component type name in meta for display
                graph.nodes[node_id.0 as usize].meta.insert(
                    "type_name".to_string(),
                    comp.type_name.to_string(),
                );
                name_to_id.insert(comp_name.clone(), node_id);
            }

            // Add connections as edges
            for eq in &class.equations {
                if let Equation::Connect { lhs, rhs } = eq {
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
                        // this, every MSL connect from a model-level
                        // connector silently drops out of the diagram.
                        let resolve_port = |
                            n: &lunco_core::diagram::ComponentNode,
                            port: &str,
                        | -> Option<usize> {
                            if port.is_empty() && !n.ports.is_empty() {
                                Some(0)
                            } else {
                                n.port_index(port).map(|p| p as usize)
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
        }

        graph
    }

    // -----------------------------------------------------------------------
    // Connection Diagram (expanded connectors as nodes)
    // -----------------------------------------------------------------------

    fn build_connection_diagram(self) -> ComponentGraph {
        let target = self.resolve_target_class();
        let mut graph = ComponentGraph::titled(self.ast_title());

        if let Some(class) = self.get_target_class(&target) {
            let mut name_to_id: HashMap<String, NodeId> = HashMap::new();

            // Track connector instances: "comp.port" → info
            let mut connector_registry: HashMap<String, ConnectorInfo> = HashMap::new();

            // Same merge as block diagram — see `build_block_diagram` for
            // the inheritance rationale.
            let mut merged: Vec<(String, Component)> = class
                .components
                .iter()
                .map(|(n, c)| (n.clone(), c.clone()))
                .collect();
            let direct_names: std::collections::HashSet<String> =
                merged.iter().map(|(n, _)| n.clone()).collect();
            for (name, comp) in collect_inherited_components(class, Some(&target), &self.ast, 0) {
                if !direct_names.contains(&name) {
                    merged.push((name, comp));
                }
            }

            // First pass: identify all connector ports on components
            for (comp_name, comp) in &merged {
                let conn_ports = get_connector_port_names(comp);
                for conn_name in &conn_ports {
                    let key = format!("{}.{}", comp_name, conn_name);
                    connector_registry.entry(key.clone()).or_insert_with(|| ConnectorInfo {
                        name: key.clone(),
                        comp_name: comp_name.clone(),
                        port_name: conn_name.clone(),
                        port_type: comp.type_name.to_string(),
                    });
                }
            }

            // Add component nodes (only those with connector ports)
            for (comp_name, comp) in &merged {
                let conn_ports = get_connector_port_names(comp);
                let ports: Vec<ComponentPort> = conn_ports
                    .iter()
                    .map(|name| ComponentPort::output(name).with_type(&comp.type_name.to_string()))
                    .collect();

                if !ports.is_empty() {
                    let node_id = graph.add_node_named(
                        NodeKind::Component,
                        comp_name,
                        format!("{}.{}", target, comp_name),
                        ports,
                    );
                    name_to_id.insert(comp_name.clone(), node_id);
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
            for eq in &class.equations {
                if let Equation::Connect { lhs, rhs } = eq {
                    let (src_node, src_port) = parse_connect_reference(lhs);
                    let (tgt_node, tgt_port) = parse_connect_reference(rhs);

                    let src_key = format!("{}.{}", src_node, src_port);
                    let tgt_key = format!("{}.{}", tgt_node, tgt_port);

                    if let (Some(&src_id), Some(&tgt_id)) =
                        (name_to_id.get(&src_key), name_to_id.get(&tgt_key))
                    {
                        graph.connect_labeled(
                            src_id, 0, tgt_id, 0, EdgeKind::Connect,
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
            let short_name = qualified_name.split('.').last().unwrap_or(qualified_name);
            let _parent = qualified_name.rsplit_once('.').map(|(p, _)| p.to_string());

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
            if let Some((parent_name, _child)) = qualified_name.rsplit_once('.') {
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

                        graph.connect(parent_id, source_port, child_id, target_port, EdgeKind::Contains);
                    }
                }
            }
        }

        graph
    }

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    fn ast_title(&self) -> String {
        if let Some(w) = &self.ast.within {
            format!("{}.{}", w, self.target_class.as_deref().unwrap_or("?"))
        } else {
            self.target_class.clone().unwrap_or_else(|| "Model".to_string())
        }
    }

    fn resolve_target_class(&self) -> String {
        if let Some(name) = &self.target_class {
            return name.clone();
        }
        // Find first non-package class
        for (name, class) in &self.ast.classes {
            if class.class_type != ClassType::Package {
                return name.clone();
            }
        }
        // Fallback to first class
        self.ast.classes.keys().next().cloned().unwrap_or_default()
    }

    fn get_target_class(&self, name: &str) -> Option<&ClassDef> {
        find_class_by_qualified_name(&self.ast, name)
    }
}

/// Resolve a qualified class name against a parsed `StoredDefinition`.
///
/// Shared between the projection builder and the drill-in install
/// path (which uses it to decide the default view for the
/// newly-opened tab — if the class has zero instantiated
/// components, landing in Canvas shows an empty diagram, so
/// Icon is a better default).
///
/// # Resolution rules
///
/// 1. **Single-segment name** — check top-level `ast.classes`,
///    then one level of nested descent. Preserves historic
///    behaviour for callers like `target_class("MyClass")`.
/// 2. **Dotted path** — walk nested classes segment-by-segment.
///    `"Blocks.Examples.FilterWithRiseTime"` inside
///    `Modelica/Blocks/package.mo` descends
///    `Blocks → Examples → FilterWithRiseTime`.
/// 3. **`within` prefix tolerance** — if the full path starts
///    with the AST's `within` clause, strip it before walking.
///    Lets drill-in callers pass `"Modelica.Blocks.Continuous.CriticalDamping"`
///    without knowing the file's internal rooting.
pub fn find_class_by_qualified_name<'a>(
    ast: &'a StoredDefinition,
    name: &str,
) -> Option<&'a ClassDef> {
    if !name.contains('.') {
        if let Some(class) = ast.classes.get(name) {
            return Some(class);
        }
        for (_, class) in &ast.classes {
            if let Some(nested) = class.classes.get(name) {
                return Some(nested);
            }
        }
        return None;
    }

    let mut path: &str = name;
    if let Some(within) = ast.within.as_ref() {
        let within_str = within.to_string();
        if let Some(rest) = path
            .strip_prefix(&within_str)
            .and_then(|s| s.strip_prefix('.'))
        {
            path = rest;
        }
    }
    let mut segments = path.split('.');
    let first = segments.next()?;
    let mut current = ast.classes.get(first)?;
    for seg in segments {
        current = current.classes.get(seg)?;
    }
    Some(current)
}

/// Walk a class's `extends` chain and collect all components inherited
/// from base classes. Direct (non-inherited) components are *not*
/// returned — the caller is responsible for merging.
///
/// `class_qualified_path` is the dotted qualified name of `class`
/// (e.g. `"Modelica.Blocks.Continuous.PID"`). It's used to resolve
/// short-form `extends` references the way MLS §5.3 prescribes:
/// walk enclosing scopes outward until a match is found. PID's
/// `extends Interfaces.SISO` resolves to
/// `Modelica.Blocks.Interfaces.SISO`, not the literal `Interfaces.SISO`.
/// Pass `None` when no such context exists (e.g. an MSL base whose
/// own qualified path we don't carry through).
///
/// Resolution order for each `extends` target:
///   1. Local lookup inside the same `StoredDefinition`.
///   2. MSL filesystem index via [`crate::class_cache::peek_or_load_msl_class`].
///   3. Both retried with each enclosing-scope prefix of
///      `class_qualified_path`.
///
/// Honors `break_names` (MLS §7.4 selective model extension).
/// Depth is capped to defend against pathological cycles.
pub(crate) fn collect_inherited_components(
    class: &ClassDef,
    class_qualified_path: Option<&str>,
    ast: &StoredDefinition,
    depth: u32,
) -> Vec<(String, Component)> {
    collect_inherited_components_with(
        class,
        class_qualified_path,
        ast,
        depth,
        &crate::class_cache::peek_msl_class_cached,
    )
}

/// Same as [`collect_inherited_components`] but takes a custom MSL
/// resolver. Tests pass [`crate::class_cache::peek_or_load_msl_class`]
/// to load synchronously from a non-worker thread; the projection
/// task uses the default cache-only resolver.
pub(crate) fn collect_inherited_components_with(
    class: &ClassDef,
    class_qualified_path: Option<&str>,
    ast: &StoredDefinition,
    depth: u32,
    msl_resolve: &dyn Fn(&str) -> Option<std::sync::Arc<ClassDef>>,
) -> Vec<(String, Component)> {
    const MAX_DEPTH: u32 = 8;
    let mut out: Vec<(String, Component)> = Vec::new();
    if depth >= MAX_DEPTH {
        return out;
    }
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for ext in &class.extends {
        let raw = ext.base_name.to_string();
        if raw.is_empty() {
            continue;
        }
        let breaks: std::collections::HashSet<&str> =
            ext.break_names.iter().map(|s| s.as_str()).collect();

        let candidates = scope_chain_candidates(&raw, class_qualified_path);

        // Resolution attempts:
        //   1. Local AST lookup (same file / same package).
        //   2. *Already-cached* MSL class from `peek_msl_class_cached` —
        //      MUST be cache-only, never trigger a fresh parse here.
        //
        // Why no fresh MSL parse: this function is called inside the
        // projection task running on Bevy's AsyncComputeTaskPool. The
        // pool is small (≈ N/4 threads); a synchronous rumoca parse of
        // a large MSL file (e.g. `Continuous.mo`, 184 KB) from inside a
        // worker that's *already* serving a parent rumoca parse stalls
        // for the full 60 s projection deadline. Pre-warming
        // cross-file MSL inheritance belongs in a separate background
        // task that runs at drill-in time and feeds the cache.
        let mut found_local: Option<(&ClassDef, String)> = None;
        let mut found_msl: Option<(std::sync::Arc<ClassDef>, String)> = None;
        for cand in &candidates {
            if let Some(base) = find_class_by_qualified_name(ast, cand) {
                found_local = Some((base, cand.clone()));
                break;
            }
            if let Some(base_arc) = msl_resolve(cand) {
                found_msl = Some((base_arc, cand.clone()));
                break;
            }
        }

        let (base, base_qpath): (&ClassDef, String) = if let Some((b, q)) = found_local {
            (b, q)
        } else if let Some((ref arc, ref q)) = found_msl {
            (&**arc, q.clone())
        } else {
            continue;
        };

        for (name, comp) in &base.components {
            if breaks.contains(name.as_str()) || seen.contains(name) {
                continue;
            }
            seen.insert(name.clone());
            out.push((name.clone(), comp.clone()));
        }
        let shim = StoredDefinition::default();
        for (name, comp) in collect_inherited_components_with(
            base,
            Some(&base_qpath),
            &shim,
            depth + 1,
            msl_resolve,
        ) {
            if breaks.contains(name.as_str()) || seen.contains(&name) {
                continue;
            }
            seen.insert(name.clone());
            out.push((name, comp));
        }
    }
    out
}

/// Generate candidate fully-qualified names for resolving a short-form
/// reference, walking outward from the most-specific enclosing scope
/// (per MLS §5.3 lookup). For raw `"Interfaces.SISO"` referenced from
/// `"Modelica.Blocks.Continuous.PID"`, yields:
///   1. `"Modelica.Blocks.Continuous.Interfaces.SISO"` (sibling scope)
///   2. `"Modelica.Blocks.Interfaces.SISO"` (parent scope) ← matches
///   3. `"Modelica.Interfaces.SISO"`
///   4. `"Interfaces.SISO"` (root)
fn scope_chain_candidates(raw: &str, ctx: Option<&str>) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(ctx) = ctx {
        let parts: Vec<&str> = ctx.split('.').collect();
        // Drop the leaf class itself, then walk up its parents.
        for i in (0..parts.len().saturating_sub(1)).rev() {
            let prefix = parts[..=i].join(".");
            out.push(format!("{}.{}", prefix, raw));
        }
    }
    out.push(raw.to_string());
    out
}

/// Information about a connector instance in a Modelica model.
#[derive(Debug, Clone)]
struct ConnectorInfo {
    #[allow(dead_code)]
    name: String,
    comp_name: String,
    port_name: String,
    port_type: String,
}

/// Extract diagram ports from a component's connectors.
fn extract_component_ports(comp: &Component) -> Vec<ComponentPort> {
    let mut ports = Vec::new();

    match &comp.causality {
        Causality::Input(_) => {
            ports.push(ComponentPort::input("u").with_type(comp.type_name.to_string()));
        }
        Causality::Output(_) => {
            ports.push(ComponentPort::output("y").with_type(comp.type_name.to_string()));
        }
        Causality::Empty => {
            // Acausal connector (e.g., Resistor, Capacitor)
            // Default to electrical-style p/n ports
            ports.push(ComponentPort::output("p").with_type(comp.type_name.to_string()));
            ports.push(ComponentPort::output("n").with_type(comp.type_name.to_string()));
        }
    }

    // Parameter value as labeled output port
    if matches!(comp.variability, Variability::Parameter(_)) {
        if let Some(ref binding) = comp.binding {
            if let Expression::Terminal { token, .. } = binding {
                ports.push(
                    ComponentPort::output("value")
                        .with_type(comp.type_name.to_string())
                        .with_description(format!("= {}", token.text)),
                );
            }
        }
    }

    ports
}

/// Get the names of connector ports on a component (e.g., "p", "n" for
/// electrical, "flange_a", "flange_b" for mechanical).
fn get_connector_port_names(comp: &Component) -> Vec<String> {
    let type_str = comp.type_name.to_string();
    let lower = type_str.to_lowercase();

    // Electrical connectors
    if lower.contains("resistor")
        || lower.contains("capacitor")
        || lower.contains("inductor")
        || lower.contains("voltage")
        || lower.contains("current")
        || lower.contains("pin")
    {
        return vec!["p".to_string(), "n".to_string()];
    }

    // Mechanical translational / rotational
    if lower.contains("spring")
        || lower.contains("damper")
        || lower.contains("mass")
        || lower.contains("flange")
        || lower.contains("inertia")
        || lower.contains("torque")
    {
        return vec!["flange_a".to_string(), "flange_b".to_string()];
    }

    // Generic block diagram I/O
    if matches!(comp.causality, Causality::Input(_)) {
        return vec!["u".to_string()];
    }
    if matches!(comp.causality, Causality::Output(_)) {
        return vec!["y".to_string()];
    }

    // Default: electrical-style ports
    vec!["p".to_string(), "n".to_string()]
}

/// Parse a Modelica component reference like `R1.p` into `(component, port)`.
///
/// Handles simple two-part references (component.port). For more complex
/// paths like `a.b.c.p`, returns the last two parts.
fn parse_connect_reference(comp_ref: &rumoca_session::parsing::ast::ComponentReference) -> (String, String) {
    let parts: Vec<String> = comp_ref
        .parts
        .iter()
        .map(|p| p.ident.text.to_string())
        .collect();

    match parts.len() {
        0 => (String::new(), String::new()),
        1 => (parts[0].clone(), String::new()),
        _ => {
            let port = parts.last().cloned().unwrap_or_default();
            let comp = parts[..parts.len() - 1].join(".");
            (comp, port)
        }
    }
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
        let builder = ModelicaComponentBuilder::from_source(source).unwrap();
        let graph = builder
            .diagram_type(DiagramType::BlockDiagram)
            .target_class("RC_Circuit")
            .build();

        assert_eq!(graph.node_count(), 3, "Should have R1, C1, V1");
        assert_eq!(graph.edge_count(), 3, "Should have 3 connect() equations");

        // find_node searches by qualified_name
        assert!(graph.find_node("RC_Circuit.R1").is_some(), "Should have R1 node");
        assert!(graph.find_node("RC_Circuit.C1").is_some(), "Should have C1 node");
        assert!(graph.find_node("RC_Circuit.V1").is_some(), "Should have V1 node");
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
        let builder = ModelicaComponentBuilder::from_source(source).unwrap();
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
"#;
        let builder = ModelicaComponentBuilder::from_source(source).unwrap();
        let graph = builder
            .diagram_type(DiagramType::ConnectionDiagram)
            .target_class("RC_Circuit")
            .build();

        // Should have R1, C1, plus connector nodes (R1.p, R1.n, C1.p, C1.n)
        assert!(graph.node_count() >= 2, "Should have at least R1 and C1");
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
        let builder = ModelicaComponentBuilder::from_source(source).unwrap();
        let graph = builder
            .diagram_type(DiagramType::PackageHierarchy)
            .build();

        // Should have MyLib, Components, Resistor, Circuits, RC
        assert!(graph.node_count() >= 3);
    }

    #[test]
    fn test_parse_connect_reference() {
        use rumoca_session::parsing::ast::{ComponentRefPart, Token as AstToken};
        use std::sync::Arc;

        fn make_ref(parts: &[(&str, &str)]) -> rumoca_session::parsing::ast::ComponentReference {
            rumoca_session::parsing::ast::ComponentReference {
                local: false,
                parts: parts
                    .iter()
                    .map(|(_name, text)| ComponentRefPart {
                        ident: AstToken {
                            text: Arc::from(*text),
                            ..Default::default()
                        },
                        subs: None,
                    })
                    .collect(),
                def_id: None,
            }
        }

        let r = make_ref(&[("R1", "R1"), ("p", "p")]);
        let (node, port) = parse_connect_reference(&r);
        assert_eq!(node, "R1");
        assert_eq!(port, "p");

        let r = make_ref(&[("A", "A"), ("B", "B"), ("p", "p")]);
        let (node, port) = parse_connect_reference(&r);
        assert_eq!(node, "A.B");
        assert_eq!(port, "p");
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
        let builder = ModelicaComponentBuilder::from_source(source).unwrap();
        let graph = builder
            .diagram_type(DiagramType::BlockDiagram)
            .target_class("PID")
            .build();

        // Core regression: inherited connectors must show up as nodes
        // so wires targeting them have something to land on. Edge
        // count is not asserted — port-name matching for `connect(u, k.u)`
        // depends on per-type port introspection (see
        // `extract_component_ports` / `get_connector_port_names`),
        // which is a separate gap.
        assert!(graph.find_node("PID.u").is_some(), "u (inherited from SISO) must be a node");
        assert!(graph.find_node("PID.y").is_some(), "y (inherited from SISO) must be a node");
        assert!(graph.find_node("PID.k").is_some(), "k (direct) must be a node");
    }

    /// End-to-end check: load the real `Modelica.Blocks.Continuous.PID`
    /// from the MSL filesystem cache and confirm `extends`-walking
    /// surfaces the inherited `u`/`y` connectors. Gated on the cache
    /// being materialised so CI without MSL doesn't fail.
    #[test]
    fn test_real_msl_pid_has_inherited_u_y() {
        let Some(pid) = crate::class_cache::peek_or_load_msl_class(
            "Modelica.Blocks.Continuous.PID",
        ) else {
            eprintln!("MSL cache not materialised — skipping");
            return;
        };
        let ast = StoredDefinition::default();
        let inherited = collect_inherited_components_with(
            &pid,
            Some("Modelica.Blocks.Continuous.PID"),
            &ast,
            0,
            &crate::class_cache::peek_or_load_msl_class,
        );
        let names: Vec<&str> = inherited.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"u"), "PID must inherit u from SISO; got {:?}", names);
        assert!(names.contains(&"y"), "PID must inherit y from SISO; got {:?}", names);
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
