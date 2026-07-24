//! API handlers for component-level operations (Add, Remove).

use super::util::{resolve_doc, strip_same_package_prefix};
use crate::document::ModelicaOp;
use crate::pretty::{ComponentDecl, Placement};
use bevy::prelude::*;
use lunco_core::{on_command, Command};
use lunco_doc::DocumentId;

/// Add a sub-component to a class.
#[Command(default)]
pub struct AddModelicaComponent {
    pub doc: DocumentId,
    pub class: String,
    pub type_name: String,
    pub name: String,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    /// Pulse-glow duration in ms. `0` = no animation (instant).
    pub animation_ms: u32,
}

#[on_command(AddModelicaComponent)]
pub fn on_add_modelica_component(trigger: On<AddModelicaComponent>, mut commands: Commands) {
    let ev = trigger.event().clone();
    commands.queue(move |world: &mut World| {
        let Some(doc) = resolve_doc(world, ev.doc) else {
            bevy::log::warn!("[AddModelicaComponent] no doc {}", ev.doc);
            return;
        };
        if ev.class.is_empty() || ev.type_name.is_empty() || ev.name.is_empty() {
            bevy::log::warn!("[AddModelicaComponent] class/type_name/name must all be non-empty");
            return;
        }
        let placement = if ev.width > 0.0 && ev.height > 0.0 {
            Placement {
                x: ev.x,
                y: ev.y,
                width: ev.width,
                height: ev.height,
            }
        } else {
            Placement::at(ev.x, ev.y)
        };
        let normalized_type = strip_same_package_prefix(&ev.class, &ev.type_name);
        let decl = ComponentDecl {
            type_name: normalized_type,
            name: ev.name.clone(),
            modifications: Vec::new(),
            placement: Some(placement),
        };
        match crate::doc_ops::apply_one_op_as(
            world,
            doc,
            ModelicaOp::AddComponent {
                class: ev.class.clone(),
                decl,
            },
            lunco_twin_journal::AuthorTag::for_tool("api"),
        ) {
            Ok(_) => {
                bevy::log::info!(
                    "[AddModelicaComponent] doc={} class={} {}={}",
                    doc.raw(),
                    ev.class,
                    ev.name,
                    ev.type_name
                );
                let anim_ms = if ev.animation_ms == 0 {
                    crate::canvas_feedback::DEFAULT_PULSE_MS
                } else {
                    ev.animation_ms
                };
                // UI-only feedback: present in the windowed editor, absent
                // (no-op) on a headless server where the queue isn't inserted.
                if let Some(mut q) =
                    world.get_resource_mut::<crate::canvas_feedback::PendingApiFocusQueue>()
                {
                    q.push(crate::canvas_feedback::PendingApiFocus {
                        doc,
                        name: ev.name.clone(),
                        queued_at: web_time::Instant::now(),
                        animation_ms: anim_ms,
                    });
                }
            }
            Err(e) => bevy::log::warn!(
                "[AddModelicaComponent] doc={} {}: {:?}",
                doc.raw(),
                ev.name,
                e
            ),
        }
    });
}

#[Command(default)]
pub struct RemoveModelicaComponent {
    pub doc: DocumentId,
    pub class: String,
    pub name: String,
}

#[on_command(RemoveModelicaComponent)]
pub fn on_remove_modelica_component(trigger: On<RemoveModelicaComponent>, mut commands: Commands) {
    let ev = trigger.event().clone();
    commands.queue(move |world: &mut World| {
        let Some(doc) = resolve_doc(world, ev.doc) else {
            return;
        };
        if ev.class.is_empty() || ev.name.is_empty() {
            return;
        }
        match crate::doc_ops::apply_one_op_as(
            world,
            doc,
            ModelicaOp::RemoveComponent {
                class: ev.class.clone(),
                name: ev.name.clone(),
            },
            lunco_twin_journal::AuthorTag::for_tool("api"),
        ) {
            Ok(_) => {
                bevy::log::info!(
                    "[RemoveModelicaComponent] doc={} {}.{}",
                    doc.raw(),
                    ev.class,
                    ev.name
                );
            }
            Err(e) => bevy::log::warn!(
                "[RemoveModelicaComponent] doc={} {}.{}: {:?}",
                doc.raw(),
                ev.class,
                ev.name,
                e
            ),
        }
    });
}
