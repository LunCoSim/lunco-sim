//! Package-tree backend — egui-free data + scanning logic for the
//! Modelica library/package browser.
//!
//! Moved out of the (egui-gated) `ui` module so the server / headless
//! build can index and resolve packages without pulling in egui. The
//! egui rendering of this tree lives in `ui::panels::package_browser`.

pub mod types;
pub mod scanner;
pub mod cache;
pub mod library_tree;

pub use types::{PackageNode, InMemoryEntry, TwinNode};
pub use cache::{PackageTreeCache, ScanResult, FileLoadResult, TwinState, RenameState};
pub use scanner::{scan_twin_folder, discover_third_party_libs, peek_class_kind_from_source};
