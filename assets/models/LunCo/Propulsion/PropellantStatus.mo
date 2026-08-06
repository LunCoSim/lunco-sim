within LunCo.Propulsion;

model PropellantStatus
  "Vehicle-level propellant telemetry and event aggregation"
  extends LunCo.Icons.PropellantStatus;

  parameter Real low_fuel_mass_kg = 200.0;
  parameter Real depleted_mass_kg = 1.0;
  parameter Real initial_propellant_mass_kg = 2000.0;
  parameter Real minimum_mass_kg = 1.0e-6;
  parameter Real event_transition_width_kg = 0.01;
  parameter Real dry_mass_kg = 2000.0;
  parameter Real dry_com_x_m = 0.0;
  parameter Real dry_com_y_m = 0.0;
  parameter Real dry_com_z_m = 0.0;
  parameter Real dry_inertia_xx_kg_m2 = 4625.0;
  parameter Real dry_inertia_yy_kg_m2 = 6250.0;
  parameter Real dry_inertia_zz_kg_m2 = 4625.0;
  parameter Real fuel_position_x_m = 0.0;
  parameter Real fuel_position_y_m = 0.8;
  parameter Real fuel_position_z_m = 0.0;
  parameter Real oxidizer_position_x_m = 0.0;
  parameter Real oxidizer_position_y_m = -0.8;
  parameter Real oxidizer_position_z_m = 0.0;

  input Real fuel_mass_kg = 0.0;
  input Real oxidizer_mass_kg = 0.0;
  output Real propellant_mass_kg;
  output Real propellant_used_kg;
  output Real propellant_fraction "Remaining fraction of the authored propellant load";
  output Real low_fuel;
  output Real depleted;
  output Real vehicle_mass_kg "Dry airframe plus live propellant mass";
  output Real vehicle_inertia_xx_kg_m2;
  output Real vehicle_inertia_yy_kg_m2;
  output Real vehicle_inertia_zz_kg_m2;
  output Real vehicle_com_x_m;
  output Real vehicle_com_y_m;
  output Real vehicle_com_z_m;

protected
  Real total_mass_kg;

equation
  propellant_mass_kg = max(0.0, fuel_mass_kg) + max(0.0, oxidizer_mass_kg);
  propellant_used_kg = max(0.0, initial_propellant_mass_kg - propellant_mass_kg);
  propellant_fraction = max(0.0, min(1.0,
    propellant_mass_kg / max(minimum_mass_kg, initial_propellant_mass_kg)));
  low_fuel = max(0.0, min(1.0,
    0.5 + 0.5 * (low_fuel_mass_kg - propellant_mass_kg)
      / max(minimum_mass_kg, event_transition_width_kg)));
  depleted = max(0.0, min(1.0,
    0.5 + 0.5 * (depleted_mass_kg - propellant_mass_kg)
      / max(minimum_mass_kg, event_transition_width_kg)));

  // The tank locations are authored parameters of this component. Treating
  // each live tank load as a point mass keeps the equations explicit while
  // applying the same parallel-axis shift as the rigid-body projection.
  total_mass_kg = max(minimum_mass_kg, dry_mass_kg + propellant_mass_kg);
  vehicle_mass_kg = total_mass_kg;
  vehicle_com_x_m = (dry_mass_kg * dry_com_x_m
      + max(0.0, fuel_mass_kg) * fuel_position_x_m
      + max(0.0, oxidizer_mass_kg) * oxidizer_position_x_m) / total_mass_kg;
  vehicle_com_y_m = (dry_mass_kg * dry_com_y_m
      + max(0.0, fuel_mass_kg) * fuel_position_y_m
      + max(0.0, oxidizer_mass_kg) * oxidizer_position_y_m) / total_mass_kg;
  vehicle_com_z_m = (dry_mass_kg * dry_com_z_m
      + max(0.0, fuel_mass_kg) * fuel_position_z_m
      + max(0.0, oxidizer_mass_kg) * oxidizer_position_z_m) / total_mass_kg;
  vehicle_inertia_xx_kg_m2 = dry_inertia_xx_kg_m2
      + dry_mass_kg * ((dry_com_y_m - vehicle_com_y_m)^2
        + (dry_com_z_m - vehicle_com_z_m)^2)
      + max(0.0, fuel_mass_kg) * ((fuel_position_y_m - vehicle_com_y_m)^2
        + (fuel_position_z_m - vehicle_com_z_m)^2)
      + max(0.0, oxidizer_mass_kg) * ((oxidizer_position_y_m - vehicle_com_y_m)^2
        + (oxidizer_position_z_m - vehicle_com_z_m)^2);
  vehicle_inertia_yy_kg_m2 = dry_inertia_yy_kg_m2
      + dry_mass_kg * ((dry_com_x_m - vehicle_com_x_m)^2
        + (dry_com_z_m - vehicle_com_z_m)^2)
      + max(0.0, fuel_mass_kg) * ((fuel_position_x_m - vehicle_com_x_m)^2
        + (fuel_position_z_m - vehicle_com_z_m)^2)
      + max(0.0, oxidizer_mass_kg) * ((oxidizer_position_x_m - vehicle_com_x_m)^2
        + (oxidizer_position_z_m - vehicle_com_z_m)^2);
  vehicle_inertia_zz_kg_m2 = dry_inertia_zz_kg_m2
      + dry_mass_kg * ((dry_com_x_m - vehicle_com_x_m)^2
        + (dry_com_y_m - vehicle_com_y_m)^2)
      + max(0.0, fuel_mass_kg) * ((fuel_position_x_m - vehicle_com_x_m)^2
        + (fuel_position_y_m - vehicle_com_y_m)^2)
      + max(0.0, oxidizer_mass_kg) * ((oxidizer_position_x_m - vehicle_com_x_m)^2
        + (oxidizer_position_y_m - vehicle_com_y_m)^2);
end PropellantStatus;
