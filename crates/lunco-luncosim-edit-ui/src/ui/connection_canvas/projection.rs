//! Pure projector: a composed USD stage → a `lunco_canvas::Scene`.
//!
//! Collection and pure projection are split so layout is testable without a
//! live stage:
//!
//! - [`collect_graph`] reads the complete live `StageView` over the canonical
//!   stage into plain [`PrimNode`] / [`Wire`] structs. Thin glue over the same
//!   read API + connection-string split the co-sim wiring derivation uses
//!   (`lunco_usd_sim_cosim::rewire_usd_connections`).
//! - [`project_diagram`] scopes complete composed topology to USD ancestry.
//! - [`project_schema`] is an explicit presentation projection for the Lunica
//!   Schema perspective. It is driven by authored USD properties and never
//!   changes the collected topology used by simulation.
//! - [`build_scene`] is a **pure function** `(nodes, wires) → Scene`: it filters
//!   the selected hierarchy and wiring participants, assigns a
//!   left-to-right dataflow layering, lays out ports, and emits nodes + edges.
//!   No USD, no Bevy — unit-tested directly.
//!
//! # What becomes a node vs an edge
//!
//! - **Node** — a USD hierarchy boundary or a prim that has connectors (`inputs:*` /
//!   `outputs:*` / `connectors:*`) or is a rigid body (`PhysicsRigidBodyAPI`). A scene may
//!   author `lunco:ui:schemaNode = true` on system boundaries; the explicit
//!   [`project_schema`] function can then select those boundaries for the
//!   readable schema projection.
//! - **Causal edge** — a property connection between `inputs:`/`outputs:`,
//!   including boundary forwarding. Drawn from USD source to target.
//! - **Acausal edge** — a `connectors:*` property connection, drawn undirected.
//! - **Joint edge** — one per prim carrying both `physics:body0` and
//!   `physics:body1`, connecting its two bodies. The joint prim also remains a
//!   node for hierarchy navigation and its own signal interfaces.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use lunco_canvas::{Edge, Node, Port, PortId, PortRef, Pos, Rect, Scene};
use lunco_usd_bevy_stage::{StageView, UsdRead};
use openusd::sdf::Path as SdfPath;

/// Node kind id registered in the canvas `VisualRegistry`.
pub(crate) const NODE_KIND: &str = "usd.prim";
/// Edge kind id registered in the canvas `VisualRegistry`.
pub(crate) const EDGE_KIND: &str = "usd.wire";

// Layout constants (world units). A node is a fixed card; ranks march right,
// rows march down. Wide enough to fit a prim leaf name + type label.
const NODE_W: f32 = 250.0;
const NODE_H: f32 = 96.0;
pub(super) const PORT_ROW_H: f32 = 19.0;
pub(super) const COL_SPACING: f32 = 360.0;
const ROW_SPACING: f32 = 230.0;
const ROW_GAP: f32 = 56.0;
pub(super) const MARGIN: f32 = 40.0;

/// Whether a wire is a co-sim dataflow connection or a physics joint.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub(crate) enum WireKind {
    /// Authored `inputs:<c>.connect` — a co-sim signal wire.
    Dataflow,
    /// Authored `connectors:*` connection — an undirected Modelica network.
    Acausal,
    /// A joint prim's `physics:body0` ↔ `physics:body1`.
    Joint,
}

/// Typed payload carried in `Node.data` for `"usd.prim"` nodes; the visual
/// factory downcasts it.
#[derive(Clone, Debug)]
pub(crate) struct UsdPrimNodeData {
    pub group_id: Option<String>,
    pub group_ports: BTreeMap<String, super::groups::GroupEndpoint>,
    pub programs: Vec<ProgramFacet>,
    pub accent: Option<DiagramAccent>,
    pub type_name: String,
    /// Applies `PhysicsRigidBodyAPI` — drawn with the body accent.
    pub is_body: bool,
    /// USD property types used by the document authoring boundary.
    pub port_types: BTreeMap<String, String>,
    pub port_sources: BTreeMap<String, Vec<String>>,
    /// Stable presentation identity; origin remains the exact USD prim.
    pub view_key: String,
    pub boundary: Option<BoundaryRole>,
}

/// Presentation roles resolved through the existing schematic theme tokens.
#[derive(Clone, Copy, Debug)]
pub(crate) enum DiagramAccent {
    Model,
    Block,
    Record,
    Package,
    Class,
    Warning,
}
impl DiagramAccent {
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Model => "model",
            Self::Block => "block",
            Self::Record => "record",
            Self::Package => "package",
            Self::Class => "class",
            Self::Warning => "warning",
        }
    }
}

/// The selected system's property groups, presented as interface terminals.
/// These describe USD direction, not a guessed runtime provider.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub(crate) enum BoundaryRole {
    Inputs,
    Outputs,
    Connectors,
}
impl BoundaryRole {
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Inputs => "inputs",
            Self::Outputs => "outputs",
            Self::Connectors => "connectors",
        }
    }
}

pub(super) fn diagram_key(node: &Node) -> Option<&str> {
    node.data
        .downcast_ref::<UsdPrimNodeData>()
        .map(|data| data.view_key.as_str())
}

/// Typed payload carried in `Edge.data` for `"usd.wire"` edges.
#[derive(Clone, Debug)]
pub(crate) struct UsdWireData {
    pub kind: WireKind,
}

/// A prim read out of the stage, before layout.
#[derive(Clone, Debug)]
pub(crate) struct PrimNode {
    pub collections: BTreeMap<String, Result<Vec<String>, String>>,
    pub programs: Vec<ProgramFacet>,
    pub variants: Result<BTreeMap<String, String>, String>,
    pub usd_origin: Option<String>,
    pub boundary: Option<BoundaryRole>,
    pub path: String,
    /// Standard USD `ui:displayName`, when authored; the path leaf is the
    /// deterministic fallback for assets that do not provide one.
    pub display_name: Option<String>,
    pub type_name: String,
    pub is_body: bool,
    /// Typed USD presentation markers copied from the composed stage. They
    /// are consumed only by [`project_schema`].
    pub schema_root: bool,
    pub schema_node: bool,
    /// Optional authored presentation column/row.  These are USD layout
    /// properties, not an engine-side classification of the prim.
    pub schema_column: Option<i32>,
    pub schema_row: Option<i32>,

    pub port_sources: BTreeMap<String, Vec<String>>,
    /// Explicit USD references to interfaces supplied by a registered runtime provider.
    pub referenced_ports: BTreeSet<String>,
    /// Connector leaf names (no `inputs:` prefix).
    pub inputs: Vec<String>,
    /// Connector leaf names (no `outputs:` prefix).
    pub outputs: Vec<String>,
    /// Acausal connector leaves, kept separate from causal inputs/outputs.
    pub connectors: Vec<String>,
    /// Declared types keyed by complete USD property identity.
    pub port_types: BTreeMap<String, String>,
}

/// Resolved authored program facts; this never starts an executor or reads source bytes.
#[derive(Clone, Debug, Hash)]
pub(crate) struct ProgramFacet {
    pub path: String,
    pub backend: String,
    pub source: String,
    pub issue: Option<String>,
}

/// A link read out of the stage, before resolution against the node set.
#[derive(Clone, Debug, Hash)]
pub(crate) struct Wire {
    pub kind: WireKind,
    /// Prim whose authored relation produced this wire. This keeps incremental
    /// projection correct for joints, whose endpoints are not the joint prim.
    pub owner_path: String,
    pub source_path: String,
    /// Full source USD property name (`outputs:`, `inputs:`, or `connectors:`).
    /// Empty for joints.
    pub source_conn: String,
    pub target_path: String,
    /// Full target USD property name. Empty for joints.
    pub target_conn: String,
}

/// Collected graph facts for one active scene prim. An active prim with no
/// graph facts is still represented so the canvas can retain its complete
/// entity-backed path index.
pub(crate) struct PrimProjection {
    pub node: Option<PrimNode>,
    pub wires: Vec<Wire>,
}

/// Read every prim in `prim_paths` + its connections out of a composed stage.
///
/// `prim_paths` are the scene's prim path strings — supplied by the caller from
/// the ECS `UsdPrimPath` entities, exactly the enumeration
/// `rewire_usd_connections` uses. The caller supplies complete composed prim
/// paths, including prims without ECS projections. `inputs:<c>` attrs are sinks, their `connections()` are the
/// producers, split at the last `.` into `(prim, property-name)`. A prim
/// carrying both joint bodies also contributes a mechanical joint wire.
pub(crate) fn collect_graph(
    view: &StageView<'_>,
    prim_paths: &[String],
) -> (Vec<PrimNode>, Vec<Wire>) {
    let mut nodes: Vec<PrimNode> = Vec::new();
    let mut wires: Vec<Wire> = Vec::new();

    for path in prim_paths {
        let Some(mut projection) = collect_prim(view, path) else {
            continue;
        };
        if let Some(node) = projection.node {
            nodes.push(node);
        }
        wires.append(&mut projection.wires);
    }

    (nodes, wires)
}

/// Read one changed prim and the wires authored by that prim.
pub(crate) fn collect_prim(view: &StageView<'_>, path: &str) -> Option<PrimProjection> {
    let Ok(p) = SdfPath::new(path) else {
        return None;
    };
    if !view.is_active(&p) {
        return None;
    }
    let mut wires = Vec::new();

    // A prim with both bodies is a joint: render it as an edge between the
    // two bodies. Keep the joint prim available for referenced value ports.
    let body0 = view.rel_target(&p, "physics:body0");
    let body1 = view.rel_target(&p, "physics:body1");
    if let (Some(a), Some(b)) = (body0, body1) {
        wires.push(Wire {
            kind: WireKind::Joint,
            owner_path: path.to_string(),
            source_path: a,
            source_conn: String::new(),
            target_path: b,
            target_conn: String::new(),
        });
    }

    let type_name = view.type_name(&p).unwrap_or_default();
    let display_name = view
        .text(&p, "ui:displayName")
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty());
    let is_body = view.has_api_schema(&p, "PhysicsRigidBodyAPI");
    let schema_root = view.boolean(&p, "lunco:ui:schemaRoot") == Some(true);
    let schema_node = view.boolean(&p, "lunco:ui:schemaNode") == Some(true);
    let schema_column = view.scalar::<i32>(&p, "lunco:ui:schemaColumn");
    let schema_row = view.scalar::<i32>(&p, "lunco:ui:schemaRow");
    let mut inputs: Vec<String> = Vec::new();
    let mut outputs: Vec<String> = Vec::new();
    let mut connectors = Vec::new();
    let mut port_types = BTreeMap::new();
    let mut port_sources = BTreeMap::new();

    for attr in view.attr_names(&p) {
        let kind = if let Some(conn) = attr.strip_prefix("inputs:") {
            inputs.push(conn.to_string());
            WireKind::Dataflow
        } else if let Some(conn) = attr.strip_prefix("outputs:") {
            outputs.push(conn.to_string());
            WireKind::Dataflow
        } else if let Some(conn) = attr.strip_prefix("connectors:") {
            connectors.push(conn.to_string());
            WireKind::Acausal
        } else {
            continue;
        };
        if let Some(type_name) = view.attr_type_name(&p, &attr) {
            port_types.insert(attr.clone(), type_name);
        }
        let sources = view.connections(&p, &attr);
        port_sources.insert(attr.clone(), sources.clone());
        for source in sources {
            let Some((source_path, property)) = source.rsplit_once('.') else {
                continue;
            };
            let valid_property = match kind {
                WireKind::Dataflow => {
                    property.starts_with("inputs:") || property.starts_with("outputs:")
                }
                WireKind::Acausal => property.starts_with("connectors:"),
                WireKind::Joint => false,
            };
            if !valid_property {
                continue;
            }
            wires.push(Wire {
                kind,
                owner_path: path.to_string(),
                source_path: source_path.to_string(),
                source_conn: property.to_string(),
                target_path: path.to_string(),
                target_conn: attr.clone(),
            });
        }
    }

    let programs = if view.has_api_schema(&p, "LunCoProgramAPI") {
        use lunco_usd_bevy_core::program::{ProgramSource, resolve_program};
        vec![match resolve_program(view, &p) {
            Ok(program) => ProgramFacet {
                path: path.into(),
                backend: format!("{:?}", program.backend),
                source: match program.source {
                    ProgramSource::Id(id) | ProgramSource::Asset(id) => id,
                    ProgramSource::Code(_) => "Inline source".into(),
                },
                issue: None,
            },
            Err(issue) => ProgramFacet {
                path: path.into(),
                backend: "Invalid program".into(),
                source: issue.property,
                issue: Some(issue.message),
            },
        }]
    } else {
        Vec::new()
    };
    let node = PrimNode {
        collections: view
            .api_schemas(&p)
            .into_iter()
            .filter_map(|schema| {
                let name = schema.strip_prefix("CollectionAPI:")?;
                Some((
                    name.to_string(),
                    view.collection_members(&p, name)
                        .map(|paths| paths.into_iter().map(|p| p.to_string()).collect())
                        .map_err(|error| error.to_string()),
                ))
            })
            .collect(),
        programs,
        variants: view
            .stage()
            .prim(p.clone())
            .variant_sets()
            .get_all_variant_selections()
            .map(|selections| {
                selections
                    .into_iter()
                    .map(|(name, selection)| (name.to_string(), selection.to_string()))
                    .collect()
            })
            .map_err(|error| error.to_string()),
        usd_origin: None,
        boundary: None,
        path: path.to_string(),
        display_name,
        type_name,
        is_body,
        schema_root,
        schema_node,
        schema_column,
        schema_row,

        inputs,
        outputs,
        connectors,
        port_types,
        port_sources,
        referenced_ports: BTreeSet::new(),
    };
    Some(PrimProjection {
        node: Some(node),
        wires,
    })
}

/// Replace graph facts owned by the affected USD paths, preserving all
/// unaffected cached facts. Structural roots invalidate their whole subtree;
/// info-only changes invalidate only the exact prim.
pub(crate) fn replace_affected_projection(
    nodes: &mut Vec<PrimNode>,
    wires: &mut Vec<Wire>,
    resynced_roots: &[String],
    info_paths: &[String],
    invalidated_wire_owners: &BTreeSet<String>,
    replacement_nodes: Vec<PrimNode>,
    replacement_wires: Vec<Wire>,
) {
    nodes.retain(|node| {
        !resynced_roots
            .iter()
            .any(|root| super::path_is_within(&node.path, root))
            && !info_paths.iter().any(|path| path == &node.path)
    });
    wires.retain(|wire| {
        !resynced_roots
            .iter()
            .any(|root| super::path_is_within(&wire.owner_path, root))
            && !info_paths.iter().any(|path| path == &wire.owner_path)
            && !invalidated_wire_owners.contains(&wire.owner_path)
    });
    nodes.extend(replacement_nodes);
    wires.extend(replacement_wires);
}

/// Find authored relation owners that must be reread after an endpoint changes.
/// A deleted body, for example, does not structurally change the joint prim
/// whose relationship still names it.
pub(crate) fn wire_owners_affected_by_paths(
    wires: &[Wire],
    resynced_roots: &[String],
    info_paths: &[String],
) -> BTreeSet<String> {
    wires
        .iter()
        .filter(|wire| {
            resynced_roots.iter().any(|root| {
                super::path_is_within(&wire.source_path, root)
                    || super::path_is_within(&wire.target_path, root)
            }) || info_paths
                .iter()
                .any(|path| path == &wire.source_path || path == &wire.target_path)
        })
        .map(|wire| wire.owner_path.clone())
        .collect()
}

/// Select the authored system boundaries for the Connections view.
///
/// This is intentionally separate from [`collect_graph`]. The canonical USD
/// graph must remain complete for simulation, editing, diagnostics, and any
/// future full-topology view. A schema projection is only a readable boundary
/// view: `lunco:ui:schemaRoot` scopes one instance and
/// `lunco:ui:schemaNode` marks the blocks that should be shown. Both are typed
/// USD properties; no path/name classification is performed here.
pub(crate) fn schema_roots(nodes: &[PrimNode]) -> Vec<String> {
    nodes
        .iter()
        .filter(|node| node.schema_root)
        .map(|node| node.path.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

pub(crate) fn project_schema(
    nodes: &[PrimNode],
    wires: &[Wire],
    schema_root: &str,
) -> (Vec<PrimNode>, Vec<Wire>) {
    let marked: BTreeSet<String> = nodes
        .iter()
        .filter(|node| {
            node.schema_node
                && (node.path == schema_root
                    || node
                        .path
                        .strip_prefix(schema_root)
                        .is_some_and(|rest| rest.starts_with('/')))
        })
        .map(|node| node.path.clone())
        .collect();

    let mut nodes: Vec<PrimNode> = nodes
        .iter()
        .filter(|node| marked.contains(&node.path))
        .cloned()
        .collect();
    let wires: Vec<Wire> = wires
        .iter()
        .filter(|wire| {
            // A same-prim forwarding binding is valid runtime topology, but not a
            // connection between two blocks. Hide it only in this presentation.
            wire.source_path != wire.target_path
                && marked.contains(&wire.source_path)
                && marked.contains(&wire.target_path)
        })
        .cloned()
        .collect();

    let connected: BTreeSet<_> = wires
        .iter()
        .flat_map(|wire| {
            [
                (&wire.source_path, &wire.source_conn),
                (&wire.target_path, &wire.target_conn),
            ]
        })
        .collect();
    for node in &mut nodes {
        node.inputs
            .retain(|name| connected.contains(&(&node.path, &format!("inputs:{name}"))));
        node.outputs
            .retain(|name| connected.contains(&(&node.path, &format!("outputs:{name}"))));
        node.connectors
            .retain(|name| connected.contains(&(&node.path, &format!("connectors:{name}"))));
    }

    (nodes, wires)
}

/// Inspect the composed topology within an explicit hierarchy scope. Unlike
/// the authored schema presentation, this retains unconnected interface ports.
pub(crate) fn project_diagram(
    nodes: &[PrimNode],
    wires: &[Wire],
    root: &str,
    include_descendants: bool,
) -> (Vec<PrimNode>, Vec<Wire>) {
    let programs: Vec<_> = nodes
        .iter()
        .flat_map(|node| node.programs.iter().cloned())
        .collect();
    let mut nodes: Vec<_> = nodes
        .iter()
        .filter(|node| {
            let parent = node
                .path
                .rsplit_once('/')
                .map(|(parent, _)| if parent.is_empty() { "/" } else { parent });
            super::path_is_within(&node.path, root)
                && (node.path == root || parent == Some(root) || include_descendants)
        })
        .cloned()
        .collect();
    // Attribute hidden program descendants to their nearest displayed USD ancestor.
    let visible: HashMap<_, _> = nodes
        .iter()
        .enumerate()
        .map(|(i, node)| (node.path.clone(), i))
        .collect();
    for node in nodes.iter_mut() {
        node.programs.clear();
    }
    for program in programs {
        let mut ancestor = program.path.as_str();
        loop {
            if let Some(index) = visible.get(ancestor) {
                nodes[*index].programs.push(program);
                break;
            }
            let Some((parent, _)) = ancestor.rsplit_once('/') else {
                break;
            };
            if parent.is_empty() {
                break;
            }
            ancestor = parent;
        }
    }
    let paths: BTreeSet<_> = nodes.iter().map(|node| node.path.as_str()).collect();
    let mut physical = BTreeSet::new();
    let mut wires: Vec<Wire> = wires
        .iter()
        .filter(|wire| {
            if !paths.contains(wire.source_path.as_str())
                || !paths.contains(wire.target_path.as_str())
            {
                return false;
            }
            if wire.kind != WireKind::Acausal {
                return true;
            }
            let a = (&wire.source_path, &wire.source_conn);
            let b = (&wire.target_path, &wire.target_conn);
            let (a, b) = if a <= b { (a, b) } else { (b, a) };
            physical.insert((a, b))
        })
        .cloned()
        .collect();
    // The selected system is an interface, rather than a second copy of its
    // children. Separate input/output terminals retain exact property names.
    if let Some(index) = nodes.iter().position(|node| node.path == root) {
        let system = nodes.remove(index);
        let mut boundaries = BTreeMap::new();
        for role in [
            BoundaryRole::Inputs,
            BoundaryRole::Outputs,
            BoundaryRole::Connectors,
        ] {
            let prefix = format!("{}:", role.name());
            let has_ports = match role {
                BoundaryRole::Inputs => !system.inputs.is_empty(),
                BoundaryRole::Outputs => !system.outputs.is_empty(),
                BoundaryRole::Connectors => !system.connectors.is_empty(),
            } || system
                .referenced_ports
                .iter()
                .any(|port| port.starts_with(&prefix));
            if !has_ports {
                continue;
            }
            let mut terminal = system.clone();
            terminal.path = format!("{root}#{}", role.name());
            terminal.usd_origin = Some(root.into());
            terminal.boundary = Some(role);
            terminal.display_name = Some(format!(
                "{} · {}",
                system
                    .display_name
                    .as_deref()
                    .unwrap_or_else(|| root.rsplit('/').next().unwrap_or(root)),
                role.name()
            ));
            terminal.programs.clear();
            terminal.is_body = false;
            terminal.type_name = "System interface".into();
            if role != BoundaryRole::Inputs {
                terminal.inputs.clear();
            }
            if role != BoundaryRole::Outputs {
                terminal.outputs.clear();
            }
            if role != BoundaryRole::Connectors {
                terminal.connectors.clear();
            }
            terminal
                .referenced_ports
                .retain(|port| port.starts_with(&prefix));
            terminal
                .port_types
                .retain(|port, _| port.starts_with(&prefix));
            terminal
                .port_sources
                .retain(|port, _| port.starts_with(&prefix));
            boundaries.insert(prefix, terminal.path.clone());
            nodes.push(terminal);
        }
        // A system may itself run a program or participate in a joint. Retain
        // its structural card for those facts, without duplicating its ports.
        if (boundaries.is_empty() && nodes.is_empty())
            || !system.programs.is_empty()
            || wires.iter().any(|wire| {
                wire.kind == WireKind::Joint
                    && (wire.source_path == root || wire.target_path == root)
            })
        {
            let mut host = system;
            host.inputs.clear();
            host.outputs.clear();
            host.connectors.clear();
            host.referenced_ports.clear();
            nodes.push(host);
        }
        for wire in &mut wires {
            if wire.kind == WireKind::Joint {
                continue;
            }
            for (path, property) in [
                (&mut wire.source_path, &wire.source_conn),
                (&mut wire.target_path, &wire.target_conn),
            ] {
                if path == root {
                    if let Some((prefix, _)) = property.split_once(':') {
                        if let Some(key) = boundaries.get(&format!("{prefix}:")) {
                            *path = key.clone();
                        }
                    }
                }
            }
        }
    }
    (nodes, wires)
}

/// Turn read prims + wires into a laid-out canvas [`Scene`]. Pure.
///
/// Lays out the supplied USD prims left-to-right by dataflow rank, and emits one canvas
/// node per prim with its declared interfaces and one edge per resolvable wire.
pub(crate) fn build_scene(nodes: Vec<PrimNode>, wires: Vec<Wire>) -> Scene {
    // Relevant = wiring-visible prims. Traversal order is preserved (stable,
    // deterministic layout across rebuilds).
    let relevant = nodes;
    let n = relevant.len();

    let index: HashMap<String, usize> = relevant
        .iter()
        .enumerate()
        .map(|(i, node)| (node.path.clone(), i))
        .collect();

    // Drop wires whose endpoints aren't both nodes (e.g. a joint body that got
    // filtered, or a source prim not yet spawned).
    let wires: Vec<Wire> = wires
        .into_iter()
        .filter(|w| index.contains_key(&w.source_path) && index.contains_key(&w.target_path))
        .collect();

    // Port declarations are authoritative. Do not manufacture an interface for
    // a malformed connection; the topology lint owns those diagnostics.
    let in_ports: Vec<BTreeSet<String>> = relevant
        .iter()
        .map(|node| {
            node.inputs
                .iter()
                .cloned()
                .chain(
                    node.referenced_ports
                        .iter()
                        .filter_map(|name| name.strip_prefix("inputs:").map(str::to_string)),
                )
                .collect()
        })
        .collect();
    let out_ports: Vec<BTreeSet<String>> = relevant
        .iter()
        .map(|node| {
            node.outputs
                .iter()
                .cloned()
                .chain(
                    node.referenced_ports
                        .iter()
                        .filter_map(|name| name.strip_prefix("outputs:").map(str::to_string)),
                )
                .collect()
        })
        .collect();
    let acausal_ports: Vec<BTreeSet<String>> = relevant
        .iter()
        .map(|node| node.connectors.iter().cloned().collect())
        .collect();
    // Condense causal cycles before ranking so feedback networks remain
    // bounded and keep their downstream models in separate columns.
    let rank = dataflow_ranks(n, &wires, &index);

    let node_heights: Vec<f32> = (0..n)
        .map(|i| {
            let port_count =
                (in_ports[i].len() + acausal_ports[i].len()).max(out_ports[i].len()) as f32;
            (NODE_H).max(46.0 + port_count * PORT_ROW_H)
        })
        .collect();

    // Position: authored schema columns/rows win. Unauthored nodes use the
    // deterministic dataflow rank and stable traversal order as a useful
    // fallback for generic scenes. Row spacing is expanded by the tallest card
    // in the previous authored row; a large port contract must never cover the
    // card below it.
    let mut rows_per_column: HashMap<i32, u32> = HashMap::new();
    let mut columns_rows: Vec<(i32, i32)> = Vec::with_capacity(n);
    let mut row_heights: BTreeMap<(i32, i32), f32> = BTreeMap::new();
    for i in 0..n {
        let column = relevant[i].schema_column.unwrap_or(rank[i]).max(0);
        let row = relevant[i].schema_row.unwrap_or_else(|| {
            let row = *rows_per_column.get(&column).unwrap_or(&0);
            rows_per_column.insert(column, row + 1);
            row as i32
        });
        let row = row.max(0);
        columns_rows.push((column, row));
        row_heights
            .entry((column, row))
            .and_modify(|height| *height = height.max(node_heights[i]))
            .or_insert(node_heights[i]);
    }
    // Prefix sums visit each occupied row once. Sparse authored row numbers
    // are accounted for arithmetically, without looping over empty rows.
    let mut row_positions = HashMap::with_capacity(row_heights.len());
    let mut previous_column = None;
    let mut next_row = 0i64;
    let mut y = MARGIN;
    for (&(column, row), &height) in &row_heights {
        if previous_column != Some(column) {
            y = MARGIN;
            next_row = 0;
            previous_column = Some(column);
        }
        y += (i64::from(row) - next_row) as f32 * (ROW_SPACING + ROW_GAP);
        row_positions.insert((column, row), y);
        y += height + ROW_GAP;
        next_row = i64::from(row) + 1;
    }
    let positions: Vec<_> = columns_rows
        .iter()
        .map(|&(column, row)| {
            Pos::new(
                MARGIN + column as f32 * COL_SPACING,
                row_positions[&(column, row)],
            )
        })
        .collect();

    let mut scene = Scene::new();
    let mut node_ids = Vec::with_capacity(n);
    for i in 0..n {
        let node = &relevant[i];
        let rect = Rect::from_min_size(positions[i], NODE_W, node_heights[i]);
        let mut ports: Vec<Port> = Vec::new();

        let ins: Vec<&String> = in_ports[i].iter().collect();
        for (k, name) in ins.iter().enumerate() {
            ports.push(Port {
                id: PortId::new(format!("inputs:{name}")),
                local_offset: Pos::new(
                    if node.boundary == Some(BoundaryRole::Inputs) {
                        NODE_W
                    } else {
                        0.0
                    },
                    port_y(k, ins.len(), node_heights[i]),
                ),
                kind: "input".into(),
            });
        }
        let outs: Vec<&String> = out_ports[i].iter().collect();
        for (k, name) in outs.iter().enumerate() {
            ports.push(Port {
                id: PortId::new(format!("outputs:{name}")),
                local_offset: Pos::new(
                    if node.boundary == Some(BoundaryRole::Outputs) {
                        0.0
                    } else {
                        NODE_W
                    },
                    port_y(k, outs.len(), node_heights[i]),
                ),
                kind: "output".into(),
            });
        }

        for (k, name) in acausal_ports[i].iter().enumerate() {
            ports.push(Port {
                id: PortId::new(format!("connectors:{name}")),
                local_offset: Pos::new(
                    0.0,
                    port_y(
                        k + ins.len(),
                        ins.len() + acausal_ports[i].len(),
                        node_heights[i],
                    ),
                ),
                kind: "acausal".into(),
            });
        }
        // Hidden joint anchors — `~jr` (right) sources a joint edge, `~jl` (left)
        // sinks it. Prefixed `~` so the visual skips painting them. Present on
        // every node so any joint edge resolves.
        ports.push(Port {
            id: PortId::new("~jr"),
            local_offset: Pos::new(NODE_W, node_heights[i] * 0.5),
            kind: "joint".into(),
        });
        ports.push(Port {
            id: PortId::new("~jl"),
            local_offset: Pos::new(0.0, node_heights[i] * 0.5),
            kind: "joint".into(),
        });

        let leaf = node
            .path
            .rsplit('/')
            .next()
            .unwrap_or(&node.path)
            .to_string();
        let id = scene.alloc_node_id();
        scene.insert_node(Node {
            id,
            rect,
            kind: NODE_KIND.into(),
            data: Arc::new(UsdPrimNodeData {
                group_id: None,
                group_ports: Default::default(),
                programs: node.programs.clone(),
                accent: None,
                type_name: node.type_name.clone(),
                is_body: node.is_body,
                port_types: node.port_types.clone(),

                port_sources: node.port_sources.clone(),
                view_key: node.path.clone(),
                boundary: node.boundary,
            }),
            ports,
            label: node.display_name.clone().unwrap_or(leaf),
            origin: Some(node.usd_origin.as_ref().unwrap_or(&node.path).clone()),
            resizable: false,
            visual_rect: None,
        });
        node_ids.push(id);
    }

    for w in &wires {
        let (s, t) = (index[&w.source_path], index[&w.target_path]);
        let (source_port, target_port) = if w.kind == WireKind::Joint {
            ("~jr", "~jl")
        } else {
            (w.source_conn.as_str(), w.target_conn.as_str())
        };
        let endpoint = |node, name| {
            let node = scene.node(node)?;
            let port = node.ports.iter().find(|port| port.id.as_str() == name)?;
            Some(port.world_pos(node.rect))
        };
        let (Some(_), Some(_)) = (
            endpoint(node_ids[s], source_port),
            endpoint(node_ids[t], target_port),
        ) else {
            continue;
        };
        let from = PortRef {
            node: node_ids[s],
            port: PortId::new(source_port),
        };
        let to = PortRef {
            node: node_ids[t],
            port: PortId::new(target_port),
        };
        let eid = scene.alloc_edge_id();
        scene.insert_edge(Edge {
            id: eid,
            from,
            to,
            kind: EDGE_KIND.into(),
            data: Arc::new(UsdWireData { kind: w.kind }),
            origin: None,
            waypoints: Vec::new(),
            waypoints_authored: false,
        });
    }

    route_edges(&mut scene);
    scene
}

/// Route forward dataflow through the gap between endpoint columns.
fn orthogonal_waypoints(from: Pos, to: Pos) -> Vec<Pos> {
    let mid_x = from.x + (to.x - from.x) * 0.5;
    vec![Pos::new(mid_x, from.y), Pos::new(mid_x, to.y)]
}

/// Longest-path ranks of the dataflow graph after collapsing strongly
/// connected components.  A feedback loop is one logical component, while
/// components downstream of it still receive a meaningful left-to-right rank.
fn dataflow_ranks(node_count: usize, wires: &[Wire], index: &HashMap<String, usize>) -> Vec<i32> {
    if node_count == 0 {
        return Vec::new();
    }

    let mut graph = vec![Vec::<usize>::new(); node_count];
    let mut reverse = vec![Vec::<usize>::new(); node_count];
    for wire in wires {
        if wire.kind != WireKind::Dataflow {
            continue;
        }
        let (Some(&source), Some(&target)) =
            (index.get(&wire.source_path), index.get(&wire.target_path))
        else {
            continue;
        };
        graph[source].push(target);
        reverse[target].push(source);
    }

    // Iterative DFS keeps long authored chains off the process call stack.
    fn visit(node: usize, graph: &[Vec<usize>], seen: &mut [bool], order: &mut Vec<usize>) {
        if seen[node] {
            return;
        }
        seen[node] = true;
        let mut stack = vec![(node, 0)];
        while let Some((current, next)) = stack.last_mut() {
            if let Some(&child) = graph[*current].get(*next) {
                *next += 1;
                if !seen[child] {
                    seen[child] = true;
                    stack.push((child, 0));
                }
            } else {
                order.push(*current);
                stack.pop();
            }
        }
    }

    fn assign(node: usize, component: usize, reverse: &[Vec<usize>], components: &mut [usize]) {
        let mut stack = vec![node];
        components[node] = component;
        while let Some(current) = stack.pop() {
            for &next in &reverse[current] {
                if components[next] == usize::MAX {
                    components[next] = component;
                    stack.push(next);
                }
            }
        }
    }

    let mut seen = vec![false; node_count];
    let mut order = Vec::with_capacity(node_count);
    for node in 0..node_count {
        visit(node, &graph, &mut seen, &mut order);
    }

    let mut components = vec![usize::MAX; node_count];
    let mut component_count = 0;
    for &node in order.iter().rev() {
        if components[node] == usize::MAX {
            assign(node, component_count, &reverse, &mut components);
            component_count += 1;
        }
    }

    let mut condensation = BTreeSet::<(usize, usize)>::new();
    let mut indegree = vec![0usize; component_count];
    for source in 0..node_count {
        for &target in &graph[source] {
            let from = components[source];
            let to = components[target];
            if from != to && condensation.insert((from, to)) {
                indegree[to] += 1;
            }
        }
    }

    let mut ready = BTreeSet::new();
    for (component, &degree) in indegree.iter().enumerate() {
        if degree == 0 {
            ready.insert(component);
        }
    }
    let mut component_rank = vec![0i32; component_count];
    while let Some(component) = ready.pop_first() {
        for &(from, to) in condensation.range((component, 0)..=(component, usize::MAX)) {
            debug_assert_eq!(from, component);
            component_rank[to] = component_rank[to].max(component_rank[from] + 1);
            indegree[to] -= 1;
            if indegree[to] == 0 {
                ready.insert(to);
            }
        }
    }

    components
        .into_iter()
        .map(|component| component_rank[component])
        .collect()
}

/// Even vertical distribution of `count` ports down a node's dynamic edge:
/// port `k` sits at `H·(k+1)/(count+1)`.
fn port_y(k: usize, count: usize, height: f32) -> f32 {
    40.0 + PORT_ROW_H
        * (k as f32 + 0.5)
            .min((count.max(1) as f32) - 0.5)
            .min(height - 46.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn document_connections_preserve_authored_types_and_lists() {
        use super::super::{build_ops, connect_op, edge_sink};
        use lunco_canvas::SceneEvent;
        use lunco_usd_document::document::{LayerId, UsdOp};
        let a = prim("/A", &[], &["out"], false);
        let mut b = prim("/B", &["in"], &[], false);
        b.port_sources.insert(
            "inputs:in".into(),
            vec!["/Other.outputs:out".into(), "/A.outputs:out".into()],
        );
        let scene = build_scene(vec![a, b], vec![dataflow("/A", "out", "/B", "in")]);
        let (edge_id, edge) = scene.edges().next().unwrap();
        let op = connect_op(&scene, &edge.from, &edge.to, &LayerId::root()).unwrap();
        assert!(
            matches!(op, UsdOp::SetConnection { type_name, sources, .. } if type_name == "double" && sources.len() == 2)
        );
        let sinks = HashMap::from([(*edge_id, edge_sink(&scene, *edge_id).unwrap())]);
        let ops = build_ops(
            &scene,
            &HashMap::new(),
            &sinks,
            &[SceneEvent::EdgeDeleted { id: *edge_id }],
            &LayerId::root(),
        )
        .unwrap();
        assert!(
            matches!(&ops[0], UsdOp::SetConnection { sources, .. } if sources == &vec!["/Other.outputs:out".to_string()])
        );
        let mut invalid = scene.clone();
        invalid.node_mut(edge.to.node).unwrap().data = Arc::new(UsdPrimNodeData {
            group_id: None,
            group_ports: Default::default(),
            programs: Vec::new(),
            accent: None,
            type_name: "Xform".into(),
            is_body: false,
            port_types: BTreeMap::from([("inputs:in".into(), "float".into())]),
            port_sources: Default::default(),
            view_key: "/Sink".into(),
            boundary: None,
        });
        assert!(connect_op(&invalid, &edge.from, &edge.to, &LayerId::root()).is_err());
    }

    #[test]
    fn diagram_scope_keeps_acausal_ports_and_excludes_siblings() {
        let mut a = prim("/System/A", &[], &["signal"], false);
        a.connectors = vec!["p".into()];
        let mut b = prim("/System/B", &["signal"], &[], false);
        b.connectors = vec!["p".into()];
        let mut physical = dataflow("/System/A", "p", "/System/B", "p");
        physical.kind = WireKind::Acausal;
        physical.source_conn = "connectors:p".into();
        physical.target_conn = "connectors:p".into();
        let mut reciprocal = physical.clone();
        std::mem::swap(&mut reciprocal.source_path, &mut reciprocal.target_path);
        std::mem::swap(&mut reciprocal.source_conn, &mut reciprocal.target_conn);
        let (nodes, wires) = project_diagram(
            &[a, b, prim("/SystemTwo/C", &["signal"], &[], false)],
            &[
                physical,
                reciprocal,
                dataflow("/System/A", "signal", "/System/B", "signal"),
            ],
            "/System",
            true,
        );
        let scene = build_scene(nodes, wires);
        assert_eq!(scene.node_count(), 2);
        assert_eq!(scene.edge_count(), 2);
        for (_, edge) in scene.edges() {
            assert!(scene.edge_endpoint_positions(edge).is_some());
            if edge.data.downcast_ref::<UsdWireData>().unwrap().kind == WireKind::Acausal {
                assert_eq!(edge.from.port.as_str(), "connectors:p");
                assert_eq!(edge.to.port.as_str(), "connectors:p");
            }
        }
        let (nodes, wires) = project_diagram(&[], &[], "/Missing", true);
        assert_eq!(build_scene(nodes, wires).node_count(), 0);
        assert_eq!(
            super::super::diagram_roots(&[prim("/Assembly/Sub/Controller", &["in"], &[], false)]),
            BTreeSet::from([
                "/".into(),
                "/Assembly".into(),
                "/Assembly/Sub".into(),
                "/Assembly/Sub/Controller".into()
            ])
        );
        let mut forwarding = dataflow("/A", "x", "/A/B", "x");
        forwarding.source_conn = "inputs:x".into();
        let forwarding_scene = build_scene(
            vec![
                prim("/A", &["x"], &["x"], false),
                prim("/A/B", &["x"], &[], false),
            ],
            vec![forwarding],
        );
        let edge = forwarding_scene.edges().next().unwrap().1;
        assert_eq!(edge.from.port.as_str(), "inputs:x");
        assert!(forwarding_scene.edge_endpoint_positions(edge).is_some());
        let chain: Vec<_> = (0..20_000)
            .map(|i| prim(&format!("/Model{i}"), &["in"], &["out"], false))
            .collect();
        let index = chain
            .iter()
            .enumerate()
            .map(|(i, node)| (node.path.clone(), i))
            .collect();
        let links: Vec<_> = chain
            .windows(2)
            .map(|pair| dataflow(&pair[0].path, "out", &pair[1].path, "in"))
            .collect();
        let ranks = dataflow_ranks(chain.len(), &links, &index);
        assert_eq!(ranks[0], 0);
        assert_eq!(ranks[19_999], 19_999);
    }

    fn prim(path: &str, ins: &[&str], outs: &[&str], is_body: bool) -> PrimNode {
        PrimNode {
            collections: Default::default(),
            programs: Vec::new(),
            variants: Ok(BTreeMap::new()),
            usd_origin: None,
            boundary: None,
            path: path.to_string(),
            display_name: None,
            type_name: "Xform".to_string(),
            is_body,
            schema_root: false,
            schema_node: false,
            schema_column: None,
            schema_row: None,

            port_sources: BTreeMap::new(),
            referenced_ports: BTreeSet::new(),
            inputs: ins.iter().map(|s| s.to_string()).collect(),
            outputs: outs.iter().map(|s| s.to_string()).collect(),
            connectors: Vec::new(),
            port_types: ins
                .iter()
                .map(|name| (format!("inputs:{name}"), "double".into()))
                .chain(
                    outs.iter()
                        .map(|name| (format!("outputs:{name}"), "double".into())),
                )
                .collect(),
        }
    }

    fn dataflow(src: &str, sc: &str, tgt: &str, tc: &str) -> Wire {
        Wire {
            kind: WireKind::Dataflow,
            owner_path: tgt.to_string(),
            source_path: src.to_string(),
            source_conn: format!("outputs:{sc}"),
            target_path: tgt.to_string(),
            target_conn: format!("inputs:{tc}"),
        }
    }

    /// Hierarchy nodes remain available even without wiring interfaces.
    #[test]
    fn usd_hierarchy_prims_are_kept() {
        let nodes = vec![
            prim("/Osc", &[], &["signal"], false),
            prim("/Terrain", &[], &[], false),
        ];
        let scene = build_scene(nodes, vec![]);
        assert_eq!(scene.node_count(), 2);
        let leaves: Vec<_> = scene.nodes().map(|(_, n)| n.label.clone()).collect();
        assert_eq!(leaves, vec!["Osc".to_string(), "Terrain".to_string()]);
    }

    /// A body prim with no connectors is kept (it can still be a joint endpoint).
    #[test]
    fn body_without_connectors_is_kept() {
        let scene = build_scene(vec![prim("/Chassis", &[], &[], true)], vec![]);
        assert_eq!(scene.node_count(), 1);
    }

    #[test]
    fn diagram_preserves_feedback_and_exposes_system_boundaries() {
        let (empty, links) =
            project_diagram(&[prim("/Empty", &[], &[], false)], &[], "/Empty", false);
        assert_eq!(
            build_scene(empty, links).node_count(),
            1,
            "empty systems retain an authoring target"
        );
        let nodes = vec![
            prim("/System", &[], &[], false),
            prim("/System/Rover", &["control"], &["position"], true),
            prim(
                "/System/Rover/Controller",
                &["position", "control"],
                &["drive"],
                false,
            ),
        ];
        let mut wires = vec![
            dataflow("/System/Rover", "position", "/System/Rover", "control"),
            dataflow(
                "/System/Rover",
                "position",
                "/System/Rover/Controller",
                "position",
            ),
        ];
        let mut input_forward = dataflow(
            "/System/Rover",
            "control",
            "/System/Rover/Controller",
            "control",
        );
        input_forward.source_conn = "inputs:control".into();
        wires.push(input_forward);
        let mut output_forward = dataflow(
            "/System/Rover/Controller",
            "drive",
            "/System/Rover",
            "position",
        );
        output_forward.target_conn = "outputs:position".into();
        wires.push(output_forward);
        let (overview, links) = project_diagram(&nodes, &wires, "/System", false);
        assert_eq!(links.len(), 1, "feedback remains an edge in the overview");
        assert_eq!(build_scene(overview, links).edge_count(), 1);
        let (detail, links) = project_diagram(&nodes, &wires, "/System/Rover", false);
        assert_eq!(
            links.len(),
            4,
            "drilling preserves both authored connections"
        );
        assert!(unresolved_links(&detail, &links).is_empty());
        let scene = build_scene(detail, links);
        assert_eq!(scene.edge_count(), 4);
        let boundary: Vec<_> = scene
            .nodes()
            .filter(|(_, n)| n.origin.as_deref() == Some("/System/Rover"))
            .collect();
        assert_eq!(boundary.len(), 2);
        assert_ne!(diagram_key(boundary[0].1), diagram_key(boundary[1].1));
        for (_, edge) in scene.edges() {
            assert_ne!(edge.from.node, edge.to.node);
        }
        use super::super::{build_ops, connect_op};
        use lunco_usd_document::document::{LayerId, UsdOp};
        for (_, edge) in scene.edges() {
            let op = connect_op(&scene, &edge.from, &edge.to, &LayerId::root()).unwrap();
            assert!(
                matches!(op, UsdOp::SetConnection { path, sources, .. } if !path.contains('#') && sources.iter().all(|source| !source.contains('#')))
            );
        }
        let terminal = boundary[0].0;
        assert!(
            build_ops(
                &scene,
                &HashMap::new(),
                &HashMap::new(),
                &[lunco_canvas::SceneEvent::NodeDeleted {
                    id: *terminal,
                    orphaned_edges: Vec::new()
                }],
                &LayerId::root()
            )
            .is_err()
        );
        let overview = build_scene(
            vec![prim("/Rover", &["control"], &["position"], true)],
            vec![dataflow("/Rover", "position", "/Rover", "control")],
        );
        let (_, edge) = overview.edges().next().unwrap();
        let card = overview.node(edge.from.node).unwrap();
        assert!(
            edge.waypoints[1].y < card.rect.min.y && edge.waypoints[2].y < card.rect.min.y,
            "feedback clears the complete card"
        );
        let mut invalid = wires.clone();
        invalid[0].source_conn = "outputs:absent".into();
        let (detail, links) = project_diagram(&nodes, &invalid, "/System/Rover", false);
        assert_eq!(unresolved_links(&detail, &links).len(), 1);
    }

    #[test]
    fn referenced_interface_does_not_require_duplicate_authored_attribute() {
        let mut body = prim("/Body", &[], &[], true);
        body.referenced_ports.insert("outputs:position_y".into());
        let sink = prim("/Controller", &["height"], &[], false);
        let nodes = vec![body, sink];
        let wires = vec![dataflow("/Body", "position_y", "/Controller", "height")];
        assert!(unresolved_links(&nodes, &wires).is_empty());
        assert!(!nodes[0].port_types.contains_key("outputs:position_y"));
        let scene = build_scene(nodes, wires);
        assert_eq!(scene.edge_count(), 1);
    }

    /// The dataflow edge resolves to a real output port on the source and input
    /// port on the sink — the endpoints the write-back path reads back.
    #[test]
    fn dataflow_edge_resolves_to_named_ports() {
        let nodes = vec![
            prim("/Osc", &[], &["signal"], false),
            prim("/Amp", &["signal"], &["scaled"], false),
        ];
        let wires = vec![dataflow("/Osc", "signal", "/Amp", "signal")];
        let scene = build_scene(nodes, wires);
        assert_eq!(scene.edge_count(), 1);
        // Every edge's endpoints resolve to existing ports.
        for (_, e) in scene.edges() {
            assert!(
                scene.edge_endpoint_positions(e).is_some(),
                "edge endpoints must resolve to ports"
            );
        }
    }

    /// A malformed wire cannot manufacture a missing authored input port.
    #[test]
    fn missing_connector_is_not_fabricated() {
        let scene = build_scene(
            vec![
                prim("/Osc", &[], &["signal"], false),
                prim("/Amp", &[], &["scaled"], false),
            ],
            vec![dataflow("/Osc", "signal", "/Amp", "signal")],
        );
        assert_eq!(scene.edge_count(), 0);
    }

    /// Layering: a pure source sits left of its sink (strictly smaller x).
    #[test]
    fn dataflow_layers_left_to_right() {
        let nodes = vec![
            prim("/Amp", &["signal"], &["scaled"], false),
            prim("/Osc", &[], &["signal"], false),
            prim("/Sink", &["scaled"], &[], false),
        ];
        let wires = vec![
            dataflow("/Osc", "signal", "/Amp", "signal"),
            dataflow("/Amp", "scaled", "/Sink", "scaled"),
        ];
        let scene = build_scene(nodes, wires);
        let x = |leaf: &str| {
            scene
                .nodes()
                .find(|(_, n)| n.label == leaf)
                .map(|(_, n)| n.rect.min.x)
                .unwrap()
        };
        assert!(x("/Osc".trim_start_matches('/')) < x("Amp"));
        assert!(x("Amp") < x("Sink"));
    }

    /// A joint prim (both bodies) becomes an edge between them; the two bodies
    /// are the only nodes.
    #[test]
    fn joint_becomes_edge_between_bodies() {
        let nodes = vec![prim("/A", &[], &[], true), prim("/B", &[], &[], true)];
        let wires = vec![Wire {
            kind: WireKind::Joint,
            owner_path: "/Joint".to_string(),
            source_path: "/A".to_string(),
            source_conn: String::new(),
            target_path: "/B".to_string(),
            target_conn: String::new(),
        }];
        let scene = build_scene(nodes, wires);
        assert_eq!(scene.node_count(), 2);
        assert_eq!(scene.edge_count(), 1);
        for (_, e) in scene.edges() {
            assert!(scene.edge_endpoint_positions(e).is_some());
        }
    }

    #[test]
    fn incremental_projection_replaces_only_authored_subtrees_and_wires() {
        let mut nodes = vec![
            prim("/Assembly", &[], &[], false),
            prim("/Assembly/Old", &[], &[], false),
            prim("/AssemblyTwo", &[], &[], false),
            prim("/Elsewhere", &[], &[], false),
            prim("/Elsewhere/Child", &[], &[], false),
        ];
        let mut wires = vec![
            dataflow("/Source", "out", "/Assembly/Old", "in"),
            Wire {
                kind: WireKind::Joint,
                owner_path: "/Assembly/Joint".to_string(),
                source_path: "/BodyA".to_string(),
                source_conn: String::new(),
                target_path: "/BodyB".to_string(),
                target_conn: String::new(),
            },
            Wire {
                kind: WireKind::Joint,
                owner_path: "/AssemblyTwo/Joint".to_string(),
                source_path: "/BodyC".to_string(),
                source_conn: String::new(),
                target_path: "/BodyD".to_string(),
                target_conn: String::new(),
            },
            dataflow("/Source", "out", "/Elsewhere", "in"),
        ];

        replace_affected_projection(
            &mut nodes,
            &mut wires,
            &["/Assembly".to_string()],
            &["/Elsewhere".to_string()],
            &BTreeSet::from([
                "/Assembly/Joint".to_string(),
                "/Assembly/Old".to_string(),
                "/Elsewhere".to_string(),
            ]),
            vec![
                prim("/Assembly/New", &["in"], &[], false),
                prim("/Elsewhere", &["in"], &[], false),
            ],
            vec![
                dataflow("/Source", "out", "/Assembly/New", "in"),
                dataflow("/Source", "out", "/Elsewhere", "in"),
            ],
        );

        let node_paths: BTreeSet<_> = nodes.iter().map(|node| node.path.as_str()).collect();
        assert_eq!(
            node_paths,
            BTreeSet::from([
                "/Assembly/New",
                "/AssemblyTwo",
                "/Elsewhere",
                "/Elsewhere/Child",
            ])
        );
        assert_eq!(wires.len(), 3);
        assert!(wires.iter().any(|wire| wire.owner_path == "/Assembly/New"));
        assert!(wires.iter().any(|wire| wire.owner_path == "/Elsewhere"));
        assert!(
            wires
                .iter()
                .any(|wire| wire.owner_path == "/AssemblyTwo/Joint")
        );
    }

    #[test]
    fn endpoint_resync_invalidates_authored_wire_owner() {
        let wires = vec![
            Wire {
                kind: WireKind::Joint,
                owner_path: "/Assembly/Joint".to_string(),
                source_path: "/Assembly/BodyA".to_string(),
                source_conn: String::new(),
                target_path: "/Assembly/BodyB".to_string(),
                target_conn: String::new(),
            },
            Wire {
                kind: WireKind::Joint,
                owner_path: "/Elsewhere/Joint".to_string(),
                source_path: "/Elsewhere/BodyA".to_string(),
                source_conn: String::new(),
                target_path: "/Elsewhere/BodyB".to_string(),
                target_conn: String::new(),
            },
        ];

        let affected = wire_owners_affected_by_paths(&wires, &["/Assembly/BodyA".into()], &[]);
        assert_eq!(affected, BTreeSet::from(["/Assembly/Joint".to_string()]));

        let mut remaining_nodes = Vec::new();
        let mut remaining_wires = wires.clone();
        replace_affected_projection(
            &mut remaining_nodes,
            &mut remaining_wires,
            &["/Assembly/BodyA".into()],
            &[],
            &affected,
            Vec::new(),
            Vec::new(),
        );
        assert_eq!(remaining_wires.len(), 1);
        assert_eq!(remaining_wires[0].owner_path, "/Elsewhere/Joint");

        assert_eq!(
            wire_owners_affected_by_paths(&wires, &[], &["/Elsewhere/BodyB".into()]),
            BTreeSet::from(["/Elsewhere/Joint".to_string()])
        );
    }

    /// A cycle doesn't hang the layering and every node still gets a bounded rank.
    #[test]
    fn cyclic_dataflow_terminates_and_bounds_rank() {
        let nodes = vec![
            prim("/A", &["x"], &["y"], false),
            prim("/B", &["y"], &["x"], false),
        ];
        let wires = vec![
            dataflow("/A", "y", "/B", "y"),
            dataflow("/B", "x", "/A", "x"),
        ];
        let scene = build_scene(nodes, wires);
        assert_eq!(scene.node_count(), 2);
        // Ranks are clamped to < n, so x stays within one column span of margin.
        for (_, node) in scene.nodes() {
            assert!(node.rect.min.x <= MARGIN + (2.0 - 1.0) * COL_SPACING + 0.5);
        }
    }

    /// The USD reader stays complete; only the explicit schema projection
    /// selects authored boundaries and removes same-prim forwarding bindings.
    #[test]
    fn schema_projection_is_explicit_and_property_driven() {
        let mut root = prim("/Lander", &[], &["out"], false);
        root.schema_root = true;
        root.schema_node = true;
        let mut controller = prim("/Lander/GNC", &["in"], &["cmd"], false);
        controller.schema_node = true;
        let internal = prim("/Lander/Internal", &["state"], &["state"], false);
        let wires = vec![
            dataflow("/Lander", "out", "/Lander/GNC", "in"),
            dataflow("/Lander", "state", "/Lander", "state"),
        ];

        let source_nodes = vec![root, controller, internal];
        let (nodes, wires) = project_schema(&source_nodes, &wires, "/Lander");
        assert_eq!(
            nodes
                .iter()
                .map(|node| node.path.as_str())
                .collect::<Vec<_>>(),
            vec!["/Lander", "/Lander/GNC"]
        );
        assert_eq!(wires.len(), 1);
        assert_eq!(wires[0].source_path, "/Lander");
        assert_eq!(wires[0].target_path, "/Lander/GNC");
    }

    #[test]
    fn schema_projection_never_falls_back_to_generic_topology() {
        let nodes = vec![
            prim("/Generic/Controller", &["in"], &["out"], false),
            prim("/Generic/Plant", &["out"], &["in"], false),
        ];
        let wires = vec![dataflow(
            "/Generic/Controller",
            "out",
            "/Generic/Plant",
            "out",
        )];

        assert!(schema_roots(&nodes).is_empty());
        let (projected, projected_wires) = project_schema(&nodes, &wires, "");
        assert!(projected.is_empty());
        assert!(projected_wires.is_empty());
    }

    #[test]
    fn schema_projection_selects_exactly_one_authored_root() {
        let mut first_root = prim("/First", &[], &["out"], false);
        first_root.schema_root = true;
        first_root.schema_node = true;
        let mut first_child = prim("/First/Plant", &["in"], &[], false);
        first_child.schema_node = true;
        let mut second_root = prim("/Second", &[], &["out"], false);
        second_root.schema_root = true;
        second_root.schema_node = true;
        let mut second_child = prim("/Second/Plant", &["in"], &[], false);
        second_child.schema_node = true;

        let nodes = vec![first_root, first_child, second_root, second_child];
        assert_eq!(schema_roots(&nodes), vec!["/First", "/Second"]);
        let (projected, _) = project_schema(&nodes, &[], "/Second");
        assert_eq!(
            projected
                .iter()
                .map(|node| node.path.as_str())
                .collect::<Vec<_>>(),
            vec!["/Second", "/Second/Plant"]
        );
    }
}

/// Refresh unauthored wire geometry after automatic or saved layout changes.
pub(super) fn route_edges(scene: &mut Scene) {
    let mut lanes: Vec<Vec<(f32, f32)>> = Vec::new();
    let routes: Vec<_> = scene
        .edges()
        .filter(|(_, edge)| {
            !edge.waypoints_authored
                && edge
                    .data
                    .downcast_ref::<UsdWireData>()
                    .is_some_and(|data| data.kind != WireKind::Joint)
        })
        .filter_map(|(id, edge)| {
            let (from, to) = scene.edge_endpoint_positions(edge)?;
            let source = scene.node(edge.from.node)?;
            let target = scene.node(edge.to.node)?;
            let route = if to.x <= from.x || edge.from.node == edge.to.node {
                // Feedback must clear the entire cards, not merely the port row.
                let span = (
                    source.rect.min.x.min(target.rect.min.x) - 24.0,
                    source.rect.max.x.max(target.rect.max.x) + 24.0,
                );
                // Interval coloring gives overlapping feedback spans independent
                // lanes while reusing lanes for disjoint spans.
                let lane = lanes
                    .iter()
                    .position(|intervals| {
                        intervals
                            .iter()
                            .all(|other| span.1 < other.0 || span.0 > other.1)
                    })
                    .unwrap_or(lanes.len());
                if lane == lanes.len() {
                    lanes.push(Vec::new());
                }
                lanes[lane].push(span);
                let top = scene
                    .nodes()
                    .filter(|(_, node)| node.rect.max.x >= span.0 && node.rect.min.x <= span.1)
                    .map(|(_, node)| node.rect.min.y)
                    .fold(source.rect.min.y.min(target.rect.min.y), f32::min);
                let y = top - 48.0 - lane as f32 * 12.0;
                let stub = |node: &Node, pos: Pos| {
                    if pos.x < node.rect.center().x {
                        node.rect.min.x - 24.0
                    } else {
                        node.rect.max.x + 24.0
                    }
                };
                let sx = stub(source, from);
                let tx = stub(target, to);
                vec![
                    Pos::new(sx, from.y),
                    Pos::new(sx, y),
                    Pos::new(tx, y),
                    Pos::new(tx, to.y),
                ]
            } else {
                orthogonal_waypoints(from, to)
            };
            Some((*id, route))
        })
        .collect();
    for (id, route) in routes {
        if let Some(edge) = scene.edge_mut(id) {
            edge.waypoints = route;
        }
    }
}

/// Explain exact unresolved property identities; hidden descendants are excluded
/// by the scope projector before this boundary.
pub(super) fn unresolved_links(nodes: &[PrimNode], wires: &[Wire]) -> Vec<String> {
    let ports: BTreeMap<_, BTreeSet<_>> = nodes
        .iter()
        .map(|node| {
            let names = node
                .inputs
                .iter()
                .map(|name| format!("inputs:{name}"))
                .chain(node.outputs.iter().map(|name| format!("outputs:{name}")))
                .chain(
                    node.connectors
                        .iter()
                        .map(|name| format!("connectors:{name}")),
                )
                .chain(node.referenced_ports.iter().cloned())
                .collect();
            (node.path.as_str(), names)
        })
        .collect();
    wires
        .iter()
        .filter(|wire| wire.kind != WireKind::Joint)
        .filter_map(|wire| {
            let source_missing = !ports
                .get(wire.source_path.as_str())
                .is_some_and(|names| names.contains(&wire.source_conn));
            let target_missing = !ports
                .get(wire.target_path.as_str())
                .is_some_and(|names| names.contains(&wire.target_conn));
            if !source_missing && !target_missing {
                return None;
            }
            Some(format!(
                "{}.{} → {}.{}: {} property is not in the composed USD port declarations",
                wire.source_path,
                wire.source_conn,
                wire.target_path,
                wire.target_conn,
                if source_missing { "source" } else { "target" }
            ))
        })
        .collect()
}

/// The USD connection property is an explicit interface reference. Recognized
/// runtime providers need not author a duplicate USD attribute: an inert stage
/// can display their references before ECS projection or Modelica preparation.
pub(super) fn resolve_referenced_interfaces(
    view: &StageView<'_>,
    nodes: &mut [PrimNode],
    wires: &[Wire],
) {
    let index: HashMap<_, _> = nodes
        .iter()
        .enumerate()
        .map(|(i, node)| (node.path.clone(), i))
        .collect();
    for node in nodes.iter_mut() {
        node.referenced_ports.clear();
    }
    let mut providers = HashMap::new();
    for wire in wires.iter().filter(|wire| wire.kind != WireKind::Joint) {
        for (path, property) in [
            (&wire.source_path, &wire.source_conn),
            (&wire.target_path, &wire.target_conn),
        ] {
            let Some(&i) = index.get(path) else {
                continue;
            };
            if nodes[i].port_sources.contains_key(property) {
                continue;
            }
            let provider = providers.entry(path.clone()).or_insert_with(|| {
                SdfPath::new(path)
                    .ok()
                    .and_then(|path| lunco_usd_bevy_stage::read::runtime_port_provider(view, &path))
            });
            if provider.is_some() {
                nodes[i].referenced_ports.insert(property.clone());
            }
        }
    }
}
