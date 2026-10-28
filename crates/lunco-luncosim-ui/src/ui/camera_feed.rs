//! Independent camera windows over the capture owner's live render targets.
use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiTextureHandle, egui};
use lunco_capture::camera_feed::{
    CameraFeeds, CreateCameraFeed, MAX_CAMERA_FEEDS, RemoveCameraFeed, SetCameraFeed,
    SetCameraFeedResolution,
};
use lunco_core::{
    GlobalEntityId, SceneResource, entity_display_name, humanize_identifier,
    markers::{Callsign, CatalogEntryId},
};
use std::collections::BTreeMap;

#[derive(Resource, Default)]
pub(super) struct CameraFeedUiState {
    last_match_request: BTreeMap<u64, ([u32; 2], u64)>,
}
impl SceneResource for CameraFeedUiState {}

pub(super) fn draw_camera_feed(
    mut contexts: EguiContexts,
    feeds: Res<CameraFeeds>,
    images: Res<Assets<Image>>,
    cameras: Query<
        (
            &GlobalEntityId,
            &Name,
            Option<&Callsign>,
            Option<&CatalogEntryId>,
        ),
        With<Projection>,
    >,
    windows: Query<&Window, With<bevy::window::PrimaryWindow>>,
    mut state: ResMut<CameraFeedUiState>,
    mut commands: Commands,
) {
    state
        .last_match_request
        .retain(|id, _| feeds.instances.contains_key(id));
    let mut candidates: Vec<_> = cameras
        .iter()
        .map(|(id, name, callsign, catalog)| {
            (
                *id,
                name.as_str().to_owned(),
                entity_display_name(Some(name), callsign, catalog),
            )
        })
        .collect();
    let mut counts = BTreeMap::new();
    for (_, _, label) in &candidates {
        *counts.entry(label.clone()).or_insert(0usize) += 1;
    }
    for (_, path, label) in &mut candidates {
        if counts.get(label).is_some_and(|count| *count > 1) {
            if let Some(parent) = path.rsplit('/').nth(1) {
                *label = format!("{} · {label}", humanize_identifier(parent));
            }
        }
    }
    candidates.sort_by(|a, b| {
        a.2.cmp(&b.2)
            .then(a.1.cmp(&b.1))
            .then(a.0.get().cmp(&b.0.get()))
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
        egui::Window::new(title).id(egui::Id::new(("live-camera-feed",id)))
            .default_size(egui::vec2(360.0,270.0)).min_size(egui::vec2(300.0,180.0)).default_pos(egui::pos2(16.0+column*385.0,660.0+row*270.0))
            .open(&mut open).resizable(true).show(ctx,|ui| {
                ui.horizontal(|ui| {ui.with_layout(egui::Layout::right_to_left(egui::Align::Center),|ui| {
                    if ui.add_enabled(feeds.instances.len()<MAX_CAMERA_FEEDS,egui::Button::new("+ View"))
                        .on_hover_text("Open another camera window (up to eight)").clicked(){commands.trigger(CreateCameraFeed{});}
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
                    egui::ComboBox::from_id_salt(("camera-picker",id)).selected_text(selected_text)
                        .width(ui.available_width()).wrap_mode(egui::TextWrapMode::Truncate).show_ui(ui,|ui| {
                        for (source,path,label) in &candidates {
                            if ui.selectable_label(feed.source==Some(*source),label).on_hover_text(path).clicked()
                                && feed.source!=Some(*source) {
                                commands.trigger(SetCameraFeed {feed:Some(*id),source:Some(*source),label:label.clone(),width:None,height:None});
                            }
                        }
                    });
                });});
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
}
