extends SceneTree

## Test Pump Control System
## Verifies that pumps control flow to engines and engines produce thrust

func _init():
	print("\n=== Pump Control System Test ===\n")
	
	var passed = 0
	var failed = 0
	var total = 0
	
	# Load starship scene
	var starship_scene = load("res://content/starship/starship.tscn")
	if not starship_scene:
		print("✗ Failed to load starship scene")
		quit()
		return
	
	var starship = starship_scene.instantiate()
	root.add_child(starship)
	
	# Wait for _ready to be called
	await process_frame
	await process_frame
	
	# Test 1: Pump Discovery
	total += 1
	print("[Test 1] Pump Discovery")
	var fuel_pump = starship.get_node_or_null("FuelPump")
	var ox_pump = starship.get_node_or_null("OxidizerPump")
	
	if fuel_pump and ox_pump:
		print("  ✓ Both pumps found")
		print("    - FuelPump: max_pressure=%.0f Pa, max_flow=%.1f kg/s" % [fuel_pump.max_pressure, fuel_pump.max_flow])
		print("    - OxidizerPump: max_pressure=%.0f Pa, max_flow=%.1f kg/s" % [ox_pump.max_pressure, ox_pump.max_flow])
		passed += 1
	else:
		print("  ✗ Pumps not found")
		failed += 1
	
	# Test 2: Pump Solver Integration
	total += 1
	print("\n[Test 2] Pump Solver Integration")
	if starship.solver_graph and fuel_pump and ox_pump:
		# Check if pumps have components
		if fuel_pump.component and ox_pump.component:
			print("  ✓ Pumps integrated with solver graph")
			print("    - FuelPump component created")
			print("    - OxidizerPump component created")
			passed += 1
		else:
			print("  ✗ Pumps missing solver components")
			if not fuel_pump.component:
				print("    Missing: FuelPump.component")
			if not ox_pump.component:
				print("    Missing: OxidizerPump.component")
			failed += 1
	else:
		print("  ✗ Solver graph or pumps not available")
		failed += 1
	
	# Test 3: Pumps Start Closed
	total += 1
	print("\n[Test 3] Initial Pump State")
	if fuel_pump and ox_pump:
		if fuel_pump.pump_power == 0.0 and ox_pump.pump_power == 0.0:
			print("  ✓ Pumps start at 0% power (closed)")
			passed += 1
		else:
			print("  ✗ Pumps should start closed")
			print("    - FuelPump: %.1f%%" % (fuel_pump.pump_power * 100))
			print("    - OxidizerPump: %.1f%%" % (ox_pump.pump_power * 100))
			failed += 1
	else:
		print("  ✗ Pumps not available")
		failed += 1
	
	# Test 4: Engine with Pumps Closed
	total += 1
	print("\n[Test 4] Engine Thrust with Closed Pumps")
	var engine = starship.get_node_or_null("RocketEngine")
	
	if engine:
		# Run physics for a few frames with pumps closed
		for i in range(10):
			starship._physics_process(0.1)
			await process_frame
		
		if engine.current_thrust < 1.0:
			print("  ✓ Engine produces no thrust with closed pumps")
			print("    - Thrust: %.2f N" % engine.current_thrust)
			passed += 1
		else:
			print("  ✗ Engine should not thrust with closed pumps")
			print("    - Thrust: %.2f N" % engine.current_thrust)
			failed += 1
	else:
		print("  ✗ Engine not found")
		failed += 1
	
	# Test 5: Open Pumps and Check Flow
	total += 1
	print("\n[Test 5] Pump Power Control")
	if fuel_pump and ox_pump:
		# Set pumps to 50% power
		fuel_pump.set_pump_power(0.5)
		ox_pump.set_pump_power(0.5)
		
		# Run physics
		for i in range(10):
			starship._physics_process(0.1)
			await process_frame
		
		if fuel_pump.actual_flow_rate > 0.1 or ox_pump.actual_flow_rate > 0.1:
			print("  ✓ Pumps produce flow when powered")
			print("    - FuelPump flow: %.2f kg/s" % fuel_pump.actual_flow_rate)
			print("    - OxidizerPump flow: %.2f kg/s" % ox_pump.actual_flow_rate)
			passed += 1
		else:
			print("  ✗ Pumps should produce flow at 50% power")
			print("    - FuelPump flow: %.2f kg/s" % fuel_pump.actual_flow_rate)
			print("    - OxidizerPump flow: %.2f kg/s" % ox_pump.actual_flow_rate)
			failed += 1
	else:
		print("  ✗ Pumps not available")
		failed += 1
	
	# Test 6: Engine Thrust with Open Pumps
	total += 1
	print("\n[Test 6] Engine Thrust with Open Pumps")
	if engine:
		# Pumps are already at 50% from previous test
		# Run more physics frames
		for i in range(10):
			starship._physics_process(0.1)
			await process_frame
		
		if engine.current_thrust > 100.0:
			print("  ✓ Engine produces thrust with open pumps")
			print("    - Thrust: %.0f N" % engine.current_thrust)
			print("    - Mass flow: %.2f kg/s" % engine.actual_mass_flow)
			passed += 1
		else:
			print("  ✗ Engine should produce thrust with open pumps")
			print("    - Thrust: %.0f N" % engine.current_thrust)
			print("    - Mass flow: %.2f kg/s" % engine.actual_mass_flow)
			failed += 1
	else:
		print("  ✗ Engine not available")
		failed += 1
	
	# Test 7: Tank Depletion
	total += 1
	print("\n[Test 7] Fuel Tank Depletion")
	var methane_tank = starship.get_node_or_null("MethaneTank")
	var oxygen_tank = starship.get_node_or_null("OxygenTank")
	
	if methane_tank and oxygen_tank:
		var initial_methane = methane_tank.get_amount()
		var initial_oxygen = oxygen_tank.get_amount()
		
		# Run with pumps at 100% for longer
		fuel_pump.set_pump_power(1.0)
		ox_pump.set_pump_power(1.0)
		
		for i in range(50):
			starship._physics_process(0.1)
			await process_frame
		
		var final_methane = methane_tank.get_amount()
		var final_oxygen = oxygen_tank.get_amount()
		
		if final_methane < initial_methane and final_oxygen < initial_oxygen:
			print("  ✓ Tanks deplete with active pumps")
			print("    - Methane: %.0f → %.0f kg (%.0f kg consumed)" % [initial_methane, final_methane, initial_methane - final_methane])
			print("    - Oxygen: %.0f → %.0f kg (%.0f kg consumed)" % [initial_oxygen, final_oxygen, initial_oxygen - final_oxygen])
			passed += 1
		else:
			print("  ✗ Tanks should deplete")
			print("    - Methane: %.0f → %.0f kg" % [initial_methane, final_methane])
			print("    - Oxygen: %.0f → %.0f kg" % [initial_oxygen, final_oxygen])
			failed += 1
	else:
		print("  ✗ Tanks not available")
		failed += 1
	
	# Summary
	print("\n" + "=".repeat(50))
	print("Tests Passed: %d/%d" % [passed, total])
	print("Tests Failed: %d/%d" % [failed, total])
	print("=".repeat(50))
	
	quit()
