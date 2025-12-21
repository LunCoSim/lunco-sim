extends SceneTree

# Preload classes
const SimulationManager = preload("res://apps/supply_chain_modeling/simulation/simulation.gd")
const StorageFacility = preload("res://apps/supply_chain_modeling/simulation/facilities/storage.gd")
const Pump = preload("res://apps/supply_chain_modeling/simulation/facilities/pump.gd")
const SolarPowerPlant = preload("res://apps/supply_chain_modeling/simulation/facilities/solar_power_plant.gd")

var simulation: SimulationManager
var source_tank: StorageFacility
var dest_tank: StorageFacility
var pump: Pump
var solar: SolarPowerPlant

func _init():
	print("Starting Oxygen Pump Verification Test...")
	
	simulation = SimulationManager.new()
	root.add_child(simulation)
	
	setup_components()
	test_oxygen_pump()
	
	print("Test completed.")
	quit()

func setup_components():
	# Source Tank (Oxygen)
	source_tank = StorageFacility.new()
	source_tank.name = "SourceTank"
	source_tank.capacity = 100.0
	source_tank.current_amount = 50.0
	source_tank.stored_resource_type = "oxygen"
	simulation.add_node(source_tank)
	
	# Dest Tank (Oxygen)
	dest_tank = StorageFacility.new()
	dest_tank.name = "DestTank"
	dest_tank.capacity = 100.0
	dest_tank.current_amount = 0.0
	dest_tank.stored_resource_type = "oxygen"
	simulation.add_node(dest_tank)
	
	# Pump (Gas)
	pump = Pump.new()
	pump.name = "O2Pump"
	pump.domain = SolverDomain.GAS # Configure for Gas
	simulation.add_node(pump)
	
	# Power Source
	solar = SolarPowerPlant.new()
	solar.name = "Solar"
	simulation.add_node(solar)

func test_oxygen_pump():
	print("\n--- Test: Oxygen Pump Flow ---")
	
	# Connect Source -> Pump -> Dest
	simulation.connect_nodes("SourceTank", 0, "O2Pump", 0) # Inlet
	simulation.connect_nodes("O2Pump", 0, "DestTank", 0) # Outlet
	
	# Connect Power -> Pump
	simulation.connect_nodes("Solar", 0, "O2Pump", 1) # Power In
	
	# Verify Domains
	print("Source Domain: %s" % source_tank.ports["fluid_port"].domain)
	print("Dest Domain: %s" % dest_tank.ports["fluid_port"].domain)
	print("Pump Domain: %s" % pump.ports["inlet"].domain)
	
	if source_tank.ports["fluid_port"].domain != SolverDomain.GAS:
		print("FAIL: Source tank should be Gas")
	if pump.ports["inlet"].domain != SolverDomain.GAS:
		print("FAIL: Pump should be Gas")
	
	# Run simulation
	simulation.paused = false
	
	print("Initial State:")
	print("  Source: %.1f" % source_tank.current_amount)
	print("  Dest: %.1f" % dest_tank.current_amount)
	
	# Simulate 10 minutes
	print("Simulating 10 minutes...")
	for i in range(10):
		simulation._physics_process(60.0)
		
	print("Final State:")
	print("  Source: %.1f" % source_tank.current_amount)
	print("  Dest: %.1f" % dest_tank.current_amount)
	
	if dest_tank.current_amount > 0.0:
		print("PASS: Oxygen moved to destination")
	else:
		print("FAIL: Oxygen did not move")
