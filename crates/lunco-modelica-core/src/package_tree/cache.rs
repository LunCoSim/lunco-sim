//! Resource state and result types for the Package Browser.

use super::types::{InMemoryEntry, PackageNode};
use crate::state::ModelLibrary;
use bevy::prelude::*;
use bevy::tasks::Task;

pub struct ScanResult {
    pub parent_id: String,
    pub children: Vec<PackageNode>,
}

/// Output of a bundled or user-file load task. `result` is the
/// outcome — `Ok(doc)` for a successful read, `Err(msg)` for any IO
/// or decode failure that should be surfaced as a load-failed
/// overlay (`StatusBus` → `BusyOutcome::Failed`). Display name /
/// library / dedup key live on the matching
/// `ui::document_openings::OpeningState::FileLoad` for
/// the duration of the load.
pub struct FileLoadResult {
    pub doc_id: lunco_doc::DocumentId,
    pub result: Result<crate::document::ModelicaDocument, String>,
}

#[derive(Resource)]
pub struct PackageTreeCache {
    pub roots: Vec<PackageNode>,
    pub tasks: Vec<Task<ScanResult>>,
    pub in_memory_models: Vec<InMemoryEntry>,
    pub bundled_tree_indexed: bool,
    /// Whether the library roots have been reconciled against the provider
    /// after construction. Native is complete at `new()`; web gains its
    /// bundle-derived extra roots once `MslLoadState::Ready`. See
    /// [`Self::reconcile_library_roots`].
    pub library_roots_synced: bool,
}

impl PackageTreeCache {
    pub fn new() -> Self {
        // Library roots come from the cfg-free provider: native discovers them
        // on the filesystem at construction; web returns just "Modelica" here
        // and the extras are filled in by `reconcile_library_roots` once the
        // parsed bundle loads. `library_root_node` decorates "Modelica" with
        // its display name / stable id.
        let mut roots: Vec<PackageNode> = super::library_tree::library_tree()
            .library_roots()
            .iter()
            .map(|lib| super::library_tree::library_root_node(lib))
            .collect();

        // Bundled models are available from the embedded asset inventory before
        // the optional editor index has been decoded. Always keep this root
        // last; the generated index enriches it through its readiness event.
        roots.push(PackageNode::Category {
            id: "bundled_root".into(),
            name: "LunCo Examples".into(),
            package_path: "Bundled".into(),
            fs_path: std::path::PathBuf::new(),
            children: Some(build_bundled_tree()),
            is_loading: false,
        });

        let bundled_tree_indexed = !crate::visual_diagram::msl_bundled_nodes().is_empty();

        Self {
            roots,
            tasks: Vec::new(),
            in_memory_models: Vec::new(),
            bundled_tree_indexed,
            library_roots_synced: false,
        }
    }

    /// Insert any library roots that became known after construction — on web,
    /// the third-party libs carried by the parsed bundle, which isn't resident
    /// when `new()` runs. Idempotent and flag-gated; native is already complete
    /// at `new()` so this only flips the flag. Preserves existing nodes and
    /// their expansion state, inserting extras just before the bundled-examples
    /// root. Call only once the bundle is ready (see the reconcile system).
    pub fn reconcile_library_roots(&mut self) {
        if self.library_roots_synced {
            return;
        }

        // Cancel any in-flight library scans that started before MSL was ready.
        self.tasks.clear();

        // Reset library roots (e.g. Modelica) so they retry expanding with the now-available MSL.
        for root in &mut self.roots {
            if let PackageNode::Category {
                id,
                children,
                is_loading,
                ..
            } = root
            {
                if id != "bundled_root" {
                    *children = None;
                    *is_loading = false;
                }
            }
        }
        let existing: std::collections::HashSet<String> = self
            .roots
            .iter()
            .filter_map(|n| match n {
                PackageNode::Category { package_path, .. } => Some(package_path.clone()),
                _ => None,
            })
            .collect();
        let insert_at = self
            .roots
            .iter()
            .position(|n| matches!(n, PackageNode::Category { id, .. } if id == "bundled_root"))
            .unwrap_or(self.roots.len());
        let mut offset = 0;
        for lib in super::library_tree::library_tree().library_roots() {
            if existing.contains(&lib) {
                continue;
            }
            self.roots.insert(
                insert_at + offset,
                super::library_tree::library_root_node(&lib),
            );
            offset += 1;
        }
        self.library_roots_synced = true;
    }
}

impl Default for PackageTreeCache {
    fn default() -> Self {
        Self::new()
    }
}

/// Initial bundled-models tree from the asset-owned embedded inventory.
/// `MslEditorIndexBecameReady` replaces these leaves with the generated tree
/// after its decode completes off-thread.
fn build_bundled_tree() -> Vec<PackageNode> {
    crate::models::bundled_models()
        .iter()
        .map(|m| PackageNode::Model {
            id: format!("bundled://{}", m.filename),
            name: m
                .filename
                .strip_suffix(".mo")
                .unwrap_or(m.filename)
                .to_string(),
            library: ModelLibrary::Bundled,
            class_kind: Some(crate::index::ClassKind::Model),
        })
        .collect()
}
