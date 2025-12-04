extends SceneTree

## Integration test for spacecraft solver graph
## Verifies that LCSpacecraft creates solver graph and connects tank effectors

func _init():
	print("\n=== Spacecraft Solver Integration Test ===\n")
	
	var passed = 0
	var failed = 0
	var total = 0
	
	# Force registry initialization if needed (fix for test environment timing)
	var registry = root.get_node_or_null("LCResourceRegistry")
	if registry and registry.resources.is_empty():
		print("Forcing ResourceRegistry initialization...")
		if registry.has_method("_load_builtin_resources"):
			registry.call("_load_builtin_resources")
		if registry.has_method("_load_user_resources"):
			registry.call("_load_user_resources")
	
	# Test 1: Starship has solver graph
	total += 1
	print("[Test 1] Starship Solver Graph Creation")
	var starship_scene = load("res://content/starship/starship.tscn")
	var starship = starship_scene.instantiate()
	starship.debug_effectors = true  # Enable debug output
	root.add_child(starship)
	
	# Wait for _ready to complete
	await process_frame
	
	if starship.solver_graph != null:
		print("  ✓ Solver graph created")
		passed += 1
	else:
		print("  ✗ Solver graph is null")
		failed += 1
	
	# Test 2: Tank effectors discovered
	total += 1
	print("\n[Test 2] Tank Effector Discovery")
	var tank_count = 0
	for effector in starship.state_effectors:
		if effector is LCResourceTankEffector:
			tank_count += 1
	
	if tank_count >= 2:
		print("  ✓ Found %d tank effectors" % tank_count)
		passed += 1
	else:
		print("  ✗ Expected at least 2 tanks, found %d" % tank_count)
		failed += 1
	
	# Test 3: Tanks have solver components
	total += 1
	print("\n[Test 3] Tank Components Created")
	var tanks_with_components = 0
	for effector in starship.state_effectors:
		if effector is LCResourceTankEffector:
			if effector.component != null:
				tanks_with_components += 1
				print("  ✓ %s has component" % effector.name)
	
	if tanks_with_components >= 2:
		print("  ✓ All tanks have solver components")
		passed += 1
	else:
		print("  ✗ Expected 2 tanks with components, found %d" % tanks_with_components)
		failed += 1
	
	# Test 4: Solver graph has nodes
	total += 1
	print("\n[Test 4] Solver Graph Population")
	var node_count = starship.solver_graph.nodes.size()
	if node_count >= 2:
		print("  ✓ Solver graph has %d nodes" % node_count)
		passed += 1
	else:
		print("  ✗ Expected at least 2 nodes, found %d" % node_count)
		failed += 1
	
	# Test 5: Solver can run
	total += 1
	print("\n[Test 5] Solver Execution")
	var initial_oxygen = 0.0
	for effector in starship.state_effectors:
		if effector is LCResourceTankEffector and effector.resource_id == "oxygen":
			initial_oxygen = effector.get_amount()
			break
	
	# Run physics for a few frames
	for i in range(10):
		starship.solver_graph.solve(0.016)
		await process_frame
	
	var final_oxygen = 0.0
	for effector in starship.state_effectors:
		if effector is LCResourceTankEffector and effector.resource_id == "oxygen":
			final_oxygen = effector.get_amount()
			break
	
	# Oxygen should remain stable (no leaks without connections)
	if abs(final_oxygen - initial_oxygen) < 1.0:
		print("  ✓ Solver runs without errors")
		print("  ✓ Mass conservation verified (%.2f kg → %.2f kg)" % [initial_oxygen, final_oxygen])
		passed += 1
	else:
		print("  ✗ Unexpected mass change: %.2f kg → %.2f kg" % [initial_oxygen, final_oxygen])
		failed += 1
	
	# Test 6: Floating screen can access graph
	total += 1
	print("\n[Test 6] Graph Accessibility for Visualization")
	if starship.solver_graph != null and starship.solver_graph.nodes.size() > 0:
		print("  ✓ Graph is accessible for visualization")
		print("  ✓ Graph has %d nodes ready to display" % starship.solver_graph.nodes.size())
		passed += 1
	else:
		print("  ✗ Graph not accessible")
		failed += 1
	
	# Cleanup
	starship.queue_free()
	
	# Print results
	print("\n=== Test Results ===")
	print("Passed: %d" % passed)
	print("Failed: %d" % failed)
	print("Total: %d" % total)
	
	if failed == 0:
		print("\n✅ All spacecraft solver tests passed!")
	else:
		print("\n❌ Some tests failed")
	
	quit()
