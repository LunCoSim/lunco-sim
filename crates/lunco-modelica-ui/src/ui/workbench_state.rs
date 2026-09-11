//! Ephemeral state owned by the Modelica editor presentation.

use bevy::prelude::*;

/// Selection and live-buffer bridge between editor panels and their host.
#[derive(Resource, Default)]
pub struct WorkbenchState {
    /// Current source text owned by the editor widget.
    pub editor_buffer: String,
    /// Entity currently shown by the Modelica panels.
    pub selected_entity: Option<Entity>,
}

#[cfg(target_arch = "wasm32")]
mod wasm_file_picker {
    use super::WorkbenchState;
    use bevy::prelude::ResMut;
    use std::sync::atomic::{AtomicPtr, Ordering};

    static FILE_LOAD_CELL: AtomicPtr<String> = AtomicPtr::new(std::ptr::null_mut());

    /// Called from JavaScript when the browser file picker returns a `.mo` file.
    #[wasm_bindgen::prelude::wasm_bindgen]
    pub fn set_file_load_result(content: &str) {
        let prev = FILE_LOAD_CELL.swap(
            Box::into_raw(Box::new(content.to_string())),
            Ordering::SeqCst,
        );
        if !prev.is_null() {
            unsafe {
                drop(Box::from_raw(prev));
            }
        }
    }

    /// Transfer a pending browser file into the editor-owned buffer.
    pub fn update_file_load_result(mut state: ResMut<WorkbenchState>) {
        let prev = FILE_LOAD_CELL.swap(std::ptr::null_mut(), Ordering::SeqCst);
        if !prev.is_null() {
            let content = unsafe { Box::from_raw(prev) };
            state.editor_buffer = *content;
        }
    }
}

#[cfg(target_arch = "wasm32")]
pub use wasm_file_picker::{set_file_load_result, update_file_load_result};
