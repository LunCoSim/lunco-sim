//! Diagram widgets — reusable chart rendering.
//!
//! Time-series plotting only at this level. Node-graph / block-diagram
//! rendering lives in domain crates (e.g. `lunco-modelica`'s
//! `canvas_diagram`) on top of `lunco-canvas` — the workbench's own
//! canvas substrate.
//!
//! ### Time-Series (pure rendering)
//! Stateless, no per-sample copies (generator-backed `PlotPoints`).
//! Domain data → `ChartSeries` → rendered.
//! ```ignore
//! let series: Vec<ChartSeries> = /* borrow from domain data */;
//! time_series_plot(ui, "plot_id", &series);
//! ```

pub mod time_series;

pub use time_series::{time_series_plot, ChartSeries};
