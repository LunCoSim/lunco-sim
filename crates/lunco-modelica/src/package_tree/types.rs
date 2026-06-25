//! Tree node types and basic structures for the Package Browser.

use crate::state::ModelLibrary;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum PackageNode {
    Category {
        id: String,
        name: String,
        /// Modelica dot-path (e.g. "Modelica.Electrical.Analog")
        package_path: String,
        /// Real filesystem path. Empty for pre-baked bundled tree
        /// nodes, which have no on-disk location.
        #[serde(default)]
        fs_path: std::path::PathBuf,
        /// None means not yet scanned. Some(vec![]) means scanned and empty.
        #[serde(default)]
        children: Option<Vec<PackageNode>>,
        /// Whether a background scan is currently in progress.
        #[serde(default, skip)]
        is_loading: bool,
    },
    Model {
        id: String,
        name: String,
        library: ModelLibrary,
        /// Modelica class kind, derived from the rumoca-parsed AST
        /// (or pre-baked from `msl_index.json` for bundled rows).
        /// `None` for legacy / fallback entries where the kind
        /// couldn't be determined.
        class_kind: Option<crate::index::ClassKind>,
    },
}

impl PackageNode {
    pub fn name(&self) -> &str {
        match self {
            PackageNode::Category { name, .. } | PackageNode::Model { name, .. } => name,
        }
    }
}

/// Tracks one in-memory ("scratch") model the user has created this
/// session.
#[derive(Debug, Clone)]
pub struct InMemoryEntry {
    pub display_name: String,
    pub id: String,
    pub doc: lunco_doc::DocumentId,
}

#[derive(Clone)]
pub struct TwinNode {
    pub path: std::path::PathBuf,
    pub name: String,
    pub children: Vec<TwinNode>,
    pub is_modelica: bool,
}
