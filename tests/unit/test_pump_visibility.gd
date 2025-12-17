extends SceneTree

## Test that pumps now create visible nodes in the solver graph

const LCSolverGraph = preload("res://core/systems/solver/solver_graph.gd")
const LCPumpComponent = preload("res://core/systems/components/pump_component.gd")
const LCTankComponent = preload("res://core/systems/components/tank_component.gd")

func _init():
	print("\n=== Testing Pump Node Visibility ===\n")
	
	var passed = 0
	var failed = 0
	var total = 0
	
	# Create solver graph
	var graph = LCSolverGraph.new()
	
	# Test 1: Create tanks
	total += 1
	print("[Test 1] Create Source and Sink Tanks")
	var source_tank = LCTankComponent.new(graph, 1.0, 2.0)
	source_tank.set_initial_mass(100.0)
	var sink_tank = LCTankComponent.new(graph, 1.0, 2.0)
	sink_tank.set_initial_mass(0.0)
	
	print("  Source tank node ID: %d" % source_tank.get_port("port").id)
	print("  Sink tank node ID: %d" % sink_tank.get_port("port").id)
	print("  ✓ Tanks created")
	passed += 1
	
	# Test 2: Create pump
	total += 1
	print("\n[Test 2] Create Pump Component")
	var pump = LCPumpComponent.new(graph, 50000.0)  # 50 kPa
	pump.max_flow = 10.0
	pump._update_conductance()
	
	if pump.pump_node:
		print("  ✓ Pump node created (ID: %d)" % pump.pump_node.id)
		print("    Display name: '%s'" % pump.pump_node.display_name)
		print("    Resource type: '%s'" % pump.pump_node.resource_type)
		print("    Domain: '%s'" % pump.pump_node.domain)
		passed += 1
	else:
		print("  ✗ Pump node NOT created")
		failed += 1
	
	# Test 3: Connect pump
	total += 1
	print("\n[Test 3] Connect Pump to Tanks")
	pump.connect_nodes(source_tank.get_port("port"), sink_tank.get_port("port"))
	
	if pump.inlet_edge and pump.outlet_edge:
		print("  ✓ Pump edges created")
		print("    Inlet: Node %d → Node %d" % [pump.inlet_edge.node_a.id, pump.inlet_edge.node_b.id])
		print("    Outlet: Node %d → Node %d" % [pump.outlet_edge.node_a.id, pump.outlet_edge.node_b.id])
		
		# Verify pump node is in the middle
		if pump.inlet_edge.node_b == pump.pump_node and pump.outlet_edge.node_a == pump.pump_node:
			print("  ✓ Pump node correctly positioned between edges")
			passed += 1
		else:
			print("  ✗ Pump node NOT correctly positioned")
			failed += 1
	else:
		print("  ✗ Pump edges NOT created")
		failed += 1
	
	# Test 4: Count total nodes
	total += 1
	print("\n[Test 4] Verify Node Count")
	var node_count = graph.nodes.size()
	print("  Total nodes in graph: %d" % node_count)
	print("  Expected: 3 (source tank + pump + sink tank)")
	
	if node_count == 3:
		print("  ✓ Correct number of nodes")
		passed += 1
	else:
		print("  ✗ Incorrect node count")
		failed += 1
	
	# Test 5: Verify pump is visible
	total += 1
	print("\n[Test 5] Verify Pump Visibility")
	var pump_found = false
	for node_id in graph.nodes:
		var node = graph.nodes[node_id]
		if node.resource_type == "pump":
			pump_found = true
			print("  ✓ Pump node found in graph (ID: %d)" % node.id)
			print("    Display name: '%s'" % node.display_name)
			break
	
	if pump_found:
		passed += 1
	else:
		print("  ✗ Pump node NOT found in graph")
		failed += 1
	
	# Test 6: Set pump power and verify
	total += 1
	print("\n[Test 6] Test Pump Power Control")
	pump.set_power(0.5)
	
	if abs(pump.power - 0.5) < 0.001:
		print("  ✓ Pump power set to 0.5")
		passed += 1
	else:
		print("  ✗ Pump power not set correctly (got %.2f)" % pump.power)
		failed += 1
	
	# Summary
	print("\n" + "=".repeat(50))
	print("Tests Passed: %d/%d" % [passed, total])
	print("Tests Failed: %d/%d" % [failed, total])
	print("=".repeat(50))
	
	if failed == 0:
		print("\n✅ All tests PASSED! Pumps are now visible in solver graph.")
	else:
		print("\n❌ Some tests FAILED.")
	
	quit()
