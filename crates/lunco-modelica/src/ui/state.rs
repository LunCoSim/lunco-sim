//! Shared simulation state for the Modelica workbench UI.
//!
//! ## Entity Viewer Pattern
//!
//! This resource is the **selection bridge** between any context (library browser,
//! 3D viewport click, colony tree) and the Modelica editor panels.
//!
//! `selected_entity` is the single source of truth — panels watch it and
//! render data for the active `ModelicaModel`. Any context can set it:
//!
//! ```rust,ignore
//! // Library Browser: double-click a .mo file
//! // 3D viewport: click a rover's solar panel
//! // Colony tree: select a subsystem node
//! state.selected_entity = Some(entity);
//! ```
//!
//! Panels don't know where the entity came from. They just render it.

use bevy::prelude::*;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use lunco_assets::assets_dir;
use lunco_doc::{DocumentHost, DocumentId};
#[cfg(target_arch = "wasm32")]
use std::sync::atomic::{AtomicPtr, Ordering};

use std::sync::Arc;

use crate::document::{ModelicaDocument, ModelicaOp};

// ---------------------------------------------------------------------------
// Model File Tracking
// ---------------------------------------------------------------------------

/// Which model is currently open in the editor.
#[derive(Debug, Clone, Default)]
pub struct OpenModel {
    /// Modelica package path (e.g., "Modelica.Electrical.Analog.Basic.Resistor")
    /// or file path for user models (e.g., "Battery.mo").
    pub model_path: String,
    /// Display name shown in breadcrumb (e.g., "Resistor" or "Battery").
    pub display_name: String,
    /// Source code text.
    pub source: Arc<str>,
    /// Byte offsets of the start of each line (prevents O(N) string allocations).
    pub line_starts: Arc<[usize]>,
    /// Memoized model name from AST.
    pub detected_name: Option<String>,
    /// Pre-computed text layout for high-performance rendering.
    pub cached_galley: Option<Arc<bevy_egui::egui::Galley>>,
    /// Whether this model is read-only.
    pub read_only: bool,
    /// Which library this model came from.
    pub library: ModelLibrary,
}

/// Which library a model belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ModelLibrary {
    /// Modelica Standard Library (read-only).
    MSL,
    /// Bundled models shipped with LunCoSim (read-only for now).
    #[default]
    Bundled,
    /// User-created models (writable, from opened folder).
    User,
    /// In-memory model created by user (writable until saved).
    InMemory,
}

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

/// Shared state for the Modelica workbench UI.
///
/// This is the **selection bridge** — `selected_entity` connects any
/// triggering context (library, 3D click, tree) to the editor panels.
#[derive(Resource)]
pub struct WorkbenchState {
    /// Current directory path for the library browser.
    pub current_path: PathBuf,
    /// Current Modelica source code in the editor.
    pub editor_buffer: String,
    /// Path of the file that produced the current editor buffer.
    /// Used to highlight the active file in the library browser.
    pub loaded_file_path: Option<PathBuf>,
    /// **Selection bridge**: which `ModelicaModel` entity panels are viewing.
    /// Set by any context (library, 3D viewport, colony tree).
    pub selected_entity: Option<Entity>,
    /// Last compilation error message, if any.
    pub compilation_error: Option<String>,
    /// Time-series data for plotted variables, keyed by entity → variable name.
    pub history: HashMap<Entity, HashMap<String, VecDeque<[f64; 2]>>>,
    /// Variable names the user has toggled for plotting.
    pub plotted_variables: HashSet<String>,
    /// Maximum history points to retain per variable.
    pub max_history: usize,
    /// Whether plots should auto-fit their axes.
    pub plot_auto_fit: bool,

    // ── Dymola-style navigation ──

    /// Which model is currently open in the editor area.
    /// Set when user clicks a file in the package browser.
    pub open_model: Option<OpenModel>,
    /// Navigation stack for back-button support.
    /// Each entry is a model_path that was previously open.
    pub navigation_stack: Vec<String>,
    /// Flag to signal the diagram panel should rebuild from open_model source.
    pub diagram_dirty: bool,
    /// Whether a model is currently being loaded in the background.
    pub is_loading: bool,
}

// ---------------------------------------------------------------------------
// ModelicaDocumentRegistry — per-entity DocumentHost tracking
// ---------------------------------------------------------------------------

/// Per-entity registry of [`DocumentHost<ModelicaDocument>`] instances.
///
/// Each `ModelicaModel` entity gets a `DocumentHost` whose [`ModelicaDocument`]
/// mirrors the **last-compiled source** for that entity. The document is
/// checkpointed via [`checkpoint_source`](Self::checkpoint_source) on every
/// successful compile, giving us:
///
/// - A canonical per-entity source history (undo/redo)
/// - A generation counter other panels can observe for change detection
/// - The foundation for cross-panel source-sharing once the Diagram /
///   Telemetry panels migrate off `ModelicaModel.original_source`
///
/// This is intentionally **shadow state** during the current migration step.
/// The live editing pipeline still flows through `EditorBufferState` and
/// `ModelicaModel`; the registry tracks committed sources alongside. Later
/// migrations will collapse the two.
#[derive(Resource, Default)]
pub struct ModelicaDocumentRegistry {
    hosts: HashMap<Entity, DocumentHost<ModelicaDocument>>,
    next_doc_id: u64,
}

impl ModelicaDocumentRegistry {
    /// Record a compile: create-or-update the document for `entity` to hold
    /// `source`. Returns `true` if the document changed (was new, or had a
    /// different prior source). A no-op compile (same source as last time)
    /// returns `false` and does not bump generation.
    pub fn checkpoint_source(&mut self, entity: Entity, source: String) -> bool {
        match self.hosts.get_mut(&entity) {
            Some(host) => {
                if host.document().source() == source {
                    return false;
                }
                // Best-effort: ReplaceSource cannot fail today, but the trait
                // signature is fallible so we swallow the Result rather than
                // propagate it. Callers don't care about the details.
                let _ = host.apply(ModelicaOp::ReplaceSource { new: source });
                true
            }
            None => {
                self.next_doc_id = self.next_doc_id.saturating_add(1);
                let doc = ModelicaDocument::new(DocumentId::new(self.next_doc_id), source);
                self.hosts.insert(entity, DocumentHost::new(doc));
                true
            }
        }
    }

    /// Immutable access to an entity's document host, if registered.
    pub fn host(&self, entity: Entity) -> Option<&DocumentHost<ModelicaDocument>> {
        self.hosts.get(&entity)
    }

    /// Mutable access to an entity's document host, if registered.
    pub fn host_mut(&mut self, entity: Entity) -> Option<&mut DocumentHost<ModelicaDocument>> {
        self.hosts.get_mut(&entity)
    }

    /// Drop the host for an entity (called when the entity is despawned).
    pub fn remove(&mut self, entity: Entity) {
        self.hosts.remove(&entity);
    }

    /// Number of registered entity documents.
    pub fn len(&self) -> usize {
        self.hosts.len()
    }

    /// Whether the registry currently tracks any documents.
    pub fn is_empty(&self) -> bool {
        self.hosts.is_empty()
    }
}

impl Default for WorkbenchState {
    fn default() -> Self {
        Self {
            current_path: assets_dir().join("models"),
            editor_buffer: String::new(),
            loaded_file_path: None,
            selected_entity: None,
            compilation_error: None,
            history: HashMap::new(),
            plotted_variables: HashSet::new(),
            max_history: 10000,
            plot_auto_fit: false,
            open_model: None,
            navigation_stack: Vec::new(),
            diagram_dirty: false,
            is_loading: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lunco_doc::Document;

    fn fake_entity(bits: u64) -> Entity {
        Entity::from_bits(bits)
    }

    #[test]
    fn checkpoint_creates_host_on_first_call() {
        let mut reg = ModelicaDocumentRegistry::default();
        let e = fake_entity(0x0000_0001_0000_0001);

        assert!(reg.host(e).is_none());
        assert!(reg.is_empty());

        let changed = reg.checkpoint_source(e, "model A end A;".into());
        assert!(changed);
        assert_eq!(reg.len(), 1);

        let host = reg.host(e).expect("host registered");
        assert_eq!(host.document().source(), "model A end A;");
        assert_eq!(host.generation(), 0, "first registration doesn't apply an op");
    }

    #[test]
    fn checkpoint_applies_op_on_change() {
        let mut reg = ModelicaDocumentRegistry::default();
        let e = fake_entity(0x0000_0002_0000_0002);

        reg.checkpoint_source(e, "model A end A;".into());
        let changed = reg.checkpoint_source(e, "model B end B;".into());
        assert!(changed);

        let host = reg.host(e).unwrap();
        assert_eq!(host.document().source(), "model B end B;");
        assert_eq!(host.generation(), 1);
        assert!(host.can_undo());
    }

    #[test]
    fn checkpoint_no_op_when_source_unchanged() {
        let mut reg = ModelicaDocumentRegistry::default();
        let e = fake_entity(0x0000_0003_0000_0003);

        reg.checkpoint_source(e, "same".into());
        let changed = reg.checkpoint_source(e, "same".into());
        assert!(!changed, "re-checkpointing identical source must not bump generation");
        assert_eq!(reg.host(e).unwrap().generation(), 0);
    }

    #[test]
    fn undo_restores_previous_source() {
        let mut reg = ModelicaDocumentRegistry::default();
        let e = fake_entity(0x0000_0004_0000_0004);

        reg.checkpoint_source(e, "v1".into());
        reg.checkpoint_source(e, "v2".into());
        reg.checkpoint_source(e, "v3".into());

        let host = reg.host_mut(e).unwrap();
        host.undo().unwrap();
        assert_eq!(host.document().source(), "v2");
        host.undo().unwrap();
        assert_eq!(host.document().source(), "v1");
    }

    #[test]
    fn remove_drops_host() {
        let mut reg = ModelicaDocumentRegistry::default();
        let e = fake_entity(0x0000_0005_0000_0005);

        reg.checkpoint_source(e, "x".into());
        assert_eq!(reg.len(), 1);

        reg.remove(e);
        assert!(reg.is_empty());
        assert!(reg.host(e).is_none());
    }

    #[test]
    fn multiple_entities_tracked_independently() {
        let mut reg = ModelicaDocumentRegistry::default();
        let a = fake_entity(0x0000_0006_0000_0006);
        let b = fake_entity(0x0000_0007_0000_0007);

        reg.checkpoint_source(a, "source_a".into());
        reg.checkpoint_source(b, "source_b".into());
        reg.checkpoint_source(a, "source_a_v2".into());

        assert_eq!(reg.host(a).unwrap().document().source(), "source_a_v2");
        assert_eq!(reg.host(b).unwrap().document().source(), "source_b");
        assert_ne!(
            reg.host(a).unwrap().document().id(),
            reg.host(b).unwrap().document().id(),
            "each entity gets a distinct DocumentId"
        );
    }
}
