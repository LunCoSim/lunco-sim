extends SceneTree

## Simple diagnostic to check if solver connections are working

func _initialize():
	print("\n=== Solver Connection Diagnostic ===\n")
	
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
	
	print("Tank A ports: ", tank_a.ports.keys())
	print("Tank B ports: ", tank_b.ports.keys())
	print("Tank A fluid_port: ", tank_a.get_port("fluid_port"))
	print("Tank B fluid_port: ", tank_b.get_port("fluid_port"))
	
	# Check solver graph
	print("\nSolver graph nodes: ", sim.solver_graph.nodes.size())
	print("Solver graph edges: ", sim.solver_graph.edges.size())
	
	# Try to connect
	print("\nConnecting tanks...")
	var result = sim.connect_nodes("TankA", 0, "TankB", 0)
	print("Connection result: ", result)
	print("Connections: ", sim.connections.size())
	
	print("\nSolver graph nodes after connect: ", sim.solver_graph.nodes.size())
	print("Solver graph edges after connect: ", sim.solver_graph.edges.size())
	
	# Check if edge was created
	if sim.connections.size() > 0:
		var conn = sim.connections[0]
		print("Connection has solver_edge: ", conn.has("solver_edge"))
		if conn.has("solver_edge"):
			print("Solver edge: ", conn["solver_edge"])
	
	sim.free()
	quit(0)
