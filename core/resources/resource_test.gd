extends Node

## Test script for resource system
## Run this scene to verify resource registry and tanks work

func _ready():
	print("=== Resource System Test ===")
	
	# Wait for registry to initialize
	await get_tree().process_frame
	
	# Test 1: Check if resources are loaded
	print("\n[Test 1] Resource Registry:")
	var all_resources = LCResourceRegistry.get_all_resources()
	print("  Loaded ", all_resources.size(), " resources")
	for res in all_resources:
		print("    - ", res.display_name, " (", res.resource_id, ") - ", res.category)
	
	# Test 2: Get specific resource
	print("\n[Test 2] Get Oxygen:")
	var oxygen = LCResourceRegistry.get_resource("oxygen")
	if oxygen:
		print("  ✓ Found: ", oxygen.display_name)
		print("    Density: ", oxygen.density, " kg/m³")
		print("    Tags: ", oxygen.tags)
	else:
		print("  ✗ Oxygen not found!")
	
	# Test 3: Create resource tank
	print("\n[Test 3] Create Oxygen Tank:")
	var tank = LCResourceTankEffector.new()
	tank.resource_id = "oxygen"
	tank.capacity = 50.0
	tank.tank_dry_mass = 5.0
	add_child(tank)
	await get_tree().process_frame
	
	print("  Tank created for: ", tank.get_resource_name())
	print("  Capacity: ", tank.capacity, " kg")
	print("  Current mass: ", tank.mass, " kg")
	
	# Test 4: Add resource
	print("\n[Test 4] Add 25kg Oxygen:")
	tank.add_resource(25.0)
	print("  Amount: ", tank.get_amount(), " kg")
	print("  Fill: ", tank.get_fill_percentage(), "%")
	print("  Total mass: ", tank.mass, " kg")
	
	# Test 5: Create second tank and transfer
	print("\n[Test 5] Transfer to Second Tank:")
	var tank2 = LCResourceTankEffector.new()
	tank2.resource_id = "oxygen"
	tank2.capacity = 30.0
	tank2.tank_dry_mass = 3.0
	add_child(tank2)
	await get_tree().process_frame
	
	var transferred = tank.transfer_to(tank2, 10.0)
	print("  Transferred: ", transferred, " kg")
	print("  Tank 1: ", tank.get_amount(), " kg (", tank.get_fill_percentage(), "%)")
	print("  Tank 2: ", tank2.get_amount(), " kg (", tank2.get_fill_percentage(), "%)")
	
	# Test 6: Get resources by category
	print("\n[Test 6] Get Gas Resources:")
	var gases = LCResourceRegistry.get_resources_by_category("gas")
	print("  Found ", gases.size(), " gases:")
	for gas in gases:
		print("    - ", gas.display_name)
	
	# Test 7: Get resources by tag
	print("\n[Test 7] Get Fuel Resources:")
	var fuels = LCResourceRegistry.get_resources_by_tag("fuel")
	print("  Found ", fuels.size(), " fuels:")
	for fuel in fuels:
		print("    - ", fuel.display_name)
	
	print("\n=== All Tests Complete ===")
