//! Reusable OS-window capability for LunCoSim hosts.
//!
//! This package owns typed window commands, merged-titlebar construction,
//! persisted primary-window geometry, and explicit command-line placement.
//! It does not depend on the concrete Workbench dock shell, so window policy
//! changes do not rebuild the shell's layout implementation.

pub mod window_command;
pub mod window_persistence;
pub mod window_placement;

pub use window_command::{
    CloseWindow, MaximizeWindow, MinimizeWindow, WindowCommandPlugin, WindowMaximized,
    merged_titlebar_window,
};
pub use window_persistence::{
    DEFAULT_WINDOW_HEIGHT, DEFAULT_WINDOW_WIDTH, SkipWindowGeometrySave, WindowGeometry,
    WindowPersistencePlugin, load_window_geometry, restored_window,
};
#[cfg(not(target_arch = "wasm32"))]
pub use window_placement::WindowPlacement;
pub use window_placement::wire_window_placement;
