//! USD time-sample authoring for the Editor Inspector.
//!
//! Animation transport remains owned by `lunco-time`; this module only exposes
//! key creation/removal for a selected USD prim. The authored curve is written
//! by `SetTimeSample`/`RemoveTimeSample`, so playback, undo, save, and agent
//! automation all observe the same USD data.

use std::collections::{BTreeSet, HashMap};

use bevy::math::EulerRot;
use bevy::prelude::*;
use bevy_egui::egui;
use lunco_doc::DocumentId;
use lunco_time::{AnimationPreview, Playback};
use lunco_usd::document::{LayerId, UsdOp};
use lunco_usd::ui::viewport::{UsdPreviewId, UsdViewportState};
use lunco_usd_bevy::{
    author::normalize_value_literal, CanonicalStages, SdfPath, UsdPrimPath, UsdRead, UsdStageAsset,
};

#[derive(Clone, Copy)]
enum KeyableChannel {
    Translate,
    Scale,
    Orient,
    Rotate(EulerRot),
}

#[derive(Clone)]
pub struct AnimationChannel {
    pub name: String,
    pub type_name: String,
    pub times: Vec<f64>,
    keyable: Option<KeyableChannel>,
}

#[derive(Default, Clone)]
pub struct UsdAnimationSessionView {
    pub preview: UsdPreviewId,
    pub entity: Option<Entity>,
    pub doc: Option<DocumentId>,
    pub edit_target: Option<LayerId>,
    pub generation: u64,
    pub path: String,
    pub time_codes_per_second: f64,
    pub channels: Vec<AnimationChannel>,
    pub unsupported_channels: Vec<String>,
}

impl UsdAnimationSessionView {
    fn clear(&mut self) {
        *self = Self::default();
    }
}

/// Session-keyed animation views. Timeline transport stays global to its
/// declared time domain; authored channel inspection is isolated per preview.
#[derive(Resource, Default)]
pub struct UsdAnimationView {
    sessions: HashMap<UsdPreviewId, UsdAnimationSessionView>,
}

impl UsdAnimationView {
    pub(crate) fn focused(&self, viewport: &UsdViewportState) -> Option<&UsdAnimationSessionView> {
        viewport
            .focused_preview_id()
            .and_then(|preview| self.sessions.get(&preview))
    }
}

fn keyable_channel(name: &str) -> Option<KeyableChannel> {
    Some(match name {
        "xformOp:translate" => KeyableChannel::Translate,
        "xformOp:scale" => KeyableChannel::Scale,
        "xformOp:orient" => KeyableChannel::Orient,
        "xformOp:rotateXYZ" => KeyableChannel::Rotate(EulerRot::XYZEx),
        "xformOp:rotateXZY" => KeyableChannel::Rotate(EulerRot::XZYEx),
        "xformOp:rotateYXZ" => KeyableChannel::Rotate(EulerRot::YXZEx),
        "xformOp:rotateYZX" => KeyableChannel::Rotate(EulerRot::YZXEx),
        "xformOp:rotateZXY" => KeyableChannel::Rotate(EulerRot::ZXYEx),
        "xformOp:rotateZYX" => KeyableChannel::Rotate(EulerRot::ZYXEx),
        _ => return None,
    })
}

fn type_for_channel<R: UsdRead>(stage: &R, path: &SdfPath, name: &str) -> Option<String> {
    stage.attr_type_name(path, name).or_else(|| {
        Some(
            match name {
                "xformOp:translate" | "xformOp:scale" => "double3",
                "xformOp:orient" => "quatd",
                name if name.starts_with("xformOp:rotate") => "double3",
                _ => return None,
            }
            .to_owned(),
        )
    })
}

/// Rebuild keyable channels for the selected prim in every open Editor preview.
pub fn produce_usd_animation_view(
    selected: Option<Res<lunco_scene_commands::SelectedEntities>>,
    target: Option<Res<crate::InspectorTarget>>,
    q: Query<&UsdPrimPath>,
    q_parents: Query<&ChildOf>,
    stages: Res<Assets<UsdStageAsset>>,
    mut canonical: NonSendMut<CanonicalStages>,
    viewport: Option<Res<UsdViewportState>>,
    mut views: ResMut<UsdAnimationView>,
) {
    let Some(viewport) = viewport else {
        views.sessions.clear();
        return;
    };
    let open: std::collections::HashSet<_> = viewport.sessions().map(|s| s.id()).collect();
    views.sessions.retain(|preview, _| open.contains(preview));

    for session in viewport.sessions() {
        let view = views
            .sessions
            .entry(session.id())
            .or_insert_with(|| UsdAnimationSessionView {
                preview: session.id(),
                doc: Some(session.doc()),
                edit_target: Some(session.edit_target().clone()),
                ..Default::default()
            });
        view.clear();
        view.preview = session.id();
        view.doc = Some(session.doc());
        view.edit_target = Some(session.edit_target().clone());
        view.generation = session.projected_generation();
        if !session.projection_ready() {
            continue;
        }

        let Some(entity) = crate::ui::selected_entity_in_preview(
            session,
            selected.as_deref(),
            target.as_deref(),
            &q,
            &q_parents,
        ) else {
            continue;
        };
        let Ok(prim) = q.get(entity) else {
            continue;
        };
        let handle = session.stage_handle();
        if prim.stage_handle.id() != handle.id() {
            continue;
        }
        let stage_id = handle.id();
        if canonical.get(stage_id).is_none() {
            if let Some(recipe) = stages.get(handle).and_then(|asset| asset.recipe.clone()) {
                canonical.get_or_build(stage_id, &recipe);
            }
        }
        let Some(stage) = canonical.get(stage_id).map(|value| value.view()) else {
            continue;
        };
        let Ok(path) = SdfPath::new(&prim.path) else {
            continue;
        };

        // xformOpOrder is the authored transform contract. Include xform
        // properties as well because a composed reference can expose a
        // channel before its order is materialized locally.
        let mut names = BTreeSet::new();
        names.extend(
            stage
                .texts(&path, "xformOpOrder")
                .into_iter()
                .filter(|name| name.starts_with("xformOp:") && name != "xformOpOrder"),
        );
        names.extend(
            stage
                .attr_names(&path)
                .into_iter()
                .filter(|name| name.starts_with("xformOp:") && name != "xformOpOrder"),
        );
        if names.is_empty() {
            names.insert("xformOp:translate".into());
        }

        let mut channels = Vec::new();
        let mut unsupported_channels = Vec::new();
        for name in names {
            let Some(type_name) = type_for_channel(&stage, &path, &name) else {
                unsupported_channels.push(name);
                continue;
            };
            let keyable = keyable_channel(&name);
            if keyable.is_none() {
                unsupported_channels.push(name.clone());
            }
            channels.push(AnimationChannel {
                times: stage.time_sample_times(&path, &name),
                name,
                type_name,
                keyable,
            });
        }

        view.entity = Some(entity);
        view.path = prim.path.clone();
        view.time_codes_per_second = stage.time_codes_per_second().max(f64::MIN_POSITIVE);
        view.channels = channels;
        view.unsupported_channels = unsupported_channels;
    }
}

fn current_time_code(ctx: &lunco_workbench::PanelCtx, time_codes_per_second: f64) -> f64 {
    let seconds = ctx
        .resource::<AnimationPreview>()
        .and_then(|preview| ctx.get::<Playback>(preview.domain))
        .map(|playback| playback.head)
        .unwrap_or_default();
    seconds * time_codes_per_second
}

fn key_literal(channel: KeyableChannel, type_name: &str, transform: Transform) -> Option<String> {
    let literal = match channel {
        KeyableChannel::Translate => format!(
            "({}, {}, {})",
            transform.translation.x, transform.translation.y, transform.translation.z
        ),
        KeyableChannel::Scale => format!(
            "({}, {}, {})",
            transform.scale.x, transform.scale.y, transform.scale.z
        ),
        KeyableChannel::Orient => {
            let q = transform.rotation.as_dquat();
            format!("({}, {}, {}, {})", q.w, q.x, q.y, q.z)
        }
        KeyableChannel::Rotate(order) => {
            let (a, b, c) = transform.rotation.to_euler(order);
            format!(
                "({}, {}, {})",
                a.to_degrees(),
                b.to_degrees(),
                c.to_degrees()
            )
        }
    };
    normalize_value_literal(type_name, &literal).ok()
}

fn apply_animation_ops(
    ctx: &mut lunco_workbench::PanelCtx,
    view: &UsdAnimationSessionView,
    label: &str,
    ops: Vec<UsdOp>,
) {
    if ops.is_empty() {
        return;
    }
    let Some(doc) = view.doc else {
        return;
    };
    ctx.trigger(lunco_usd::commands::ApplyUsdOps {
        doc,
        parent_gen: Some(view.generation),
        label: label.to_owned(),
        ops,
    });
}

fn animation_time_label(time: f64, tcps: f64) -> String {
    format!("{:.3}s / {:.3}", time / tcps, time)
}

/// Paint key creation/removal for the selected USD prim. Playback controls stay
/// in Environment and continue to use the existing `ControlAnimation` command.
pub fn authored_animation_section(
    ui: &mut bevy_egui::egui::Ui,
    ctx: &mut lunco_workbench::PanelCtx,
    entity: Entity,
) {
    let Some(view) = ctx
        .resource::<UsdViewportState>()
        .and_then(|viewport| {
            ctx.resource::<UsdAnimationView>()
                .and_then(|views| views.focused(viewport))
        })
        .filter(|view| view.entity == Some(entity))
        .cloned()
    else {
        return;
    };
    let Some(transform) = ctx.get::<Transform>(entity).copied() else {
        return;
    };
    let time = current_time_code(ctx, view.time_codes_per_second);

    egui::CollapsingHeader::new("USD Animation")
        .default_open(false)
        .show(ui, |ui| {
            ui.label(format!(
                "Playhead: {}",
                animation_time_label(time, view.time_codes_per_second)
            ));
            if ui.button("Key current pose").clicked() {
                let ops = view
                    .channels
                    .iter()
                    .filter_map(|channel| {
                        let kind = channel.keyable?;
                        Some(UsdOp::SetTimeSample {
                            edit_target: view.edit_target.clone()?,
                            path: view.path.clone(),
                            name: channel.name.clone(),
                            type_name: channel.type_name.clone(),
                            time,
                            value: key_literal(kind, &channel.type_name, transform)?,
                        })
                    })
                    .collect();
                apply_animation_ops(ctx, &view, "Key USD pose", ops);
            }
            let removable = view.channels.iter().any(|channel| {
                channel
                    .times
                    .iter()
                    .any(|sample| sample.total_cmp(&time).is_eq())
            });
            if ui
                .add_enabled(removable, egui::Button::new("Remove key at playhead"))
                .clicked()
            {
                let Some(edit_target) = view.edit_target.clone() else {
                    return;
                };
                let ops = view
                    .channels
                    .iter()
                    .filter(|channel| {
                        channel
                            .times
                            .iter()
                            .any(|sample| sample.total_cmp(&time).is_eq())
                    })
                    .map(|channel| UsdOp::RemoveTimeSample {
                        edit_target: edit_target.clone(),
                        path: view.path.clone(),
                        name: channel.name.clone(),
                        time,
                    })
                    .collect();
                apply_animation_ops(ctx, &view, "Remove USD pose key", ops);
            }
            for channel in &view.channels {
                if channel.times.is_empty() {
                    continue;
                }
                let samples = channel
                    .times
                    .iter()
                    .map(|sample| animation_time_label(*sample, view.time_codes_per_second))
                    .collect::<Vec<_>>()
                    .join(", ");
                ui.label(format!("{}: {samples}", channel.name));
            }
            if !view.unsupported_channels.is_empty() {
                ui.colored_label(
                    egui::Color32::YELLOW,
                    format!(
                        "Uneditable xform channels: {}",
                        view.unsupported_channels.join(", ")
                    ),
                );
            }
            ui.small("Use Environment → Animation to play or scrub the USD timeline.");
        });
}
