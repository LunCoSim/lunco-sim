//! Reusable egui widgets shared by LunCoSim domain UI crates.
//!
//! These controls deliberately do not depend on the concrete workbench shell.
//! They can therefore be used by lightweight panels, headless composition
//! code, and alternate hosts without pulling in docking, viewport, or window
//! management.
//!
//! Application text fields use [`text_editor`] builders so domain panels can
//! add their own labels, validation, and commit behavior without creating a
//! second widget setup. Panels own any containing `ScrollArea` needed to bound
//! multiline editors or long result lists.

pub mod icons;
pub mod text_editor;
pub mod tree;

pub use icons::{UiIcon, icon_button, icon_button_sized, icon_text_button, paint_icon};

/// Edit an optional photographic exposure, starting from the inherited EV100.
pub fn fixed_exposure_ev100(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut Option<f64>,
    inherited: f64,
) -> bool {
    let mut fixed = value.is_some();
    let toggled = ui.checkbox(&mut fixed, label).changed();
    if toggled {
        *value = fixed.then_some(inherited);
    }
    let edited = value.as_mut().is_some_and(|ev100| {
        ui.add(egui::DragValue::new(ev100).speed(0.1).suffix(" EV100"))
            .on_hover_text("Lower EV100 brightens the image. Higher EV100 darkens it.")
            .changed()
    });
    toggled || edited
}
