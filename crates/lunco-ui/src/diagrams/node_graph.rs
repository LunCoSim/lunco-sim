//! Node-graph rendering via egui-snarl.
//!
//! Re-exports the core types needed for domain crates to build interactive
//! node graph panels. The rendering layer is decoupled from the data:
//!
//! ```text
//!   Domain crate (e.g. lunco-modelica, lunco-fsw)
//!     ├── Defines node type enum (what data each node holds)
//!     ├── Implements SnarlViewer<T> (how nodes render)
//!     └── Owns Snarl<T> resource (the graph data)
//!           │
//!           ▼
//!   lunco-ui (this crate)
//!     └── Re-exports egui-snarl types for convenience
//! ```
//!
//! ## Entity Viewer Pattern
//!
//! Node graphs are entity viewers — they watch a selected entity and render
//! its graph data. The same `SnarlViewer` implementation works whether the
//! graph is in a dockable panel, a 3D overlay, or a mission dashboard.
//!
//! ## Usage
//!
//! ```ignore
//! // In domain crate — define your node type
//! #[derive(serde::Serialize, serde::Deserialize)]
//! enum ModelicaNode {
//!     Component { name: String, ports: Vec<String> },
//!     Connector { name: String, port_type: String },
//! }
//!
//! // Implement SnarlViewer
//! impl SnarlViewer<ModelicaNode> for DiagramViewer {
//!     fn title(&mut self, node: &ModelicaNode) -> String { ... }
//!     fn inputs(&mut self, node: &ModelicaNode) -> usize { ... }
//!     fn outputs(&mut self, node: &ModelicaNode) -> usize { ... }
//!     fn show_input(&mut self, pin: &InPin, ui: &mut Ui, snarl: &mut Snarl<ModelicaNode>) -> impl SnarlPin { ... }
//!     fn show_output(&mut self, pin: &OutPin, ui: &mut Ui, snarl: &mut Snarl<ModelicaNode>) -> impl SnarlPin { ... }
//! }
//!
//! // Render in panel
//! let mut snarl = world.resource_mut::<DiagramState>().snarl.take().unwrap();
//! let mut viewer = DiagramViewer;
//! snarl.show(&mut viewer, &SnarlStyle::default(), "diagram", ui);
//! ```

// Re-export egui-snarl core types for domain crates
pub use egui_snarl::{InPin, InPinId, NodeId, OutPin, OutPinId, Snarl};
pub use egui_snarl::ui::SnarlViewer;
