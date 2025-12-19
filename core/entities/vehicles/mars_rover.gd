class_name LCMarsRover
extends LCVehicle

## Realistic Mars rover (Curiosity/Perseverance style)
##
## Features:
## - RTG power source (120W continuous)
## - 6-wheel rocker-bogie suspension
## - Science instruments (cameras, spectrometers, drill)
## - Communications systems

func _ready():
	super._ready()
	
	# Only build if we don't have children (fresh spawn)
	if get_child_count() == 0:
		_build_rover()
		refresh_effectors()
		
	# Ensure controller exists
	if not has_node("RoverController"):
		var controller = LCRoverController.new()
		controller.name = "RoverController"
		add_child(controller)
		
	# Ensure input adapter exists
	if not has_node("RoverInputAdapter"):
		var input_adapter = LCRoverInputAdapter.new()
		input_adapter.name = "RoverInputAdapter"
		# Set target to controller
		input_adapter.target = get_node("RoverController")
		add_child(input_adapter)

func _build_rover():
	print("Building Mars Rover...")
	
	# ========================================
	# CHASSIS AND STRUCTURE
	# ========================================
	
	# Main chassis
	var chassis = LCStateEffector.new()
	chassis.mass = 180.0  # kg (main body structure)
	chassis.name = "Chassis"
	chassis.position = Vector3(0, 0.5, 0)
	add_child(chassis)
	
	var chassis_mesh = CSGBox3D.new()
	chassis_mesh.size = Vector3(2.0, 0.5, 3.0)
	chassis_mesh.material = StandardMaterial3D.new()
	chassis_mesh.material.albedo_color = Color(0.8, 0.8, 0.8)
	chassis.add_child(chassis_mesh)
	
	# Warm Electronics Box (WEB) - houses computers
	var web = LCStateEffector.new()
	web.mass = 45.0  # kg
	web.power_consumption = 50.0  # 50W for computing
	web.name = "WarmElectronicsBox"
	web.position = Vector3(0, 0.6, 0)
	add_child(web)
	
	var web_mesh = CSGBox3D.new()
	web_mesh.size = Vector3(1.5, 0.4, 2.0)
	web_mesh.material = StandardMaterial3D.new()
	web_mesh.material.albedo_color = Color(0.9, 0.7, 0.0) # Gold foil
	web.add_child(web_mesh)
	
	# Add collision shape for the main body
	var collision_shape = CollisionShape3D.new()
	var box_shape = BoxShape3D.new()
	box_shape.size = Vector3(2.0, 0.5, 3.0)
	collision_shape.shape = box_shape
	collision_shape.position = Vector3(0, 0.5, 0)
	add_child(collision_shape)
	
	# Configure physics
	mass = 900.0  # Curiosity rover mass
	center_of_mass_mode = RigidBody3D.CENTER_OF_MASS_MODE_CUSTOM
	center_of_mass = Vector3(0, 0.3, 0)
	
	# ========================================
	# MOBILITY SYSTEM (6 wheels)
	# ========================================
	
	# Wheel positions (rocker-bogie suspension)
	var wheel_positions = [
		Vector3(0.8, 0, 1.2),   # Front left
		Vector3(0.8, 0, 0),     # Middle left
		Vector3(0.8, 0, -1.2),  # Rear left
		Vector3(-0.8, 0, 1.2),  # Front right
		Vector3(-0.8, 0, 0),    # Middle right
		Vector3(-0.8, 0, -1.2), # Rear right
	]
	
	for i in range(6):
		var wheel = LCWheelEffector.new()
		wheel.mass = 15.0  # kg per wheel
		wheel.wheel_radius = 0.25  # 25cm radius
		wheel.suspension_stiffness = 50.0
		wheel.suspension_damping = 5.0
		wheel.max_torque = 200.0  # Nm
		
		# Configure traction and steering
		wheel.use_as_traction = true # All-wheel drive
		
		# Corner steering (Front and Rear wheels)
		# Indices: 0 (FL), 2 (RL), 3 (FR), 5 (RR)
		if i == 0 or i == 2 or i == 3 or i == 5:
			wheel.use_as_steering = true
			
		wheel.name = "Wheel_" + str(i)
		wheel.position = wheel_positions[i]
		add_child(wheel)
		
		var wheel_mesh = CSGCylinder3D.new()
		wheel_mesh.radius = 0.25
		wheel_mesh.height = 0.2
		wheel_mesh.sides = 16
		wheel_mesh.rotation.z = PI / 2
		wheel_mesh.material = StandardMaterial3D.new()
		wheel_mesh.material.albedo_color = Color(0.2, 0.2, 0.2)
		wheel.add_child(wheel_mesh)
	
	# ========================================
	# POWER SYSTEM
	# ========================================
	
	# Multi-Mission Radioisotope Thermoelectric Generator (MMRTG)
	var rtg = LCStateEffector.new()
	rtg.mass = 45.0  # kg
	rtg.name = "RTG"
	rtg.position = Vector3(0, 0.3, -1.5)
	
	# RTG produces constant power (decays slowly over years)
	# Thermal power: 2000W, Electrical efficiency: 6%
	rtg.power_production = 120.0  # 120W electrical (at beginning of life)
	add_child(rtg)
	
	var rtg_mesh = CSGCylinder3D.new()
	rtg_mesh.radius = 0.3
	rtg_mesh.height = 0.8
	rtg_mesh.rotation.x = PI / 4 # Angled
	rtg_mesh.material = StandardMaterial3D.new()
	rtg_mesh.material.albedo_color = Color(0.9, 0.9, 0.9)
	rtg.add_child(rtg_mesh)
	
	# Lithium-ion battery (for peak power demands)
	var battery = LCBatteryEffector.new()
	battery.capacity = 5.0  # 5 kWh (42 Ah at 28V)
	battery.charge_level = 1.0
	battery.max_charge_rate = 200.0  # 200W charging
	battery.max_discharge_rate = 500.0  # 500W peak discharge
	battery.name = "Battery"
	battery.position = Vector3(0, 0.4, 0)
	add_child(battery)
	
	# ========================================
	# SCIENCE INSTRUMENTS
	# ========================================
	
	# Mast Camera (Mastcam)
	var mastcam = LCStateEffector.new()
	mastcam.mass = 3.0  # kg
	mastcam.power_consumption = 15.0  # 15W when active
	mastcam.name = "Mastcam"
	mastcam.position = Vector3(0, 2.0, 0.3)
	add_child(mastcam)
	
	var mast_mesh = CSGCylinder3D.new()
	mast_mesh.radius = 0.05
	mast_mesh.height = 1.2
	mast_mesh.position.y = -0.6
	mastcam.add_child(mast_mesh)
	
	var head_mesh = CSGBox3D.new()
	head_mesh.size = Vector3(0.3, 0.2, 0.2)
	mastcam.add_child(head_mesh)
	
	# ChemCam (laser spectrometer)
	var chemcam = LCStateEffector.new()
	chemcam.mass = 5.5  # kg
	chemcam.power_consumption = 30.0  # 30W when firing laser
	chemcam.name = "ChemCam"
	chemcam.position = Vector3(0, 2.0, 0.4)
	add_child(chemcam)
	
	# Sample Analysis at Mars (SAM)
	var sam = LCStateEffector.new()
	sam.mass = 40.0  # kg
	sam.power_consumption = 80.0  # 80W when analyzing
	sam.name = "SAM"
	sam.position = Vector3(0, 0.5, 0.5)
	add_child(sam)
	
	# Robotic arm
	var arm = LCStateEffector.new()
	arm.mass = 30.0  # kg
	arm.power_consumption = 40.0  # 40W when moving
	arm.name = "RoboticArm"
	arm.position = Vector3(0.5, 0.5, 0.8)
	add_child(arm)
	
	# Drill
	var drill = LCStateEffector.new()
	drill.mass = 8.0  # kg
	drill.power_consumption = 100.0  # 100W when drilling
	drill.name = "Drill"
	drill.position = Vector3(0.5, 0.3, 0.9)
	add_child(drill)
	
	# ========================================
	# COMMUNICATIONS
	# ========================================
	
	# High-gain antenna (for direct Earth communication)
	var hga = LCStateEffector.new()
	hga.mass = 8.0  # kg
	hga.power_consumption = 50.0  # 50W when transmitting
	hga.name = "HighGainAntenna"
	hga.position = Vector3(0, 1.0, -0.5)
	add_child(hga)
	
	# UHF antenna (for Mars orbiter relay)
	var uhf = LCStateEffector.new()
	uhf.mass = 2.0  # kg
	uhf.power_consumption = 15.0  # 15W when transmitting
	uhf.name = "UHF_Antenna"
	uhf.position = Vector3(0, 0.8, 0)
	add_child(uhf)
	
	# ========================================
	# THERMAL CONTROL
	# ========================================
	
	# Radioisotope Heater Units (RHUs) - passive heating
	for i in range(8):
		var rhu = LCStateEffector.new()
		rhu.mass = 0.04  # 40g each
		rhu.name = "RHU_" + str(i)
		# Positioned around sensitive components
		add_child(rhu)
	
	# ========================================
	# NAVIGATION AND SENSORS
	# ========================================
	
	# Inertial Measurement Unit (IMU)
	var imu = LCStateEffector.new()
	imu.mass = 1.5  # kg
	imu.power_consumption = 5.0  # 5W
	imu.name = "IMU"
	imu.position = Vector3(0, 0.6, 0)
	add_child(imu)
	
	# Hazard avoidance cameras (4 pairs)
	for i in range(8):
		var hazcam = LCStateEffector.new()
		hazcam.mass = 0.5  # kg
		hazcam.power_consumption = 5.0  # 5W when active
		hazcam.name = "HazCam_" + str(i)
		add_child(hazcam)
	
	# Navigation cameras (2 pairs)
	for i in range(4):
		var navcam = LCStateEffector.new()
		navcam.mass = 0.6  # kg
		navcam.power_consumption = 6.0  # 6W when active
		navcam.name = "NavCam_" + str(i)
		navcam.position = Vector3(0, 1.8, 0.2)
		add_child(navcam)
	
	# ========================================
	# SAMPLE COLLECTION SYSTEM
	# ========================================
	
	# Sample cache (for storing core samples)
	var sample_cache = LCStateEffector.new()
	sample_cache.mass = 15.0  # kg (includes storage tubes)
	sample_cache.name = "SampleCache"
	sample_cache.position = Vector3(0, 0.4, -0.5)
	add_child(sample_cache)
