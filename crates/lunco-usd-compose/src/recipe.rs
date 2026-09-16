//! Send-safe input recipe for a composed OpenUSD stage.

use std::collections::HashMap;

/// A dependency that USD could not materialize while opening a stage.
///
/// The identifiers are logical USD asset paths, never native filesystem paths.
/// Keeping the diagnostic at this boundary means the same authored problem is
/// reported identically on Windows, macOS, Linux, and wasm.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StageDependencyDiagnostic {
    /// Layer that authored the composition arc.
    pub referring_layer: String,
    /// Canonical logical identifier of the missing dependency.
    pub dependency: String,
}

impl StageDependencyDiagnostic {
    /// Describe one missing dependency without leaking the reader's native
    /// cache path into the portable diagnostic.
    pub fn missing(referring_layer: impl Into<String>, dependency: impl Into<String>) -> Self {
        Self {
            referring_layer: referring_layer.into(),
            dependency: dependency.into(),
        }
    }
}

impl std::fmt::Display for StageDependencyDiagnostic {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "USD composition dependency `{}` referenced by `{}` was not found; the available parts \
             were loaded, but this authored arc remains unresolved",
            self.dependency,
            self.referring_layer
        )
    }
}

/// Bounds for one USD layer-closure fetch.
///
/// These limits protect both the asynchronous prefetch path and the native
/// composition path from an authored dependency graph that is unexpectedly
/// deep, wide, or large. Missing files do not consume the byte budget, but do
/// consume the layer/dependency budget because they are still graph nodes that
/// must be diagnosed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StageClosureLimits {
    /// Maximum number of distinct discovered layer identifiers, including the root.
    pub max_layers: usize,
    /// Maximum number of composition dependencies declared by one layer.
    pub max_dependencies_per_layer: usize,
    /// Maximum logical dependency depth from the root layer.
    pub max_depth: usize,
    /// Maximum total bytes retained for successfully fetched layers.
    pub max_bytes: usize,
}

impl Default for StageClosureLimits {
    fn default() -> Self {
        Self {
            max_layers: 4096,
            max_dependencies_per_layer: 4096,
            max_depth: 128,
            max_bytes: 256 * 1024 * 1024,
        }
    }
}

/// A resolved root identifier plus the successfully fetched transitive layer
/// closure. Missing dependencies remain absent and are recorded in
/// [`StageRecipe::dependency_diagnostics`].
///
/// The recipe crosses async/task boundaries as data. The Bevy runtime crate
/// consumes it to build its non-`Send` canonical stage; document authoring uses
/// the same bytes to resolve authored arcs without owning runtime state.
#[derive(Debug, Clone)]
pub struct StageRecipe {
    /// Canonical identifier of the root layer.
    pub root_id: String,
    /// Layer identifier to file bytes for the available closure.
    pub bytes: HashMap<String, Vec<u8>>,
    /// Non-fatal missing dependencies discovered while fetching the closure.
    /// OpenUSD independently retains its own composition diagnostics when the
    /// recipe is opened; this field preserves the async reader's logical URI
    /// context for runtime/API consumers.
    pub dependency_diagnostics: Vec<StageDependencyDiagnostic>,
}

impl StageRecipe {
    /// Build a recipe from a root and its successfully fetched layer bytes.
    pub fn new(root_id: impl Into<String>, bytes: HashMap<String, Vec<u8>>) -> Self {
        Self {
            root_id: root_id.into(),
            bytes,
            dependency_diagnostics: Vec::new(),
        }
    }

    /// Build a single-layer recipe for an in-memory source.
    pub fn from_source(root_id: impl Into<String>, source: &str) -> Self {
        let root_id = root_id.into();
        let bytes = HashMap::from([(root_id.clone(), source.as_bytes().to_vec())]);
        Self::new(root_id, bytes)
    }
}
