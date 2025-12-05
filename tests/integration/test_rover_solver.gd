extends SceneTree

## Test Rover Solver Integration

func _init():
	print("\n=== Rover Solver Integration Test ===\n")
	
	var passed = 0
	var failed = 0
	var total = 0
	
	# Load rover scene
	var rover_scene = load("res://apps/3dsim/entities/rover/rover.tscn")
	if not rover_scene:
		print("✗ Failed to load rover scene")
		quit()
		return
	
	var rover = rover_scene.instantiate()
	root.add_child(rover)
	
	# Wait for _ready to be called
	await process_frame
	await process_frame
	
	# Test 1: Rover has solver graph
	total += 1
	print("[Test 1] Rover Solver Graph Creation")
	if rover.solver_graph:
		print("  ✓ Rover has solver graph")
		passed += 1
	else:
		print("  ✗ Rover missing solver graph")
		failed += 1
	
	# Test 2: Battery discovered
	total += 1
	print("\n[Test 2] Battery Discovery")
	var battery = rover.get_node_or_null("Battery")
	if battery:
		print("  ✓ Battery node found")
		if battery.solver_node:
			print("    - Has solver node (Domain: %s)" % battery.solver_node.domain)
			print("    - Capacitance: %.2f F" % battery.solver_node.capacitance)
			passed += 1
		else:
			print("  ✗ Battery has no solver node")
			failed += 1
	else:
		print("  ✗ Battery node not found")
		failed += 1
	
	# Test 3: Solar Panel discovered
	total += 1
	print("\n[Test 3] Solar Panel Discovery")
	var solar = rover.get_node_or_null("SolarPanel")
	if solar:
		print("  ✓ Solar panel node found")
		if solar.solver_node:
			print("    - Has solver node (Domain: %s)" % solar.solver_node.domain)
			passed += 1
		else:
			print("  ✗ Solar panel has no solver node")
			failed += 1
	else:
		print("  ✗ Solar panel node not found")
		failed += 1
	
	# Test 4: Effectors in rover
	total += 1
	print("\n[Test 4] Rover Effector Discovery")
	print("  State effectors: %d" % rover.state_effectors.size())
	for effector in rover.state_effectors:
		print("    - %s (%s)" % [effector.name, effector.get_class()])
	
	print("  Dynamic effectors: %d" % rover.dynamic_effectors.size())
	for effector in rover.dynamic_effectors:
		print("    - %s (%s)" % [effector.name, effector.get_class()])
	
	if rover.state_effectors.size() >= 2 and rover.dynamic_effectors.size() >= 8:  # 8 motors
		print("  ✓ Rover discovered all effectors")
		passed += 1
	else:
		print("  ✗ Expected at least 2 state effectors and 8 dynamic effectors")
		print("    Got: %d state, %d dynamic" % [rover.state_effectors.size(), rover.dynamic_effectors.size()])
		failed += 1
	
	# Test 5: Solver graph nodes
	total += 1
	print("\n[Test 5] Solver Graph Nodes")
	if rover.solver_graph:
		print("  Total nodes in graph: %d" % rover.solver_graph.nodes.size())
		for node_id in rover.solver_graph.nodes:
			var node = rover.solver_graph.nodes[node_id]
			var name_str = node.display_name if node.display_name else ("Node " + str(node.id))
			print("    - %s (Domain: %s)" % [name_str, node.domain])
		
		if rover.solver_graph.nodes.size() >= 2:
			print("  ✓ Solver graph has nodes")
			passed += 1
		else:
			print("  ✗ Expected at least 2 nodes in solver graph")
			failed += 1
	else:
		print("  ✗ No solver graph")
		failed += 1
	
	# Summary
	print("\n" + "=".repeat(50))
	print("Tests Passed: %d/%d" % [passed, total])
	print("Tests Failed: %d/%d" % [failed, total])
	print("=".repeat(50))
	
	quit()
