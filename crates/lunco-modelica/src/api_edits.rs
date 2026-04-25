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
use crate::pretty::{ComponentDecl, ConnectEquation, Line, Placement, PortRef};
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
            .register_type::<ApplyModelicaOps>()
            .register_type::<ApiOp>()
            .register_type::<ApiPlacement>()
            .register_type::<ApiModification>()
            .add_observer(on_set_document_source)
            .add_observer(on_add_modelica_component)
            .add_observer(on_remove_modelica_component)
            .add_observer(on_connect_components)
            .add_observer(on_disconnect_components)
            .add_observer(on_apply_modelica_ops);
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

// ─── Batched ApplyModelicaOps + Reflect-friendly mirror enum ───────────
//
// `ModelicaOp` (in `crate::document`) carries a `Range<usize>` for the
// `EditText` variant, which is not directly Reflect-derivable. Rather
// than creep Reflect derives across the doc + pretty layers, this
// module defines a *mirror* enum [`ApiOp`] that mirrors the structural
// op variants we want callers to fire over the API and converts to the
// internal type at the observer boundary.
//
// The structural ops (`Add/RemoveComponent`, `Add/RemoveConnection`,
// `SetPlacement`) cover what the canvas drag-drop pipeline needs and
// what an external agent would reasonably want. Free-form text edits
// (`ReplaceSource`, `EditText`) are reachable via [`SetDocumentSource`]
// instead.
//
// The canvas + GUI panels fire [`ApplyModelicaOps`] in lieu of calling
// [`crate::ui::panels::canvas_diagram::apply_ops`] directly, so all
// mutations go through the same Reflect command surface (per
// AGENTS.md §4.1 rule 3).

/// Reflect-friendly placement payload.
#[derive(Reflect, Clone, Debug, Default)]
pub struct ApiPlacement {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// Reflect-friendly key/value modification entry. The internal
/// [`ComponentDecl`] holds these as `Vec<(String, String)>` — the
/// tuple shape doesn't deserialise cleanly from the JSON callers
/// actually send, so this struct is the wire form.
#[derive(Reflect, Clone, Debug, Default)]
pub struct ApiModification {
    pub name: String,
    pub value: String,
}

/// One structural op against a Modelica document, in a Reflect-friendly
/// shape. Variants mirror the structural subset of
/// [`crate::document::ModelicaOp`] — text-level ops are out of scope
/// here and use [`SetDocumentSource`] instead.
///
/// Connection variants encode `from`/`to` as separate `component` +
/// `port` strings rather than dot-paths so the Reflect deserializer
/// can validate fields directly without parsing string syntax.
#[derive(Reflect, Clone, Debug, Default)]
pub enum ApiOp {
    /// Default value for Reflect — never appears in real payloads.
    #[default]
    Noop,
    AddComponent {
        class: String,
        type_name: String,
        name: String,
        modifications: Vec<ApiModification>,
        placement: ApiPlacement,
    },
    RemoveComponent {
        class: String,
        name: String,
    },
    AddConnection {
        class: String,
        from_component: String,
        from_port: String,
        to_component: String,
        to_port: String,
        /// Optional polyline waypoints, flattened as
        /// `[x0, y0, x1, y1, ...]`. Empty = no annotation, renderer
        /// uses its auto-router.
        line_points: Vec<f32>,
    },
    RemoveConnection {
        class: String,
        from_component: String,
        from_port: String,
        to_component: String,
        to_port: String,
    },
    SetPlacement {
        class: String,
        name: String,
        placement: ApiPlacement,
    },
    SetParameter {
        class: String,
        component: String,
        param: String,
        value: String,
    },
}

/// Batched edit event — primary entry point for both the GUI canvas
/// pipeline and external API callers that want to apply multiple ops
/// in a single observer pass.
///
/// Each op is applied in order through the same `host.apply` path the
/// individual per-op events use. Today every applied op is a separate
/// undo entry (matches pre-migration behaviour); transactional grouping
/// — applying N ops as one undo step — is a follow-up.
#[derive(Event, Reflect, Clone, Debug, Default)]
#[reflect(Event, Default)]
pub struct ApplyModelicaOps {
    pub doc: u64,
    pub ops: Vec<ApiOp>,
}

fn on_apply_modelica_ops(
    trigger: On<ApplyModelicaOps>,
    mut commands: Commands,
) {
    let raw = trigger.event().doc;
    let ops = trigger.event().ops.clone();
    commands.queue(move |world: &mut World| {
        let Some(doc) = resolve_doc(world, raw) else {
            bevy::log::warn!("[ApplyModelicaOps] no doc for id {}", raw);
            return;
        };
        let internal: Vec<ModelicaOp> = ops.iter().filter_map(api_op_to_internal).collect();
        if internal.is_empty() {
            return;
        }
        crate::ui::panels::canvas_diagram::apply_ops_public(world, doc, internal);
    });
}

/// Convert one mirror [`ApiOp`] to the internal [`ModelicaOp`].
/// Returns `None` for `Noop` and for malformed payloads (e.g.
/// `placement.width <= 0` we treat as "no override" → default extent).
fn api_op_to_internal(op: &ApiOp) -> Option<ModelicaOp> {
    match op {
        ApiOp::Noop => None,
        ApiOp::AddComponent {
            class,
            type_name,
            name,
            modifications,
            placement,
        } => {
            if class.is_empty() || type_name.is_empty() || name.is_empty() {
                return None;
            }
            Some(ModelicaOp::AddComponent {
                class: class.clone(),
                decl: ComponentDecl {
                    type_name: type_name.clone(),
                    name: name.clone(),
                    modifications: modifications
                        .iter()
                        .map(|m| (m.name.clone(), m.value.clone()))
                        .collect(),
                    placement: Some(api_placement_to_internal(placement)),
                },
            })
        }
        ApiOp::RemoveComponent { class, name } => {
            if class.is_empty() || name.is_empty() {
                return None;
            }
            Some(ModelicaOp::RemoveComponent {
                class: class.clone(),
                name: name.clone(),
            })
        }
        ApiOp::AddConnection {
            class,
            from_component,
            from_port,
            to_component,
            to_port,
            line_points,
        } => {
            let from = port_ref_or_none(from_component, from_port)?;
            let to = port_ref_or_none(to_component, to_port)?;
            let line = if line_points.is_empty() {
                None
            } else {
                let pairs: Vec<(f32, f32)> = line_points
                    .chunks(2)
                    .filter(|c| c.len() == 2)
                    .map(|c| (c[0], c[1]))
                    .collect();
                if pairs.is_empty() { None } else { Some(Line { points: pairs }) }
            };
            Some(ModelicaOp::AddConnection {
                class: class.clone(),
                eq: ConnectEquation { from, to, line },
            })
        }
        ApiOp::RemoveConnection {
            class,
            from_component,
            from_port,
            to_component,
            to_port,
        } => {
            let from = port_ref_or_none(from_component, from_port)?;
            let to = port_ref_or_none(to_component, to_port)?;
            Some(ModelicaOp::RemoveConnection {
                class: class.clone(),
                from,
                to,
            })
        }
        ApiOp::SetPlacement {
            class,
            name,
            placement,
        } => {
            if class.is_empty() || name.is_empty() {
                return None;
            }
            Some(ModelicaOp::SetPlacement {
                class: class.clone(),
                name: name.clone(),
                placement: api_placement_to_internal(placement),
            })
        }
        ApiOp::SetParameter {
            class,
            component,
            param,
            value,
        } => {
            if class.is_empty() || component.is_empty() || param.is_empty() {
                return None;
            }
            Some(ModelicaOp::SetParameter {
                class: class.clone(),
                component: component.clone(),
                param: param.clone(),
                value: value.clone(),
            })
        }
    }
}

fn api_placement_to_internal(p: &ApiPlacement) -> Placement {
    Placement {
        x: p.x,
        y: p.y,
        width: if p.width > 0.0 { p.width } else { 20.0 },
        height: if p.height > 0.0 { p.height } else { 20.0 },
    }
}

fn port_ref_or_none(component: &str, port: &str) -> Option<PortRef> {
    if component.is_empty() || port.is_empty() {
        return None;
    }
    Some(PortRef::new(component.to_string(), port.to_string()))
}

/// Convert an internal [`ModelicaOp`] back to its [`ApiOp`] mirror.
/// Used by GUI panels (canvas drag-drop, diagram viewer) to fire
/// [`ApplyModelicaOps`] with already-constructed ops, keeping a single
/// command pipeline for both UI and external API callers (per
/// AGENTS.md §4.1).
///
/// Returns `None` for non-structural ops (`ReplaceSource`, `EditText`)
/// — those go through [`SetDocumentSource`] / typed-event paths
/// instead, so a UI accidentally trying to fire them via the
/// structural pipeline is a no-op rather than a silent corruption.
pub(crate) fn internal_op_to_api(op: &ModelicaOp) -> Option<ApiOp> {
    match op {
        ModelicaOp::AddComponent { class, decl } => Some(ApiOp::AddComponent {
            class: class.clone(),
            type_name: decl.type_name.clone(),
            name: decl.name.clone(),
            modifications: decl
                .modifications
                .iter()
                .map(|(k, v)| ApiModification {
                    name: k.clone(),
                    value: v.clone(),
                })
                .collect(),
            placement: decl
                .placement
                .map(internal_placement_to_api)
                .unwrap_or_default(),
        }),
        ModelicaOp::RemoveComponent { class, name } => Some(ApiOp::RemoveComponent {
            class: class.clone(),
            name: name.clone(),
        }),
        ModelicaOp::AddConnection { class, eq } => {
            let line_points = eq
                .line
                .as_ref()
                .map(|l| l.points.iter().flat_map(|(x, y)| [*x, *y]).collect())
                .unwrap_or_default();
            Some(ApiOp::AddConnection {
                class: class.clone(),
                from_component: eq.from.component.clone(),
                from_port: eq.from.port.clone(),
                to_component: eq.to.component.clone(),
                to_port: eq.to.port.clone(),
                line_points,
            })
        }
        ModelicaOp::RemoveConnection { class, from, to } => Some(ApiOp::RemoveConnection {
            class: class.clone(),
            from_component: from.component.clone(),
            from_port: from.port.clone(),
            to_component: to.component.clone(),
            to_port: to.port.clone(),
        }),
        ModelicaOp::SetPlacement {
            class,
            name,
            placement,
        } => Some(ApiOp::SetPlacement {
            class: class.clone(),
            name: name.clone(),
            placement: internal_placement_to_api(*placement),
        }),
        ModelicaOp::SetParameter {
            class,
            component,
            param,
            value,
        } => Some(ApiOp::SetParameter {
            class: class.clone(),
            component: component.clone(),
            param: param.clone(),
            value: value.clone(),
        }),
        // Text-level ops are intentionally excluded from this pipeline.
        ModelicaOp::ReplaceSource { .. } | ModelicaOp::EditText { .. } => None,
    }
}

fn internal_placement_to_api(p: Placement) -> ApiPlacement {
    ApiPlacement {
        x: p.x,
        y: p.y,
        width: p.width,
        height: p.height,
    }
}

/// Fire [`ApplyModelicaOps`] for a batch of internal-typed ops.
///
/// GUI helper: the canvas pipeline still constructs `ModelicaOp` values
/// internally (the event flow translates scene events directly into
/// the typed-op shape), but per AGENTS.md §4.1 rule 3 the *application*
/// must go through a Reflect command. This converts to mirror ops and
/// fires the event in one place so panels do not duplicate the
/// conversion.
///
/// Skips ops that have no mirror form (text-level
/// `ReplaceSource`/`EditText`) — those are not meant to flow through
/// the structural pipeline. If every input op was non-structural the
/// trigger is omitted (no-op).
pub fn trigger_apply_ops(
    world: &mut World,
    doc: lunco_doc::DocumentId,
    ops: Vec<ModelicaOp>,
) {
    let api_ops: Vec<ApiOp> = ops.iter().filter_map(internal_op_to_api).collect();
    if api_ops.is_empty() {
        return;
    }
    world.commands().trigger(ApplyModelicaOps {
        doc: doc.raw(),
        ops: api_ops,
    });
}
