//! Package-tree backend — egui-free data + scanning logic for the
//! Modelica library/package browser.
//!
//! Moved out of the (egui-gated) `ui` module so the server / headless
//! build can index and resolve packages without pulling in egui. The
//! egui rendering of this tree lives in `ui::panels::package_browser`.

pub mod cache;
pub mod library_tree;
pub mod scanner;

pub use cache::{FileLoadResult, PackageTreeCache, ScanResult};
pub use scanner::peek_class_kind_from_source;
