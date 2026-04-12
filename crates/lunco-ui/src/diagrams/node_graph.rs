//! Node-graph widget — renders system architecture diagrams.
//!
//! Uses `egui-snarl` for draggable nodes with bezier-curve connections.
//!
//! Unlike the time-series widget (which is a pure rendering function),
//! node graphs require the domain crate to define a node type and implement
//! the `SnarlViewer` trait — this is how egui-snarl works by design.
//!
//! ## Usage
//! ```ignore
//! // In domain crate — define your node type
//! #[derive(serde::Serialize, serde::Deserialize)]
//! enum ObcNode { Block { name: String }, Port { name: String } }
//!
//! // Implement SnarlViewer
//! impl SnarlViewer<ObcNode> for ObcViewer {
//!     fn show_body(&mut self, id: NodeId, node: &ObcNode, ui: &mut Ui, _) {
//!         match node { ObcNode::Block { name } => ui.label(name), _ => {} }
//!     }
//! }
//!
//! // Render in panel
//! let mut snarl = world.resource_mut::<ObcArchitectureSnarl>();
//! let mut viewer = ObcViewer;
//! snarl.0.show("obc_graph", ui, &mut viewer);
//! ```

pub use egui_snarl::{InPin, InPinId, NodeId, OutPin, OutPinId, Snarl};
pub use egui_snarl::ui::SnarlViewer;
