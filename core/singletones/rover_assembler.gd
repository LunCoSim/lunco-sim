class_name RoverAssembler
extends Node

## Helper class to assemble complex rovers at runtime.
## Can be used by the BuilderManager or test scripts.

const CHASSIS_SIZE = Vector3(2.0, 0.5, 3.0)
const WHEEL_RADIUS = 0.4

static func spawn_full_rover(parent: Node, position: Vector3) -> LCVehicle:
	# 1. Create Base Vehicle
	var rover = LCVehicle.new()
	rover.name = "FullRover_" + str(randi() % 1000)
	rover.position = position
	rover.mass = 100.0
	parent.add_child(rover)
	
	# 2. Add Visuals & Collision (Chassis)
	var chassis_mesh = CSGBox3D.new()
	chassis_mesh.size = CHASSIS_SIZE
	chassis_mesh.name = "ChassisVisual"
	rover.add_child(chassis_mesh)
	
	var collision = CollisionShape3D.new()
	var shape = BoxShape3D.new()
	shape.size = CHASSIS_SIZE
	collision.shape = shape
	collision.name = "ChassisCollision"
	rover.add_child(collision)
	
	# 3. Add Wheels (4x)
	_add_wheel(rover, Vector3(-1.0, 0.0, -1.0), true)  # Front Left
	_add_wheel(rover, Vector3( 1.0, 0.0, -1.0), true)  # Front Right
	_add_wheel(rover, Vector3(-1.0, 0.0,  1.0), false) # Back Left
	_add_wheel(rover, Vector3( 1.0, 0.0,  1.0), false) # Back Right
	
	# 4. Add State Effectors
	# Fuel Tank
	var tank = LCFuelTankEffector.new()
	tank.name = "FuelTank"
	tank.tank_dry_mass = 10.0
	tank.fuel_capacity = 50.0
	tank.fuel_mass = 50.0
	tank.position = Vector3(0, 0.5, 0)
	rover.add_child(tank)
	
	# Battery
	var battery = LCBatteryEffector.new()
	battery.name = "MainBattery"
	battery.capacity = 1000.0 # Wh
	battery.current_charge = 1000.0
	battery.position = Vector3(0, 0.3, 0)
	rover.add_child(battery)
	
	# Solar Panel
	var solar = LCSolarPanelEffector.new()
	solar.name = "SolarArray"
	solar.panel_area = 2.0
	solar.panel_efficiency = 0.25
	solar.position = Vector3(0, 0.6, -0.5)
	solar.is_deployable = true
	rover.add_child(solar)
	
	# Reaction Wheel (for stability)
	var rw = LCReactionWheelEffector.new()
	rw.name = "ReactionWheel"
	rw.max_torque = 50.0
	rw.position = Vector3(0, 0, 0)
	rover.add_child(rw)
	
	# 5. Add Dynamic Effectors
	# Thrusters (RCS style)
	_add_thruster(rover, tank, Vector3(0, 0, 1), Vector3(0, 0, -1.6), "MainEngine")
	
	# 6. Add Sensors
	# Lidar
	var lidar = LCLidarEffector.new()
	lidar.name = "LidarTop"
	lidar.lidar_mode = LCLidarEffector.LidarMode.HORIZONTAL_SCAN
	lidar.position = Vector3(0, 0.8, -1.0)
	lidar.max_range = 50.0
	rover.add_child(lidar)
	
	# Camera
	var cam = LCCameraEffector.new()
	cam.name = "FrontCamera"
	cam.position = Vector3(0, 0.5, -1.5)
	cam.enable_object_detection = true
	rover.add_child(cam)
	
	# IMU
	var imu = LCIMUEffector.new()
	imu.name = "IMU"
	imu.position = Vector3(0, 0, 0)
	rover.add_child(imu)
	
	# GPS
	var gps = LCGPSEffector.new()
	gps.name = "GPS"
	gps.position = Vector3(0, 0.5, 1.0)
	rover.add_child(gps)
	
	print("RoverAssembler: Spawned full rover at ", position)
	return rover

static func _add_wheel(parent: Node, pos: Vector3, steers: bool):
	var wheel = VehicleWheel3D.new()
	wheel.position = pos
	wheel.use_as_traction = true
	wheel.use_as_steering = steers
	wheel.wheel_radius = WHEEL_RADIUS
	wheel.suspension_travel = 0.3
	wheel.suspension_stiffness = 50.0
	parent.add_child(wheel)
	
	# Visual
	var mesh = CSGCylinder3D.new()
	mesh.radius = WHEEL_RADIUS
	mesh.height = 0.3
	mesh.rotation.z = PI/2
	wheel.add_child(mesh)

static func _add_thruster(parent: Node, tank: LCFuelTankEffector, dir: Vector3, pos: Vector3, name: String):
	var thruster = LCThrusterEffector.new()
	thruster.name = name
	thruster.position = pos
	thruster.thrust_direction = dir
	thruster.max_thrust = 1000.0
	thruster.fuel_tank = tank
	parent.add_child(thruster)
