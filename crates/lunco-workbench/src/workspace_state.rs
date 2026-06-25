//! Per-Twin (per-project) workbench state — VSCode's `workspaceStorage`.
//!
//! VSCode keeps *volatile UI state* (which editors were open, the active
//! one, window layout) in a global store keyed by a hash of the
//! workspace path — **not** inside the project folder, so repos stay
//! clean. We do the same: each Twin gets a
//! `~/.lunco/workspace-state/<hash>.json` keyed off its root path.
//!
//! ## What's stored (and what isn't)
//!
//! - **Active perspective** — restored on Twin activation (workbench
//!   local, side-effect free).
//! - **Open document paths + active document** — persisted so a future
//!   session-restore can reopen them. We do *not* auto-reopen yet:
//!   reopening means replaying domain-specific open commands
//!   (`OpenClass` for Modelica, scene-open for USD, …) with parse /
//!   recompile side effects. That wiring is a deliberate follow-up; the
//!   data is captured now so it's ready when it lands.
//!
//! Global, app-wide preferences (theme, perf HUD, **default window
//! geometry**) stay in `~/.lunco/settings.json` via `lunco-settings` —
//! see [`crate::window_persistence`]. This module owns only the
//! per-project slice.
//!
//! ## Persistence pattern
//!
//! Mirrors recents (`session.rs`): load on Twin activation, save on
//! change via a serialized-snapshot compare (so unrelated
//! `WorkspaceResource` mutations don't write), atomic tmp+rename, and a
//! corrupt / missing file degrades to "open with defaults" — never a
//! panic.

use std::path::{Path, PathBuf};

use bevy::prelude::*;
use lunco_doc::DocumentOrigin;
use serde::{Deserialize, Serialize};

use lunco_workspace::WorkspaceResource;
use crate::WorkbenchLayout;

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
    /// replacement via [`DocumentSessionCodec::instance_remap`] on restore.
    /// 0 when the domain has no dock tab (USD) or for older state files.
    #[serde(default)]
    pub tab_instance: u64,
    /// Opaque per-domain view state — canvas zoom/pan, etc. `null` when
    /// the domain has none. The Modelica codec serializes its per-tab
    /// `Viewport` here; USD leaves it null. Generic `Value` keeps
    /// `lunco-workbench` domain-agnostic.
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
    /// set or any buffer changes (fold of doc ids + generations). Lets
    /// capture skip the (allocating) snapshot build in the steady state
    /// — no per-frame buffer clones (AGENTS.md §7.1).
    fn revision(&self, world: &World) -> u64;
    /// Snapshot every open document of this kind, each paired with its
    /// **live** `DocumentId` (`raw()`) for *this* session. The id lets
    /// the workbench match the active tab reliably (origins can differ
    /// between the registry and the Workspace entry); it is not
    /// persisted — ids aren't stable across runs.
    fn capture(&self, world: &mut World) -> Vec<(u64, DocumentSnapshot)>;
    /// Recreate one document from a snapshot, replaying the domain's
    /// normal open path (which opens the tab + registers the entry).
    /// Returns the freshly-allocated `DocumentId.raw()` so the workbench
    /// can remap the persisted dock tree's tab instance ids onto it;
    /// `None` if restore was a no-op (e.g. the registry was missing).
    fn restore(&self, world: &mut World, snap: &DocumentSnapshot) -> Option<u64>;
    /// Apply the snapshot's opaque [`view_state`](DocumentSnapshot::view_state)
    /// (canvas zoom/pan, …) to the **live** document identified by
    /// `live_id` (`DocumentId.raw()`). Called for *every* restored doc —
    /// both freshly [`restore`](Self::restore)d ones **and** docs the app
    /// auto-opened that matched a snapshot (so a reopened diagram restores
    /// its camera even when the open itself was deduped). Default no-op for
    /// domains without per-doc view state (USD). Runs after `restore`.
    fn apply_view_state(&self, _world: &mut World, _live_id: u64, _snap: &DocumentSnapshot) {}
    /// Report how this doc's **dock tab instance id** maps from the saved
    /// session to this one: `(old, new)` where `old` is
    /// [`DocumentSnapshot::tab_instance`] (the instance id in the persisted
    /// dock tree) and `new` is the live instance id after [`restore`] opened
    /// the tab. Used only by 5a (dock-arrangement restore) to remap
    /// `TabId::Instance` ids in the deserialized dock onto the live tabs.
    /// `live_id` is the freshly-restored `DocumentId.raw()`. Returns `None`
    /// when the domain has no dock tab to remap (USD) — the default.
    fn instance_remap(
        &self,
        _world: &mut World,
        _snap: &DocumentSnapshot,
        _live_id: u64,
    ) -> Option<(u64, u64)> {
        None
    }
    /// The workbench `PanelId` string of the dock tab kind this codec's
    /// documents use (e.g. `"modelica_model_view"`). The dock remap (5a)
    /// keys on `(kind, instance)` so tab ids are only rewritten within the
    /// codec's own kind — different kinds share the `u64` instance space
    /// (e.g. a model-view tab and a plot tab can both be instance 1), so a
    /// flat instance→instance map would cross-rewrite them. `None` (default)
    /// means [`instance_remap`](Self::instance_remap) is unused.
    fn dock_tab_kind(&self) -> Option<&'static str> {
        None
    }
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

/// Per-Twin volatile UI state. One of these per project, stored at
/// [`workspace_state_path`].
#[derive(Serialize, Deserialize, Default, Clone, PartialEq, Debug)]
pub struct WorkspaceState {
    /// The Twin root this state belongs to (empty when no Twin is
    /// active — a "no-folder" session still hot-exits its docs). Stored
    /// so a hash collision (two paths landing on the same file stem) is
    /// detectable — a mismatch is treated as a miss.
    pub twin_root: PathBuf,
    /// `PerspectiveId` string of the perspective active at save time.
    /// `None` ⇒ leave the app's startup default.
    pub perspective: Option<String>,
    /// Hot-exit snapshots of every open document, in open order.
    pub documents: Vec<DocumentSnapshot>,
    /// Index into [`documents`](Self::documents) of the active tab.
    pub active_document: Option<usize>,
    /// Serialized `egui_dock::DockState<TabId>` — the full center/side
    /// arrangement plus split sizes and the active leaf (lunica's whole
    /// layout lives here). `None` ⇒ no dock captured (older file, or an
    /// app that doesn't use the dock); restore falls back to the
    /// perspective preset + codec-opened tabs.
    #[serde(default)]
    pub dock: Option<serde_json::Value>,
}

impl WorkspaceState {
    /// Load the state for a Twin root. Returns `None` on missing /
    /// unreadable / corrupt file, or when the stored `twin_root` doesn't
    /// match (hash collision guard) — all of which mean "use defaults".
    pub fn load(twin_root: &Path) -> Option<Self> {
        let path = workspace_state_path(twin_root);
        use lunco_storage::Storage;
        let bytes = lunco_storage::FileStorage::new()
            .read_sync(&lunco_storage::StorageHandle::File(path.clone()))
            .ok()?;
        let text = String::from_utf8(bytes).ok()?;
        let state: WorkspaceState = serde_json::from_str(&text).ok()?;
        if state.twin_root != twin_root {
            warn!(
                "[WorkspaceState] {} stores a different twin_root ({}); ignoring (hash collision?)",
                path.display(),
                state.twin_root.display(),
            );
            return None;
        }
        Some(state)
    }

    /// Atomically write this state for its `twin_root` (tmp + rename so a
    /// kill mid-write can't corrupt the file).
    pub fn save(&self) -> std::io::Result<()> {
        let path = workspace_state_path(&self.twin_root);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let tmp = path.with_extension("json.tmp");
        // Write through lunco-storage (clippy-banned `std::fs::write`,
        // wasm-incompatible); `rename` isn't on the ban list, so the
        // atomic tmp→final swap is preserved.
        {
            use lunco_storage::Storage;
            lunco_storage::FileStorage::new()
                .write_sync(&lunco_storage::StorageHandle::File(tmp.clone()), json.as_bytes())
                .map_err(|e| std::io::Error::other(e.to_string()))?;
        }
        std::fs::rename(&tmp, &path)
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

/// Resolve the on-disk path for a Twin's state file:
/// `<config>/workspace-state/<fnv1a-hex>.json`. Honours the
/// `LUNCOSIM_CONFIG` override via `lunco_assets::user_config_dir`.
///
/// The root is canonicalized first when possible so cwd-relative and
/// absolute spellings of the same folder collapse to one key; falls back
/// to the raw path bytes when canonicalization fails (e.g. the folder
/// was deleted).
pub fn workspace_state_path(twin_root: &Path) -> PathBuf {
    let canonical = std::fs::canonicalize(twin_root).unwrap_or_else(|_| twin_root.to_path_buf());
    let key = fnv1a64(canonical.to_string_lossy().as_bytes());
    lunco_assets::user_config_dir()
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
    /// Hex key of the Twin the snapshot belongs to.
    key: Option<String>,
    /// Pretty-printed JSON of the last-saved [`WorkspaceState`].
    json: String,
    /// Cheap fold gating the (allocating) snapshot build — see
    /// [`gate_value`].
    rev: u64,
    /// Set once `rev` has been computed at least once.
    seeded: bool,
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
    /// Set when the loaded state had documents but NONE resolved to a live
    /// doc (restore produced nothing). Guards against the clobber footgun:
    /// `initialized` is set before the restore body runs, so a restore that
    /// silently no-ops would otherwise let the next persist overwrite the
    /// saved session with an empty state. When this is set we skip persist
    /// for the rest of the session, preserving the file untouched.
    restore_failed: bool,
}

/// **Phase 5a (dock-arrangement restore).** Serialize the full
/// `egui_dock::DockState` (split sizes + tab arrangement) and re-install it
/// on restore. The earlier blank-center regression was caused by remapping
/// `TabId::Instance` ids in the wrong key space: the dock instance is the
/// domain's tab id (Modelica `ModelTabs` counter), NOT `DocumentId.raw()`,
/// so the stale instance pointed at no live tab and rendered empty. Fixed
/// by [`DocumentSessionCodec::instance_remap`] + [`DocumentSnapshot::tab_instance`]
/// (old tab id → live tab id); `set_dock_from_json` now remaps mapped
/// instances and KEEPS unmapped ones (stable-id tabs like the default plot).
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

/// Absolute root of the active Twin, or the empty path for a "no-folder"
/// session (which still hot-exits its docs into a sentinel file).
fn active_twin_root(world: &World) -> PathBuf {
    let ws = world.resource::<WorkspaceResource>();
    ws.active_twin
        .and_then(|id| ws.twin(id))
        .map(|t| t.root.clone())
        .unwrap_or_default()
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
fn gate_value(world: &mut World) -> u64 {
    let docs = session_revision(world);
    let persp = world
        .resource::<WorkbenchLayout>()
        .active_perspective()
        .map(|p| fnv1a64(p.as_str().as_bytes()))
        .unwrap_or(0);
    let twin = fnv1a64(active_twin_root(world).to_string_lossy().as_bytes());
    // Fold in the focused tab so switching tabs re-fires the gate and
    // re-saves the active index (the dock focus is the real signal;
    // `active_document` is the fallback the build also uses).
    let active = world
        .resource::<WorkbenchLayout>()
        .active_tab_instance()
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
    let dock = if RESTORE_DOCK_ARRANGEMENT {
        world
            .resource::<WorkbenchLayout>()
            .dock_json()
            .and_then(|v| serde_json::to_string(&v).ok())
            .map(|s| fnv1a64(s.as_bytes()))
            .unwrap_or(0)
    } else {
        0
    };
    docs.wrapping_add(persp)
        .wrapping_add(twin)
        .wrapping_add(active)
        .wrapping_add(dock)
}

/// Build the full hot-exit state from live resources.
fn build_state(world: &mut World) -> WorkspaceState {
    let twin_root = active_twin_root(world);
    let perspective = world
        .resource::<WorkbenchLayout>()
        .active_perspective()
        .map(|p| p.as_str().to_string());
    let pairs = capture_documents(world);
    // Active tab = index of the document whose live id matches the
    // focused dock tab. The dock's focused leaf is authoritative;
    // `WorkspaceResource.active_document` is a fallback for the rare
    // path that sets it but never focuses a tab. Doc tabs carry their
    // `DocumentId.raw()` as the instance, so this matches `pairs` ids.
    let active_id = world
        .resource::<WorkbenchLayout>()
        .active_tab_instance()
        .or_else(|| {
            world
                .resource::<WorkspaceResource>()
                .active_document
                .map(|id| id.raw())
        });
    let active_document =
        active_id.and_then(|aid| pairs.iter().position(|(id, _)| *id == aid));
    // Stamp each snapshot with its live id so the persisted dock tree's
    // tab instances can be remapped onto the restored docs next launch.
    let documents: Vec<DocumentSnapshot> = pairs
        .into_iter()
        .map(|(id, mut s)| {
            s.id = id;
            s
        })
        .collect();
    let dock = if RESTORE_DOCK_ARRANGEMENT {
        let layout = world.resource::<WorkbenchLayout>();
        if layout.perspective_chrome_complete() {
            layout.dock_json()
        } else {
            // Don't persist a transient chrome-less dock (a centre-driven
            // perspective whose dock momentarily lost its side/right/bottom
            // panels — e.g. mid-switch through the viewport-only rebuild
            // branch). Writing it would round-trip as a layout with missing
            // panels; dropping it lets restore rebuild chrome from intent.
            None
        }
    } else {
        None // see RESTORE_DOCK_ARRANGEMENT
    };
    WorkspaceState {
        twin_root,
        perspective,
        documents,
        active_document,
        dock,
    }
}

/// Restore the active Twin's saved session — perspective + open
/// documents (with their preserved buffers) — on startup and on every
/// Twin switch. Exclusive system: codecs need `&mut World`.
fn restore_workspace_state(world: &mut World) {
    let active = world.resource::<WorkspaceResource>().active_twin;

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

    {
        let mut applied = world.resource_mut::<AppliedTwin>();
        applied.initialized = true;
        applied.twin = active;
    }

    let root = active_twin_root(world);
    let Some(state) = WorkspaceState::load(&root) else {
        return;
    };

    // Perspective: reconcile against the registered set (unknown → drop).
    if let Some(persp) = &state.perspective {
        world
            .resource_mut::<WorkbenchLayout>()
            .activate_perspective_by_str(persp);
    }

    // Restore the sandbox panel sizes regardless of whether any docs are
    // saved (a no-doc 3D session still has resized panels to restore).
    if state.documents.is_empty() {
        // No docs, but a saved dock layout (e.g. side-panel arrangement /
        // split sizes) can still apply with an empty remap (no instance
        // tabs to remap).
        if RESTORE_DOCK_ARRANGEMENT {
            if let Some(dock) = state.dock.clone() {
                let mut layout = world.resource_mut::<WorkbenchLayout>();
                layout.set_dock_from_json(dock, &std::collections::HashMap::new());
                // A persisted tree can omit the active perspective's
                // chrome (saved during a viewport-only state / older
                // layout); re-attach it so side/right panels don't
                // silently vanish in dock-mode.
                layout.ensure_chrome_present();
            }
        }
        return;
    }

    // Dedup against docs the app already opened on its own (auto-open,
    // cosim): skip any saved snapshot whose origin is already present.
    // Untitled origins carry a per-run name so they never collide and
    // always re-open — exactly right for scratch buffers.
    let existing: Vec<(u64, DocumentOrigin)> = capture_documents(world)
        .into_iter()
        .map(|(id, s)| (id, s.origin))
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
    // `DocumentSnapshot::tab_instance` / `instance_remap` / `dock_tab_kind`),
    // so the persisted dock tree's `TabId::Instance` ids are remapped onto
    // the live tabs — scoped per kind so a model-view tab and a plot tab that
    // share an instance number aren't cross-rewritten.
    let mut id_map: std::collections::HashMap<(&'static str, u64), u64> =
        std::collections::HashMap::new();
    let mut any_live = false;

    world.resource_scope(|world, reg: Mut<DocumentSessionRegistry>| {
        for idx in order {
            let snap = &state.documents[idx];
            let codec = reg.codecs.iter().find(|c| c.kind() == snap.kind);
            // Resolve the live id: an already-open doc (auto-open / cosim /
            // dedup) reuses its live id; otherwise the codec recreates it.
            let live_id = if let Some((live, _)) =
                existing.iter().find(|(_, o)| o == &snap.origin)
            {
                Some(*live)
            } else if let Some(c) = codec {
                c.restore(world, snap)
            } else {
                warn!(
                    "[WorkspaceState] no codec for kind {:?}; dropping restored doc {:?}",
                    snap.kind, snap.title
                );
                None
            };
            // Apply the per-doc view state (zoom/pan) and collect the dock
            // tab-instance remap, regardless of whether the doc was freshly
            // restored or matched an already-open one.
            if let (Some(c), Some(lid)) = (codec, live_id) {
                any_live = true;
                c.apply_view_state(world, lid, snap);
                if let (Some(kind), Some((old_inst, new_inst))) =
                    (c.dock_tab_kind(), c.instance_remap(world, snap, lid))
                {
                    id_map.insert((kind, old_inst), new_inst);
                }
            }
        }
    });

    // Clobber guard: the saved state had docs but none resolved live →
    // restore effectively failed. Flag it so `persist_workspace_state`
    // skips writing (an empty state would overwrite the good session).
    if !state.documents.is_empty() && !any_live {
        world.resource_mut::<AppliedTwin>().restore_failed = true;
        warn!(
            "[WorkspaceState] restore loaded {} doc(s) but none became live; \
             skipping persist this session to preserve the saved file",
            state.documents.len()
        );
    }

    // Apply the persisted dock tree last (5a), overwriting the
    // default-position tabs the codecs just opened with the saved
    // arrangement + split sizes. `id_map` remaps doc tab instances (old
    // tab id → live tab id) so the re-installed dock's `TabId::Instance`
    // entries point at the live tabs; the codecs' deferred `OpenTab` then
    // focuses them. See RESTORE_DOCK_ARRANGEMENT.
    if RESTORE_DOCK_ARRANGEMENT {
        if let Some(dock) = state.dock.clone() {
            let mut layout = world.resource_mut::<WorkbenchLayout>();
            layout.set_dock_from_json(dock, &id_map);
            // Guarantee the perspective's chrome (side browser /
            // inspectors / bottom) survives a partial or viewport-only
            // persisted tree. Open documents are preserved by the
            // rebuild; only split sizes reset.
            layout.ensure_chrome_present();
        }
    }
}

/// Persist the active session when it changes. Cheaply gated by
/// [`gate_value`] (so buffers aren't cloned every frame), then
/// snapshot-compared like recents before any disk write. Native-only.
fn persist_workspace_state(world: &mut World) {
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
    let rev = gate_value(world);
    {
        let last = world.resource::<WorkspaceStateLast>();
        if last.seeded && last.rev == rev {
            return;
        }
    }
    let state = build_state(world);
    let key = format!("{:016x}", fnv1a64(
        std::fs::canonicalize(&state.twin_root)
            .unwrap_or_else(|_| state.twin_root.clone())
            .to_string_lossy()
            .as_bytes(),
    ));
    let current = match serde_json::to_string_pretty(&state) {
        Ok(s) => s,
        Err(e) => {
            warn!("[WorkspaceState] serialise failed: {e}");
            return;
        }
    };
    let mut last = world.resource_mut::<WorkspaceStateLast>();
    last.rev = rev;
    last.seeded = true;
    if last.key.as_deref() == Some(key.as_str()) && current == last.json {
        return;
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        if let Err(e) = state.save() {
            warn!("[WorkspaceState] save failed: {e}");
            return;
        }
    }
    last.key = Some(key);
    last.json = current;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// FNV-1a is stable for a given input — the keying must not drift,
    /// or yesterday's state files become unfindable.
    #[test]
    fn fnv1a64_is_stable() {
        assert_eq!(fnv1a64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a64(b"a"), 0xaf63_dc4c_8601_ec8c);
    }

    /// Distinct Twin roots must land on distinct state files.
    #[test]
    fn distinct_roots_distinct_paths() {
        let a = workspace_state_path(Path::new("/tmp/lunco-test-twin-a"));
        let b = workspace_state_path(Path::new("/tmp/lunco-test-twin-b"));
        assert_ne!(a, b);
        assert!(a.to_string_lossy().ends_with(".json"));
    }

    /// End-to-end: save round-trips through disk, and a state file whose
    /// stored `twin_root` doesn't match the lookup root is rejected
    /// (hash-collision guard). One test so the `LUNCOSIM_CONFIG` env
    /// override (read by `user_config_dir`) isn't raced by siblings.
    #[test]
    fn save_load_roundtrip_and_collision_guard() {
        let tmp = std::env::temp_dir().join(format!(
            "lunco-ws-state-test-{}",
            fnv1a64(b"roundtrip-fixture")
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("LUNCOSIM_CONFIG", &tmp);

        let root = tmp.join("proj");
        std::fs::create_dir_all(&root).unwrap();
        let state = WorkspaceState {
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
            dock: None,
        };
        state.save().unwrap();

        let loaded = WorkspaceState::load(&root).expect("round-trips");
        assert_eq!(loaded, state);

        // Tamper the stored root → load must reject it.
        let path = workspace_state_path(&root);
        let mut bad = state.clone();
        bad.twin_root = PathBuf::from("/totally/different");
        std::fs::write(&path, serde_json::to_string(&bad).unwrap()).unwrap();
        assert!(WorkspaceState::load(&root).is_none(), "collision guard");

        std::env::remove_var("LUNCOSIM_CONFIG");
        let _ = std::fs::remove_dir_all(&tmp);
    }
}

/// Registers per-Twin workspace-state load/save. Added by
/// [`WorkbenchPlugin`](crate::WorkbenchPlugin) (which owns
/// [`WorkbenchLayout`]). Idempotent.
pub struct WorkspaceStatePlugin;

impl Plugin for WorkspaceStatePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<WorkspaceStateLast>()
            .init_resource::<AppliedTwin>()
            .init_resource::<DocumentSessionRegistry>()
            .add_systems(
                Update,
                (restore_workspace_state, persist_workspace_state).chain(),
            );
    }
}
