//! # lunco-doc-bevy
//!
//! Bevy integration for the LunCoSim Document System.
//!
//! `lunco-doc` is pure data (zero deps, headless-testable). This crate
//! adds the ECS-facing half: generic document-lifecycle events that any
//! domain's registry (Modelica, USD, scripting, SysML, …) fires through,
//! plus the [`TwinJournal`] that subscribes to those events to give the
//! whole project one canonical change log.
//!
//! ## The architectural rule
//!
//! **Documents are mutated only through `#[Command]` observers.** Every
//! user intent — clicking Compile, typing in the editor, dropping a
//! component on the diagram, tweaking a parameter, invoking a remote
//! script — is a command whose observer applies ops to one or more
//! [`lunco_doc::Document`]s. The documents fire
//! [`DocumentChanged`] and its siblings, and every subscriber
//! (re-render, re-parse, telemetry refresh, plot-variable update,
//! replay, audit) reacts from those events.
//!
//! This rule is the nucleus: it means
//!
//! - **Undo/redo works everywhere for free** — per-document
//!   [`lunco_doc::DocumentHost`] stacks handle it.
//! - **Scripting / API / keyboard shortcuts share one path** — they
//!   all fire the same commands.
//! - **The Twin journal is a complete record** — nothing mutates a
//!   document without going through an event this crate already logs.
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
//! ## TwinJournal
//!
//! [`TwinJournal`] is a session-scoped, append-only log of document
//! events. It is *not* undo history — per-document undo still lives on
//! `DocumentHost`. The journal answers different questions: "what
//! happened this session, in what order, across all documents?" —
//! useful for replay, audit, debugging, and future cross-doc
//! transactions.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use std::collections::HashMap;
use std::time::Instant;

use bevy::prelude::*;
use lunco_doc::DocumentId;

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
/// Precedes any [`DocumentChanged`] for the same id that would otherwise
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
/// with [`DocumentChanged`] to maintain the save indicator.
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
/// [`DocumentChanged`], and performs whatever view-state sync the
/// domain requires (e.g. for Modelica, update the text buffer). Domains
/// that don't own `doc` ignore the trigger.
#[derive(Event, Clone, Debug)]
pub struct UndoDocument {
    /// The document whose most recent op should be undone.
    pub doc: DocumentId,
}

/// Request to redo the last undone op on the document.
///
/// Counterpart of [`UndoDocument`]. Same per-domain dispatch rules.
#[derive(Event, Clone, Debug)]
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
#[derive(Event, Clone, Debug)]
pub struct SaveDocument {
    /// The document to persist.
    pub doc: DocumentId,
}

/// Request the owning domain persist the document **to a user-picked
/// new location**. Fires a file-picker, writes the current source to
/// the chosen path, rebinds the document's [`DocumentOrigin`] to the
/// new writable `File` variant, updates `last_saved_generation`, and
/// fires [`DocumentSaved`] on success.
///
/// Differs from [`SaveDocument`] in two ways:
/// 1. Always opens the picker, even when the document already has a
///    canonical path — Save-As intentionally overrides the bound path.
/// 2. Handles [`DocumentOrigin::Untitled`] — the picker's chosen path
///    promotes the doc to a writable File origin.
///
/// Cancelling the picker is a no-op (no error toast, no save event).
#[derive(Event, Clone, Debug)]
pub struct SaveAsDocument {
    /// The document to persist.
    pub doc: DocumentId,
}

/// Request to remove the document from its registry (and any linked
/// runtime state — entities, caches).
///
/// Handled per-domain: the owning registry calls its remove-document
/// path, which fires [`DocumentClosed`]. Foreign domains ignore the
/// trigger. Idempotent — closing a non-existent or already-closed
/// document is a no-op.
#[derive(Event, Clone, Debug)]
pub struct CloseDocument {
    /// The document to close.
    pub doc: DocumentId,
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
        app.init_resource::<Keybindings>()
            .add_systems(Update, keyboard_to_intent);
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
// TwinJournal
// ─────────────────────────────────────────────────────────────────────────────

/// One entry in the [`TwinJournal`].
///
/// Journal entries intentionally do *not* carry op payloads — ops live
/// on their `DocumentHost`'s undo stack. The journal is a *summary*
/// timeline: "at time `t`, event `kind` happened on document `doc`,
/// attributed to `origin`." Consumers that need the full op walk the
/// host.
#[derive(Clone, Debug)]
pub struct TwinEvent {
    /// Wall-clock time the event was appended (session-local).
    pub at: Instant,
    /// Who / what produced the event (`Local` user, `Remote` peer,
    /// `Replay` harness). Audit and filter-by-source both use this.
    pub origin: EventOrigin,
    /// What happened.
    pub kind: TwinEventKind,
}

/// Discriminant for [`TwinEvent`].
///
/// Kept flat (not a wrapped Bevy event) so consumers can serialize,
/// diff, and persist the journal without pulling in Bevy's event types.
/// When a future transactional / multi-document op lands, add a
/// `Transaction { name, events: Vec<TwinEventKind> }` variant here.
#[derive(Clone, Debug)]
pub enum TwinEventKind {
    /// A document was added to its registry.
    Opened {
        /// The id of the newly-opened document.
        doc: DocumentId,
    },
    /// A document had an op applied (source advanced).
    Changed {
        /// The id of the changed document.
        doc: DocumentId,
    },
    /// A document was saved to disk.
    Saved {
        /// The id of the saved document.
        doc: DocumentId,
    },
    /// A document was removed from its registry.
    Closed {
        /// The id of the closed document.
        doc: DocumentId,
    },
}

/// Twin-level, append-only, session-scoped change log.
///
/// **Not user-facing undo.** Per-document undo stays on
/// [`lunco_doc::DocumentHost`]. This resource records every document
/// lifecycle event for replay, audit, diagnostics, and cross-document
/// introspection.
///
/// The journal is unbounded today. When a session's journal outgrows
/// comfortable memory, rotation (to disk, to a ring buffer) will land
/// here; the public shape of reads won't change.
#[derive(Resource, Default)]
pub struct TwinJournal {
    entries: Vec<TwinEvent>,
}

impl TwinJournal {
    /// Append an event with the default [`EventOrigin::Local`].
    /// Convenience for tests / ad-hoc seeding; observer-path uses
    /// [`push_with_origin`](Self::push_with_origin) so remote-sourced
    /// events retain their provenance.
    pub fn push(&mut self, kind: TwinEventKind) {
        self.push_with_origin(kind, EventOrigin::Local);
    }

    /// Append an event with an explicit origin.
    pub fn push_with_origin(&mut self, kind: TwinEventKind, origin: EventOrigin) {
        self.entries.push(TwinEvent {
            at: Instant::now(),
            origin,
            kind,
        });
    }

    /// Read all entries in chronological order.
    pub fn entries(&self) -> &[TwinEvent] {
        &self.entries
    }

    /// Total entry count.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the journal is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Last `n` entries, newest last. Convenience for UI / debug.
    pub fn tail(&self, n: usize) -> &[TwinEvent] {
        let start = self.entries.len().saturating_sub(n);
        &self.entries[start..]
    }

    /// All entries for a given document, newest last.
    pub fn filter_by_doc(&self, doc: DocumentId) -> impl Iterator<Item = &TwinEvent> {
        self.entries.iter().filter(move |e| event_doc(&e.kind) == doc)
    }
}

fn event_doc(kind: &TwinEventKind) -> DocumentId {
    match kind {
        TwinEventKind::Opened { doc }
        | TwinEventKind::Changed { doc }
        | TwinEventKind::Saved { doc }
        | TwinEventKind::Closed { doc } => *doc,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Plugin
// ─────────────────────────────────────────────────────────────────────────────

/// Registers the [`TwinJournal`] resource and the four lifecycle-event
/// subscribers that append to it.
///
/// Domain crates (lunco-modelica, lunco-usd, …) add this plugin once
/// per app. Events from any domain's registry flow into one journal.
pub struct TwinJournalPlugin;

impl Plugin for TwinJournalPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TwinJournal>()
            .init_resource::<Presence>()
            .add_observer(on_document_opened)
            .add_observer(on_document_changed)
            .add_observer(on_document_saved)
            .add_observer(on_document_closed);
    }
}

fn on_document_opened(trigger: On<DocumentOpened>, mut journal: ResMut<TwinJournal>) {
    let ev = trigger.event();
    journal.push_with_origin(TwinEventKind::Opened { doc: ev.doc }, ev.origin.clone());
}

fn on_document_changed(trigger: On<DocumentChanged>, mut journal: ResMut<TwinJournal>) {
    let ev = trigger.event();
    journal.push_with_origin(TwinEventKind::Changed { doc: ev.doc }, ev.origin.clone());
}

fn on_document_saved(trigger: On<DocumentSaved>, mut journal: ResMut<TwinJournal>) {
    let ev = trigger.event();
    journal.push_with_origin(TwinEventKind::Saved { doc: ev.doc }, ev.origin.clone());
}

fn on_document_closed(trigger: On<DocumentClosed>, mut journal: ResMut<TwinJournal>) {
    let ev = trigger.event();
    journal.push_with_origin(TwinEventKind::Closed { doc: ev.doc }, ev.origin.clone());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn journal_push_and_tail() {
        let mut j = TwinJournal::default();
        let d1 = DocumentId::new(1);
        let d2 = DocumentId::new(2);
        j.push(TwinEventKind::Opened { doc: d1 });
        j.push(TwinEventKind::Changed { doc: d1 });
        j.push(TwinEventKind::Opened { doc: d2 });
        assert_eq!(j.len(), 3);
        assert_eq!(j.tail(2).len(), 2);
        assert_eq!(j.filter_by_doc(d1).count(), 2);
        assert_eq!(j.filter_by_doc(d2).count(), 1);
    }
}
