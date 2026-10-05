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
    /// Assemble this fetched closure under a new USD root identity. The source
    /// transport remains outside the recipe: relative composition arcs are
    /// interpreted under both layer anchors by the shared USD dependency
    /// reader, and explicit absolute identifiers keep their authored identity.
    /// No alias identifiers or rewritten layer bytes are retained.
    pub fn reanchor(&self, root_id: &str) -> anyhow::Result<Self> {
        let root_id = crate::canonicalize_at(root_id, None)?;
        let limits = StageClosureLimits::default();
        let mut total_bytes = 0_usize;
        let mut output = Self::new(root_id.clone(), HashMap::new());
        let mut frontier =
            std::collections::VecDeque::from([(self.root_id.clone(), root_id, 0_usize)]);
        let mut seen = std::collections::HashSet::new();
        while let Some((source_id, target_id, depth)) = frontier.pop_front() {
            if !seen.insert((source_id.clone(), target_id.clone())) {
                continue;
            }
            let raw = self.bytes.get(&source_id).ok_or_else(|| {
                anyhow::anyhow!("USD closure root `{source_id}` has no fetched content")
            })?;
            if let Some(previous) = output.bytes.get(&target_id) {
                anyhow::ensure!(
                    previous == raw,
                    "USD reanchored layers disagree at `{target_id}`"
                );
            } else {
                total_bytes = total_bytes.checked_add(raw.len()).ok_or_else(|| {
                    anyhow::anyhow!("USD reanchored closure byte count overflowed")
                })?;
                crate::check_stage_closure_limits(
                    &limits,
                    output.bytes.len() + 1,
                    depth,
                    0,
                    total_bytes,
                )?;
                output.bytes.insert(target_id.clone(), raw.clone());
            }
            let source_children = crate::child_layer_ids(&source_id, raw)?;
            let target_children = crate::child_layer_ids(&target_id, raw)?;
            crate::check_stage_closure_limits(
                &limits,
                output.bytes.len(),
                depth,
                target_children.len(),
                total_bytes,
            )?;
            anyhow::ensure!(
                source_children.len() == target_children.len(),
                "USD reanchoring changed composition dependency classification"
            );
            for (source_child, target_child) in source_children.into_iter().zip(target_children) {
                if self.bytes.contains_key(&source_child) {
                    frontier.push_back((source_child, target_child, depth + 1));
                } else {
                    output
                        .dependency_diagnostics
                        .push(StageDependencyDiagnostic::missing(
                            target_id.clone(),
                            target_child,
                        ));
                }
            }
        }
        Ok(output)
    }

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
    fn reanchor_composes_nested_arcs_and_preserves_authored_asset_values() {
        let transport_root = "twin://fixture/models/root.usda";
        let absolute = "lunco://library/shared.usda";
        let root = br#"#usda 1.0
(
    defaultPrim = "Asset"
    subLayers = [@parts/child.usda@, @lunco://library/shared.usda@]
)
def Xform "Asset" {}
"#;
        let child = br#"#usda 1.0
(
    subLayers = [@grand.usda@]
)
over "Asset" {
    custom asset texture = @images/paint #%.png@
}
"#;
        let grand = b"#usda 1.0\nover \"Asset\" { custom string nested = \"composed\" }\n";
        let absolute_bytes = b"#usda 1.0\nover \"Asset\" { custom string shared = \"absolute\" }\n";
        let recipe = StageRecipe::new(
            transport_root,
            HashMap::from([
                (transport_root.to_owned(), root.to_vec()),
                (
                    "twin://fixture/models/parts/child.usda".to_owned(),
                    child.to_vec(),
                ),
                (
                    "twin://fixture/models/parts/grand.usda".to_owned(),
                    grand.to_vec(),
                ),
                (absolute.to_owned(), absolute_bytes.to_vec()),
            ]),
        );
        let native_root = std::env::temp_dir()
            .join("generic recipe # %")
            .join("root.usda");
        let native_root = lunco_storage::file_path_to_uri(&native_root).expect("native root URI");
        for anchor in [transport_root, native_root.as_str()] {
            let prepared = recipe.reanchor(anchor).expect("reanchor fetched closure");
            assert_eq!(prepared.root_id, anchor);
            assert_eq!(prepared.bytes.len(), 4);
            assert!(prepared.bytes.contains_key(absolute));
            let child_id = crate::child_layer_ids(anchor, root).unwrap()[0].clone();
            let grand_id = crate::child_layer_ids(&child_id, child).unwrap()[0].clone();
            assert_eq!(prepared.bytes[&child_id], child);
            assert_eq!(prepared.bytes[&grand_id], grand);
            if anchor != transport_root {
                assert!(!prepared.bytes.contains_key(transport_root));
            }
            let resolver = crate::LuncoUsdResolver::new(prepared.bytes).expect("valid resolver");
            let stage = openusd::usd::Stage::builder()
                .resolver(resolver)
                .open(anchor)
                .expect("compose closure");
            let prim = stage.prim(openusd::sdf::Path::new("/Asset").unwrap());
            assert_eq!(
                prim.attribute("nested").get::<String>().unwrap().as_deref(),
                Some("composed")
            );
            assert_eq!(
                prim.attribute("shared").get::<String>().unwrap().as_deref(),
                Some("absolute")
            );
            let attribute = prim.attribute("texture");
            let value = attribute
                .get::<openusd::sdf::Value>()
                .unwrap()
                .unwrap()
                .try_as_asset_path()
                .expect("asset value");
            assert_eq!(value.as_str(), "images/paint #%.png");
            let stack = attribute.property_stack().unwrap();
            assert_eq!(stack[0].0, child_id);
            let image_id = crate::canonicalize_at(
                value.as_str(),
                Some(&openusd::ar::ResolvedPath::new(&stack[0].0)),
            )
            .unwrap();
            if anchor == transport_root {
                assert_eq!(image_id, "twin://fixture/models/parts/images/paint #%.png");
            } else {
                assert_eq!(
                    lunco_storage::file_uri_to_path(&image_id).unwrap().unwrap(),
                    lunco_storage::file_uri_to_path(anchor)
                        .unwrap()
                        .unwrap()
                        .parent()
                        .unwrap()
                        .join("parts/images/paint #%.png")
                );
            }
        }
    }

    #[test]
    fn reanchor_retains_missing_arcs_and_rejects_invalid_roots() {
        let recipe = StageRecipe::from_source(
            "twin://fixture/root.usda",
            "#usda 1.0\n( subLayers = [@missing.usda@] )\ndef Scope \"Root\" {}\n",
        );
        let root = lunco_storage::file_path_to_uri(&std::env::temp_dir().join("generic root.usda"))
            .unwrap();
        let prepared = recipe.reanchor(&root).unwrap();
        assert_eq!(prepared.dependency_diagnostics.len(), 1);
        assert_eq!(prepared.dependency_diagnostics[0].referring_layer, root);
        assert!(
            prepared.dependency_diagnostics[0]
                .dependency
                .starts_with("file:")
        );
        assert!(recipe.reanchor("file:///invalid%00.usda").is_err());
        assert!(
            StageRecipe::new("missing.usda", HashMap::new())
                .reanchor(&root)
                .is_err()
        );
    }

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
