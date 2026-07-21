//! Which floating overlays the viewport shows — a persisted user preference.
//!
//! Three things used to draw over the 3D view unconditionally in the View
//! perspective: the sky clock (top-left), the view-mode switcher (top-centre) and
//! the rover HUD. The first two are *chrome you configure once*, not information
//! you read every frame, and neither had an off switch anywhere — their visibility
//! was a pair of system `run_if`s and nothing else, so "hide it" meant editing
//! Rust.
//!
//! Both are now OFF by default and live behind [`OverlaySettings`], toggled from
//! the workbench **Time** menu. That menu is also where the sky clock's own
//! controls now live, so turning the overlay off does not take the capability with
//! it — you can retarget and rescale the celestial clock from the menu alone.
//!
//! The rover HUD is deliberately NOT in here: it only draws while you are
//! possessing a vessel, so it is already answering a question you just asked.

use bevy::prelude::*;
use lunco_settings::{AppSettingsExt, SettingsSection};
use serde::{Deserialize, Serialize};

/// Persisted visibility of the optional viewport overlays. Stored under the
/// `"overlays"` key of `settings.json`.
///
/// `Default` is all-off: a fresh install shows the terrain and nothing on top of
/// it. Both fields are opt-IN, the same rule the celestial subsystem and the
/// trajectory lines already follow — content and chrome appear because something
/// asked for them, never because a default said yes.
#[derive(Resource, Serialize, Deserialize, Default, Clone, Copy, PartialEq, Debug)]
pub struct OverlaySettings {
    /// The sky-clock pill (top-left): celestial epoch, follow/independent, rate.
    pub sky_clock: bool,
    /// The view-mode switcher pill (top-centre): Surface / Moon / Earth, which
    /// doubles as the readout of which body the camera is focused on.
    pub view_switcher: bool,
}

impl SettingsSection for OverlaySettings {
    const KEY: &'static str = "overlays";
}

/// `run_if` for the sky-clock overlay.
pub(crate) fn sky_clock_visible(settings: Option<Res<OverlaySettings>>) -> bool {
    settings.is_some_and(|s| s.sky_clock)
}

/// `run_if` for the view-mode switcher overlay.
pub(crate) fn view_switcher_visible(settings: Option<Res<OverlaySettings>>) -> bool {
    settings.is_some_and(|s| s.view_switcher)
}

/// Contribute the overlay checkboxes to the workbench Time menu, alongside the
/// sky-clock controls themselves (`celestial_time::sky_clock_menu_ui`).
///
/// Registered at `Startup`; a no-op when the workbench layout is absent (headless
/// runs, `scene_test`), which is why it takes `&mut World` and bails rather than
/// requiring the resource.
pub(crate) fn register_time_menu(world: &mut World) {
    use bevy_egui::egui;
    let Some(mut layout) = world.get_resource_mut::<lunco_workbench::WorkbenchLayout>() else {
        return;
    };
    layout.register_time_menu(|ui, world| {
        super::celestial_time::sky_clock_menu_ui(ui, world);

        ui.separator();
        ui.label(egui::RichText::new("Viewport overlays").weak().small());
        // Edit a COPY and write back only on a real change: `resource_mut` marks
        // the resource changed on any deref, and the settings writer persists on
        // change — so drawing the menu would rewrite settings.json every frame the
        // menu is open.
        let mut edited = *world.resource::<OverlaySettings>();
        ui.checkbox(&mut edited.sky_clock, "Sky clock (top-left)")
            .on_hover_text(
                "Floating pill showing the celestial epoch and its clock coupling. \
                 The same controls are in this menu — the overlay is only for \
                 keeping them on screen.",
            );
        ui.checkbox(&mut edited.view_switcher, "View switcher (top-centre)")
            .on_hover_text(
                "Surface / Moon / Earth pill. The highlighted chip is the body the \
                 camera is currently focused on.",
            );
        if edited != *world.resource::<OverlaySettings>() {
            *world.resource_mut::<OverlaySettings>() = edited;
        }
    });
}

/// Registers [`OverlaySettings`] (persisted) and its Time-menu rows.
pub(crate) fn plugin(app: &mut App) {
    app.register_settings_section::<OverlaySettings>();
    app.add_systems(Startup, register_time_menu);
}
