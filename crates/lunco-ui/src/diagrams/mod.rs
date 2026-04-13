//! Diagram widgets — reusable chart and graph rendering.
//!
//! ## Architecture: Two Diagram Types
//!
//! ### Time-Series (pure rendering)
//! Zero-copy, stateless. Domain data → `ChartSeries` → rendered.
//! ```ignore
//! let series: Vec<ChartSeries> = /* borrow from domain data */;
//! time_series_plot(ui, "plot_id", &series);
//! ```
//!
//! ### Node Graphs (entity viewers)
//! Domain crates own the graph data and viewer implementation.
//! This crate re-exports egui-snarl types for convenience.
//!
//! ```text
//!   Domain crate (lunco-modelica, lunco-fsw, etc.)
//!     ├── Defines node type enum
//!     ├── Implements SnarlViewer<T>
//!     └── Owns Snarl<T> resource
//!           │
//!           ▼
//!   lunco-ui::diagrams::node_graph
//!     └── Re-exports egui-snarl types (InPin, OutPin, Snarl, SnarlViewer)
//! ```
//!
//! ## Entity Viewer Pattern
//!
//! All diagrams watch a selected entity and render its data.
//! The same panel works in workbench, 3D overlay, or mission dashboard.
//!
//! See `docs/research-ui-ux-architecture.md` for full architecture.

pub mod time_series;
pub mod node_graph;

pub use time_series::{time_series_plot, ChartSeries};
pub use node_graph::{InPin, InPinId, NodeId, OutPin, OutPinId, Snarl, SnarlViewer};
