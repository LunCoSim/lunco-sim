class_name LCLunarLander
extends LCSpacecraft

## Realistic Apollo-style lunar lander
##
## Features:
## - Descent stage (8,200 kg fuel, 45 kN engine)
## - Ascent stage (2,400 kg fuel, 15.6 kN engine)
## - 16 RCS thrusters
## - Life support and power systems

func _ready():
	super._ready()
	
	# Only build if we don't have children (fresh spawn)
	if get_child_count() == 0:
		_build_lander()
		refresh_effectors()
		
	# Ensure controller exists
	if not has_node("SpacecraftController"):
		var controller = LCSpacecraftController.new()
		controller.name = "SpacecraftController"
		add_child(controller)
		
	# Ensure input adapter exists
	if not has_node("SpacecraftInputAdapter"):
		var input_adapter = LCSpacecraftInputAdapter.new()
		input_adapter.name = "SpacecraftInputAdapter"
		# Set target to controller
		input_adapter.target = get_node("SpacecraftController")
		add_child(input_adapter)

func _build_lander():
	print("Building Lunar Lander...")
	
	# ========================================
	# DESCENT STAGE
	# ========================================
	
	# Descent stage structure
	var descent_structure = LCStateEffector.new()
	descent_structure.mass = 2150.0  # kg (dry mass)
	descent_structure.name = "DescentStageStructure"
	descent_structure.position = Vector3(0, 0, 0)
	add_child(descent_structure)
	
	var ds_mesh = CSGBox3D.new()
	ds_mesh.size = Vector3(4.0, 1.0, 4.0) # Octagonal base approx
	ds_mesh.material = StandardMaterial3D.new()
	ds_mesh.material.albedo_color = Color(0.9, 0.8, 0.4) # Gold foil
	ds_mesh.material.metallic = 0.8
	ds_mesh.material.roughness = 0.2
	descent_structure.add_child(ds_mesh)
	
	# Descent fuel tank (hypergolic propellant)
	var descent_fuel = LCFuelTankEffector.new()
	descent_fuel.fuel_capacity = 8200.0  # kg
	descent_fuel.fuel_mass = 8200.0  # Start full
	descent_fuel.tank_dry_mass = 200.0  # Tank structure mass
	descent_fuel.tank_type = LCFuelTankEffector.TankType.CYLINDRICAL
	descent_fuel.tank_height = 2.0
	descent_fuel.name = "DescentFuelTank"
	descent_fuel.position = Vector3(0, -0.5, 0)
	add_child(descent_fuel)
	
	var df_mesh = CSGCylinder3D.new()
	df_mesh.radius = 1.5
	df_mesh.height = 1.0
	df_mesh.sides = 16
	df_mesh.material = StandardMaterial3D.new()
	df_mesh.material.albedo_color = Color(0.7, 0.7, 0.7)
	descent_fuel.add_child(df_mesh)
	
	# Add collision shape for the main body
	var collision_shape = CollisionShape3D.new()
	var cylinder_shape = CylinderShape3D.new()
	cylinder_shape.radius = 2.0
	cylinder_shape.height = 2.0
	collision_shape.shape = cylinder_shape
	collision_shape.position = Vector3(0, 0, 0)
	add_child(collision_shape)
	
	# Configure physics
	mass = 15000.0  # Apollo LM mass (approximate)
	center_of_mass_mode = RigidBody3D.CENTER_OF_MASS_MODE_CUSTOM
	center_of_mass = Vector3(0, 0.5, 0)
	
	# Descent engine
	var descent_engine = LCThrusterEffector.new()
	descent_engine.max_thrust = 45000.0  # 45 kN (10,000 lbf)
	descent_engine.specific_impulse = 311.0  # seconds
	descent_engine.thrust_direction = Vector3(0, 1, 0)  # Upward thrust
	descent_engine.can_vector = true
	descent_engine.max_gimbal_angle = 6.0  # degrees
	descent_engine.gimbal_rate = 10.0  # deg/s
	descent_engine.efficiency = 0.95
	descent_engine.name = "DescentEngine"
	descent_engine.position = Vector3(0, -2.0, 0)
	add_child(descent_engine)
	
	var de_mesh = CSGCylinder3D.new()
	de_mesh.radius = 0.8
	de_mesh.height = 1.5
	de_mesh.cone = true # Make it a nozzle shape
	de_mesh.material = StandardMaterial3D.new()
	de_mesh.material.albedo_color = Color(0.2, 0.2, 0.2)
	descent_engine.add_child(de_mesh)
	
	# Landing legs (4 legs)
	for i in range(4):
		var angle = i * PI / 2.0
		var leg_position = Vector3(cos(angle) * 2.0, -1.5, sin(angle) * 2.0)
		
		var leg = LCStateEffector.new()
		leg.mass = 30.0  # kg per leg
		leg.name = "LandingLeg_" + str(i)
		leg.position = leg_position
		add_child(leg)
		
		var leg_mesh = CSGCylinder3D.new()
		leg_mesh.radius = 0.1
		leg_mesh.height = 2.0
		leg_mesh.material = StandardMaterial3D.new()
		leg_mesh.material.albedo_color = Color(0.9, 0.8, 0.4)
		leg.add_child(leg_mesh)
		
		var foot_mesh = CSGCylinder3D.new()
		foot_mesh.radius = 0.5
		foot_mesh.height = 0.1
		foot_mesh.position.y = -1.0
		leg.add_child(foot_mesh)
	
	# ========================================
	# ASCENT STAGE
	# ========================================
	
	# Ascent stage structure (crew cabin)
	var ascent_structure = LCStateEffector.new()
	ascent_structure.mass = 2180.0  # kg (includes crew cabin, life support)
	ascent_structure.name = "AscentStageStructure"
	ascent_structure.position = Vector3(0, 1.5, 0)
	add_child(ascent_structure)
	
	var as_mesh = CSGSphere3D.new() # Approximate cabin shape
	as_mesh.radius = 1.8
	as_mesh.material = StandardMaterial3D.new()
	as_mesh.material.albedo_color = Color(0.8, 0.8, 0.8)
	ascent_structure.add_child(as_mesh)
	
	# Ascent fuel tank
	var ascent_fuel = LCFuelTankEffector.new()
	ascent_fuel.fuel_capacity = 2400.0  # kg
	ascent_fuel.fuel_mass = 2400.0  # Start full
	ascent_fuel.tank_dry_mass = 100.0
	ascent_fuel.tank_type = LCFuelTankEffector.TankType.SPHERICAL
	ascent_fuel.name = "AscentFuelTank"
	ascent_fuel.position = Vector3(0, 1.0, 0)
	add_child(ascent_fuel)
	
	# Ascent engine
	var ascent_engine = LCThrusterEffector.new()
	ascent_engine.max_thrust = 15600.0  # 15.6 kN (3,500 lbf)
	ascent_engine.specific_impulse = 311.0  # seconds
	ascent_engine.thrust_direction = Vector3(0, 1, 0)
	ascent_engine.can_vector = false  # Fixed nozzle
	ascent_engine.efficiency = 0.95
	ascent_engine.name = "AscentEngine"
	ascent_engine.position = Vector3(0, 0.5, 0)
	add_child(ascent_engine)
	
	# ========================================
	# RCS SYSTEM (Reaction Control System)
	# ========================================
	
	# RCS fuel tank (shared for all thrusters)
	var rcs_fuel = LCResourceTankEffector.new()
	rcs_fuel.resource_id = "hydrazine"
	rcs_fuel.capacity = 287.0  # kg
	rcs_fuel.set_amount(287.0)
	rcs_fuel.name = "RCS_FuelTank"
	rcs_fuel.position = Vector3(0, 1.2, 0)
	add_child(rcs_fuel)
	
	# RCS thrusters (16 total: 4 clusters of 4)
	var rcs_positions = [
		Vector3(1.5, 1.5, 0),   # Right
		Vector3(-1.5, 1.5, 0),  # Left
		Vector3(0, 1.5, 1.5),   # Front
		Vector3(0, 1.5, -1.5),  # Back
	]
	
	var rcs_directions = [
		[Vector3(-1, 0, 0), Vector3(0, 1, 0), Vector3(0, -1, 0), Vector3(0, 0, 1)],
		[Vector3(1, 0, 0), Vector3(0, 1, 0), Vector3(0, -1, 0), Vector3(0, 0, -1)],
		[Vector3(0, 0, -1), Vector3(0, 1, 0), Vector3(0, -1, 0), Vector3(1, 0, 0)],
		[Vector3(0, 0, 1), Vector3(0, 1, 0), Vector3(0, -1, 0), Vector3(-1, 0, 0)],
	]
	
	for cluster_idx in range(4):
		for thruster_idx in range(4):
			var rcs = LCThrusterEffector.new()
			rcs.max_thrust = 445.0  # 445 N (100 lbf)
			rcs.specific_impulse = 280.0  # seconds
			rcs.thrust_direction = rcs_directions[cluster_idx][thruster_idx]
			rcs.fuel_resource_id = "hydrazine"
			rcs.min_on_time = 0.014  # 14ms minimum pulse
			rcs.efficiency = 0.90
			rcs.name = "RCS_C" + str(cluster_idx) + "_T" + str(thruster_idx)
			rcs.position = rcs_positions[cluster_idx]
			add_child(rcs)
	
	# ========================================
	# POWER SYSTEM
	# ========================================
	
	# Primary batteries
	var battery_primary = LCBatteryEffector.new()
	battery_primary.capacity = 28.0  # kWh (Apollo LM had ~28 kWh)
	battery_primary.charge_level = 1.0
	battery_primary.max_charge_rate = 5000.0  # 5 kW
	battery_primary.max_discharge_rate = 5000.0
	battery_primary.name = "PrimaryBattery"
	battery_primary.position = Vector3(0.5, 1.0, 0)
	add_child(battery_primary)
	
	# Secondary battery (backup)
	var battery_secondary = LCBatteryEffector.new()
	battery_secondary.capacity = 14.0  # kWh
	battery_secondary.charge_level = 1.0
	battery_secondary.max_charge_rate = 5000.0
	battery_secondary.max_discharge_rate = 5000.0
	battery_secondary.name = "SecondaryBattery"
	battery_secondary.position = Vector3(-0.5, 1.0, 0)
	add_child(battery_secondary)
	
	# ========================================
	# LIFE SUPPORT
	# ========================================
	
	# Oxygen tank
	var oxygen_tank = LCResourceTankEffector.new()
	oxygen_tank.resource_id = "oxygen"
	oxygen_tank.capacity = 48.0  # kg (enough for 2 crew, 3 days)
	oxygen_tank.set_amount(48.0)
	oxygen_tank.name = "OxygenTank"
	oxygen_tank.position = Vector3(0.8, 1.3, 0)
	add_child(oxygen_tank)
	
	# Water tank
	var water_tank = LCResourceTankEffector.new()
	water_tank.resource_id = "water"
	water_tank.capacity = 150.0  # kg
	water_tank.set_amount(150.0)
	water_tank.name = "WaterTank"
	water_tank.position = Vector3(-0.8, 1.3, 0)
	add_child(water_tank)
	
	# Life support system (consumes O2 and power)
	var life_support = LCStateEffector.new()
	life_support.mass = 150.0
	life_support.power_consumption = 600.0  # 600W average
	life_support.name = "LifeSupportSystem"
	life_support.position = Vector3(0, 1.4, 0)
	add_child(life_support)
	
	# ========================================
	# SENSORS AND AVIONICS
	# ========================================
	
	# Avionics and guidance computer
	var avionics = LCStateEffector.new()
	avionics.mass = 70.0  # kg
	avionics.power_consumption = 300.0  # 300W
	avionics.name = "Avionics"
	avionics.position = Vector3(0, 1.6, 0)
	add_child(avionics)
	
	# Communications
	var comms = LCStateEffector.new()
	comms.mass = 30.0
	comms.power_consumption = 150.0  # 150W
	comms.name = "Communications"
	comms.position = Vector3(0, 2.0, 0)
	add_child(comms)
