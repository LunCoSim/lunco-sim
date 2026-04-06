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
    let bench = commands.spawn((
        BatteryTestBench,
        Name::new("Battery_Test_Bench"),
        ModelicaModel {
            model_path: "assets/models/Battery.mo".to_string(),
            model_name: "Battery".to_string(),
            parameters: {
                let mut p = std::collections::HashMap::new();
                p.insert("capacity".to_string(), 1.0);
                p.insert("voltage_nom".to_string(), 12.0);
                p.insert("R_internal".to_string(), 0.01);
                p.insert("T_filter".to_string(), 0.1);
                p
            },
            ..default()
        },
        ModelicaInput {
            variable_name: "current_in".to_string(),
            value: 0.0,
        },
    )).id();

    // Spawn outputs as children since Bevy doesn't allow multiple components of same type on one entity
    commands.entity(bench).with_children(|parent| {
        parent.spawn((
            ModelicaOutput {
                variable_name: "soc_out".to_string(),
                ..default()
            },
            Name::new("SOC_Output"),
        ));
        parent.spawn((
            ModelicaOutput {
                variable_name: "voltage_out".to_string(),
                ..default()
            },
            Name::new("Voltage_Output"),
        ));
    });
}

fn battery_control_ui(
    mut contexts: EguiContexts,
    mut q_battery: Query<(&mut ModelicaInput, &ModelicaModel, &Children), With<BatteryTestBench>>,
    q_outputs: Query<&ModelicaOutput>,
) {
    let Ok(ctx) = contexts.ctx_mut() else { return; };

    egui::Window::new("🔋 Battery Test Bench").show(ctx, |ui| {
        if let Some((mut input, model, children)) = q_battery.iter_mut().next() {
            ui.heading("Status");
            
            // Find specific outputs from children
            let mut soc = 0.0;
            let mut voltage = 0.0;
            for child in children.iter() {
                if let Ok(output) = q_outputs.get(child) {
                    if output.variable_name == "soc_out" { soc = output.value; }
                    if output.variable_name == "voltage_out" { voltage = output.value; }
                }
            }

            ui.add(egui::ProgressBar::new(soc as f32)
                .text(format!("State of Charge: {:.2}%", soc * 100.0)));
            
            ui.label(format!("Terminal Voltage: {:.2}V", voltage));
            ui.label(format!("Simulation Time: {:.2}s", model.current_time));
            
            ui.separator();
            
            ui.heading("Controls");
            ui.add(egui::Slider::new(&mut input.value, -100.0..=100.0)
                .text("Current (A)")
                .suffix(" A"));
            
            ui.label("Note: Positive current discharges, negative charges.");

            ui.separator();
            
            ui.collapsing("Technical Details", |ui| {
                ui.label(format!("Model: {}", model.model_name));
                ui.label(format!("Input [current]: {:.4}", input.value));
                ui.label(format!("Output [soc_out]: {:.4}", soc));
                ui.label(format!("Output [voltage_out]: {:.4}", voltage));
            });
        } else {
            ui.label("Waiting for battery entity...");
        }
    });
}
