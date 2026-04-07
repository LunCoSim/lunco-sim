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
