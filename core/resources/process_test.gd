extends Node

## Test script for process effector system
## Tests recipes, process effectors, and resource conversion

func _ready():
	print("=== Process System Test ===")
	
	# Wait for registries to initialize
	await get_tree().process_frame
	await get_tree().process_frame
	
	# Test 1: Check recipes loaded
	print("\n[Test 1] Recipe Registry:")
	var all_recipes = LCRecipeRegistry.get_all_recipes()
	print("  Loaded ", all_recipes.size(), " recipes")
	for recipe in all_recipes:
		print("    - ", recipe.recipe_name, " (", recipe.recipe_id, ")")
		print("      Duration: ", recipe.duration, "s, Power: ", recipe.power_required, "W")
	
	# Test 2: Create ISRU setup
	print("\n[Test 2] Create ISRU Facility:")
	
	# Input tanks
	var regolith_tank = LCResourceTankEffector.new()
	regolith_tank.resource_id = "regolith"
	regolith_tank.capacity = 100.0
	regolith_tank.set_amount(50.0)  # Start with 50kg
	add_child(regolith_tank)
	
	var h2_tank = LCResourceTankEffector.new()
	h2_tank.resource_id = "hydrogen"
	h2_tank.capacity = 10.0
	h2_tank.set_amount(5.0)  # Start with 5kg
	add_child(h2_tank)
	
	# Output tank
	var o2_tank = LCResourceTankEffector.new()
	o2_tank.resource_id = "oxygen"
	o2_tank.capacity = 50.0
	add_child(o2_tank)
	
	await get_tree().process_frame
	
	print("  Created tanks:")
	print("    Regolith: ", regolith_tank.get_amount(), "/", regolith_tank.capacity, " kg")
	print("    Hydrogen: ", h2_tank.get_amount(), "/", h2_tank.capacity, " kg")
	print("    Oxygen: ", o2_tank.get_amount(), "/", o2_tank.capacity, " kg")
	
	# Test 3: Create ISRU processor
	print("\n[Test 3] Create ISRU Processor:")
	var isru = LCISRUProcessor.new()
	add_child(isru)
	await get_tree().process_frame
	
	# Connect tanks
	isru.connect_tank(regolith_tank)
	isru.connect_tank(h2_tank)
	isru.connect_tank(o2_tank)
	
	print("  Processor created and tanks connected")
	print("  Recipe: ", isru.recipe.recipe_name if isru.recipe else "None")
	print("  Status: ", isru.get_status())
	
	# Test 4: Run process for a few cycles
	print("\n[Test 4] Run ISRU Process:")
	isru.start_process()
	
	for i in range(3):
		print("\n  Cycle ", i + 1, ":")
		print("    Status: ", isru.get_status())
		print("    Progress: ", isru.get_cycle_progress(), "%")
		
		# Wait for cycle to complete
		await get_tree().create_timer(isru.recipe.duration + 0.5).timeout
		
		print("    After cycle:")
		print("      Regolith: ", regolith_tank.get_amount(), " kg")
		print("      Hydrogen: ", h2_tank.get_amount(), " kg")
		print("      Oxygen: ", o2_tank.get_amount(), " kg")
		print("      Cycles completed: ", isru.total_cycles_completed)
	
	isru.stop_process()
	
	# Test 5: Electrolyzer
	print("\n[Test 5] Water Electrolysis:")
	
	var water_tank = LCResourceTankEffector.new()
	water_tank.resource_id = "water"
	water_tank.capacity = 50.0
	water_tank.set_amount(20.0)
	add_child(water_tank)
	
	var h2_output = LCResourceTankEffector.new()
	h2_output.resource_id = "hydrogen"
	h2_output.capacity = 10.0
	add_child(h2_output)
	
	var o2_output = LCResourceTankEffector.new()
	o2_output.resource_id = "oxygen"
	o2_output.capacity = 50.0
	add_child(o2_output)
	
	await get_tree().process_frame
	
	var electrolyzer = LCElectrolyzer.new()
	add_child(electrolyzer)
	await get_tree().process_frame
	
	electrolyzer.connect_tank(water_tank)
	electrolyzer.connect_tank(h2_output)
	electrolyzer.connect_tank(o2_output)
	
	print("  Before: Water=", water_tank.get_amount(), " H2=", h2_output.get_amount(), " O2=", o2_output.get_amount())
	
	electrolyzer.start_process()
	await get_tree().create_timer(electrolyzer.recipe.duration + 0.5).timeout
	
	print("  After:  Water=", water_tank.get_amount(), " H2=", h2_output.get_amount(), " O2=", o2_output.get_amount())
	
	# Test 6: Fuel Cell
	print("\n[Test 6] Fuel Cell Power:")
	
	var fuel_cell = LCFuelCell.new()
	add_child(fuel_cell)
	await get_tree().process_frame
	
	# Use outputs from electrolyzer
	fuel_cell.connect_tank(h2_output)
	fuel_cell.connect_tank(o2_output)
	
	var power_tank = LCResourceTankEffector.new()
	power_tank.resource_id = "electrical_power"
	power_tank.capacity = 100.0
	add_child(power_tank)
	await get_tree().process_frame
	
	fuel_cell.connect_tank(power_tank)
	
	print("  Before: H2=", h2_output.get_amount(), " O2=", o2_output.get_amount(), " Power=", power_tank.get_amount())
	
	fuel_cell.start_process()
	await get_tree().create_timer(fuel_cell.recipe.duration + 0.5).timeout
	
	print("  After:  H2=", h2_output.get_amount(), " O2=", o2_output.get_amount(), " Power=", power_tank.get_amount())
	print("  Power output: ", fuel_cell.get_power_output(), " W")
	
	print("\n=== All Process Tests Complete ===")
