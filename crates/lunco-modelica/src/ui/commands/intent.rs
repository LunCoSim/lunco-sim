//! Intent resolvers: translate abstract EditorIntent into concrete Modelica commands.

use crate::state::ModelicaDocumentRegistry;
use bevy::prelude::*;
use lunco_doc_bevy::{EditorIntent, RedoDocument, SaveAsDocument, SaveDocument, UndoDocument};
use lunco_workbench::file_ops::NewDocument;

// ─── Observers ───────────────────────────────────────────────────────────────

pub fn resolve_editor_intent(
    trigger: On<EditorIntent>,
    workspace: Res<lunco_workspace::WorkspaceResource>,
    registry: Res<ModelicaDocumentRegistry>,
    mut pending_closes: ResMut<lunco_workbench::PendingTabCloses>,
    mut commands: Commands,
) {
    let Some(doc) = workspace.active_document else {
        return;
    };
    if registry.host(doc).is_none() {
        return;
    }

    match *trigger.event() {
        EditorIntent::Undo => commands.trigger(UndoDocument { doc }),
        EditorIntent::Redo => commands.trigger(RedoDocument { doc }),
        EditorIntent::Save => commands.trigger(SaveDocument { doc }),
        EditorIntent::SaveAs => commands.trigger(SaveAsDocument {
            doc,
            path: String::new(),
        }),
        EditorIntent::Close => {
            commands.queue(move |world: &mut World| {
                let Some(tab_id) = world
                    .resource::<crate::model_tabs::ModelTabs>()
                    .any_for_doc(doc)
                else {
                    return;
                };
                if let Some(mut q) = world.get_resource_mut::<lunco_workbench::PendingTabCloses>() {
                    q.push(lunco_workbench::TabId::Instance {
                        kind: crate::ui::MODEL_VIEW_KIND,
                        instance: tab_id,
                    });
                }
            });
            let _ = &mut pending_closes;
        }
        EditorIntent::Compile => {
            commands.trigger(super::compile::CompileActiveModel {
                doc,
                class: String::new(),
            });
        }
        EditorIntent::NewDocument => {}
        // Doc-agnostic: handled by the shared `lunco-tutorial` launcher's own
        // resolver (`resolve_show_tutorial_intent`), which needs no active
        // document. No-op here.
        EditorIntent::ShowTutorial => {}
    }
}

pub fn resolve_new_document_intent(trigger: On<EditorIntent>, mut commands: Commands) {
    if matches!(*trigger.event(), EditorIntent::NewDocument) {
        commands.trigger(NewDocument {
            kind: String::new(),
        });
    }
}
