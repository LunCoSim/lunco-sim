//! Compile the reusable lander propulsion pieces and one USD-shaped assembly.
//!
//! The scene owns these connections, so this test mirrors the generated
//! topology: two tanks feed two pumps, and the pumps feed one chamber through
//! the shared acausal `FluidPort`. A scalar-only mock would miss the
//! pressure/flow contract that the runtime now projects.

use lunco_modelica::ModelicaCompiler;

fn package_member(suffix: &str) -> String {
    lunco_assets::models::package_files("LunCo")
        .into_iter()
        .find(|(path, _)| path.ends_with(suffix))
        .map(|(_, source)| source)
        .unwrap_or_else(|| panic!("{suffix} is part of the shipped LunCo package"))
}

#[test]
fn propulsion_members_compile_with_their_declared_icons_and_ports() {
    for suffix in [
        "Propulsion/PropellantStatus.mo",
        "Propulsion/RCSThruster.mo",
        "Propulsion/RCSJet.mo",
    ] {
        let mut compiler = ModelicaCompiler::new();
        let class_name = suffix
            .rsplit_once('/')
            .and_then(|(_, name)| name.strip_suffix(".mo"))
            .expect("package member filename");
        let result = compiler.compile_str(
            class_name,
            &package_member(suffix),
            &format!("lunco://models/LunCo/{suffix}"),
        );
        assert!(result.is_ok(), "{suffix}: {:?}", result.err());
        assert!(
            package_member(suffix).contains("extends LunCo.Icons."),
            "{suffix} must inherit an authored semantic Modelica icon"
        );
    }
}

#[test]
fn generated_style_propulsion_network_compiles_and_publishes_evidence() {
    let source = r#"
model GeneratedPropulsion
  input Real valve_opening;
  output Real thrust_n;
  output Real fuel_mass_kg;
  output Real chamber_pressure_pa;

  LunCo.Propulsion.PropellantTank fuel_tank(initial_mass_kg = 1000.0);
  LunCo.Propulsion.PropellantTank oxidizer_tank(initial_mass_kg = 1000.0);
  LunCo.Propulsion.Turbopump fuel_pump(maximum_flow_kgs = 8.0);
  LunCo.Propulsion.Turbopump oxidizer_pump(maximum_flow_kgs = 21.0);
  LunCo.Propulsion.CombustionChamber chamber;

equation
  fuel_pump.valve_opening = valve_opening;
  oxidizer_pump.valve_opening = valve_opening;
  fuel_pump.available_mass_kg = fuel_tank.mass_kg;
  oxidizer_pump.available_mass_kg = oxidizer_tank.mass_kg;
  connect(fuel_tank.outlet, fuel_pump.inlet);
  connect(fuel_pump.outlet, chamber.fuel_in);
  connect(oxidizer_tank.outlet, oxidizer_pump.inlet);
  connect(oxidizer_pump.outlet, chamber.oxidizer_in);
  thrust_n = chamber.thrust_n;
  fuel_mass_kg = fuel_tank.mass_kg;
  chamber_pressure_pa = chamber.chamber_pressure_pa;
end GeneratedPropulsion;
"#;

    let mut compiler = ModelicaCompiler::new();
    let dae = compiler
        .compile_str(
            "GeneratedPropulsion",
            source,
            "generated://GeneratedPropulsion.mo",
        )
        .expect("generated propulsion network compiles");
    let names: Vec<String> = dae
        .dae
        .variables
        .outputs
        .keys()
        .map(ToString::to_string)
        .collect();
    for expected in ["thrust_n", "fuel_mass_kg", "chamber_pressure_pa"] {
        assert!(
            names
                .iter()
                .any(|name| name == expected || name.ends_with(expected)),
            "generated network must publish `{expected}`, got {names:?}"
        );
    }
}

#[test]
fn generated_rcs_network_preserves_each_valve_binding() {
    let source = r#"
model GeneratedRcs
  input Real pitch_pos_a_valve;
  input Real pitch_pos_b_valve;
  input Real pitch_neg_a_valve;
  input Real pitch_neg_b_valve;
  input Real roll_pos_a_valve;
  input Real roll_pos_b_valve;
  input Real roll_neg_a_valve;
  input Real roll_neg_b_valve;
  input Real yaw_pos_a_valve;
  input Real yaw_pos_b_valve;
  input Real yaw_neg_a_valve;
  input Real yaw_neg_b_valve;
  output Real pitch_pos_a_thrust_n;
  output Real pitch_pos_b_thrust_n;
  output Real pitch_neg_a_thrust_n;
  output Real pitch_neg_b_thrust_n;
  output Real roll_pos_a_thrust_n;
  output Real roll_pos_b_thrust_n;
  output Real roll_neg_a_thrust_n;
  output Real roll_neg_b_thrust_n;
  output Real yaw_pos_a_thrust_n;
  output Real yaw_pos_b_thrust_n;
  output Real yaw_neg_a_thrust_n;
  output Real yaw_neg_b_thrust_n;

  LunCo.Propulsion.RCSJet RcsPitchPosModel(f_nom_n = 2500.0);
  LunCo.Propulsion.RCSJet RcsPitchPosBModel(f_nom_n = 2500.0);
  LunCo.Propulsion.RCSJet RcsPitchNegModel(f_nom_n = 2500.0);
  LunCo.Propulsion.RCSJet RcsPitchNegBModel(f_nom_n = 2500.0);
  LunCo.Propulsion.RCSJet RcsRollPosModel(f_nom_n = 2500.0);
  LunCo.Propulsion.RCSJet RcsRollPosBModel(f_nom_n = 2500.0);
  LunCo.Propulsion.RCSJet RcsRollNegModel(f_nom_n = 2500.0);
  LunCo.Propulsion.RCSJet RcsRollNegBModel(f_nom_n = 2500.0);
  LunCo.Propulsion.RCSJet RcsYawPosModel(f_nom_n = 2500.0);
  LunCo.Propulsion.RCSJet RcsYawPosBModel(f_nom_n = 2500.0);
  LunCo.Propulsion.RCSJet RcsYawNegModel(f_nom_n = 2500.0);
  LunCo.Propulsion.RCSJet RcsYawNegBModel(f_nom_n = 2500.0);

equation
  RcsPitchPosModel.valve_opening = pitch_pos_a_valve;
  RcsPitchPosBModel.valve_opening = pitch_pos_b_valve;
  RcsPitchNegModel.valve_opening = pitch_neg_a_valve;
  RcsPitchNegBModel.valve_opening = pitch_neg_b_valve;
  RcsRollPosModel.valve_opening = roll_pos_a_valve;
  RcsRollPosBModel.valve_opening = roll_pos_b_valve;
  RcsRollNegModel.valve_opening = roll_neg_a_valve;
  RcsRollNegBModel.valve_opening = roll_neg_b_valve;
  RcsYawPosModel.valve_opening = yaw_pos_a_valve;
  RcsYawPosBModel.valve_opening = yaw_pos_b_valve;
  RcsYawNegModel.valve_opening = yaw_neg_a_valve;
  RcsYawNegBModel.valve_opening = yaw_neg_b_valve;
  pitch_pos_a_thrust_n = RcsPitchPosModel.thrust_n;
  pitch_pos_b_thrust_n = RcsPitchPosBModel.thrust_n;
  pitch_neg_a_thrust_n = RcsPitchNegModel.thrust_n;
  pitch_neg_b_thrust_n = RcsPitchNegBModel.thrust_n;
  roll_pos_a_thrust_n = RcsRollPosModel.thrust_n;
  roll_pos_b_thrust_n = RcsRollPosBModel.thrust_n;
  roll_neg_a_thrust_n = RcsRollNegModel.thrust_n;
  roll_neg_b_thrust_n = RcsRollNegBModel.thrust_n;
  yaw_pos_a_thrust_n = RcsYawPosModel.thrust_n;
  yaw_pos_b_thrust_n = RcsYawPosBModel.thrust_n;
  yaw_neg_a_thrust_n = RcsYawNegModel.thrust_n;
  yaw_neg_b_thrust_n = RcsYawNegBModel.thrust_n;
end GeneratedRcs;
    "#;

    let mut compiler = ModelicaCompiler::new();
    let prior_source = r#"
model PriorAllocator
  input Real desired_force_x;
  input Real desired_force_y;
  input Real desired_force_z;
  input Real desired_torque_x;
  input Real desired_torque_y;
  input Real desired_torque_z;
  output Real command_a;
  output Real command_b;

  LunCo.Actuation.WrenchAllocator allocator(
    actuator_count = 2,
    allocation_pinv = [0.0, 0.0, 0.0, 1.0, 0.0, 0.0; 0.0, 0.0, 0.0, -1.0, 0.0, 0.0],
    lower_command = {0.0, 0.0},
    upper_command = {1.0, 1.0});
equation
  allocator.desired_force_x = desired_force_x;
  allocator.desired_force_y = desired_force_y;
  allocator.desired_force_z = desired_force_z;
  allocator.desired_torque_x = desired_torque_x;
  allocator.desired_torque_y = desired_torque_y;
  allocator.desired_torque_z = desired_torque_z;
  command_a = allocator.command[1];
  command_b = allocator.command[2];
end PriorAllocator;
"#;
    compiler
        .compile_str(
            "PriorAllocator",
            prior_source,
            "generated://PriorAllocator.mo",
        )
        .expect("the preceding generated allocator compiles");
    let dae = compiler
        .compile_str("GeneratedRcs", source, "generated://GeneratedRcs.mo")
        .expect("generated RCS network compiles");
    let mut opts = rumoca_sim::SimOptions::default();
    opts.t_end = 2.0;
    let mut stepper =
        rumoca_sim::SimulationSession::new(&dae.dae, opts).expect("generated RCS stepper builds");
    stepper
        .set_input("roll_pos_a_valve", 0.8)
        .expect("positive valve is a live input");
    for input in [
        "pitch_pos_a_valve",
        "pitch_pos_b_valve",
        "pitch_neg_a_valve",
        "pitch_neg_b_valve",
        "roll_pos_b_valve",
        "roll_neg_a_valve",
        "roll_neg_b_valve",
        "yaw_pos_a_valve",
        "yaw_pos_b_valve",
        "yaw_neg_a_valve",
        "yaw_neg_b_valve",
    ] {
        stepper
            .set_input(input, 0.0)
            .unwrap_or_else(|error| panic!("{input} is a live input: {error}"));
    }
    stepper.step(1.0 / 60.0).expect("RCS network steps");

    let pos = stepper
        .get("RcsRollPosModel.valve_opening")
        .expect("positive valve is observable")
        .expect("positive valve has a value");
    let neg = stepper
        .get("RcsRollNegModel.valve_opening")
        .expect("negative valve is observable")
        .expect("negative valve has a value");
    assert!(
        (pos - 0.8).abs() < 1.0e-9,
        "positive valve was remapped: {pos}"
    );
    assert!(neg.abs() < 1.0e-9, "negative valve was remapped: {neg}");

    let plume_intensity = stepper
        .get("RcsRollPosModel.light_intensity")
        .expect("positive RCS plume intensity is observable")
        .expect("positive RCS plume intensity has a value");
    let plume_radius = stepper
        .get("RcsRollPosModel.light_radius")
        .expect("positive RCS plume radius is observable")
        .expect("positive RCS plume radius has a value");
    assert!(
        plume_intensity.is_finite() && plume_intensity > 0.0,
        "positive valve must drive a visible plume light: {plume_intensity}"
    );
    assert!(
        plume_radius > 0.06,
        "positive valve must expand the plume source radius: {plume_radius}"
    );
}
