// ── Rhai Flight Software (FSW) Sensor Logic Scenario ────────────────────────
//
// Application-level logic reads ONLY authored SENSOR outputs (IMU, ThermalSensor,
// ElectricalSensor, StarTracker, Altimeter), NEVER ground truth directly.
//
// Enforces FSW control actions:
// - Under-voltage load shedding on ElectricalSensor low bus voltage.
// - Survival heater activation on ThermalSensor temperature drop.
// - Navigation mode switch (StarTracker lock -> IMU dead reckoning).

fn evaluate_fsw_sensor_logic(telemetry) {
    let actions = #{
        load_shedding_active: false,
        survival_heater_active: false,
        nav_mode: "STAR_TRACKER_ATTITUDE",
        descent_engine_enable: true
    };

    // 1. Electrical Sensor Evaluation (EPS bus voltage telemetry)
    if telemetry.v_bus_sensor < 22.0 || telemetry.adc_v_counts < 1800 {
        actions.load_shedding_active = true;
    }

    // 2. Thermal Sensor Evaluation (RTD temperature probe telemetry)
    if telemetry.temp_sensor_k < 263.15 || telemetry.temp_sensor_c < -10.0 {
        actions.survival_heater_active = true;
    }

    // 3. Star Tracker & IMU Sensor Evaluation (Optical lock & rate telemetry)
    if telemetry.tracking_lock < 0.5 {
        actions.nav_mode = "IMU_DEAD_RECKONING";
    }

    // 4. Touchdown Contact Switch Sensor Evaluation
    if telemetry.touchdown_contact > 0.5 {
        actions.descent_engine_enable = false;
    }

    return actions;
}
