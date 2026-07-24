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
        Self {
            doc,
            origin: EventOrigin::Local,
        }
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
        Self {
            doc,
            origin: EventOrigin::Local,
        }
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
        Self {
            doc,
            origin: EventOrigin::Local,
        }
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
        Self {
            doc,
            origin: EventOrigin::Local,
        }
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
    /// Open the app's guided tutorial / product tour (default: `F1`). Like
    /// [`NewDocument`](Self::NewDocument), it targets no document — a
    /// domain resolver (e.g. lunica's tutorial launcher) turns it into a
    /// concrete launch command. Apps with no tutorial simply ignore it.
    ShowTutorial,
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
        Self {
            key,
            ctrl: true,
            shift: false,
            alt: false,
        }
    }
    /// Convenience constructor for `Ctrl+Shift+<key>`.
    pub const fn ctrl_shift(key: KeyCode) -> Self {
        Self {
            key,
            ctrl: true,
            shift: true,
            alt: false,
        }
    }
    /// Convenience constructor for a bare (no-modifier) key.
    pub const fn plain(key: KeyCode) -> Self {
        Self {
            key,
            ctrl: false,
            shift: false,
            alt: false,
        }
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
        map.insert(KeyChord::plain(KeyCode::F1), EditorIntent::ShowTutorial);
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
        .map(|c| c.egui_wants_keyboard_input())
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
fn view_sync_fanout(_trigger: On<DocumentChanged>, mut egui_q: Query<&mut bevy_egui::EguiContext>) {
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
    /// the closure's value.
    ///
    /// Poison recovery: a panic in *any* journal writer would otherwise poison
    /// this mutex forever, turning one glitch into a per-frame panic loop that
    /// nothing can clear. The journal is an append-only log of already-applied
    /// ops — a mid-write panic leaves no broken invariant behind (a partially
    /// pushed entry is not possible; `Vec::push` is the last step), so the
    /// guard is recovered with `into_inner` rather than re-panicked.
    pub fn with_read<R>(&self, f: impl FnOnce(&CanonicalJournal) -> R) -> R {
        let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        f(&guard)
    }

    /// Run a closure with exclusive write access to the journal.
    /// Poison-recovering — see [`with_read`](Self::with_read).
    pub fn with_write<R>(&self, f: impl FnOnce(&mut CanonicalJournal) -> R) -> R {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        f(&mut guard)
    }

    /// The author id stamped onto locally-recorded entries. Placeholder
    /// (`AuthorId::local()`) until a networked peer stamps its identity via
    /// [`set_local_author`](Self::set_local_author).
    pub fn local_author(&self) -> AuthorId {
        self.with_read(|j| j.local_author().clone())
    }

    /// Set the peer-unique author id for future local entries — see
    /// [`CanonicalJournal::set_local_author`]. Each peer (host + each client)
    /// must set a distinct id so cross-peer entry ids don't collide.
    pub fn set_local_author(&self, author: AuthorId) {
        self.with_write(|j| j.set_local_author(author));
    }

    /// Run `f` with a change set open: every journal entry recorded inside it —
    /// by ANY recorder, at any depth, since they all append with
    /// `change_set: None` and inherit the ambient one — joins a single
    /// transaction-style undo unit, which `UndoManager::take_undo_group` then
    /// undoes as a whole.
    ///
    /// This is the seam a multi-op command handler wraps itself in.
    /// `AttachComponent` lowers to seven `UsdOp`s; without this, seven journal
    /// entries land and one undo peels off ONE of them, leaving the object
    /// half-attached. With it, they are one unit.
    ///
    /// The change set is closed even if `f` returns early. It is NOT closed on a
    /// panic unwinding through `f` — but a panic there wedges the command
    /// anyway, and the next `begin` simply replaces the ambient id, so a stale
    /// open set cannot corrupt later entries beyond over-grouping one command.
    /// Do not nest (see [`Journal::begin_change_set`]).
    pub fn change_set<R>(&self, label: impl Into<String>, f: impl FnOnce() -> R) -> R {
        let author = AuthorTag::local_user();
        self.with_write(|j| j.begin_change_set(label.into(), author));
        let out = f();
        self.with_write(|j| j.end_change_set());
        out
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
        // Poison-recovering: see `JournalResource::with_read`.
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.append_remote(entry);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// JournalOpRecorder — auto-bridge DocumentHost edits into the journal
// ─────────────────────────────────────────────────────────────────────────────

/// [`lunco_doc::OpRecorder`] that mirrors every apply / undo / redo on a
/// [`lunco_doc::DocumentHost`] into a [`JournalResource`], losslessly via
/// [`record_op`](CanonicalJournal::record_op).
///
/// This is the **A3 auto-bridge**: installed once per host by
/// [`attach_journal_recorder`], it removes all per-op recording code from the
/// domains — and, crucially, journals **undo/redo** too, which previously
/// bypassed every domain's record path entirely.
pub struct JournalOpRecorder {
    journal: JournalResource,
    doc: DocumentId,
    /// Provenance for the *next* edit, set one-shot by the domain apply
    /// funnel via [`set_next_author`](lunco_doc::OpRecorder::set_next_author)
    /// and consumed by [`record`](#impl-OpRecorder). `None` ⇒ attribute to
    /// the local user. Interior-mutable because `OpRecorder` records through
    /// `&self`; ops apply sequentially on the ECS thread, so the brief lock
    /// is uncontended.
    next_author: Mutex<Option<AuthorTag>>,
}

impl JournalOpRecorder {
    /// Record into `journal` under document `doc`. Each edit is attributed
    /// to the local user unless the apply funnel set a one-shot author via
    /// [`set_next_author`](lunco_doc::OpRecorder::set_next_author) first.
    pub fn new(journal: JournalResource, doc: DocumentId) -> Self {
        Self {
            journal,
            doc,
            next_author: Mutex::new(None),
        }
    }
}

impl<O: lunco_twin_journal::OpPayload> lunco_doc::OpRecorder<O> for JournalOpRecorder {
    fn record(&self, forward: &O, inverse: &O) {
        // Consume the one-shot author; absent one, the edit is the local
        // user's (manual edits, undo/redo).
        // Poison-recovering: an `Option<AuthorTag>` has no invariant a mid-write
        // panic can break, and re-panicking here would wedge every subsequent
        // edit, every frame. See `JournalResource::with_read`.
        let author = self
            .next_author
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
            .unwrap_or_else(AuthorTag::local_user);
        self.journal.with_write(|j| {
            if let Err(err) = j.record_op(author, self.doc, forward, inverse, None) {
                warn!("[journal] record_op failed for doc {:?}: {err}", self.doc);
            }
        });
    }

    fn set_next_author(&self, user: &str, tool: &str) {
        *self.next_author.lock().unwrap_or_else(|e| e.into_inner()) = Some(AuthorTag {
            user: user.to_string(),
            tool: tool.to_string(),
        });
    }
}

/// Install a [`JournalOpRecorder`] on `host` so its future edits journal
/// automatically. Generic over the domain — the host supplies its own
/// [`DocumentId`], so one helper serves USD, Modelica, and any future domain.
/// Call once per host (guard with [`lunco_doc::DocumentHost::has_recorder`]).
pub fn attach_journal_recorder<D>(host: &mut lunco_doc::DocumentHost<D>, journal: &JournalResource)
where
    D: lunco_doc::Document,
    D::Op: lunco_twin_journal::OpPayload,
{
    let doc = host.document().id();
    host.set_recorder(Arc::new(JournalOpRecorder::new(journal.clone(), doc)));
}

// ─────────────────────────────────────────────────────────────────────────────
// DocumentRegistry — one registry for every domain
// ─────────────────────────────────────────────────────────────────────────────

/// Pending lifecycle events drained from a [`DocumentRegistry`].
pub struct PendingEvents {
    /// Docs newly added since the last drain.
    pub opened: Vec<DocumentId>,
    /// Docs whose generation advanced since the last drain.
    pub changed: Vec<DocumentId>,
    /// Docs removed since the last drain.
    pub closed: Vec<DocumentId>,
}

/// Every live document of one domain, keyed by [`DocumentId`] — **and the one
/// place that knows a file-backed document's identity is its path.**
///
/// This was hand-copied per domain (`DocumentRegistry<UsdDocument>`, `ModelicaDocument
/// Registry`, `ScriptRegistry`): same `hosts` map, same `next_doc_id`, same
/// pending rings, same journal wiring — and each hand-rolled (or omitted) the
/// open-by-path rule, so each broke differently:
///
/// * **USD** deduped by path but never refreshed the content ⇒ re-opening an
///   edited `.usda` replayed the OLD scene until the app was restarted.
/// * **Modelica** never deduped at all ⇒ opening one `.mo` twice minted TWO
///   documents, two tabs, two undo stacks, both saving over each other.
///
/// One missing concept, two opposite bugs. [`open_file`](Self::open_file) is
/// that concept, written once: **identity is reused, content is not.**
///
/// WHY IT LIVES HERE, not in `lunco-doc`:
/// * `lunco-twin-journal` depends on `lunco-doc`, so `lunco-doc` reaching the
///   journal is a dependency CYCLE. (`JournalResource` is in this crate anyway.)
/// * It must be a Bevy `Resource` so each domain can alias it
///   (`type DocumentRegistry<UsdDocument> = DocumentRegistry<UsdDocument>`) and its call
///   sites keep working; the orphan rule blocks `impl Resource` for a foreign
///   type from here.
/// * `lunco-doc`'s [`OpRecorder`](lunco_doc::OpRecorder) contract already says
///   the concrete recorder "lives in the ECS layer" — this crate IS that layer.
///   The per-domain registries each wired the journal themselves; this hoists
///   that up one level rather than inventing a new seam.
///
/// (Not because `lunco-doc` is dep-free — it isn't. Its own manifest notes it
/// pulls bevy transitively through `lunco-core`, "a regression on the original
/// headless data model stance".)
#[derive(Resource)]
pub struct DocumentRegistry<D: lunco_doc::Document> {
    hosts: HashMap<DocumentId, lunco_doc::DocumentHost<D>>,
    /// Twin-journal handle, wired once the [`JournalResource`] appears. When
    /// set, every host gets a [`JournalOpRecorder`] so edits — including undo /
    /// redo — auto-record. `None` in headless-without-journal builds.
    journal: Option<JournalResource>,
    next_doc_id: u64,
    pending_opened: Vec<DocumentId>,
    pending_changes: Vec<DocumentId>,
    pending_closed: Vec<DocumentId>,
    /// Per-document disk watermarks: for each file this document depends on, the
    /// modification time when we last read or wrote it.
    /// [`stale_docs`](Self::stale_docs) compares against current mtimes to spot a
    /// change made *behind the app's back* — a git pull, an external editor.
    ///
    /// A document's own file is just one entry; [`watch_files`](Self::watch_files)
    /// lets a domain add the rest of its dependency closure (a USD scene's
    /// referenced layers, a DEM). The registry stays domain-agnostic — it never
    /// computes a closure, it only watches what it is handed — so every document
    /// type inherits the mechanism.
    ///
    /// Detection is deliberately split from policy: this notices, the caller
    /// decides. Per the collaboration doc, an external change while a sim runs
    /// must **badge, never auto-reload** — a silent reload restarts the world.
    watermarks: HashMap<DocumentId, HashMap<std::path::PathBuf, std::time::SystemTime>>,
}

impl<D: lunco_doc::Document> Default for DocumentRegistry<D> {
    fn default() -> Self {
        Self {
            hosts: HashMap::new(),
            journal: None,
            next_doc_id: 0,
            pending_opened: Vec::new(),
            pending_changes: Vec::new(),
            pending_closed: Vec::new(),
            watermarks: HashMap::new(),
        }
    }
}

impl<D: lunco_doc::Document> DocumentRegistry<D>
where
    D::Op: lunco_twin_journal::OpPayload,
{
    /// Install `doc` under a fresh id, wire its recorder, and queue its
    /// lifecycle events. The low-level path — see
    /// [`open_file`](Self::open_file) for anything with a filesystem path.
    fn install(&mut self, make: impl FnOnce(DocumentId) -> D) -> DocumentId {
        self.next_doc_id = self.next_doc_id.saturating_add(1);
        let id = DocumentId::new(self.next_doc_id);
        self.hosts
            .insert(id, lunco_doc::DocumentHost::new(make(id)));
        // Fit the journal recorder at creation so the very first edit is
        // journaled. No-op until `set_journal` retro-fits.
        self.attach_recorder(id);
        // One Opened (lifecycle) + one Changed (initial-source seed) so a
        // subscriber that only listens to changes still sees the initial source.
        self.pending_opened.push(id);
        self.pending_changes.push(id);
        id
    }

    /// Wire the Twin-journal handle and retro-fit a recorder onto every existing
    /// host. Called once, reactively, the frame the [`JournalResource`] appears.
    pub fn set_journal(&mut self, journal: JournalResource) {
        self.journal = Some(journal);
        let ids: Vec<_> = self.hosts.keys().copied().collect();
        for id in ids {
            self.attach_recorder(id);
        }
    }

    fn attach_recorder(&mut self, id: DocumentId) {
        if let Some(journal) = &self.journal {
            if let Some(host) = self.hosts.get_mut(&id) {
                if !host.has_recorder() {
                    attach_journal_recorder(host, journal);
                }
            }
        }
    }

    /// Borrow the host for `doc`, or `None` if unknown.
    pub fn host(&self, doc: DocumentId) -> Option<&lunco_doc::DocumentHost<D>> {
        self.hosts.get(&doc)
    }

    /// Mutably borrow the host for `doc`. Direct mutations through this handle
    /// MUST be paired with [`mark_changed`](Self::mark_changed) — the registry
    /// can't see arbitrary uses of `&mut DocumentHost`.
    pub fn host_mut(&mut self, doc: DocumentId) -> Option<&mut lunco_doc::DocumentHost<D>> {
        self.hosts.get_mut(&doc)
    }

    /// True iff `doc` is a document we own.
    pub fn contains(&self, doc: DocumentId) -> bool {
        self.hosts.contains_key(&doc)
    }

    /// Every live document id.
    pub fn ids(&self) -> impl Iterator<Item = DocumentId> + '_ {
        self.hosts.keys().copied()
    }

    /// Queue a `Changed` event for `doc` after a direct `host_mut` mutation.
    pub fn mark_changed(&mut self, doc: DocumentId) {
        self.pending_changes.push(doc);
    }

    /// Apply an op via the host and queue a Changed notification. Convenience
    /// wrapper so callers don't have to remember [`mark_changed`](Self::mark_changed).
    pub fn apply(
        &mut self,
        doc: DocumentId,
        op: D::Op,
    ) -> Result<lunco_doc::Ack, lunco_doc::Reject> {
        let host = self
            .hosts
            .get_mut(&doc)
            .ok_or_else(|| lunco_doc::Reject::InvalidOp(format!("unknown doc {doc}")))?;
        let ack = host.apply(lunco_doc::Mutation::local(op))?;
        self.pending_changes.push(doc);
        Ok(ack)
    }

    /// Drop `doc` and queue its `Closed` event.
    pub fn remove(&mut self, doc: DocumentId) -> Option<lunco_doc::DocumentHost<D>> {
        let host = self.hosts.remove(&doc)?;
        self.pending_closed.push(doc);
        Some(host)
    }

    /// Drain the pending-events rings.
    pub fn drain_pending(&mut self) -> PendingEvents {
        PendingEvents {
            opened: std::mem::take(&mut self.pending_opened),
            changed: std::mem::take(&mut self.pending_changes),
            closed: std::mem::take(&mut self.pending_closed),
        }
    }
}

impl<D: lunco_doc::Document> DocumentRegistry<D>
where
    D::Op: lunco_twin_journal::OpPayload + serde::de::DeserializeOwned,
{
    /// Apply a **journal op** to `doc` for replay (journal→scene projection, the
    /// networked-edit consume path) **without recording it**. The op arrived via
    /// `Journal::append_remote` and is already in the journal, so re-recording it
    /// (as [`apply`](Self::apply) would, via the host's [`JournalOpRecorder`])
    /// would mint a duplicate local entry. This bypasses the recorder by applying
    /// straight to the document, then marks `doc` changed so views re-project.
    ///
    /// **This is the multi-user consume path**, and it is generic on purpose: any
    /// domain whose `Op` is deserializable replays remote edits through it. How
    /// well that behaves is decided entirely by the domain's op ADDRESSING —
    /// path/name-addressed ops (`/World/Rover.translate`, `AddComponent{class}`)
    /// merge per-property the way Omniverse's `.live` layer does; byte-offset ops
    /// (`EditText{range}`) break the moment the base moves; whole-file ops
    /// (`ScriptOp::SetSource`) mean the last writer silently erases the other.
    ///
    /// Returns `false` (logged, non-fatal) if the doc is unknown, the payload
    /// doesn't parse as this domain's op, or the apply is rejected (e.g. AddPrim
    /// of an existing prim when replaying already-reflected history — harmless).
    pub fn replay_op(&mut self, doc: DocumentId, op: &serde_json::Value) -> bool {
        let parsed = match serde_json::from_value::<D::Op>(op.clone()) {
            Ok(op) => op,
            Err(e) => {
                warn!("[doc-replay] op payload does not parse for this domain: {e}");
                return false;
            }
        };
        let Some(host) = self.hosts.get_mut(&doc) else {
            return false;
        };
        match host.document_mut().apply(parsed) {
            Ok(_) => {
                self.pending_changes.push(doc);
                true
            }
            Err(e) => {
                warn!("[doc-replay] apply rejected on doc {doc}: {e:?}");
                false
            }
        }
    }
}

impl<D: lunco_doc::FileBacked> DocumentRegistry<D>
where
    D::Op: lunco_twin_journal::OpPayload,
{
    /// Allocate a new document that is **not** backed by a file: File→New
    /// (untitled) or a bundled example.
    ///
    /// It takes [`PathlessOrigin`](lunco_doc::PathlessOrigin), not
    /// `DocumentOrigin`, so it *cannot* be handed a path. **A file-backed open
    /// goes through [`open_file`](Self::open_file)**, which enforces
    /// one-document-per-path. This used to accept any origin, and a `File`
    /// origin for an already-open path minted a SECOND document for one file:
    /// two tabs, two undo stacks, two journal streams, racing saves. The
    /// signature now carries that rule instead of a doc comment asking nicely.
    ///
    /// Session restore reinstates a stored `File` origin verbatim and uses
    /// [`restore`](Self::restore).
    pub fn allocate(&mut self, source: String, origin: lunco_doc::PathlessOrigin) -> DocumentId {
        self.install(|id| D::with_origin(id, source, origin.into()))
    }

    /// Reinstate a document from persisted session state, origin and all.
    ///
    /// **Session restore only.** This is the one caller that legitimately
    /// reinstates a `File` origin without re-reading disk: it restores saved
    /// in-memory state, which may be dirty, and discarding that is exactly what
    /// restore exists to prevent. Everything else opening a file wants
    /// [`open_file`](Self::open_file).
    ///
    /// Restoring a path that is already open mints a second document for it —
    /// the split-brain `allocate` was locked down to prevent. Restore runs once
    /// against an empty registry, so it doesn't check; don't call it elsewhere.
    pub fn restore(&mut self, source: String, origin: lunco_doc::DocumentOrigin) -> DocumentId {
        self.install(|id| D::with_origin(id, source, origin))
    }

    /// The document backing `path`, if that file is open. **The path IS the
    /// identity** of a file-backed document.
    pub fn doc_for_file(&self, path: &std::path::Path) -> Option<DocumentId> {
        self.ids().find(|id| {
            self.host(*id)
                .and_then(|h| match h.document().origin() {
                    lunco_doc::DocumentOrigin::File { path: p, .. } => {
                        Some(lunco_doc::same_file(p, path))
                    }
                    _ => None,
                })
                .unwrap_or(false)
        })
    }

    /// Open `path` backed by `source` (its current on-disk text), returning the
    /// document for that file and what had to happen to get there.
    ///
    /// **Use this for every file-backed open, in every domain.** Identity and
    /// content are two decisions, and the codebase used to make the second one
    /// by accident: both USD's and Modelica's open paths were shaped
    /// `if !already_open { allocate(source) }`, so a freshly-read `source` was
    /// silently dropped when the file was already open (USD), or a duplicate
    /// document was minted because nobody checked at all (Modelica). Here it's a
    /// typed [`OpenOutcome`], not a fallthrough.
    ///
    /// `source` is a PARAMETER, so the registry never reads or caches a file:
    /// the caller decides where bytes come from (local disk, or a client's
    /// replicated bytes). "Cache only on the client" holds by construction.
    ///
    /// The registry deliberately keeps NO path→id index: `document_mut()` is
    /// public and Save-As rebinds origins behind the registry's back, so a
    /// cached index would silently rot — the exact bug class this kills. Origins
    /// ARE the truth; we scan them. O(open documents), off the hot path.
    pub fn open_file(
        &mut self,
        path: impl Into<std::path::PathBuf>,
        source: String,
    ) -> (DocumentId, lunco_doc::OpenOutcome) {
        use lunco_doc::OpenOutcome;
        let path = path.into();
        let Some(id) = self.doc_for_file(&path) else {
            // Straight to `install`: this is the ONE authorized way to mint a
            // file-backed document, and it earns that by having just proved the
            // path isn't open. `allocate` can no longer express a `File` origin.
            let id = self.install(|id| {
                D::with_origin(id, source, lunco_doc::DocumentOrigin::writable_file(path))
            });
            // Baseline the watermark off the origin we just wrote — one stamping
            // path for open, reload, and save. The domain adds its dependency
            // closure with its own `watch_files` call once it has parsed.
            self.watch_files(id, []);
            return (id, OpenOutcome::Allocated);
        };
        let Some(host) = self.hosts.get_mut(&id) else {
            unreachable!("doc_for_file returned an id with no host");
        };
        if host.document().is_dirty() {
            // We did NOT take disk content, so the watermark stays put: the
            // document is still ahead of (or diverged from) disk, and the caller
            // wants to know the file moved, not be told we're in sync with it.
            return (id, OpenOutcome::KeptDirty);
        }
        if !host.document_mut().reload_base(&source) {
            return (id, OpenOutcome::KeptUnparsable);
        }
        // Fresh disk content landed — re-baseline. Drops any previously-watched
        // dependency: the new content's closure may differ, and the domain
        // re-registers it after parsing.
        self.watch_files(id, []);
        // The content moved — same ring the mutating ops feed, so views rebuild
        // off an open exactly as they would off an edit.
        self.pending_changes.push(id);
        (id, OpenOutcome::Refreshed)
    }

    /// Watch `id`'s **dependency closure** in addition to its own file: the
    /// layers a USD scene references, a DEM it points at, anything whose change
    /// makes the document stale.
    ///
    /// The registry never computes a closure — that is domain knowledge (USD
    /// arcs, a Modelica `import`). The domain walks its own graph and hands the
    /// paths here, which is what keeps this mechanism usable by every document
    /// type. Call it after each open/reload, since the closure moves with the
    /// content.
    ///
    /// Replaces the watched set (own file always included), so a reference the
    /// user deleted stops being watched instead of pinning a phantom dependency.
    pub fn watch_files(
        &mut self,
        id: DocumentId,
        deps: impl IntoIterator<Item = std::path::PathBuf>,
    ) {
        let own = self.hosts.get(&id).and_then(|h| {
            h.document()
                .origin()
                .canonical_path()
                .map(std::path::Path::to_path_buf)
        });
        let stamped = own
            .into_iter()
            .chain(deps)
            .filter_map(|p| file_mtime(&p).map(|m| (p, m)))
            .collect();
        self.watermarks.insert(id, stamped);
    }

    /// Re-stamp every file `id` watches to its current mtime. Call after a
    /// successful **save**: those bytes are now ours, so a later
    /// [`stale_docs`](Self::stale_docs) must not flag our own write as an
    /// external change.
    pub fn note_saved(&mut self, id: DocumentId) {
        let Some(watched) = self.watermarks.get(&id) else {
            return;
        };
        let restamped = watched
            .keys()
            .filter_map(|p| file_mtime(p).map(|m| (p.clone(), m)))
            .collect();
        self.watermarks.insert(id, restamped);
    }

    /// Documents with a dependency that changed on disk since we last read or
    /// wrote it — a git pull, an external editor, another tool. A document is
    /// stale if **any** file it watches moved, its own or a referenced one.
    ///
    /// Detection only: the caller decides what to do. Per the collaboration
    /// doc, surface it (badge / status line) and **never auto-reload** while a
    /// sim is running — that would restart the world. A vanished file is not
    /// reported stale: deletion is a different event, and stat-failure must not
    /// masquerade as "changed".
    pub fn stale_docs(&self) -> Vec<DocumentId> {
        self.watermarks
            .iter()
            .filter(|(_, watched)| {
                watched
                    .iter()
                    .any(|(path, mark)| file_mtime(path).is_some_and(|now| now > *mark))
            })
            .map(|(id, _)| *id)
            .collect()
    }
}

/// Modification time of `path`, or `None` if it can't be stat'd (missing,
/// permissions, a platform without mtime). Callers treat `None` as "don't
/// watch" — never as a sentinel that compares as changed.
fn file_mtime(path: &std::path::Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

// ─────────────────────────────────────────────────────────────────────────────
// Plugin
// ─────────────────────────────────────────────────────────────────────────────

/// Registers the canonical [`JournalResource`] (op log), the [`Presence`]
/// resource (collaboration seed), the lifecycle-event subscribers that
/// record `Lifecycle` entries into the canonical journal, and the
/// close-time [`DocumentDiagnostics`] cleanup shared by every document
/// domain.
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
            .add_observer(on_document_closed)
            .add_observer(diagnostics::drop_diagnostics_on_close);
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
