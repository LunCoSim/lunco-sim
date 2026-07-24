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
// Diagram node positions are a plain 2D point. Aliased from bevy's `Vec2` (not
// `egui::Pos2`) so this core module — consumed by the index/indexer/query
// backend — carries no egui dependency. The egui editor converts at its render
// boundary (`Vec2`↔`egui::Pos2` share `{x, y}: f32`).
use bevy::math::Vec2 as Pos2;
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
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for DiagramNodeId {
    fn default() -> Self {
        Self::new()
    }
}

/// Causality classification of a connector port, derived from the
/// connector class's variable declarations. Drives the port marker
/// shape (square / triangle / circle) and the wire's arrowhead
/// behaviour in the canvas renderer — replaces earlier leaf-name
/// heuristics (`ends_with("Input")`) that only worked for MSL's
/// naming conventions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum PortKind {
    /// Connector has exactly one `input` variable and no `flow`
    /// variables — canonical signal input (RealInput, BooleanInput).
    Input,
    /// Connector has exactly one `output` variable and no `flow`
    /// variables — canonical signal output (RealOutput, …).
    Output,
    /// Connector has any `flow` variable (physical connector with
    /// conservation), or neither / both causality variants — the
    /// acausal case (Pin, Flange, HeatPort, FluidPort, custom fuel
    /// port). Rendered as a filled circle; wire has no arrowhead.
    #[default]
    Acausal,
}

/// Metadata for one `flow` variable declared in a connector class —
/// the kind of variable whose sign decides animation direction and
/// whose declared unit labels the hover tooltip.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FlowVarMeta {
    /// Variable name inside the connector (e.g. `"m_dot"`, `"i"`).
    pub name: String,
    /// Unit string from `Real(unit="…")` / `SI.MassFlowRate` quantity
    /// — empty when the connector didn't declare one.
    pub unit: String,
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
    /// Wire color (RGB 0..=255) sourced from the connector class's
    /// `Icon` annotation — `lineColor` of the first colored graphic,
    /// `fillColor` as fallback. `None` means "fall back to the
    /// leaf-name palette in `wire_color_for`". Mirrors the OMEdit /
    /// Dymola behavior where wire colors come from each connector
    /// class's icon definition rather than a hardcoded table.
    #[serde(default)]
    pub color: Option<[u8; 3]>,
    /// Port size in the parent class's icon coordinate system, taken
    /// from `annotation(Placement(transformation(extent=...)))` on the
    /// connector declaration. Used by the canvas painter to render
    /// the connector class's authored `Icon` at the correct scale —
    /// MSL convention is to draw a connector instance at its placement
    /// extent (typically 20×20 in icon coords, scaled with the parent
    /// to produce the small ~2-unit flange dot OMEdit shows). Defaults
    /// to (20, 20) when the placement is missing — safe fallback that
    /// matches the most common authoring pattern.
    #[serde(default = "PortDef::default_size")]
    pub size_x: f32,
    #[serde(default = "PortDef::default_size")]
    pub size_y: f32,
    /// Rotation in degrees from `Placement(transformation(rotation=...))`
    /// on the port declaration (CCW, Modelica convention). Used by
    /// the canvas painter to rotate the connector class's authored Icon
    /// so e.g. a `rotation=270` input port shows its triangle pointing
    /// upward into the parent edge it sits on.
    #[serde(default)]
    pub rotation_deg: f32,
    /// Causality classification derived from the connector class's
    /// variables. Drives port shape + arrowhead.
    #[serde(default)]
    pub kind: PortKind,
    /// Flow-variable descriptors. Empty for causal connectors.
    /// Non-empty → renderer samples each var from the per-frame
    /// state snapshot to animate flow and populate tooltips with
    /// the authored unit.
    #[serde(default)]
    pub flow_vars: Vec<FlowVarMeta>,
}

impl PortDef {
    fn default_size() -> f32 {
        20.0
    }
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

/// On-disk shape of `msl_index.json`. Wraps the legacy
/// `Vec<crate::index::ClassEntry>` payload alongside the pre-baked
/// bundled `PackageNode` tree so the indexer can ship both in
/// a single artifact. The reader accepts both this struct *and*
/// the bare-array legacy form for backward compatibility.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MslIndex {
    /// Palette / component metadata — what `crate::index::ClassEntry` has
    /// always carried.
    pub components: Vec<crate::index::ClassEntry>,
    /// Pre-baked `PackageNode` tree for the bundled-models root in
    /// the Package Browser. Indexer emits these directly so the
    /// runtime is a trivial deserialise — no shape conversion.
    /// Empty when the indexer was run before this format landed.
    #[serde(default)]
    pub bundled: Vec<crate::package_tree::types::PackageNode>,
}

/// A node instance placed on the visual canvas.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagramNode {
    pub id: DiagramNodeId,
    /// Instance name (e.g., "R1", "C1").
    pub instance_name: String,
    /// Component definition reference.
    pub component_def: crate::index::ClassEntry,
    /// Parameter values (name → value).
    pub parameter_values: HashMap<String, String>,
    /// Canvas-world centre of the icon — kept as the authoritative
    /// drag target. The full icon-local → canvas-world transform is
    /// in [`icon_transform`](Self::icon_transform); this field
    /// duplicates the translation part for drag handlers that don't
    /// want to do matrix math. Use [`set_position`](Self::set_position)
    /// to keep both consistent.
    pub position: Pos2,
    /// Single affine transform from this node's icon-local Modelica
    /// coords (-100..100, +Y up) to canvas world coords (+Y down).
    /// Encodes mirror, rotation, scale, and translation in one
    /// matrix — every per-feature field this used to host
    /// (`extent_size`/`rotation_degrees`/`mirror_x`/`mirror_y`) is
    /// folded into this one struct. The canvas projector uses it for
    /// port positioning, edge-stub directions, the icon's bounding
    /// rect, and (eventually) the icon body itself.
    #[serde(default)]
    pub icon_transform: crate::icon_transform::IconTransform,
    /// Whether the node is selected.
    pub selected: bool,
    /// True when the source declares the component with an `if <cond>`
    /// clause — MSL convention is to render these dimmed/translucent
    /// because they're "design-time visible, runtime-conditional"
    /// (e.g. `Constant Dzero(k=0) if not with_D` in `LimPID`).
    #[serde(default)]
    pub is_conditional: bool,
}

impl DiagramNode {
    /// Update the node's canvas centre, keeping the cached
    /// [`position`](Self::position) field and the translation half of
    /// [`icon_transform`](Self::icon_transform) in lock-step. Drag
    /// handlers should call this rather than mutating either field
    /// directly.
    pub fn set_position(&mut self, pos: Pos2) {
        let dx = pos.x - self.position.x;
        let dy = pos.y - self.position.y;
        self.position = pos;
        self.icon_transform.m[2] += dx;
        self.icon_transform.m[5] += dy;
    }
}

/// A connection between two component ports.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagramEdge {
    pub id: Uuid,
    pub source_node: DiagramNodeId,
    pub source_port: String,
    pub target_node: DiagramNodeId,
    pub target_port: String,
    /// Authored polyline waypoints in Modelica coords (+Y up, the
    /// same frame `connect(...) annotation(Line(points={{x,y},...}))`
    /// uses). `None` or empty → renderer falls back to the orthogonal
    /// Z auto-router. Endpoints are the port stubs; waypoints sit
    /// between them in source order.
    #[serde(default)]
    pub waypoints: Vec<(f32, f32)>,
    /// True when the source annotation specified `smooth=Smooth.Bezier`.
    /// Renderer switches to a Catmull-Rom curve through the polyline
    /// points instead of straight segments.
    #[serde(default)]
    pub smooth_bezier: bool,
    /// `Line(color={r,g,b})` override from source. When `Some`, the
    /// renderer uses this instead of the connector-derived colour.
    #[serde(default)]
    pub color: Option<[u8; 3]>,
    /// `Line(thickness=…)` override from source. When `Some`, the
    /// renderer scales its base stroke width by this value (1.0 =
    /// default). `None` when source kept the Modelica default 0.25.
    #[serde(default)]
    pub thickness: Option<f32>,
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
        let counter = self
            .name_counters
            .entry(component_name.to_string())
            .or_insert(0);
        *counter += 1;
        // Use first letter as prefix: Resistor → R1, Capacitor → C1
        let prefix = component_name
            .chars()
            .next()
            .unwrap_or('X')
            .to_uppercase()
            .to_string();
        format!("{}{}", prefix, counter)
    }

    /// Add a node to the diagram.
    pub fn add_node(&mut self, def: crate::index::ClassEntry, position: Pos2) -> DiagramNodeId {
        self.add_node_with_id(DiagramNodeId::new(), def, position)
    }

    /// Add a node with a specific ID.
    pub fn add_node_with_id(
        &mut self,
        id: DiagramNodeId,
        def: crate::index::ClassEntry,
        position: Pos2,
    ) -> DiagramNodeId {
        let instance_name = self.next_instance_name(def.short_name());
        let mut parameter_values = HashMap::new();
        for param in &def.parameters {
            parameter_values.insert(param.name.clone(), param.default.clone());
        }
        // Default IconTransform = identity scaled to the standard
        // 20×20 fallback box, translated to `position`. Importer paths
        // that have a real Placement override this immediately.
        // `from_placement` bakes in the Y flip — recovered cleanly by
        // flipping `position.y` above (Modelica +Y up, position is screen-Y).
        let icon_transform = crate::icon_transform::IconTransform::from_placement(
            (position.x, -position.y),
            (20.0, 20.0),
            false,
            false,
            0.0,
            (0.0, 0.0),
        );

        self.nodes.push(DiagramNode {
            id,
            instance_name,
            component_def: def,
            parameter_values,
            position,
            icon_transform,
            selected: false,
            is_conditional: false,
        });
        id
    }

    /// Remove a node and its connected edges.
    pub fn remove_node(&mut self, id: DiagramNodeId) {
        self.nodes.retain(|n| n.id != id);
        self.edges
            .retain(|e| e.source_node != id && e.target_node != id);
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
            (e.source_node == source_node
                && e.source_port == source_port
                && e.target_node == target_node
                && e.target_port == target_port)
                || (e.source_node == target_node
                    && e.source_port == target_port
                    && e.target_node == source_node
                    && e.target_port == source_port)
        });
        if !exists {
            self.edges.push(DiagramEdge {
                id: Uuid::new_v4(),
                source_node,
                source_port,
                target_node,
                target_port,
                waypoints: Vec::new(),
                smooth_bezier: false,
                color: None,
                thickness: None,
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

static MSL_LIBRARY: OnceLock<MslIndex> = OnceLock::new();

/// Returns the MSL component definitions available in the palette.
/// Loaded from `msl_index.json` — either via the in-memory MSL bundle
/// (web) or directly off disk (native).
///
/// The cache uses `OnceLock::set` only after a *successful* load so
/// early calls (e.g. the `prewarm_msl_library` startup task on wasm
/// before the MSL bundle has finished fetching) return an empty slice
/// without permanently poisoning the cache. Subsequent calls retry the
/// load until it succeeds, then memoize. Per-frame cost while empty is
/// one `OnceLock::get` + one hashmap lookup — negligible.
pub fn msl_class_library() -> &'static [crate::index::ClassEntry] {
    msl_index().map(|i| i.components.as_slice()).unwrap_or(&[])
}

/// Pre-baked `PackageNode` tree for the bundled-models root in the
/// Package Browser. Empty when the running `msl_index.json` predates
/// this format — callers should fall back to flat-leaf rendering.
pub fn msl_bundled_nodes() -> &'static [crate::package_tree::types::PackageNode] {
    msl_index().map(|i| i.bundled.as_slice()).unwrap_or(&[])
}

fn msl_index() -> Option<&'static MslIndex> {
    if let Some(idx) = MSL_LIBRARY.get() {
        return Some(idx);
    }
    if let Some(idx) = try_load_msl_index() {
        let _ = MSL_LIBRARY.set(idx);
    }
    MSL_LIBRARY.get()
}

fn try_load_msl_index() -> Option<MslIndex> {
    // 1. In-memory bundle (set by `MslRemotePlugin` on wasm; also
    //    populated on native if we ever decide to load via the same
    //    pipeline). This wins so a host that has both still uses the
    //    deliberately-shipped index over whatever happens to be on disk.
    if let Some(bytes) = lunco_assets::msl::msl_read(std::path::Path::new("msl_index.json")) {
        if let Ok(text) = std::str::from_utf8(&bytes) {
            if let Some(idx) = parse_msl_index(text) {
                return Some(idx);
            }
        }
    }
    // 2. Native filesystem fallback.
    #[cfg(not(target_arch = "wasm32"))]
    {
        let path = lunco_assets::msl_dir().join("msl_index.json");
        if let Ok(content) = std::fs::read_to_string(path) {
            if let Some(idx) = parse_msl_index(&content) {
                return Some(idx);
            }
        }
    }
    None
}

/// Deserialise `msl_index.json` accepting both the new
/// [`MslIndex`] object form and the legacy bare-array form. Older
/// caches already on user disks parse cleanly via the array branch
/// (with `bundled` left empty); freshly indexed caches use the
/// object form.
fn parse_msl_index(text: &str) -> Option<MslIndex> {
    // Tolerant load: parse components strictly, but treat the
    // bundled tree as opaque — if it's the legacy `BundledFileTree`
    // shape (pre-PR7) or any other unknown form, drop it silently
    // and let the runtime fall back to flat leaves. The user reruns
    // `msl_indexer` to repopulate it.
    #[derive(Deserialize)]
    struct Relaxed {
        components: Vec<crate::index::ClassEntry>,
        #[serde(default)]
        bundled: serde_json::Value,
    }
    if let Ok(relaxed) = serde_json::from_str::<Relaxed>(text) {
        let bundled =
            serde_json::from_value::<Vec<crate::package_tree::types::PackageNode>>(relaxed.bundled)
                .unwrap_or_default();
        return Some(MslIndex {
            components: relaxed.components,
            bundled,
        });
    }
    if let Ok(components) = serde_json::from_str::<Vec<crate::index::ClassEntry>>(text) {
        return Some(MslIndex {
            components,
            bundled: Vec::new(),
        });
    }
    None
}

/// Get unique categories from the MSL library.
pub fn msl_categories() -> Vec<String> {
    let mut cats: Vec<String> = msl_class_library()
        .iter()
        .map(|c| c.category.clone())
        .collect();
    cats.sort();
    cats.dedup();
    cats
}

/// Get components in a category.
pub fn msl_classes_in_category(category: &str) -> Vec<crate::index::ClassEntry> {
    msl_class_library()
        .iter()
        .filter(|c| c.category == category)
        .cloned()
        .collect()
}

/// Lookup a component definition by its MSL path.
pub fn msl_class_by_path(path: &str) -> Option<crate::index::ClassEntry> {
    msl_class_library().iter().find(|c| c.name == path).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_msl_library_not_empty() {
        let lib = msl_class_library();
        assert!(!lib.is_empty());
        assert!(lib.iter().any(|c| c.short_name() == "Resistor"));
        assert!(lib.iter().any(|c| c.short_name() == "Ground"));
    }
}
