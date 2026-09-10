//! Which floating overlays the viewport shows — a persisted user preference.
//!
//! Three things used to draw over the 3D view unconditionally in the View
//! perspective: the sky clock (top-left), the view-mode switcher (top-centre) and
//! the rover HUD. The first two are *chrome you configure once*, not information
//! you read every frame, and neither had an off switch anywhere — their visibility
//! was a pair of system `run_if`s and nothing else, so "hide it" meant editing
//! Rust.
//!
//! Both are now OFF by default and live behind [`OverlaySettings`]. The sky
//! clock is configured from the workbench **Time** menu, while the view
//! switcher is configured from **Camera** beside its body controls.
//!
//! The rover HUD is deliberately NOT in here: it only draws while you are
//! possessing a vessel, so it is already answering a question you just asked.

use bevy::prelude::*;
use lunco_settings::{AppSettingsExt, SettingsSection};
use lunco_twin::TwinSettingValue;
use lunco_workbench::{input_overlay::InputOverlaySettings, perf_hud::PerfHudSettings, MenuCtx};
use lunco_workspace::{ResetTwinSetting, SetTwinSetting, TwinSettingInput, WorkspaceResource};
use serde::{Deserialize, Serialize};

/// Persisted visibility of the optional viewport overlays. Stored under the
/// `"overlays"` key of `settings.json`.
///
/// `Default` is all-off: a fresh install shows the terrain and nothing on top of
/// it. Both fields are opt-IN, the same rule the celestial subsystem and the
/// trajectory lines already follow — content and chrome appear because something
/// asked for them, never because a default said yes.
#[derive(Resource, Serialize, Deserialize, Default, Clone, Copy, PartialEq, Debug)]
pub(crate) struct OverlaySettings {
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

/// Contribute the sky-clock controls and visibility preference to the workbench
/// Time menu.
///
/// Registered at `Startup`; a no-op when the workbench layout is absent (headless
/// runs, `luncosim test`), which is why it takes `&mut World` and bails rather than
/// requiring the resource.
pub(crate) fn register_time_menu(world: &mut World) {
    let Some(mut layout) = world.get_resource_mut::<lunco_workbench::WorkbenchLayout>() else {
        return;
    };
    layout.register_time_menu(|ui, ctx| {
        super::celestial_time::sky_clock_menu_ui(ui, ctx);

        ui.separator();
        ui.label(
            bevy_egui::egui::RichText::new("Viewport overlays")
                .weak()
                .small(),
        );
        // Edit a copy and write back only on a real change: `set_resource`
        // applies the replacement after the menu pass, so opening the menu does
        // not mark the resource changed and rewrite settings.json.
        let Some(mut edited) = ctx.resource::<OverlaySettings>().copied() else {
            return;
        };
        let original = edited;
        ui.checkbox(&mut edited.sky_clock, "Time HUD (top-left)")
            .on_hover_text(
                "Show the floating celestial time HUD. The same sky-clock controls \
                 remain available in this menu when the HUD is hidden.",
            );
        if edited != original {
            ctx.set_resource(edited);
        }
    });
}

/// Contribute the view-switcher preference to the workbench Camera menu.
pub(crate) fn camera_menu_ui(ui: &mut bevy_egui::egui::Ui, ctx: &mut lunco_workbench::MenuCtx) {
    // Edit a COPY and write back only on a real change: `set_resource` applies
    // the replacement after the menu pass, so opening the menu does not mark
    // settings changed and rewrite settings.json.
    let Some(mut edited) = ctx.resource::<OverlaySettings>().copied() else {
        return;
    };
    let original = edited;
    ui.separator();
    ui.label(
        bevy_egui::egui::RichText::new("Viewport overlays")
            .weak()
            .small(),
    );
    ui.checkbox(&mut edited.view_switcher, "View switcher (top-centre)")
        .on_hover_text(
            "Surface / Moon / Earth pill. The highlighted chip is the body the \
             camera is currently focused on.",
        );
    if edited != original {
        ctx.set_resource(edited);
    }
}

const CAMERA_STATUS_SETTING: &str = "ui.camera_status";
const CAMERA_STATUS_DEFAULT: bool = true;

/// Read a boolean project setting through the active Twin that owns it.
///
/// The Settings menu is only a view over the generic Twin setting map. It does
/// not cache a second value, and it does not manufacture a Twin when the
/// workspace is between Twin lifecycles.
fn active_twin_bool_setting(
    ctx: &MenuCtx<'_>,
    key: &str,
    default: bool,
) -> Result<(bool, bool), &'static str> {
    let workspace = ctx
        .resource::<WorkspaceResource>()
        .ok_or("no workspace is active")?;
    let twin_id = workspace.active_twin.ok_or("no Twin is active")?;
    let twin = workspace
        .twin(twin_id)
        .ok_or("the active Twin is no longer available")?;
    let manifest = twin
        .manifest
        .as_ref()
        .ok_or("the active Twin has no manifest")?;
    match manifest.setting(key) {
        Some(TwinSettingValue::Bool(value)) => Ok((*value, true)),
        Some(_) => Err("the Twin setting is not boolean"),
        None => Ok((default, false)),
    }
}

/// Render one Twin-owned boolean setting without taking ownership of its
/// persistence or runtime gate. `Reset` removes the explicit override so the
/// authored default becomes visible again.
fn render_twin_bool_setting(
    ui: &mut bevy_egui::egui::Ui,
    ctx: &mut MenuCtx<'_>,
    key: &'static str,
    label: &str,
    default: bool,
    help: &str,
) {
    let (mut value, explicit) = match active_twin_bool_setting(ctx, key, default) {
        Ok(value) => value,
        Err(reason) => {
            let mut disabled = false;
            ui.add_enabled_ui(false, |ui| {
                ui.checkbox(&mut disabled, label);
            });
            ui.label(
                bevy_egui::egui::RichText::new(format!("Unavailable: {reason}"))
                    .weak()
                    .small(),
            );
            return;
        }
    };

    let original = value;
    let mut reset = false;
    ui.horizontal(|ui| {
        ui.checkbox(&mut value, label).on_hover_text(help);
        if explicit && ui.small_button("Use default").clicked() {
            reset = true;
        }
    });
    if reset {
        ctx.trigger(ResetTwinSetting {
            key: key.to_string(),
        });
    } else if value != original {
        ctx.trigger(SetTwinSetting {
            key: key.to_string(),
            value: TwinSettingInput::Bool(value),
        });
    }
    ui.label(
        bevy_egui::egui::RichText::new(if explicit {
            "Twin override"
        } else if default {
            "Authored default: on"
        } else {
            "Authored default: off"
        })
        .weak()
        .small(),
    );
}

/// Register one discoverable HUD submenu over the existing visibility owners.
///
/// The rows intentionally write the same resources and generic Twin setting
/// used by the Time/Camera menus, typed commands, and runtime surface gate.
/// Automatic surfaces are shown as read-only inventory: possession, authored
/// USD metadata, or transient lifecycle state remains their visibility owner.
fn register_hud_settings_menu(world: &mut World) {
    let Some(mut layout) = world.get_resource_mut::<lunco_workbench::WorkbenchLayout>() else {
        return;
    };
    layout.register_settings_submenu("HUD", |ui, ctx| {
        use bevy_egui::egui;

        ui.label(egui::RichText::new("User-controlled HUDs").weak().small());

        let Some(mut overlays) = ctx.resource::<OverlaySettings>().copied() else {
            return;
        };
        let original_overlays = overlays;
        ui.checkbox(&mut overlays.sky_clock, "Time HUD (top-left)")
            .on_hover_text("Show the celestial time HUD and its current epoch.");
        ui.checkbox(
            &mut overlays.view_switcher,
            "Surface view switcher (top-centre)",
        )
        .on_hover_text("Show the Surface / Moon / Earth view controls.");
        if overlays != original_overlays {
            ctx.set_resource(overlays);
        }

        let Some(mut performance) = ctx.resource::<PerfHudSettings>().copied() else {
            return;
        };
        let original_performance = performance;
        ui.checkbox(&mut performance.enabled, "Performance HUD (status bar)")
            .on_hover_text("Show live FPS, frame time, and optional physics timing.");
        if performance != original_performance {
            ctx.set_resource(performance);
        }

        let Some(mut input) = ctx.resource::<InputOverlaySettings>().copied() else {
            return;
        };
        let original_input = input;
        ui.checkbox(&mut input.enabled, "Input HUD (bottom-centre)")
            .on_hover_text("Show the live keyboard and mouse input visualiser.");
        if input != original_input {
            ctx.set_resource(input);
        }

        render_twin_bool_setting(
            ui,
            ctx,
            CAMERA_STATUS_SETTING,
            "Camera/status HUD (top-right)",
            CAMERA_STATUS_DEFAULT,
            "Show the active camera/status surface in the View perspective.",
        );

        ui.separator();
        ui.label(egui::RichText::new("Automatic HUDs").weak().small());
        for (label, owner) in [
            ("Rover HUD", "Driven-vessel possession and capability state"),
            (
                "Lander control cards",
                "Authored USD control-HUD metadata on the active scene",
            ),
            (
                "Terrain and scenario-download progress",
                "Transient terrain/network lifecycle state",
            ),
            (
                "Tutorial, notification, and blackout HUDs",
                "Lesson and runtime session lifecycle state",
            ),
        ] {
            ui.horizontal(|ui| {
                ui.label(label);
                ui.label(egui::RichText::new("Automatic").weak().small());
            });
            ui.label(egui::RichText::new(owner).weak().small());
        }
    });
}

/// Registers [`OverlaySettings`] (persisted) and its menu rows.
pub(crate) fn plugin(app: &mut App) {
    app.register_settings_section::<OverlaySettings>();
    app.add_systems(Startup, (register_time_menu, register_hud_settings_menu));
}
