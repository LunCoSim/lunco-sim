model PropellantMassContracts
  "Distributed live tank groups, asymmetric depletion and dry-limit regression"
  LunCo.Propulsion.PropellantStatus distributed(
    dry_mass_kg=2000.0,
    dry_com_x_m=0.2, dry_com_y_m=0.1, dry_com_z_m=-0.3,
    dry_inertia_xx_kg_m2=4625.0, dry_inertia_yy_kg_m2=6250.0,
    dry_inertia_zz_kg_m2=4625.0,
    fuel_position_x_m=0.0, fuel_position_y_m=1.39, fuel_position_z_m=0.0,
    oxidizer_position_x_m=0.0, oxidizer_position_y_m=1.39, oxidizer_position_z_m=0.0,
    fuel_specific_inertia_xx_m2=0.0, fuel_specific_inertia_yy_m2=0.7225,
    fuel_specific_inertia_zz_m2=0.7225,
    oxidizer_specific_inertia_xx_m2=0.7225, oxidizer_specific_inertia_yy_m2=0.7225,
    oxidizer_specific_inertia_zz_m2=0.0,
    fuel_mass_kg=1000.0*(1.0-time),
    oxidizer_mass_kg=1000.0*(1.0-time)^2);
  // Omitted group spread describes point loads at the species centroids.
  LunCo.Propulsion.PropellantStatus point(
    fuel_mass_kg=1000.0, oxidizer_mass_kg=1000.0);
  // Negative inventories must not contribute mass, COM or residual inertia.
  LunCo.Propulsion.PropellantStatus empty(
    fuel_mass_kg=-1.0, oxidizer_mass_kg=-2.0,
    fuel_specific_inertia_xx_m2=7.0, oxidizer_specific_inertia_zz_m2=9.0);
end PropellantMassContracts;
