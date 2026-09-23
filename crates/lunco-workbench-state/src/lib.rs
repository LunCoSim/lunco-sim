//! Per-Twin (per-project) workbench state — VSCode's `workspaceStorage`.
//!
//! VSCode keeps *volatile UI state* (which editors were open, the active
//! one, window layout) in a global store keyed by a hash of the
//! workspace path — **not** inside the project folder, so repos stay
//! clean. We do the same: each Twin gets a
//! the shared LunCoSim config directory's `workspace-state/<hash>.json`, keyed
//! off its root path.
//!
//! ## What's stored (and what isn't)
//!
//! - **Active perspective** — restored on Twin activation (workbench
//!   local, side-effect free). A host may provide a one-shot initial
//!   perspective for an explicit launch through [`WorkspaceStateRestorePolicy`].
//! - **Open documents + active document** — hot-exit state is saved and
//!   restored for an explicitly active Twin. With no active Twin, the host
//!   uses its startup defaults and does not load or save workspace state.
//!
//! Global, app-wide preferences (theme, perf HUD, **default window
//! geometry**) stay in the shared LunCoSim settings file via `lunco-settings` —
//! see `lunco-workbench-window::WindowPersistencePlugin`. This module owns
//! only the per-project slice.
//!
//! ## Persistence pattern
//!
//! Mirrors recents (`session.rs`): load on Twin activation and save on
//! change via a serialized-snapshot compare (so unrelated
//! `WorkspaceResource` mutations don't write). The ECS thread schedules
//! storage reads, domain preparation, serialization, and atomic writes on
//! Bevy's task pool. A corrupt or missing file degrades to "open with
//! defaults" — never a panic.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;

use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task, block_on, futures_lite::future};
use lunco_doc::{DocumentId, DocumentOrigin};
use lunco_workbench_core::PanelId;
use serde::{Deserialize, Serialize};

use lunco_workspace::WorkspaceResource;

/// Provides the concrete layout operations needed by workspace-state
/// persistence without making this package depend on a particular dock shell.
///
/// The state package owns the persisted representation and lifecycle. A host
/// supplies this adapter for its own layout implementation; the default
/// Workbench registers its adapter when it installs [`WorkspaceStatePlugin`].
pub trait WorkspaceStateLayoutProvider: Send + Sync + 'static {
    /// Return the active perspective's stable string id, if one is active.
    fn active_perspective(&self, world: &World) -> Option<String>;
    /// Return the focused instance-tab id, if the layout has one focused.
    fn active_tab_instance(&self, world: &World) -> Option<u64>;
    /// Return a cheap structural layout revision for change detection.
    fn dock_layout_hash(&self, world: &World) -> u64;
    /// Capture every restorable perspective dock and its slot intent.
    fn capture_perspective_docks(&self, world: &World) -> HashMap<String, PerspectiveDockSnapshot>;
    /// Activate a persisted perspective id when it is registered by the host.
    fn activate_perspective_by_str(&self, world: &mut World, id: &str) -> bool;
    /// Restore persisted dock trees after domain documents have been opened.
    fn seed_perspective_docks(
        &self,
        world: &mut World,
        docks: &HashMap<String, PerspectiveDockSnapshot>,
        id_map: &HashMap<(&'static str, u64), u64>,
        discard_unmapped_kinds: &HashSet<&'static str>,
    );
}

#[derive(Resource)]
struct LayoutProvider(std::sync::Arc<dyn WorkspaceStateLayoutProvider>);

fn with_layout<R>(
    world: &mut World,
    f: impl FnOnce(&dyn WorkspaceStateLayoutProvider, &World) -> R,
) -> R {
    world.resource_scope(|world, provider: Mut<LayoutProvider>| f(provider.0.as_ref(), world))
}

fn with_layout_mut<R>(
    world: &mut World,
    f: impl FnOnce(&dyn WorkspaceStateLayoutProvider, &mut World) -> R,
) -> R {
    world.resource_scope(|world, provider: Mut<LayoutProvider>| f(provider.0.as_ref(), world))
}

/// Host-supplied presentation intent for the first per-Twin workspace restore.
///
/// An explicit launch request can have a stronger presentation contract than
/// the last editor session. The host consumes this intent once, after a real
/// Twin becomes active; later Twin switches restore each Twin's persisted
/// perspective normally.
#[derive(Resource, Clone, Debug, Default, PartialEq, Eq)]
pub struct WorkspaceStateRestorePolicy {
    initial_perspective: Option<String>,
}

impl WorkspaceStateRestorePolicy {
    /// Select a perspective for the first Twin restored in this process.
    pub fn with_initial_perspective(id: impl Into<String>) -> Self {
        Self {
            initial_perspective: Some(id.into()),
        }
    }

    fn take_initial_perspective(&mut self) -> Option<String> {
        self.initial_perspective.take()
    }
}

/// Hot-exit snapshot of one open document — VSCode-style. Carries the
/// **live editor buffer** (`source`), not just a path, so unsaved edits
/// survive a restart and are restored as in-memory content rather than
/// re-read from disk.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct DocumentSnapshot {
    /// Codec id this doc belongs to (e.g. `"modelica"`). Matched against
    /// the registered [`DocumentSessionCodec`]s on restore; unknown
    /// kinds are dropped (an app that doesn't host that domain).
    pub kind: String,
    /// Where the doc came from (Untitled / File / Bundled). Already
    /// serde in `lunco-doc`.
    pub origin: DocumentOrigin,
    /// Tab title at save time.
    pub title: String,
    /// The editor buffer text — the UI state being preserved.
    pub source: String,
    /// Whether the doc had unsaved changes (best-effort on restore).
    pub dirty: bool,
    /// This session's live `DocumentId.raw()` at save time. **Not** stable
    /// across runs — used only to remap the persisted dock tree's tab
    /// instance ids (`TabId::Instance { instance, .. }`) onto the
    /// freshly-restored documents. Defaults to 0 for older state files.
    #[serde(default)]
    pub id: u64,
    /// This session's **dock tab instance id** for this doc's primary tab
    /// — the value carried in the persisted dock tree as
    /// `TabId::Instance { instance, .. }`. For Modelica this is the
    /// `ModelTabs` tab id, which is a SEPARATE counter from
    /// [`id`](Self::id) (`DocumentId.raw()`) — they only coincide when docs
    /// and tabs open in lockstep, so the dock remap (5a) must key on this,
    /// not on `id`. The codec fills it in `capture` and reports the live
    /// replacement via [`DocumentSessionCodec::instance_remaps`] on restore.
    /// 0 when the domain has no dock tab or for older state files.
    #[serde(default)]
    pub tab_instance: u64,
    /// Opaque per-domain view state — canvas zoom/pan, etc. The Modelica
    /// codec serializes its per-tab `Viewport` here; domain codecs own any
    /// other view state. Generic `Value` keeps `lunco-workbench`
    /// domain-agnostic.
    #[serde(default)]
    pub view_state: serde_json::Value,
}

/// Per-domain hook letting `lunco-workbench` capture and restore open
/// documents **without depending on the domain crate** (domains depend
/// on the workbench, not the reverse). Each domain registers one impl
/// via [`AppDocumentSessionExt::register_document_session_codec`];
/// mirrors the `BrowserSectionRegistry` pattern (11-workbench §5a).
pub trait DocumentSessionCodec: Send + Sync + 'static {
    /// Stable codec id, stored in [`DocumentSnapshot::kind`].
    fn kind(&self) -> &'static str;
    /// Cheap monotonic-ish signal that changes when this domain's open
    /// set, buffers, or persisted views change. Lets
    /// capture skip the (allocating) snapshot build in the steady state
    /// — no per-frame buffer clones (AGENTS.md §7.1).
    fn revision(&self, world: &World) -> u64;
    /// Snapshot every open document of this kind, each paired with its
    /// **live** `DocumentId` (`raw()`) for *this* session. The id lets
    /// the workbench match the active tab reliably (origins can differ
    /// between the registry and the Workspace entry); it is not
    /// persisted — ids aren't stable across runs.
    fn capture(&self, world: &mut World) -> Vec<(u64, DocumentSnapshot)>;
    /// Prepare one persisted snapshot for restoration without blocking the
    /// ECS thread. Domain adapters use this to refresh clean file-backed
    /// buffers from their current source before the synchronous document
    /// registry admits them. The default keeps the stored snapshot as-is.
    fn prepare_restore(
        &self,
        _world: &World,
        snapshot: DocumentSnapshot,
    ) -> Pin<Box<dyn Future<Output = PreparedDocumentSnapshot> + Send>> {
        Box::pin(async move { PreparedDocumentSnapshot::new(snapshot) })
    }
    /// Recreate one document from a snapshot, replaying the domain's
    /// normal open path (which opens the tab + registers the entry).
    /// Returns the freshly-allocated `DocumentId.raw()` so the workbench
    /// can remap the persisted dock tree's tab instance ids onto it;
    /// `None` if restore was a no-op (e.g. the registry was missing).
    fn restore(&self, world: &mut World, snap: &DocumentSnapshot) -> Option<u64>;
    /// Reconcile a saved snapshot with an already-open document that has the
    /// same origin. Domains can refresh a clean buffer from the prepared
    /// snapshot while retaining a live dirty buffer. The default preserves the
    /// already-open document unchanged.
    fn restore_existing(
        &self,
        _world: &mut World,
        _snap: &DocumentSnapshot,
        live_id: u64,
    ) -> Option<u64> {
        Some(live_id)
    }
    /// Apply the snapshot's opaque [`view_state`](DocumentSnapshot::view_state)
    /// (canvas zoom/pan, …) to the **live** document identified by
    /// `live_id` (`DocumentId.raw()`). Called for *every* restored doc —
    /// both freshly [`restore`](Self::restore)d ones **and** docs the app
    /// auto-opened that matched a snapshot (so a reopened diagram restores
    /// its camera even when the open itself was deduped). Default no-op for
    /// domains without additional per-doc view state. Runs after `restore`.
    fn apply_view_state(&self, _world: &mut World, _live_id: u64, _snap: &DocumentSnapshot) {}
    /// Whether a dock tab instance belongs to this saved document. The
    /// default covers domains with one instance stored in `tab_instance`;
    /// codecs with multiple views can recognize their extra instance ids.
    fn owns_tab_instance(&self, snapshot: &DocumentSnapshot, instance: u64) -> bool {
        snapshot.tab_instance == instance
    }
    /// Report each saved dock-tab instance remapped to its live instance.
    /// This allows one document to own several views while keeping one
    /// canonical document snapshot. `live_id` is the `DocumentId.raw()` after
    /// [`restore`] (or document deduplication). Domains without instance tabs
    /// return an empty vector.
    fn instance_remaps(
        &self,
        _world: &mut World,
        _snap: &DocumentSnapshot,
        _live_id: u64,
    ) -> Vec<(u64, u64)> {
        Vec::new()
    }
    /// The workbench `PanelId` string of the dock tab kind this codec's
    /// documents use (e.g. `"modelica_model_view"`). The dock remap (5a)
    /// keys on `(kind, instance)` so tab ids are only rewritten within the
    /// codec's own kind — different kinds share the `u64` instance space
    /// (e.g. a model-view tab and a plot tab can both be instance 1), so a
    /// flat instance→instance map would cross-rewrite them. `None` (default)
    /// means [`instance_remaps`](Self::instance_remaps) is unused.
    fn dock_tab_kind(&self) -> Option<&'static str> {
        None
    }
    /// Return a dynamic dock-tab kind whose unmatched instances must be
    /// discarded during restore. Stable-instance tabs should leave this unset.
    fn discard_unmapped_dock_tab_kind(&self) -> Option<&'static str> {
        None
    }
}

/// Result of a domain's background preparation of one session document.
pub struct PreparedDocumentSnapshot {
    /// Snapshot admitted by the domain's normal restore path.
    pub snapshot: DocumentSnapshot,
    /// Read or recovery issue that should be shown in the application log.
    pub warning: Option<String>,
}

impl PreparedDocumentSnapshot {
    /// Create a prepared snapshot without a warning.
    pub fn new(snapshot: DocumentSnapshot) -> Self {
        Self {
            snapshot,
            warning: None,
        }
    }

    /// Create a prepared snapshot with a visible recovery warning.
    pub fn with_warning(snapshot: DocumentSnapshot, warning: impl Into<String>) -> Self {
        Self {
            snapshot,
            warning: Some(warning.into()),
        }
    }
}

/// One document's contribution to a [`DocumentSessionCodec::revision`] fold:
/// mixes a document's id and generation into a single word. XOR-combine the term
/// of every open document into a running accumulator (order-independent), then
/// pass the accumulator and document count to [`finalize_revision`].
///
/// CQ-112: this exact bit-mixing was duplicated byte-for-byte in the modelica
/// and usd session codecs; extracting it keeps both revision signals in lockstep.
pub fn revision_term(id_raw: u64, generation: u64) -> u64 {
    id_raw
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .rotate_left((generation & 63) as u32)
        ^ generation.wrapping_mul(0x1000_0000_01b3)
}

/// Combine the XOR-folded [`revision_term`] accumulator with the open-document
/// `count` into the final revision word. Mixing the count in makes the revision
/// change on open/close even when the surviving terms happen to XOR-cancel.
///
/// CQ-112: shared by the modelica and usd session codecs.
pub fn finalize_revision(acc: u64, count: u64) -> u64 {
    acc.wrapping_add(count.wrapping_mul(0x100_0000_01b3))
}

/// Registry of per-domain [`DocumentSessionCodec`]s. Populated at plugin
/// `build` time; iterated by the capture / restore systems.
#[derive(Resource, Default)]
pub struct DocumentSessionRegistry {
    codecs: Vec<Box<dyn DocumentSessionCodec>>,
}

impl DocumentSessionRegistry {
    /// Register a codec. Last-registered-wins is irrelevant — kinds are
    /// expected unique.
    pub fn register(&mut self, codec: impl DocumentSessionCodec) {
        self.codecs.push(Box::new(codec));
    }
}

/// App extension to register a [`DocumentSessionCodec`] from a domain
/// plugin's `build`.
pub trait AppDocumentSessionExt {
    /// Register a per-domain document session codec for hot-exit
    /// capture / restore.
    fn register_document_session_codec(&mut self, codec: impl DocumentSessionCodec) -> &mut Self;
}

impl AppDocumentSessionExt for App {
    fn register_document_session_codec(&mut self, codec: impl DocumentSessionCodec) -> &mut Self {
        self.world_mut()
            .get_resource_or_init::<DocumentSessionRegistry>()
            .register(codec);
        self
    }
}

/// Serialized snapshot of one perspective's dock tree + slot intent — the
/// unit [`WorkspaceState::docks`] stores per perspective so a return visit
/// after a restart restores the exact tabs + splits + active centre tab the
/// user left in THAT mode, not its preset. The host layout provider supplies
/// the live dock implementation; this DTO keeps the dock as opaque JSON so
/// restore can route it through the host's reconciliation path and carries
/// the slot intent so a later rebuild/reset reproduces the saved layout.
#[derive(Serialize, Deserialize, Default, Clone, PartialEq, Debug)]
pub struct PerspectiveDockSnapshot {
    /// Revision of the perspective preset that produced this snapshot.
    /// Snapshots from an older preset are ignored so a shipped layout change
    /// can replace stale persisted slot assignments once.
    #[serde(default)]
    pub layout_revision: u32,
    /// Serialized `egui_dock::DockState<TabId>` — split sizes, tab
    /// arrangement, active leaf. Parsed + reconciled on restore.
    #[serde(default)]
    pub dock: serde_json::Value,
    // The seven fields below are the SLOT INTENT at save time — the
    // perspective's declared panel ids per slot, plus which centre tab was
    // active. Restored into the cache slot verbatim, so a later rebuild/reset
    // reproduces the saved layout even though `dock` above already encodes the
    // live tree.
    /// Panel ids declared for the left/side browser slot.
    #[serde(default)]
    pub side_browser: Vec<PanelId>,
    /// Panel ids declared for the lower leaf of the left/side browser split.
    #[serde(default)]
    pub side_browser_bottom: Vec<PanelId>,
    /// Panel ids declared for the centre slot (one tab each).
    #[serde(default)]
    pub center: Vec<PanelId>,
    /// Index into [`center`](Self::center) of the tab that was active.
    #[serde(default)]
    pub active_center_tab: usize,
    /// Panel ids declared for the right inspector slot.
    #[serde(default)]
    pub right_inspector: Vec<PanelId>,
    /// Panel ids declared for the lower leaf of the right inspector split.
    #[serde(default)]
    pub right_inspector_bottom: Vec<PanelId>,
    /// Panel ids declared for the bottom slot.
    #[serde(default)]
    pub bottom: Vec<PanelId>,
}

/// Persisted logical position of one runtime-authored surface.
///
/// The runtime UI owner supplies the surface's authored size and clamps this
/// position to the current target. Workbench stores only the user override so
/// an authored geometry change remains the default for surfaces that were not
/// moved.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
pub struct RuntimeSurfaceLayout {
    /// Logical x coordinate of the surface's top-left corner.
    pub left: f32,
    /// Logical y coordinate of the surface's top-left corner.
    pub top: f32,
}

impl RuntimeSurfaceLayout {
    /// Return whether this persisted position is safe to apply.
    pub fn is_finite(self) -> bool {
        self.left.is_finite() && self.top.is_finite()
    }
}

/// Per-Twin persisted positions for runtime-authored draggable surfaces.
///
/// This is the single layout store for HUI/Flair runtime surfaces. The
/// surface manifest remains the default/visibility authority; this resource
/// contains only validated user overrides and participates in the existing
/// workspace-state snapshot gate.
#[derive(Resource, Default, Clone, PartialEq, Debug)]
pub struct RuntimeSurfaceLayouts {
    layouts: HashMap<String, RuntimeSurfaceLayout>,
    revision: u64,
}

impl RuntimeSurfaceLayouts {
    /// Look up a user override by stable authored surface identity.
    pub fn get(&self, id: &str) -> Option<RuntimeSurfaceLayout> {
        self.layouts.get(id).copied()
    }

    /// Return all currently retained overrides for workspace persistence.
    pub fn as_map(&self) -> &HashMap<String, RuntimeSurfaceLayout> {
        &self.layouts
    }

    /// Monotonic change signal used by workspace persistence.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Replace the active Twin's layout snapshot during workspace restore.
    pub(crate) fn replace(&mut self, layouts: HashMap<String, RuntimeSurfaceLayout>) {
        let layouts = layouts
            .into_iter()
            .filter(|(_, layout)| layout.is_finite())
            .collect();
        if self.layouts != layouts {
            self.layouts = layouts;
            self.bump_revision();
        }
    }

    /// Store a finite user position. Returns whether the snapshot changed.
    pub fn set(&mut self, id: impl Into<String>, layout: RuntimeSurfaceLayout) -> bool {
        if !layout.is_finite() {
            return false;
        }
        let id = id.into();
        if self.layouts.get(&id).copied() == Some(layout) {
            return false;
        }
        self.layouts.insert(id, layout);
        self.bump_revision();
        true
    }

    /// Remove a user override so the authored default is used again.
    pub fn reset(&mut self, id: &str) -> bool {
        if self.layouts.remove(id).is_some() {
            self.bump_revision();
            true
        } else {
            false
        }
    }

    /// Drop entries for surfaces that are no longer authored, and invalid
    /// entries that cannot be applied safely.
    pub fn retain_ids(&mut self, ids: &std::collections::HashSet<String>) {
        let before = self.layouts.len();
        self.layouts
            .retain(|id, layout| ids.contains(id) && layout.is_finite());
        if self.layouts.len() != before {
            self.bump_revision();
        }
    }

    /// Clear all user overrides at the Twin lifecycle boundary.
    pub(crate) fn clear(&mut self) {
        if !self.layouts.is_empty() {
            self.layouts.clear();
            self.bump_revision();
        }
    }

    fn bump_revision(&mut self) {
        self.revision = self
            .revision
            .checked_add(1)
            .expect("runtime surface layout revision exhausted");
    }
}

/// Per-Twin volatile UI state. One of these per project, stored at
/// [`workspace_state_path`].
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceState {
    /// Version of the persisted representation.
    pub schema_version: u32,
    /// The non-empty Twin root this state belongs to. Stored so a hash
    /// collision (two paths landing on the same file stem) is detectable —
    /// a mismatch is treated as a miss.
    pub twin_root: PathBuf,
    /// `PerspectiveId` string of the perspective active at save time.
    /// `None` ⇒ leave the app's startup default.
    pub perspective: Option<String>,
    /// Hot-exit snapshots of every open document, in open order. **Global**
    /// across perspectives: the open-doc set is the shared compile/cosim
    /// state (one `ModelTabs`), so it's captured once, not per mode. Only
    /// the *visible tab arrangement* differs per perspective — see
    /// [`docks`](Self::docks).
    pub documents: Vec<DocumentSnapshot>,
    /// Index into [`documents`](Self::documents) of the active tab in the
    /// [`perspective`](Self::perspective) active at save time.
    pub active_document: Option<usize>,
    /// Per-perspective dock trees + slot intent, keyed by `PerspectiveId`
    /// string. The active perspective's tree is `docks[perspective]`. Each
    /// perspective keeps its own tabs/splits across restarts while the
    /// underlying documents stay shared. Empty for apps that don't persist
    /// a dock. The inverse of the registered layout provider's capture method.
    #[serde(default)]
    pub docks: HashMap<String, PerspectiveDockSnapshot>,
    /// User positions of authored draggable runtime UI surfaces, keyed by
    /// stable surface identity. Missing entries use manifest defaults.
    #[serde(default)]
    pub runtime_surface_layouts: HashMap<String, RuntimeSurfaceLayout>,
}

/// Current serialized workspace-state format.
pub const WORKSPACE_STATE_SCHEMA_VERSION: u32 = 2;

impl Default for WorkspaceState {
    fn default() -> Self {
        Self {
            schema_version: WORKSPACE_STATE_SCHEMA_VERSION,
            twin_root: PathBuf::new(),
            perspective: None,
            documents: Vec::new(),
            active_document: None,
            docks: HashMap::new(),
            runtime_surface_layouts: HashMap::new(),
        }
    }
}

/// Decode the current persisted representation. Older workspace layouts are
/// rejected and preserved by [`WorkspaceState::load`] as `.json.bad`; no old
/// dock shape is translated into the current one at runtime.
fn decode_workspace_state(text: &str) -> Result<WorkspaceState, String> {
    let state: WorkspaceState = serde_json::from_str(text).map_err(|e| e.to_string())?;
    if state.schema_version != WORKSPACE_STATE_SCHEMA_VERSION {
        return Err(format!(
            "unsupported workspace state schema version {} (current {})",
            state.schema_version, WORKSPACE_STATE_SCHEMA_VERSION
        ));
    }
    Ok(state)
}

impl WorkspaceState {
    /// Load the state for a non-empty Twin root. Returns `None` for an empty
    /// root, a missing / unreadable / corrupt file, or when the stored
    /// `twin_root` doesn't match (hash collision guard) — all mean "use defaults".
    pub fn load(twin_root: &Path) -> Option<Self> {
        Self::load_with_json(twin_root).map(|(state, _)| state)
    }

    /// Load a state and retain the serialized value so the async save gate
    /// can avoid rewriting an unchanged file after startup.
    fn load_with_json(twin_root: &Path) -> Option<(Self, String)> {
        if twin_root.as_os_str().is_empty() {
            return None;
        }
        let path = workspace_state_path(twin_root);
        use lunco_storage::Storage;
        let bytes = lunco_storage::FileStorage::new()
            .read_sync(&lunco_storage::StorageHandle::File(path.clone()))
            .ok()?;
        let text = String::from_utf8(bytes).ok()?;
        let state = match decode_workspace_state(&text) {
            Ok(state) => state,
            Err(e) => {
                // Falling back to defaults means the next save overwrites this
                // file, taking the user's whole dock arrangement with it.
                // Preserve it as `workspace.json.bad` (the `lunco-settings`
                // pattern) so a hand-fixable typo stays recoverable.
                let bad = path.with_extension("json.bad");
                warn!(
                    "[WorkspaceState] {} is not valid JSON ({e}); preserving as {} and starting fresh",
                    path.display(),
                    bad.display(),
                );
                if let Err(e) = lunco_storage::write_file_sync(&bad, text.as_bytes()) {
                    warn!(
                        "[WorkspaceState] could not preserve corrupt state to {}: {e}",
                        bad.display(),
                    );
                }
                return None;
            }
        };
        if state.twin_root != twin_root {
            warn!(
                "[WorkspaceState] {} stores a different twin_root ({}); ignoring (hash collision?)",
                path.display(),
                state.twin_root.display(),
            );
            return None;
        }
        Some((state, text))
    }

    /// Atomically write this state for its `twin_root` (tmp + rename so a
    /// kill mid-write can't corrupt the file).
    pub fn save(&self) -> std::io::Result<()> {
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        self.save_serialized(&json)
    }

    /// Write an already-serialized state to disk. The per-frame persist
    /// path serializes the state once for its change-compare; this lets it
    /// reuse that exact string instead of `save()` re-serializing the same
    /// value a second time per write (CQ-209).
    pub fn save_serialized(&self, json: &str) -> std::io::Result<()> {
        if self.twin_root.as_os_str().is_empty() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "workspace state requires an active Twin root",
            ));
        }
        let path = workspace_state_path(&self.twin_root);
        // CQ-107: persist through the Storage API (atomic tmp+rename,
        // creates parent dirs) instead of hand-rolling `std::fs`.
        lunco_storage::write_file_sync(&path, json.as_bytes())
            .map_err(|e| std::io::Error::other(e.to_string()))
    }
}

/// FNV-1a 64-bit hash. Used to key the per-Twin state file by path.
/// Picked over `DefaultHasher` because the latter's output is *not*
/// guaranteed stable across std versions — a state file written today
/// must still be found after a toolchain bump.
fn fnv1a64(bytes: &[u8]) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut h = OFFSET;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(PRIME);
    }
    h
}

fn workspace_session_key(root: &Path) -> String {
    format!("{:016x}", fnv1a64(root.to_string_lossy().as_bytes()))
}

/// Resolve the on-disk path for a Twin's state file (the root must be
/// non-empty):
/// `<config>/workspace-state/<fnv1a-hex>.json`. Honours the
/// `LUNCOSIM_CONFIG` override via `lunco_settings::user_config_dir`.
///
/// The root is canonicalized first when possible so cwd-relative and
/// absolute spellings of the same folder collapse to one key; falls back
/// to the raw path bytes when canonicalization fails (e.g. the folder
/// was deleted).
pub fn workspace_state_path(twin_root: &Path) -> PathBuf {
    let canonical = lunco_storage::canonicalize_file_path(twin_root)
        .unwrap_or_else(|_| twin_root.to_path_buf());
    let key = fnv1a64(canonical.to_string_lossy().as_bytes());
    lunco_settings::user_config_dir()
        .join("workspace-state")
        .join(format!("{key:016x}.json"))
}

// ─────────────────────────────────────────────────────────────────────────────
// Bevy wiring
// ─────────────────────────────────────────────────────────────────────────────

/// Last-saved snapshot, keyed by the active Twin, so the save system
/// only writes the file when this Twin's state actually changed.
#[derive(Resource, Default)]
struct WorkspaceStateLast {
    /// Process-local key of the Twin root used to compare snapshots.
    key: Option<String>,
    /// Exact root path of the Twin the snapshot belongs to.
    root: Option<PathBuf>,
    /// Serialized value of the last-saved [`WorkspaceState`].
    json: Option<String>,
    /// Cheap fold gating the (allocating) snapshot build — see
    /// [`gate_value`].
    rev: u64,
    /// Set once `rev` has been computed at least once.
    seeded: bool,
}

#[derive(Resource, Default)]
struct PendingWorkspaceRestore(Option<WorkspaceRestoreTask>);

struct WorkspaceRestoreTask {
    twin: lunco_workspace::TwinId,
    root: PathBuf,
    initial_perspective: Option<String>,
    loaded_json: Option<String>,
    state: Option<WorkspaceState>,
    task: WorkspaceRestoreTaskKind,
}

enum WorkspaceRestoreTaskKind {
    Loading(Task<Option<(WorkspaceState, String)>>),
    Preparing(Task<Vec<PreparedDocumentSnapshot>>),
}

#[derive(Resource, Default)]
struct PendingWorkspaceSave(Option<WorkspaceSaveTask>);

struct WorkspaceSaveTask {
    key: String,
    root: PathBuf,
    rev: u64,
    task: Task<Result<String, String>>,
}

/// Tracks restore progress so it fires once the app's own startup docs
/// have *settled* (apps like lunica auto-open a default doc, async), and
/// then once per later Twin switch.
#[derive(Resource, Default)]
struct AppliedTwin {
    /// Set after the initial (startup) restore has run.
    initialized: bool,
    /// Twin we last restored for (re-runs on change).
    twin: Option<lunco_workspace::TwinId>,
    /// Last-seen session revision while waiting for startup to settle.
    settle_rev: u64,
    /// Consecutive frames the revision has held steady.
    settle_frames: u32,
    /// Frames waited overall — a hard cap so restore still fires even if
    /// the doc set never stops churning.
    settle_budget: u32,
    /// Set when any saved document could not be reconciled. This keeps a
    /// partial restore from overwriting unresolved snapshot data on save.
    restore_failed: bool,
}

/// **Phase 5a (dock-arrangement restore).** Serialize the full
/// `egui_dock::DockState` (split sizes + tab arrangement) and re-install it
/// on restore. The earlier blank-center regression was caused by remapping
/// `TabId::Instance` ids in the wrong key space: the dock instance is the
/// domain's tab id (Modelica `ModelTabs` counter), NOT `DocumentId.raw()`,
/// so the stale instance pointed at no live tab and rendered empty. Fixed
/// by [`DocumentSessionCodec::instance_remaps`] + [`DocumentSnapshot::tab_instance`]
/// (old tab id → live tab id); `set_dock_from_json` remaps mapped instances,
/// keeps unmatched stable-id tabs like the default plot, and drops unmatched
/// document-backed tabs when their codec requests it.
/// The codec's own `OpenTab` (fired before the dock is re-installed) opens +
/// focuses the live tab, so the re-installed dock's matching instance
/// renders. Restore still falls back gracefully (keeps the codec-opened
/// layout) if the saved dock won't parse or nothing survives reconciliation.
const RESTORE_DOCK_ARRANGEMENT: bool = true;

/// Frames the open-doc set must hold steady before the startup restore
/// runs — long enough for async auto-open to land, short enough to feel
/// instant (~3 frames ≈ 50 ms at 60 Hz).
const SETTLE_FRAMES: u32 = 3;
/// Hard cap on settle waiting (~1 s at 60 Hz) so restore can't be
/// starved by a perpetually-churning doc set.
const SETTLE_BUDGET: u32 = 60;

/// Absolute root of the active Twin, when one is active.
fn active_twin_root(world: &World) -> Option<PathBuf> {
    let ws = world.resource::<WorkspaceResource>();
    ws.active_twin
        .and_then(|id| ws.twin(id))
        .map(|t| t.root.clone())
        .filter(|root| !root.as_os_str().is_empty())
}

/// Concat every registered codec's open-doc snapshots, each paired with
/// its live `DocumentId` (`raw()`) for active-tab matching.
fn capture_documents(world: &mut World) -> Vec<(u64, DocumentSnapshot)> {
    let mut out = Vec::new();
    if world.get_resource::<DocumentSessionRegistry>().is_none() {
        return out;
    }
    world.resource_scope(|world, reg: Mut<DocumentSessionRegistry>| {
        for codec in &reg.codecs {
            out.extend(codec.capture(world));
        }
    });
    // A Workspace intentionally keeps closed-Twin documents as loose session
    // state. Per-Twin hot-exit must nevertheless persist only the active
    // scope; otherwise the next Twin restores documents from an unrelated
    // project and recreates the ownership leak this state is meant to avoid.
    if let Some(workspace) = world.get_resource::<WorkspaceResource>() {
        out.retain(|(raw_id, _)| {
            workspace
                .document(DocumentId::new(*raw_id))
                .map(|entry| workspace.document_is_in_active_scope(entry))
                .unwrap_or(true)
        });
    }
    out
}

/// Fold of every codec's `revision` — changes when any open buffer or
/// the open set changes. Cheap (no buffer clones).
fn session_revision(world: &mut World) -> u64 {
    let mut r = 0u64;
    if world.get_resource::<DocumentSessionRegistry>().is_none() {
        return 0;
    }
    world.resource_scope(|world, reg: Mut<DocumentSessionRegistry>| {
        for codec in &reg.codecs {
            r = r.wrapping_add(codec.revision(world));
        }
    });
    r
}

/// Cheap value that changes when anything we persist changes (docs,
/// perspective, active Twin) — gates the expensive capture/serialize.
fn gate_value(world: &mut World) -> Option<u64> {
    let twin_root = active_twin_root(world)?;
    let docs = session_revision(world);
    let (persp, active, dock) = with_layout(world, |layout, world| {
        let persp = layout
            .active_perspective(world)
            .map(|p| fnv1a64(p.as_bytes()))
            .unwrap_or(0);
        // Fold in the focused tab so switching tabs re-fires the gate and
        // re-saves the active index (the dock focus is the real signal;
        // `active_document` is the fallback the build also uses).
        let active = layout
            .active_tab_instance(world)
            .or_else(|| {
                world
                    .resource::<WorkspaceResource>()
                    .active_document
                    .map(|id| id.raw())
            })
            .map(|raw| raw.wrapping_mul(0x9E37_79B9_7F4A_7C15))
            .unwrap_or(0);
        // Fold the dock arrangement (split sizes + tab layout + active leaf)
        // so a drag re-fires the save. Only when 5a is on — otherwise the
        // dock isn't persisted and folding it would re-save needlessly.
        // Uses a direct structural hash, NOT JSON: this gate runs every
        // frame, and serializing the dock to JSON just to hash it churned a
        // `Value` tree + `String` per frame for no good reason (CQ-209).
        let dock = if RESTORE_DOCK_ARRANGEMENT {
            layout.dock_layout_hash(world)
        } else {
            0
        };
        (persp, active, dock)
    });
    let twin = fnv1a64(twin_root.to_string_lossy().as_bytes());
    let runtime_surface_layouts = world.resource::<RuntimeSurfaceLayouts>().revision();
    Some(
        docs.wrapping_add(persp)
            .wrapping_add(twin)
            .wrapping_add(active)
            .wrapping_add(dock)
            .wrapping_add(runtime_surface_layouts),
    )
}

/// Build the full hot-exit state from live resources.
fn build_state(world: &mut World) -> Option<WorkspaceState> {
    let twin_root = active_twin_root(world)?;
    let perspective = with_layout(world, |layout, world| layout.active_perspective(world));
    let pairs = capture_documents(world);
    // Active document = index of the snapshot whose live document or saved
    // primary tab matches the focused dock tab. The dock's focused leaf is
    // authoritative; `WorkspaceResource.active_document` is a fallback for
    // the rare path that sets it but never focuses a tab.
    let active_id = with_layout(world, |layout, world| {
        layout.active_tab_instance(world).or_else(|| {
            world
                .resource::<WorkspaceResource>()
                .active_document
                .map(|id| id.raw())
        })
    });
    let active_document = active_id.and_then(|aid| {
        pairs.iter().position(|(id, snapshot)| {
            *id == aid
                || world
                    .get_resource::<DocumentSessionRegistry>()
                    .and_then(|registry| {
                        registry
                            .codecs
                            .iter()
                            .find(|codec| codec.kind() == snapshot.kind)
                    })
                    .is_some_and(|codec| codec.owns_tab_instance(snapshot, aid))
        })
    });
    // Stamp each snapshot with its live id so the persisted dock tree's
    // tab instances can be remapped onto the restored docs next launch.
    let documents: Vec<DocumentSnapshot> = pairs
        .into_iter()
        .map(|(id, mut s)| {
            s.id = id;
            s
        })
        .collect();
    // Capture every perspective's dock tree (active live + each cached) —
    // each perspective's chrome is checked individually inside the capture,
    // so a transient chrome-less dock is skipped rather than round-tripping
    // as a layout with missing panels.
    let docks = if RESTORE_DOCK_ARRANGEMENT {
        with_layout(world, |layout, world| {
            layout.capture_perspective_docks(world)
        })
    } else {
        HashMap::new() // see RESTORE_DOCK_ARRANGEMENT
    };
    let runtime_surface_layouts = world.resource::<RuntimeSurfaceLayouts>().as_map().clone();
    Some(WorkspaceState {
        schema_version: WORKSPACE_STATE_SCHEMA_VERSION,
        twin_root,
        perspective,
        documents,
        active_document,
        docks,
        runtime_surface_layouts,
    })
}

/// Restore the active Twin's saved session — perspective + open
/// documents (with their preserved buffers) — on startup and on every
/// Twin switch. Exclusive system: codecs need `&mut World`.
fn restore_workspace_state(world: &mut World) {
    let active = world.resource::<WorkspaceResource>().active_twin;

    // No-folder sessions use the host's startup layout. They do not share a
    // process-wide document snapshot across luncosim, lunica, and other hosts.
    if active.is_none() {
        world.resource_mut::<PendingWorkspaceRestore>().0 = None;
        let changed = {
            let mut applied = world.resource_mut::<AppliedTwin>();
            if applied.initialized && applied.twin.is_some() {
                applied.twin = None;
                applied.restore_failed = false;
                applied.settle_frames = 0;
                applied.settle_budget = 0;
                true
            } else {
                false
            }
        };
        if changed {
            world.resource_mut::<RuntimeSurfaceLayouts>().clear();
        }
        return;
    }

    if world
        .resource::<PendingWorkspaceRestore>()
        .0
        .as_ref()
        .is_some_and(|pending| pending.twin == active.unwrap())
    {
        poll_workspace_restore(world);
        return;
    }
    world.resource_mut::<PendingWorkspaceRestore>().0 = None;

    // Decide whether to run this frame. Startup restore waits for the
    // doc set to settle (apps auto-open async); a later Twin switch runs
    // immediately (no startup churn to race).
    let twin_changed = {
        let applied = world.resource::<AppliedTwin>();
        applied.initialized && applied.twin != active
    };
    if !twin_changed {
        let rev = session_revision(world);
        let mut applied = world.resource_mut::<AppliedTwin>();
        if applied.initialized {
            return; // startup restore already done, twin unchanged
        }
        if rev == applied.settle_rev {
            applied.settle_frames += 1;
        } else {
            applied.settle_rev = rev;
            applied.settle_frames = 0;
        }
        applied.settle_budget += 1;
        let settled =
            applied.settle_frames >= SETTLE_FRAMES || applied.settle_budget >= SETTLE_BUDGET;
        if !settled {
            return;
        }
    }

    let Some(root) = active_twin_root(world) else {
        warn!("[WorkspaceState] active Twin has no root; skipping workspace restore");
        {
            let mut applied = world.resource_mut::<AppliedTwin>();
            applied.initialized = true;
            applied.twin = active;
            applied.restore_failed = true;
        }
        world.resource_mut::<RuntimeSurfaceLayouts>().clear();
        return;
    };

    {
        let mut applied = world.resource_mut::<AppliedTwin>();
        applied.initialized = true;
        applied.twin = active;
        applied.restore_failed = false;
    }

    let initial_perspective = world
        .resource_mut::<WorkspaceStateRestorePolicy>()
        .take_initial_perspective();

    let load_root = root.clone();
    let task = AsyncComputeTaskPool::get()
        .spawn(async move { WorkspaceState::load_with_json(&load_root) });
    world.resource_mut::<PendingWorkspaceRestore>().0 = Some(WorkspaceRestoreTask {
        twin: active.unwrap(),
        root,
        initial_perspective,
        loaded_json: None,
        state: None,
        task: WorkspaceRestoreTaskKind::Loading(task),
    });
}

fn poll_workspace_restore(world: &mut World) {
    let Some(mut pending) = world.resource_mut::<PendingWorkspaceRestore>().0.take() else {
        return;
    };
    let active = world.resource::<WorkspaceResource>().active_twin;
    let current_root = active_twin_root(world);
    if active != Some(pending.twin) || current_root.as_ref() != Some(&pending.root) {
        return;
    }

    if let WorkspaceRestoreTaskKind::Loading(task) = &mut pending.task {
        let Some(loaded) = block_on(future::poll_once(task)) else {
            world.resource_mut::<PendingWorkspaceRestore>().0 = Some(pending);
            return;
        };
        let Some((state, json)) = loaded else {
            world.resource_mut::<RuntimeSurfaceLayouts>().clear();
            if let Some(perspective) = pending.initial_perspective.take() {
                with_layout_mut(world, |layout, world| {
                    layout.activate_perspective_by_str(world, &perspective);
                });
            }
            seed_workspace_state_last(world, &pending.root, None);
            return;
        };
        let preparation = prepare_workspace_documents(world, &state);
        pending.state = Some(state);
        pending.loaded_json = Some(json);
        pending.task = WorkspaceRestoreTaskKind::Preparing(preparation);
        world.resource_mut::<PendingWorkspaceRestore>().0 = Some(pending);
        return;
    }

    let WorkspaceRestoreTaskKind::Preparing(task) = &mut pending.task else {
        world.resource_mut::<PendingWorkspaceRestore>().0 = Some(pending);
        return;
    };
    let Some(prepared) = block_on(future::poll_once(task)) else {
        world.resource_mut::<PendingWorkspaceRestore>().0 = Some(pending);
        return;
    };
    let mut state = pending
        .state
        .take()
        .expect("document preparation has a workspace state");
    for (snapshot, prepared) in state.documents.iter_mut().zip(prepared) {
        *snapshot = prepared.snapshot;
        if let Some(warning) = prepared.warning {
            warn!("[WorkspaceState] {}", warning);
        }
    }
    seed_workspace_state_last(world, &pending.root, pending.loaded_json.take());
    apply_workspace_state(world, state, pending.initial_perspective.take());
}

fn prepare_workspace_documents(
    world: &mut World,
    state: &WorkspaceState,
) -> Task<Vec<PreparedDocumentSnapshot>> {
    let mut preparations = Vec::with_capacity(state.documents.len());
    world.resource_scope(|world, registry: Mut<DocumentSessionRegistry>| {
        for snapshot in &state.documents {
            if let Some(codec) = registry
                .codecs
                .iter()
                .find(|codec| codec.kind() == snapshot.kind)
            {
                preparations.push(codec.prepare_restore(world, snapshot.clone()));
            } else {
                let snapshot = snapshot.clone();
                preparations.push(
                    Box::pin(async move { PreparedDocumentSnapshot::new(snapshot) })
                        as Pin<Box<dyn Future<Output = PreparedDocumentSnapshot> + Send>>,
                );
            }
        }
    });
    AsyncComputeTaskPool::get().spawn(async move {
        let mut prepared = Vec::with_capacity(preparations.len());
        for preparation in preparations {
            prepared.push(preparation.await);
        }
        prepared
    })
}

fn seed_workspace_state_last(world: &mut World, root: &Path, json: Option<String>) {
    let mut last = world.resource_mut::<WorkspaceStateLast>();
    last.key = Some(workspace_session_key(root));
    last.root = Some(root.to_path_buf());
    last.json = json;
    last.rev = 0;
    last.seeded = false;
}

fn apply_workspace_state(
    world: &mut World,
    state: WorkspaceState,
    initial_perspective: Option<String>,
) {
    world
        .resource_mut::<RuntimeSurfaceLayouts>()
        .replace(state.runtime_surface_layouts.clone());

    // Perspective: reconcile against the registered set (unknown → drop).
    if let Some(persp) = initial_perspective.or(state.perspective) {
        with_layout_mut(world, |layout, world| {
            layout.activate_perspective_by_str(world, &persp);
        });
    }

    // Open every saved document once — the doc set is GLOBAL (shared
    // compile/cosim state across perspectives), so each doc is restored a
    // single time and its tab instance id is remapped identically in every
    // perspective's dock tree below. With no saved docs the open loop is a
    // no-op and the per-perspective dock seed still restores panel layouts /
    // split sizes (a no-doc 3D session still has resized panels).

    // Reconcile docs the app already opened on its own (auto-open, cosim)
    // through their domain codec. File-backed codecs use the registry's
    // same-file identity; untitled origins retain their session names.
    let mut resolved_documents: Vec<(String, u64, DocumentOrigin)> = capture_documents(world)
        .into_iter()
        .map(|(id, snapshot)| (snapshot.kind, id, snapshot.origin))
        .collect();

    // Restore order: non-active first, the active doc last, so the
    // existing open pipeline leaves it focused.
    let mut order: Vec<usize> = (0..state.documents.len()).collect();
    if let Some(active_idx) = state.active_document {
        if active_idx < order.len() {
            order.retain(|&i| i != active_idx);
            order.push(active_idx);
        }
    }

    // (dock kind, saved tab instance) → live instance (see
    // `DocumentSnapshot::tab_instance` / `instance_remaps` / `dock_tab_kind`),
    // so the persisted dock tree's `TabId::Instance` ids are remapped onto
    // the live tabs — scoped per kind so a model-view tab and a plot tab that
    // share an instance number aren't cross-rewritten.
    let mut id_map: std::collections::HashMap<(&'static str, u64), u64> =
        std::collections::HashMap::new();
    let mut discard_unmapped_kinds = HashSet::new();
    let mut restore_incomplete = false;

    world.resource_scope(|world, reg: Mut<DocumentSessionRegistry>| {
        discard_unmapped_kinds.extend(
            reg.codecs
                .iter()
                .filter_map(|codec| codec.discard_unmapped_dock_tab_kind()),
        );
        for idx in order {
            let snap = &state.documents[idx];
            let codec = reg.codecs.iter().find(|c| c.kind() == snap.kind);
            // Resolve the live id: same-origin startup docs reconcile through
            // the domain codec, which can restore saved buffers without
            // minting a second identity for the file.
            let existing_id = resolved_documents
                .iter()
                .find(|(kind, _, origin)| kind == &snap.kind && origin == &snap.origin)
                .map(|(_, live, _)| *live);
            let live_id = if let Some(live) = existing_id {
                if let Some(codec) = codec {
                    codec.restore_existing(world, snap, live)
                } else {
                    Some(live)
                }
            } else if let Some(codec) = codec {
                codec.restore(world, snap)
            } else {
                warn!(
                    "[WorkspaceState] no codec for kind {:?}; dropping restored doc {:?}",
                    snap.kind, snap.title
                );
                restore_incomplete = true;
                None
            };
            if let Some(live_id) = live_id {
                resolved_documents.push((snap.kind.clone(), live_id, snap.origin.clone()));
            } else {
                restore_incomplete = true;
            }
            // Apply the per-doc view state (zoom/pan) and collect the dock
            // tab-instance remap, regardless of whether the doc was freshly
            // restored or matched an already-open one.
            if let (Some(c), Some(lid)) = (codec, live_id) {
                c.apply_view_state(world, lid, snap);
                if let Some(kind) = c.dock_tab_kind() {
                    for (old_inst, new_inst) in c.instance_remaps(world, snap, lid) {
                        id_map.insert((kind, old_inst), new_inst);
                    }
                }
            }
        }
    });

    // Clobber guard: any unresolved snapshot makes this restore incomplete.
    // Skip persistence so a partial restore cannot overwrite saved state.
    if restore_incomplete {
        world.resource_mut::<AppliedTwin>().restore_failed = true;
        warn!(
            "[WorkspaceState] some saved documents could not be restored; \
             skipping persist this session to preserve the saved file"
        );
    }

    // Re-install every perspective's saved dock tree (5a). The active
    // perspective's tree is reconciled into the live dock (overwriting the
    // default-position tabs the codecs just opened with the saved
    // arrangement + split sizes); every other saved perspective is
    // reconciled into the per-perspective cache so switching to it restores
    // its own tabs. `id_map` remaps doc tab instances (old tab id → live
    // tab id) across all trees; the codecs' deferred `OpenTab` then focuses
    // them. See RESTORE_DOCK_ARRANGEMENT.
    if RESTORE_DOCK_ARRANGEMENT {
        with_layout_mut(world, |layout, world| {
            layout.seed_perspective_docks(world, &state.docks, &id_map, &discard_unmapped_kinds);
        });
    }
}

/// Persist the active session when it changes. Cheaply gated by
/// [`gate_value`] (so buffers aren't cloned every frame), then
/// snapshot-compared like recents before any disk write. Native-only.
fn persist_workspace_state(world: &mut World) {
    if world.resource::<PendingWorkspaceRestore>().0.is_some() {
        return;
    }

    if let Some(mut pending) = world.resource_mut::<PendingWorkspaceSave>().0.take() {
        let Some(result) = block_on(future::poll_once(&mut pending.task)) else {
            world.resource_mut::<PendingWorkspaceSave>().0 = Some(pending);
            return;
        };
        record_workspace_save_result(world, pending.key, pending.root, pending.rev, result);
        return;
    }

    // Don't persist until the startup restore has run — otherwise the
    // app's own auto-opened docs would overwrite the saved session
    // before `restore_workspace_state` gets to read it (the systems are
    // chained restore→persist, so by the settle frame this is true).
    {
        let applied = world.resource::<AppliedTwin>();
        if !applied.initialized {
            return;
        }
        // A restore that loaded docs but resolved none live must NOT be
        // followed by a persist — that would write an empty state over the
        // still-good saved session. Leave the file as-is this session.
        if applied.restore_failed {
            return;
        }
    }
    let Some(rev) = gate_value(world) else {
        return;
    };
    let Some(root) = active_twin_root(world) else {
        return;
    };
    let key = workspace_session_key(&root);
    {
        let last = world.resource::<WorkspaceStateLast>();
        if last.seeded && last.rev == rev && last.root.as_ref() == Some(&root) {
            return;
        }
    }
    let Some(state) = build_state(world) else {
        return;
    };
    let previous_key = world.resource::<WorkspaceStateLast>().key.clone();
    let previous_json = world.resource::<WorkspaceStateLast>().json.clone();
    let task_key = key.clone();
    let task = AsyncComputeTaskPool::get().spawn(async move {
        let json = serde_json::to_string_pretty(&state).map_err(|error| error.to_string())?;
        if previous_key.as_deref() != Some(task_key.as_str())
            || previous_json.as_deref() != Some(json.as_str())
        {
            #[cfg(not(target_arch = "wasm32"))]
            state
                .save_serialized(&json)
                .map_err(|error| error.to_string())?;
        }
        Ok(json)
    });
    world.resource_mut::<PendingWorkspaceSave>().0 = Some(WorkspaceSaveTask {
        key,
        root,
        rev,
        task,
    });
}

fn record_workspace_save_result(
    world: &mut World,
    key: String,
    root: PathBuf,
    rev: u64,
    result: Result<String, String>,
) {
    let mut last = world.resource_mut::<WorkspaceStateLast>();
    last.key = Some(key);
    last.root = Some(root);
    last.rev = rev;
    last.seeded = true;
    match result {
        Ok(json) => last.json = Some(json),
        Err(error) => warn!("[WorkspaceState] save failed: {error}"),
    }
}

/// Keep process shutdown behind the workspace snapshot writer. The task owns
/// serialized state and uses the normal `lunco-storage` atomic write, so it
/// can finish without reading or mutating ECS state.
fn wait_for_workspace_save_before_exit(world: &mut World) {
    let has_exit = world
        .get_resource::<bevy::ecs::message::Messages<bevy::app::AppExit>>()
        .is_some_and(|messages| !messages.is_empty());
    if !has_exit {
        return;
    }
    let Some(WorkspaceSaveTask {
        key,
        root,
        rev,
        task,
    }) = world.resource_mut::<PendingWorkspaceSave>().0.take()
    else {
        return;
    };
    let result = block_on(task);
    record_workspace_save_result(world, key, root, rev, result);
}

/// Registers per-Twin workspace-state load/save for a host layout.
pub struct WorkspaceStatePlugin {
    provider: std::sync::Arc<dyn WorkspaceStateLayoutProvider>,
}

impl WorkspaceStatePlugin {
    /// Create the state plugin with the host's concrete layout adapter.
    pub fn new(provider: impl WorkspaceStateLayoutProvider) -> Self {
        Self {
            provider: std::sync::Arc::new(provider),
        }
    }
}

impl Plugin for WorkspaceStatePlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(LayoutProvider(std::sync::Arc::clone(&self.provider)))
            .init_resource::<WorkspaceStateLast>()
            .init_resource::<PendingWorkspaceRestore>()
            .init_resource::<PendingWorkspaceSave>()
            .init_resource::<AppliedTwin>()
            .init_resource::<WorkspaceStateRestorePolicy>()
            .init_resource::<RuntimeSurfaceLayouts>()
            .init_resource::<DocumentSessionRegistry>()
            .add_observer(clear_runtime_surface_layouts_on_twin_closed)
            .add_systems(
                Update,
                (restore_workspace_state, persist_workspace_state).chain(),
            )
            .add_systems(Last, wait_for_workspace_save_before_exit);
    }
}

fn clear_runtime_surface_layouts_on_twin_closed(
    _trigger: On<lunco_workspace::TwinClosed>,
    mut layouts: ResMut<RuntimeSurfaceLayouts>,
) {
    layouts.clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_perspective_is_consumed_once() {
        let mut policy = WorkspaceStateRestorePolicy::with_initial_perspective("sandbox_view");

        assert_eq!(
            policy.take_initial_perspective().as_deref(),
            Some("sandbox_view")
        );
        assert_eq!(policy.take_initial_perspective(), None);
    }

    #[test]
    fn app_exit_waits_for_workspace_snapshot_write() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_message::<bevy::app::AppExit>();
        app.init_resource::<PendingWorkspaceSave>();
        app.init_resource::<WorkspaceStateLast>();
        let task = AsyncComputeTaskPool::get().spawn(async { Ok("saved snapshot".to_owned()) });
        app.world_mut().resource_mut::<PendingWorkspaceSave>().0 = Some(WorkspaceSaveTask {
            key: "twin-key".into(),
            root: PathBuf::from("/twin"),
            rev: 42,
            task,
        });
        app.world_mut()
            .resource_mut::<bevy::ecs::message::Messages<bevy::app::AppExit>>()
            .write(bevy::app::AppExit::Success);

        wait_for_workspace_save_before_exit(app.world_mut());

        assert!(
            app.world()
                .resource::<bevy::ecs::message::Messages<bevy::app::AppExit>>()
                .len()
                == 1
        );
        let last = app.world().resource::<WorkspaceStateLast>();
        assert_eq!(last.json.as_deref(), Some("saved snapshot"));
        assert_eq!(last.rev, 42);
        assert!(last.seeded);
    }

    /// FNV-1a is stable for a given input — the keying must not drift,
    /// or yesterday's state files become unfindable.
    #[test]
    fn fnv1a64_is_stable() {
        assert_eq!(fnv1a64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a64(b"a"), 0xaf63_dc4c_8601_ec8c);
    }

    #[test]
    fn runtime_surface_layouts_accept_finite_positions_and_prune_stale_ids() {
        let mut layouts = RuntimeSurfaceLayouts::default();
        let initial_revision = layouts.revision();

        assert!(!layouts.set(
            "invalid",
            RuntimeSurfaceLayout {
                left: f32::NAN,
                top: 0.0,
            },
        ));
        assert_eq!(layouts.revision(), initial_revision);

        let camera = RuntimeSurfaceLayout {
            left: 120.0,
            top: 48.0,
        };
        assert!(layouts.set("camera-status", camera));
        assert!(!layouts.set("camera-status", camera));
        assert_eq!(layouts.get("camera-status"), Some(camera));

        assert!(layouts.set(
            "stale-surface",
            RuntimeSurfaceLayout {
                left: 8.0,
                top: 16.0,
            },
        ));
        let live_ids: std::collections::HashSet<_> =
            ["camera-status".to_owned()].into_iter().collect();
        layouts.retain_ids(&live_ids);
        assert_eq!(layouts.get("camera-status"), Some(camera));
        assert_eq!(layouts.get("stale-surface"), None);

        assert!(layouts.reset("camera-status"));
        assert!(!layouts.reset("camera-status"));
        assert_eq!(layouts.get("camera-status"), None);
    }

    /// Distinct Twin roots must land on distinct state files.
    #[test]
    fn distinct_roots_distinct_paths() {
        let a = workspace_state_path(Path::new("/tmp/lunco-test-twin-a"));
        let b = workspace_state_path(Path::new("/tmp/lunco-test-twin-b"));
        assert_ne!(a, b);
        assert!(a.to_string_lossy().ends_with(".json"));
    }

    /// End-to-end: save round-trips through the storage boundary, and a state
    /// file whose stored `twin_root` doesn't match the lookup root is rejected
    /// (hash-collision guard). One test keeps the process-level test config
    /// isolation in one place.
    #[test]
    fn save_load_roundtrip_and_collision_guard() {
        lunco_settings::isolate_config_dir_for_tests("workbench-state");
        let temp = tempfile::tempdir().expect("temporary workspace-state directory");
        let root = temp.path().join("proj");
        let state = WorkspaceState {
            schema_version: WORKSPACE_STATE_SCHEMA_VERSION,
            twin_root: root.clone(),
            perspective: Some("analyze".into()),
            documents: vec![
                DocumentSnapshot {
                    kind: "modelica".into(),
                    origin: DocumentOrigin::writable_file(root.join("a.mo")),
                    title: "a.mo".into(),
                    source: "model A end A;".into(),
                    dirty: true,
                    id: 1,
                    tab_instance: 0,
                    view_state: serde_json::Value::Null,
                },
                DocumentSnapshot {
                    kind: "modelica".into(),
                    origin: DocumentOrigin::untitled("Untitled-2"),
                    title: "Untitled-2".into(),
                    source: "model Scratch end Scratch;".into(),
                    dirty: true,
                    id: 2,
                    tab_instance: 0,
                    view_state: serde_json::json!({"zoom": 1.5}),
                },
            ],
            active_document: Some(0),
            docks: HashMap::new(),
            runtime_surface_layouts: HashMap::new(),
        };
        state.save().unwrap();

        let loaded = WorkspaceState::load(&root).expect("round-trips");
        assert_eq!(loaded, state);

        // Tamper the stored root → load must reject it.
        let path = workspace_state_path(&root);
        let mut bad = state;
        bad.twin_root = PathBuf::from("/totally/different");
        lunco_storage::write_file_sync(&path, serde_json::to_string(&bad).unwrap().as_bytes())
            .unwrap();
        assert!(WorkspaceState::load(&root).is_none(), "collision guard");
    }

    /// Per-perspective docks round-trip through serde, and the written form
    /// carries no single-`dock` field. In-memory — avoids
    /// the process-global `LUNCOSIM_CONFIG` env var the disk test above
    /// uses, so it can run in parallel with it.
    #[test]
    fn per_perspective_docks_serde_roundtrip() {
        let state = WorkspaceState {
            schema_version: WORKSPACE_STATE_SCHEMA_VERSION,
            twin_root: PathBuf::from("/proj"),
            perspective: Some("build".into()),
            documents: Vec::new(),
            active_document: None,
            docks: HashMap::from([
                (
                    "design".to_string(),
                    PerspectiveDockSnapshot {
                        layout_revision: 0,
                        dock: serde_json::json!({"surfaces": []}),
                        side_browser: vec![PanelId("browser")],
                        side_browser_bottom: vec![],
                        center: vec![PanelId("canvas")],
                        active_center_tab: 2,
                        right_inspector: vec![],
                        right_inspector_bottom: vec![],
                        bottom: vec![PanelId("plots")],
                    },
                ),
                (
                    "build".to_string(),
                    PerspectiveDockSnapshot {
                        dock: serde_json::json!({"surfaces": [{"main": []}]}),
                        ..Default::default()
                    },
                ),
            ]),
            runtime_surface_layouts: HashMap::new(),
        };
        let json = serde_json::to_string(&state).unwrap();
        let back: WorkspaceState = serde_json::from_str(&json).unwrap();
        assert_eq!(back, state, "per-perspective docks must round-trip");
        // Each perspective's slot intent survives verbatim.
        let design = &back.docks["design"];
        assert_eq!(design.side_browser, vec![PanelId("browser")]);
        assert_eq!(design.bottom, vec![PanelId("plots")]);
        assert_eq!(design.active_center_tab, 2);
    }

    #[test]
    fn pre_versioned_single_dock_is_rejected() {
        let old = serde_json::json!({
            "twin_root": "/proj",
            "perspective": "build",
            "documents": [],
            "active_document": null,
            "dock": {"surfaces": [{"main": []}]}
        });
        assert!(decode_workspace_state(&old.to_string()).is_err());
    }

    #[test]
    fn unknown_workspace_state_fields_and_versions_are_rejected() {
        let unknown_field = serde_json::json!({
            "schema_version": WORKSPACE_STATE_SCHEMA_VERSION,
            "twin_root": "/proj",
            "perspective": null,
            "documents": [],
            "active_document": null,
            "docks": {},
            "old_dock": {}
        });
        assert!(decode_workspace_state(&unknown_field.to_string()).is_err());

        let future = serde_json::json!({
            "schema_version": WORKSPACE_STATE_SCHEMA_VERSION + 1,
            "twin_root": "/proj",
            "perspective": null,
            "documents": [],
            "active_document": null,
            "docks": {}
        });
        assert!(decode_workspace_state(&future.to_string()).is_err());
    }
}
