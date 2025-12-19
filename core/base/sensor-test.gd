extends Node3D

## Test scene for robotic sensor effectors.
## Demonstrates lidar, IMU, camera, and GPS sensors.

@onready var robot: LCVehicle = $Robot

# Sensor references
var lidar: LCLidarEffector
var imu: LCIMUEffector
var camera: LCCameraEffector
var gps: LCGPSEffector

# Test controls
var test_mode: int = 0
var test_time: float = 0.0
var display_interval: float = 1.0
var last_display_time: float = 0.0

func _ready():
	print("=== Robotic Sensor Test ===")
	print("Controls:")
	print("  1 - Test Lidar")
	print("  2 - Test IMU")
	print("  3 - Test Camera")
	print("  4 - Test GPS")
	print("  5 - Test All Sensors")
	print("  Space - Reset")
	print("")
	
	_find_sensors()
	_print_sensor_summary()

func _process(delta):
	test_time += delta
	
	# Handle input
	if Input.is_action_just_pressed("ui_select"):  # Space
		_reset_test()
	
	if Input.is_key_pressed(KEY_1):
		test_mode = 1
	elif Input.is_key_pressed(KEY_2):
		test_mode = 2
	elif Input.is_key_pressed(KEY_3):
		test_mode = 3
	elif Input.is_key_pressed(KEY_4):
		test_mode = 4
	elif Input.is_key_pressed(KEY_5):
		test_mode = 5
	
	# Display status at intervals
	if test_time - last_display_time >= display_interval:
		_print_status()
		last_display_time = test_time

func _find_sensors():
	for effector in robot.state_effectors:
		if effector is LCLidarEffector:
			lidar = effector
		elif effector is LCIMUEffector:
			imu = effector
		elif effector is LCCameraEffector:
			camera = effector
		elif effector is LCGPSEffector:
			gps = effector

func _print_sensor_summary():
	print("Detected Sensors:")
	print("  Lidar: %s" % ("YES" if lidar else "NO"))
	print("  IMU: %s" % ("YES" if imu else "NO"))
	print("  Camera: %s" % ("YES" if camera else "NO"))
	print("  GPS: %s" % ("YES" if gps else "NO"))
	print("")

func _print_status():
	if test_mode == 0:
		return
	
	print("\n--- Sensor Status (t=%.1fs, Mode=%d) ---" % [test_time, test_mode])
	
	match test_mode:
		1:
			_print_lidar_status()
		2:
			_print_imu_status()
		3:
			_print_camera_status()
		4:
			_print_gps_status()
		5:
			_print_all_sensors_status()

func _print_lidar_status():
	if not lidar:
		print("Lidar: NOT FOUND")
		return
	
	print("Lidar:")
	print("  Mode: %s" % LCLidarEffector.LidarMode.keys()[lidar.lidar_mode])
	print("  Enabled: %s" % lidar.is_enabled)
	print("  Valid: %s" % lidar.is_valid)
	
	match lidar.lidar_mode:
		LCLidarEffector.LidarMode.SINGLE_BEAM:
			print("  Distance: %.2f m" % lidar.distance)
		LCLidarEffector.LidarMode.HORIZONTAL_SCAN:
			print("  Scan Points: %d" % lidar.get_point_count())
			if lidar.get_point_count() > 0:
				var closest = lidar.get_closest_point()
				print("  Closest Point: %.2f m at (%.2f, %.2f, %.2f)" % [
					closest.length(),
					closest.x, closest.y, closest.z
				])
		LCLidarEffector.LidarMode.FULL_3D:
			print("  Point Cloud: %d points" % lidar.get_point_count())

func _print_imu_status():
	if not imu:
		print("IMU: NOT FOUND")
		return
	
	print("IMU:")
	print("  Enabled: %s" % imu.is_enabled)
	print("  Valid: %s" % imu.is_valid)
	
	if imu.is_valid:
		var accel = imu.get_linear_acceleration()
		var gyro = imu.get_angular_velocity()
		
		print("  Linear Acceleration: (%.3f, %.3f, %.3f) m/s²" % [accel.x, accel.y, accel.z])
		print("  Angular Velocity: (%.3f, %.3f, %.3f) rad/s" % [gyro.x, gyro.y, gyro.z])
		
		if imu.enable_bias_drift:
			print("  Accel Bias: (%.4f, %.4f, %.4f)" % [
				imu.current_accel_bias.x,
				imu.current_accel_bias.y,
				imu.current_accel_bias.z
			])
			print("  Gyro Bias: (%.5f, %.5f, %.5f)" % [
				imu.current_gyro_bias.x,
				imu.current_gyro_bias.y,
				imu.current_gyro_bias.z
			])

func _print_camera_status():
	if not camera:
		print("Camera: NOT FOUND")
		return
	
	print("Camera:")
	print("  Enabled: %s" % camera.is_enabled)
	print("  Valid: %s" % camera.is_valid)
	print("  FOV: %.1f°" % camera.field_of_view)
	print("  Resolution: %dx%d" % [camera.resolution.x, camera.resolution.y])
	
	if camera.is_valid:
		var objects = camera.get_visible_objects()
		print("  Visible Objects: %d" % objects.size())
		
		if objects.size() > 0:
			print("  Objects in view:")
			for obj in objects.slice(0, min(5, objects.size())):
				print("    - %s: %.2fm, az=%.1f°, el=%.1f°%s" % [
					obj.name,
					obj.distance,
					obj.azimuth,
					obj.elevation,
					" [CENTER]" if obj.in_center else ""
				])
		
		if camera.enable_depth:
			print("  Center Depth: %.2f m" % camera.image_center_depth)

func _print_gps_status():
	if not gps:
		print("GPS: NOT FOUND")
		return
	
	print("GPS:")
	print("  Mode: %s" % LCGPSEffector.GPSMode.keys()[gps.gps_mode])
	print("  Enabled: %s" % gps.is_enabled)
	print("  Has Fix: %s" % gps.has_fix)
	print("  Satellites: %d" % gps.satellite_count)
	print("  HDOP: %.2f" % gps.hdop)
	
	if gps.has_valid_fix():
		var pos = gps.get_measured_position()
		print("  Position: (%.2f, %.2f, %.2f) m" % [pos.x, pos.y, pos.z])
		print("  Accuracy: ±%.2f m" % gps.get_position_accuracy())
		
		if gps.measure_velocity:
			var vel = gps.get_velocity()
			print("  Velocity: (%.2f, %.2f, %.2f) m/s" % [vel.x, vel.y, vel.z])
		
		if not gps.use_local_coordinates:
			print("  Lat/Lon: %.6f°, %.6f°" % [gps.latitude, gps.longitude])
			print("  Altitude: %.2f m" % gps.altitude)

func _print_all_sensors_status():
	_print_lidar_status()
	print("")
	_print_imu_status()
	print("")
	_print_camera_status()
	print("")
	_print_gps_status()

func _reset_test():
	test_mode = 0
	test_time = 0.0
	last_display_time = 0.0
	print("\n=== Test Reset ===\n")

# Helper function to create test obstacles
func create_test_obstacles():
	# Create some boxes for sensors to detect
	for i in range(5):
		var box = CSGBox3D.new()
		box.size = Vector3(1, 1, 1)
		box.position = Vector3(
			randf_range(-10, 10),
			0.5,
			randf_range(-10, 10)
		)
		add_child(box)
		
		# Add collision shape
		var static_body = StaticBody3D.new()
		var collision = CollisionShape3D.new()
		var shape = BoxShape3D.new()
		shape.size = box.size
		collision.shape = shape
		static_body.add_child(collision)
		box.add_child(static_body)
