extends SceneTree

## Integration test for Supply Chain components with the solver

var tests_passed = 0
var tests_failed = 0

func _initialize():
	print("\n=== Running Supply Chain Integration Tests ===\n")
	
	test_storage_to_storage()
	test_pump_between_tanks()
	
	print("\n=== Test Results ===")
	print("Passed: ", tests_passed)
	print("Failed: ", tests_failed)
	print("Total: ", tests_passed + tests_failed)
	
	if tests_failed == 0:
		print("\n✅ All tests passed!")
		quit(0)
	else:
		print("\n❌ Some tests failed!")
		quit(1)

func assert_gt(value, expected, message):
	if value > expected:
		print("  ✓ ", message)
		tests_passed += 1
	else:
		print("  ✗ ", message, " (expected > ", expected, ", got ", value, ")")
		tests_failed += 1

func assert_lt(value, expected, message):
	if value < expected:
		print("  ✓ ", message)
		tests_passed += 1
	else:
		print("  ✗ ", message, " (expected < ", expected, ", got ", value, ")")
		tests_failed += 1

func assert_almost_eq(value, expected, tolerance, message):
	if abs(value - expected) < tolerance:
		print("  ✓ ", message)
		tests_passed += 1
	else:
		print("  ✗ ", message, " (expected ~", expected, ", got ", value, ")")
		tests_failed += 1

func test_storage_to_storage():
	print("\n[Test 1] Storage to Storage Connection")
	
	# Create simulation manager
	var sim = SimulationManager.new()
	
	# Create two storage tanks
	var tank_a = StorageFacility.new()
	tank_a.name = "TankA"
	tank_a.capacity = 100.0
	tank_a.current_amount = 50.0
	tank_a.stored_resource_type = "water"
	
	var tank_b = StorageFacility.new()
	tank_b.name = "TankB"
	tank_b.capacity = 100.0
	tank_b.current_amount = 0.0
	tank_b.stored_resource_type = "water"
	
	# Add to simulation
	sim.add_node(tank_a)
	sim.add_node(tank_b)
	
	# Connect them
	var result = sim.connect_nodes("TankA", 0, "TankB", 0)
	
	# Resume simulation (it's paused by default)
	sim.resume_simulation()
	
	# Simulate for a few steps
	for i in range(10):
		sim._physics_process(0.1)
	
	# Verify mass transfer
	var total_mass = tank_a.current_amount + tank_b.current_amount
	assert_almost_eq(total_mass, 50.0, 0.1, "Mass should be conserved")
	assert_lt(tank_a.current_amount, 50.0, "Tank A should have lost mass")
	assert_gt(tank_b.current_amount, 0.0, "Tank B should have gained mass")
	
	sim.free()

func test_pump_between_tanks():
	print("\n[Test 2] Pump Between Tanks")
	
	# Create simulation manager
	var sim = SimulationManager.new()
	
	# Create source tank (low pressure)
	var source = StorageFacility.new()
	source.name = "Source"
	source.capacity = 100.0
	source.current_amount = 10.0
	source.stored_resource_type = "water"
	
	# Create target tank (high pressure - would block passive flow)
	var target = StorageFacility.new()
	target.name = "Target"
	target.capacity = 100.0
	target.current_amount = 80.0
	target.stored_resource_type = "water"
	
	# Create pump
	var pump = Pump.new()
	pump.name = "Pump"
	pump.pump_rate = 10.0
	pump.power_available = 100.0  # Sufficient power
	
	# Add to simulation
	sim.add_node(source)
	sim.add_node(pump)
	sim.add_node(target)
	
	# Connect: Source -> Pump -> Target
	sim.connect_nodes("Source", 0, "Pump", 0)  # Source to pump inlet
	sim.connect_nodes("Pump", 0, "Target", 0)  # Pump outlet to target
	
	# Resume simulation
	sim.resume_simulation()
	
	# Simulate
	for i in range(10):
		sim._physics_process(0.1)
	
	# Verify pump pushed water uphill
	assert_lt(source.current_amount, 10.0, "Source should have lost mass")
	assert_gt(target.current_amount, 80.0, "Target should have gained mass despite back-pressure")
	
	sim.free()
