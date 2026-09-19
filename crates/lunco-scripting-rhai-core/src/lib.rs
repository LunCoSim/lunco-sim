//! Low-level Rhai backend mechanics shared by production scripting hosts.
//!
//! This package owns asset-scoped module resolution, native vector/quaternion
//! functions, task-tree lowering, typed UI request values, and persisted-name
//! validation. The parent `lunco-scripting` package owns lifecycle, documents,
//! commands, and world integration.

pub mod module_resolver;
pub mod names;
pub mod rhai_assembly;
pub mod rhai_math;
pub mod task_tree;
pub mod ui_bridge;
pub mod values;
