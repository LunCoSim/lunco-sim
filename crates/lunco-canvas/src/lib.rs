//! `lunco-canvas` — a stateful 2D scene editor substrate.
//!
//! This crate is the generic substrate under LunCoSim's Modelica
//! diagram, future node-graph editors, annotation overlays, and any
//! other "things arranged in a 2D pannable/zoomable plane" UI.
//!
//! # The shape at a glance
//!
//! ```text
//!     ┌──────────────────────────────────────────────────────┐
//!     │  Canvas                                              │
//!     │   ├─ Scene      (authored: nodes, edges, positions)  │
//!     │   ├─ Viewport   (pan, zoom, smooth anim)             │
//!     │   ├─ Selection  (primary + set)                      │
//!     │   ├─ Tool       ▲ one active; handles input          │
//!     │   ├─ Layer[]    ▲ ordered render passes              │
//!     │   ├─ Overlay[]  ▲ floating screen-space UI           │
//!     │   └─ VisualRegistry (kind-id → NodeVisual / EdgeVisual)
//!     └──────────────────────────────────────────────────────┘
//! ```
//!
//! The three `▲` slots are the extension seams. New features land
//! as a new impl of one of these traits — never as a change to the
//! canvas's public API.
//!
//! # Minimum shipping — B1 scope
//!
//! B1 (this change) creates the crate, lands the data model,
//! viewport math, visual / tool / layer / overlay traits, and unit
//! tests for hit-testing and coordinate round-trips. [`Canvas::ui`]
//! walks its layer pipeline but does **not** yet route input —
//! pan/zoom/drag/connect land in B2 alongside the Modelica projector
//! that replaces the egui-snarl-based diagram panel.
//!
//! # Minimum up-front choices (so later cases slot in)
//!
//! - [`visual::DrawCtx`] carries `&mut egui::Ui` + `time: f64` +
//!   `extras: &dyn Any`. Widget-in-node, animated edges, and viz
//!   overlays all land as new visual impls without touching the
//!   trait signature.
//! - [`tool::Tool`] + [`layer::Layer`] + [`overlay::Overlay`] exist
//!   from day one, with one `Tool` and the default layer set
//!   shipping now. Future tools/layers/overlays are additive.
//! - [`scene::Scene`] derives `Serialize + Deserialize`;
//!   [`visual::VisualRegistry`] rebuilds `Box<dyn _>` visuals from
//!   `(kind, data)` on load. That's the hook `SceneDocument` (in
//!   `lunco-doc`) plugs into when the colony / pure-dataflow / pure-
//!   annotation use cases need a first-class canvas document type.

pub mod canvas;
pub mod event;
pub mod layer;
pub mod overlay;
pub mod scene;
pub mod selection;
pub mod theme;
pub mod tool;
pub mod viewport;
pub mod visual;

pub use canvas::{Canvas, SnapSettings};
pub use event::{ContextTarget, InputEvent, Modifiers, MouseButton, SceneEvent};
pub use layer::{
    EdgesLayer, GridLayer, Layer, NodesLayer, SelectionLayer, ToolPreviewLayer,
};
pub use overlay::{Anchor, NavBarOverlay, Overlay, OverlayCtx};
pub use theme::CanvasLayerTheme;
pub use scene::{Edge, EdgeId, Node, NodeId, Port, PortId, PortRef, Pos, Rect, Scene};
pub use selection::{SelectItem, Selection};
pub use tool::{CanvasOps, DefaultTool, Tool, ToolOutcome};
pub use viewport::{Viewport, ViewportConfig};
pub use visual::{
    DrawCtx, EdgeVisual, NodeHit, NodeVisual, PlaceholderEdgeVisual, PlaceholderNodeVisual,
    VisualRegistry,
};
