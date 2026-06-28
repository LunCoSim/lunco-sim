//! # lunco-doc-bevy
//!
//! Bevy integration for the LunCoSim Document System.
//!
//! `lunco-doc` is pure data (zero deps, headless-testable). This crate
//! adds the ECS-facing half: generic document-lifecycle events that any
//! domain's registry (Modelica, USD, scripting, SysML, …) fires through,
//! plus the [`JournalResource`] — a Bevy wrapper around the canonical
//! Twin journal in `lunco-twin-journal`. Lifecycle events become
//! `EntryKind::Lifecycle` records; structural ops become `EntryKind::Op`
//! records emitted by domain mutation paths. One log per Twin.
//!
//! ## The architectural rule
//!
//! **Documents are mutated only through `#[Command]` observers.** Every
//! user intent — clicking Compile, typing in the editor, dropping a
//! component on the diagram, tweaking a parameter, invoking a remote
//! script — is a command whose observer applies ops to one or more
//! [`lunco_doc::Document`]s. The documents fire
//! [`crate::DocumentChanged`] and its siblings, and every subscriber
//! (re-render, re-parse, telemetry refresh, plot-variable update,
//! replay, audit) reacts from those events.
//!
//! This rule is the nucleus: it means
//!
//! - **Undo/redo works everywhere for free** — per-document
//!   [`lunco_doc::DocumentHost`] stacks handle it; per-twin Workspace
//!   Undo is a journal scope filter.
//! - **Scripting / API / keyboard shortcuts share one path** — they
//!   all fire the same commands.
//! - **The Twin journal is a complete record** — every `Op` and
//!   lifecycle event lands in one canonical store.
//! - **Views don't poll** — they `On<DocumentChanged>` (or similar)
//!   and react.
//!
//! ## Events
//!
//! All four events carry a [`lunco_doc::DocumentId`] so subscribers can
//! resolve the backing document from whichever domain registry owns it.
//! They are plain Bevy `Event`s (no `Reflect`), because `DocumentId`
//! intentionally stays bevy-reflect-free in `lunco-doc`. Commands that
//! need to be reflected (for scripting dispatch) wrap their `DocumentId`
//! with `#[reflect(ignore)]` or similar.
//!
//! ## JournalResource
//!
//! [`JournalResource`] wraps the canonical Twin journal
//! ([`lunco_twin_journal::Journal`]) in `Arc<Mutex<_>>` for ECS access.
//! It is *not* undo history — per-document undo still lives on
//! `DocumentHost`. The journal answers different questions: "what
//! happened this session, in what order, across all documents and
//! domains?" — useful for replay, audit, debugging, the journal panel,
//! and future cross-doc transactions / multi-user sync.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod diagnostics;
pub use diagnostics::DocumentDiagnostics;
// The pure-data half lives in lunco-doc; re-export for convenience so callers
// can reach the whole diagnostics surface from one place.
pub use lunco_doc::{status_json, DocDiagnostics};

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use bevy::prelude::*;
use lunco_core::Command;
use lunco_doc::DocumentId;
use lunco_twin_journal::{
    AuthorId, AuthorTag, Journal as CanonicalJournal, JournalEntry, JournalSink, LifecycleKind,
    TwinId,
};

// ─────────────────────────────────────────────────────────────────────────────
// EventOrigin — which side of the wire produced this event
// ─────────────────────────────────────────────────────────────────────────────
//
// Seed for the server/collaboration future. Today every event is
// `EventOrigin::Local` (fired by this client's own user action). The
// field exists so that when networking lands, observers can filter
// `my edits` vs `incoming edits` without a cross-crate refactor, and
// a replay harness (tests, journal replay, CI fixtures) can clearly
// mark synthetic events.

/// Which side of a (current or future) wire fired an event.
///
/// Kept deliberately coarse — enough to distinguish local/remote/replay
/// for rendering and journaling. Finer attribution (which specific
/// remote client / which keystroke within a session) lives on
/// [`UserId`] / journal entries, not here.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub enum EventOrigin {
    /// The event was produced by a user action in this client's
    /// session. Default — covers every case until networking arrives.
    #[default]
    Local,
    /// The event arrived from a remote peer over the sync transport.
    /// `peer` is the originating client's identity as reported by the
    /// transport.
    Remote {
        /// Opaque peer identity from the transport layer.
        peer: String,
    },
    /// The event was emitted by journal replay (test fixtures, "open
    /// this twin" reconstruction). Observers that persist side-effects
    /// should skip these to avoid re-writing the journal.
    Replay,
}

// ─────────────────────────────────────────────────────────────────────────────
// Lifecycle events
// ─────────────────────────────────────────────────────────────────────────────

/// A [`Document`](lunco_doc::Document) in some domain registry just had
/// an op applied (source changed, generation bumped).
///
/// Subscribers observe via `On<DocumentChanged>` to re-render, re-parse,
/// invalidate caches, or refresh derived UI. Firing happens from the
/// domain registry's mutation path — typically via a `drain_pending_changes`
/// system that fans queued ids out as observer triggers.
#[derive(Event, Clone, Debug)]
pub struct DocumentChanged {
    /// The document whose generation just advanced.
    pub doc: DocumentId,
    /// Which side of the wire produced this event. Local by default.
    pub origin: EventOrigin,
}

impl DocumentChanged {
    /// Local-origin constructor. Default for UI-triggered paths.
    pub fn local(doc: DocumentId) -> Self {
        Self { doc, origin: EventOrigin::Local }
    }
}

/// A new [`Document`](lunco_doc::Document) was added to a domain
/// registry (file opened, New Model created, script allocated, …).
///
/// Fires once per document, immediately after the registry inserts it.
/// Precedes any [`crate::DocumentChanged`] for the same id that would otherwise
/// represent "filled-with-initial-source".
#[derive(Event, Clone, Debug)]
pub struct DocumentOpened {
    /// The newly-registered document.
    pub doc: DocumentId,
    /// Which side of the wire produced this event.
    pub origin: EventOrigin,
}

impl DocumentOpened {
    /// Local-origin constructor.
    pub fn local(doc: DocumentId) -> Self {
        Self { doc, origin: EventOrigin::Local }
    }
}

/// A [`Document`](lunco_doc::Document) was removed from its registry
/// (entity despawned + cleanup, explicit Close, registry reset).
///
/// Subscribers can drop per-document caches (plots, telemetry history,
/// layout state). The id may be reused later for a different document;
/// consumers should not assume ids are stable across Close→Open pairs.
#[derive(Event, Clone, Debug)]
pub struct DocumentClosed {
    /// The document that no longer exists in the registry.
    pub doc: DocumentId,
    /// Which side of the wire produced this event.
    pub origin: EventOrigin,
}

impl DocumentClosed {
    /// Local-origin constructor.
    pub fn local(doc: DocumentId) -> Self {
        Self { doc, origin: EventOrigin::Local }
    }
}

/// A [`Document`](lunco_doc::Document)'s contents were persisted to
/// disk (Save, Save As).
///
/// Fires after a successful write. Dirty trackers use this together
/// with [`crate::DocumentChanged`] to maintain the save indicator.
#[derive(Event, Clone, Debug)]
pub struct DocumentSaved {
    /// The document that was just persisted.
    pub doc: DocumentId,
    /// Which side of the wire produced this event.
    pub origin: EventOrigin,
}

impl DocumentSaved {
    /// Local-origin constructor.
    pub fn local(doc: DocumentId) -> Self {
        Self { doc, origin: EventOrigin::Local }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Commands — generic document operations
// ─────────────────────────────────────────────────────────────────────────────
//
// The events above are *notifications* (something already happened).
// The commands below are *requests* (please do this, possibly).
//
// **Ownership convention**: commands carry a [`DocumentId`] without
// specifying which domain owns it. Each domain's plugin installs
// `On<CommandName>` observers that inspect their own registry and
// either act on the document (if known) or no-op (if foreign).
// Multiple domain plugins coexist cleanly because each no-ops on ids
// it doesn't own. The same id is never owned by two domains —
// registries are the source of truth for ownership.
//
// This pattern keeps generic commands domain-agnostic while letting
// domain-specific behavior (how to undo a text op vs. a USD scene op
// vs. a SysML diagram op) live in each domain crate.

/// Request to undo one op on the document, syncing any dependent UI
/// state (editor buffer, diagram canvas) to match the reverted source.
///
/// Handled per-domain: the registry that owns `doc` runs its
/// [`DocumentHost`](lunco_doc::DocumentHost)`::undo()`, fires
/// [`crate::DocumentChanged`], and performs whatever view-state sync the
/// domain requires (e.g. for Modelica, update the text buffer). Domains
/// that don't own `doc` ignore the trigger.
#[Command(default)]
pub struct UndoDocument {
    /// The document whose most recent op should be undone.
    pub doc: DocumentId,
}

/// Request to redo the last undone op on the document.
///
/// Counterpart of [`UndoDocument`]. Same per-domain dispatch rules.
#[Command(default)]
pub struct RedoDocument {
    /// The document whose most recent undone op should be re-applied.
    pub doc: DocumentId,
}

/// Request to persist the document's current source to disk.
///
/// Handled per-domain: the owning registry resolves the document's
/// canonical path, writes the source, and fires [`DocumentSaved`] on
/// success. No-ops if the document has no canonical path (Save-As
/// needed — separate command, not defined yet) or if the backing
/// library is read-only (MSL, Bundled in Modelica's case).
///
/// Dirty state (generation vs. last-saved generation) is a per-document
/// concern; the owning domain updates its internal tracker in the
/// observer.
#[Command(default)]
pub struct SaveDocument {
    /// The document to persist.
    pub doc: DocumentId,
}

/// Request the owning domain persist the document **to a new location**.
///
/// `path` semantics mirror [`OpenFile`](lunco_workbench::file_ops::OpenFile):
///
/// - **Empty** → the observer fires
///   [`lunco_workbench::picker::PickHandle`](../lunco_workbench/picker/struct.PickHandle.html)
///   with `PickFollowUp::SaveAs(doc)` and returns. The workbench's
///   `on_pick_resolved` re-fires this command with the chosen path
///   filled in. Cancellation is silent.
/// - **Non-empty** → the observer writes directly, rebinds the
///   document's [`lunco_doc::DocumentOrigin`] to the new writable `File` variant,
///   updates `last_saved_generation`, and fires [`DocumentSaved`].
///
/// This single shape covers UI dialogs, recents, drag-drop, HTTP
/// automation, and the Untitled-promotion path (Ctrl+S on a draft
/// routes to `SaveAsDocument { doc, path: "" }`).
#[Command(default)]
pub struct SaveAsDocument {
    /// The document to persist.
    pub doc: DocumentId,
    /// Target path. Empty triggers the picker.
    pub path: String,
}

/// Request to remove the document from its registry (and any linked
/// runtime state — entities, caches).
///
/// Handled per-domain: the owning registry calls its remove-document
/// path, which fires [`DocumentClosed`]. Foreign domains ignore the
/// trigger. Idempotent — closing a non-existent or already-closed
/// document is a no-op.
#[Command(default)]
pub struct CloseDocument {
    /// The document to close.
    pub doc: DocumentId,
}

/// Create a new untitled document of the given kind.
///
/// `kind` is the registered `DocumentKindId` string (`"modelica"`,
/// `"julia"`, `"usd"`, …). An **empty** `kind` is the "use the
/// default" signal — the workbench-side observer looks up the registry,
/// picks the first kind whose `can_create_new` is true, and re-fires
/// this command with the resolved kind. That's how Ctrl+N reaches a
/// sensible default without the keybind owner having to know which
/// domain crates are loaded.
///
/// Domain crates add observers that gate on `cmd.kind == "<their_id>"`
/// and create the actual document. The workbench's default observer only
/// handles the empty-kind resolution.
///
/// Lives here (not in the egui workbench) so headless / sandbox / server
/// binaries can dispatch document creation by `kind` without the UI
/// shell — the picker-driven path is a workbench concern, the typed verb
/// is a document-lifecycle concern.
#[Command(default)]
pub struct NewDocument {
    /// Registered document kind id, or empty for "default".
    pub kind: String,
}

/// Open a file at `path` into a new tab.
///
/// Empty `path` triggers a native Open-File picker (a workbench concern)
/// and re-fires this command with the chosen path on success. A
/// **non-empty** `path` skips the dialog — that's how HTTP automation,
/// recents, drag-drop, and headless / server callers reach the same code
/// path without any UI.
///
/// The actual loading is domain-specific: `lunco-modelica` observes this
/// and reads `.mo` files; `lunco-usd` observes it for `.usd*`. Each
/// domain's observer ignores paths it doesn't own, so they coexist.
///
/// Lives here (not in the egui workbench) so headless / sandbox / server
/// binaries can open files by path; only the empty-path picker dispatch
/// stays in the workbench.
#[Command(default)]
pub struct OpenFile {
    /// Filesystem path or URI (`bundled://`, `mem://`). Empty triggers
    /// the picker (workbench only).
    pub path: String,
}

// ─────────────────────────────────────────────────────────────────────────────
// Intent layer — abstract user actions, rebindable, context-resolved
// ─────────────────────────────────────────────────────────────────────────────
//
// Keys ──[Keybindings]──▶ EditorIntent ──[resolver]──▶ concrete Command
//
// The intent layer exists so keyboard shortcuts (and future menu items,
// toolbar buttons, voice commands, accessibility tools) can be
// configured without code changes. A user rebinds `Ctrl+Z` to `Save`
// by editing `Keybindings`; they never see the concrete
// `UndoDocument { doc }` / `SaveDocument { doc }` commands.
//
// The resolver (in each domain crate) picks the active document and
// fires the matching concrete command. This keeps intents domain-
// agnostic — `EditorIntent::Undo` works whether the active doc is
// Modelica, USD, or something else. Domains install their own
// resolvers; the first one to recognise the active doc wins.

/// High-level user action, independent of which keys or buttons
/// triggered it and of which document it targets.
///
/// Fired by the input-to-intent system (keyboard, menus) and resolved
/// into one or more concrete commands by domain-specific observers.
#[derive(Event, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EditorIntent {
    /// Revert the most recent op on the active document.
    Undo,
    /// Re-apply the most recently undone op on the active document.
    Redo,
    /// Persist the active document to disk.
    Save,
    /// Persist the active document to a new path (picker dialog).
    /// Not implemented yet; the dispatch is here so keybindings can
    /// already reserve a chord (`Ctrl+Shift+S` by default).
    SaveAs,
    /// Remove the active document from its registry.
    Close,
    /// Compile and run the active document (domain-specific meaning —
    /// Modelica translates this to `CompileModel`; other domains may
    /// ignore or repurpose).
    Compile,
    /// Create a new untitled document and open a tab for it. Domain
    /// resolvers handle this without any active-doc ownership check —
    /// "new" by definition has no existing target.
    NewDocument,
}

/// A key chord (modifier + key) that triggers an [`EditorIntent`].
///
/// Built-in bindings (see [`Keybindings::default`]) cover the standard
/// text-editor / IDE set. Apps can replace [`Keybindings`] with a
/// user-configured map loaded from settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct KeyChord {
    /// The non-modifier key.
    pub key: KeyCode,
    /// Whether `Ctrl` (or `Cmd` on macOS) is held.
    pub ctrl: bool,
    /// Whether `Shift` is held.
    pub shift: bool,
    /// Whether `Alt` (or `Option` on macOS) is held.
    pub alt: bool,
}

impl KeyChord {
    /// Convenience constructor for `Ctrl+<key>`.
    pub const fn ctrl(key: KeyCode) -> Self {
        Self { key, ctrl: true, shift: false, alt: false }
    }
    /// Convenience constructor for `Ctrl+Shift+<key>`.
    pub const fn ctrl_shift(key: KeyCode) -> Self {
        Self { key, ctrl: true, shift: true, alt: false }
    }
    /// Convenience constructor for a bare (no-modifier) key.
    pub const fn plain(key: KeyCode) -> Self {
        Self { key, ctrl: false, shift: false, alt: false }
    }
}

/// Resource mapping [`KeyChord`]s to [`EditorIntent`]s.
///
/// The default bindings cover the common editor set; apps can replace
/// the resource wholesale (or mutate individual entries) to honor a
/// user config. Multiple chords can map to the same intent (e.g.
/// `Ctrl+Y` and `Ctrl+Shift+Z` both → [`EditorIntent::Redo`]).
#[derive(Resource, Clone, Debug)]
pub struct Keybindings {
    /// The binding table. Keys are chords, values are intents.
    pub map: HashMap<KeyChord, EditorIntent>,
}

impl Default for Keybindings {
    fn default() -> Self {
        let mut map = HashMap::new();
        map.insert(KeyChord::ctrl(KeyCode::KeyZ), EditorIntent::Undo);
        map.insert(KeyChord::ctrl_shift(KeyCode::KeyZ), EditorIntent::Redo);
        map.insert(KeyChord::ctrl(KeyCode::KeyY), EditorIntent::Redo);
        map.insert(KeyChord::ctrl(KeyCode::KeyS), EditorIntent::Save);
        map.insert(KeyChord::ctrl_shift(KeyCode::KeyS), EditorIntent::SaveAs);
        map.insert(KeyChord::ctrl(KeyCode::KeyW), EditorIntent::Close);
        map.insert(KeyChord::ctrl(KeyCode::KeyN), EditorIntent::NewDocument);
        map.insert(KeyChord::plain(KeyCode::F5), EditorIntent::Compile);
        Self { map }
    }
}

/// Which intents should *not* fire while an egui widget is claiming
/// keyboard input (typically: text field has focus and wants its
/// native undo/redo behavior). These intents defer; others (Save,
/// Compile) fire regardless because no text widget handles them.
fn intent_defers_to_text_widget(intent: EditorIntent) -> bool {
    matches!(intent, EditorIntent::Undo | EditorIntent::Redo)
}

/// Input-to-intent system: reads keyboard state, matches against
/// [`Keybindings`], and fires the [`EditorIntent`] via
/// [`Commands::trigger`].
///
/// Registered by [`EditorIntentPlugin`]. Domain crates install
/// resolvers (observers of [`EditorIntent`]) that fire concrete
/// document commands ([`UndoDocument`] etc.) for their owned docs.
#[cfg(feature = "ui")]
pub fn keyboard_to_intent(
    keys: Res<ButtonInput<KeyCode>>,
    keybindings: Res<Keybindings>,
    mut egui_ctx: bevy_egui::EguiContexts,
    mut commands: Commands,
) {
    let ctrl = keys.pressed(KeyCode::ControlLeft)
        || keys.pressed(KeyCode::ControlRight)
        || keys.pressed(KeyCode::SuperLeft)
        || keys.pressed(KeyCode::SuperRight);
    let shift = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
    let alt = keys.pressed(KeyCode::AltLeft) || keys.pressed(KeyCode::AltRight);

    let wants_keyboard = egui_ctx
        .ctx_mut()
        .ok()
        .map(|c| c.wants_keyboard_input())
        .unwrap_or(false);

    for (chord, intent) in keybindings.map.iter() {
        if !keys.just_pressed(chord.key) {
            continue;
        }
        if chord.ctrl != ctrl || chord.shift != shift || chord.alt != alt {
            continue;
        }
        if wants_keyboard && intent_defers_to_text_widget(*intent) {
            continue;
        }
        commands.trigger(*intent);
    }
}

/// Registers the [`Keybindings`] resource and the input-to-intent
/// system. Domain crates add their own intent resolvers on top.
pub struct EditorIntentPlugin;

impl Plugin for EditorIntentPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Keybindings>();
        // The keyboard→intent system reads egui focus to defer Undo/Redo to
        // text widgets; pure UI, so it's only present in `ui` builds.
        #[cfg(feature = "ui")]
        app.add_systems(Update, keyboard_to_intent);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// View sync — wake every panel on every document change
// ─────────────────────────────────────────────────────────────────────────────
//
// `egui_dock` only paints the focused tab per pane, so background tabs
// viewing the same doc don't observe gen bumps until clicked. There's no
// per-doc subscriber registry; instead we exploit the fact that any egui
// context with a queued repaint paints at least one more frame. One
// observer requests a repaint on every context whenever a document
// changes; panels' existing per-render gates then re-derive if stale.
// Coalesced within a frame, so the per-mutation cost stays at microseconds.

/// Registers the [`view_sync_fanout`] observer. Add once per app.
pub struct ViewSyncPlugin;

impl Plugin for ViewSyncPlugin {
    fn build(&self, _app: &mut App) {
        // Repaint fanout pokes egui contexts — UI-only.
        #[cfg(feature = "ui")]
        _app.add_observer(view_sync_fanout);
    }
}

#[cfg(feature = "ui")]
fn view_sync_fanout(
    _trigger: On<DocumentChanged>,
    mut egui_q: Query<&mut bevy_egui::EguiContext>,
) {
    for mut ctx in egui_q.iter_mut() {
        ctx.get_mut().request_repaint();
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Presence — placeholder for multi-user collaboration
// ─────────────────────────────────────────────────────────────────────────────
//
// Architectural seed: when the Twin grows into a server that hosts
// live collaboration (USD-Nucleus style), each connected client shows
// up here. Cursors, selections, and per-user color identity render
// from this resource. For the single-user app today it stays empty
// and panels that read it skip their collaboration UI.
//
// Zero code touches this today. Planting the type now so that when
// networking lands, the rest of the codebase doesn't need a
// cross-cutting refactor to start rendering presence.

/// Opaque user identifier. Small (`u64`) so it's cheap to copy
/// around in events. Mapped to a display name + visual identity in
/// [`Presence`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct UserId(pub u64);

impl UserId {
    /// Reserved id for "this client's local user" in a single-user
    /// session. Once authentication lands, real ids start at 1.
    pub const LOCAL: UserId = UserId(0);
}

/// Everything the UI needs to render a collaborator's presence —
/// display name, identifying color (for avatar chip, cursor tint),
/// and optional pointers into the workspace (which doc / where in
/// it they're looking at).
#[derive(Debug, Clone)]
pub struct PresenceInfo {
    /// Human-readable name shown in avatar chips / tooltips.
    pub display_name: String,
    /// RGB tint for this user's cursor, selection halo, edit
    /// indicators. Chosen by the server so clients agree.
    pub color: [u8; 3],
    /// Document the user is currently viewing, if any. `None` means
    /// the user is in the workspace root / no doc focused.
    pub active_doc: Option<DocumentId>,
    /// Normalized cursor position relative to screen [x, y], both in [0.0, 1.0].
    /// None means cursor is off-screen.
    pub cursor: Option<[f32; 2]>,
}

impl PresenceInfo {
    /// Convenience: `egui::Color32` form for the UI layer.
    ///
    /// Kept here (not on [`UserId`]) because the RGB triple lives in
    /// `PresenceInfo`. Callers that need the color can `.color32()`.
    pub fn color_rgba(&self) -> [u8; 4] {
        [self.color[0], self.color[1], self.color[2], 255]
    }
}

/// Global presence registry — who else is in this Twin session, and
/// where they are.
///
/// Single-user today: stays empty, panels that consult it render
/// nothing. Multi-user future: populated by the sync transport on
/// connect/disconnect events; panels iterate it to draw cursors,
/// avatar chips, "someone else is editing this doc" indicators.
#[derive(Resource, Default)]
pub struct Presence {
    /// Connected users keyed by id. Insertion order is not stable;
    /// consumers that need stable display order should sort by
    /// [`PresenceInfo::display_name`] or by [`UserId`].
    pub users: HashMap<UserId, PresenceInfo>,
}

impl Presence {
    /// Users that are currently viewing the given document.
    pub fn viewers_of(&self, doc: DocumentId) -> impl Iterator<Item = (&UserId, &PresenceInfo)> {
        self.users
            .iter()
            .filter(move |(_, info)| info.active_doc == Some(doc))
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// JournalResource — canonical op journal as a Bevy resource
// ─────────────────────────────────────────────────────────────────────────────
//
// The pure-Rust [`CanonicalJournal`] (in `lunco-twin-journal`) is the
// single source of truth for every recorded change in the active Twin —
// ops, raw text edits, snapshots, lifecycle events. This Bevy resource
// wraps it in `Arc<Mutex<_>>` so that:
//
// - **Domain mutation paths** (Modelica `apply_ops_public`, USD ops, …)
//   record entries from observer bodies via [`BevyJournalSink`] without
//   needing exclusive `World` access.
// - **Read-side panels** (`JournalLog`, history view, audit) read the
//   journal through `Res<JournalResource>` — locking is brief and only
//   while computing the displayed slice.
// - **Background tasks** (replay, sync, save) hold an `Arc` to the same
//   journal across thread boundaries.
//
// One Twin → one JournalResource. Multi-Twin (when it lands) becomes a
// `HashMap<TwinId, JournalResource>` resource; the per-Twin journal API
// stays unchanged.

/// The canonical op journal for the currently-active Twin, wrapped for
/// Bevy ECS access.
///
/// Cloning is cheap (`Arc` clone). Lock the inner journal briefly to
/// read or append; never hold the lock across a system boundary.
#[derive(Resource, Clone)]
pub struct JournalResource {
    inner: Arc<Mutex<CanonicalJournal>>,
}

impl JournalResource {
    /// Create a new resource around an empty journal scoped to one Twin.
    pub fn new(twin: TwinId, local_author: AuthorId) -> Self {
        Self {
            inner: Arc::new(Mutex::new(CanonicalJournal::new(twin, local_author))),
        }
    }

    /// Create a resource around an existing journal (used when the
    /// journal is constructed elsewhere — e.g. loaded from disk later).
    pub fn from_journal(journal: CanonicalJournal) -> Self {
        Self {
            inner: Arc::new(Mutex::new(journal)),
        }
    }

    /// Default-initialise with a `local-twin` Twin id and the local
    /// author. Sufficient for single-Twin sessions; multi-Twin replaces
    /// this with explicit construction.
    pub fn default_local() -> Self {
        Self::new(TwinId::new("local-twin"), AuthorId::local())
    }

    /// Run a closure with shared read access to the journal. Returns
    /// the closure's value. Panics if the lock is poisoned.
    pub fn with_read<R>(&self, f: impl FnOnce(&CanonicalJournal) -> R) -> R {
        let guard = self.inner.lock().expect("journal lock poisoned");
        f(&*guard)
    }

    /// Run a closure with exclusive write access to the journal.
    /// Panics if the lock is poisoned.
    pub fn with_write<R>(&self, f: impl FnOnce(&mut CanonicalJournal) -> R) -> R {
        let mut guard = self.inner.lock().expect("journal lock poisoned");
        f(&mut *guard)
    }

    /// Build a [`JournalSink`] handle that records into this resource.
    /// Cheap — just clones the inner `Arc`.
    pub fn sink(&self) -> BevyJournalSink {
        BevyJournalSink {
            inner: self.inner.clone(),
        }
    }

    /// Convenience: number of entries currently in the journal.
    pub fn len(&self) -> usize {
        self.with_read(|j| j.len())
    }

    /// Convenience: whether the journal has no entries yet.
    pub fn is_empty(&self) -> bool {
        self.with_read(|j| j.is_empty())
    }
}

impl Default for JournalResource {
    fn default() -> Self {
        Self::default_local()
    }
}

/// [`JournalSink`] implementation that pushes into a [`JournalResource`].
///
/// Created via [`JournalResource::sink`]. Domain mutation paths hold one
/// of these and call `sink.record(entry)` after a successful op apply.
#[derive(Clone)]
pub struct BevyJournalSink {
    inner: Arc<Mutex<CanonicalJournal>>,
}

impl JournalSink for BevyJournalSink {
    fn record(&self, entry: JournalEntry) {
        // Sink contract: this is the *remote-replay / forwarded entry*
        // path. Local mutation paths allocate EntryIds via the journal
        // itself (`record_op` / `record_text_edit` / etc. through
        // `JournalResource::with_write`), bypassing this sink.
        //
        // The trait exists so future remote-replay paths (entries
        // arriving from a peer with pre-allocated EntryIds) can push
        // through the same generic sink interface.
        let mut guard = self.inner.lock().expect("journal lock poisoned");
        guard.append_remote(entry);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Plugin
// ─────────────────────────────────────────────────────────────────────────────

/// Registers the canonical [`JournalResource`] (op log), the [`Presence`]
/// resource (collaboration seed), and the four lifecycle-event
/// subscribers that record `Lifecycle` entries into the canonical
/// journal.
///
/// Domain crates (lunco-modelica, lunco-usd, …) add this plugin once
/// per app. Events from any domain's registry flow into one canonical
/// journal.
///
/// **Note:** the previous lifecycle-summary `TwinJournal` Resource was
/// removed in the foundation consolidation — see `crates-index.md`.
/// The `JournalResource` is now the single source of truth; consumers
/// read lifecycle events via `journal.entries_for_doc(doc)` filtered on
/// `EntryKind::Lifecycle`.
pub struct TwinJournalPlugin;

impl Plugin for TwinJournalPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<JournalResource>()
            .init_resource::<Presence>()
            .add_observer(on_document_opened)
            .add_observer(on_document_saved)
            .add_observer(on_document_closed);
        // DocumentChanged is observed by the canonical journal indirectly:
        // structural `Op` entries are recorded by the domain mutation
        // path (e.g. lunco-modelica `apply_ops_public`) before this event
        // fires for view fan-out. No observer needed here.
    }
}

/// Translate the Bevy lifecycle [`EventOrigin`] into a journal
/// [`AuthorTag`]. Today: local user with the originating tool labeled.
/// Future: remote peers carry their `peer` id through `AuthorTag::user`.
fn author_for_origin(origin: &EventOrigin) -> AuthorTag {
    match origin {
        EventOrigin::Local => AuthorTag::local_user(),
        EventOrigin::Remote { peer } => AuthorTag {
            user: peer.clone(),
            tool: "remote".into(),
        },
        EventOrigin::Replay => AuthorTag {
            user: "local".into(),
            tool: "replay".into(),
        },
    }
}

fn on_document_opened(trigger: On<DocumentOpened>, canonical: Res<JournalResource>) {
    let ev = trigger.event();
    let author = author_for_origin(&ev.origin);
    canonical.with_write(|j| {
        j.record_lifecycle(
            author,
            ev.doc,
            LifecycleKind::Opened {
                source_hash: String::new(),
            },
        );
    });
}

fn on_document_saved(trigger: On<DocumentSaved>, canonical: Res<JournalResource>) {
    let ev = trigger.event();
    let author = author_for_origin(&ev.origin);
    canonical.with_write(|j| {
        j.record_lifecycle(author, ev.doc, LifecycleKind::Saved);
    });
}

fn on_document_closed(trigger: On<DocumentClosed>, canonical: Res<JournalResource>) {
    let ev = trigger.event();
    let author = author_for_origin(&ev.origin);
    canonical.with_write(|j| {
        j.record_lifecycle(author, ev.doc, LifecycleKind::Closed);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use lunco_twin_journal::{AuthorId, EntryKind, TwinId};

    #[test]
    fn journal_resource_records_lifecycle_entries() {
        let res = JournalResource::new(TwinId::new("test"), AuthorId::local());
        res.with_write(|j| {
            j.record_lifecycle(
                AuthorTag::local_user(),
                DocumentId::new(1),
                LifecycleKind::Saved,
            );
        });
        let count = res.with_read(|j| j.entries().count());
        assert_eq!(count, 1);
        let saved = res.with_read(|j| {
            j.entries_for_doc(DocumentId::new(1))
                .any(|e| matches!(e.kind, EntryKind::Lifecycle(LifecycleKind::Saved)))
        });
        assert!(saved);
    }
}
