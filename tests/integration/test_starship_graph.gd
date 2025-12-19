extends SceneTree

## Test Starship Solver Graph Integration

func _init():
	print("\n=== Starship Solver Graph Test ===\n")
	
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
	
	# Test 1: Starship has solver graph
	total += 1
	print("[Test 1] Starship Solver Graph Creation")
	if starship.solver_graph:
		print("  ✓ Starship has solver graph")
		passed += 1
	else:
		print("  ✗ Starship missing solver graph")
		failed += 1
	
	# Test 2: Tanks discovered
	total += 1
	print("\n[Test 2] Tank Discovery")
	var oxygen_tank = starship.get_node_or_null("OxygenTank")
	var methane_tank = starship.get_node_or_null("MethaneTank")
	
	if oxygen_tank and methane_tank:
		print("  ✓ Both tanks found")
		print("    - Oxygen: %.0f kg" % oxygen_tank.get_amount())
		print("    - Methane: %.0f kg" % methane_tank.get_amount())
		passed += 1
	else:
		print("  ✗ Tanks not found")
		failed += 1
	
	# Test 3: Thruster discovered
	total += 1
	print("\n[Test 3] Thruster Discovery")
	var thruster = starship.get_node_or_null("RocketEngine")
	
	if thruster:
		print("  ✓ Thruster found")
		print("    - Max thrust: %.0f N" % thruster.max_thrust)
		print("    - Fuel tank: %s" % thruster.fuel_tank.name if thruster.fuel_tank else "None")
		print("    - Oxidizer tank: %s" % thruster.oxidizer_tank.name if thruster.oxidizer_tank else "None")
		passed += 1
	else:
		print("  ✗ Thruster not found")
		failed += 1
	
	# Test 4: Solver graph nodes
	total += 1
	print("\n[Test 4] Solver Graph Nodes")
	if starship.solver_graph:
		print("  Total nodes: %d" % starship.solver_graph.nodes.size())
		for node_id in starship.solver_graph.nodes:
			var node = starship.solver_graph.nodes[node_id]
			var name_str = node.display_name if node.display_name else ("Node " + str(node.id))
			print("    - %s (Domain: %s, Resource: %s)" % [name_str, node.domain, node.resource_type])
		
		# Should have: 2 tanks + 1 engine node = 3 nodes
		if starship.solver_graph.nodes.size() >= 3:
			print("  ✓ Expected nodes present")
			passed += 1
		else:
			print("  ✗ Expected at least 3 nodes, got %d" % starship.solver_graph.nodes.size())
			failed += 1
	else:
		print("  ✗ No solver graph")
		failed += 1
	
	# Test 5: Connections
	total += 1
	print("\n[Test 5] Graph Connections")
	if starship.solver_graph:
		print("  Total edges: %d" % starship.solver_graph.edges.size())
		for edge_id in starship.solver_graph.edges:
			var edge = starship.solver_graph.edges[edge_id]
			var from_name = edge.node_a.display_name if edge.node_a.display_name else ("Node " + str(edge.node_a.id))
			var to_name = edge.node_b.display_name if edge.node_b.display_name else ("Node " + str(edge.node_b.id))
			print("    - %s → %s" % [from_name, to_name])
		
		# Should have 2 connections (tank → thruster for fuel and oxidizer)
		if starship.solver_graph.edges.size() >= 2:
			print("  ✓ Connections present")
			passed += 1
		else:
			print("  ✗ Expected at least 2 connections")
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
