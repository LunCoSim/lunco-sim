//! Diagram widgets — reusable chart and graph rendering.
//!
//! ## Time-Series: pure rendering function, zero copies
//! ```ignore
//! let series: Vec<ChartSeries> = /* borrow from domain data */;
//! time_series_plot(ui, "plot_id", &series);
//! ```
//!
//! ## Node Graphs: re-exports egui-snarl types
//! Domain crates define their own node type + SnarlViewer,
//! then render with `snarl.show("id", ui, &mut viewer)`.

pub mod time_series;
pub mod node_graph;

pub use time_series::{time_series_plot, ChartSeries};
pub use node_graph::{InPin, InPinId, NodeId, OutPin, OutPinId, Snarl, SnarlViewer};
