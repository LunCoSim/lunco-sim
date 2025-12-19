extends SceneTree

## Diagnostic to check initial values

func _initialize():
	print("\n=== Initial Values Diagnostic ===\n")
	
	# Create simulation manager
	var sim = SimulationManager.new()
	
	# Create tank with initial mass
	var tank_a = StorageFacility.new()
	tank_a.name = "TankA"
	tank_a.capacity = 100.0
	tank_a.current_amount = 50.0
	print("Tank A current_amount BEFORE add_node: ", tank_a.current_amount)
	
	# Add to simulation
	sim.add_node(tank_a)
	
	print("Tank A current_amount AFTER add_node: ", tank_a.current_amount)
	
	# Check port
	var port = tank_a.get_port("fluid_port")
	if port:
		print("Port flow_accumulation: ", port.flow_accumulation)
		print("Port potential: ", port.potential)
		print("Port capacitance: ", port.capacitance)
		print("Port is_storage: ", port.is_storage)
	
	# Run update_solver_state
	tank_a.update_solver_state()
	
	print("\nAfter update_solver_state:")
	if port:
		print("Port flow_accumulation: ", port.flow_accumulation)
		print("Port potential: ", port.potential)
		print("Port capacitance: ", port.capacitance)
	
	sim.free()
	quit(0)
