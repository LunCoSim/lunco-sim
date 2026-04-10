//! Shared simulation state for the Modelica workbench UI.
//!
//! This resource holds data shared across all panels:
//! - `selected_entity`: which model entity is active
//! - `history`: time-series data for plotted variables
//! - `logs`: system log messages
//! - `editor_buffer`: current source code

use bevy::prelude::*;
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
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

#[derive(Resource)]
pub struct WorkbenchState {
    pub current_path: PathBuf,
    pub editor_buffer: String,
    pub selected_entity: Option<Entity>,
    pub compilation_error: Option<String>,
    pub history: HashMap<Entity, HashMap<String, VecDeque<[f64; 2]>>>,
    pub plotted_variables: std::collections::HashSet<String>,
    pub logs: VecDeque<String>,
    pub max_history: usize,
    pub plot_auto_fit: bool,
}

impl Default for WorkbenchState {
    fn default() -> Self {
        Self {
            current_path: PathBuf::from("assets/models"),
            editor_buffer: String::new(),
            selected_entity: None,
            compilation_error: None,
            history: HashMap::new(),
            plotted_variables: std::collections::HashSet::new(),
            logs: VecDeque::new(),
            max_history: 10000,
            plot_auto_fit: false,
        }
    }
}
