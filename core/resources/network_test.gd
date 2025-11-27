extends Node

## Test script for resource network system
## Tests automatic resource flow between tanks and processors

func _ready():
	print("=== Resource Network Test ===")
	
	# Wait for registries
	await get_tree().process_frame
	await get_tree().process_frame
	
	# Test 1: Create a simple network
	print("\n[Test 1] Simple Tank Network:")
	
	var network = LCResourceNetwork.new()
	add_child(network)
	
	# Create two oxygen tanks
	var tank1 = LCResourceTankEffector.new()
	tank1.resource_id = "oxygen"
	tank1.capacity = 50.0
	tank1.set_amount(40.0)  # Full tank
	tank1.name = "OxygenTank1"
	add_child(tank1)
	
	var tank2 = LCResourceTankEffector.new()
	tank2.resource_id = "oxygen"
	tank2.capacity = 50.0
	tank2.set_amount(10.0)  # Empty tank
	tank2.name = "OxygenTank2"
	add_child(tank2)
	
	await get_tree().process_frame
	
	# Add to network
	var node1 = network.add_node(tank1)
	var node2 = network.add_node(tank2)
	
	# Connect them
	network.connect_nodes(node1, node2, 5.0)  # 5 kg/s flow rate
	
	print("  Tank 1: ", tank1.get_amount(), " kg")
	print("  Tank 2: ", tank2.get_amount(), " kg")
	print("  Network nodes: ", network.nodes.size())
	print("  Network connections: ", network.connections.size())
	
	# Wait for flow
	print("\n  Waiting for flow...")
	await get_tree().create_timer(2.0).timeout
	
	print("  After 2 seconds:")
	print("  Tank 1: ", tank1.get_amount(), " kg")
	print("  Tank 2: ", tank2.get_amount(), " kg")
	
	# Test 2: ISRU facility with network
	print("\n[Test 2] ISRU Facility with Auto-Flow:")
	
	var facility_network = LCResourceNetwork.new()
	add_child(facility_network)
	
	# Input tanks
	var regolith = LCResourceTankEffector.new()
	regolith.resource_id = "regolith"
	regolith.capacity = 100.0
	regolith.set_amount(80.0)
	regolith.name = "RegolithTank"
	add_child(regolith)
	
	var hydrogen_in = LCResourceTankEffector.new()
	hydrogen_in.resource_id = "hydrogen"
	hydrogen_in.capacity = 20.0
	hydrogen_in.set_amount(15.0)
	hydrogen_in.name = "HydrogenInput"
	add_child(hydrogen_in)
	
	# Output tank
	var oxygen_out = LCResourceTankEffector.new()
	oxygen_out.resource_id = "oxygen"
	oxygen_out.capacity = 100.0
	oxygen_out.name = "OxygenOutput"
	add_child(oxygen_out)
	
	# Hydrogen recycle tank
	var hydrogen_out = LCResourceTankEffector.new()
	hydrogen_out.resource_id = "hydrogen"
	hydrogen_out.capacity = 20.0
	hydrogen_out.name = "HydrogenRecycle"
	add_child(hydrogen_out)
	
	await get_tree().process_frame
	
	# ISRU processor
	var isru = LCISRUProcessor.new()
	isru.name = "ISRUProcessor"
	add_child(isru)
	await get_tree().process_frame
	
	# Connect tanks to processor
	isru.connect_tank(regolith)
	isru.connect_tank(hydrogen_in)
	isru.connect_tank(oxygen_out)
	isru.connect_tank(hydrogen_out)
	
	# Add to network and auto-connect
	facility_network.add_node(regolith)
	facility_network.add_node(hydrogen_in)
	facility_network.add_node(oxygen_out)
	facility_network.add_node(hydrogen_out)
	facility_network.add_process(isru)
	facility_network.auto_connect()
	
	print("  Facility network created:")
	print("    Nodes: ", facility_network.nodes.size())
	print("    Connections: ", facility_network.connections.size())
	print("  Initial state:")
	print("    Regolith: ", regolith.get_amount(), " kg")
	print("    H2 Input: ", hydrogen_in.get_amount(), " kg")
	print("    O2 Output: ", oxygen_out.get_amount(), " kg")
	print("    H2 Recycle: ", hydrogen_out.get_amount(), " kg")
	
	# Start ISRU
	isru.start_process()
	print("\n  ISRU started, running for 3 cycles...")
	
	for i in range(3):
		await get_tree().create_timer(isru.recipe.duration + 0.5).timeout
		print("\n  After cycle ", i + 1, ":")
		print("    Regolith: ", regolith.get_amount(), " kg")
		print("    H2 Input: ", hydrogen_in.get_amount(), " kg")
		print("    O2 Output: ", oxygen_out.get_amount(), " kg")
		print("    H2 Recycle: ", hydrogen_out.get_amount(), " kg")
		print("    Total O2 in network: ", facility_network.get_total_resource("oxygen"), " kg")
	
	# Test 3: Vehicle with integrated network
	print("\n[Test 3] Vehicle with Resource Network:")
	
	var vehicle = LCVehicle.new()
	vehicle.debug_effectors = true
	vehicle.name = "TestVehicle"
	add_child(vehicle)
	
	# Add tanks to vehicle
	var v_o2 = LCResourceTankEffector.new()
	v_o2.resource_id = "oxygen"
	v_o2.capacity = 30.0
	v_o2.set_amount(10.0)
	v_o2.name = "VehicleOxygen"
	vehicle.add_child(v_o2)
	
	var v_h2 = LCResourceTankEffector.new()
	v_h2.resource_id = "hydrogen"
	v_h2.capacity = 10.0
	v_h2.set_amount(5.0)
	v_h2.name = "VehicleHydrogen"
	vehicle.add_child(v_h2)
	
	var v_power = LCResourceTankEffector.new()
	v_power.resource_id = "electrical_power"
	v_power.capacity = 100.0
	v_power.name = "VehiclePower"
	vehicle.add_child(v_power)
	
	# Add fuel cell
	var fuel_cell = LCFuelCell.new()
	fuel_cell.name = "FuelCell"
	vehicle.add_child(fuel_cell)
	
	await get_tree().process_frame
	await get_tree().process_frame
	
	# Connect fuel cell
	fuel_cell.connect_tank(v_h2)
	fuel_cell.connect_tank(v_o2)
	fuel_cell.connect_tank(v_power)
	
	print("  Vehicle network:")
	if vehicle.resource_network:
		print("    Nodes: ", vehicle.resource_network.nodes.size())
		print("    Connections: ", vehicle.resource_network.connections.size())
	
	print("  Initial state:")
	print("    O2: ", v_o2.get_amount(), " kg")
	print("    H2: ", v_h2.get_amount(), " kg")
	print("    Power: ", v_power.get_amount(), " kWh")
	
	# Start fuel cell
	fuel_cell.start_process()
	print("\n  Fuel cell started, running for 3 cycles...")
	
	for i in range(3):
		await get_tree().create_timer(fuel_cell.recipe.duration + 0.5).timeout
		print("\n  After cycle ", i + 1, ":")
		print("    O2: ", v_o2.get_amount(), " kg")
		print("    H2: ", v_h2.get_amount(), " kg")
		print("    Power: ", v_power.get_amount(), " kWh")
		print("    Power output: ", fuel_cell.get_power_output(), " W")
	
	print("\n=== All Network Tests Complete ===")
