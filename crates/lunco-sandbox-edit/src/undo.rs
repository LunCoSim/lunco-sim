//! Ctrl+Z / Ctrl+Shift+Z — bound to the **document's** undo.
//!
//! There is no editor-side undo stack. There used to be: a `Vec<UndoAction>` that
//! remembered "this entity was spawned" / "this was its old Transform" and wrote the
//! ECS back on Ctrl+Z. It was a second, weaker undo running alongside the real one,
//! and it was wrong in both directions:
//!
//! - It **did not know about the document.** An undone spawn stayed in the runtime
//!   layer and in the journal, so the scene and its source of truth disagreed — and
//!   the next projection could bring the "undone" entity back.
//! - It **only knew two verbs.** Spawn and transform. Terrain strokes, property
//!   edits, attaches, detaches and waypoints — all authored, all invertible — were
//!   simply not undoable, because nobody had taught the stack about them.
//!
//! Every authored edit already reaches the world as a `UsdOp` through `ApplyUsdOp`,
//! and `UsdDocument::apply` returns a **typed inverse** for each. So undo is a
//! property of the document, and it is free: pop the inverse, apply it, let the
//! projection re-derive the ECS. It journals and replicates like any other op, it
//! covers every verb automatically, and there is exactly one of it.
//!
//! The editor's job is therefore only to bind the key.

use bevy::prelude::*;
use lunco_usd::commands::{RedoEdit, UndoEdit};

/// Ctrl+Z → undo, Ctrl+Shift+Z / Ctrl+Y → redo, both on the active document.
///
/// Ignored while egui holds the keyboard, so Ctrl+Z in a text field (the rhai editor,
/// a name box) edits the text instead of reverting the scene.
pub fn handle_undo_input(
    keys: Res<ButtonInput<KeyCode>>,
    egui_focus: Res<lunco_core::EguiFocus>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    mut commands: Commands,
) {
    if egui_focus.wants_keyboard {
        return;
    }
    let ctrl = keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight);
    if !ctrl {
        return;
    }
    let shift = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);

    let redo = keys.just_pressed(KeyCode::KeyY) || (shift && keys.just_pressed(KeyCode::KeyZ));
    let undo = !shift && keys.just_pressed(KeyCode::KeyZ);
    if !undo && !redo {
        return;
    }

    let Some(workspace) = workspace else { return };
    let Some(doc) = workspace.0.active_document else {
        info!("[undo] no active document — nothing to undo");
        return;
    };
    if redo {
        commands.trigger(RedoEdit { doc });
    } else {
        commands.trigger(UndoEdit { doc });
    }
}
