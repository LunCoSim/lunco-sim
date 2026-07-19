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
//! # Feature `ui`
//!
//! The egui render stack — [`canvas`], [`layer`], [`overlay`],
//! [`visual`] — sits behind the `ui` feature (off by default). A
//! plain dependency gets the data model only (`scene` / `viewport` /
//! `selection` / `tool` / `event`) and links no `bevy_egui`, so
//! headless builds can consume canvas scenes without a GPU stack.
//!
//! # Minimum shipping — B1 scope
//!
//! B1 (this change) creates the crate, lands the data model,
//! viewport math, visual / tool / layer / overlay traits, and unit
//! tests for hit-testing and coordinate round-trips. [`Canvas::ui`]
//! walks its layer pipeline but does **not** yet route input —
//! pan/zoom/drag/connect land in B2 alongside the Modelica projector
//! used by `canvas_diagram`.
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

#[cfg(feature = "ui")]
pub mod canvas;
pub mod event;
#[cfg(feature = "ui")]
pub mod layer;
#[cfg(feature = "ui")]
pub mod overlay;
pub mod scene;
pub mod selection;
pub mod tool;
pub mod viewport;
#[cfg(feature = "ui")]
pub mod visual;

#[cfg(feature = "ui")]
pub use canvas::Canvas;
pub use event::{ContextTarget, InputEvent, Modifiers, MouseButton, SceneEvent};
#[cfg(feature = "ui")]
pub use layer::{
    EdgesLayer, GridLayer, Layer, NodesLayer, SelectionLayer, ToolPreviewLayer,
};
#[cfg(feature = "ui")]
pub use overlay::{Anchor, NavBarOverlay, Overlay, OverlayCtx};
pub use scene::{
    empty_node_data, Edge, EdgeHitKind, EdgeId, Node, NodeData, NodeId, NodeHitKind, Port,
    PortId, PortRef, Pos, Rect, Scene,
};
pub use selection::{SelectItem, Selection};
pub use tool::{CanvasOps, DefaultTool, SnapSettings, Tool, ToolOutcome};
pub use viewport::{Viewport, ViewportConfig};
#[cfg(feature = "ui")]
pub use visual::{
    DrawCtx, EdgeVisual, NodeHit, NodeVisual, PlaceholderEdgeVisual, PlaceholderNodeVisual,
    VisualRegistry,
};
