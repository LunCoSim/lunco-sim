//! Shared simulation state for the Modelica workbench UI.
//!
//! This resource holds data that is shared across all panels:
//! - `selected_entity`: which model entity is currently active
//! - `history`: time-series data for plotted variables
//! - `logs`: system log messages
//! - `editor_buffer`: current source code (used by editor and parameter substitution)

use bevy::prelude::*;
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;

/// Shared state for the Modelica workbench UI.
///
/// This resource is accessible by all panel plugins. It contains:
/// - Selection state (which entity is active)
/// - Simulation history (time-series data for plots)
/// - Editor content (source code buffer)
/// - System logs
/// - Plot configuration
#[derive(Resource)]
pub struct WorkbenchState {
    /// Current file browser location.
    pub current_path: PathBuf,
    /// Current source code being edited.
    pub editor_buffer: String,
    /// Which entity is selected for editing/monitoring.
    pub selected_entity: Option<Entity>,
    /// Last compilation error, if any.
    pub compilation_error: Option<String>,
    /// History of variables: Entity → VariableName → DataPoints `[time, value]`.
    pub history: HashMap<Entity, HashMap<String, VecDeque<[f64; 2]>>>,
    /// Which variables are currently plotted.
    pub plotted_variables: std::collections::HashSet<String>,
    /// System log messages (newest at back).
    pub logs: VecDeque<String>,
    /// Maximum number of data points to keep per variable.
    pub max_history: usize,
    /// When true, the next plot render will call `.reset()` to auto-fit the view.
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
