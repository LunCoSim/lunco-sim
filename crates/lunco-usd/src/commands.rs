//! `UsdCommandsPlugin` — typed-command surface for USD documents.
//!
//! Plumbs USD into the shared workbench command bus described in
//! `AGENTS.md` §4.2:
//!
//! - **Open**: observes [`OpenFile`]
//!   and handles paths with a USD extension. Modelica observes the same
//!   command for `.mo`; future SysML / mission crates will join the
//!   chorus. Each observer is responsible for its own extension gate so
//!   an `OpenFile { path: "/foo.mo" }` doesn't end up parsed as USD.
//! - **New**: observes [`NewDocument`]
//!   gated on `kind == "usd"`. Lets File→New surface "USD Stage" once
//!   the kind is registered.
//! - **Save**: observes
//!   [`SaveDocument`] gated on
//!   [`UsdDocumentRegistry::contains`].
//! - **Notifications**: each frame drains the registry's pending rings
//!   into [`DocumentOpened`],
//!   [`lunco_doc_bevy::DocumentChanged`], and
//!   [`DocumentClosed`] so views
//!   subscribe through the canonical channels rather than polling the
//!   registry directly.
//!
//! Registers the `usd` document kind in
//! [`DocumentKindRegistry`] on build
//! so File menus, picker dialogs, and `twin.toml` parsers see USD
//! without any central edit.

use bevy::prelude::*;
use lunco_core::{Command, on_command, register_commands};
use lunco_doc::{DocumentId, DocumentOrigin};
use lunco_doc_bevy::{DocumentChanged, DocumentClosed, DocumentOpened, SaveDocument};
use lunco_twin::{DocumentKind, DocumentKindId, DocumentKindMeta, DocumentKindRegistry, FileKind};
use lunco_workbench::file_ops::{NewDocument, OpenFile};
use lunco_workbench::{TwinAdded, WorkspaceResource};

use crate::document::UsdOp;
use crate::registry::UsdDocumentRegistry;

/// Stable id for the USD document kind in
/// [`DocumentKindRegistry`].
pub const USD_DOCUMENT_KIND: &str = "usd";

/// Plugin that registers the USD document kind, the typed-command
/// observers, and the pending-event drain system.
///
/// **Layer 2 (domain).** No UI, no Bevy renderer touches — added by
/// [`UsdPlugins`](crate::UsdPlugins) so any binary that pulls in USD
/// gets the document surface, even headless / sandbox bins.
pub struct UsdCommandsPlugin;

impl Plugin for UsdCommandsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<UsdDocumentRegistry>();

        // Self-register with the workbench's plugin-driven document
        // kind registry. `init_resource` defends against the case where
        // the workbench plugin hasn't been added yet — we still own
        // our entry, the workbench picks it up when it boots.
        app.init_resource::<DocumentKindRegistry>();
        app.world_mut()
            .resource_mut::<DocumentKindRegistry>()
            .register(
                DocumentKindId::new(USD_DOCUMENT_KIND),
                DocumentKindMeta {
                    display_name: "USD Stage".into(),
                    extensions: vec!["usda", "usdc", "usd"],
                    can_create_new: true,
                    default_filename: Some("NewStage.usda"),
                    uri_scheme: Some("usd"),
                    manifest_section: Some("usd"),
                },
            );

        app.add_systems(Update, drain_usd_pending_events);
        app.add_observer(open_usd_docs_on_twin_added);
        register_all_commands(app);
    }
}

/// On `TwinAdded`, eagerly fire [`OpenFile`] for every `.usd*` file
/// the new Twin contains. Mirrors how a Modelica twin's `.mo` files
/// surface in the browser — but for USD we go all the way to "open
/// the document" so cosim can wire `lunco:modelicaModel` /
/// `lunco:simWires` participants from prim attributes through
/// [`UsdSimPlugin`](lunco_usd_sim::UsdSimPlugin).
///
/// Skips USD files inside child Twins — those have their own
/// `TwinAdded` event when the workspace eagerly opens them.
fn open_usd_docs_on_twin_added(
    trigger: On<TwinAdded>,
    workspace: Res<WorkspaceResource>,
    mut commands: Commands,
) {
    let twin_id = trigger.event().twin;
    let Some(twin) = workspace.twin(twin_id) else {
        return;
    };
    for file in twin.files() {
        if matches!(file.kind, FileKind::Document(DocumentKind::Usd)) {
            let abs = twin.root.join(&file.relative_path);
            commands.trigger(OpenFile {
                path: abs.to_string_lossy().into_owned(),
            });
        }
    }
}

register_commands!(
    on_apply_usd_op,
    on_new_document,
    on_open_file,
    on_save_document,
);

// ─────────────────────────────────────────────────────────────────────
// OpenFile — gated on USD extensions
// ─────────────────────────────────────────────────────────────────────

#[on_command(OpenFile)]
fn on_open_file(trigger: On<OpenFile>, mut commands: Commands) {
    let path = trigger.event().path.clone();
    if !is_usd_path(&path) {
        return;
    }
    commands.queue(move |world: &mut World| {
        let path_buf = std::path::PathBuf::from(&path);
        let source = match std::fs::read_to_string(&path_buf) {
            Ok(s) => s,
            Err(e) => {
                bevy::log::warn!("[OpenUsd] {} read failed: {}", path, e);
                return;
            }
        };
        let mut registry = world.resource_mut::<UsdDocumentRegistry>();
        let doc_id = registry.allocate(
            source,
            DocumentOrigin::File {
                path: path_buf,
                writable: true,
            },
        );
        bevy::log::info!("[OpenUsd] opened `{}` as {}", path, doc_id);

        // Also import into the running scene (additive — Blender's
        // File → Append). Helper no-ops on same `(asset, root_prim)`,
        // and warns + skips for files outside the asset root.
        lunco_usd_sim::cosim::spawn_scene_root_world(world, &path, "");
    });
}

// ─────────────────────────────────────────────────────────────────────
// NewDocument — File→New "USD Stage"
// ─────────────────────────────────────────────────────────────────────

#[on_command(NewDocument)]
fn on_new_document(trigger: On<NewDocument>, mut commands: Commands) {
    if trigger.event().kind != USD_DOCUMENT_KIND {
        return;
    }
    commands.queue(|world: &mut World| {
        let mut registry = world.resource_mut::<UsdDocumentRegistry>();
        let next = registry.ids().count() + 1;
        let doc_id = registry.allocate(
            DEFAULT_USDA_SCAFFOLD.to_string(),
            DocumentOrigin::untitled(format!("UntitledStage-{}.usda", next)),
        );
        bevy::log::info!("[NewUsd] created untitled USD stage as {}", doc_id);
    });
}

/// Minimal valid `.usda` source for File→New. One empty `World` Xform
/// — enough that the parser is happy and the user has somewhere to
/// add prims.
const DEFAULT_USDA_SCAFFOLD: &str =
    "#usda 1.0\n(\n    defaultPrim = \"World\"\n)\n\ndef Xform \"World\"\n{\n}\n";

// ─────────────────────────────────────────────────────────────────────
// SaveDocument — gated on registry membership
// ─────────────────────────────────────────────────────────────────────

#[on_command(SaveDocument)]
fn on_save_document(trigger: On<SaveDocument>, mut commands: Commands) {
    let doc_id = trigger.event().doc;
    commands.queue(move |world: &mut World| {
        let registry = world.resource::<UsdDocumentRegistry>();
        let Some(host) = registry.host(doc_id) else {
            return;
        };
        let doc = host.document();
        let path = match doc.origin() {
            DocumentOrigin::File {
                path,
                writable: true,
            } => path.clone(),
            DocumentOrigin::File {
                writable: false, ..
            } => {
                bevy::log::warn!("[SaveUsd] {} is read-only", doc_id);
                return;
            }
            DocumentOrigin::Untitled { .. } => {
                bevy::log::warn!(
                    "[SaveUsd] {} is Untitled — Save-As required",
                    doc_id
                );
                return;
            }
            DocumentOrigin::Bundled { .. } => {
                bevy::log::warn!(
                    "[SaveUsd] {} is a bundled example — read-only",
                    doc_id
                );
                return;
            }
        };
        let source = doc.source().to_string();
        if let Err(e) = std::fs::write(&path, &source) {
            bevy::log::error!("[SaveUsd] {} write to {} failed: {}", doc_id, path.display(), e);
            return;
        }
        // Borrow mut to mark saved. `host_mut` doesn't bump the
        // change ring because saving doesn't change the document — it
        // only resets the dirty marker.
        if let Some(host) = world
            .resource_mut::<UsdDocumentRegistry>()
            .host_mut(doc_id)
        {
            host.document_mut().mark_saved();
        }
        bevy::log::info!("[SaveUsd] {} saved to {}", doc_id, path.display());
    });
}

// ─────────────────────────────────────────────────────────────────────
// ApplyUsdOp — typed entry for programmatic / UI-driven edits
// ─────────────────────────────────────────────────────────────────────

/// Apply a [`UsdOp`] to the named document via the typed-command bus.
///
/// Same shape as `lunco-modelica`'s op-dispatch commands: UI clicks,
/// HTTP API calls, and scripts all dispatch this; the observer
/// routes it through [`UsdDocumentRegistry::apply`] so undo/redo,
/// change notification, and read-only enforcement stay in one place.
#[Command(default)]
pub struct ApplyUsdOp {
    /// Target document.
    pub doc: DocumentId,
    /// Operation to apply.
    pub op: UsdOp,
}

#[on_command(ApplyUsdOp)]
fn on_apply_usd_op(trigger: On<ApplyUsdOp>, mut commands: Commands) {
    let doc = trigger.event().doc;
    let op = trigger.event().op.clone();
    commands.queue(move |world: &mut World| {
        let mut registry = world.resource_mut::<UsdDocumentRegistry>();
        match registry.apply(doc, op) {
            Ok(ack) => {
                bevy::log::debug!(
                    "[ApplyUsdOp] {} → gen {}",
                    doc,
                    ack.new_gen.unwrap_or(0)
                );
            }
            Err(reject) => {
                bevy::log::warn!("[ApplyUsdOp] {} rejected: {:?}", doc, reject);
            }
        }
    });
}

// ─────────────────────────────────────────────────────────────────────
// Pending-event drain — registry rings → trigger events
// ─────────────────────────────────────────────────────────────────────

/// Each frame, drain the registry's pending-event rings into the
/// canonical [`lunco_doc_bevy`] notification triggers.
///
/// Mirrors the publish-events system in `lunco-modelica`. Cheap
/// no-op when nothing is pending; gated implicitly by the
/// `Vec::is_empty` checks inside `drain_pending`.
fn drain_usd_pending_events(
    mut registry: ResMut<UsdDocumentRegistry>,
    mut commands: Commands,
) {
    let pending = registry.drain_pending();
    if pending.opened.is_empty()
        && pending.changed.is_empty()
        && pending.closed.is_empty()
    {
        return;
    }
    for doc in pending.opened {
        commands.trigger(DocumentOpened::local(doc));
    }
    for doc in pending.changed {
        commands.trigger(DocumentChanged::local(doc));
    }
    for doc in pending.closed {
        commands.trigger(DocumentClosed::local(doc));
    }
}

// ─────────────────────────────────────────────────────────────────────
// helpers
// ─────────────────────────────────────────────────────────────────────

/// True if `path`'s extension is one of `usda` / `usdc` / `usd`.
/// Used by the `OpenFile` observer to skip non-USD paths.
pub fn is_usd_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    matches!(
        std::path::Path::new(&lower)
            .extension()
            .and_then(|s| s.to_str()),
        Some("usda") | Some("usdc") | Some("usd")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_usd_path_recognises_extensions() {
        assert!(is_usd_path("/tmp/scene.usda"));
        assert!(is_usd_path("scene.USD"));
        assert!(is_usd_path("foo/bar.usdc"));
        assert!(!is_usd_path("/tmp/model.mo"));
        assert!(!is_usd_path("README.md"));
        assert!(!is_usd_path(""));
    }

    /// Smoke-test: building the plugin into a minimal app inserts
    /// the registry, the document kind, and survives one frame.
    #[test]
    fn plugin_boots_and_registers_kind() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(UsdCommandsPlugin);
        app.update();

        assert!(app.world().contains_resource::<UsdDocumentRegistry>());
        let kinds = app.world().resource::<DocumentKindRegistry>();
        let meta = kinds
            .meta(&DocumentKindId::new(USD_DOCUMENT_KIND))
            .expect("usd kind registered");
        assert_eq!(meta.display_name, "USD Stage");
        assert_eq!(meta.extensions, vec!["usda", "usdc", "usd"]);
    }

    #[test]
    fn open_file_for_usd_path_creates_document() {
        // Write a tiny .usda to a tempfile we can resolve.
        let tmp_dir = std::env::temp_dir();
        let tmp_path = tmp_dir.join("lunco_usd_open_file_test.usda");
        std::fs::write(&tmp_path, "#usda 1.0\ndef Xform \"X\" {}\n").unwrap();

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(UsdCommandsPlugin);
        app.update();

        app.world_mut().trigger(OpenFile {
            path: tmp_path.to_string_lossy().to_string(),
        });
        // Two ticks: one to flush the queued world-command, one for
        // the drain system to publish DocumentOpened.
        app.update();
        app.update();

        let reg = app.world().resource::<UsdDocumentRegistry>();
        assert_eq!(reg.ids().count(), 1, "exactly one USD doc opened");

        let _ = std::fs::remove_file(&tmp_path);
    }

    #[test]
    fn open_file_for_non_usd_path_is_noop() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(UsdCommandsPlugin);
        app.update();

        app.world_mut().trigger(OpenFile {
            path: "/tmp/some_model.mo".to_string(),
        });
        app.update();
        app.update();

        let reg = app.world().resource::<UsdDocumentRegistry>();
        assert_eq!(reg.ids().count(), 0, "non-USD path must not allocate");
    }

    #[test]
    fn apply_usd_op_builds_a_rover_through_typed_command_bus() {
        use crate::document::{LayerId, UsdOp};
        use lunco_doc::Document;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(UsdCommandsPlugin);
        app.update();

        // Allocate a blank document.
        let doc_id = {
            let mut reg = app.world_mut().resource_mut::<UsdDocumentRegistry>();
            reg.allocate(
                "#usda 1.0\n".to_string(),
                DocumentOrigin::untitled("UntitledRover.usda".to_string()),
            )
        };
        app.update();

        // Drive a sequence of ApplyUsdOp commands — same path UI
        // toolbars and the HTTP API will use.
        let ops = [
            UsdOp::AddPrim {
                edit_target: LayerId::root(),
                parent_path: "/".into(),
                name: "Rover".into(),
                type_name: Some("Xform".into()),
            },
            UsdOp::AddPrim {
                edit_target: LayerId::root(),
                parent_path: "/Rover".into(),
                name: "Body".into(),
                type_name: Some("Cube".into()),
            },
            UsdOp::AddPrim {
                edit_target: LayerId::root(),
                parent_path: "/Rover".into(),
                name: "WheelFL".into(),
                type_name: Some("Cube".into()),
            },
            UsdOp::SetTranslate {
                edit_target: LayerId::root(),
                path: "/Rover/WheelFL".into(),
                value: [1.0, 0.0, 1.0],
            },
        ];
        for op in ops {
            app.world_mut().trigger(ApplyUsdOp { doc: doc_id, op });
            app.update();
        }
        // One more tick to flush any final queued world commands.
        app.update();

        let reg = app.world().resource::<UsdDocumentRegistry>();
        let host = reg.host(doc_id).expect("doc still alive");
        let src = host.document().source();
        assert!(src.contains("def Xform \"Rover\""));
        assert!(src.contains("def Cube \"Body\""));
        assert!(src.contains("def Cube \"WheelFL\""));
        assert!(src.contains("xformOp:translate = (1, 0, 1)"));
        // Generation advanced once per op.
        assert_eq!(host.document().generation(), 4);
    }

    #[test]
    fn twin_added_auto_opens_usd_docs() {
        // Build a temp folder with a `twin.toml`, two `.usda` files,
        // and one `.mo` (must be ignored), then drive a TwinAdded
        // event through the workbench. Our observer should fire
        // OpenFile for each `.usda`, and the OpenFile observer
        // (already verified) should allocate a UsdDocument per path.
        use lunco_twin::TwinMode;
        use lunco_workbench::WorkspaceResource;

        let tmp = std::env::temp_dir().join("lunco_usd_twin_phase7_test");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        // Minimal `twin.toml` — keeps the folder a real Twin.
        std::fs::write(
            tmp.join("twin.toml"),
            "name = \"phase7\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        std::fs::write(
            tmp.join("scene_a.usda"),
            "#usda 1.0\ndef Xform \"A\" {}\n",
        )
        .unwrap();
        std::fs::write(
            tmp.join("scene_b.usda"),
            "#usda 1.0\ndef Xform \"B\" {}\n",
        )
        .unwrap();
        std::fs::write(tmp.join("controller.mo"), "model Controller end Controller;\n")
            .unwrap();

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<WorkspaceResource>();
        app.add_plugins(UsdCommandsPlugin);
        app.update();

        // Open the Twin and fire TwinAdded ourselves — mirrors what
        // the workbench's open-folder observer does.
        let twin = match TwinMode::open(&tmp).expect("twin opens") {
            TwinMode::Twin(t) => t,
            other => panic!("expected Twin variant, got {:?}", other),
        };
        let twin_id = app
            .world_mut()
            .resource_mut::<WorkspaceResource>()
            .add_twin(twin);
        app.world_mut()
            .trigger(lunco_workbench::TwinAdded { twin: twin_id });
        // Several ticks to flush: TwinAdded → OpenFile triggers →
        // queued world commands run → DocumentOpened drains.
        for _ in 0..4 {
            app.update();
        }

        let reg = app.world().resource::<UsdDocumentRegistry>();
        assert_eq!(
            reg.ids().count(),
            2,
            "exactly two USD docs should auto-open from the twin"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn new_document_with_usd_kind_creates_untitled() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(UsdCommandsPlugin);
        app.update();

        app.world_mut().trigger(NewDocument {
            kind: USD_DOCUMENT_KIND.to_string(),
        });
        app.update();
        app.update();

        let reg = app.world().resource::<UsdDocumentRegistry>();
        assert_eq!(reg.ids().count(), 1);
        let id = reg.ids().next().unwrap();
        assert!(reg.host(id).unwrap().document().origin().is_untitled());
    }
}
