//! Focused UI application for testing Modelica battery models.

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts, EguiPlugin, EguiPrimaryContextPass};
use lunco_modelica::{
    LunCoModelicaPlugin, 
    ui::ModelicaInspectorPlugin, 
    ModelicaModel, 
    ModelicaInput, 
    ModelicaOutput
};

fn main() {
    App::new()
        .add_plugins(DefaultPlugins)
        .add_plugins(EguiPlugin::default())
        .add_plugins(LunCoModelicaPlugin)
        .add_plugins(ModelicaInspectorPlugin)
        .add_systems(Startup, setup_test_bench)
        .add_systems(EguiPrimaryContextPass, battery_control_ui)
        .run();
}

#[derive(Component)]
struct BatteryTestBench;

fn setup_test_bench(mut commands: Commands) {
    commands.spawn(Camera2d);

    // Spawn the battery test entity
    commands.spawn((
        BatteryTestBench,
        Name::new("Battery_Test_Bench"),
        ModelicaModel {
            model_path: "assets/models/Battery.mo".to_string(),
            model_name: "Battery".to_string(),
            ..default()
        },
        ModelicaInput {
            variable_name: "current".to_string(),
            value: 0.0,
        },
        ModelicaOutput {
            variable_name: "soc_out".to_string(),
            ..default()
        }
    ));
}

fn battery_control_ui(
    mut contexts: EguiContexts,
    mut q_battery: Query<(&mut ModelicaInput, &ModelicaOutput, &ModelicaModel), With<BatteryTestBench>>,
) {
    let Ok(ctx) = contexts.ctx_mut() else { return; };

    egui::Window::new("🔋 Battery Test Bench").show(ctx, |ui| {
        if let Some((mut input, output, model)) = q_battery.iter_mut().next() {
            ui.heading("Status");
            ui.add(egui::ProgressBar::new(output.value as f32)
                .text(format!("State of Charge: {:.2}%", output.value * 100.0)));
            
            ui.label(format!("Simulation Time: {:.2}s", model.current_time));
            
            ui.separator();
            
            ui.heading("Controls");
            ui.add(egui::Slider::new(&mut input.value, -100.0..=100.0)
                .text("Current (A)")
                .suffix(" A"));
            
            ui.label("Note: Positive current discharges, negative charges.");

            if ui.button("Reset SOC").clicked() {
                ui.label("Reset not yet implemented in worker.");
            }

            ui.separator();
            
            ui.collapsing("Technical Details", |ui| {
                ui.label(format!("Model: {}", model.model_name));
                ui.label(format!("Input [current]: {:.4}", input.value));
                ui.label(format!("Output [soc_out]: {:.4}", output.value));
            });
        } else {
            ui.label("Waiting for battery entity...");
        }
    });
}
