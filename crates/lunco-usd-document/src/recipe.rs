//! Send-safe input recipe for a composed OpenUSD stage.

use std::collections::HashMap;

/// A resolved root identifier plus the complete transitive layer closure.
///
/// The recipe crosses async/task boundaries as data. The Bevy runtime crate
/// consumes it to build its non-`Send` canonical stage; document authoring uses
/// the same bytes to resolve authored arcs without owning runtime state.
#[derive(Debug, Clone)]
pub struct StageRecipe {
    /// Canonical identifier of the root layer.
    pub root_id: String,
    /// Layer identifier to file bytes for the complete closure.
    pub bytes: HashMap<String, Vec<u8>>,
}

impl StageRecipe {
    /// Build a single-layer recipe for an in-memory source.
    pub fn from_source(root_id: impl Into<String>, source: &str) -> Self {
        let root_id = root_id.into();
        let bytes = HashMap::from([(root_id.clone(), source.as_bytes().to_vec())]);
        Self { root_id, bytes }
    }
}
