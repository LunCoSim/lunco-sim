//! Workbench layout composition and dock rendering.

use super::*;
use lunco_workbench_core::trigger_or_defer;

pub(super) fn render_layout(
    ctx: &egui::Context,
    layout: &mut WorkbenchLayout,
    world: &mut World,
    theme: &lunco_theme::Theme,
    menus: &WorkbenchMenuRegistry,
) {
    // ── Clean capture ───────────────────────────────────────────────
    // A frame the offline recorder is capturing is a FILM frame: the whole
    // workbench chrome — menu/title bar, status strip (and its FPS readout),
    // activity bar, dock — must not be burnt into the footage. Skipping this
    // pass while a recording is active leaves the 3D view full-bleed; the
    // deliberate overlays (input HUD, vessel telemetry, notifications) are
    // drawn by their own systems and survive. Scoped to `active`, so the
    // editor comes back the instant the recorder stops — and between shots,
    // where no frame is captured, any flicker is invisible in the footage.
    // The offline recorder is driven over the command surface, so it — and with it
    // the question "is a frame being captured?" — exists only under `api`.
    #[cfg(feature = "api")]
    if world
        .get_resource::<lunco_capture::screenshot::OfflineRecordingState>()
        .is_some_and(|r| r.active)
        && !world
            .get_resource::<OfflineRecordingPresentation>()
            .is_some_and(|p| p.retain_workbench_chrome)
    {
        return;
    }

    // ── Edge resize (custom-decorations only) ───────────────────────
    // Bevy's `decorations: false` strips the WM resize handles too, so
    // we re-implement them: when the pointer hovers an N-pixel border,
    // swap the cursor to the right resize icon and forward press to
    // winit's `start_drag_resize`. Skipped on macOS, where the OS
    // titlebar still owns the window frame.
    #[cfg(not(target_os = "macos"))]
    {
        const RESIZE_BORDER: f32 = 6.0;
        let screen = ctx.content_rect();
        let pointer = ctx.input(|i| i.pointer.hover_pos());
        if let Some(p) = pointer {
            let dx = if p.x < screen.left() + RESIZE_BORDER {
                -1
            } else if p.x > screen.right() - RESIZE_BORDER {
                1
            } else {
                0
            };
            let dy = if p.y < screen.top() + RESIZE_BORDER {
                -1
            } else if p.y > screen.bottom() - RESIZE_BORDER {
                1
            } else {
                0
            };
            use bevy::math::CompassOctant;
            let dir = match (dx, dy) {
                (-1, -1) => Some(CompassOctant::NorthWest),
                (0, -1) => Some(CompassOctant::North),
                (1, -1) => Some(CompassOctant::NorthEast),
                (1, 0) => Some(CompassOctant::East),
                (1, 1) => Some(CompassOctant::SouthEast),
                (0, 1) => Some(CompassOctant::South),
                (-1, 1) => Some(CompassOctant::SouthWest),
                (-1, 0) => Some(CompassOctant::West),
                _ => None,
            };
            if let Some(dir) = dir {
                ctx.set_cursor_icon(match dir {
                    CompassOctant::North => egui::CursorIcon::ResizeNorth,
                    CompassOctant::South => egui::CursorIcon::ResizeSouth,
                    CompassOctant::East => egui::CursorIcon::ResizeEast,
                    CompassOctant::West => egui::CursorIcon::ResizeWest,
                    CompassOctant::NorthEast => egui::CursorIcon::ResizeNorthEast,
                    CompassOctant::NorthWest => egui::CursorIcon::ResizeNorthWest,
                    CompassOctant::SouthEast => egui::CursorIcon::ResizeSouthEast,
                    CompassOctant::SouthWest => egui::CursorIcon::ResizeSouthWest,
                });
                if ctx.input(|i| i.pointer.primary_pressed()) {
                    if let Ok(mut w) = world
                        .query_filtered::<&mut bevy::window::Window, bevy::prelude::With<bevy::window::PrimaryWindow>>()
                        .single_mut(world)
                    {
                        w.start_drag_resize(dir);
                    }
                }
            }
        }
    }

    // ── Opaque-mode backdrop (must run first) ───────────────────────
    // Paint `get_panel_backdrop(theme)` on the background layer BEFORE
    // any panel shapes. egui draws within a layer in shape-issue order,
    // so a rect_filled issued AFTER the menu bar / dock / status bar
    // would paint over them — exactly the "invisible menu" regression
    // the opaque-backdrop change once introduced. Running it first
    // keeps the fill underneath.
    //
    // The trigger is "are there dock tabs?", not "any registered panel
    // transparent?". The latter included transparent side-panels
    // (Inspector, Spawn Palette, …) registered globally but unused in
    // the current perspective, suppressing the backdrop incorrectly
    // and letting the 3D camera bleed through Welcome in
    // modelica_analyze. The dock-tabs check matches the 3D-app vs
    // dock-app branch below — dock mode wants an opaque backdrop;
    // 3D-app mode leaves the centre transparent for Bevy to render
    // through.
    // Backdrop strategy (egui paints over the 3D framebuffer, alpha-
    // blended; only Camera3d's viewport rect is left transparent so
    // 3D shows). Three cases:
    //   - View (empty layout)  → no backdrop. Camera3d paints full
    //     window; chrome (menu/status) overpaints on top.
    //   - Design (no ViewportPanel) → full-window backdrop. Camera3d
    //     is inactive; backdrop fills the framebuffer so no garbage.
    //   - Build (ViewportPanel in layout) → backdrop EVERYWHERE
    //     EXCEPT the ViewportPanel rect. Painted as four strips
    //     around the rect so the dock-leaf gaps (tab-strip header
    //     above the panel, padding below) match theme instead of
    //     showing uncleared framebuffer pixels as a black hole.
    // Only a chrome-only perspective (no ViewportPanel and no full-window
    // scene contract) needs a full-window backdrop to fill the framebuffer —
    // Camera3d is inactive there. A scene-backed perspective keeps Camera3d
    // running full-window; egui chrome opaquely overlays where panels are and
    // the rest stays transparent so 3D shows through (including dock-leaf
    // gaps).
    // An active placeholder message means the scene is empty — and so the USD
    // avatar `Camera3d` was despawned. View mode (empty layout) normally skips
    // the backdrop because `Camera3d` paints the full window; with no camera
    // that assumption breaks and the *last rendered frame* (stale rovers) would
    // show through. The selected camera's actual render state is the
    // authoritative signal: cover the framebuffer until that camera is
    // active, even before the domain placeholder resource catches up after
    // deferred scene teardown. Painted here (before the menu/status panels)
    // so it stays on the background layer *under* the chrome — painting it
    // after the panels would overdraw them.
    let viewport_empty = world
        .get_resource::<ViewportPlaceholder>()
        .is_some_and(|p| p.message.is_some());
    let no_active_scene_camera = !scene_camera_is_rendering(world);
    let needs_full_backdrop = needs_full_backdrop(layout, viewport_empty, no_active_scene_camera);
    if needs_full_backdrop {
        let painter = ctx.layer_painter(egui::LayerId::background());
        painter.rect_filled(ctx.content_rect(), 0.0, get_panel_backdrop(theme));
    }

    // ── Menu bar ────────────────────────────────────────────────────
    // Doubles as the OS title bar (window chrome is disabled in the
    // binary's `Window` setup — see `lunica.rs`). Bare
    // areas of the row drag the window; double-click toggles maximize;
    // window control buttons (─ ▢ ✕) sit on the far right on
    // Linux/Windows. macOS keeps native traffic lights — we just inset
    // the menu past them.
    // egui 0.35 unified the panel API: panels, the central area, and the dock
    // now render *inside* a `Ui` rather than directly onto the `Context`. Build
    // one root Ui spanning the whole viewport; every panel below shows into it,
    // consuming edges in call order, and the dock/centre takes the remainder.
    let mut viewport_ui = egui::Ui::new(
        ctx.clone(),
        "lunco_workbench_viewport".into(),
        egui::UiBuilder::new()
            .layer_id(egui::LayerId::background())
            .max_rect(ctx.viewport_rect()),
    );

    let titlebar_height = theme.spacing.titlebar_height;
    let titlebar_control_size = egui::vec2(
        theme.spacing.titlebar_control_size.x,
        theme.spacing.titlebar_control_size.y,
    );

    egui::Panel::top("lunco_workbench_menu_bar")
        // Match the dock tab-bar height so the merged title-bar
        // doesn't read as a thin sliver above thicker rows below.
        .exact_size(titlebar_height)
        .show(&mut viewport_ui, |ui| {
        // egui::MenuBar normally creates an 18px compact row of its own.
        // Make that row use the whole title-bar height before constructing
        // any menu or control, so every response is centred against the same
        // rectangle instead of each widget being centred against a different
        // implicit row height.
        ui.spacing_mut().interact_size.y = titlebar_height;
        ui.spacing_mut().item_spacing = egui::vec2(
            theme.spacing.item_spacing,
            theme.spacing.item_spacing,
        );

        // Drag region must be registered BEFORE the menu buttons so
        // egui's last-wins hit-testing lets buttons capture clicks
        // over their own area while bare gaps drag the OS window.
        let drag_resp = ui.interact(
            ui.max_rect(),
            ui.id().with("titlebar_drag"),
            egui::Sense::click_and_drag(),
        );
        if drag_resp.drag_started() {
            // start_drag_move() must be called on the live Window
            // component synchronously with the press event — routing
            // through a command would defer it past the press and
            // winit would refuse the drag. Direct mutation is the
            // right call here.
            if let Ok(mut w) = world
                .query_filtered::<&mut bevy::window::Window, bevy::prelude::With<bevy::window::PrimaryWindow>>()
                .single_mut(world)
            {
                w.start_drag_move();
            }
        }
        if drag_resp.double_clicked() {
            trigger_or_defer(world, lunco_workbench_window::MaximizeWindow { maximized: None });
        }

        // Window title — read straight off the primary Bevy window so the
        // binary stays the source of truth for what the bar advertises (e.g.
        // listening port). It is painted after the left and right groups have
        // been laid out, inside the actual gap between them, so it cannot
        // overlap controls on a compact window.
        let title = world
            .query_filtered::<&bevy::window::Window, bevy::prelude::With<bevy::window::PrimaryWindow>>()
            .single(world)
            .ok()
            .map(|w| w.title.clone())
            .unwrap_or_default();

        // `ui.horizontal` defaults to top-aligned cross-axis; with the
        // menu bar bumped to 30px the buttons would stick to the top
        // edge. Explicit `Align::Center` keeps them vertically centred
        // in the bar.
        // MenuBar creates its own compact horizontal child. Put that child
        // inside the full-height title-bar layout so the row is centred in
        // the 30px bar rather than starting at its top edge.
        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
            egui::MenuBar::new()
                .config(
                    egui::menu::MenuConfig::new()
                        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside),
                )
                .ui(ui, |ui| {
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
            // Collected screen-rects of the menu buttons + transport
            // controls. Published to `HelpAnchors` after this layout
            // closure finishes so we don't double-borrow `world`
            // while the menu_button closures already hold it.
            let mut anchor_rects: Vec<(String, egui::Rect)> = Vec::new();
            anchor_rects.push(("menu.bar".to_owned(), ui.max_rect()));

            // macOS: leave room for the native traffic lights that
            // float over our content because of `fullsize_content_view`.
            #[cfg(target_os = "macos")]
            ui.add_space(78.0);
            let mut direct_menu_labels = vec![
                "File".to_owned(),
                "Edit".to_owned(),
                "View".to_owned(),
            ];
            direct_menu_labels.extend(menus.custom_menus.iter().map(|(name, _)| name.clone()));
            direct_menu_labels.extend(menus.scripted_menu_labels().map(str::to_owned));
            direct_menu_labels.extend([
                "Settings".to_owned(),
                "Help".to_owned(),
                "Time".to_owned(),
            ]);
            let mut measured_labels = std::collections::HashSet::new();
            direct_menu_labels.retain(|label| measured_labels.insert(label.clone()));
            let direct_menu_width =
                measured_menu_row_width(ui, direct_menu_labels.iter().map(String::as_str));
            let menu_mode = top_menu_mode(
                ui.available_width(),
                direct_menu_width,
                measured_titlebar_right_width(ui, layout, titlebar_control_size),
            );
            let r_file = ui.menu_button("File", |ui| {
                // Active doc gates Save / Save As / Close — there's
                // nothing to save when no document is focused.
                let active_doc =
                    world.resource::<WorkspaceResource>().active_document;
                let has_active = active_doc.is_some();

                // -- New ----------------------------------------------
                // Submenu populated from `DocumentKindRegistry`. Each
                // entry fires `NewDocument { kind }`; the matching
                // domain observer creates the doc. Ctrl+N fires the
                // default-resolution path through `EditorIntent`.
                ui.menu_button("New", |ui| {
                    if ui.button("Twin…").clicked() {
                        trigger_or_defer(world, lunco_workspace::open::CreateTwin {
                            path: String::new(),
                            name: String::new(),
                            default_scene: String::new(),
                        });
                        ui.close();
                    }
                    let registry = world
                        .resource::<lunco_twin::DocumentKindRegistry>();
                    let entries: Vec<(String, String, Option<&'static str>)> = registry
                        .creatable()
                        .into_iter()
                        .map(|(id, m)| {
                            (
                                id.as_str().to_string(),
                                m.display_name.clone(),
                                m.default_filename,
                            )
                        })
                        .collect();
                    if entries.is_empty() {
                        ui.label(
                            egui::RichText::new("(no document kinds registered)")
                                .weak()
                                .italics(),
                        );
                    } else {
                        ui.separator();
                        for (index, (kind, display, default_filename)) in
                            entries.into_iter().enumerate()
                        {
                            // Ctrl+N resolves to the first registered
                            // creatable kind, which is the first entry after
                            // deterministic display-name sorting.
                            let label = new_document_menu_label(index, &display);
                            let response = ui.button(label);
                            let response = match default_filename {
                                Some(filename) => response.on_hover_text(format!(
                                    "Create {display} with default filename {filename}"
                                )),
                                None => response,
                            };
                            if response.clicked() {
                                trigger_or_defer(world, lunco_doc_bevy::NewDocument { kind });
                                ui.close();
                            }
                        }
                    }
                });
                ui.separator();

                // -- Open ---------------------------------------------
                if ui.button("Open File…\tCtrl+O").clicked() {
                    trigger_or_defer(world, lunco_workbench_file_ops::ShowOpenFilePicker {});
                    ui.close();
                }
                // Open Folder + Recents are native-only for now.
                //
                // TODO(wasm): the browser has no folder picker that
                // hands back a usable path (`webkitdirectory` only
                // exposes loose files, not a writable Twin root), and
                // recents are persisted to the shared LunCoSim config directory —
                // there is no home dir on wasm and recorded paths
                // can't be re-read (no filesystem, picked content is
                // consumed once). Wasm equivalents need a directory
                // picker via the File-System-Access API and recents
                // backed by localStorage / IndexedDB.
                #[cfg(not(target_arch = "wasm32"))]
                {
                    // Open Folder auto-classifies on the resolved path
                    // — `twin.toml` present routes to Twin mode,
                    // absence gives a plain folder workspace. The
                    // strict-mode `OpenTwin` typed command remains
                    // available to recents/HTTP/scripts that want
                    // explicit Twin semantics, but isn't worth a
                    // separate menu entry.
                    if ui.button("Open Folder/Twin…").clicked() {
                        trigger_or_defer(world, lunco_workbench_file_ops::ShowOpenFolderPicker {});
                        ui.close();
                    }

                    // -- Recents ------------------------------------
                    // Twin folders and loose files have separate
                    // lists per VS Code precedent — recently-edited
                    // files within a Twin shouldn't crowd out the
                    // much-shorter list of recently-opened projects.
                    // Persisted to the shared LunCoSim config directory
                    // (cross-platform) by `WorkspacePlugin`.
                    let (recent_twins, recent_files) = {
                        let ws = world.resource::<WorkspaceResource>();
                        (
                            ws.recents.twin_paths.clone(),
                            ws.recents.loose_paths.clone(),
                        )
                    };
                    ui.add_enabled_ui(!recent_twins.is_empty(), |ui| {
                        ui.menu_button("Open Recent Twin", |ui| {
                            for path in &recent_twins {
                                let label = path
                                    .file_name()
                                    .and_then(|s| s.to_str())
                                    .unwrap_or_else(|| path.to_str().unwrap_or("(invalid)"));
                                if ui
                                    .button(label)
                                    .on_hover_text(path.display().to_string())
                                    .clicked()
                                {
                                    trigger_or_defer(world, lunco_workspace::open::OpenTwin {
                                        path: path.display().to_string(),
                                    });
                                    ui.close();
                                }
                            }
                        });
                    })
                    .response
                    .on_disabled_hover_text("No Twin folders opened yet");
                    ui.add_enabled_ui(!recent_files.is_empty(), |ui| {
                        ui.menu_button("Open Recent File", |ui| {
                            for path in &recent_files {
                                let label = path
                                    .file_name()
                                    .and_then(|s| s.to_str())
                                    .unwrap_or_else(|| path.to_str().unwrap_or("(invalid)"));
                                if ui
                                    .button(label)
                                    .on_hover_text(path.display().to_string())
                                    .clicked()
                                {
                                    trigger_or_defer(world, lunco_doc_bevy::OpenFile {
                                        path: path.display().to_string(),
                                    });
                                    ui.close();
                                }
                            }
                        });
                    })
                    .response
                    .on_disabled_hover_text("No files opened yet");
                }
                ui.separator();

                // -- Save ---------------------------------------------
                // Save / Save As route through `EditorIntent` so the
                // menu, Ctrl+S, and HTTP API funnel through the same
                // domain resolver.
                if menu_item(ui, has_active, "Save", "Ctrl+S", "No document open")
                    .clicked()
                {
                    trigger_or_defer(world, lunco_doc_bevy::EditorIntent::Save);
                    ui.close();
                }
                if menu_item(
                    ui,
                    has_active,
                    "Save As…",
                    "Ctrl+Shift+S",
                    "No document open",
                )
                .clicked()
                {
                    trigger_or_defer(world, lunco_doc_bevy::EditorIntent::SaveAs);
                    ui.close();
                }
                if ui.button("Save All").clicked() {
                    trigger_or_defer(world, lunco_workbench_file_ops::SaveAll {});
                    ui.close();
                }
                if ui.button("Save as Twin…").clicked() {
                    trigger_or_defer(world, lunco_workbench_file_ops::SaveAsTwin {
                        folder: String::new(),
                    });
                    ui.close();
                }
                ui.separator();

                // Network actions belong to File because they operate on the
                // current session/document connection, while the status bar
                // remains the always-visible connection indicator.
                let r_network = ui.menu_button("Network", |ui| {
                    render_network_menu(ui, world);
                });
                anchor_rects.push(("menu.network".to_owned(), r_network.response.rect));

                ui.separator();

                #[cfg(target_arch = "wasm32")]
                {
                    // Browser sharing remains available through the web UI;
                    // the desktop shell does not expose this action.
                    if menu_item(ui, has_active, "Copy Share Link", "", "No document open")
                        .on_hover_text(
                            "Copy a URL that encodes this model's source — \
                             anyone who opens it gets the model (nothing is uploaded)",
                        )
                        .clicked()
                    {
                        trigger_or_defer(world, lunco_workbench_file_ops::CopyShareLink {});
                        ui.close();
                    }
                    ui.separator();
                }

                if !menus.file_menu.is_empty() {
                    for cb in &menus.file_menu {
                        run_menu_callback(ui, world, cb.as_ref());
                    }
                    ui.separator();
                }

                // -- Close --------------------------------------------
                if menu_item(ui, has_active, "Close", "Ctrl+W", "No document open")
                    .clicked()
                {
                    trigger_or_defer(world, lunco_doc_bevy::EditorIntent::Close);
                    ui.close();
                }
            });
            anchor_rects.push(("menu.file".to_owned(), r_file.response.rect));
            if matches!(menu_mode, TopMenuMode::Direct) {
                let r_edit = ui.menu_button("Edit", |ui| {
                    render_edit_menu(ui, world, menus);
                });
                anchor_rects.push(("menu.edit".to_owned(), r_edit.response.rect));
            } else {
                let r_more = ui.menu_button("More", |ui| {
                    ui.menu_button("Edit", |ui| {
                        render_edit_menu(ui, world, menus);
                    });
                    render_custom_menus(ui, world, menus, None);
                    ui.menu_button("Settings", |ui| {
                        render_settings_menu(ui, world, menus);
                    });
                    ui.menu_button("Help", |ui| {
                        render_help_menu(ui, world, menus);
                    });
                    ui.menu_button("Time", |ui| {
                        render_time_menu(ui, world, menus);
                    });
                });
                anchor_rects.push(("menu.more".to_owned(), r_more.response.rect));
            }
            let r_view = ui.menu_button("View", |ui| {
                if ui.button("Reset Layout").clicked() {
                    // Recovery hatch: re-apply the active perspective's preset,
                    // restoring panels (notably the 3D Viewport) a stale
                    // persisted layout dropped.
                    world
                        .resource_mut::<PendingLayoutRequests>()
                        .0
                        .push(LayoutRequest::Reset);
                    ui.close();
                }
                ui.separator();
                if ui.button("Toggle Activity Bar").clicked() {
                    world
                        .resource_mut::<PendingLayoutRequests>()
                        .0
                        .push(LayoutRequest::SetActivityBar(!layout.activity_bar));
                    ui.close();
                }
                ui.separator();
                // Panels are grouped into workflow submenus using the category
                // each one declares (`Panel::menu_group`). Each row is a
                // checkbox showing whether the panel is currently in the dock;
                // clicking a closed one re-docks it in its default slot.
                // `Hidden` panels never appear (fixtures like the viewport,
                // layout-only entries, instance-tab facets).
                struct ViewPanelEntry {
                    group: PanelMenuGroup,
                    title: String,
                    slot: PanelSlot,
                    open: bool,
                    singleton: Option<PanelId>,
                    instance: Option<(PanelId, u64)>,
                }
                let panels_meta: Vec<ViewPanelEntry> = {
                    let docked: std::collections::HashSet<PanelId> = layout
                        .dock
                        .iter_all_tabs()
                        .filter_map(|(_, id)| match id {
                            TabId::Singleton(pid) => Some(*pid),
                            TabId::Instance { .. } => None,
                        })
                        .collect();
                    let mut sorted: Vec<ViewPanelEntry> =
                        layout
                            .panels
                            .values()
                            .filter(|p| p.menu_group() != PanelMenuGroup::Hidden)
                            .map(|p| {
                                let id = p.id();
                                ViewPanelEntry {
                                    group: p.menu_group(),
                                    title: p.title(),
                                    slot: p.default_slot(),
                                    open: docked.contains(&id),
                                    singleton: Some(id),
                                    instance: None,
                                }
                            })
                            .collect();
                    // Instance panels normally have no global menu entry.
                    // Include only the canonical instance explicitly exposed
                    // by the panel, so document tabs remain discoverable by
                    // their owning workflow rather than becoming noise here.
                    for (kind, panel) in &layout.instance_panels {
                        let Some(entry) = panel.menu_entry() else {
                            continue;
                        };
                        let tab = TabId::Instance {
                            kind: *kind,
                            instance: entry.instance,
                        };
                        sorted.push(ViewPanelEntry {
                            group: entry.group,
                            title: entry.title.to_owned(),
                            slot: panel.default_slot(),
                            open: layout.dock.find_tab(&tab).is_some(),
                            singleton: None,
                            instance: Some((*kind, entry.instance)),
                        });
                    }
                    // Group first, then title — but sort titles on their FIRST
                    // ALPHANUMERIC char: sorting on the raw string put every
                    // emoji-prefixed title ("🛠 Tools") in a block after every
                    // plain one, which is why related panels never sat together.
                    let sort_key = |t: &str| -> String {
                        t.chars()
                            .skip_while(|c| !c.is_alphanumeric())
                            .collect::<String>()
                            .to_lowercase()
                    };
                    sorted.sort_by(|a, b| {
                        a.group
                            .cmp(&b.group)
                            .then_with(|| sort_key(&a.title).cmp(&sort_key(&b.title)))
                    });
                    sorted
                };
                let mut panels_by_group: Vec<(PanelMenuGroup, Vec<ViewPanelEntry>)> = Vec::new();
                for entry in panels_meta {
                    if let Some((group, entries)) = panels_by_group.last_mut() {
                        if *group == entry.group {
                            entries.push(entry);
                            continue;
                        }
                    }
                    panels_by_group.push((entry.group, vec![entry]));
                }

                for (group, entries) in panels_by_group {
                    let heading = match group {
                        PanelMenuGroup::Builder => "Builder",
                        PanelMenuGroup::Editor => "Editor",
                        PanelMenuGroup::Lunica => "Lunica",
                        PanelMenuGroup::Other => "Other",
                        PanelMenuGroup::Hidden => unreachable!("filtered above"),
                    };
                    ui.menu_button(heading, |ui| {
                        for entry in entries {
                            let ViewPanelEntry {
                                title,
                                slot,
                                open: is_open,
                                singleton,
                                instance,
                                ..
                            } = entry;
                            let mut checked = is_open;
                            if ui.checkbox(&mut checked, title).clicked() {
                                if checked && !is_open {
                                    if let Some((kind, instance)) = instance {
                                        world
                                            .resource_mut::<PendingTabRequests>()
                                            .0
                                            .push(TabRequest::Open(OpenTab { kind, instance }));
                                        ui.close();
                                        continue;
                                    }
                                    let Some(id) = singleton else {
                                        ui.close();
                                        continue;
                                    };
                                    // Track in the slot list so persistence /
                                    // perspective queries see it. Insert into
                                    // the *live* dock without a full rebuild
                                    // — rebuild_dock would wipe instance tabs
                                    // (model views) the user has open.
                                    //
                                    // A hidden default slot has no preset dock region;
                                    // opening it explicitly gives it a stable side-browser
                                    // home until Reset Layout.
                                    let slot = match slot {
                                        PanelSlot::Hidden => PanelSlot::SideBrowser,
                                        other => other,
                                    };
                                    world
                                        .resource_mut::<PendingLayoutRequests>()
                                        .0
                                        .push(LayoutRequest::AddSingleton { id, slot });
                                } else if !checked && is_open {
                                    if let Some((kind, instance)) = instance {
                                        world
                                            .resource_mut::<PendingTabRequests>()
                                            .0
                                            .push(TabRequest::Close(CloseTab { kind, instance }));
                                        ui.close();
                                        continue;
                                    }
                                    let Some(id) = singleton else {
                                        ui.close();
                                        continue;
                                    };
                                    // Untrack from slot lists.
                                    world
                                        .resource_mut::<PendingLayoutRequests>()
                                        .0
                                        .push(LayoutRequest::RemoveSingleton(id));
                                }
                                ui.close();
                            }
                        }
                    });
                }
            });
            anchor_rects.push(("menu.view".to_owned(), r_view.response.rect));

            if matches!(menu_mode, TopMenuMode::Direct) {
            // Custom top-level menus are rendered through the same helper in
            // direct and compact layouts, so registered commands keep one
            // owner and one callback path.
            render_custom_menus(ui, world, menus, Some(&mut anchor_rects));

            let r_settings = ui.menu_button("Settings", |ui| {
                render_settings_menu(ui, world, menus);
            });
            anchor_rects.push(("menu.settings".to_owned(), r_settings.response.rect));
            let r_help = ui.menu_button("Help", |ui| {
                render_help_menu(ui, world, menus);
            });
            anchor_rects.push(("menu.help".to_owned(), r_help.response.rect));

            // Time — causal simulation-rate controls and any registered
            // application-level time actions. Pause/resume stays on the toolbar.
            let r_time = ui.menu_button("Time", |ui| {
                render_time_menu(ui, world, menus);
            });
            anchor_rects.push(("menu.time".to_owned(), r_time.response.rect));
            }

            // Pause/Resume simulation via the single transport authority
            // (`TimeTransport.mode`, doc 19). The spine maps `Paused` onto
            // `relative_speed = 0`, freezing tick + avian (`Time<Physics>` derives
            // from Virtual) + epoch together — and now stays in sync with the
            // avatar pause hotkey and the mission-control / celestial panels, which
            // write the same resource.
            {
                let paused = world
                    .get_resource::<lunco_time::TimeTransport>()
                    .is_some_and(|t| matches!(t.mode, lunco_time::TransportMode::Paused));
                let (icon, hover) = if paused {
                    (UiIcon::Play, "Resume simulation")
                } else {
                    (UiIcon::Pause, "Pause simulation")
                };
                let btn_resp = icon_button_sized(ui, icon, hover, titlebar_control_size);
                anchor_rects.push(("toolbar.run".to_owned(), btn_resp.rect));
                if btn_resp.clicked() {
                    trigger_or_defer(world, lunco_time::SetTimeTransport {
                        playing: Some(paused),
                        ..default()
                    });
                }

                // PAUSE/RESUME AND NOTHING ELSE. The rate selector used to sit here
                // too; it moved to the Time menu above. The toolbar is the place for
                // the one verb you reach for mid-drive, not for the whole clock.
            }

            // Perspective tabs live in the menu bar (right-aligned).
            // No separate transport bar — saves a row of vertical space.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // Window controls — far right on Linux/Windows where
                // the OS chrome is gone. macOS keeps the native traffic
                // lights, so we don't draw our own. On wasm the browser
                // tab owns the chrome, so min/max/close don't apply.
                #[cfg(all(not(target_os = "macos"), not(target_arch = "wasm32")))]
                {
                    let is_max = world
                        .get_resource::<lunco_workbench_window::WindowMaximized>()
                        .map(|s| s.0)
                        .unwrap_or(false);
                    let close_response =
                        icon_button_sized(ui, UiIcon::Close, "Close", titlebar_control_size);
                    anchor_rects.push(("window.close".to_owned(), close_response.rect));
                    if close_response.clicked() {
                        trigger_or_defer(world, lunco_workbench_window::CloseWindow {});
                    }
                    let max_icon = if is_max {
                        UiIcon::Restore
                    } else {
                        UiIcon::Maximize
                    };
                    let max_hover = if is_max { "Restore" } else { "Maximize" };
                    let maximize_response =
                        icon_button_sized(ui, max_icon, max_hover, titlebar_control_size);
                    anchor_rects.push(("window.maximize".to_owned(), maximize_response.rect));
                    if maximize_response.clicked() {
                        trigger_or_defer(world, lunco_workbench_window::MaximizeWindow { maximized: None });
                    }
                    let minimize_response = icon_button_sized(
                        ui,
                        UiIcon::Minimize,
                        "Minimize",
                        titlebar_control_size,
                    );
                    anchor_rects.push(("window.minimize".to_owned(), minimize_response.rect));
                    if minimize_response.clicked() {
                        trigger_or_defer(world, lunco_workbench_window::MinimizeWindow {});
                    }
                    ui.separator();
                }
                let tabs = perspective_switcher_tabs(layout);
                if tabs.len() > 1 {
                    for (id, title, is_active) in tabs {
                        let mut label = egui::RichText::new(title.as_str()).color(if is_active {
                            theme.colors.text
                        } else {
                            theme.colors.subtext1
                        });
                        if is_active {
                            label = label.strong();
                        }
                        let mut button = egui::Button::new(label)
                            .corner_radius(theme.rounding.button)
                            .selected(is_active)
                            .stroke(egui::Stroke::NONE);
                        if is_active {
                            button = button.fill(theme.tokens.surface_raised);
                        }
                        let response = ui.add(button);
                        anchor_rects.push((perspective_help_anchor(id), response.rect));
                        if response.clicked() && !is_active {
                            world
                                .resource_mut::<PendingLayoutRequests>()
                                .0
                                .push(LayoutRequest::ActivatePerspective(id.0.to_owned()));
                        }
                    }
                }
            });

            if !title.is_empty() {
                let right_group_start = anchor_rects
                    .iter()
                    .filter(|(key, _)| {
                        key.starts_with("menu.perspective.") || key.starts_with("window.")
                    })
                    .map(|(_, rect)| rect.left())
                    .min_by(f32::total_cmp)
                    .unwrap_or_else(|| ui.min_rect().right());
                let left_group_end = anchor_rects
                    .iter()
                    .filter(|(key, _)| {
                        key != "menu.bar"
                            && key != "menu.network"
                            && !key.starts_with("window.")
                            && !key.starts_with("menu.perspective.")
                    })
                    .map(|(_, rect)| rect.right())
                    .max_by(f32::total_cmp)
                    .unwrap_or_else(|| ui.min_rect().left());
                let title_gap = egui::Rect::from_min_max(
                    egui::pos2(left_group_end, ui.min_rect().top()),
                    egui::pos2(right_group_start, ui.min_rect().bottom()),
                );
                let shown = truncate_title_to_width(
                    ui,
                    &title,
                    (title_gap.width() - ui.spacing().item_spacing.x * 2.0).max(0.0),
                );
                if !shown.is_empty() {
                    ui.painter().text(
                        title_gap.center(),
                        egui::Align2::CENTER_CENTER,
                        shown,
                        egui::FontId::proportional(12.0),
                        theme.tokens.text_subdued,
                    );
                }
            }

            // Flush collected button rects into `HelpAnchors` now
            // that the menu_button closures have returned and no
            // longer borrow `world`.
            if let Some(mut a) = world.get_resource_mut::<HelpAnchors>() {
                for (k, r) in anchor_rects {
                    a.set(k, r);
                }
            }
                });
        });
    });
    });

    // ── Status bar ──────────────────────────────────────────────────
    // Drives off the cross-cutting `StatusBus` resource. Latest event
    // shows in the strip; click opens a popup with recent history.
    let status_surface_fill = panel_surface_fill(
        theme,
        world
            .resource::<WorkbenchAppearanceSettings>()
            .translucent_tab_content,
    );
    egui::Panel::bottom("lunco_workbench_status_bar")
        .frame(egui::Frame::NONE.fill(status_surface_fill))
        .show_separator_line(false)
        .show(&mut viewport_ui, |ui| {
            render_status_bar_inner(ui, world, theme);
        });

    // ── Activity bar ────────────────────────────────────────────────
    if layout.activity_bar {
        egui::Panel::left("lunco_workbench_activity_bar")
            .resizable(false)
            .exact_size(40.0)
            .show(&mut viewport_ui, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(4.0);
                    for label in ["Files", "Parts", "Assets", "Find", "Settings"] {
                        ui.label(label);
                        ui.add_space(8.0);
                    }
                });
            });
    }

    // ── Dock area / side panels ─────────────────────────────────────
    // Two-mode rendering:
    //   1. If the active perspective is centre-driven (non-empty centre
    //      intent, e.g. the modelica workbench's Code/Diagram), render the
    //      full DockArea.
    //   2. Otherwise (viewport-only perspective like the luncosim's `View`),
    //      render the side panels with plain SidePanel / TopBottomPanel and
    //      leave the central area transparent for the 3D viewport.
    //
    // The gate is the centre *intent* (`layout.center`), not merely "does
    // the dock hold any tab". A hybrid app (the rover luncosim embeds the
    // Modelica workbench) can have document/model instance tabs parked in
    // the dock while a viewport-only perspective is active — e.g. restored
    // on boot before the user switches to a doc-capable perspective.
    // Keying off the dock alone would flip the whole workbench into
    // dock-mode and paint tab chrome over the 3D scene; keying off the
    // perspective's centre intent keeps `View` pure-3D and leaves the
    // parked docs hidden until the user switches to a centre-driven
    // perspective (which re-attaches them via `rebuild_dock`).
    let has_dock_tabs = !layout.center.is_empty() && layout.dock.iter_all_tabs().next().is_some();
    let translucent_tab_content = world
        .resource::<WorkbenchAppearanceSettings>()
        .translucent_tab_content;
    let panel_surface = panel_content_surface_style(theme, translucent_tab_content);

    if has_dock_tabs {
        let WorkbenchLayout {
            panels,
            instance_panels,
            dock,
            side_browser,
            right_inspector,
            bottom,
            ..
        } = &mut *layout;
        let mut viewer = PanelTabViewer {
            panels,
            instance_panels,
            world,
            surface: panel_surface,
        };
        let mut style = Style::from_egui(viewport_ui.style().as_ref());
        // Drop the outer dock border — it shows up as a thin line along
        // the inside edge of the side panels and looks like dead pixels
        // when the dock is otherwise transparent.
        style.main_surface_border_stroke = egui::Stroke::NONE;
        // Drop the resize separator's idle colour — that's the 1px line
        // between docked panels. Hover/drag colours stay so the user
        // can still find and grab the divider.
        style.separator.color_idle = egui::Color32::TRANSPARENT;
        // Drop the per-tab body border (the rectangle around every
        // panel content area). This is the "border when unfolded".
        style.tab.tab_body.stroke = egui::Stroke::NONE;
        // The tab strip is distinct chrome, so it uses the theme's crust while
        // ordinary tab bodies use mantle. The dedicated ViewportPanel remains
        // transparent because it hosts the full-window scene camera.
        style.tab_bar.bg_fill = theme.colors.crust;
        // Drop the hairline under the active tab name too — same
        // visual-noise reason as the tab body stroke.
        style.tab_bar.hline_color = egui::Color32::TRANSPARENT;
        // egui_dock's `Style::from_egui` defaults pull tab colours
        // from `visuals.widgets`, but the result still doesn't track
        // our Light/Dark palette cleanly: inactive tabs come out
        // washed out and active tabs lose contrast against the bar.
        // Bind every interaction state to the theme so tabs read
        // consistently in both modes.
        // The selected tab is a raised, high-contrast surface; inactive tabs stay
        // quiet until hovered. egui_dock uses `focused` for the active tab in
        // the focused leaf, so the active treatment must be applied to both
        // states or the selection disappears as soon as the pane is focused.
        let palette = &theme.colors;
        style.tab.tab_body.bg_fill = panel_surface_fill(theme, translucent_tab_content);
        let active_fill = theme.tokens.surface_raised;
        // The selected tab is already distinguished by its raised fill and
        // text. Keep the tab name area quiet: outlines make the tab strip look
        // like a row of nested controls, especially when the accent colour is
        // active elsewhere in the workbench.
        let active_corner_radius = theme.rounding.button;
        for tab in [
            &mut style.tab.active,
            &mut style.tab.focused,
            &mut style.tab.active_with_kb_focus,
            &mut style.tab.focused_with_kb_focus,
        ] {
            tab.bg_fill = active_fill;
            tab.text_color = palette.text;
            tab.outline_color = egui::Color32::TRANSPARENT;
            tab.corner_radius = active_corner_radius.into();
        }
        style.tab.inactive.bg_fill = egui::Color32::TRANSPARENT;
        style.tab.inactive.text_color = palette.subtext1;
        style.tab.inactive.outline_color = egui::Color32::TRANSPARENT;
        style.tab.inactive.corner_radius = active_corner_radius.into();
        style.tab.hovered.bg_fill = palette.surface1;
        style.tab.hovered.text_color = palette.text;
        style.tab.hovered.outline_color = egui::Color32::TRANSPARENT;
        style.tab.hovered.corner_radius = active_corner_radius.into();
        style.tab.inactive_with_kb_focus.bg_fill = palette.surface1;
        style.tab.inactive_with_kb_focus.text_color = palette.text;
        style.tab.inactive_with_kb_focus.outline_color = egui::Color32::TRANSPARENT;
        style.tab.inactive_with_kb_focus.corner_radius = active_corner_radius.into();
        // TODO(egui_dock 0.18 bug — remove when fixed/updated upstream):
        // egui_dock writes a NaN split fraction into the tree from inside its
        // own `show()` every frame a pane is squeezed to zero width — see
        // `sanitize_dock_fractions` for the exact `0.0/0.0` site. So we must
        // re-assert the invariant every frame, right before layout, or egui
        // asserts ("rect is nan"). Drop this call once egui_dock guards its
        // `delta / range`; the load-time sanitize stays regardless.
        sanitize_dock_fractions(dock);

        // Guard against degenerate viewport rects. On Windows + Intel
        // Vulkan the swapchain can present a zero/non-finite size for
        // the first frames after the window is mapped; egui_dock 0.18
        // then computes `min + dim_size * fraction` with
        // `Rect::NOTHING`, yielding NaN, and egui asserts in
        // `advance_cursor_after_rect`. Skip the dock for that frame.
        let screen = ctx.content_rect();
        if screen.width().is_finite()
            && screen.height().is_finite()
            && screen.width() > 1.0
            && screen.height() > 1.0
        {
            // The dock's true extent for the scene-pick gate: the rect we are
            // ABOUT to hand it — i.e. what's left of the root Ui after the menu /
            // status / activity bars consumed their edges. Anything outside it is
            // bare full-window 3D that must read as scene, not chrome.
            //
            // Must be measured BEFORE `show_inside`. The old code took
            // `viewport_ui.min_rect()` AFTER it, but `viewport_ui` is the ROOT
            // background Ui spanning the whole window — the menu bar and status bar
            // are drawn into it too — so `min_rect()` came back as ≈ the entire
            // window. `in_dock` was then true everywhere and the chrome blanket
            // swallowed every click on bare 3D outside a dock leaf.
            let dock_rect = viewport_ui.available_rect_before_wrap();
            DockArea::new(dock)
                .style(style)
                .show_inside(&mut viewport_ui, &mut viewer);
            // The scene viewport LEAF's rect, straight from egui_dock's post-layout
            // tree. `LeafNode::rect` persists even when the leaf is COLLAPSED or the
            // viewport sits behind another tab — cases where `ViewportPanel::render`
            // doesn't run and so can't record the scene pick leaf itself. Feeding
            // this to the gate keeps the full-window 3D clickable through
            // collapse / fold / background (else a collapsed centre goes dead to
            // clicks). `panels`/`dock` are usable again here — `viewer`'s reborrows
            // ended at `show_inside`.
            let scene_vp_tab = panels
                .iter()
                .find(|(_, p)| p.scene_target() == Some(PanelRenderTarget::MainViewport))
                .map(|(id, _)| TabId::Singleton(*id));
            let scene_vp_rect = scene_vp_tab.and_then(|vp_tab| {
                dock.main_surface().iter().find_map(|node| match node {
                    egui_dock::Node::Leaf(leaf) if leaf.tabs.contains(&vp_tab) => Some(leaf.rect),
                    _ => None,
                })
            });

            // Publish generic slot anchors from the laid-out dock tree. The
            // alternate explicit-panel renderer below publishes these from
            // egui::Panel responses; docked perspectives need the same
            // contract or a guided would fail merely because its authored
            // perspective uses egui_dock.
            if let Some(mut a) = world.get_resource_mut::<HelpAnchors>() {
                a.set("panel.center", screen);
                let (side_rect, right_rect, bottom_rect) =
                    dock_group_rects(dock, side_browser, right_inspector, bottom);
                if let Some(rect) = side_rect {
                    a.set("panel.side_browser", rect);
                }
                if let Some(rect) = right_rect {
                    a.set("panel.right_inspector", rect);
                }
                if let Some(rect) = bottom_rect {
                    a.set("panel.bottom", rect);
                }
            }
            if let Some(mut g) = world.get_resource_mut::<ScenePickGate>() {
                g.set_dock_rect(dock_rect);
                g.set_scene_viewport_rect(scene_vp_rect);
            }
        }
    } else {
        // 3D-app mode — explicit side panels, transparent centre.
        // Defaults are percentages of the current window so the layout
        // looks right whether the user runs in 1280×720 or 4K. Targets
        // mirror a 10/80/10 split: side panels 10% of window width each;
        // bottom dock 20% of window height.
        let screen = ctx.content_rect();
        // Defaults are percentages of the current window so the layout
        // looks right whether the user runs in 1280×720 or 4K. Targets
        // mirror a 10/80/10 split: side panels 10% of window width each;
        // bottom dock 20% of window height. egui then owns the live width
        // in its own memory for the session (not persisted — luncosim-style
        // perspectives keep their sizes in the dock tree via 5a instead).
        let side_default = (screen.width() * 0.10).max(140.0);
        let right_default = (screen.width() * 0.10).max(140.0);
        let bottom_default = (screen.height() * 0.20).max(120.0);

        let side_panel_fill = panel_surface_fill(
            theme,
            world
                .resource::<WorkbenchAppearanceSettings>()
                .translucent_tab_content,
        );

        if let Some(id) = layout.side_browser.first().copied() {
            let r = egui::Panel::left("lunco_workbench_side_panel_left")
                .resizable(true)
                .default_size(side_default)
                .min_size(120.0)
                .max_size(screen.width() * 0.3)
                .frame(
                    egui::Frame::side_top_panel(viewport_ui.style().as_ref()).fill(side_panel_fill),
                )
                .show(&mut viewport_ui, |ui| {
                    render_panel_solo(ui, &id, layout, world, panel_surface);
                });
            publish_panel_anchor(world, id, r.response.rect);
            if let Some(mut a) = world.get_resource_mut::<HelpAnchors>() {
                a.set("panel.side_browser", r.response.rect);
            }
        }
        if let Some(id) = layout.right_inspector.first().copied() {
            let r = egui::Panel::right("lunco_workbench_side_panel_right")
                .resizable(true)
                .default_size(right_default)
                .min_size(140.0)
                .max_size(screen.width() * 0.3)
                .frame(
                    egui::Frame::side_top_panel(viewport_ui.style().as_ref()).fill(side_panel_fill),
                )
                .show(&mut viewport_ui, |ui| {
                    render_panel_solo(ui, &id, layout, world, panel_surface);
                });
            publish_panel_anchor(world, id, r.response.rect);
            if let Some(mut a) = world.get_resource_mut::<HelpAnchors>() {
                a.set("panel.right_inspector", r.response.rect);
            }
        }
        if let Some(id) = layout.bottom.first().copied() {
            let r = egui::Panel::bottom("lunco_workbench_bottom_panel")
                .resizable(true)
                .default_size(bottom_default)
                .min_size(60.0)
                .frame(
                    egui::Frame::side_top_panel(viewport_ui.style().as_ref()).fill(side_panel_fill),
                )
                .show(&mut viewport_ui, |ui| {
                    render_panel_solo(ui, &id, layout, world, panel_surface);
                });
            publish_panel_anchor(world, id, r.response.rect);
            if let Some(mut a) = world.get_resource_mut::<HelpAnchors>() {
                a.set("panel.bottom", r.response.rect);
            }
        }
        // Central area: do NOT call CentralPanel — egui's bottom/side
        // panels reserve their space and the remaining region stays
        // free for the 3D scene that Bevy renders to the full window.
        // Scene-vs-chrome picking is handled by bevy_picking (egui occlusion via
        // bevy_egui's picking backend), so there's no pointer gate to compute
        // here anymore.
    }

    // ── Empty-viewport placeholder ──────────────────────────────────
    // Drawn last so it sits on top of the (empty) 3D framebuffer. Only
    // when a domain crate set a message (e.g. USD command domain: "no scene
    // loaded") AND the viewport is actually on screen — View (empty
    // layout, full-window 3D) or Build (ViewportPanel in the centre).
    // Never in Design mode, where Camera3d is inactive and the centre
    // is chrome. Centered on the window, which is the viewport region
    // in View mode and close enough in Build.
    let placeholder = world
        .get_resource::<ViewportPlaceholder>()
        .and_then(|p| p.message.clone());
    if let Some(msg) = placeholder {
        let viewport_visible = viewport::layout_is_empty(layout)
            || viewport::layout_contains_panel(layout, VIEWPORT_PANEL_ID);
        if viewport_visible {
            egui::Area::new(egui::Id::new("lunco_viewport_empty_placeholder"))
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .interactable(false)
                .show(ctx, |ui| {
                    ui.label(
                        egui::RichText::new(msg)
                            .color(theme.tokens.text_subdued)
                            .italics()
                            .size(16.0),
                    );
                });
        }
    }
}
