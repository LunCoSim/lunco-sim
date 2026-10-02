//! Bevy ECS binding for [`Workspace`].
//!
//! The [`Workspace`] type itself is pure data; this module ships the thin
//! ECS bridge every consumer needs: a `Resource` newtype plus add/close
//! events so observers can react to session changes without polling the
//! whole Workspace each frame. It uses only the bevy ECS/app substrate
//! (`Resource`, `Event`, `Plugin`, observers) — no render/winit/egui — so a
//! windowed editor and a `--no-ui` server install the exact same binding.
//!
//! Recents *persistence* (which needs the on-disk config dir) is NOT here —
//! it lives in the consumer that owns config-dir resolution (the workbench),
//! keeping this crate free of asset/config dependencies.

use bevy::prelude::*;
use std::ops::{Deref, DerefMut};

use crate::{DocumentEntry, TwinId, Workspace};
use lunco_doc::DocumentId;

// ─────────────────────────────────────────────────────────────────────────────
// Resource
// ─────────────────────────────────────────────────────────────────────────────

/// Bevy-side wrapper around [`Workspace`].
///
/// Newtyped rather than using `Workspace` directly as a `Resource` so we can
/// add ECS-side invariants (active-tab derivation, change-detection filters)
/// here without touching the core data type.
///
/// Access via `Res<WorkspaceResource>` / `ResMut<WorkspaceResource>`;
/// `Deref` / `DerefMut` expose the full `Workspace` API.
#[derive(Resource, Default, Debug)]
pub struct WorkspaceResource(pub Workspace);

impl WorkspaceResource {
    /// Construct a fresh, empty workspace.
    pub fn new() -> Self {
        Self(Workspace::new())
    }
}

impl Deref for WorkspaceResource {
    type Target = Workspace;
    fn deref(&self) -> &Workspace {
        &self.0
    }
}

impl DerefMut for WorkspaceResource {
    fn deref_mut(&mut self) -> &mut Workspace {
        &mut self.0
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Events — fine-grained signals for observers
// ─────────────────────────────────────────────────────────────────────────────

/// A Twin was just added to the Workspace. Carries the id the
/// Workspace minted so the observer can, e.g., focus the new twin in
/// the Twin Browser or activate a default Perspective.
#[derive(Event, Clone, Copy, Debug)]
pub struct TwinAdded {
    /// The id assigned by [`Workspace::add_twin`].
    pub twin: TwinId,
}

/// A Twin was removed from the Workspace. Its documents and pending work
/// are retired through their owning lifecycle handlers before replacement.
#[derive(Event, Clone, Debug)]
pub struct TwinClosed {
    /// The id that used to identify the Twin.
    pub twin: TwinId,
    /// The root that was removed. Consumers use this to retire external
    /// registries keyed by filesystem root before a replacement is opened.
    pub root: std::path::PathBuf,
    /// Whether this Twin was the active workspace Twin immediately before it
    /// was removed. Consumers that own active-session state use this edge for
    /// teardown; closing a non-active workspace Twin must not interrupt the
    /// active session.
    pub was_active: bool,
}

/// A Document was just opened (registered in the Workspace). The
/// Workspace stores only metadata; domain registries own the parsed
/// source. Observers listening for this typically populate per-doc UI
/// state (open a tab, start a buffer mirror, etc.).
#[derive(Event, Clone, Copy, Debug)]
pub struct DocumentOpened {
    /// The id minted by the owning domain registry.
    pub doc: DocumentId,
}

/// A Document was just closed. The tab is gone; domain cleanup (drop
/// the undo stack, release caches) belongs in this observer.
#[derive(Event, Clone, Copy, Debug)]
pub struct DocumentClosed {
    /// The id that used to identify the Document.
    pub doc: DocumentId,
}

/// A filesystem entry inside a Twin was renamed. Fired by the
/// `RenameTwinEntry` observer after the on-disk move + Twin re-scan
/// succeed. Domain plugins observe this to chain follow-up work that
/// the workbench layer can't sensibly do itself:
///
/// - Modelica: if both ends are `.mo`, rename the file's top-level
///   class declaration so the file-stem invariant holds (and, in a
///   future slice, rewrite cross-file references).
/// - USD: rewrite `references = @./old@` payloads in sibling stages.
///
/// Paths are absolute and post-rename for `new_abs`; `old_abs` is the
/// pre-rename absolute path. Both refer to the same entry kind (file
/// or directory).
#[derive(Event, Clone, Debug)]
pub struct FileRenamed {
    /// The Twin the entry belongs to.
    pub twin: TwinId,
    /// Absolute path before the rename (no longer on disk).
    pub old_abs: std::path::PathBuf,
    /// Absolute path after the rename.
    pub new_abs: std::path::PathBuf,
    /// `true` if the entry is a directory, `false` if a regular file.
    pub is_dir: bool,
}

// ─────────────────────────────────────────────────────────────────────────────
// Convenience command-side helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Request the Workspace register a Document. Observer fans out to
/// `Workspace::add_document` and fires [`DocumentOpened`].
///
/// Why an event rather than calling `Workspace::add_document`
/// directly: callers don't always hold a `ResMut<WorkspaceResource>`
/// (observer functions can't take `&mut World`, some places hold a
/// `Commands`), and keeping a single event-driven entry point makes
/// external-change watchers and the API route converge on the same
/// code path.
#[derive(Event, Clone, Debug)]
pub struct RegisterDocument {
    /// Metadata for the new entry.
    pub entry: DocumentEntry,
}

/// Request the Workspace drop a Document.
#[derive(Event, Clone, Copy, Debug)]
pub struct UnregisterDocument {
    /// Which Document to drop.
    pub doc: DocumentId,
}

fn on_register_document(
    trigger: On<RegisterDocument>,
    mut ws: ResMut<WorkspaceResource>,
    mut commands: Commands,
) {
    let id = trigger.event().entry.id;
    ws.add_document(trigger.event().entry.clone());
    commands.trigger(DocumentOpened { doc: id });
}

fn on_unregister_document(
    trigger: On<UnregisterDocument>,
    mut ws: ResMut<WorkspaceResource>,
    mut commands: Commands,
) {
    let id = trigger.event().doc;
    if ws.close_document(id).is_some() {
        commands.trigger(DocumentClosed { doc: id });
    }
}

/// Close Twin-owned documents through their domain owners, then remove the
/// workspace metadata. Capture ownership before deferred teardown can remove
/// registries or change active scope; foreign and loose documents stay open.
fn on_twin_closed(
    trigger: On<TwinClosed>,
    workspace: Res<WorkspaceResource>,
    mut commands: Commands,
) {
    let event = trigger.event();
    let docs: Vec<_> = workspace
        .documents()
        .iter()
        .filter(|entry| crate::document_belongs_to_twin_root(entry, event.twin, &event.root))
        .map(|entry| entry.id)
        .collect();
    commands.queue(move |world: &mut World| {
        close_documents(world, docs);
    });
}

/// Apply domain cleanup before retiring workspace metadata.
pub(crate) fn close_documents(world: &mut World, docs: Vec<DocumentId>) {
    for doc in docs {
        world.trigger(lunco_doc_bevy::CloseDocument { doc_id: doc });
        world.flush();
        world.trigger(UnregisterDocument { doc });
        world.flush();
    }
}

/// Plugin: install the [`WorkspaceResource`] and the register/unregister
/// command-event observers. Add it once; it's idempotent at the call site
/// (guard with `is_plugin_added`). Recents persistence is wired separately
/// by the consumer that owns config-dir resolution.
pub struct WorkspacePlugin;

impl Plugin for WorkspacePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<WorkspaceResource>()
            .add_observer(on_register_document)
            .add_observer(on_unregister_document)
            .add_observer(on_twin_closed);
        // Twin edit-journal history persistence — load on open, save on
        // `DocumentSaved` + debounced periodic — to `<twin>/history/journal.json`,
        // for twins that opted in with `[journal] persist = true`. ONE plugin,
        // shared with the headless server (DRY). No-op without a
        // `JournalResource`, so journal-free builds are unaffected.
        app.add_plugins(crate::journal_persistence::WorkspaceJournalPlugin);
        // `OpenTwin` + the folder-scan pipeline. Here rather than in the
        // workbench because opening a Twin needs no window — see `crate::open`.
        crate::open::build(app);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DocumentKindId;
    use lunco_doc::DocumentOrigin;

    #[test]
    fn twin_close_retires_owned_documents_through_domain_commands() {
        #[derive(Resource, Default)]
        struct Closed(Vec<DocumentId>);
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(WorkspacePlugin)
            .init_resource::<Closed>()
            .add_observer(
                |event: On<lunco_doc_bevy::CloseDocument>, mut closed: ResMut<Closed>| {
                    closed.0.push(event.doc_id);
                },
            );
        let twin = crate::TwinId::new(1);
        let root = std::path::PathBuf::from("closed-twin");
        for (raw, origin, context_twin) in [
            (
                1,
                DocumentOrigin::writable_file(root.join("scene.usda")),
                None,
            ),
            (2, DocumentOrigin::untitled("draft"), Some(twin)),
            (3, DocumentOrigin::writable_file("other/model.mo"), None),
        ] {
            app.world_mut()
                .resource_mut::<WorkspaceResource>()
                .add_document(DocumentEntry {
                    id: DocumentId::new(raw),
                    kind: DocumentKindId::new("test"),
                    origin,
                    context_twin,
                    title: "test".into(),
                    dirty: false,
                });
        }
        app.world_mut().trigger(TwinClosed {
            twin,
            root,
            was_active: true,
        });
        app.world_mut().flush();
        assert_eq!(
            app.world().resource::<Closed>().0,
            vec![DocumentId::new(1), DocumentId::new(2)]
        );
        let workspace = app.world().resource::<WorkspaceResource>();
        assert_eq!(workspace.documents().len(), 1);
        assert_eq!(workspace.documents()[0].id, DocumentId::new(3));
    }

    #[test]
    fn resource_defaults_empty() {
        let r = WorkspaceResource::default();
        assert_eq!(r.documents().len(), 0);
    }

    #[test]
    fn deref_exposes_workspace_api() {
        let mut r = WorkspaceResource::new();
        r.add_document(DocumentEntry {
            id: DocumentId::new(1),
            kind: DocumentKindId::new("modelica"),
            origin: DocumentOrigin::untitled("X"),
            context_twin: None,
            title: "X".into(),
            dirty: true,
        });
        assert_eq!(r.documents().len(), 1);
    }
}
