extends SceneTree

# Preload solver classes
const LCSolverGraph = preload("res://core/systems/solver/solver_graph.gd")
const LCSolverNode = preload("res://core/systems/solver/solver_node.gd")
const LCSolverEdge = preload("res://core/systems/solver/solver_edge.gd")

# Preload component classes
const LCResourceComponent = preload("res://core/systems/components/resource_component.gd")
const LCTankComponent = preload("res://core/systems/components/tank_component.gd")
const LCPipeComponent = preload("res://core/systems/components/pipe_component.gd")
const LCPumpComponent = preload("res://core/systems/components/pump_component.gd")

func _init():
	print("Starting Dynamic Solver Test...")
	test_simple_flow()
	test_pump_flow()
	print("All tests completed.")
	quit()

func test_simple_flow():
	print("\n--- Test 1: Simple Gravity Flow (Tank A -> Tank B) ---")
	var graph = LCSolverGraph.new()
	
	# Create Tank A (High)
	var tank_a = LCTankComponent.new(graph, 1.0, 2.0) # 2m tall
	tank_a.set_initial_mass(1000.0) # Full
	
	# Create Tank B (Low)
	var tank_b = LCTankComponent.new(graph, 1.0, 2.0)
	tank_b.set_initial_mass(0.0) # Empty
	
	# Create Pipe
	var pipe = LCPipeComponent.new(graph)
	pipe.connect_nodes(tank_a.get_port("port"), tank_b.get_port("port"))
	
	print("Initial State: Tank A = %.1f kg, Tank B = %.1f kg" % [tank_a.mass, tank_b.mass])
	
	# Simulate 10 seconds
	for i in range(100):
		var delta = 0.1
		tank_a.update(delta)
		tank_b.update(delta)
		pipe.update(delta)
		graph.solve(delta)
		
		if i % 20 == 0:
			print("T=%.1fs: A=%.1f kg (P=%.0f), B=%.1f kg (P=%.0f), Flow=%.2f" % [
				i * delta, 
				tank_a.mass, tank_a.get_port("port").pressure,
				tank_b.mass, tank_b.get_port("port").pressure,
				pipe.edge.flow_rate
			])
			
	print("Final State: Tank A = %.1f kg, Tank B = %.1f kg" % [tank_a.mass, tank_b.mass])
	
	if abs(tank_a.mass - tank_b.mass) < 10.0:
		print("PASS: Tanks equalized.")
	else:
		print("FAIL: Tanks did not equalize.")

func test_pump_flow():
	print("\n--- Test 2: Pump Flow (Tank B -> Tank A against gravity) ---")
	var graph = LCSolverGraph.new()
	
	# Tank A (High, Full)
	var tank_a = LCTankComponent.new(graph, 1.0, 2.0)
	tank_a.set_initial_mass(500.0)
	
	# Tank B (Low, Full)
	var tank_b = LCTankComponent.new(graph, 1.0, 2.0)
	tank_b.set_initial_mass(500.0)
	
	# Pump pushing B -> A
	var pump = LCPumpComponent.new(graph, 50000.0) # 50kPa head
	pump.connect_nodes(tank_b.get_port("port"), tank_a.get_port("port"))
	pump.set_power(1.0)
	
	print("Initial: A=%.1f, B=%.1f" % [tank_a.mass, tank_b.mass])
	
	# Simulate
	for i in range(50):
		var delta = 0.1
		tank_a.update(delta)
		tank_b.update(delta)
		pump.update(delta)
		graph.solve(delta)
	
	print("Final: A=%.1f, B=%.1f" % [tank_a.mass, tank_b.mass])
	
	if tank_a.mass > tank_b.mass:
		print("PASS: Pump moved mass to Tank A.")
	else:
		print("FAIL: Pump failed to move mass.")
