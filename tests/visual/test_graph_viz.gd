extends Control

@onready var graph_view = $GraphView
var sim: SimulationManager

func _ready():
	# Create simulation
	sim = SimulationManager.new()
	add_child(sim)
	
	# Create some components
	var tank_a = StorageFacility.new()
	tank_a.name = "TankA"
	tank_a.capacity = 100.0
	tank_a.current_amount = 80.0
	tank_a.stored_resource_type = "water"
	sim.add_node(tank_a)
	
	var tank_b = StorageFacility.new()
	tank_b.name = "TankB"
	tank_b.capacity = 100.0
	tank_b.current_amount = 20.0
	tank_b.stored_resource_type = "water"
	sim.add_node(tank_b)
	
	var pump = Pump.new()
	pump.name = "Pump"
	pump.pump_rate = 20.0
	pump.power_available = 100.0
	sim.add_node(pump)
	
	# Connect
	sim.connect_nodes("TankA", 0, "Pump", 0)
	sim.connect_nodes("Pump", 0, "TankB", 0)
	
	# Resume simulation
	sim.resume_simulation()
	
	# Visualize
	graph_view.load_from_solver_graph(sim.solver_graph)
	
	print("GraphView children: ", graph_view.get_child_count())
	var node_count = 0
	for child in graph_view.get_children():
		if child is GraphNode:
			node_count += 1
			print("Created node: ", child.name, " Title: ", child.title)
	
	if node_count >= 4: # TankA, TankB, Pump Inlet, Pump Outlet
		print("✅ Visualization created successfully")
	else:
		print("❌ Visualization failed, expected at least 4 nodes, got ", node_count)
	
	get_tree().quit()

func _process(delta):
	pass
