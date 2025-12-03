extends SceneTree

## Standalone test for the upgraded LCSolverGraph with storage nodes and integration

var tests_passed = 0
var tests_failed = 0

func _initialize():
	print("\n=== Running LCSolverGraph Upgrade Tests ===\n")
	
	test_storage_node_integration()
	test_pump_with_pressure_source()
	test_unidirectional_flow()
	test_mass_clamping()
	
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

func assert_gte(value, expected, message):
	if value >= expected:
		print("  ✓ ", message)
		tests_passed += 1
	else:
		print("  ✗ ", message, " (expected >= ", expected, ", got ", value, ")")
		tests_failed += 1

func assert_eq(value, expected, message):
	if value == expected:
		print("  ✓ ", message)
		tests_passed += 1
	else:
		print("  ✗ ", message, " (expected ", expected, ", got ", value, ")")
		tests_failed += 1

func assert_almost_eq(value, expected, tolerance, message):
	if abs(value - expected) < tolerance:
		print("  ✓ ", message)
		tests_passed += 1
	else:
		print("  ✗ ", message, " (expected ~", expected, ", got ", value, ")")
		tests_failed += 1

func test_storage_node_integration():
	print("\n[Test 1] Storage Node Integration")
	
	# Create a simple graph: Tank A -> Pipe -> Tank B
	var graph = LCSolverGraph.new()
	
	# Tank A (starts with 100kg)
	var tank_a = graph.add_node(0.0, false, "Fluid")
	tank_a.set_capacitance(100.0)  # 100kg capacity
	tank_a.flow_accumulation = 50.0  # Start with 50kg
	tank_a.potential = tank_a.flow_accumulation / tank_a.capacitance  # 0.5
	
	# Tank B (starts empty)
	var tank_b = graph.add_node(0.0, false, "Fluid")
	tank_b.set_capacitance(100.0)
	tank_b.flow_accumulation = 0.0
	tank_b.potential = 0.0
	
	# Pipe connecting them (high conductance for fast flow)
	var pipe = graph.connect_nodes(tank_a, tank_b, 10.0, "Fluid")
	
	# Simulate for 1 second
	var delta = 1.0
	graph.solve(delta)
	
	# Verify flow direction (should be A -> B due to pressure difference)
	assert_gt(pipe.flow_rate, 0.0, "Flow should be from A to B")
	
	# Verify mass conservation
	var total_mass_before = 50.0
	var total_mass_after = tank_a.flow_accumulation + tank_b.flow_accumulation
	assert_almost_eq(total_mass_before, total_mass_after, 0.01, "Mass should be conserved")
	
	# Verify Tank A lost mass
	assert_lt(tank_a.flow_accumulation, 50.0, "Tank A should have lost mass")
	
	# Verify Tank B gained mass
	assert_gt(tank_b.flow_accumulation, 0.0, "Tank B should have gained mass")

func test_pump_with_pressure_source():
	print("\n[Test 2] Pump with Pressure Source")
	
	# Create graph: Tank A -> Pump -> Tank B
	var graph = LCSolverGraph.new()
	
	# Tank A (low pressure)
	var tank_a = graph.add_node(0.0, false, "Fluid")
	tank_a.set_capacitance(100.0)
	tank_a.flow_accumulation = 10.0
	tank_a.potential = 0.1
	
	# Tank B (high pressure - would normally prevent flow)
	var tank_b = graph.add_node(0.0, false, "Fluid")
	tank_b.set_capacitance(100.0)
	tank_b.flow_accumulation = 80.0
	tank_b.potential = 0.8
	
	# Pump (adds pressure source to overcome back-pressure)
	var pump = graph.connect_nodes(tank_a, tank_b, 5.0, "Fluid")
	pump.potential_source = 1.0  # Strong pump
	
	# Solve
	graph.solve(1.0)
	
	# Verify pump overcomes back-pressure
	assert_gt(pump.flow_rate, 0.0, "Pump should push fluid uphill")

func test_unidirectional_flow():
	print("\n[Test 3] Unidirectional Flow (Check Valve)")
	
	# Test check valve behavior
	var graph = LCSolverGraph.new()
	
	var node_a = graph.add_node(0.5, false, "Fluid")
	var node_b = graph.add_node(1.0, false, "Fluid")  # Higher pressure
	
	var check_valve = graph.connect_nodes(node_a, node_b, 10.0, "Fluid")
	check_valve.is_unidirectional = true
	
	graph.solve(0.1)
	
	# Flow should be blocked (B has higher pressure than A)
	assert_eq(check_valve.flow_rate, 0.0, "Check valve should block reverse flow")

func test_mass_clamping():
	print("\n[Test 4] Mass Clamping (No Negative Mass)")
	
	# Verify that tanks cannot go negative
	var graph = LCSolverGraph.new()
	
	var tank = graph.add_node(0.0, false, "Fluid")
	tank.set_capacitance(100.0)
	tank.flow_accumulation = 5.0  # Very little mass
	
	var sink = graph.add_node(0.0, true, "Fluid")  # Ground (vacuum)
	sink.potential = 0.0
	
	var pipe = graph.connect_nodes(tank, sink, 100.0, "Fluid")  # High conductance
	
	# Simulate for long enough to drain tank
	for i in range(10):
		graph.solve(1.0)
	
	# Tank should be empty, not negative
	assert_gte(tank.flow_accumulation, 0.0, "Mass should not go negative")
	assert_almost_eq(tank.flow_accumulation, 0.0, 0.01, "Tank should be empty")
