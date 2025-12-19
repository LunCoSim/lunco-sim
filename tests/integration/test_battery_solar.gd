extends SceneTree

## Test Battery and Solar Panel Solver Integration

func _init():
	print("\n=== Battery & Solar Panel Solver Integration Test ===\n")
	
	var passed = 0
	var failed = 0
	var total = 0
	
	# Create solver graph
	var graph = LCSolverGraph.new()
	
	# Test 1: Battery creates solver node
	total += 1
	print("[Test 1] Battery Solver Node Creation")
	var battery = LCBatteryEffector.new()
	battery.name = "TestBattery"
	battery.capacity = 1000.0  # 1000 Wh
	battery.nominal_voltage = 28.0
	battery.initial_charge = 500.0  # 50% charged
	battery._ready()  # Manually call _ready
	battery.set_solver_graph(graph)
	
	if battery.solver_node:
		print("  ✓ Battery created solver node")
		print("    - Domain: %s" % battery.solver_node.domain)
		print("    - Capacitance: %.2f F" % battery.solver_node.capacitance)
		print("    - Initial charge: %.2f Coulombs" % battery.solver_node.flow_accumulation)
		passed += 1
	else:
		print("  ✗ Battery failed to create solver node")
		failed += 1
	
	# Test 2: Solar Panel creates solver node
	total += 1
	print("\n[Test 2] Solar Panel Solver Node Creation")
	var solar = LCSolarPanelEffector.new()
	solar.name = "TestSolarPanel"
	solar.panel_area = 10.0  # 10 m²
	solar.panel_efficiency = 0.3
	solar.max_power_output = 3000.0  # 3 kW
	solar._ready()  # Manually call _ready
	solar.set_solver_graph(graph)
	
	if solar.solver_node:
		print("  ✓ Solar panel created solver node")
		print("    - Domain: %s" % solar.solver_node.domain)
		passed += 1
	else:
		print("  ✗ Solar panel failed to create solver node")
		failed += 1
	
	# Test 3: Connect solar panel to battery
	total += 1
	print("\n[Test 3] Connect Solar Panel to Battery")
	
	if solar.solver_node and battery.solver_node:
		# Add a small conductance (wire resistance)
		var wire_conductance = 100.0  # High conductance (low resistance)
		var edge = graph.connect_nodes(solar.solver_node, battery.solver_node, wire_conductance, "Electrical")
		
		if edge:
			print("  ✓ Connected solar panel to battery")
			passed += 1
		else:
			print("  ✗ Failed to connect nodes")
			failed += 1
	else:
		print("  ✗ Missing solver nodes")
		failed += 1
	
	# Test 4: Simulate solar charging
	total += 1
	print("\n[Test 4] Solar Charging Simulation")
	
	# Set sun direction to point at panel
	solar.sun_direction = Vector3(0, 0, 1)
	solar.solar_flux = 1361.0  # Full sun
	
	# Run simulation for a few frames
	var initial_charge = battery.current_charge
	print("  Initial battery charge: %.2f Wh" % initial_charge)
	
	for i in range(10):
		solar._update_power_generation(0.1)
		graph.solve(0.1)
		battery._physics_process(0.1)
	
	var final_charge = battery.current_charge
	print("  Final battery charge: %.2f Wh" % final_charge)
	print("  Solar power output: %.2f W" % solar.current_power_output)
	print("  Solar current: %.2f A" % solar.solver_node.flow_source)
	print("  Battery voltage: %.2f V" % battery.solver_node.potential)
	
	if final_charge > initial_charge:
		print("  ✓ Battery is charging from solar panel")
		passed += 1
	else:
		print("  ✗ Battery not charging (charge: %.2f -> %.2f)" % [initial_charge, final_charge])
		failed += 1
	
	# Test 5: Verify graph has correct nodes
	total += 1
	print("\n[Test 5] Graph Topology")
	print("  Total nodes: %d" % graph.nodes.size())
	print("  Total edges: %d" % graph.edges.size())
	
	if graph.nodes.size() == 2 and graph.edges.size() == 1:
		print("  ✓ Graph topology correct")
		passed += 1
	else:
		print("  ✗ Unexpected graph topology")
		failed += 1
	
	# Summary
	print("\n" + "=".repeat(50))
	print("Tests Passed: %d/%d" % [passed, total])
	print("Tests Failed: %d/%d" % [failed, total])
	print("=".repeat(50))
	
	quit()

