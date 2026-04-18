//! Built-in visualization kinds.
//!
//! Each module defines one [`Visualization`](crate::viz::Visualization)
//! impl. New kinds live in their own module and register via
//! [`AppVizExt::register_visualization`](crate::registry::AppVizExt).

pub mod line_plot;
