//! Shared simulation state for the Modelica workbench UI.
//!
//! ## Entity Viewer Pattern
//!
//! This resource is the **selection bridge** between any context (library browser,
//! 3D viewport click, colony tree) and the Modelica editor panels.
//!
//! `selected_entity` is the single source of truth — panels watch it and
//! render data for the active `ModelicaModel`. Any context can set it:
//!
//! ```rust,ignore
//! // Library Browser: double-click a .mo file
//! // 3D viewport: click a rover's solar panel
//! // Colony tree: select a subsystem node
//! state.selected_entity = Some(entity);
//! ```
//!
//! Panels don't know where the entity came from. They just render it.

use bevy::prelude::*;
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use lunco_assets::assets_dir;
#[cfg(target_arch = "wasm32")]
use std::sync::atomic::{AtomicPtr, Ordering};

/// Static cell bridging JS file picker → Bevy system on wasm32.
/// Set by `set_file_load_result` when user selects a .mo file.
/// Read and cleared by `update_file_load_result` each frame.
#[cfg(target_arch = "wasm32")]
static FILE_LOAD_CELL: AtomicPtr<String> = AtomicPtr::new(std::ptr::null_mut());

/// Called from JS when a .mo file is loaded via browser file picker.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn set_file_load_result(content: &str) {
    let prev = FILE_LOAD_CELL.swap(Box::into_raw(Box::new(content.to_string())), Ordering::SeqCst);
    if !prev.is_null() {
        unsafe { drop(Box::from_raw(prev)); }
    }
}

/// Consumes pending file load from browser file picker and updates editor buffer.
/// Runs each frame on wasm32.
#[cfg(target_arch = "wasm32")]
pub fn update_file_load_result(mut state: ResMut<WorkbenchState>) {
    let prev = FILE_LOAD_CELL.swap(std::ptr::null_mut(), Ordering::SeqCst);
    if !prev.is_null() {
        let content = unsafe { Box::from_raw(prev) };
        state.editor_buffer = *content;
    }
}

/// Shared state for the Modelica workbench UI.
///
/// This is the **selection bridge** — `selected_entity` connects any
/// triggering context (library, 3D click, tree) to the editor panels.
#[derive(Resource)]
pub struct WorkbenchState {
    /// Current directory path for the library browser.
    pub current_path: PathBuf,
    /// Current Modelica source code in the editor.
    pub editor_buffer: String,
    /// **Selection bridge**: which `ModelicaModel` entity panels are viewing.
    /// Set by any context (library, 3D viewport, colony tree).
    pub selected_entity: Option<Entity>,
    /// Last compilation error message, if any.
    pub compilation_error: Option<String>,
    /// Time-series data for plotted variables, keyed by entity → variable name.
    pub history: HashMap<Entity, HashMap<String, VecDeque<[f64; 2]>>>,
    /// Variable names the user has toggled for plotting.
    pub plotted_variables: std::collections::HashSet<String>,
    /// Maximum history points to retain per variable.
    pub max_history: usize,
    /// Whether plots should auto-fit their axes.
    pub plot_auto_fit: bool,
}

impl Default for WorkbenchState {
    fn default() -> Self {
        Self {
            current_path: assets_dir().join("models"),
            editor_buffer: String::new(),
            selected_entity: None,
            compilation_error: None,
            history: HashMap::new(),
            plotted_variables: std::collections::HashSet::new(),
            max_history: 10000,
            plot_auto_fit: false,
        }
    }
}
