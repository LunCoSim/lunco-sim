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
            self.dependency, self.referring_layer
        )
    }
}

/// Bounds for one USD layer-closure fetch.
///
/// These limits protect both the asynchronous prefetch path and the native
/// composition path from an authored dependency graph that is unexpectedly
/// deep, wide, or large. Missing files do not consume the byte budget, but do
/// consume the layer/dependency budget because they are still graph nodes that
/// must be diagnosed. `max_parallel_reads` bounds only sibling I/O in the
/// asynchronous Bevy loader.
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
    /// Maximum number of sibling layers read concurrently by the Bevy loader.
    pub max_parallel_reads: usize,
}

impl Default for StageClosureLimits {
    fn default() -> Self {
        Self {
            max_layers: 4096,
            max_dependencies_per_layer: 4096,
            max_depth: 128,
            max_bytes: 256 * 1024 * 1024,
            max_parallel_reads: 16,
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

/// Stable content identity for one layer in an admitted USD composition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StageLayerContent {
    /// Canonical logical identifier used by the USD resolver.
    pub layer_id: String,
    /// Canonical CIDv1 raw + SHA-256 identity for the exact fetched layer content.
    pub cid: lunco_hash::content::Cid,
}

/// Content-addressed snapshot of a complete root USD composition closure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StageContentClosure {
    /// Canonical logical identifier of the root layer.
    pub root_id: String,
    /// All fetched layers, sorted by `layer_id` for stable comparison/encoding.
    pub layers: Vec<StageLayerContent>,
}

/// Why a USD recipe cannot provide a complete replay identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StageContentClosureError {
    /// The root layer was not among the successfully fetched bytes.
    RootLayerMissing { root_id: String },
    /// Composition had missing dependencies, so the available bytes are partial.
    UnresolvedDependencies(Vec<StageDependencyDiagnostic>),
    /// A fetched layer has no usable resolver identity.
    EmptyLayerIdentifier,
}

impl std::fmt::Display for StageContentClosureError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RootLayerMissing { root_id } => {
                write!(
                    formatter,
                    "USD root layer `{root_id}` has no fetched content"
                )
            }
            Self::UnresolvedDependencies(diagnostics) => {
                formatter.write_str("USD composition has unresolved dependencies: ")?;
                for (index, diagnostic) in diagnostics.iter().enumerate() {
                    if index > 0 {
                        formatter.write_str(", ")?;
                    }
                    write!(
                        formatter,
                        "`{}` referenced by `{}`",
                        diagnostic.dependency, diagnostic.referring_layer
                    )?;
                }
                Ok(())
            }
            Self::EmptyLayerIdentifier => {
                formatter.write_str("USD composition contains an empty layer identifier")
            }
        }
    }
}

impl std::error::Error for StageContentClosureError {}

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

    /// Snapshot the complete fetched closure using stable content identities.
    ///
    /// The recipe must contain its root layer and have no unresolved
    /// composition dependencies. The returned rows are independent of
    /// `HashMap` iteration order and use canonical CIDv1 raw + SHA-256 content
    /// identities. Process-local stage revisions are deliberately not used.
    pub fn content_closure(&self) -> Result<StageContentClosure, StageContentClosureError> {
        if !self.dependency_diagnostics.is_empty() {
            let mut diagnostics = self.dependency_diagnostics.clone();
            diagnostics.sort_by(|left, right| {
                (&left.referring_layer, &left.dependency)
                    .cmp(&(&right.referring_layer, &right.dependency))
            });
            return Err(StageContentClosureError::UnresolvedDependencies(
                diagnostics,
            ));
        }
        if !self.bytes.contains_key(&self.root_id) {
            return Err(StageContentClosureError::RootLayerMissing {
                root_id: self.root_id.clone(),
            });
        }

        let mut layers = self
            .bytes
            .iter()
            .map(|(layer_id, bytes)| {
                if layer_id.is_empty() {
                    return Err(StageContentClosureError::EmptyLayerIdentifier);
                }
                Ok(StageLayerContent {
                    layer_id: layer_id.clone(),
                    cid: lunco_hash::content::cid(bytes),
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        layers.sort_by(|left, right| left.layer_id.cmp(&right.layer_id));

        Ok(StageContentClosure {
            root_id: self.root_id.clone(),
            layers,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_closure_is_stable_and_addresses_every_layer() {
        let root = "twin://demo/scenes/root.usda";
        let child = "twin://demo/models/rover.usda";
        let first = StageRecipe::new(
            root,
            HashMap::from([
                (child.to_owned(), b"child".to_vec()),
                (root.to_owned(), b"root".to_vec()),
            ]),
        );
        let second = StageRecipe::new(
            root,
            HashMap::from([
                (root.to_owned(), b"root".to_vec()),
                (child.to_owned(), b"child".to_vec()),
            ]),
        );

        let snapshot = first.content_closure().expect("complete closure");
        assert_eq!(snapshot, second.content_closure().expect("same closure"));
        assert_eq!(snapshot.root_id, root);
        assert_eq!(snapshot.layers[0].layer_id, child);
        assert_eq!(snapshot.layers[0].cid, lunco_hash::content::cid(b"child"));
        assert_eq!(snapshot.layers[1].cid, lunco_hash::content::cid(b"root"));
    }

    #[test]
    fn content_closure_rejects_partial_or_unidentified_recipes() {
        let missing_root = StageRecipe::new("root.usda", HashMap::new());
        assert_eq!(
            missing_root.content_closure(),
            Err(StageContentClosureError::RootLayerMissing {
                root_id: "root.usda".into()
            })
        );

        let mut partial = StageRecipe::from_source("root.usda", "root");
        partial
            .dependency_diagnostics
            .push(StageDependencyDiagnostic::missing(
                "root.usda",
                "child.usda",
            ));
        assert_eq!(
            partial.content_closure(),
            Err(StageContentClosureError::UnresolvedDependencies(vec![
                StageDependencyDiagnostic::missing("root.usda", "child.usda")
            ]))
        );

        let unidentified = StageRecipe::new(
            "root.usda",
            HashMap::from([
                ("root.usda".into(), b"root".to_vec()),
                (String::new(), b"unknown".to_vec()),
            ]),
        );
        assert_eq!(
            unidentified.content_closure(),
            Err(StageContentClosureError::EmptyLayerIdentifier)
        );
    }
}
