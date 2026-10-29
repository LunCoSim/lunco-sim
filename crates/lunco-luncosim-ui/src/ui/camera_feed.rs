//! Independent camera windows over the capture owner's live render targets.
use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiTextureHandle, egui};
use lunco_capture::camera_feed::{
    CameraFeeds, CreateCameraFeed, MAX_CAMERA_FEEDS, RemoveCameraFeed, SetCameraFeed,
    SetCameraFeedResolution, SetCameraImageEffects,
};
use lunco_command_contracts::{Ack, OpId, Reject};
use lunco_core::{Command, MarkClientLocalExt, on_command, register_commands};
use lunco_core::{
    GlobalEntityId, SceneResource, entity_display_name,
    markers::{Callsign, CatalogEntryId},
};
use lunco_render::camera_image::{CameraImageEffects, CameraImageMode};
use std::collections::BTreeMap;

/// Open live image controls. Omitted source follows the selected main camera.
#[Command(default)]
pub struct ShowCameraImageSettings {
    pub source: Option<GlobalEntityId>,
}
pub(super) fn install(app: &mut App) {
    register_all_commands(app);
    app.mark_client_local::<ShowCameraImageSettings>();
}
#[on_command(ShowCameraImageSettings)]
fn show_image_settings(
    trigger: On<ShowCameraImageSettings>,
    registry: Res<lunco_api::registry::ApiEntityRegistry>,
    cameras: Query<&Projection>,
    mut state: ResMut<CameraFeedUiState>,
) -> Result<Ack, Reject> {
    if let Some(source) = cmd.source {
        if !registry
            .resolve(&source)
            .is_some_and(|entity| cameras.contains(entity))
        {
            return Err(Reject::InvalidOp(
                "Image settings require a live camera source".into(),
            ));
        }
    }
    state.main_source = cmd.source;
    state.main_open = true;
    Ok(Ack::new(OpId::new()))
}
register_commands!(show_image_settings);

#[derive(Resource, Default)]
pub(super) struct CameraFeedUiState {
    last_match_request: BTreeMap<u64, ([u32; 2], u64)>,
    shader_drafts: BTreeMap<(bool, u64), String>,
    searches: BTreeMap<u64, String>,
    main_open: bool,
    main_source: Option<GlobalEntityId>,
}
impl SceneResource for CameraFeedUiState {}

pub(super) fn draw_camera_feed(
    mut contexts: EguiContexts,
    feeds: Res<CameraFeeds>,
    images: Res<Assets<Image>>,
    theme: Res<lunco_theme::Theme>,
    cameras: Query<
        (
            &GlobalEntityId,
            &Name,
            Option<&Callsign>,
            Option<&CatalogEntryId>,
            Option<&CameraImageEffects>,
        ),
        With<Projection>,
    >,
    windows: Query<&Window, With<bevy::window::PrimaryWindow>>,
    mut state: ResMut<CameraFeedUiState>,
    mut commands: Commands,
    selection: Res<lunco_usd_bevy_camera::camera_switch::CameraSelectionStatus>,
    presentations: Query<&lunco_render::camera_image::CameraImagePresentation>,
) {
    state
        .last_match_request
        .retain(|id, _| feeds.instances.contains_key(id));
    let mut candidates: Vec<_> = cameras
        .iter()
        .map(|(id, name, callsign, catalog, _)| {
            (
                *id,
                name.as_str().to_owned(),
                entity_display_name(Some(name), callsign, catalog),
            )
        })
        .collect();
    let paths: Vec<_> = candidates.iter().map(|(_, path, _)| path.clone()).collect();
    let labels = lunco_camera_core::camera_display_labels(&paths);
    for (candidate, label) in candidates.iter_mut().zip(labels) {
        candidate.2 = label;
    }
    let owner_paths: Vec<_> = selection
        .camera_group_paths
        .values()
        .cloned()
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    let owner_labels: BTreeMap<_, _> = owner_paths
        .iter()
        .cloned()
        .zip(lunco_camera_core::camera_display_labels(&owner_paths))
        .collect();
    let group = |path: &String| {
        selection
            .camera_group_paths
            .get(path)
            .and_then(|owner| owner_labels.get(owner))
            .cloned()
            .unwrap_or_else(|| "Scene views".into())
    };
    candidates.sort_by(|a, b| {
        group(&a.1)
            .cmp(&group(&b.1))
            .then(a.2.cmp(&b.2))
            .then(a.1.cmp(&b.1))
    });
    state
        .searches
        .retain(|id, _| feeds.instances.contains_key(id));
    state.shader_drafts.retain(|(source, id), _| {
        if *source {
            cameras.iter().any(|(gid, ..)| gid.get() == *id)
        } else {
            feeds.instances.contains_key(id)
        }
    });
    for (index, (id, feed)) in feeds.instances.iter().enumerate() {
        let target = feed.image.as_ref().and_then(|handle| {
            images.get(handle).map(|image| {
                let texture = contexts.add_image(EguiTextureHandle::Weak(handle.id()));
                (texture, image.width() as f32 / image.height() as f32)
            })
        });
        let selected = candidates
            .iter()
            .find(|(source, _, _)| feed.source == Some(*source));
        let title = if feed.source.is_none() {
            format!("Camera view {id}")
        } else if feed.label.starts_with('/') {
            selected.map_or_else(|| feed.label.clone(), |(_, _, label)| label.clone())
        } else {
            feed.label.clone()
        };
        let Ok(ctx) = contexts.ctx_mut() else {
            return;
        };
        let mut open = true;
        // At most eight views: this is a bounded rendering/layout conversion.
        let column = (index % 3) as f32;
        let row = (index / 3) as f32;
        egui::Window::new(&title).id(egui::Id::new(("live-camera-feed",id)))
            .default_size(egui::vec2(420.0,300.0)).min_size(egui::vec2(360.0,180.0)).default_pos(egui::pos2(16.0+column*445.0,640.0+row*310.0))
            .title_bar(false).collapsible(false).order(lunco_theme::HudTier::ORDER)
            .frame(lunco_theme::hud_window_frame(&theme)).resizable(true).show(ctx,|ui| {
                ui.horizontal(|ui| {ui.with_layout(egui::Layout::right_to_left(egui::Align::Center),|ui| {
                    if ui.small_button("×").on_hover_text("Close this HUD").clicked(){open=false;}
                    if ui.add_enabled(feeds.instances.len()<MAX_CAMERA_FEEDS,egui::Button::new("+").small())
                        .on_hover_text("New independent camera HUD (up to eight)").clicked(){commands.trigger(CreateCameraFeed{});}
                    let mut effects=feed.effects.clone();
                    ui.menu_button("Style",|ui| {
                        let draft=state.shader_drafts.entry((false,*id)).or_insert_with(||effects.shader.clone());
                        if image_controls(ui,&mut effects,draft) {
                            commands.trigger(SetCameraImageEffects{feed:Some(*id),source:None,settings:effects.clone()});
                        }
                    });
                    let label=if feed.resolution_error.is_some(){"Res !"}else if feed.match_window{"Window"}else{"Res"};
                    let menu=ui.menu_button(label,|ui| {
                        ui.strong("Render resolution");
                        ui.weak(format!("Current: {} × {} pixels",feed.width,feed.height));
                        if let Some(error)=&feed.resolution_error {ui.colored_label(ui.visuals().error_fg_color,error);}
                        if ui.selectable_label(feed.match_window,"Match view window (live)").clicked() {
                            state.last_match_request.remove(id);
                            commands.trigger(SetCameraFeedResolution{feed:*id,match_window:true,width:None,height:None});ui.close();
                        }
                        if let Ok(window)=windows.single() {
                            let (width,height)=(window.physical_width(),window.physical_height());
                            if ui.button(format!("Use app window: {width} × {height}")).clicked() {
                                commands.trigger(SetCameraFeedResolution{feed:*id,match_window:false,width:Some(width),height:Some(height)});ui.close();
                            }
                        }
                        ui.separator();
                        for (width,height) in [(480,270),(640,360),(1280,720),(1920,1080),(2560,1440),(3840,2160)] {
                            if ui.selectable_label(!feed.match_window && feed.width==width && feed.height==height,format!("{width} × {height}")).clicked() {
                                commands.trigger(SetCameraFeedResolution{feed:*id,match_window:false,width:Some(width),height:Some(height)});ui.close();
                            }
                        }
                    });
                    menu.response.on_hover_text(feed.resolution_error.as_deref().unwrap_or("Render resolution; Window matches this view's image area in physical pixels"));
                    let selected_text=selected.map_or("Choose camera…",|(_,_,label)|label.as_str());
                    let selected_text=format!("Camera · {selected_text}");
                    egui::ComboBox::from_id_salt(("camera-picker",id)).selected_text(selected_text)
                        .width(ui.available_width()).wrap_mode(egui::TextWrapMode::Truncate).show_ui(ui,|ui| {
                        let search=state.searches.entry(*id).or_default();
                        ui.add(egui::TextEdit::singleline(search).hint_text("Find rover or camera…"));
                        let filter=search.to_lowercase();let mut previous=None;
                        for (source,path,label) in &candidates {
                            let owner=group(path);
                            if !filter.is_empty() && !format!("{owner} {path} {label}").to_lowercase().contains(&filter){continue;}
                            if previous.as_ref()!=Some(&owner){ui.separator();ui.strong(&owner);previous=Some(owner);}
                            if ui.selectable_label(feed.source==Some(*source),label).on_hover_text(path).clicked()
                                && feed.source!=Some(*source) {
                                commands.trigger(SetCameraFeed {feed:Some(*id),source:Some(*source),label:label.clone(),width:None,height:None});
                            }
                        }
                    }).response.on_hover_text(&title);
                });});
                ui.add_space(theme.spacing.window_padding);
                if feed.source.is_some() {
                    let caption=if feed.image_ready(&presentations) {
                        format!("{:?} · {} × {}",feed.effects.mode,feed.width,feed.height)
                    }else{"Preparing image…".into()};
                    ui.add(egui::Label::new(caption).truncate()).on_hover_text(
                        "Image settings must finish rendering before capture. Shader diagnostics appear in Recent Events.");
                }
                let available=ui.available_size();
                if feed.match_window && available.x>0.0 && available.y>0.0 {
                    // Logical UI points become physical render-target pixels only here.
                    let physical=available*ctx.pixels_per_point();
                    if physical.x.is_finite() && physical.y.is_finite() && physical.x<(u32::MAX as f32) && physical.y<(u32::MAX as f32) {
                        let measured=[physical.x.round() as u32,physical.y.round() as u32];
                        if measured==[feed.width,feed.height] {state.last_match_request.remove(id);}
                        else if !ui.input(|i|i.pointer.any_down()) && state.last_match_request.get(id)!=Some(&(measured,feeds.allocation_revision)) {
                            state.last_match_request.insert(*id,(measured,feeds.allocation_revision));
                            commands.trigger(SetCameraFeedResolution{feed:*id,match_window:true,width:Some(measured[0]),height:Some(measured[1])});
                        }
                    }
                } else {state.last_match_request.remove(id);}
                if let Some((texture,ratio))=target {
                    let size=if feed.match_window {available} else {let width=available.x.min(available.y*ratio);egui::vec2(width,width/ratio)};
                    if size.x>0.0 && size.y>0.0 {ui.image((texture,size));}
                } else {ui.weak("Select a camera to start this view.");}

            });
        if !open {
            commands.trigger(RemoveCameraFeed { feed: *id });
        }
    }
    if state.main_open {
        let source = cameras.iter().find(|(id, name, _, _, _)| {
            state.main_source.map_or_else(
                || selection.active_name.as_deref() == Some(name.as_str()),
                |source| source == **id,
            )
        });
        let Ok(ctx) = contexts.ctx_mut() else {
            return;
        };
        let mut open = state.main_open;
        egui::Window::new("Camera image controls")
            .id(egui::Id::new("camera-image-controls"))
            .open(&mut open)
            .default_width(390.0)
            .show(ctx, |ui| {
                if let Some((source, name, callsign, catalog, settings)) = source {
                    ui.strong(entity_display_name(Some(name), callsign, catalog));
                    ui.label(name.as_str());
                    let mut effects = settings.cloned().unwrap_or_default();
                    let draft = state
                        .shader_drafts
                        .entry((true, source.get()))
                        .or_insert_with(|| effects.shader.clone());
                    if image_controls(ui, &mut effects, draft) {
                        commands.trigger(SetCameraImageEffects {
                            feed: None,
                            source: Some(*source),
                            settings: effects,
                        });
                    }
                } else {
                    ui.label("Select a main camera to edit its image.");
                }
            });
        state.main_open = open;
    }
}

fn image_controls(
    ui: &mut egui::Ui,
    effects: &mut CameraImageEffects,
    shader: &mut String,
) -> bool {
    ui.set_max_width(390.0);
    let mut changed = false;
    ui.horizontal(|ui| {
        changed |= ui
            .selectable_value(&mut effects.mode, CameraImageMode::Color, "Color")
            .changed();
        changed |= ui
            .selectable_value(&mut effects.mode, CameraImageMode::Depth, "Depth")
            .changed();
    });
    if effects.mode == CameraImageMode::Depth {
        ui.weak("Opaque axial depth · metres · near red → yellow → far green; sky black");
        ui.horizontal(|ui| {
            ui.label("Near");
            changed |= ui
                .add(
                    egui::DragValue::new(&mut effects.near_m)
                        .speed(0.1)
                        .suffix(" m")
                        .range(0.0..=effects.far_m - 0.001),
                )
                .changed();
            ui.label("Far");
            changed |= ui
                .add(
                    egui::DragValue::new(&mut effects.far_m)
                        .speed(1.0)
                        .suffix(" m")
                        .range(effects.near_m + 0.001..=1.0e7),
                )
                .changed();
        });
    }
    ui.separator();
    ui.strong("Sensor noise");
    for (label, value) in [
        ("Read noise σ", &mut effects.read_noise),
        ("Shot noise", &mut effects.shot_noise),
        ("Row noise", &mut effects.row_noise),
        ("Dropout fraction", &mut effects.dropout),
    ] {
        changed |= ui
            .add(
                egui::Slider::new(value, 0.0..=1.0)
                    .text(label)
                    .fixed_decimals(3),
            )
            .changed();
    }
    changed |= ui
        .checkbox(&mut effects.animated, "Animate noise")
        .changed();
    ui.horizontal(|ui| {
        ui.label("Seed");
        changed |= ui.add(egui::DragValue::new(&mut effects.seed)).changed();
    });
    let mut quantized = effects.quantization_levels > 0;
    if ui.checkbox(&mut quantized, "Quantize intensity").changed() {
        effects.quantization_levels = if quantized { 256 } else { 0 };
        changed = true;
    }
    if quantized {
        changed |= ui
            .add(
                egui::DragValue::new(&mut effects.quantization_levels)
                    .range(2..=65536)
                    .suffix(" levels"),
            )
            .changed();
    }
    if ui.button("Clear noise").clicked() {
        effects.read_noise = 0.0;
        effects.shot_noise = 0.0;
        effects.row_noise = 0.0;
        effects.dropout = 0.0;
        effects.quantization_levels = 0;
        changed = true;
    }
    ui.collapsing("Shader workflow", |ui| {
        ui.weak("Edit a library WESL shader, then ReloadShader to update live views.");
        ui.text_edit_singleline(shader);
        if ui.button("Apply shader asset").clicked() {
            effects.shader = shader.clone();
            changed = true;
        }
    });
    if let Err(error) = effects.validate() {
        ui.colored_label(ui.visuals().error_fg_color, error);
        return false;
    }
    changed
}
