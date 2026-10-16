//! Shared resource-link presentation; registered asset byte loaders own I/O.

use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPrimaryContextPass, egui};
use lunco_workbench_core::commands::OpenAssetResource;
use lunco_workbench_core::uri::{UriClicked, UriResolution};

const TEXT_SCROLL: &str = "asset-resource-text";

#[derive(Resource, Default)]
struct ResourcePreview {
    uri: Option<String>,
    rejection: Option<String>,
}

#[lunco_core::on_command(OpenAssetResource)]
fn open_resource(
    trigger: On<OpenAssetResource>,
    mut preview: ResMut<ResourcePreview>,
    mut contexts: EguiContexts,
) {
    preview.uri = Some(trigger.event().uri.clone());
    preview.rejection = None;
    if let Ok(ctx) = contexts.ctx_mut() {
        lunco_assets_runtime::ui_loader::invalidate_resource_image(ctx, &trigger.event().uri);
    }
}
lunco_core::register_commands!(open_resource,);

fn on_link(trigger: On<UriClicked>, mut preview: ResMut<ResourcePreview>, mut commands: Commands) {
    match &trigger.event().resolution {
        UriResolution::OpenResource { uri } => {
            commands.trigger(OpenAssetResource { uri: uri.clone() });
        }
        UriResolution::Rejected { message } => {
            preview.uri = Some(trigger.event().uri.clone());
            preview.rejection = Some(message.clone());
        }
        _ => {}
    }
}

fn retire_preview(
    _trigger: On<lunco_workspace::TwinClosed>,
    mut preview: ResMut<ResourcePreview>,
    mut contexts: EguiContexts,
) {
    *preview = ResourcePreview::default();
    if let Ok(ctx) = contexts.ctx_mut() {
        lunco_assets_runtime::ui_loader::invalidate_resource_images(ctx);
    }
}

fn preview_available_size(ui: &egui::Ui) -> egui::Vec2 {
    // Scroll content may offer infinite height; previews use the visible body.
    ui.available_rect_before_wrap()
        .intersect(ui.clip_rect())
        .size()
        .max(egui::Vec2::ZERO)
}

fn draw(
    mut contexts: EguiContexts,
    mut preview: ResMut<ResourcePreview>,
    server: Res<bevy::asset::AssetServer>,
    settings: Res<lunco_assets_runtime::ui_loader::AssetUiSettings>,
    mut installed: Local<bool>,
    mut commands: Commands,
) {
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    if !*installed {
        lunco_assets_runtime::ui_loader::install_image_decoders(ctx);
        for scheme in [
            lunco_assets_core::LUNCO_SCHEME,
            lunco_assets_core::TWIN_SCHEME,
            lunco_assets_runtime::library::LIBRARY_SCHEME,
        ] {
            ctx.add_bytes_loader(std::sync::Arc::new(
                lunco_assets_runtime::ui_loader::AssetBytesLoader::new(
                    scheme,
                    server.clone(),
                    settings.max_resource_bytes,
                    |uri| {
                        lunco_assets_core::asset_path::load_asset_path(uri, None, None, None)
                            .map(lunco_assets_core::registered_read::AssetReadTarget::Registered)
                            .map_err(|error| error.to_string())
                    },
                ),
            ));
        }
        ctx.add_bytes_loader(std::sync::Arc::new(
            lunco_assets_runtime::ui_loader::AssetBytesLoader::new(
                "file",
                server.clone(),
                settings.max_resource_bytes,
                |uri| {
                    let path = lunco_storage::file_uri_to_path(uri)
                        .map_err(|error| error.to_string())?
                        .ok_or("not a file URI")?;
                    Ok(lunco_assets_core::registered_read::AssetReadTarget::File(
                        path,
                    ))
                },
            ),
        ));
        *installed = true;
    }
    let Some(uri) = preview.uri.clone() else {
        return;
    };
    let reset_scroll = preview.is_changed();
    let mut open = true;
    egui::Window::new("Resource")
        .id(egui::Id::new("asset-resource-preview"))
        .default_pos(ctx.content_rect().center())
        .pivot(egui::Align2::CENTER_CENTER)
        .constrain_to(ctx.content_rect())
        .vscroll(true)
        .open(&mut open)
        .show(ctx, |ui| {
            if reset_scroll {
                ui.scroll_to_cursor(Some(egui::Align::TOP));
                // Reset stable scroll owners before bytes arrive, including
                // text whose first ready frame follows asynchronous loading.
                let text_scroll_id = ui.make_persistent_id(egui::IdSalt::new(TEXT_SCROLL));
                egui::scroll_area::State::default().store(ctx, text_scroll_id);
            }
            ui.add_sized(
                egui::vec2(preview_available_size(ui).x, ui.spacing().interact_size.y),
                egui::Label::new(&uri)
                    .truncate()
                    .selectable(true)
                    .halign(egui::Align::LEFT),
            )
            .context_menu(|ui| {
                if ui.button("Copy URI").clicked() {
                    ui.ctx().copy_text(uri.clone());
                    ui.close();
                }
            });
            if ui.button("Reload").clicked() {
                commands.trigger(OpenAssetResource { uri: uri.clone() });
            }
            if let Some(error) = &preview.rejection {
                ui.label(error);
                return;
            }
            match ctx.try_load_bytes(&uri) {
                Ok(egui::load::BytesPoll::Pending { .. }) => {
                    ui.spinner();
                    ui.label("Loading resource…");
                }
                Ok(egui::load::BytesPoll::Ready { bytes, .. }) => {
                    let prefix =
                        &bytes.as_ref()[..bytes.len().min(settings.max_text_preview_bytes)];
                    let text = std::str::from_utf8(prefix).or_else(|error| {
                        if error.error_len().is_none() {
                            std::str::from_utf8(&prefix[..error.valid_up_to()])
                        } else {
                            Err(error)
                        }
                    });
                    if let Ok(text) = text {
                        if text.len() < bytes.len() {
                            ui.label(format!("Showing {} of {} bytes", text.len(), bytes.len()));
                        }
                        egui::ScrollArea::both()
                            .id_salt(TEXT_SCROLL)
                            .max_height(preview_available_size(ui).y)
                            .show(ui, |ui| {
                                ui.add(egui::Label::new(text).selectable(true));
                            });
                    } else {
                        let available = preview_available_size(ui);
                        let image = egui::Image::new(uri.as_str()).max_size(available);
                        match image.load_for_size(ctx, available) {
                            Ok(egui::load::TexturePoll::Ready { .. }) => {
                                ui.add(image.shrink_to_fit());
                            }
                            Ok(egui::load::TexturePoll::Pending { .. }) => {
                                ui.spinner();
                            }
                            Err(error) => {
                                ui.label(format!(
                                    "{} bytes. Preview unavailable: {error}",
                                    bytes.len()
                                ));
                            }
                        }
                    }
                }
                Err(error) => {
                    ui.label(format!("Could not load resource: {error}"));
                }
            }
        });
    if !open {
        *preview = ResourcePreview::default();
    }
}

pub(super) fn install(app: &mut App) {
    {
        use lunco_settings::AppSettingsExt;
        app.register_settings_section::<lunco_assets_runtime::ui_loader::AssetUiSettings>();
    }
    app.init_resource::<ResourcePreview>()
        .add_observer(on_link)
        .add_observer(retire_preview)
        .add_systems(
            EguiPrimaryContextPass,
            draw.after(lunco_workbench_core::WorkbenchRenderSet),
        );
    register_all_commands(app);
}
