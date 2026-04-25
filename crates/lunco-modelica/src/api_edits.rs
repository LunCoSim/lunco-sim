//! Mutation events for Modelica documents.
//!
//! These commands let a caller mutate open Modelica documents —
//! replace the source verbatim, add or remove components, add or
//! remove `connect` equations. The internal AST ops
//! (`ModelicaOp::AddComponent`, `RemoveComponent`, `AddConnection`,
//! `RemoveConnection`) already exist for the canvas drag-and-drop
//! path; this module wraps them as Reflect-registered events so the
//! GUI command surface and the API share one input shape (per AGENTS.md
//! §4.1).
//!
//! ## Exposure
//!
//! These events are Reflect-registered and exposed on the external
//! API surface (HTTP, MCP, `discover_schema`) by default — the same
//! visibility every other typed command has. The GUI dispatches them
//! too, so there is one input shape across in-process and external
//! callers (per AGENTS.md §4.1).
//!
//! The [`lunco_api::ApiVisibility`] infrastructure is available for
//! future "opt-out" surfaces — domain crates can hide names from the
//! external API without un-registering the events — but Modelica edit
//! commands do not use it today.
//!
//! Mutations honour undo (the existing `ModelicaOp` pipeline builds
//! inverse ops) and the live-source debounced reparse path (writing
//! source bumps the document generation, firing the same change
//! journal as UI-driven edits).
//!
//! ## What this is NOT
//!
//! - **No transactional batching** in v1. Each op is committed and
//!   undoable independently. If a sequence of `add_component` +
//!   `connect` half-succeeds, the agent has to clean up explicitly.
//! - **No structural validation**. `connect(a.x, b.y)` does not check
//!   whether `a.x` exists — Modelica's compile pass catches that. The
//!   API is a syntax-shaped passthrough.
//! - **No diff-based source edits**. `set_document_source` replaces
//!   the entire buffer. A future iteration can add range-based edits.

use bevy::prelude::*;
use lunco_doc::DocumentId;

use crate::document::ModelicaOp;
use crate::pretty::{ComponentDecl, ConnectEquation, Placement, PortRef};
use crate::ui::state::ModelicaDocumentRegistry;

/// Plugin that registers the Modelica edit events + observers.
pub struct ModelicaApiEditPlugin;

impl Plugin for ModelicaApiEditPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<SetDocumentSource>()
            .register_type::<AddModelicaComponent>()
            .register_type::<RemoveModelicaComponent>()
            .register_type::<ConnectComponents>()
            .register_type::<DisconnectComponents>()
            .add_observer(on_set_document_source)
            .add_observer(on_add_modelica_component)
            .add_observer(on_remove_modelica_component)
            .add_observer(on_connect_components)
            .add_observer(on_disconnect_components);
    }
}

// ─── SetDocumentSource ─────────────────────────────────────────────────

/// Replace an open document's entire source text. Bypasses structured
/// ops — the new source goes straight into the document via
/// `checkpoint_source`, which fires the same change journal as a typed
/// edit. Existing undo history is preserved (a `set_document_source`
/// is a single undoable transition like any other op).
///
/// Useful for agents doing whole-file rewrites, applying lints, or
/// importing source from an external tool. Range-based edits are out
/// of scope for v1.
#[derive(Event, Reflect, Clone, Debug, Default)]
#[reflect(Event, Default)]
pub struct SetDocumentSource {
    pub doc: u64,
    pub source: String,
}

fn on_set_document_source(
    trigger: On<SetDocumentSource>,
    mut commands: Commands,
) {
    let doc_raw = trigger.event().doc;
    let source = trigger.event().source.clone();
    commands.queue(move |world: &mut World| {
        let Some(doc) = resolve_doc(world, doc_raw) else {
            bevy::log::warn!("[SetDocumentSource] no doc for id {}", doc_raw);
            return;
        };
        let mut registry = world.resource_mut::<ModelicaDocumentRegistry>();
        registry.checkpoint_source(doc, source);
        bevy::log::info!("[SetDocumentSource] doc={} replaced", doc.raw());
    });
}

// ─── AddModelicaComponent ──────────────────────────────────────────────

/// Add a sub-component to a class. Wraps [`ModelicaOp::AddComponent`].
///
/// `class` is the parent class name where the component lands (e.g.
/// `"RocketStage"`). `type_name` is the component's declared type
/// (e.g. `"Modelica.Electrical.Analog.Basic.Resistor"` or `"Tank"`).
/// `name` is the instance name (e.g. `"r1"`). Optional `x`, `y`, `w`,
/// `h` set the diagram placement; `(0, 0, 20, 20)` is the default.
#[derive(Event, Reflect, Clone, Debug, Default)]
#[reflect(Event, Default)]
pub struct AddModelicaComponent {
    pub doc: u64,
    pub class: String,
    pub type_name: String,
    pub name: String,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

fn on_add_modelica_component(
    trigger: On<AddModelicaComponent>,
    mut commands: Commands,
) {
    let ev = trigger.event().clone();
    commands.queue(move |world: &mut World| {
        let Some(doc) = resolve_doc(world, ev.doc) else {
            bevy::log::warn!("[AddModelicaComponent] no doc {}", ev.doc);
            return;
        };
        if ev.class.is_empty() || ev.type_name.is_empty() || ev.name.is_empty() {
            bevy::log::warn!(
                "[AddModelicaComponent] class/type_name/name must all be non-empty"
            );
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
        let decl = ComponentDecl {
            type_name: ev.type_name.clone(),
            name: ev.name.clone(),
            modifications: Vec::new(),
            placement: Some(placement),
        };
        let mut registry = world.resource_mut::<ModelicaDocumentRegistry>();
        let Some(host) = registry.host_mut(doc) else {
            return;
        };
        match host.apply(ModelicaOp::AddComponent {
            class: ev.class.clone(),
            decl,
        }) {
            Ok(_) => bevy::log::info!(
                "[AddModelicaComponent] doc={} class={} {}={}",
                doc.raw(),
                ev.class,
                ev.name,
                ev.type_name
            ),
            Err(e) => bevy::log::warn!(
                "[AddModelicaComponent] doc={} {}: {:?}",
                doc.raw(),
                ev.name,
                e
            ),
        }
    });
}

// ─── RemoveModelicaComponent ───────────────────────────────────────────

#[derive(Event, Reflect, Clone, Debug, Default)]
#[reflect(Event, Default)]
pub struct RemoveModelicaComponent {
    pub doc: u64,
    pub class: String,
    pub name: String,
}

fn on_remove_modelica_component(
    trigger: On<RemoveModelicaComponent>,
    mut commands: Commands,
) {
    let ev = trigger.event().clone();
    commands.queue(move |world: &mut World| {
        let Some(doc) = resolve_doc(world, ev.doc) else {
            return;
        };
        if ev.class.is_empty() || ev.name.is_empty() {
            return;
        }
        let mut registry = world.resource_mut::<ModelicaDocumentRegistry>();
        let Some(host) = registry.host_mut(doc) else {
            return;
        };
        match host.apply(ModelicaOp::RemoveComponent {
            class: ev.class.clone(),
            name: ev.name.clone(),
        }) {
            Ok(_) => bevy::log::info!(
                "[RemoveModelicaComponent] doc={} {}.{}",
                doc.raw(),
                ev.class,
                ev.name
            ),
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

// ─── ConnectComponents ─────────────────────────────────────────────────

/// Add a `connect(a.p, b.q)` equation to a class.
///
/// `from` and `to` are dot-paths (`"tank.outlet"`, `"valve.inlet"`).
/// The first segment is the component instance, the second the port
/// name. Existing connections are not deduplicated — Modelica permits
/// multiple connect equations on the same pair, and dedup is a
/// caller-side concern.
#[derive(Event, Reflect, Clone, Debug, Default)]
#[reflect(Event, Default)]
pub struct ConnectComponents {
    pub doc: u64,
    pub class: String,
    pub from: String,
    pub to: String,
}

fn on_connect_components(
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
            from,
            to,
            line: None,
        };
        let mut registry = world.resource_mut::<ModelicaDocumentRegistry>();
        let Some(host) = registry.host_mut(doc) else {
            return;
        };
        match host.apply(ModelicaOp::AddConnection {
            class: ev.class.clone(),
            eq,
        }) {
            Ok(_) => bevy::log::info!(
                "[ConnectComponents] doc={} {}: {} -> {}",
                doc.raw(),
                ev.class,
                ev.from,
                ev.to
            ),
            Err(e) => bevy::log::warn!(
                "[ConnectComponents] doc={} failed: {:?}",
                doc.raw(),
                e
            ),
        }
    });
}

// ─── DisconnectComponents ──────────────────────────────────────────────

#[derive(Event, Reflect, Clone, Debug, Default)]
#[reflect(Event, Default)]
pub struct DisconnectComponents {
    pub doc: u64,
    pub class: String,
    pub from: String,
    pub to: String,
}

fn on_disconnect_components(
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
        let mut registry = world.resource_mut::<ModelicaDocumentRegistry>();
        let Some(host) = registry.host_mut(doc) else {
            return;
        };
        match host.apply(ModelicaOp::RemoveConnection {
            class: ev.class.clone(),
            from,
            to,
        }) {
            Ok(_) => bevy::log::info!(
                "[DisconnectComponents] doc={} {}: {} -/- {}",
                doc.raw(),
                ev.class,
                ev.from,
                ev.to
            ),
            Err(e) => bevy::log::warn!(
                "[DisconnectComponents] doc={} failed: {:?}",
                doc.raw(),
                e
            ),
        }
    });
}

// ─── helpers ───────────────────────────────────────────────────────────

fn resolve_doc(world: &mut World, raw: u64) -> Option<DocumentId> {
    if raw == 0 {
        world
            .get_resource::<lunco_workbench::WorkspaceResource>()
            .and_then(|ws| ws.active_document)
    } else {
        Some(DocumentId::new(raw))
    }
}

/// Split `"component.port"` into a [`PortRef`]. Multi-dot paths
/// (`a.b.port`) are intentionally rejected — Modelica permits them but
/// the canvas + ops in this crate work on `instance.port` only.
fn parse_port_ref(s: &str) -> Option<PortRef> {
    let (component, port) = s.split_once('.')?;
    if component.is_empty() || port.is_empty() || port.contains('.') {
        return None;
    }
    Some(PortRef::new(component.to_string(), port.to_string()))
}
