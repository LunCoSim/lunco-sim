extends Node3D

## Test scene for critical spacecraft effectors.
## Demonstrates fuel tanks, thrusters, reaction wheels, solar panels, and batteries.

@onready var spacecraft: LCVehicle = $Spacecraft

# Test controls
var test_mode: int = 0
var test_time: float = 0.0

func _ready():
	print("=== Spacecraft Effector Test ===")
	print("Controls:")
	print("  1 - Test Thrusters")
	print("  2 - Test Reaction Wheels")
	print("  3 - Test Solar Panels")
	print("  4 - Test Battery Charge/Discharge")
	print("  5 - Test Fuel Depletion")
	print("  Space - Reset")
	print("")
	
	_print_initial_state()

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
	
	# Run test
	match test_mode:
		1:
			_test_thrusters(delta)
		2:
			_test_reaction_wheels(delta)
		3:
			_test_solar_panels(delta)
		4:
			_test_battery(delta)
		5:
			_test_fuel_depletion(delta)
	
	# Print status every second
	if int(test_time) % 1 == 0 and test_time - delta < int(test_time):
		_print_status()

func _test_thrusters(delta):
	# Fire thrusters in sequence
	var thrusters = _get_effectors_of_type(LCThrusterEffector)
	if thrusters.is_empty():
		return
	
	var thruster_index = int(test_time) % thrusters.size()
	var thruster: LCThrusterEffector = thrusters[thruster_index]
	
	# Pulse thrust
	var pulse_phase = fmod(test_time, 2.0)
	if pulse_phase < 1.0:
		thruster.set_thrust(0.5 + 0.5 * sin(test_time * PI))
	else:
		thruster.set_thrust(0.0)

func _test_reaction_wheels(delta):
	# Command reaction wheels to apply torque
	var rws = _get_effectors_of_type(LCReactionWheelEffector)
	if rws.is_empty():
		return
	
	for rw: LCReactionWheelEffector in rws:
		# Sinusoidal torque command
		var torque_level = sin(test_time * 2.0)
		rw.set_torque_normalized(torque_level)

func _test_solar_panels(delta):
	# Rotate sun direction to test solar panel tracking
	var sun_dir = Vector3(cos(test_time * 0.5), 0.5, sin(test_time * 0.5)).normalized()
	
	var panels = _get_effectors_of_type(LCSolarPanelEffector)
	for panel: LCSolarPanelEffector in panels:
		panel.update_sun_direction(sun_dir)
		if panel.can_articulate:
			panel.enable_sun_tracking()

func _test_battery(delta):
	# Cycle battery charge/discharge
	var batteries = _get_effectors_of_type(LCBatteryEffector)
	if batteries.is_empty():
		return
	
	var battery: LCBatteryEffector = batteries[0]
	
	# Charge for 5 seconds, discharge for 5 seconds
	var cycle_phase = fmod(test_time, 10.0)
	if cycle_phase < 5.0:
		battery.charge(100.0, delta)
	else:
		battery.discharge(50.0, delta)

func _test_fuel_depletion(delta):
	# Fire all thrusters to deplete fuel
	var thrusters = _get_effectors_of_type(LCThrusterEffector)
	for thruster: LCThrusterEffector in thrusters:
		thruster.set_thrust(1.0)  # Full thrust

func _reset_test():
	test_mode = 0
	test_time = 0.0
	
	# Reset all effectors
	var thrusters = _get_effectors_of_type(LCThrusterEffector)
	for thruster: LCThrusterEffector in thrusters:
		thruster.set_thrust(0.0)
	
	var rws = _get_effectors_of_type(LCReactionWheelEffector)
	for rw: LCReactionWheelEffector in rws:
		rw.set_torque(0.0)
	
	print("\n=== Test Reset ===\n")

func _print_initial_state():
	print("Initial Spacecraft State:")
	print("  Mass: %.2f kg" % spacecraft.total_mass)
	print("  Power Production: %.2f W" % spacecraft.power_production)
	print("  Power Consumption: %.2f W" % spacecraft.power_consumption)
	print("")
	
	_print_effector_summary()

func _print_status():
	if test_mode == 0:
		return
	
	print("\n--- Status (t=%.1fs, Mode=%d) ---" % [test_time, test_mode])
	print("Vehicle:")
	print("  Mass: %.2f kg" % spacecraft.total_mass)
	print("  Power: %.2f W (prod) - %.2f W (cons) = %.2f W (net)" % [
		spacecraft.power_production,
		spacecraft.power_consumption,
		spacecraft.power_available
	])
	print("  Velocity: %s" % spacecraft.linear_velocity)
	print("  Angular Velocity: %s" % spacecraft.angular_velocity)
	
	# Print effector-specific status
	match test_mode:
		1:
			_print_thruster_status()
		2:
			_print_reaction_wheel_status()
		3:
			_print_solar_panel_status()
		4:
			_print_battery_status()
		5:
			_print_fuel_status()

func _print_thruster_status():
	var thrusters = _get_effectors_of_type(LCThrusterEffector)
	print("Thrusters:")
	for thruster: LCThrusterEffector in thrusters:
		print("  %s: %.2f N (%.1f%% cmd), firing=%s" % [
			thruster.name,
			thruster.current_thrust,
			thruster.thrust_command * 100,
			thruster.is_firing
		])

func _print_reaction_wheel_status():
	var rws = _get_effectors_of_type(LCReactionWheelEffector)
	print("Reaction Wheels:")
	for rw: LCReactionWheelEffector in rws:
		print("  %s: %.3f Nms (%.1f%% full), speed=%.2f rad/s, sat=%s" % [
			rw.name,
			rw.stored_momentum,
			(abs(rw.stored_momentum) / rw.max_momentum) * 100,
			rw.wheel_speed,
			rw.is_saturated
		])

func _print_solar_panel_status():
	var panels = _get_effectors_of_type(LCSolarPanelEffector)
	print("Solar Panels:")
	for panel: LCSolarPanelEffector in panels:
		print("  %s: %.2f W, sun_angle=%.1f°, deployed=%.1f%%" % [
			panel.name,
			panel.current_power_output,
			panel.get_sun_angle(),
			panel.deployment_fraction * 100
		])

func _print_battery_status():
	var batteries = _get_effectors_of_type(LCBatteryEffector)
	print("Batteries:")
	for battery: LCBatteryEffector in batteries:
		print("  %s: %.2f Wh (%.1f%% SoC), charging=%s, discharging=%s" % [
			battery.name,
			battery.current_charge,
			battery.state_of_charge * 100,
			battery.is_charging,
			battery.is_discharging
		])

func _print_fuel_status():
	var tanks = _get_effectors_of_type(LCResourceTankEffector)
	print("Fuel Tanks:")
	for tank: LCResourceTankEffector in tanks:
		print("  %s: %.2f kg (%.1f%% full)" % [
			tank.name,
			tank.get_amount(),
			tank.get_fill_percentage()
		])

func _print_effector_summary():
	print("Effectors:")
	print("  Fuel Tanks (Resource): %d" % _get_effectors_of_type(LCResourceTankEffector).size())
	print("  Thrusters: %d" % _get_effectors_of_type(LCThrusterEffector).size())
	print("  Reaction Wheels: %d" % _get_effectors_of_type(LCReactionWheelEffector).size())
	print("  Solar Panels: %d" % _get_effectors_of_type(LCSolarPanelEffector).size())
	print("  Batteries: %d" % _get_effectors_of_type(LCBatteryEffector).size())
	print("")

func _get_effectors_of_type(type) -> Array:
	var result = []
	for effector in spacecraft.state_effectors:
		if is_instance_of(effector, type):
			result.append(effector)
	for effector in spacecraft.dynamic_effectors:
		if is_instance_of(effector, type):
			result.append(effector)
	return result
