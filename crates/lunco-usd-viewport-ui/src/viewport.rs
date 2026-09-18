//! Workbench panels for the USD preview render runtime.

use bevy::prelude::*;
use egui;
use lunco_doc::DocumentId;
use lunco_doc_bevy::DocumentRegistry;
use lunco_usd_document::document::UsdDocument;
use lunco_usd_viewport_core::{
    ApplyUsdInspectionPreset, CloseUsdPreviewView, DeleteUsdInspectionPreset, FocusUsdPreviewView,
    FrameUsdPreviewView, OpenUsdPreviewView, ResetUsdPreviewView, SaveUsdInspectionPreset,
    SetUsdPreviewProjection, SetUsdPreviewTextLayer, SetUsdPreviewViewMode, UsdInspectionSettings,
    UsdPreviewId, UsdPreviewProjection, UsdPreviewTextLayer, UsdPreviewViewId, UsdPreviewViewMode,
    UsdViewportState,
};
use lunco_usd_viewport_runtime::{
    preview_drag_channels, UsdPreviewRenderTargets, UsdPreviewViewMeasured, UsdViewportClick,
    UsdViewportMeasured, UsdViewportOrbitInput, USD_PREVIEW_VIEW_PANEL_ID, USD_VIEWPORT_PANEL_ID,
};
use lunco_viewport_core::PanelRect;
use lunco_workbench_core::scene_pick::ScenePickGate;
use lunco_workbench_core::{
    commands::CloseTab, InstancePanel, Panel, PanelCtx, PanelId, PanelRenderTarget,
    PanelScrollPolicy, PanelSlot,
};

// UsdViewportPanel
// ─────────────────────────────────────────────────────────────────────

/// Workbench panel displaying the focused USD preview view. Other sessions and
/// dockable views remain independently addressable and editable.
pub(crate) struct UsdViewportPanel;

impl Panel for UsdViewportPanel {
    fn id(&self) -> PanelId {
        USD_VIEWPORT_PANEL_ID
    }

    fn title(&self) -> String {
        "USD Preview".to_string()
    }

    fn menu_group(&self) -> lunco_workbench_core::PanelMenuGroup {
        lunco_workbench_core::PanelMenuGroup::Scene
    }

    fn default_slot(&self) -> PanelSlot {
        PanelSlot::Center
    }

    fn scene_target(&self) -> Option<PanelRenderTarget> {
        // This is NOT the full-window 3D scene: it renders a camera to an offscreen
        // `Image` and shows it as an `egui::Image` with its own `click_and_drag`
        // orbit handling (below). Declaring `MainViewport` here made every drag over
        // the preview ALSO drive the main avatar camera, and let `bevy_picking` mesh
        // hits fire in the main scene *behind* the image. As an `Offscreen` target it
        // owns its own input and the gate keeps the main scene out of it — while the
        // dock dispatch still records it as an opaque blocked region (it has the
        // default opaque background), so nothing leaks through.
        Some(PanelRenderTarget::Offscreen(USD_VIEWPORT_PANEL_ID))
    }

    fn closable(&self) -> bool {
        false
    }

    fn scroll_policy(&self) -> PanelScrollPolicy {
        PanelScrollPolicy::SelfManaged
    }

    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut PanelCtx) {
        let Some(view) = ctx
            .resource::<UsdViewportState>()
            .and_then(UsdViewportState::focused_view_id)
        else {
            ui.centered_and_justified(|ui| ui.label("No USD preview is open."));
            return;
        };
        render_preview_view(ui, ctx, view, true);
    }
}

/// Render one view through either the focused singleton surface or an
/// instance tab. The USD stage and editor state are read-only here; all focus,
/// sizing, and orbit changes are emitted as typed intents after paint.
fn render_preview_view(
    ui: &mut egui::Ui,
    ctx: &mut PanelCtx,
    view_id: UsdPreviewViewId,
    singleton: bool,
) {
    let tex_id = ctx
        .resource::<UsdPreviewRenderTargets>()
        .and_then(|targets| targets.texture_id(view_id));
    let (focused_doc, next_view, projection, mode, text_layer, active_preset) = ctx
        .resource::<UsdViewportState>()
        .and_then(|state| {
            let view = state.view(view_id)?;
            let session = state.session(view.preview())?;
            let next_view = singleton
                .then(|| state.next_view_id())
                .flatten()
                .map(|view| (session.id(), view));
            Some((
                Some(session.doc()),
                next_view,
                view.projection(),
                view.mode(),
                view.text_layer(),
                view.active_preset().map(str::to_owned),
            ))
        })
        .unwrap_or_else(|| {
            (
                None,
                None,
                UsdPreviewProjection::default(),
                UsdPreviewViewMode::default(),
                UsdPreviewTextLayer::default(),
                None,
            )
        });
    let name = focused_doc
        .and_then(|doc| {
            ctx.resource::<DocumentRegistry<UsdDocument>>()
                .and_then(|registry| registry.host(doc))
                .map(|host| host.document().origin().display_name())
        })
        .unwrap_or_else(|| "(no stage)".to_string());

    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(&name).strong());
        if ui
            .selectable_label(mode == UsdPreviewViewMode::Visual, "Visual")
            .on_hover_text("Visual — render the composed USD stage")
            .clicked()
        {
            ctx.trigger(SetUsdPreviewViewMode {
                view: view_id,
                mode: UsdPreviewViewMode::Visual,
            });
        }
        if ui
            .selectable_label(mode == UsdPreviewViewMode::Text, "Text")
            .on_hover_text("Text — inspect authored or composed USDA")
            .clicked()
        {
            ctx.trigger(SetUsdPreviewViewMode {
                view: view_id,
                mode: UsdPreviewViewMode::Text,
            });
        }
        if mode == UsdPreviewViewMode::Visual {
            if ui
                .selectable_label(
                    projection == UsdPreviewProjection::Perspective,
                    "Perspective",
                )
                .clicked()
            {
                ctx.trigger(SetUsdPreviewProjection {
                    view: view_id,
                    projection: UsdPreviewProjection::Perspective,
                });
            }
            if ui
                .selectable_label(
                    projection == UsdPreviewProjection::Orthographic,
                    "Orthographic",
                )
                .clicked()
            {
                ctx.trigger(SetUsdPreviewProjection {
                    view: view_id,
                    projection: UsdPreviewProjection::Orthographic,
                });
            }
            if ui.button("Frame").clicked() {
                ctx.trigger(FrameUsdPreviewView { view: view_id });
            }
            if ui.button("Reset").clicked() {
                ctx.trigger(ResetUsdPreviewView { view: view_id });
            }

            let preset_names = ctx
                .resource::<UsdInspectionSettings>()
                .map(|settings| {
                    settings
                        .presets
                        .iter()
                        .map(|preset| preset.name.clone())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let preset_id = egui::Id::new(("usd-inspection-preset-name", view_id.0));
            let mut preset_name = ui
                .data_mut(|data| data.get_temp::<String>(preset_id))
                .unwrap_or_else(|| {
                    active_preset
                        .clone()
                        .unwrap_or_else(|| "inspection".to_string())
                });
            ui.add(
                egui::TextEdit::singleline(&mut preset_name)
                    .desired_width(110.0)
                    .hint_text("preset name"),
            );
            ui.data_mut(|data| data.insert_temp(preset_id, preset_name.clone()));
            if ui.button("Save view").clicked() {
                ctx.trigger(SaveUsdInspectionPreset {
                    view: view_id,
                    name: preset_name.clone(),
                });
            }
            if !preset_names.is_empty() {
                let selected = active_preset
                    .as_deref()
                    .filter(|name| preset_names.iter().any(|candidate| candidate == name))
                    .unwrap_or("Presets");
                egui::ComboBox::from_id_salt(("usd-inspection-presets", view_id.0))
                    .selected_text(selected)
                    .show_ui(ui, |ui| {
                        for name in &preset_names {
                            if ui
                                .selectable_label(active_preset.as_deref() == Some(name), name)
                                .clicked()
                            {
                                ctx.trigger(ApplyUsdInspectionPreset {
                                    view: view_id,
                                    name: name.clone(),
                                });
                            }
                        }
                    });
                if ui.button("Delete").clicked() {
                    ctx.trigger(DeleteUsdInspectionPreset { name: preset_name });
                }
            }
        }
        if let Some((preview, view)) = next_view {
            if ui.button("Open view").clicked() {
                ctx.trigger(OpenUsdPreviewView { preview, view });
            }
        }
    });
    if mode == UsdPreviewViewMode::Visual {
        ui.small("L-drag pan · gizmo handles edit · R-drag orbit · M-drag pan · wheel zoom");
    }
    ui.separator();

    if mode == UsdPreviewViewMode::Text {
        if singleton {
            ctx.trigger(UsdViewportMeasured {
                view: view_id,
                over_scene: false,
                visible: false,
                image_rect: None,
            });
        } else {
            ctx.trigger(UsdPreviewViewMeasured {
                view: view_id,
                over_scene: false,
                visible: false,
                image_rect: None,
            });
        }
        render_preview_text(ui, ctx, view_id, focused_doc, text_layer);
        return;
    }

    let Some(tex_id) = tex_id else {
        ui.centered_and_justified(|ui| {
            ui.label(
                egui::RichText::new("The USD preview is still being projected.")
                    .weak()
                    .italics(),
            );
        });
        return;
    };

    let size = ui.available_size();
    let response = ui.add(
        egui::Image::new(egui::load::SizedTexture::new(tex_id, size))
            .sense(egui::Sense::click_and_drag()),
    );
    let image_rect = panel_rect_from_egui(response.rect, ui.ctx());
    let over_scene = ui.rect_contains_pointer(response.rect);
    if singleton {
        ctx.trigger(UsdViewportMeasured {
            view: view_id,
            over_scene,
            visible: true,
            image_rect: Some(image_rect),
        });
    } else {
        ctx.trigger(UsdPreviewViewMeasured {
            view: view_id,
            over_scene,
            visible: true,
            image_rect: Some(image_rect),
        });
        // Selecting a dock tab is a view-focus action. The instance renderer
        // publishes that choice so all native editor panels follow the same
        // view/session binding.
        let already_focused = ctx
            .resource::<UsdViewportState>()
            .is_some_and(|state| state.focused_view_id() == Some(view_id));
        if !already_focused {
            ctx.trigger(FocusUsdPreviewView { view: view_id });
        }
    }

    if response.clicked_by(egui::PointerButton::Primary) {
        if let Some(pointer) = response.interact_pointer_pos() {
            let modifiers = ui.ctx().input(|input| input.modifiers);
            ctx.trigger(UsdViewportClick {
                view: view_id,
                position: Vec2::new(
                    pointer.x - response.rect.min.x,
                    pointer.y - response.rect.min.y,
                ),
                viewport_size: Vec2::new(response.rect.width(), response.rect.height()),
                shift: modifiers.shift,
                ctrl: modifiers.ctrl,
            });
        }
    }

    let gizmo_pointer_capture = ctx
        .resource::<ScenePickGate>()
        .is_some_and(ScenePickGate::tool_pointer_capture);
    let (drag, pan) = if response.dragged() {
        let shift = ui.ctx().input(|input| input.modifiers.shift);
        let (orbit, pan) = preview_drag_channels(
            response.dragged_by(egui::PointerButton::Primary),
            response.dragged_by(egui::PointerButton::Middle),
            response.dragged_by(egui::PointerButton::Secondary),
            shift,
            gizmo_pointer_capture,
        );
        let delta = response.drag_delta();
        (
            if orbit { delta } else { egui::Vec2::ZERO },
            if pan { delta } else { egui::Vec2::ZERO },
        )
    } else {
        (egui::Vec2::ZERO, egui::Vec2::ZERO)
    };
    let scroll_y = if response.hovered() || response.contains_pointer() {
        ui.ctx().input(|input| input.smooth_scroll_delta.y)
    } else {
        0.0
    };
    if drag != egui::Vec2::ZERO || pan != egui::Vec2::ZERO || scroll_y != 0.0 {
        ctx.trigger(UsdViewportOrbitInput {
            view: view_id,
            drag,
            pan,
            viewport_size: response.rect.size(),
            scroll_y,
        });
    }
}

/// Convert the exact image rectangle painted by egui to the physical-pixel
/// rectangle used by the preview camera and the gizmo frontend.
fn panel_rect_from_egui(rect: egui::Rect, ctx: &egui::Context) -> PanelRect {
    let ppp = ctx.pixels_per_point().max(f32::EPSILON);
    let min = rect.min * ppp;
    let max = rect.max * ppp;
    let min_x = min.x.max(0.0).floor() as u32;
    let min_y = min.y.max(0.0).floor() as u32;
    let max_x = max.x.max(min.x).ceil() as u32;
    let max_y = max.y.max(min.y).ceil() as u32;
    PanelRect {
        origin: UVec2::new(min_x, min_y),
        size: UVec2::new(
            max_x.saturating_sub(min_x).max(1),
            max_y.saturating_sub(min_y).max(1),
        ),
    }
}

/// Render the text view over the same preview session. The text is a
/// generation-matched snapshot of the document's authored layer or the
/// composed preview layer; this surface is deliberately read-only because
/// document mutations belong to typed USD commands and the existing source
/// editor owns direct text edits.
fn render_preview_text(
    ui: &mut egui::Ui,
    ctx: &mut PanelCtx,
    view_id: UsdPreviewViewId,
    doc: Option<DocumentId>,
    text_layer: UsdPreviewTextLayer,
) {
    let Some(doc) = doc else {
        ui.centered_and_justified(|ui| ui.label("No USD document is open."));
        return;
    };
    let edit_target = ctx
        .resource::<UsdViewportState>()
        .and_then(|state| {
            let view = state.view(view_id)?;
            state_session_edit_target(state, view.preview())
        })
        .unwrap_or("@root@");
    let Some((source_path, read_only, dirty, generation, edit_target)) = ctx
        .resource::<DocumentRegistry<UsdDocument>>()
        .and_then(|registry| registry.host(doc))
        .map(|host| {
            let document = host.document();
            let source_path = document
                .origin()
                .canonical_path()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| format!("mem://{}", document.origin().display_name()));
            (
                source_path,
                document.origin().is_read_only(),
                document.is_dirty(),
                host.generation(),
                edit_target,
            )
        })
    else {
        ui.centered_and_justified(|ui| ui.label("The USD document is no longer available."));
        return;
    };

    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Source").strong());
        ui.label(egui::RichText::new(source_path).monospace().weak());
        ui.separator();
        ui.label(egui::RichText::new(format!("Layer: {edit_target}")).weak());
        ui.separator();
        ui.label(
            egui::RichText::new(if read_only {
                "Read-only"
            } else {
                "Editable document · text view is read-only"
            })
            .weak(),
        );
        ui.separator();
        ui.label(egui::RichText::new(if dirty { "Unsaved" } else { "Saved" }).weak());
    });
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Text layer").weak());
        if ui
            .selectable_label(text_layer == UsdPreviewTextLayer::Authored, "Authored")
            .on_hover_text("Authored — the document layer used for Save")
            .clicked()
        {
            ctx.trigger(SetUsdPreviewTextLayer {
                view: view_id,
                layer: UsdPreviewTextLayer::Authored,
            });
        }
        if ui
            .selectable_label(text_layer == UsdPreviewTextLayer::Composed, "Composed")
            .on_hover_text("Composed — the source snapshot rendered by Visual mode")
            .clicked()
        {
            ctx.trigger(SetUsdPreviewTextLayer {
                view: view_id,
                layer: UsdPreviewTextLayer::Composed,
            });
        }
    });
    ui.separator();

    let _ = ctx.resource_scope::<UsdViewportState, _>(|_, state| {
        let Some(view) = state.view(view_id) else {
            ui.label("This preview view is no longer available.");
            return;
        };
        let Some(session) = state.session(view.preview()) else {
            ui.label("This preview session is no longer available.");
            return;
        };
        let fresh = session.text.displayed_generation == Some(generation)
            && session.text.requested_generation == Some(generation);
        if !fresh {
            if let Some(error) = &session.text.error {
                ui.label(egui::RichText::new(error).weak());
            } else if session.text.loading {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(egui::RichText::new("Loading USD text…").weak().italics());
                });
            } else {
                ui.label(egui::RichText::new("USD text is not available yet.").weak());
            }
            return;
        }
        let text = match text_layer {
            UsdPreviewTextLayer::Authored => session.text.authored.as_deref(),
            UsdPreviewTextLayer::Composed => session.text.composed.as_deref(),
        };
        let Some(text) = text else {
            ui.label(egui::RichText::new("USD text is not available yet.").weak());
            return;
        };
        let mut text = text;
        egui::ScrollArea::both()
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                ui.add(
                    lunco_workbench_widgets::text_editor::code(&mut text)
                        .desired_width(f32::INFINITY)
                        .interactive(false),
                );
            });
    });
}

fn state_session_edit_target(state: &UsdViewportState, preview: UsdPreviewId) -> Option<&str> {
    state
        .session(preview)
        .map(|session| session.edit_target.as_str())
}

/// A dockable view over one existing USD preview session. Multiple instances
/// share the session's projected stage but render through independent cameras.
pub(crate) struct UsdPreviewViewPanel;

impl InstancePanel for UsdPreviewViewPanel {
    fn kind(&self) -> PanelId {
        USD_PREVIEW_VIEW_PANEL_ID
    }

    fn default_slot(&self) -> PanelSlot {
        PanelSlot::Center
    }

    fn title(&self, world: &World, instance: u64) -> String {
        let view = UsdPreviewViewId(instance);
        let name = world
            .get_resource::<UsdViewportState>()
            .and_then(|state| state.view(view))
            .and_then(|view| {
                world.resource::<DocumentRegistry<UsdDocument>>().host(
                    world
                        .resource::<UsdViewportState>()
                        .session(view.preview())?
                        .doc(),
                )
            })
            .map(|host| host.document().origin().display_name())
            .unwrap_or_else(|| "USD".to_string());
        format!("{name} · View {instance}")
    }

    fn scroll_policy(&self) -> PanelScrollPolicy {
        PanelScrollPolicy::SelfManaged
    }

    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut PanelCtx, instance: u64) {
        render_preview_view(ui, ctx, UsdPreviewViewId(instance), false);
    }

    fn tab_context_menu(&mut self, ui: &mut egui::Ui, ctx: &mut PanelCtx, instance: u64) {
        if ui.button("Close view").clicked() {
            let view = UsdPreviewViewId(instance);
            ctx.trigger(CloseUsdPreviewView { view });
            ctx.trigger(CloseTab {
                kind: USD_PREVIEW_VIEW_PANEL_ID,
                instance,
            });
        }
    }
}
