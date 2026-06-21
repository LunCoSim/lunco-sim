//! API handlers for diagram-level operations (Connect, Disconnect).

use bevy::prelude::*;
use lunco_core::{Command, on_command};
use lunco_doc::DocumentId;
use crate::document::ModelicaOp;
use crate::pretty::{ConnectEquation};
use super::util::{resolve_doc, parse_port_ref};

/// Add a `connect(a.p, b.q)` equation to a class.
#[Command(default)]
pub struct ConnectComponents {
    pub doc: DocumentId,
    pub class: String,
    pub from: String,
    pub to: String,
    /// Edge-flash duration in ms. `0` = no animation.
    pub animation_ms: u32,
}

#[on_command(ConnectComponents)]
pub fn on_connect_components(
    trigger: On<ConnectComponents>,
    mut commands: Commands,
) {
    let ev = trigger.event().clone();
    commands.queue(move |world: &mut World| {
        let Some(doc) = resolve_doc(world, ev.doc) else {
            return;
        };
        let Some(from) = parse_port_ref(&ev.from) else {
            bevy::log::warn!(
                "[ConnectComponents] `from` must be `component.port`, got `{}`",
                ev.from
            );
            return;
        };
        let Some(to) = parse_port_ref(&ev.to) else {
            bevy::log::warn!(
                "[ConnectComponents] `to` must be `component.port`, got `{}`",
                ev.to
            );
            return;
        };
        let eq = ConnectEquation {
            from: from.clone(),
            to: to.clone(),
            line: None,
        };
        match crate::doc_ops::apply_one_op_as(
            world,
            doc,
            ModelicaOp::AddConnection {
                class: ev.class.clone(),
                eq: eq.clone(),
            },
            lunco_twin_journal::AuthorTag::for_tool("api"),
        ) {
            Ok(_) => {
                bevy::log::info!(
                    "[ConnectComponents] doc={} {}: {} -> {}",
                    doc.raw(),
                    ev.class,
                    ev.from,
                    ev.to
                );
                let anim_ms = if ev.animation_ms == 0 {
                    crate::ui::panels::canvas_diagram::DEFAULT_EDGE_FLASH_MS
                } else {
                    ev.animation_ms
                };
                if let Some(mut q) = world.get_resource_mut::<
                    crate::ui::panels::canvas_diagram::PendingApiConnectionQueue,
                >() {
                    q.push(crate::ui::panels::canvas_diagram::PendingApiConnection {
                        doc,
                        from_component: from.component,
                        from_port: from.port,
                        to_component: to.component,
                        to_port: to.port,
                        queued_at: web_time::Instant::now(),
                        animation_ms: anim_ms,
                    });
                }
            }
            Err(e) => bevy::log::warn!(
                "[ConnectComponents] doc={} {}: {:?}",
                doc.raw(),
                ev.class,
                e
            ),
        }
    });
}

#[Command(default)]
pub struct DisconnectComponents {
    pub doc: DocumentId,
    pub class: String,
    pub from: String,
    pub to: String,
}

#[on_command(DisconnectComponents)]
pub fn on_disconnect_components(
    trigger: On<DisconnectComponents>,
    mut commands: Commands,
) {
    let ev = trigger.event().clone();
    commands.queue(move |world: &mut World| {
        let Some(doc) = resolve_doc(world, ev.doc) else {
            return;
        };
        let Some(from) = parse_port_ref(&ev.from) else {
            return;
        };
        let Some(to) = parse_port_ref(&ev.to) else {
            return;
        };
        match crate::doc_ops::apply_one_op_as(
            world,
            doc,
            ModelicaOp::RemoveConnection {
                class: ev.class.clone(),
                from,
                to,
            },
            lunco_twin_journal::AuthorTag::for_tool("api"),
        ) {
            Ok(_) => {
                bevy::log::info!(
                    "[DisconnectComponents] doc={} {}: {} -> {}",
                    doc.raw(),
                    ev.class,
                    ev.from,
                    ev.to
                );
            }
            Err(e) => bevy::log::warn!(
                "[DisconnectComponents] doc={} {}: {:?}",
                doc.raw(),
                ev.class,
                e
            ),
        }
    });
}
