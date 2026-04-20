//! Bevy-side wiring for [`lunco_workspace::Workspace`].
//!
//! The Workspace type itself is headless so CI / API-only servers can
//! use it without pulling in bevy. This module ships the thin bridge
//! the UI needs: a `Resource` newtype plus events for add/close of
//! Twins and Documents so observers can react to session changes
//! without polling the entire Workspace every frame.
//!
//! **Scope (step 5a):** resource + event shapes only. Reader migration
//! (Twin Browser, ModelicaDocumentRegistry, etc.) is step 5b.

use bevy::prelude::*;
use std::ops::{Deref, DerefMut};

use lunco_doc::DocumentId;
use lunco_workspace::{DocumentEntry, TwinId, Workspace};

// ─────────────────────────────────────────────────────────────────────────────
// Resource
// ─────────────────────────────────────────────────────────────────────────────

/// Bevy-side wrapper around [`lunco_workspace::Workspace`].
///
/// Newtyped rather than using `Workspace` directly as a `Resource` so
/// (a) the `lunco-workspace` crate stays bevy-free for headless use,
/// and (b) we can add UI-side invariants (active-tab derivation,
/// change-detection filters) here without touching the core type.
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

/// A Twin was just closed (removed from the Workspace). Documents
/// that were associated with it are *not* closed automatically — they
/// become loose docs until the Workspace is explicitly told otherwise.
#[derive(Event, Clone, Copy, Debug)]
pub struct TwinClosed {
    /// The id that used to identify the Twin.
    pub twin: TwinId,
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

/// Plugin: initialise the resource and install the command-event
/// observers. `WorkbenchPlugin` calls this automatically.
pub struct WorkspacePlugin;

impl Plugin for WorkspacePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<WorkspaceResource>()
            .add_observer(on_register_document)
            .add_observer(on_unregister_document);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use lunco_doc::DocumentOrigin;
    use lunco_workspace::DocumentKind;

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
            kind: DocumentKind::Modelica,
            origin: DocumentOrigin::untitled("X"),
            context_twin: None,
            title: "X".into(),
        });
        assert_eq!(r.documents().len(), 1);
    }
}
