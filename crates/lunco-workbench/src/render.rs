//! Workbench rendering and UI composition.
//!
//! The shell lifecycle stays in the parent module; this module owns the
//! egui dock, menus, status presentation, and graphics/settings rendering.

use super::*;
use lunco_workbench_perf_ui::{frame_history, frame_ms_stats, PerfHudSettings, PerfStats};

#[derive(Resource)]
pub(crate) struct WorkbenchVisualsCache {
    revision: u64,
    theme: Arc<lunco_theme::Theme>,
    visuals: egui::Visuals,
    applied_context_revision: u64,
}

impl Default for WorkbenchVisualsCache {
    fn default() -> Self {
        Self {
            revision: u64::MAX,
            theme: Arc::new(lunco_theme::Theme::default()),
            visuals: egui::Visuals::dark(),
            applied_context_revision: u64::MAX,
        }
    }
}

pub(crate) fn render_workbench(world: &mut World) {
    let ctx = {
        let mut state: bevy::ecs::system::SystemState<EguiContexts> =
            bevy::ecs::system::SystemState::new(world);
        let Ok(mut contexts) = state.get_mut(world) else {
            return;
        };
        match contexts.ctx_mut() {
            Ok(ctx) => ctx.clone(),
            Err(_) => return,
        }
    };

    // egui's expansion diagnostics are developer overlays, not workbench UI.
    // Keep them disabled so debug builds cannot paint red layout markers over
    // the application when the panel layout changes.
    #[cfg(debug_assertions)]
    {
        let layout_debug_enabled = {
            let style = ctx.global_style();
            style.debug.show_expand_width || style.debug.show_expand_height
        };
        if layout_debug_enabled {
            ctx.global_style_mut(|style| {
                style.debug.show_expand_width = false;
                style.debug.show_expand_height = false;
            });
        }
    }

    let Some(mut layout) = world.remove_resource::<WorkbenchLayout>() else {
        return;
    };
    let Some(mut menus) = world.remove_resource::<WorkbenchMenuRegistry>() else {
        world.insert_resource(layout);
        return;
    };

    // Clear stale anchor rects at the start of each frame; menu /
    // panel writers refresh them as they render.
    if let Some(mut anchors) = world.get_resource_mut::<HelpAnchors>() {
        anchors.clear();
    }

    // Drop last pass's panel rects; the panels in the active layout refill them as
    // they paint. A panel that left the layout (closed tab, perspective switch)
    // must not keep driving its consumer (`resize_viewport_image`) from a stale
    // rect. Cleared HERE rather than in `First` because the consumers run in
    // `Update` — i.e. before this pass — and would otherwise always see an empty
    // map.
    if let Some(mut rects) = world.get_resource_mut::<PanelRects>() {
        rects.clear();
    }

    // Tell the pick gate its per-frame inputs are real this frame. (The inputs
    // themselves were cleared in `First` by `reset_scene_pick_gate`, which — unlike
    // this pass — is guaranteed to run every frame. See `ScenePickGate`.)
    if let Some(mut gate) = world.get_resource_mut::<ScenePickGate>() {
        gate.mark_rendered();
    }

    let (theme_revision, theme_changed) = {
        let theme_ref = world
            .get_resource_ref::<lunco_theme::Theme>()
            .expect("WorkbenchPlugin requires ThemePlugin");
        (theme_ref.revision(), theme_ref.is_changed())
    };
    let cache_needs_refresh = world
        .get_resource::<WorkbenchVisualsCache>()
        .is_none_or(|cache| cache.revision != theme_revision);
    let updated_theme = if theme_changed || cache_needs_refresh {
        Some(
            world
                .get_resource::<lunco_theme::Theme>()
                .expect("WorkbenchPlugin requires ThemePlugin")
                .clone(),
        )
    } else {
        None
    };
    let theme = {
        let mut cache =
            world.get_resource_or_insert_with::<WorkbenchVisualsCache>(Default::default);
        // The revision is the normal invalidation contract. Bevy change
        // detection also covers the existing public Theme fields when a host
        // mutates one directly; that keeps the derived snapshot correct while
        // those fields remain part of the public resource API. Keep the
        // immutable snapshot in an Arc so a stable frame does not deep-clone
        // the complete Theme merely to pass it through the exclusive egui
        // renderer.
        let theme_changed = cache.revision != theme_revision || theme_changed;
        if theme_changed {
            let source = updated_theme
                .as_ref()
                .expect("theme changed without a refreshed Theme snapshot");
            cache.theme = Arc::new(source.clone());
            cache.visuals = cache.theme.to_visuals();
            cache.revision = theme_revision;
        }
        if theme_changed || cache.applied_context_revision != theme_revision {
            // Context visuals are global egui state. Reapplying the same copy
            // on every frame needlessly clones and invalidates style state
            // while the workbench is otherwise only reading the cached
            // presentation.
            ctx.set_visuals(cache.visuals.clone());
            cache.applied_context_revision = theme_revision;
        }
        Arc::clone(&cache.theme)
    };

    layout_render::render_layout(&ctx, &mut layout, world, &theme, &mut menus);

    world.insert_resource(layout);
    world.insert_resource(menus);
    // No scene-pointer gate is computed here: scene picking is bevy_picking-driven
    // and egui occlusion is handled by bevy_egui's picking backend.
}

/// Return the screen rect occupied by the dock leaves containing any panel in
/// each requested group. Generic guided anchors describe authored
/// workbench slots, not particular tabs, so a stacked slot is represented by
/// the union of its leaves. The dock tree is walked once for all three groups;
/// anchor publication must not turn one layout pass into three full scans.
pub(crate) fn dock_group_rects(
    dock: &egui_dock::DockState<TabId>,
    side_browser: &[PanelId],
    right_inspector: &[PanelId],
    bottom: &[PanelId],
) -> (Option<egui::Rect>, Option<egui::Rect>, Option<egui::Rect>) {
    if side_browser.is_empty() && right_inspector.is_empty() && bottom.is_empty() {
        return (None, None, None);
    }

    let mut side_rect = None;
    let mut right_rect = None;
    let mut bottom_rect = None;
    for node in dock.main_surface().iter() {
        let egui_dock::Node::Leaf(leaf) = node else {
            continue;
        };
        let contains = |ids: &[PanelId]| {
            !ids.is_empty()
                && leaf.tabs.iter().any(|tab| match tab {
                    TabId::Singleton(id) => ids.contains(id),
                    TabId::Instance { kind, .. } => ids.contains(kind),
                })
        };
        if contains(side_browser) {
            side_rect =
                Some(side_rect.map_or(leaf.rect, |current: egui::Rect| current.union(leaf.rect)));
        }
        if contains(right_inspector) {
            right_rect =
                Some(right_rect.map_or(leaf.rect, |current: egui::Rect| current.union(leaf.rect)));
        }
        if contains(bottom) {
            bottom_rect =
                Some(bottom_rect.map_or(leaf.rect, |current: egui::Rect| current.union(leaf.rect)));
        }
    }
    (side_rect, right_rect, bottom_rect)
}

/// Record a docked panel's blocked region into the scene-pick gate.
///
/// `body` is the whole leaf content area the panel was given. What it *blocks*
/// depends on its background:
/// - **Transparent** leaf → egui_dock paints nothing, so only the card the panel
///   actually drew (`ui.min_rect()`) blocks; `body − card` is see-through and the
///   full-window 3D behind it must stay clickable.
/// - **Opaque** leaf (the default: egui_dock fills it with `tab_body.bg_fill`) →
///   the WHOLE body blocks. Recording `min_rect()` here was the bug: any panel
///   whose content is shorter than its leaf turned its own painted background into
///   a "transparent gap", so clicking the empty lower half of a Modelica panel
///   picked in the hidden 3D scene behind it. (Worse for a panel that early-returns
///   without allocating: `min_rect()` is then a zero-size rect at the leaf's
///   top-left and the entire body read as gap.)
fn record_chrome(world: &mut World, ui: &egui::Ui, body: egui::Rect, transparent: bool) {
    let card = if transparent { ui.min_rect() } else { body };
    if let Some(mut gate) = world.get_resource_mut::<ScenePickGate>() {
        gate.record_chrome_panel(body, card);
    }
}

/// `egui_dock::TabViewer` impl that delegates each tab's render to
/// the matching `Panel` (for singletons) or `InstancePanel` (for
/// multi-instance tabs), looking them up by the tab's [`TabId`].
pub(crate) struct PanelTabViewer<'a> {
    pub(crate) panels: &'a mut HashMap<PanelId, Box<dyn Panel>>,
    pub(crate) instance_panels: &'a mut HashMap<PanelId, Box<dyn InstancePanel>>,
    pub(crate) world: &'a mut World,
    pub(crate) surface: PanelSurfaceStyle,
}

/// Publish the exact screen rect for a registered panel. Generic dock-slot
/// anchors remain available for lessons about a whole slot, while named panel
/// lessons use this one registry-owned key in every Workbench render mode.
pub(crate) fn publish_panel_anchor(world: &mut World, id: PanelId, rect: egui::Rect) {
    if let Some(mut anchors) = world.get_resource_mut::<HelpAnchors>() {
        anchors.set(format!("panel.{}", id.as_str()), rect);
    }
}

impl<'a> TabViewer for PanelTabViewer<'a> {
    type Tab = TabId;

    fn title(&mut self, tab: &mut Self::Tab) -> egui::WidgetText {
        match *tab {
            TabId::Singleton(id) => match self.panels.get(&id) {
                Some(p) => p.dynamic_title(self.world).into(),
                None => format!("?{}?", id.as_str()).into(),
            },
            TabId::Instance { kind, instance } => match self.instance_panels.get(&kind) {
                Some(p) => p.title(self.world, instance).into(),
                None => format!("?{}#{}?", kind.as_str(), instance).into(),
            },
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, tab: &mut Self::Tab) {
        // Publish this panel's rect so feature-tour overlays can
        // spotlight it by id (`panel.<panel_id>`). Done before
        // render so even early-returning panels still register an
        // anchor for the current frame.
        let panel_rect = ui.max_rect();
        let measured_panel_rect = PanelRects::panel_rect_from_ui(ui);
        let panel_id = match *tab {
            TabId::Singleton(id) => id,
            TabId::Instance { kind, .. } => kind,
        };
        publish_panel_anchor(self.world, panel_id, panel_rect);

        // Publish the active tab's authoritative screen rect before rendering
        // its contents. Camera/image panels and runtime-authored surfaces both
        // consume this same geometry; neither needs to infer dock positions.
        if let Some(mut rects) = self.world.get_resource_mut::<PanelRects>() {
            match *tab {
                TabId::Singleton(_) => rects.record(panel_id, measured_panel_rect),
                TabId::Instance { instance, .. } => {
                    rects.record_instance(panel_id, instance, measured_panel_rect)
                }
            }
        }

        match *tab {
            TabId::Singleton(id) => {
                // Take-and-return pattern so the panel can itself borrow
                // other panels' metadata via the layout (future-proof).
                if let Some(mut panel) = self.panels.remove(&id) {
                    // Capability-narrowed context (no raw &mut World).
                    // Mutations the panel emits are queued and applied
                    // after paint (WP-8 structural prevention).
                    let is_main_scene =
                        panel.scene_target() == Some(PanelRenderTarget::MainViewport);
                    let transparent = panel_body_is_transparent(
                        self.world,
                        panel.transparent_background(),
                        is_main_scene,
                    );
                    // Full leaf body (egui_dock clips the tab-content ui to the
                    // whole leaf area, below the tab bar) — NOT `max_rect()`, which
                    // only spans the growable content and misses the transparent
                    // area below a short card.
                    let body = ui.clip_rect();
                    let scroll_policy = panel.scroll_policy();
                    let mut ctx = PanelCtx::with_surface(self.world, self.surface);
                    match scroll_policy {
                        PanelScrollPolicy::Vertical => {
                            egui::ScrollArea::vertical()
                                .id_salt(("workbench_panel_body", id.as_str()))
                                .auto_shrink([false; 2])
                                .show(ui, |ui| panel.render(ui, &mut ctx));
                        }
                        PanelScrollPolicy::SelfManaged => panel.render(ui, &mut ctx),
                    }
                    let intents = ctx.take_intents();
                    self.panels.insert(id, panel);
                    intents.apply(self.world);
                    if !is_main_scene {
                        record_chrome(self.world, ui, body, transparent);
                    }
                } else {
                    let error_color = self
                        .world
                        .get_resource::<lunco_theme::Theme>()
                        .map(|t| t.tokens.error)
                        .unwrap_or(egui::Color32::LIGHT_RED);
                    ui.colored_label(
                        error_color,
                        format!("Panel `{}` not registered", id.as_str()),
                    );
                }
            }
            TabId::Instance { kind, instance } => {
                if let Some(mut panel) = self.instance_panels.remove(&kind) {
                    // Instance tabs are always chrome — no `InstancePanel` hosts a
                    // live scene (the scene viewport and the USD preview are both
                    // singleton `Panel`s).
                    let transparent = panel_body_is_transparent(
                        self.world,
                        panel.transparent_background(),
                        false,
                    );
                    let body = ui.clip_rect();
                    let scroll_policy = panel.scroll_policy();
                    let mut ctx = PanelCtx::with_surface(self.world, self.surface);
                    match scroll_policy {
                        PanelScrollPolicy::Vertical => {
                            egui::ScrollArea::vertical()
                                .id_salt(("workbench_instance_panel_body", kind.as_str(), instance))
                                .auto_shrink([false; 2])
                                .show(ui, |ui| panel.render(ui, &mut ctx, instance));
                        }
                        PanelScrollPolicy::SelfManaged => panel.render(ui, &mut ctx, instance),
                    }
                    let intents = ctx.take_intents();
                    self.instance_panels.insert(kind, panel);
                    intents.apply(self.world);
                    record_chrome(self.world, ui, body, transparent);
                } else {
                    let error_color = self
                        .world
                        .get_resource::<lunco_theme::Theme>()
                        .map(|t| t.tokens.error)
                        .unwrap_or(egui::Color32::LIGHT_RED);
                    ui.colored_label(
                        error_color,
                        format!("InstancePanel kind `{}` not registered", kind.as_str()),
                    );
                }
            }
        }
    }

    fn id(&mut self, tab: &mut Self::Tab) -> egui::Id {
        egui::Id::new(("lunco_workbench_tab", tab.debug_id()))
    }

    /// Disable egui_dock's per-tab ScrollArea wrapper. Panels that
    /// need scrolling (code editor, docs view, telemetry lists) own
    /// their own ScrollArea internally; the dock-level wrapper would
    /// otherwise pull panel-local toolbars / sticky headers into the
    /// scrollable region and hide them when the body scrolls.
    fn scroll_bars(&self, _tab: &Self::Tab) -> [bool; 2] {
        [false, false]
    }

    fn clear_background(&self, tab: &Self::Tab) -> bool {
        match *tab {
            TabId::Singleton(id) => {
                let Some(panel) = self.panels.get(&id) else {
                    return true;
                };
                let is_main_scene = panel.scene_target() == Some(PanelRenderTarget::MainViewport);
                !panel_body_is_transparent(
                    self.world,
                    panel.transparent_background(),
                    is_main_scene,
                )
            }
            TabId::Instance { kind, .. } => {
                let Some(panel) = self.instance_panels.get(&kind) else {
                    return true;
                };
                !panel_body_is_transparent(self.world, panel.transparent_background(), false)
            }
        }
    }

    fn is_closeable(&self, tab: &Self::Tab) -> bool {
        match *tab {
            TabId::Singleton(id) => match self.panels.get(&id) {
                Some(panel) => panel.closable(),
                None => true,
            },
            TabId::Instance { kind, .. } => match self.instance_panels.get(&kind) {
                Some(panel) => panel.closable(),
                None => true,
            },
        }
    }

    /// Called when the user clicks the tab's × button. Returning
    /// [`OnCloseResponse::Ignore`] cancels the close; the tab stays.
    /// For multi-instance tabs we queue the id and cancel, so
    /// domain crates can confirm-on-unsaved-changes before the tab
    /// actually goes away. Singleton panels close immediately.
    fn on_close(&mut self, tab: &mut Self::Tab) -> OnCloseResponse {
        match *tab {
            TabId::Singleton(_) => OnCloseResponse::Close,
            TabId::Instance { .. } => {
                // `WorkbenchLayout` is extracted during render, so we
                // use the standalone `PendingTabCloses` resource. A
                // domain-side system drains it each frame, prompts
                // if needed, and fires `CloseTab` on user confirmation.
                self.world.resource_mut::<PendingTabCloses>().push(*tab);
                OnCloseResponse::Ignore
            }
        }
    }

    fn context_menu(&mut self, ui: &mut egui::Ui, tab: &mut Self::Tab, _path: egui_dock::NodePath) {
        // Domain hook: dispatch to the registered InstancePanel so it
        // can draw its own menu items (Pin, Open in new view, …).
        // Singletons and unknown-kind instance tabs get no extras —
        // egui_dock still surfaces its built-in "Close" item below.
        if let TabId::Instance { kind, instance } = *tab {
            // Take the panel out so it can mutably borrow `self.world`
            // freely while drawing its menu, then put it back. Mirrors
            // how `tab_ui` swaps panels in/out for render to dodge the
            // self-borrow conflict.
            if let Some(mut panel) = self.instance_panels.remove(&kind) {
                let mut ctx = PanelCtx::with_surface(self.world, self.surface);
                panel.tab_context_menu(ui, &mut ctx, instance);
                let intents = ctx.take_intents();
                self.instance_panels.insert(kind, panel);
                intents.apply(self.world);
            }
        }
    }

    fn tab_style_override(
        &self,
        tab: &Self::Tab,
        global_style: &egui_dock::TabStyle,
    ) -> Option<egui_dock::TabStyle> {
        // The viewport tab's header is dead space (the panel itself
        // renders nothing — the 3D scene shows behind). Make the tab
        // header fully invisible: transparent background, outline, and
        // text. The bar still occupies its 24-px row because
        // egui_dock 0.18 has no per-leaf hide-bar option.
        if *tab == TabId::Singleton(VIEWPORT_PANEL_ID) {
            let mut style = global_style.clone();
            let invisible = egui::Color32::TRANSPARENT;
            for s in [
                &mut style.active,
                &mut style.inactive,
                &mut style.focused,
                &mut style.hovered,
            ] {
                s.bg_fill = invisible;
                s.outline_color = invisible;
                s.text_color = invisible;
            }
            return Some(style);
        }
        None
    }
}

/// One menu row in the top-bar drop-downs: label + optional shortcut,
/// greys out when `enabled` is false and then explains itself via
/// `disabled_hint` ("No document open", "Nothing to undo", …).
///
/// Extracted because the `add_enabled(…, Button::new("Label\tShortcut"))`
/// pattern is shared by Save / Close / Undo / Redo and every copy needs the
/// disabled hint — with the hint a
/// required parameter it can't be forgotten on the next menu item.
/// Returns the [`egui::Response`] so callers can still chain
/// `.on_hover_text(…)` and `.clicked()`.
pub(crate) fn menu_item(
    ui: &mut egui::Ui,
    enabled: bool,
    label: &str,
    shortcut: &str,
    disabled_hint: &str,
) -> egui::Response {
    let text = if shortcut.is_empty() {
        label.to_owned()
    } else {
        format!("{label}\t{shortcut}")
    };
    ui.add_enabled(enabled, egui::Button::new(text))
        .on_disabled_hover_text(disabled_hint)
}

pub(crate) fn new_document_menu_label(index: usize, display: &str) -> String {
    if index == 0 {
        format!("{display}\tCtrl+N")
    } else {
        display.to_owned()
    }
}

fn settings_choice_menu<T>(
    ui: &mut egui::Ui,
    label: String,
    current: &mut T,
    choices: impl IntoIterator<Item = (T, String)>,
) -> bool
where
    T: Copy + PartialEq,
{
    let mut changed = false;
    ui.menu_button(label, |ui| {
        for (choice, text) in choices {
            if ui.selectable_label(*current == choice, text).clicked() {
                *current = choice;
                changed = true;
                ui.close();
            }
        }
    });
    changed
}

/// Run one contributed menu callback behind the capability-limited
/// [`MenuCtx`], then apply its typed intent while the workbench layout is
/// still temporarily removed from the world.
pub(crate) fn run_menu_callback(
    ui: &mut egui::Ui,
    world: &mut World,
    callback: &(dyn Fn(&mut egui::Ui, &mut MenuCtx) + Send + Sync),
) {
    let mut menu = MenuCtx::new(world);
    callback(ui, &mut menu);
    menu.take_intents().apply(world);
}

/// Render the network controls shared by the File → Network submenu.
///
/// The workbench owns only the menu surface and typed bridge events; the
/// networking adapter owns connection behavior and observes those events.
pub(crate) fn render_network_menu(ui: &mut egui::Ui, world: &mut World) {
    use lunco_core_session::{NetConnectRequest, NetDisconnectRequest, NetStatus, NetworkRole};

    let status = world
        .get_resource::<NetStatus>()
        .cloned()
        .unwrap_or_default();

    // User Profile Settings Name Input
    let mut profile = world.resource_mut::<lunco_settings::ProfileSettings>();
    let mut name_changed = false;
    ui.horizontal(|ui| {
        ui.label("Name:");
        if ui.text_edit_singleline(&mut profile.username).changed() {
            name_changed = true;
        }
    });
    if name_changed {
        let mut p = world.resource_mut::<lunco_settings::ProfileSettings>();
        p.set_changed();
    }
    ui.separator();

    match status.role {
        NetworkRole::Host => {
            ui.label(format!("Hosting · {}", status.endpoint));
            ui.separator();
            // Copy invite link. The address a guest should dial isn't
            // knowable from the host side (which interface?), so it's
            // editable — prefilled with the best-guess LAN IP:port the
            // adapter detected (`invite_hint`). The link carries the
            // self-signed cert digest in its `#fragment` so a browser
            // guest can pin it. Built inline (workbench keeps no
            // networking dep, D7); the canonical format lives in
            // `lunco_networking::connect_link`.
            let addr_id = ui.make_persistent_id("lunco_network_invite_address");
            let mut invite_addr = ui.data_mut(|d| {
                d.get_temp::<String>(addr_id)
                    .unwrap_or_else(|| status.invite_hint.clone())
            });
            ui.horizontal(|ui| {
                ui.label("Guest dials:");
                ui.text_edit_singleline(&mut invite_addr);
            });
            let digest = status.invite_digest.trim();
            let frag = if digest.is_empty() {
                String::new()
            } else {
                format!("#{digest}")
            };
            let a = invite_addr.trim();
            let enabled = !a.is_empty();
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(enabled, egui::Button::new("Copy web link"))
                    .on_hover_text("https://lunica.lunco.space/?connect=… — opens in a browser")
                    .on_disabled_hover_text("Enter the address guests should dial first")
                    .clicked()
                {
                    let link = format!("https://lunica.lunco.space/?connect={a}{frag}");
                    ui.ctx().copy_text(link);
                }
                let app_q = if digest.is_empty() {
                    String::new()
                } else {
                    format!("&digest={digest}")
                };
                if ui
                    .add_enabled(enabled, egui::Button::new("Copy app link"))
                    .on_hover_text("luncosim://connect?… — opens the desktop app")
                    .on_disabled_hover_text("Enter the address guests should dial first")
                    .clicked()
                {
                    let link = format!("luncosim://connect?address={a}{app_q}");
                    ui.ctx().copy_text(link);
                }
            });
            ui.data_mut(|d| d.insert_temp(addr_id, invite_addr));
        }
        NetworkRole::Client => {
            let state = if status.connected {
                "Connected"
            } else {
                "Connecting…"
            };
            ui.label(format!("{state} -> {}", status.endpoint));
            if ui.button("Disconnect").clicked() {
                world.trigger(NetDisconnectRequest);
                ui.close();
            }
        }
        NetworkRole::Standalone => {
            ui.label("Single-player (local)");
            ui.separator();
            // Editable address persisted in egui temp memory so it
            // survives across frames while the menu is open. Seeded
            // from the adapter's `connect_hint` (page origin / local).
            let id = ui.make_persistent_id("lunco_network_menu_address");
            let mut address = ui.data_mut(|d| {
                d.get_temp::<String>(id).unwrap_or_else(|| {
                    if status.connect_hint.is_empty() {
                        format!("127.0.0.1:{}", lunco_core_session::DEFAULT_HOST_PORT)
                    } else {
                        status.connect_hint.clone()
                    }
                })
            });
            ui.horizontal(|ui| {
                ui.label("Server:");
                ui.text_edit_singleline(&mut address);
            });
            // Optional self-signed cert digest to pin. A browser
            // joining a self-signed LAN/dev host by IP needs this
            // (it can't skip TLS validation); paste the digest the
            // host prints (`🔐 WebTransport cert digest: …`). Leave
            // blank for a CA-cert host or a native bare-IP dial.
            let digest_id = ui.make_persistent_id("lunco_network_menu_digest");
            let mut digest = ui
                .data_mut(|d| d.get_temp::<String>(digest_id))
                .unwrap_or_default();
            ui.horizontal(|ui| {
                ui.label("Cert digest:");
                ui.add(
                    text_editor::singleline(&mut digest).hint_text("optional — self-signed host"),
                );
            });
            let enabled = !address.trim().is_empty();
            if ui
                .add_enabled(enabled, egui::Button::new("Connect"))
                .on_disabled_hover_text("Enter a server address first")
                .clicked()
            {
                world.trigger(NetConnectRequest {
                    address: address.clone(),
                    digest: digest.clone(),
                });
                ui.close();
            }
            ui.data_mut(|d| d.insert_temp(id, address));
            ui.data_mut(|d| d.insert_temp(digest_id, digest));
        }
    }
}

/// Render the document-editing commands used by both the direct Edit menu and
/// the compact title-bar overflow menu.
pub(crate) fn render_edit_menu(
    ui: &mut egui::Ui,
    world: &mut World,
    menus: &mut WorkbenchMenuRegistry,
) {
    let has_active = world
        .resource::<WorkspaceResource>()
        .active_document
        .is_some();
    // Ask the domain probes whether the active document's undo/redo stacks are
    // actually non-empty; first probe to recognise the document wins (same
    // contract as the EditorIntent resolvers). No probe answering falls back
    // to plain "a document is active" so a domain that registered no probe
    // keeps working entries.
    let (can_undo, can_redo) = menus
        .undo_probes
        .iter()
        .find_map(|probe| probe(&UndoProbeCtx::new(world)))
        .unwrap_or((has_active, has_active));
    let undo_hint = if has_active {
        "Nothing to undo"
    } else {
        "No document open"
    };
    let redo_hint = if has_active {
        "Nothing to redo"
    } else {
        "No document open"
    };
    if menu_item(ui, can_undo, "Undo", "Ctrl+Z", undo_hint).clicked() {
        world.trigger(lunco_doc_bevy::EditorIntent::Undo);
        ui.close();
    }
    if menu_item(ui, can_redo, "Redo", "Ctrl+Shift+Z", redo_hint).clicked() {
        world.trigger(lunco_doc_bevy::EditorIntent::Redo);
        ui.close();
    }

    // Domain plugins (e.g. the Modelica code editor) contribute Cut/Copy/
    // Paste/Select-All here via `register_edit_menu`. The capability-limited
    // MenuCtx keeps the command path shared with the direct menu.
    let callbacks = std::mem::take(&mut menus.edit_menu);
    if !callbacks.is_empty() {
        ui.separator();
        for cb in &callbacks {
            run_menu_callback(ui, world, cb.as_ref());
        }
    }
    menus.edit_menu = callbacks;
}

/// Render Settings in either its direct top-level menu or the compact
/// overflow menu. Settings submenu sizing remains owned by the existing
/// viewport-bounded helper.
pub(crate) fn render_settings_menu(
    ui: &mut egui::Ui,
    world: &mut World,
    menus: &mut WorkbenchMenuRegistry,
) {
    ui.label(egui::RichText::new("Theme").weak().small());
    let mut theme = world.resource_mut::<lunco_theme::Theme>();
    let mode = theme.mode;

    let label = match mode {
        lunco_theme::ThemeMode::Dark => "Dark",
        lunco_theme::ThemeMode::Light => "Light",
    };

    if ui.button(label).clicked() {
        theme.toggle_mode();
    }
    ui.separator();

    // Feature areas stay discoverable without forcing the root Settings menu
    // to contain every row or fill the viewport.
    let submenus = std::mem::take(&mut menus.settings_submenus);
    for (label, callbacks) in &submenus {
        ui.menu_button(label, |ui| {
            let max_width = settings_submenu_max_width(ui.ctx().content_rect().width());
            let max_height = ui.spacing().interact_size.y * 24.0;
            egui::ScrollArea::vertical()
                .max_width(max_width)
                .max_height(max_height)
                .auto_shrink([true, true])
                .show(ui, |ui| {
                    for (i, callback) in callbacks.iter().enumerate() {
                        if i > 0 {
                            ui.separator();
                        }
                        run_menu_callback(ui, world, callback.as_ref());
                    }
                });
        });
    }
    menus.settings_submenus = submenus;
}

/// Render Help in either its direct top-level menu or the compact overflow
/// menu.
pub(crate) fn render_help_menu(
    ui: &mut egui::Ui,
    world: &mut World,
    menus: &mut WorkbenchMenuRegistry,
) {
    if let Some(identity) = world.get_resource::<lunco_workbench_core::BuildIdentity>() {
        ui.label(format!(
            "{} · {}",
            running_app_name(),
            identity.version_label()
        ));
        if let Some(source_url) = identity.source_url() {
            ui.hyperlink_to("View source commit on GitHub", source_url);
        } else {
            ui.label(
                egui::RichText::new("Source commit unavailable")
                    .weak()
                    .italics(),
            );
        }
    }
    let callbacks = std::mem::take(&mut menus.help_menu);
    if !callbacks.is_empty() {
        ui.separator();
        for cb in &callbacks {
            run_menu_callback(ui, world, cb.as_ref());
        }
    }
    menus.help_menu = callbacks;
}

/// Render Time in either its direct top-level menu or the compact overflow
/// menu. Every rate still uses the single TimeTransport command authority.
pub(crate) fn render_time_menu(
    ui: &mut egui::Ui,
    world: &mut World,
    menus: &mut WorkbenchMenuRegistry,
) {
    ui.label(egui::RichText::new("Simulation rate").weak().small());
    let (paused, rate) = world
        .get_resource::<lunco_time::TimeTransport>()
        .map(|t| (matches!(t.mode, lunco_time::TransportMode::Paused), t.rate))
        .unwrap_or((false, 1.0));

    // Every listed rate uses the same causal fixed-step path. Higher rates
    // drain more fixed iterations per render frame while the fixed timestep
    // and solver fidelity stay unchanged.
    ui.label(egui::RichText::new("Physics realtime").weak().small());
    ui.horizontal(|ui| {
        for &m in lunco_time::REALTIME_RATE_OPTIONS {
            let on = !paused && (rate - m).abs() < f64::EPSILON;
            if ui
                .selectable_label(on, lunco_time::realtime_rate_label(m))
                .on_hover_text("Run the simulation (physics included) at this rate")
                .clicked()
            {
                world.trigger(lunco_time::SetTimeTransport {
                    playing: Some(true),
                    rate: Some(m),
                });
            }
        }
    });
    if !rate.is_finite() || rate > lunco_time::MAX_REALTIME_RATE {
        // `Res<Theme>`, NOT `lunco_theme::active(ctx)`: the latter reads a
        // per-frame copy that only the Modelica canvas ever publishes, so
        // everywhere else it silently returns `Theme::dark()`.
        let warn = world
            .get_resource::<lunco_theme::Theme>()
            .map(|t| t.tokens.warning)
            .unwrap_or(egui::Color32::YELLOW);
        ui.label(egui::RichText::new(format!("Unsupported live rate: {rate:.0}x")).color(warn))
            .on_hover_text("Live transport is bounded to 64x; higher rates are rejected.");
    }

    let callbacks = std::mem::take(&mut menus.time_menu);
    if !callbacks.is_empty() {
        ui.separator();
        for cb in &callbacks {
            run_menu_callback(ui, world, cb.as_ref());
        }
    }
    menus.time_menu = callbacks;
}

/// Render registered custom menus without creating a second callback path.
pub(crate) fn render_custom_menus(
    ui: &mut egui::Ui,
    world: &mut World,
    menus: &mut WorkbenchMenuRegistry,
    mut anchors: Option<&mut Vec<(String, egui::Rect)>>,
) {
    let custom_menus = std::mem::take(&mut menus.custom_menus);
    for (name, cb) in &custom_menus {
        let response = ui.menu_button(name, |ui| {
            run_menu_callback(ui, world, cb.as_ref());
        });
        if let Some(anchors) = anchors.as_deref_mut() {
            anchors.push((name.clone(), response.response.rect));
        }
    }
    menus.custom_menus = custom_menus;
}

/// The title-bar policy is based on measured widget widths and the same
/// available width that egui gives the menu row. File and View remain direct
/// on compact windows; the registered domain menus plus secondary application
/// menus move together under More so no command is duplicated or lost.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TopMenuMode {
    Direct,
    Overflow,
}

fn measured_menu_button_width(ui: &egui::Ui, label: &str) -> f32 {
    let font = egui::TextStyle::Button.resolve(ui.style());
    ui.painter()
        .layout_no_wrap(label.to_owned(), font, ui.visuals().text_color())
        .size()
        .x
        + 2.0 * ui.spacing().button_padding.x
}

pub(crate) fn measured_menu_row_width<'a>(
    ui: &egui::Ui,
    labels: impl IntoIterator<Item = &'a str>,
) -> f32 {
    let labels: Vec<&str> = labels.into_iter().collect();
    let buttons = labels
        .iter()
        .map(|label| measured_menu_button_width(ui, label))
        .sum::<f32>();
    buttons + ui.spacing().item_spacing.x * labels.len().saturating_sub(1) as f32
}

pub(crate) fn top_menu_mode(
    available_width: f32,
    direct_menu_width: f32,
    right_controls_width: f32,
) -> TopMenuMode {
    if available_width >= direct_menu_width + right_controls_width {
        TopMenuMode::Direct
    } else {
        TopMenuMode::Overflow
    }
}

pub(crate) fn measured_titlebar_right_width(
    ui: &egui::Ui,
    layout: &WorkbenchLayout,
    titlebar_control_size: egui::Vec2,
) -> f32 {
    let tabs = perspective_switcher_tabs(layout);
    let tab_width = measured_menu_row_width(ui, tabs.iter().map(|(_, title, _)| title.as_str()));
    let transport_width = titlebar_control_size.x;
    #[cfg(all(not(target_os = "macos"), not(target_arch = "wasm32")))]
    let window_controls_width = titlebar_control_size.x * 3.0;
    #[cfg(any(target_os = "macos", target_arch = "wasm32"))]
    let window_controls_width = 0.0;
    #[cfg(all(not(target_os = "macos"), not(target_arch = "wasm32")))]
    let window_control_count = 3;
    #[cfg(any(target_os = "macos", target_arch = "wasm32"))]
    let window_control_count = 0;
    let buttons = tabs.len() + 1 + window_control_count;
    let gaps = buttons.saturating_sub(1) as f32 * ui.spacing().item_spacing.x;
    tab_width + transport_width + window_controls_width + gaps + ui.spacing().item_spacing.x * 2.0
}

pub(crate) fn truncate_title_to_width(ui: &egui::Ui, title: &str, max_width: f32) -> String {
    let font = egui::FontId::proportional(12.0);
    let color = ui.visuals().text_color();
    if max_width <= 0.0 {
        return String::new();
    }
    let width = |text: &str| {
        ui.painter()
            .layout_no_wrap(text.to_owned(), font.clone(), color)
            .size()
            .x
    };
    if width(title) <= max_width {
        return title.to_owned();
    }
    // The listening endpoint is the operational part of the title. Preserve
    // it ahead of the decorative application name when a compact gap cannot
    // fit the complete window title.
    if let Some(index) = title.find("Listening on") {
        let listening = &title[index..];
        if width(listening) <= max_width {
            return listening.to_owned();
        }
    }
    let ellipsis = "…";
    let ellipsis_width = width(ellipsis);
    if ellipsis_width > max_width {
        return String::new();
    }
    let mut chars: Vec<char> = title.chars().collect();
    while !chars.is_empty() {
        chars.pop();
        let mut candidate: String = chars.iter().collect();
        candidate.push_str(ellipsis);
        if width(&candidate) <= max_width {
            return candidate;
        }
    }
    String::new()
}

pub(crate) fn needs_full_backdrop(
    layout: &WorkbenchLayout,
    viewport_empty: bool,
    no_active_scene_camera: bool,
) -> bool {
    let scene_backed_perspective = layout.active_perspective_scene_visible_when_docked();
    (!scene_backed_perspective
        && !viewport::layout_is_empty(layout)
        && !viewport::layout_contains_panel(layout, VIEWPORT_PANEL_ID))
        || viewport_empty
        || no_active_scene_camera
}

/// Return whether the camera bound by the viewport is currently rendering.
///
/// `SceneViewport::active_camera` is the selection binding, while
/// `Camera::is_active` is the renderer's actual state. A perspective switch
/// can publish a new visible layout before the camera reconciler has activated
/// the bound camera; treating the binding alone as rendered would expose an
/// incomplete presentation during that transition.
pub(crate) fn scene_camera_is_rendering(world: &World) -> bool {
    let Some(active_camera) = world
        .get_resource::<lunco_viewport_core::SceneViewport>()
        .and_then(|viewport| viewport.active_camera)
    else {
        return false;
    };

    world
        .get_entity(active_camera)
        .ok()
        .and_then(|entity| entity.get::<Camera>())
        .is_some_and(|camera| camera.is_active)
}

/// Build the title-bar perspective entries from the registered perspectives.
/// Registration controls availability for authored flows and API commands;
/// [`Perspective::show_in_switcher`] controls only everyday navigation chrome.
pub(crate) fn perspective_switcher_tabs(
    layout: &WorkbenchLayout,
) -> Vec<(PerspectiveId, String, bool)> {
    let active = layout.active_perspective;
    layout
        .perspectives
        .iter()
        .filter(|perspective| perspective.show_in_switcher())
        .map(|perspective| {
            let id = perspective.id();
            (id, perspective.title(), active == Some(id))
        })
        // Iterate in reverse so right-to-left layout still puts them in
        // registration order from left to right.
        .rev()
        .collect()
}

/// Stable help-anchor key for a perspective tab in the title bar.
pub(crate) fn perspective_help_anchor(id: PerspectiveId) -> String {
    format!("menu.perspective.{}", id.as_str())
}

/// Render a single panel inside its own egui container (side-panel mode).
/// Mirrors PanelTabViewer's lookup-and-take-back pattern.
/// Render the bottom status strip. Reads from [`lunco_status_core::status_bus::StatusBus`]
/// (cross-cutting; populated by source library load, compile, sim, etc.) and
/// renders a click-to-expand popup with recent history.
pub(crate) fn render_status_bar_inner(
    ui: &mut egui::Ui,
    world: &mut World,
    theme: &lunco_theme::Theme,
) {
    use lunco_status_core::status_bus::{StatusBarAction, StatusBus, StatusLevel};

    let popup_id = ui.make_persistent_id("lunco_workbench_status_bar_popup");

    // Snapshot what we need from the bus into local owned values so
    // we don't hold a borrow across the popup callback (it also wants
    // to read the bus).
    struct LatestSnapshot {
        source: &'static str,
        message: String,
        level: StatusLevel,
        progress_pct: Option<f64>,
    }
    let (latest, history): (
        Option<LatestSnapshot>,
        Vec<(StatusEventKey, lunco_status_core::status_bus::StatusEvent)>,
    ) = {
        let bus = world.resource::<StatusBus>();
        let latest = bus.display_latest().map(|e| LatestSnapshot {
            source: e.source,
            message: e.message.clone(),
            level: e.level,
            progress_pct: e.progress_pct(),
        });
        let discrete: Vec<_> = bus.history().cloned().collect();
        let discrete_len = discrete.len();
        let history_total = bus.history_total();
        let mut history: Vec<_> = discrete
            .into_iter()
            .enumerate()
            .map(|(offset, event)| {
                (
                    discrete_status_event_key(history_total, discrete_len, offset),
                    event,
                )
            })
            .collect();
        history.extend(
            bus.active_progress()
                .cloned()
                .map(|event| (StatusEventKey::Progress(event.scope, event.source), event)),
        );
        history.sort_by_key(|(_, event)| event.at);
        (latest, history)
    };
    let perf_stats = world.resource::<PerfStats>().clone();
    // Raw frame times straight out of Bevy's own `Diagnostic` ring buffer — `PerfStats`
    // no longer shadows it with a second `VecDeque` holding the same values.
    let frame_history: Vec<f32> = world
        .get_resource::<bevy::diagnostic::DiagnosticsStore>()
        .map(frame_history)
        .unwrap_or_default();
    let perf_enabled = world.resource::<PerfHudSettings>().enabled;
    // The networking chip only paints when not standalone; reserve room
    // for it on the right so the clickable status region doesn't overlap.
    let net_active = world
        .get_resource::<lunco_core_session::NetStatus>()
        .map(|s| !matches!(s.role, lunco_core_session::NetworkRole::Standalone))
        .unwrap_or(false);
    let scene_name = world
        .get_resource::<CurrentSceneName>()
        .map(|s| s.0.clone())
        .unwrap_or_default();
    let scene_path = world
        .get_resource::<CurrentScenePath>()
        .map(|s| s.0.clone())
        .unwrap_or_default();
    let scene_popup_id = ui.make_persistent_id("lunco_workbench_loaded_scene_popup");
    let recent_events_width = status_popup_width(ui.ctx().content_rect().width());
    let popup_width = recent_events_width;

    ui.horizontal(|ui| {
        let bar_width = ui.available_width();
        // Reserve the exact bounded footprint of every control to the right of
        // the status scope. The controls shrink together on compact windows;
        // the left scope never competes with an unbounded label.
        let right_widths =
            status_bar_right_widths(bar_width, perf_enabled, net_active, !scene_name.is_empty());
        let right_reserve = right_widths.total();

        let status_width =
            status_bar_notification_width(bar_width, right_reserve, recent_events_width);

        // The status message scope on the left
        let latest_attention = latest
            .as_ref()
            .is_some_and(|event| event.level == StatusLevel::Attention);
        let mut attention_clicked = false;
        let response = ui
            .allocate_ui_with_layout(
                egui::vec2(status_width, 18.0),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    if let Some(l) = latest.as_ref() {
                        let dot_color = match l.level {
                            StatusLevel::Error => theme.tokens.error,
                            StatusLevel::Attention => theme.tokens.error,
                            StatusLevel::Warn => theme.tokens.warning,
                            StatusLevel::Progress | StatusLevel::Info => theme.tokens.success,
                        };
                        let attention = l.level == StatusLevel::Attention;
                        // The strip is a single-line summary surface. Keep the
                        // complete event in the tooltip and history popup, but
                        // never let diagnostic/newline-heavy payloads define
                        // the button or label's intrinsic width.
                        let display_message = status_message_summary(&l.message);
                        if attention {
                            attention_clicked = ui
                                .add_sized(
                                    [ui.available_width(), 18.0],
                                    egui::Button::new(
                                        egui::RichText::new(display_message)
                                            .small()
                                            .strong()
                                            .color(theme.tokens.error),
                                    )
                                    .truncate(),
                                )
                                .on_hover_text(&l.message)
                                .clicked();
                        } else {
                            // Painted circle instead of `●` so we don't depend
                            // on a font that ships U+25CF (the wasm build's
                            // egui font fallback chain doesn't, hence "tofu"
                            // boxes for that glyph).
                            let (rect, _) = ui
                                .allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                            ui.painter().circle_filled(rect.center(), 4.0, dot_color);
                            let notification = status_notification_layout_job(
                                ui.style(),
                                l.level,
                                l.source,
                                display_message,
                                dot_color,
                            );
                            let notification_width = status_bar_message_width(
                                ui.available_width(),
                                l.progress_pct.is_some(),
                                ui.spacing().item_spacing.x,
                            );
                            ui.add_sized(
                                [notification_width, 18.0],
                                egui::Label::new(notification).truncate(),
                            )
                            .on_hover_text(&l.message);
                            if l.level == StatusLevel::Progress {
                                if let Some(pct) = l.progress_pct {
                                    ui.add(
                                        egui::ProgressBar::new((pct as f32) / 100.0)
                                            .desired_width(120.0)
                                            .desired_height(6.0),
                                    );
                                } else {
                                    ui.spinner();
                                }
                            }
                        }
                    } else {
                        ui.label(egui::RichText::new("ready").small().weak());
                    }
                },
            )
            .response;

        // Keep the notification compact without pulling the right-hand
        // controls away from the edge of the status bar. Mark the end of the
        // clickable notification before the spacer so the alignment gap
        // cannot be mistaken for part of the recent-event surface.
        let spacer_width = (bar_width - status_width - right_reserve).max(0.0);
        if spacer_width > 0.0 {
            ui.separator();
            ui.add_space(spacer_width);
        }

        if attention_clicked {
            if let Some(source) = latest
                .as_ref()
                .filter(|event| event.level == StatusLevel::Attention)
                .map(|event| event.source)
            {
                world.trigger(StatusBarAction { source });
            } else {
                unreachable!("attention status button rendered without an attention event");
            }
        } else if !latest_attention
            && response
                .interact(egui::Sense::click())
                .on_hover_text("Click to view recent status events")
                .clicked()
        {
            egui::Popup::toggle_id(ui.ctx(), popup_id);
        }

        if !scene_name.is_empty() {
            ui.separator();
            let scene_response = ui
                .allocate_ui_with_layout(
                    egui::vec2(right_widths.scene, 18.0),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        ui.add_sized(
                            [right_widths.scene, 18.0],
                            egui::Label::new(
                                egui::RichText::new(format!("Scene: {}", scene_name)).small(),
                            )
                            .truncate()
                            .sense(egui::Sense::click()),
                        )
                    },
                )
                .inner
                .on_hover_text("Click to show the full path of the loaded USD file");
            if scene_response.clicked() {
                egui::Popup::toggle_id(ui.ctx(), scene_popup_id);
            }
            if !scene_path.is_empty() {
                egui::Popup::from_response(&scene_response)
                    .id(scene_popup_id)
                    .align(egui::RectAlign::BOTTOM_START)
                    .open_memory(None)
                    .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                    .show(|ui| {
                        ui.set_min_width(520.0);
                        ui.heading("Loaded USD file");
                        ui.separator();
                        ui.add(
                            egui::Label::new(egui::RichText::new(&scene_path).monospace()).wrap(),
                        );
                    });
            }
        }

        ui.separator();

        render_net_chip(ui, world, theme, right_widths.net);

        // Right-aligned perf segment. Hidden when the HUD is off so
        // we don't show stale zeroes; toggled via `TogglePerfHud` or
        // the Settings menu.
        if perf_enabled {
            let perf_width = status_bar_perf_width(ui.available_width(), right_widths.perf);
            if perf_width > 0.0 {
                let p99 = frame_ms_stats(&frame_history).map(|(_, _, p99)| p99);
                let (required_text, perf_text) = perf_hud_text(
                    perf_stats.fps,
                    perf_stats.frame_ms,
                    perf_stats.physics_ms,
                    p99,
                );
                ui.allocate_ui_with_layout(
                    egui::vec2(perf_width, 18.0),
                    egui::Layout::right_to_left(egui::Align::Center),
                    |ui| {
                        let item_spacing = ui.spacing().item_spacing.x;
                        let required_width = perf_text_width(ui, &required_text);
                        let sparkline_width = perf_hud_sparkline_width(
                            perf_width,
                            required_width,
                            item_spacing,
                            !frame_history.is_empty(),
                        );
                        let label_width =
                            (ui.available_width() - sparkline_width - item_spacing).max(1.0);
                        let displayed_perf_text =
                            fit_perf_text_to_width(ui, &required_text, &perf_text, label_width);
                        // `fit_perf_text_to_width` applies the HUD's
                        // required-before-optional policy. A second generic
                        // truncator would reintroduce an ellipsis after the
                        // policy selected a complete required line.
                        ui.add_sized(
                            [label_width, 18.0],
                            egui::Label::new(
                                egui::RichText::new(displayed_perf_text).small().monospace(),
                            ),
                        )
                        .on_hover_text(&perf_text);
                        draw_frame_time_sparkline(ui, &frame_history, theme, sparkline_width);
                    },
                );
            }
        }

        // egui::Popup is the post-0.31 API. `open_memory(None)` ties
        // the open state to egui's memory keyed by `popup_id`, so the
        // `toggle_popup` call above flips it.
        egui::Popup::from_response(&response)
            .id(popup_id)
            .width(popup_width)
            .align(egui::RectAlign::TOP_START)
            .layout(egui::Layout::top_down_justified(egui::Align::LEFT))
            .open_memory(None)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .frame(
                egui::Frame::new()
                    .fill(theme.tokens.overlay_backdrop)
                    .stroke(egui::Stroke::new(1.0, theme.tokens.overlay_border))
                    .corner_radius(6.0)
                    .inner_margin(egui::Margin::same(8)),
            )
            .show(|ui| {
                ui.set_min_width(popup_width);
                ui.set_max_width(popup_width);
                ui.set_max_height(360.0);
                ui.heading("Recent status events");
                ui.separator();
                let mut popup_attention_source = None;
                egui::ScrollArea::vertical()
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        if history.is_empty() {
                            ui.label(egui::RichText::new("(no events yet)").weak());
                            return;
                        }
                        // Newest first.
                        for (key, ev) in history.iter().rev() {
                            if render_status_event_row(
                                ui,
                                ev,
                                theme,
                                ui.make_persistent_id(("workbench_status_event", key)),
                            ) {
                                popup_attention_source = Some(ev.source);
                            }
                        }
                    });
                if let Some(source) = popup_attention_source {
                    world.trigger(StatusBarAction { source });
                }
            });
    });
}

const STATUS_EVENT_LEVEL_WIDTH: f32 = 56.0;
const STATUS_EVENT_SOURCE_WIDTH: f32 = 84.0;
const STATUS_EVENT_PROGRESS_WIDTH: f32 = 120.0;
const STATUS_EVENT_ATTENTION_WIDTH: f32 = 80.0;

/// Render every history item through the same level/source/message/progress
/// columns. Warn/Error rows expand their complete diagnostic when clicked,
/// while Attention adds the owning status action.
fn render_status_event_row(
    ui: &mut egui::Ui,
    event: &lunco_status_core::status_bus::StatusEvent,
    theme: &lunco_theme::Theme,
    details_id: egui::Id,
) -> bool {
    let has_details = matches!(
        event.level,
        lunco_status_core::status_bus::StatusLevel::Warn
            | lunco_status_core::status_bus::StatusLevel::Error
    );
    let mut details = egui::collapsing_header::CollapsingState::load_with_default_open(
        ui.ctx(),
        details_id,
        false,
    );
    let row_width = ui.available_width();
    let compact = row_width < 420.0;
    let level_width = if compact {
        48.0
    } else {
        STATUS_EVENT_LEVEL_WIDTH
    };
    let source_width = if compact {
        68.0
    } else {
        STATUS_EVENT_SOURCE_WIDTH
    };
    let has_progress = status_event_has_progress(event.level, event.progress);
    let progress_width = if has_progress {
        if compact {
            72.0
        } else {
            STATUS_EVENT_PROGRESS_WIDTH
        }
    } else {
        0.0
    };
    let action_width = status_event_action_width(event.level, compact);
    let column_gap = if compact {
        4.0
    } else {
        ui.spacing().item_spacing.x
    };
    let column_count = 3 + usize::from(has_progress) + usize::from(action_width > 0.0);
    let column_gaps = column_count.saturating_sub(1) as f32;
    let message_width = (row_width
        - level_width
        - source_width
        - progress_width
        - action_width
        - column_gap * column_gaps)
        .max(1.0);
    let display_message = if has_details {
        status_message_summary(&event.message)
    } else {
        event.message.as_str()
    };
    let mut attention_clicked = false;

    let row_response = ui.horizontal_top(|ui| {
        ui.set_width(row_width);
        ui.spacing_mut().item_spacing.x = column_gap;

        // Keep the level column width identical for every row so that
        // diagnostic rows do not shift the source or message columns.
        add_status_text(
            ui,
            level_width,
            egui::Label::new(
                status_event_rich_text(status_level_label(event.level))
                    .strong()
                    .color(status_level_color(event.level, theme)),
            ),
        );
        add_status_text(
            ui,
            source_width,
            egui::Label::new(status_event_rich_text(event.source).strong()).truncate(),
        )
        .on_hover_text(event.source);
        add_status_text(
            ui,
            message_width,
            egui::Label::new(status_event_rich_text(display_message))
                .wrap()
                .halign(egui::Align::LEFT),
        )
        .on_hover_text(&event.message);

        if let Some(pct) = event.progress_pct() {
            ui.add_sized(
                [progress_width, 0.0],
                egui::ProgressBar::new((pct as f32) / 100.0)
                    .desired_width(progress_width)
                    .desired_height(6.0),
            );
        } else if has_progress {
            ui.allocate_ui_with_layout(
                egui::vec2(progress_width, ui.spacing().interact_size.y),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.spinner();
                },
            );
        }

        if event.level == lunco_status_core::status_bus::StatusLevel::Attention {
            attention_clicked = ui
                .add_sized(
                    [action_width, 0.0],
                    egui::Button::new(if compact { "Act" } else { "Continue" }),
                )
                .on_hover_text("Continue")
                .clicked();
        }
    });

    if has_details {
        let row_interaction = ui
            .interact(
                row_response.response.rect,
                details_id.with("toggle"),
                egui::Sense::click(),
            )
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text(if details.is_open() {
                "Click to hide diagnostics"
            } else {
                "Click to show diagnostics"
            });
        if row_interaction.clicked() {
            details.toggle(ui);
        }
    }

    if has_details {
        details.show_body_unindented(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    status_event_rich_text("Diagnostics")
                        .strong()
                        .color(status_level_color(event.level, theme)),
                );
                if ui.small_button("Copy diagnostics").clicked() {
                    ui.ctx().copy_text(event.message.clone());
                }
            });
            ui.add(egui::Label::new(status_event_rich_text(event.message.as_str())).wrap());
        });
    }

    attention_clicked
}

/// Add fixed-width status text without egui's justified child layout.
///
/// `Ui::add_sized` creates a justified child UI. A wrapped label in that
/// layout stretches inter-word gaps to fill its column, which makes long
/// status messages appear as character-spaced text. A normal horizontal child
/// keeps the column width while leaving the label's galley proportional.
fn add_status_text(ui: &mut egui::Ui, width: f32, label: egui::Label) -> egui::Response {
    add_status_text_with_layout(
        ui,
        width,
        egui::Layout::left_to_right(egui::Align::Center),
        label,
    )
}

fn add_status_text_with_layout(
    ui: &mut egui::Ui,
    width: f32,
    layout: egui::Layout,
    label: egui::Label,
) -> egui::Response {
    ui.allocate_ui_with_layout(egui::vec2(width, 0.0), layout, |ui| ui.add(label))
        .inner
}

fn status_event_action_width(
    level: lunco_status_core::status_bus::StatusLevel,
    compact: bool,
) -> f32 {
    if level == lunco_status_core::status_bus::StatusLevel::Attention {
        if compact {
            56.0
        } else {
            STATUS_EVENT_ATTENTION_WIDTH
        }
    } else {
        0.0
    }
}

fn status_event_has_progress(
    level: lunco_status_core::status_bus::StatusLevel,
    progress: Option<(u64, u64)>,
) -> bool {
    progress.is_some() || level == lunco_status_core::status_bus::StatusLevel::Progress
}

const STATUS_BAR_PROGRESS_WIDTH: f32 = 120.0;

fn status_bar_message_width(available_width: f32, has_progress: bool, item_spacing: f32) -> f32 {
    let progress_reserve = if has_progress {
        STATUS_BAR_PROGRESS_WIDTH + item_spacing
    } else {
        0.0
    };
    (available_width - progress_reserve).max(1.0)
}

fn status_event_rich_text(text: impl Into<String>) -> egui::RichText {
    egui::RichText::new(text).family(egui::FontFamily::Proportional)
}

fn status_notification_layout_job(
    style: &egui::Style,
    level: lunco_status_core::status_bus::StatusLevel,
    source: &str,
    message: &str,
    level_color: egui::Color32,
) -> egui::text::LayoutJob {
    let font_id = egui::TextStyle::Small.resolve(style);
    let normal = egui::TextFormat {
        font_id: font_id.clone(),
        color: style.visuals.text_color(),
        ..Default::default()
    };
    let source_format = egui::TextFormat {
        font_id: font_id.clone(),
        color: style.visuals.strong_text_color(),
        ..Default::default()
    };
    let level_format = egui::TextFormat {
        font_id,
        color: level_color,
        ..Default::default()
    };
    let mut job = egui::text::LayoutJob::default();
    job.append(status_level_label(level), 0.0, level_format);
    if !source.is_empty() {
        job.append(" ", 0.0, normal.clone());
        job.append(source, 0.0, source_format);
    }
    if !message.is_empty() {
        job.append(
            if source.is_empty() { " " } else { ": " },
            0.0,
            normal.clone(),
        );
        job.append(message, 0.0, normal);
    }
    job
}

fn status_level_label(level: lunco_status_core::status_bus::StatusLevel) -> &'static str {
    match level {
        lunco_status_core::status_bus::StatusLevel::Info => "INFO",
        lunco_status_core::status_bus::StatusLevel::Warn => "WARN",
        lunco_status_core::status_bus::StatusLevel::Error => "ERROR",
        lunco_status_core::status_bus::StatusLevel::Attention => "ACTION",
        lunco_status_core::status_bus::StatusLevel::Progress => "PROGRESS",
    }
}

fn status_level_color(
    level: lunco_status_core::status_bus::StatusLevel,
    theme: &lunco_theme::Theme,
) -> egui::Color32 {
    match level {
        lunco_status_core::status_bus::StatusLevel::Error
        | lunco_status_core::status_bus::StatusLevel::Attention => theme.tokens.error,
        lunco_status_core::status_bus::StatusLevel::Warn => theme.tokens.warning,
        lunco_status_core::status_bus::StatusLevel::Info
        | lunco_status_core::status_bus::StatusLevel::Progress => theme.tokens.text_subdued,
    }
}

fn status_message_summary(message: &str) -> &str {
    message
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or(message)
}

const STATUS_POPUP_VIEWPORT_MARGIN: f32 = 24.0;
const STATUS_POPUP_VIEWPORT_RATIO: f32 = 0.5;
const STATUS_POPUP_MIN_WIDTH: f32 = 420.0;
const STATUS_POPUP_MAX_WIDTH: f32 = 960.0;
const SETTINGS_SUBMENU_MAX_WIDTH: f32 = 640.0;

/// Bound a menu's requested width to the usable egui content viewport.
///
/// Menu callbacks use the result with `Ui::set_width`, which keeps the popup
/// from growing back to an intrinsic long-label width after the callback has
/// established its content policy.
pub fn menu_popup_max_width(content_width: f32, requested_max_width: f32) -> f32 {
    let available = (content_width - STATUS_POPUP_VIEWPORT_MARGIN).max(1.0);
    available.min(requested_max_width.max(1.0))
}

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
enum StatusEventKey {
    Discrete(u64),
    Progress(lunco_status_core::status_bus::BusyScope, &'static str),
}

fn discrete_status_event_key(
    history_total: u64,
    history_len: usize,
    offset: usize,
) -> StatusEventKey {
    StatusEventKey::Discrete(
        history_total
            .saturating_sub(history_len as u64)
            .saturating_add(offset as u64),
    )
}

fn status_popup_width(content_width: f32) -> f32 {
    let available = (content_width - STATUS_POPUP_VIEWPORT_MARGIN).max(0.0);
    (available * STATUS_POPUP_VIEWPORT_RATIO)
        .clamp(STATUS_POPUP_MIN_WIDTH, STATUS_POPUP_MAX_WIDTH)
        .min(available)
}

fn settings_submenu_max_width(content_width: f32) -> f32 {
    menu_popup_max_width(content_width, SETTINGS_SUBMENU_MAX_WIDTH)
}

const STATUS_BAR_MIN_SCOPE_WIDTH: f32 = 160.0;
const STATUS_BAR_NOTIFICATION_MAX_WIDTH: f32 = 280.0;
const STATUS_BAR_NOTIFICATION_POPUP_RATIO: f32 = 0.30;
const STATUS_BAR_NOTIFICATION_MIN_WIDTH: f32 = 140.0;
const STATUS_BAR_SEPARATOR_RESERVE: f32 = 12.0;
const STATUS_BAR_BASE_OVERHEAD: f32 = 16.0;
const STATUS_BAR_SCENE_MAX_WIDTH: f32 = 150.0;
const STATUS_BAR_NET_MAX_WIDTH: f32 = 220.0;
const STATUS_BAR_PERF_MAX_WIDTH: f32 = 480.0;
/// The normal compact-window budget reserved for the FPS/frame/physics fields.
/// Optional p99 detail and the sparkline yield before these values are clipped.
const STATUS_BAR_PERF_REQUIRED_WIDTH: f32 = 360.0;
const STATUS_BAR_PERF_EDGE_INSET: f32 = 8.0;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct StatusBarRightWidths {
    scene: f32,
    net: f32,
    perf: f32,
    overhead: f32,
}

impl StatusBarRightWidths {
    fn total(self) -> f32 {
        self.scene + self.net + self.perf + self.overhead
    }
}

fn status_bar_segment_width(available_width: f32, requested_width: f32) -> f32 {
    available_width.max(0.0).min(requested_width.max(0.0))
}

fn status_bar_perf_width(available_width: f32, requested_width: f32) -> f32 {
    status_bar_segment_width(
        (available_width - STATUS_BAR_PERF_EDGE_INSET).max(0.0),
        requested_width,
    )
}

fn perf_hud_text(
    fps: f32,
    frame_ms: f32,
    physics_ms: Option<f32>,
    p99_ms: Option<f32>,
) -> (String, String) {
    // Keep the required metrics together at the front. The width-aware
    // renderer can then discard optional p99 detail without ellipsizing the
    // FPS/frame/physics readout.
    let mut required = format!("FPS {:>5.1} · {:>5.1}ms", fps, frame_ms);
    if let Some(ms) = physics_ms {
        required.push_str(&format!(" · phys {:>4.1}ms", ms));
    }

    let mut full = required.clone();
    if let Some(ms) = p99_ms {
        full.push_str(&format!(" · p99 {:>5.1}ms", ms));
    }
    (required, full)
}

fn perf_hud_sparkline_width(
    perf_width: f32,
    required_width: f32,
    item_spacing: f32,
    has_history: bool,
) -> f32 {
    if !has_history {
        return 0.0;
    }
    (perf_width - required_width - item_spacing)
        .max(0.0)
        .min(120.0)
}

fn perf_text_width(ui: &egui::Ui, text: &str) -> f32 {
    let font = egui::FontId::monospace(egui::TextStyle::Small.resolve(ui.style()).size);
    ui.painter()
        .layout_no_wrap(text.to_owned(), font, ui.visuals().text_color())
        .size()
        .x
}

fn fit_perf_text_to_width(ui: &egui::Ui, required: &str, full: &str, max_width: f32) -> String {
    fit_text_to_width(required, full, max_width, |text| perf_text_width(ui, text))
}

fn fit_text_to_width(
    required: &str,
    full: &str,
    max_width: f32,
    text_width: impl Fn(&str) -> f32,
) -> String {
    if max_width <= 0.0 {
        return String::new();
    }
    if text_width(full) <= max_width {
        return full.to_owned();
    }
    // Optional detail is intentionally omitted as a whole. This is the
    // important distinction from generic truncation: a required metric line
    // that fits must never acquire an ellipsis merely because p99 does not.
    if text_width(required) <= max_width {
        return required.to_owned();
    }

    let ellipsis = "…";
    let ellipsis_width = text_width(ellipsis);
    if ellipsis_width > max_width {
        return String::new();
    }
    let mut chars: Vec<char> = required.chars().collect();
    while !chars.is_empty() {
        chars.pop();
        let mut candidate: String = chars.iter().collect();
        candidate.push_str(ellipsis);
        if text_width(&candidate) <= max_width {
            return candidate;
        }
    }
    String::new()
}

/// Give the latest-event notification a compact, stable footprint while
/// preserving enough room for a readable single-line summary. The full event
/// remains available through the strip tooltip and history popup.
fn status_bar_notification_width(
    available_width: f32,
    right_reserve: f32,
    recent_events_width: f32,
) -> f32 {
    let available = (available_width - right_reserve).max(1.0);
    let proportional = (recent_events_width * STATUS_BAR_NOTIFICATION_POPUP_RATIO)
        .max(STATUS_BAR_NOTIFICATION_MIN_WIDTH);
    available.min(proportional.min(STATUS_BAR_NOTIFICATION_MAX_WIDTH))
}

/// Keep every right-hand status control inside the width reserved from the
/// left status scope. On compact windows the controls shrink together and
/// truncate their low-priority text instead of colliding with one another.
fn status_bar_right_widths(
    available_width: f32,
    perf_enabled: bool,
    net_active: bool,
    scene_visible: bool,
) -> StatusBarRightWidths {
    let separator_count =
        2.0 + if scene_visible { 1.0 } else { 0.0 } + if net_active { 1.0 } else { 0.0 };
    let overhead = STATUS_BAR_BASE_OVERHEAD + separator_count * STATUS_BAR_SEPARATOR_RESERVE;
    let scene = if scene_visible {
        STATUS_BAR_SCENE_MAX_WIDTH
    } else {
        0.0
    };
    let net = if net_active {
        STATUS_BAR_NET_MAX_WIDTH
    } else {
        0.0
    };
    let perf = if perf_enabled {
        STATUS_BAR_PERF_MAX_WIDTH
    } else {
        0.0
    };
    let max_controls = scene + net + perf;
    let budget = (available_width - STATUS_BAR_MIN_SCOPE_WIDTH - overhead).max(0.0);
    let perf_proportional = if max_controls > 0.0 {
        (budget * perf / max_controls).min(perf)
    } else {
        0.0
    };
    let perf = if perf > 0.0 {
        budget
            .min(perf)
            .max(perf_proportional.max(STATUS_BAR_PERF_REQUIRED_WIDTH.min(budget)))
    } else {
        0.0
    };
    let other_controls = scene + net;
    let other_scale = if other_controls > 0.0 {
        ((budget - perf).max(0.0) / other_controls).min(1.0)
    } else {
        0.0
    };

    StatusBarRightWidths {
        scene: scene * other_scale,
        net: net * other_scale,
        perf,
        overhead,
    }
}

/// Render the always-visible networking chip in the status bar.
/// Reads `lunco_core_session::NetStatus` (always present; populated by the
/// optional `lunco-networking` adapter when it's wired). Silent (zero pixels)
/// in single-player (`Standalone`), so non-networked apps show nothing.
///
/// - **Host**: green dot, `HOST :PORT · N peers` (this window's listen port).
/// - **Client (connected)**: green dot, `CLIENT → host:port`.
/// - **Client (connecting)**: amber dot, `connecting → host:port`.
fn render_net_chip(ui: &mut egui::Ui, world: &mut World, theme: &lunco_theme::Theme, width: f32) {
    use lunco_core_session::{NetStatus, NetworkRole};
    let Some(status) = world.get_resource::<NetStatus>().cloned() else {
        return;
    };
    let (dot, label) = match status.role {
        // Single-player — the wire is inert, so show nothing.
        NetworkRole::Standalone => return,
        NetworkRole::Host => {
            let s = if status.peers == 1 { "" } else { "s" };
            (
                theme.tokens.success,
                format!("HOST {} · {} peer{s}", status.endpoint, status.peers),
            )
        }
        NetworkRole::Client if status.connected => (
            theme.tokens.success,
            format!("CLIENT → {}", status.endpoint),
        ),
        NetworkRole::Client => (
            theme.tokens.warning,
            format!("connecting → {}", status.endpoint),
        ),
    };
    ui.allocate_ui_with_layout(
        egui::vec2(width, 18.0),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
            ui.painter().circle_filled(rect.center(), 4.0, dot);
            let label_width = (ui.available_width() - ui.spacing().item_spacing.x).max(1.0);
            ui.add_sized(
                [label_width, 18.0],
                egui::Label::new(egui::RichText::new(label).small()).truncate(),
            )
            .on_hover_text("LunCoSim networking");
        },
    );
    ui.separator();
}

/// Draws a small frame-time sparkline in the status bar so spikes
/// the smoothed `FPS` number hides become visible. Y axis auto-
/// scales to whatever the worst recent sample was; a faint reference
/// line at 16.67 ms (60 FPS) anchors the eye.
fn draw_frame_time_sparkline(
    ui: &mut egui::Ui,
    frame_history: &[f32],
    theme: &lunco_theme::Theme,
    width: f32,
) {
    if frame_history.is_empty() {
        return;
    }
    // Plot dimensions chosen to fit the 18 px-tall status bar with
    // a few px of breathing room.
    let size = egui::vec2(width, 14.0);
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let painter = ui.painter().with_clip_rect(rect);

    // Auto-scale: top of the plot is the worst recent sample, but
    // never below ~25 ms so a calm 60 FPS run doesn't make 1 ms
    // jitter look like a spike.
    let max_ms: f32 = frame_history
        .iter()
        .copied()
        .fold(0.0_f32, f32::max)
        .max(25.0_f32);

    // 16.67 ms (60 FPS) reference line — pulls from `text_subdued`
    // and softens with alpha so it doesn't compete with the trace.
    let muted = theme.tokens.text_subdued;
    let muted_soft = muted.alpha(80);
    let ref_y = rect.bottom() - rect.height() * (16.67 / max_ms).min(1.0);
    painter.line_segment(
        [
            egui::pos2(rect.left(), ref_y),
            egui::pos2(rect.right(), ref_y),
        ],
        egui::Stroke::new(0.5, muted_soft),
    );

    let n = frame_history.len();
    let step = rect.width() / (lunco_workbench_perf_ui::FRAME_HISTORY_LEN - 1).max(1) as f32;
    let mut prev: Option<egui::Pos2> = None;
    for (i, ms) in frame_history.iter().enumerate() {
        let x = rect.left() + i as f32 * step;
        let y = rect.bottom() - rect.height() * (*ms / max_ms).clamp(0.0, 1.0);
        let here = egui::pos2(x, y);
        // Per-sample colour: success ≤16.67 ms, warning ≤33 ms, error above.
        let colour = if *ms <= 16.67 {
            theme.tokens.success
        } else if *ms <= 33.34 {
            theme.tokens.warning
        } else {
            theme.tokens.error
        };
        if let Some(p) = prev {
            painter.line_segment([p, here], egui::Stroke::new(1.0, colour));
        }
        prev = Some(here);
    }
    // Outline so the plot reads as a chart, not random pixels.
    let outline = muted.alpha(100);
    painter.rect_stroke(
        rect,
        0.0,
        egui::Stroke::new(0.5, outline),
        egui::StrokeKind::Inside,
    );
    let _ = n;
}

pub(crate) fn render_panel_solo(
    ui: &mut egui::Ui,
    id: &PanelId,
    layout: &mut WorkbenchLayout,
    world: &mut World,
    surface: PanelSurfaceStyle,
) {
    if let Some(panel) = layout.panels.get(id) {
        ui.label(egui::RichText::new(panel.title()).strong());
        ui.separator();
    }
    if let Some(mut panel) = layout.panels.remove(id) {
        let scroll_policy = panel.scroll_policy();
        let mut ctx = PanelCtx::with_surface(world, surface);
        match scroll_policy {
            PanelScrollPolicy::Vertical => {
                egui::ScrollArea::vertical()
                    .id_salt(("workbench_solo_panel_body", id.as_str()))
                    .auto_shrink([false; 2])
                    .show(ui, |ui| panel.render(ui, &mut ctx));
            }
            PanelScrollPolicy::SelfManaged => panel.render(ui, &mut ctx),
        }
        let intents = ctx.take_intents();
        layout.panels.insert(*id, panel);
        intents.apply(world);
    } else {
        let error_color = world
            .get_resource::<lunco_theme::Theme>()
            .map(|t| t.tokens.error)
            .unwrap_or(egui::Color32::LIGHT_RED);
        ui.colored_label(
            error_color,
            format!("Panel `{}` not registered", id.as_str()),
        );
    }
}

pub(crate) fn register_workbench_appearance_settings_menu(world: &mut World) {
    use bevy_egui::egui;
    let Some(mut menus) = world.get_resource_mut::<WorkbenchMenuRegistry>() else {
        return;
    };
    menus.register_settings_submenu("Appearance", |ui, ctx| {
        let Some(mut settings) = ctx
            .resource::<WorkbenchAppearanceSettings>()
            .copied()
        else {
            return;
        };
        let original = settings;
        ui.checkbox(
            &mut settings.translucent_tab_content,
            "Translucent tab content",
        )
        .on_hover_text(
            "Keep the 3D scene visible behind every tab body and its standard panel surface. "
                .to_string(),
        );
        ui.label(
            egui::RichText::new(
                "On uses a themed translucent surface; off uses the same opaque background in every tab.",
            )
            .weak()
            .small(),
        );
        if settings != original {
            ctx.set_resource(settings);
        }
    });
}

pub(crate) fn register_graphics_settings_menu(world: &mut World) {
    use bevy_egui::egui;
    let Some(mut menus) = world.get_resource_mut::<WorkbenchMenuRegistry>() else {
        return;
    };
    menus.register_settings_submenu("Graphics", |ui, ctx| {
        ui.label(egui::RichText::new("Rendering").weak().small());
        if let Some(current) = ctx.resource::<lunco_render::RenderingQualitySettings>() {
            let mut settings = *current;
            let current_preset = settings.preset();
            let mut selected_preset = current_preset.unwrap_or(lunco_render::RenderingQuality::High);
            let preset_label = current_preset.map_or("Custom", |preset| preset.label());
            let preset_changed = settings_choice_menu(
                ui,
                format!("Rendering quality: {preset_label}"),
                &mut selected_preset,
                lunco_render::RenderingQuality::all()
                    .into_iter()
                    .map(|quality| (quality, quality.label().to_owned())),
            );
            if preset_changed {
                settings.apply_preset(selected_preset);
            }
            let shadow_map_sizes = ctx
                .resource::<lunco_render_recovery::RenderCapabilities>()
                .and_then(|capabilities| capabilities.supported_shadow_map_sizes());
            ui.label(
                egui::RichText::new(
                    "High is the highest shipped visual budget; USD-authored sky and lunar surface shaders remain authoritative.",
                )
                .weak()
                .small(),
            );
            ui.label(
                egui::RichText::new(
                    "Presets only suggest values. The fields below are authoritative and are never silently downgraded to another preset.",
                )
                .weak()
                .small(),
            );
            ui.collapsing("Shadow allocation", |ui| {
                if let Some(sizes) = shadow_map_sizes.as_deref() {
                    settings_choice_menu(
                        ui,
                        format!(
                            "Directional map: {} px",
                            settings.directional_shadow_map_size
                        ),
                        &mut settings.directional_shadow_map_size,
                        sizes
                            .iter()
                            .copied()
                            .map(|size| (size, format!("{size} px"))),
                    );
                    settings_choice_menu(
                        ui,
                        format!("Point map: {} px", settings.point_shadow_map_size),
                        &mut settings.point_shadow_map_size,
                        sizes
                            .iter()
                            .copied()
                            .map(|size| (size, format!("{size} px"))),
                    );
                } else {
                    ui.label(
                        egui::RichText::new(
                            "Shadow-map sizes are unavailable until the render device reports its limits.",
                        )
                        .weak()
                        .small(),
                    );
                }
                ui.add(
                    egui::DragValue::new(&mut settings.directional_cascades)
                        .speed(1.0)
                        .range(1..=bevy::pbr::MAX_CASCADES_PER_LIGHT)
                        .prefix("Directional cascades: "),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.max_directional_shadow_casters)
                        .speed(1.0)
                        .range(0..=bevy::pbr::MAX_DIRECTIONAL_LIGHTS)
                        .prefix("Directional shadow casters: "),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.max_point_shadow_casters)
                        .speed(1.0)
                        .prefix("Point shadow casters: "),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.max_spot_shadow_casters)
                        .speed(1.0)
                        .prefix("Spot shadow casters: "),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.shadow_depth_bias)
                        .speed(0.005)
                        .prefix("Shadow depth bias: "),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.shadow_normal_bias)
                        .speed(0.1)
                        .prefix("Shadow normal bias: "),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.shadow_budget_bytes)
                        .speed(1024.0 * 1024.0)
                        .range(1..=u64::MAX)
                        .prefix("Logical shadow byte ceiling: ")
                        .suffix(" bytes"),
                );
                ui.label(
                    egui::RichText::new(
                        "This explicit Depth32 shadow-storage ceiling must cover the configured caster limits. It never changes map sizes, cascades, or caster limits automatically; adapter limits are reported separately.",
                    )
                    .weak()
                    .small(),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.shadow_minimum_distance)
                        .speed(0.1)
                        .prefix("Shadow minimum distance: ")
                        .suffix(" m"),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.shadow_first_cascade_far_bound)
                        .speed(1.0)
                        .prefix("First cascade far bound: ")
                        .suffix(" m"),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.shadow_maximum_distance)
                        .speed(10.0)
                        .prefix("Maximum shadow distance: ")
                        .suffix(" m"),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.shadow_cascade_overlap)
                        .speed(0.01)
                        .prefix("Cascade overlap: "),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.local_light_default_range)
                        .speed(1.0)
                        .prefix("Local-light default range: ")
                        .suffix(" m"),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.local_shadow_map_near_z)
                        .speed(0.01)
                        .prefix("Local shadow near Z: ")
                        .suffix(" m"),
                );
            });
            ui.collapsing("Horizon terrain shadows", |ui| {
                ui.checkbox(
                    &mut settings.horizon_shadow_cache_enabled,
                    "Use pre-baked horizon shadow cache",
                );
                ui.add(
                    egui::DragValue::new(&mut settings.horizon_shadow_cache_sun_threshold_deg)
                        .speed(0.01)
                        .range(0.001..=179.0)
                        .prefix("Cache refresh angle: ")
                        .suffix("°"),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.horizon_march_steps)
                        .speed(1.0)
                        .range(1..=4096)
                        .prefix("Live march steps: "),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.horizon_cache_samples_per_axis)
                        .speed(1.0)
                        .range(1..=8)
                        .prefix("Cache samples per axis: "),
                );
                ui.label(
                    egui::RichText::new(
                        "These are explicit terrain-shadow quality controls. Cache use and bake sampling are never changed automatically by the platform or memory budget.",
                    )
                    .weak()
                    .small(),
                );
            });
            ui.collapsing("Light defaults", |ui| {
                ui.label(
                    egui::RichText::new(
                        "These values apply only when a USD light omits its intensity; authored USD intensity remains authoritative.",
                    )
                    .weak()
                    .small(),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.distant_light_default_illuminance)
                        .speed(1_000.0)
                        .prefix("Distant-light default: ")
                        .suffix(" lx"),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.local_light_default_intensity)
                        .speed(100.0)
                        .prefix("Sphere-light default: ")
                        .suffix(" lm"),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.rect_light_default_intensity)
                        .speed(100.0)
                        .prefix("Rect-light default: ")
                        .suffix(" lm"),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.dome_default_intensity)
                        .speed(100.0)
                        .prefix("Textured-dome default: ")
                        .suffix(" cd/m²"),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.dome_cubemap_face_size)
                        .speed(128.0)
                        .range(1..=4096)
                        .prefix("Dome cubemap face size: ")
                        .suffix(" px (power of two)"),
                );
            });
            ui.collapsing("Parametric surfaces", |ui| {
                ui.label(
                    egui::RichText::new(
                        "NURBS tessellation controls mesh detail only; USD control nets, orders, and authored trim data remain authoritative.",
                    )
                    .weak()
                    .small(),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.nurbs_surface_samples_per_control_span)
                        .speed(1.0)
                        .range(1..=64)
                        .prefix("Samples per control span: "),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.nurbs_surface_minimum_subdivisions)
                        .speed(1.0)
                        .range(1..=4096)
                        .prefix("Surface minimum subdivisions: "),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.nurbs_surface_maximum_subdivisions)
                        .speed(1.0)
                        .range(1..=4096)
                        .prefix("Surface maximum subdivisions: "),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.nurbs_trim_curve_samples)
                        .speed(1.0)
                        .range(1..=4096)
                        .prefix("Trim-curve samples: "),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.nurbs_trim_minimum_subdivisions)
                        .speed(1.0)
                        .range(1..=4096)
                        .prefix("Trim minimum subdivisions: "),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.nurbs_trim_maximum_subdivisions)
                        .speed(1.0)
                        .range(1..=4096)
                        .prefix("Trim maximum subdivisions: "),
                );
            });
            ui.collapsing("Primitive meshes", |ui| {
                ui.label(
                    egui::RichText::new(
                        "These settings control viewer tessellation for USD spheres, cylinders, cones, and capsules; USD dimensions remain authoritative.",
                    )
                    .weak()
                    .small(),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.primitive_sphere_longitudes)
                        .speed(1.0)
                        .range(3..=4096)
                        .prefix("Sphere longitudes: "),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.primitive_sphere_latitudes)
                        .speed(1.0)
                        .range(2..=4096)
                        .prefix("Sphere latitudes: "),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.primitive_radial_segments)
                        .speed(1.0)
                        .range(3..=4096)
                        .prefix("Cylinder/cone radial segments: "),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.primitive_capsule_longitudes)
                        .speed(1.0)
                        .range(3..=4096)
                        .prefix("Capsule longitudes: "),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.primitive_capsule_latitudes)
                        .speed(1.0)
                        .range(2..=4096)
                        .prefix("Capsule latitudes: "),
                );
            });
            ui.collapsing("Curve tubes", |ui| {
                ui.label(
                    egui::RichText::new(
                        "These settings control only the viewer tessellation of USD curve tubes; curve points, widths, and topology remain authored USD data.",
                    )
                    .weak()
                    .small(),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.curve_samples_per_segment)
                        .speed(1.0)
                        .range(1..=4096)
                        .prefix("Samples per curve segment: "),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.curve_radial_segments)
                        .speed(1.0)
                        .range(3..=4096)
                        .prefix("Tube radial segments: "),
                );
            });
            ui.collapsing("Camera look", |ui| {
                ui.label(
                    egui::RichText::new(
                        "These settings apply to scene cameras when USD does not author an environment bloom override.",
                    )
                    .weak()
                    .small(),
                );
                let tone_map_label = match settings.camera_tone_map {
                    lunco_render::ToneMap::None => "None",
                    lunco_render::ToneMap::TonyMcMapface => "TonyMcMapface",
                    lunco_render::ToneMap::AgX => "AgX",
                    lunco_render::ToneMap::AcesFitted => "ACES fitted",
                    lunco_render::ToneMap::Reinhard => "Reinhard",
                };
                settings_choice_menu(
                    ui,
                    format!("Tone map: {tone_map_label}"),
                    &mut settings.camera_tone_map,
                    [
                        (lunco_render::ToneMap::None, "None"),
                        (lunco_render::ToneMap::TonyMcMapface, "TonyMcMapface"),
                        (lunco_render::ToneMap::AgX, "AgX"),
                        (lunco_render::ToneMap::AcesFitted, "ACES fitted"),
                        (lunco_render::ToneMap::Reinhard, "Reinhard"),
                    ]
                    .into_iter()
                    .map(|(tone_map, label)| (tone_map, label.to_owned())),
                );
                let msaa_label = match settings.camera_msaa {
                    lunco_render::MsaaLevel::Off => "Off",
                    lunco_render::MsaaLevel::X2 => "2x",
                    lunco_render::MsaaLevel::X4 => "4x",
                };
                settings_choice_menu(
                    ui,
                    format!("Camera MSAA: {msaa_label}"),
                    &mut settings.camera_msaa,
                    [
                        (lunco_render::MsaaLevel::Off, "Off"),
                        (lunco_render::MsaaLevel::X2, "2x"),
                        (lunco_render::MsaaLevel::X4, "4x"),
                    ]
                    .into_iter()
                    .map(|(msaa, label)| (msaa, label.to_owned())),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.camera_exposure_ev100)
                        .speed(0.1)
                        .prefix("Unauthored camera exposure (EV100): "),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.camera_bloom_intensity)
                        .speed(0.01)
                        .prefix("Bloom intensity: "),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.camera_bloom_low_frequency_boost)
                        .speed(0.01)
                        .prefix("Bloom low-frequency boost: "),
                );
                ui.label(
                    egui::RichText::new(
                        "A positive bloom intensity enables HDR. An authored USD environment value wins over this default; no automatic quality downgrade is applied.",
                    )
                    .weak()
                    .small(),
                );
            });
            ui.collapsing("Presentation recovery", |ui| {
                ui.label(
                    egui::RichText::new(
                        "These are safety timings for render failures, not quality fallbacks. The renderer never changes quality automatically.",
                    )
                    .weak()
                    .small(),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.render_failure_quiet_period_secs)
                        .speed(0.1)
                        .range(0.01..=settings.render_failure_give_up_after_secs)
                        .prefix("Failure quiet period: ")
                        .suffix(" s"),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.render_failure_give_up_after_secs)
                        .speed(0.5)
                        .range(settings.render_failure_quiet_period_secs..=3600.0)
                        .prefix("Stop presentation after: ")
                        .suffix(" s"),
                );
            });
            ui.collapsing("Terrain mesh cache", |ui| {
                ui.add(
                    egui::DragValue::new(&mut settings.terrain_mesh_cache_bytes)
                        .speed(16.0 * 1024.0 * 1024.0)
                        .range(1..=u64::MAX)
                        .prefix("Mesh-cache byte ceiling: ")
                        .suffix(" bytes"),
                );
                ui.label(
                    egui::RichText::new(
                        "The cache evicts least-recently-used meshes at this explicit ceiling; terrain detail is not silently downgraded.",
                    )
                    .weak()
                    .small(),
                );
            });
            ui.collapsing("Terrain derived maps", |ui| {
                ui.add(
                    egui::DragValue::new(&mut settings.terrain_derived_map_resolution)
                        .speed(128.0)
                        .range(1..=4096)
                        .prefix("Map resolution: ")
                        .suffix(" px/side (power of two)"),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.terrain_derived_ao_directions)
                        .speed(1.0)
                        .range(1..=64)
                        .prefix("AO directions: "),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.terrain_derived_ao_steps)
                        .speed(1.0)
                        .range(1..=64)
                        .prefix("AO steps per direction: "),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.terrain_derived_ao_radius_fraction)
                        .speed(0.01)
                        .range(0.01..=1.0)
                        .prefix("AO radius fraction: "),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.terrain_derived_roughness_base)
                        .speed(0.01)
                        .range(0.0..=1.0)
                        .prefix("Flat-ground roughness: "),
                );
                ui.add(
                    egui::DragValue::new(
                        &mut settings.terrain_derived_roughness_saturation_radians,
                    )
                    .speed(0.01)
                    .range(0.01..=std::f32::consts::FRAC_PI_2)
                    .prefix("Roughness saturation slope: ")
                    .suffix(" rad"),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.terrain_derived_texture_anisotropy)
                        .speed(1.0)
                        .range(1..=16)
                        .prefix("Derived-texture anisotropy: "),
                );
                ui.label(
                    egui::RichText::new(
                        "These settings control the baked terrain roughness, ambient-occlusion, normal textures, and filtering. Changes rebake off-thread and keep the previous maps visible until ready.",
                    )
                    .weak()
                    .small(),
                );
            });
            ui.collapsing("Terrain rocks", |ui| {
                ui.add(
                    egui::DragValue::new(&mut settings.terrain_rock_max_instances)
                        .speed(128.0)
                        .range(1..=1_000_000)
                        .prefix("Maximum rock instances: "),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.terrain_rock_mesh_buckets)
                        .speed(1.0)
                        .range(2..=64)
                        .prefix("Rock size buckets: "),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.terrain_rock_mesh_cube_count)
                        .speed(1.0)
                        .range(1..=64)
                        .prefix("Boxes per rock mesh: "),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.terrain_rock_lod_start_distance)
                        .speed(10.0)
                        .range(0.0..=100_000.0)
                        .prefix("Rock LOD start: ")
                        .suffix(" m"),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.terrain_rock_lod_fade_distance)
                        .speed(10.0)
                        .range(0.1..=100_000.0)
                        .prefix("Rock LOD fade: ")
                        .suffix(" m"),
                );
                ui.label(
                    egui::RichText::new(
                        "The instance limit is explicit: authored density is never silently reduced by a hidden renderer cap. Mesh detail and native visibility distances are Graphics settings.",
                    )
                    .weak()
                    .small(),
                );
            });
            ui.collapsing("Terrain LOD", |ui| {
                ui.add(
                    egui::DragValue::new(&mut settings.terrain_lod_tile_resolution)
                        .speed(2.0)
                        .range(3..=4097)
                        .prefix("Streamed tile resolution: "),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.terrain_lod_cinematic_resolution)
                        .speed(2.0)
                        .range(3..=4097)
                        .prefix("Cinematic tile resolution: "),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.terrain_lod_pixel_error)
                        .speed(0.1)
                        .range(0.1..=32.0)
                        .prefix("Screen error: ")
                        .suffix(" px"),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.terrain_lod_max_depth)
                        .speed(1.0)
                        .range(1..=20)
                        .prefix("Max depth: "),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.terrain_lod_probe_resolution)
                        .speed(2.0)
                        .range(3..=257)
                        .prefix("Error probe resolution: "),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.terrain_lod_bakes_per_frame)
                        .speed(1.0)
                        .range(1..=256)
                        .prefix("Bakes per frame: "),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.terrain_lod_max_inflight_bakes)
                        .speed(1.0)
                        .range(1..=512)
                        .prefix("In-flight bakes: "),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.terrain_lod_tile_budget)
                        .speed(16.0)
                        .range(1..=8192)
                        .prefix("Selected tile budget: "),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.terrain_lod_cover_edits_per_frame)
                        .speed(4.0)
                        .range(1..=4096)
                        .prefix("Cover edits per frame: "),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.terrain_lod_hysteresis_ratio)
                        .speed(0.01)
                        .range(1.01..=4.0)
                        .prefix("LOD hysteresis ratio: "),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.terrain_lod_morph_start_ratio)
                        .speed(0.01)
                        .range(0.0..=0.99)
                        .prefix("Geomorph start ratio: "),
                );
                ui.label(
                    egui::RichText::new(
                        "These are explicit terrain rendering controls. A custom value is applied as authored; the renderer does not silently choose a lower preset.",
                    )
                    .weak()
                    .small(),
                );
            });
            let validation_error = settings.validate().err().map(str::to_owned).or_else(|| {
                let capabilities = ctx
                    .resource::<lunco_render_recovery::RenderCapabilities>()
                    .filter(|capabilities| capabilities.is_ready())?;
                lunco_render_recovery::validate_profile_for_capabilities(
                    settings.profile(),
                    capabilities,
                )
                .err()
            });
            if let Some(reason) = validation_error {
                let error_color = ctx
                    .resource::<lunco_theme::Theme>()
                    .map(|theme| theme.tokens.error)
                    .unwrap_or(egui::Color32::LIGHT_RED);
                ui.colored_label(
                    error_color,
                    format!("Graphics settings rejected: {reason}"),
                );
            }
            // Keep invalid edits in memory so dependent fields can be corrected
            // over multiple UI interactions. Runtime consumers validate at their
            // own boundaries and preserve the last applied quality; the settings
            // persister likewise refuses to replace the last valid disk value.
            if settings != *current {
                ctx.set_resource(settings);
            }
        }

        ui.separator();
        if let Some(current) = ctx.resource::<lunco_render::CommunicationLineSettings>() {
            let mut settings = *current;
            ui.checkbox(&mut settings.show, "Show communication lines")
                .on_hover_text(
                    "Display runtime connectivity beams between communication endpoints. \
                     Off by default; the setting affects only this viewer.",
                );
            if settings != *current {
                ctx.set_resource(settings);
            }
        }

        ui.separator();
        ui.label(egui::RichText::new("Terrain").weak().small());
        let Some(mut settings) = ctx.resource::<lunco_settings::TerrainSettings>().cloned() else {
            return;
        };
        let original = settings.clone();
        ui.add(
            egui::Slider::new(&mut settings.visual_detail_radius_m, 5.0..=200.0)
                .text("Camera detail radius (m)"),
        )
        .on_hover_text(
            "Distance around each active terrain camera that requests the finest \
             available terrain geometry. Persisted to the shared LunCoSim settings file.",
        );
        ui.add(
            egui::Slider::new(&mut settings.visual_detail_hysteresis_m, 0.0..=200.0)
                .text("Camera detail retention (m)"),
        )
        .on_hover_text(
            "Extra distance that keeps already-refined tiles resident while the \
             camera moves away, preventing fine-to-coarse-to-fine flicker. \
             Persisted to the shared LunCoSim settings file.",
        );
        if settings != original {
            ctx.set_resource(settings);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_dock::DockState;
    use lunco_workbench_core::PerspectiveLayoutPlan;
    use lunco_workbench_core::PerspectiveSlotPlan;
    use lunco_workbench_layout::heal_non_finite_nulls;

    #[test]
    fn tab_content_setting_uses_translucent_default_for_panel_chrome() {
        let mut world = World::new();

        // Standalone panel harnesses retain the panel declaration when the
        // workbench appearance resource is not installed.
        assert!(panel_body_is_transparent(&world, true, false));

        world.insert_resource(WorkbenchAppearanceSettings::default());
        // The application default uses a translucent body, which still blocks
        // scene input across the complete panel leaf.
        assert!(!panel_body_is_transparent(&world, true, false));

        world
            .resource_mut::<WorkbenchAppearanceSettings>()
            .translucent_tab_content = false;
        assert!(!panel_body_is_transparent(&world, true, false));
        // The scene host is always transparent, independent of the preference.
        assert!(panel_body_is_transparent(&world, false, true));
    }

    #[test]
    fn tab_content_setting_deserializes_empty_section() {
        let settings: WorkbenchAppearanceSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(settings, WorkbenchAppearanceSettings::default());
    }

    #[test]
    fn panel_surface_fill_uses_themed_translucency() {
        let theme = lunco_theme::Theme::dark();
        let translucent = panel_surface_fill(&theme, true);

        assert_eq!(translucent, theme.tokens.overlay_backdrop);
        assert!(translucent.a() > 0 && translucent.a() < u8::MAX);
        assert_eq!(panel_surface_fill(&theme, false), theme.colors.mantle);
    }

    #[test]
    fn build_identity_formats_the_shared_version_label() {
        let identity = lunco_workbench_core::BuildIdentity::new(
            "0.6.0-nightly.37.1",
            "abc12345-dirty",
            "https://github.com/LunCoSim/lunco-sim",
        );

        assert_eq!(
            identity.version_label(),
            "Version 0.6.0-nightly.37.1 (abc12345-dirty)"
        );
        assert_eq!(
            identity.source_url().as_deref(),
            Some("https://github.com/LunCoSim/lunco-sim/commit/abc12345")
        );
    }

    #[test]
    fn new_document_shortcut_only_labels_the_default_entry() {
        assert_eq!(
            new_document_menu_label(0, "Modelica Model"),
            "Modelica Model\tCtrl+N"
        );
        assert_eq!(new_document_menu_label(1, "USD Stage"), "USD Stage");
    }

    /// `FocusPanel` can arrive from another UI/domain plugin before (or without)
    /// `WorkbenchPlugin`; an absent dock is a valid no-op state, not a fatal
    /// observer-parameter error.
    #[test]
    fn focus_panel_without_workbench_layout_does_not_panic() {
        let mut app = App::new();
        app.add_observer(on_focus_panel);
        app.world_mut().trigger(FocusPanel {
            id: "not_mounted".into(),
        });
    }

    #[test]
    fn backdrop_waits_for_the_bound_camera_to_be_active() {
        let mut world = World::new();
        let camera = world
            .spawn(Camera {
                is_active: false,
                ..default()
            })
            .id();
        world.insert_resource(lunco_viewport_core::SceneViewport {
            active_camera: Some(camera),
            ..default()
        });

        assert!(!scene_camera_is_rendering(&world));

        world
            .entity_mut(camera)
            .get_mut::<Camera>()
            .unwrap()
            .is_active = true;
        assert!(scene_camera_is_rendering(&world));
    }

    struct SceneBackedTestPerspective;

    impl Perspective for SceneBackedTestPerspective {
        fn id(&self) -> PerspectiveId {
            PerspectiveId("scene_backed_test")
        }

        fn title(&self) -> String {
            "Scene-backed test".into()
        }

        fn scene_visible_when_docked(&self) -> bool {
            true
        }

        fn layout(&self) -> PerspectiveLayoutPlan {
            PerspectiveLayoutPlan::new()
        }
    }

    #[test]
    fn scene_backed_dock_does_not_paint_opaque_backdrop() {
        let mut layout = WorkbenchLayout::default();
        layout.register_perspective(SceneBackedTestPerspective);
        layout.right_inspector.push(PanelId("command_deck"));

        assert!(!needs_full_backdrop(&layout, false, false));
        assert!(needs_full_backdrop(&layout, true, false));
        assert!(needs_full_backdrop(&layout, false, true));
    }

    #[test]
    fn status_popup_width_is_compact_but_uses_available_viewport() {
        assert_eq!(status_popup_width(200.0), 176.0);
        assert_eq!(status_popup_width(320.0), 296.0);
        assert_eq!(status_popup_width(1024.0), 500.0);
        assert_eq!(status_popup_width(1920.0), 948.0);
        assert_eq!(status_popup_width(4000.0), STATUS_POPUP_MAX_WIDTH);
    }

    #[test]
    fn settings_submenu_width_is_content_bounded_by_the_viewport() {
        assert_eq!(settings_submenu_max_width(200.0), 176.0);
        assert_eq!(
            settings_submenu_max_width(1024.0),
            SETTINGS_SUBMENU_MAX_WIDTH
        );
        assert_eq!(
            settings_submenu_max_width(4000.0),
            SETTINGS_SUBMENU_MAX_WIDTH
        );
    }

    #[test]
    fn menu_popup_width_never_exceeds_the_content_viewport() {
        assert_eq!(menu_popup_max_width(200.0, 420.0), 176.0);
        assert_eq!(menu_popup_max_width(1024.0, 420.0), 420.0);
        assert_eq!(menu_popup_max_width(4000.0, 420.0), 420.0);
    }

    #[test]
    fn top_menu_overflow_starts_before_right_controls_can_collide() {
        assert_eq!(top_menu_mode(800.0, 500.0, 300.0), TopMenuMode::Direct);
        assert_eq!(top_menu_mode(799.0, 500.0, 300.0), TopMenuMode::Overflow);
    }

    #[test]
    fn status_event_keys_survive_newer_entries_and_ring_eviction() {
        let existing = discrete_status_event_key(3, 3, 1);
        let after_append = discrete_status_event_key(4, 4, 1);
        assert_eq!(existing, after_append);

        let retained_after_eviction = discrete_status_event_key(201, 200, 0);
        let retained_after_another_eviction = discrete_status_event_key(202, 200, 0);
        assert_ne!(retained_after_eviction, retained_after_another_eviction);
    }

    #[test]
    fn status_message_summary_keeps_diagnostic_lines_for_expanded_detail() {
        let message = "\nmissing prim /World/Terrain\ncaused by: /twins/apollo15/terrain";
        assert_eq!(
            status_message_summary(message),
            "missing prim /World/Terrain"
        );
        assert!(message.contains("caused by: /twins/apollo15/terrain"));
    }

    #[test]
    fn status_event_rows_reserve_width_only_for_attention_action() {
        assert!(!status_event_has_progress(
            lunco_status_core::status_bus::StatusLevel::Info,
            None
        ));
        assert!(status_event_has_progress(
            lunco_status_core::status_bus::StatusLevel::Progress,
            None
        ));
        assert!(status_event_has_progress(
            lunco_status_core::status_bus::StatusLevel::Info,
            Some((1, 2))
        ));
        assert_eq!(
            status_event_action_width(lunco_status_core::status_bus::StatusLevel::Info, false),
            0.0
        );
        assert_eq!(
            status_event_action_width(lunco_status_core::status_bus::StatusLevel::Warn, false),
            0.0
        );
        assert_eq!(
            status_event_action_width(lunco_status_core::status_bus::StatusLevel::Error, true),
            0.0
        );
        assert_eq!(
            status_event_action_width(lunco_status_core::status_bus::StatusLevel::Attention, false),
            STATUS_EVENT_ATTENTION_WIDTH
        );
    }

    #[test]
    fn latest_status_message_width_is_bounded_by_remaining_space() {
        assert_eq!(status_bar_message_width(500.0, false, 4.0), 500.0);
        assert_eq!(status_bar_message_width(500.0, true, 4.0), 376.0);
        assert_eq!(status_bar_message_width(80.0, true, 4.0), 1.0);
    }

    #[test]
    fn latest_status_notification_tracks_popup_and_yields_to_right_controls() {
        assert_eq!(
            status_bar_notification_width(1600.0, 120.0, 960.0),
            STATUS_BAR_NOTIFICATION_MAX_WIDTH
        );
        assert_eq!(status_bar_notification_width(520.0, 120.0, 960.0), 280.0);
        assert_eq!(status_bar_notification_width(400.0, 120.0, 420.0), 140.0);
        assert_eq!(status_bar_notification_width(120.0, 160.0, 420.0), 1.0);
    }

    #[test]
    fn latest_status_notification_is_one_flowing_string() {
        let job = status_notification_layout_job(
            &egui::Style::default(),
            lunco_status_core::status_bus::StatusLevel::Warn,
            "updates",
            "updates unavailable",
            egui::Color32::YELLOW,
        );

        assert_eq!(job.text, "WARN updates: updates unavailable");
    }

    #[test]
    fn status_bar_right_controls_fit_the_reserved_compact_width() {
        let compact = status_bar_right_widths(960.0, true, true, true);
        assert!(compact.total() <= 800.0);
        assert!(compact.scene <= STATUS_BAR_SCENE_MAX_WIDTH);
        assert!(compact.net <= STATUS_BAR_NET_MAX_WIDTH);
        assert!(compact.perf <= STATUS_BAR_PERF_MAX_WIDTH);

        let wide = status_bar_right_widths(1600.0, true, true, true);
        assert_eq!(wide.scene, STATUS_BAR_SCENE_MAX_WIDTH);
        assert_eq!(wide.net, STATUS_BAR_NET_MAX_WIDTH);
        assert_eq!(wide.perf, STATUS_BAR_PERF_MAX_WIDTH);
    }

    #[test]
    fn status_bar_segment_never_exceeds_remaining_width() {
        assert_eq!(status_bar_segment_width(472.0, 480.0), 472.0);
        assert_eq!(status_bar_segment_width(640.0, 480.0), 480.0);
        assert_eq!(status_bar_segment_width(-1.0, 480.0), 0.0);
    }

    #[test]
    fn status_bar_perf_segment_keeps_a_right_edge_inset() {
        assert_eq!(status_bar_perf_width(472.0, 480.0), 464.0);
        assert_eq!(status_bar_perf_width(640.0, 480.0), 480.0);
        assert_eq!(status_bar_perf_width(4.0, 480.0), 0.0);
    }

    #[test]
    fn perf_hud_keeps_required_metrics_before_optional_detail() {
        let (required, full) = perf_hud_text(22.3, 44.8, Some(0.5), Some(492.4));

        assert!(full.starts_with(&required));
        assert!(required.contains("FPS"));
        assert!(required.contains("44.8ms"));
        assert!(required.contains("phys"));
        assert!(full.find("phys").unwrap() < full.find("p99").unwrap());
    }

    #[test]
    fn perf_hud_drops_optional_detail_before_ellipsizing_required_metrics() {
        let (required, full) = perf_hud_text(22.3, 44.8, Some(0.5), Some(492.4));
        let width = |text: &str| text.chars().count() as f32;

        assert_eq!(
            fit_text_to_width(&required, &full, required.chars().count() as f32, width),
            required
        );
    }

    #[test]
    fn perf_hud_sparkline_yields_space_to_required_metrics() {
        assert_eq!(perf_hud_sparkline_width(360.0, 220.0, 8.0, true), 120.0);
        assert_eq!(perf_hud_sparkline_width(300.0, 220.0, 8.0, true), 72.0);
        assert_eq!(perf_hud_sparkline_width(220.0, 220.0, 8.0, true), 0.0);
        assert_eq!(perf_hud_sparkline_width(360.0, 220.0, 8.0, false), 0.0);
    }

    #[test]
    fn status_bar_right_controls_prioritize_readable_perf_metrics() {
        let compact = status_bar_right_widths(960.0, true, true, true);

        assert!(compact.perf >= STATUS_BAR_PERF_REQUIRED_WIDTH);
        assert!(compact.total() <= 800.0);
    }

    #[test]
    fn focus_panel_is_queued_while_layout_is_scoped_out() {
        let mut app = App::new();
        app.init_resource::<PendingPanelFocus>();
        app.add_observer(on_focus_panel);
        app.world_mut().trigger(FocusPanel {
            id: "source_viewer".into(),
        });
        assert_eq!(
            app.world().resource::<PendingPanelFocus>().0,
            ["source_viewer"]
        );
    }

    struct NavigationInstancePanel;

    impl InstancePanel for NavigationInstancePanel {
        fn kind(&self) -> PanelId {
            PanelId("navigation_instance")
        }

        fn default_slot(&self) -> PanelSlot {
            PanelSlot::Center
        }

        fn title(&self, _world: &World, instance: u64) -> String {
            format!("Instance {instance}")
        }

        fn render(&mut self, _ui: &mut egui::Ui, _ctx: &mut PanelCtx, _instance: u64) {}
    }

    #[test]
    fn deferred_navigation_applies_perspective_before_tab() {
        let mut app = App::new();
        app.init_resource::<PendingLayoutRequests>()
            .init_resource::<PendingTabRequests>();

        let mut layout = WorkbenchLayout::default();
        layout.register(DockPanel(PanelId("center")));
        layout.register_instance_panel(NavigationInstancePanel);
        layout.register_perspective(CenterPerspective {
            id: PerspectiveId("source"),
        });
        layout.register_perspective(CenterPerspective {
            id: PerspectiveId("destination"),
        });

        // Match the render-time state: the workbench has queued both intents
        // while its layout was unavailable, then receives the live layout
        // before the next Update drain.
        app.insert_resource(layout);
        app.world_mut()
            .resource_mut::<PendingLayoutRequests>()
            .0
            .push(LayoutRequest::ActivatePerspective("destination".into()));
        app.world_mut()
            .resource_mut::<PendingTabRequests>()
            .0
            .push(TabRequest::Open(OpenTab {
                kind: PanelId("navigation_instance"),
                instance: 7,
            }));
        app.add_systems(
            Update,
            (drain_pending_layout_requests, drain_pending_tab_requests).chain(),
        );

        app.update();

        let layout = app.world().resource::<WorkbenchLayout>();
        assert_eq!(
            layout.active_perspective(),
            Some(PerspectiveId("destination"))
        );
        assert!(layout.dock.iter_all_tabs().any(|(_, tab)| {
            *tab == TabId::Instance {
                kind: PanelId("navigation_instance"),
                instance: 7,
            }
        }));
    }

    #[test]
    fn open_tab_is_queued_while_layout_is_scoped_out() {
        let mut app = App::new();
        app.init_resource::<PendingTabRequests>();
        app.add_observer(on_open_tab);
        app.world_mut().trigger(OpenTab {
            kind: PanelId("navigation_instance"),
            instance: 7,
        });

        assert_eq!(app.world().resource::<PendingTabRequests>().0.len(), 1);
    }

    struct FocusPanelFixture;

    impl Panel for FocusPanelFixture {
        fn id(&self) -> PanelId {
            PanelId("focus_fixture")
        }

        fn title(&self) -> String {
            "Focus fixture".into()
        }

        fn default_slot(&self) -> PanelSlot {
            PanelSlot::SideBrowser
        }

        fn render(&mut self, _ui: &mut egui::Ui, _ctx: &mut PanelCtx) {}
    }

    #[test]
    fn focus_panel_mounts_a_registered_closed_panel_in_its_authored_slot() {
        let mut layout = WorkbenchLayout::default();
        layout.register(FocusPanelFixture);
        // Simulate the viewport-only perspective: the panel remains registered
        // globally, but its tab is not part of this perspective's dock.
        layout.side_browser.clear();
        layout.rebuild_dock();
        assert!(layout
            .dock
            .find_tab(&TabId::Singleton(PanelId("focus_fixture")))
            .is_none());

        focus_panel_now(&mut layout, "focus_fixture");

        assert!(layout
            .dock
            .find_tab(&TabId::Singleton(PanelId("focus_fixture")))
            .is_some());
        assert_eq!(layout.side_browser, [PanelId("focus_fixture")]);
    }

    struct DockPanel(PanelId);

    impl Panel for DockPanel {
        fn id(&self) -> PanelId {
            self.0
        }

        fn title(&self) -> String {
            self.0 .0.to_string()
        }

        fn default_slot(&self) -> PanelSlot {
            PanelSlot::Center
        }

        fn render(&mut self, _ui: &mut egui::Ui, _ctx: &mut PanelCtx) {}
    }

    #[test]
    fn stacked_side_slots_build_independent_top_and_bottom_leaves() {
        let mut layout = WorkbenchLayout::default();
        for id in ["entities", "telemetry", "viewport", "inspector", "spawn"] {
            layout.register(DockPanel(PanelId(id)));
        }

        let mut plan = PerspectiveLayoutPlan::new();
        plan.side_browser =
            PerspectiveSlotPlan::new().stacked([PanelId("entities")], [PanelId("telemetry")]);
        plan.center = PerspectiveSlotPlan::new().single(Some(PanelId("viewport")));
        plan.right_inspector =
            PerspectiveSlotPlan::new().stacked([PanelId("inspector")], [PanelId("spawn")]);
        layout.apply_perspective_plan(plan);

        let leaves: Vec<Vec<TabId>> = layout
            .dock
            .main_surface()
            .iter()
            .filter_map(|node| match node {
                egui_dock::Node::Leaf(leaf) => Some(leaf.tabs.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(leaves.len(), 5, "center plus two stacked side regions");
        for id in ["entities", "telemetry", "viewport", "inspector", "spawn"] {
            assert!(
                leaves
                    .iter()
                    .any(|tabs| tabs.contains(&TabId::Singleton(PanelId(id)))),
                "missing dock panel {id}"
            );
        }
    }

    #[test]
    fn registering_a_predeclared_stacked_panel_keeps_its_declared_slot() {
        let mut layout = WorkbenchLayout::default();
        let mut plan = PerspectiveLayoutPlan::new();
        plan.side_browser =
            PerspectiveSlotPlan::new().stacked([PanelId("entities")], [PanelId("telemetry")]);
        plan.center = PerspectiveSlotPlan::new().single(Some(PanelId("viewport")));
        plan.right_inspector =
            PerspectiveSlotPlan::new().stacked([PanelId("inspector")], [PanelId("spawn")]);
        layout.apply_perspective_plan(plan);

        layout.register(DockPanel(PanelId("entities")));
        layout.register(DockPanel(PanelId("telemetry")));
        layout.register(DockPanel(PanelId("viewport")));
        layout.register(DockPanel(PanelId("inspector")));
        layout.register(DockPanel(PanelId("spawn")));

        assert_eq!(layout.side_browser, [PanelId("entities")]);
        assert_eq!(layout.side_browser_bottom, [PanelId("telemetry")]);
        assert_eq!(layout.right_inspector, [PanelId("inspector")]);
        assert_eq!(layout.right_inspector_bottom, [PanelId("spawn")]);
        assert!(layout.bottom.is_empty());
    }

    #[test]
    fn late_panel_registration_does_not_mutate_active_perspective() {
        let mut layout = WorkbenchLayout::default();
        layout.register_perspective(TestPerspective {
            id: PerspectiveId("view"),
            title: "View",
            marker: PanelId("side_panel"),
        });
        assert!(layout.center.is_empty());

        layout.register(DockPanel(PanelId("late_center")));

        assert_eq!(layout.active_perspective(), Some(PerspectiveId("view")));
        assert!(layout.center.is_empty());
        assert!(!layout
            .dock
            .iter_all_tabs()
            .any(|(_, tab)| *tab == TabId::Singleton(PanelId("late_center"))));
        assert!(layout.panels.contains_key(&PanelId("late_center")));
    }

    struct TestPerspective {
        id: PerspectiveId,
        title: &'static str,
        marker: PanelId,
    }

    impl Perspective for TestPerspective {
        fn id(&self) -> PerspectiveId {
            self.id
        }
        fn title(&self) -> String {
            self.title.to_string()
        }
        fn show_in_switcher(&self) -> bool {
            self.id != PerspectiveId("hidden")
        }
        fn layout(&self) -> PerspectiveLayoutPlan {
            let mut plan = PerspectiveLayoutPlan::new();
            plan.side_browser = PerspectiveSlotPlan::new().single(Some(self.marker));
            plan
        }
    }

    struct CenterPerspective {
        id: PerspectiveId,
    }

    impl Perspective for CenterPerspective {
        fn id(&self) -> PerspectiveId {
            self.id
        }

        fn title(&self) -> String {
            self.id.0.to_string()
        }

        fn layout(&self) -> PerspectiveLayoutPlan {
            let mut plan = PerspectiveLayoutPlan::new();
            plan.center = PerspectiveSlotPlan::new().tabs([PanelId("center")]);
            plan
        }
    }

    #[test]
    fn first_registered_perspective_auto_activates() {
        let mut layout = WorkbenchLayout::default();
        assert!(layout.active_perspective().is_none());

        layout.register_perspective(TestPerspective {
            id: PerspectiveId("a"),
            title: "A",
            marker: PanelId("panel_a"),
        });

        assert_eq!(layout.active_perspective(), Some(PerspectiveId("a")));
        assert_eq!(layout.side_browser, vec![PanelId("panel_a")]);
    }

    #[test]
    fn second_perspective_does_not_override_active() {
        let mut layout = WorkbenchLayout::default();
        layout.register_perspective(TestPerspective {
            id: PerspectiveId("a"),
            title: "A",
            marker: PanelId("panel_a"),
        });
        layout.register_perspective(TestPerspective {
            id: PerspectiveId("b"),
            title: "B",
            marker: PanelId("panel_b"),
        });

        assert_eq!(layout.active_perspective(), Some(PerspectiveId("a")));
        assert_eq!(layout.side_browser, vec![PanelId("panel_a")]);
    }

    #[test]
    fn hidden_perspective_stays_registered_but_is_not_a_switcher_tab() {
        let mut layout = WorkbenchLayout::default();
        for (id, marker) in [
            ("a", "panel_a"),
            ("hidden", "panel_hidden"),
            ("b", "panel_b"),
        ] {
            layout.register_perspective(TestPerspective {
                id: PerspectiveId(id),
                title: id,
                marker: PanelId(marker),
            });
        }

        let ids = perspective_switcher_tabs(&layout)
            .into_iter()
            .map(|(id, _, _)| id)
            .collect::<Vec<_>>();
        assert_eq!(ids, vec![PerspectiveId("b"), PerspectiveId("a")]);

        // Hiding navigation chrome must not remove the activation/API path.
        layout.activate_perspective(PerspectiveId("hidden"));
        assert_eq!(layout.active_perspective(), Some(PerspectiveId("hidden")));
    }

    #[test]
    fn perspective_help_anchor_uses_the_registered_id() {
        assert_eq!(
            perspective_help_anchor(PerspectiveId("sandbox_view")),
            "menu.perspective.sandbox_view"
        );
    }

    #[test]
    fn help_uses_visible_perspective_title_as_its_canonical_label() {
        let mut layout = WorkbenchLayout::default();
        layout.register_perspective(TestPerspective {
            id: PerspectiveId("visible"),
            title: "Build",
            marker: PanelId("visible_panel"),
        });
        layout.register_perspective(TestPerspective {
            id: PerspectiveId("hidden"),
            title: "Hidden",
            marker: PanelId("hidden_panel"),
        });

        assert_eq!(
            lunco_workbench_help_ui::visible_perspective_title(&layout, PerspectiveId("visible"),),
            Some("Build".to_owned())
        );
        assert_eq!(
            lunco_workbench_help_ui::visible_perspective_title(&layout, PerspectiveId("hidden"),),
            None
        );
    }

    #[test]
    fn activate_perspective_applies_preset() {
        let mut layout = WorkbenchLayout::default();
        layout.register_perspective(TestPerspective {
            id: PerspectiveId("a"),
            title: "A",
            marker: PanelId("panel_a"),
        });
        layout.register_perspective(TestPerspective {
            id: PerspectiveId("b"),
            title: "B",
            marker: PanelId("panel_b"),
        });

        layout.activate_perspective(PerspectiveId("b"));
        assert_eq!(layout.active_perspective(), Some(PerspectiveId("b")));
        assert_eq!(layout.side_browser, vec![PanelId("panel_b")]);
    }

    #[test]
    fn activate_unknown_perspective_is_noop() {
        let mut layout = WorkbenchLayout::default();
        layout.register_perspective(TestPerspective {
            id: PerspectiveId("a"),
            title: "A",
            marker: PanelId("panel_a"),
        });

        layout.activate_perspective(PerspectiveId("ghost"));
        assert_eq!(layout.active_perspective(), Some(PerspectiveId("a")));
        assert_eq!(layout.side_browser, vec![PanelId("panel_a")]);
    }

    #[test]
    fn perspectives_keep_separate_docks() {
        let mut layout = WorkbenchLayout::default();
        layout.register_perspective(TestPerspective {
            id: PerspectiveId("a"),
            title: "A",
            marker: PanelId("panel_a"),
        });
        layout.register_perspective(TestPerspective {
            id: PerspectiveId("b"),
            title: "B",
            marker: PanelId("panel_b"),
        });
        // A is active (first-registered). Simulate a tab open only in A.
        let only_a = TabId::Singleton(PanelId("only_in_a"));
        layout.dock = DockState::new(vec![only_a]);

        // A → B: B must NOT inherit A's tab.
        layout.activate_perspective(PerspectiveId("b"));
        assert_eq!(layout.active_perspective(), Some(PerspectiveId("b")));
        assert!(
            !layout.dock.iter_all_tabs().any(|(_, t)| *t == only_a),
            "B inherited A's tab — each perspective must keep its own dock"
        );

        // B → A: A's tab must come back from the per-perspective cache.
        layout.activate_perspective(PerspectiveId("a"));
        assert_eq!(layout.active_perspective(), Some(PerspectiveId("a")));
        assert!(
            layout.dock.iter_all_tabs().any(|(_, t)| *t == only_a),
            "A's tab was not restored on return"
        );
    }

    #[test]
    fn reset_to_default_layout_drops_cached_tabs() {
        let mut layout = WorkbenchLayout::default();
        layout.register_perspective(TestPerspective {
            id: PerspectiveId("a"),
            title: "A",
            marker: PanelId("panel_a"),
        });
        let only_a = TabId::Singleton(PanelId("only_in_a"));
        layout.dock = DockState::new(vec![only_a]);
        // Round-trip through another perspective so A's tab is cached, then
        // a reset of A must produce a clean preset, not the cached tab.
        layout.register_perspective(TestPerspective {
            id: PerspectiveId("b"),
            title: "B",
            marker: PanelId("panel_b"),
        });
        layout.activate_perspective(PerspectiveId("b"));
        layout.activate_perspective(PerspectiveId("a"));
        assert!(layout.dock.iter_all_tabs().any(|(_, t)| *t == only_a));

        layout.reset_to_default_layout();
        assert!(
            !layout.dock.iter_all_tabs().any(|(_, t)| *t == only_a),
            "reset restored the cached tab instead of a clean preset"
        );
        assert_eq!(layout.side_browser, vec![PanelId("panel_a")]);
    }

    #[test]
    fn reset_to_default_perspective_discards_current_view_and_all_caches() {
        let mut layout = WorkbenchLayout::default();
        layout.register_perspective(TestPerspective {
            id: PerspectiveId("a"),
            title: "A",
            marker: PanelId("panel_a"),
        });
        layout.register_perspective(TestPerspective {
            id: PerspectiveId("b"),
            title: "B",
            marker: PanelId("panel_b"),
        });

        layout.activate_perspective(PerspectiveId("b"));
        layout.dock = DockState::new(vec![TabId::Singleton(PanelId("stale"))]);
        layout.activate_perspective(PerspectiveId("a"));
        assert_eq!(layout.active_perspective(), Some(PerspectiveId("a")));
        assert!(layout.dock_cache.contains_key(&PerspectiveId("b")));

        layout.reset_to_default_perspective();

        assert_eq!(layout.active_perspective(), Some(PerspectiveId("a")));
        assert!(layout.dock_cache.is_empty());
        assert!(!layout
            .dock
            .iter_all_tabs()
            .any(|(_, tab)| *tab == TabId::Singleton(PanelId("stale"))));
        assert_eq!(layout.side_browser, vec![PanelId("panel_a")]);
    }

    #[test]
    fn required_perspective_keeps_guided_presentations_in_their_authored_layout() {
        let mut layout = WorkbenchLayout::default();
        layout.register_perspective(TestPerspective {
            id: PerspectiveId("view"),
            title: "View",
            marker: PanelId("view_panel"),
        });
        layout.register_perspective(TestPerspective {
            id: PerspectiveId("build"),
            title: "Build",
            marker: PanelId("build_panel"),
        });

        layout.set_required_perspective(Some("build"));
        layout.activate_perspective(PerspectiveId("view"));

        assert_eq!(layout.active_perspective(), Some(PerspectiveId("build")));
        assert_eq!(layout.side_browser, [PanelId("build_panel")]);

        layout.set_required_perspective(None);
        layout.activate_perspective(PerspectiveId("view"));
        assert_eq!(layout.active_perspective(), Some(PerspectiveId("view")));
    }

    /// A NaN split fraction (egui_dock poisons the tree from inside `show`)
    /// serializes to JSON `null`, which used to fail `from_value` outright —
    /// so the user's whole dock silently reset on every launch. Build a real
    /// split, poison it, and round-trip through serde rather than asserting on
    /// a hand-written JSON literal, so this stays honest if egui_dock changes
    /// its wire format.
    #[test]
    fn nan_split_fraction_survives_a_dock_json_round_trip() {
        let mut dock: DockState<TabId> = DockState::new(vec![TabId::Singleton(PanelId("a"))]);
        dock.main_surface_mut().split_right(
            egui_dock::NodeIndex::root(),
            0.5,
            vec![TabId::Singleton(PanelId("b"))],
        );

        // Reproduce the upstream `0.0 / 0.0` poisoning.
        for (_surface, node) in dock.iter_all_nodes_mut() {
            if let egui_dock::Node::Horizontal(s) | egui_dock::Node::Vertical(s) = node {
                s.fraction = f32::NAN;
            }
        }

        let mut value = serde_json::to_value(&dock).expect("dock serializes");
        assert!(
            serde_json::from_value::<DockState<TabId>>(value.clone()).is_err(),
            "expected non-finite floats to serialize as null and fail to parse \
             — if this now succeeds, serde_json changed and the heal is dead code"
        );

        heal_non_finite_nulls(&mut value);
        let restored: DockState<TabId> =
            serde_json::from_value(value).expect("healed dock must parse");

        let fractions: Vec<f32> = restored
            .iter_all_nodes()
            .filter_map(|(_surface, node)| match node {
                egui_dock::Node::Horizontal(s) | egui_dock::Node::Vertical(s) => Some(s.fraction),
                _ => None,
            })
            .collect();
        assert!(
            !fractions.is_empty(),
            "split node did not survive the round trip"
        );
        assert!(
            fractions.iter().all(|f| (*f - 0.5).abs() < f32::EPSILON),
            "healed fractions should default to 0.5, got {fractions:?}"
        );
    }

    #[test]
    fn perspective_plan_materializes_center_tabs_in_order() {
        let mut layout = WorkbenchLayout::default();
        let mut plan = PerspectiveLayoutPlan::new();
        plan.center = PerspectiveSlotPlan::new().tabs([PanelId("a"), PanelId("b")]);
        layout.apply_perspective_plan(plan);
        assert_eq!(layout.center, vec![PanelId("a"), PanelId("b")]);
    }

    #[test]
    fn perspective_plan_selects_the_requested_center_tab() {
        let mut layout = WorkbenchLayout::default();
        let mut plan = PerspectiveLayoutPlan::new();
        plan.center = PerspectiveSlotPlan::new().tabs([PanelId("code"), PanelId("diagram")]);
        plan.active_center_tab = Some(1);
        layout.apply_perspective_plan(plan);
        assert_eq!(layout.active_center_tab, 1);
    }

    #[test]
    fn perspective_plan_clamps_an_out_of_range_center_tab() {
        let mut layout = WorkbenchLayout::default();
        let mut plan = PerspectiveLayoutPlan::new();
        plan.center = PerspectiveSlotPlan::new().tabs([PanelId("x")]);
        plan.active_center_tab = Some(2);
        layout.apply_perspective_plan(plan);
        assert_eq!(layout.active_center_tab, 0);
    }
}
