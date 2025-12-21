extends SceneTree

# Preload classes
const SimulationManager = preload("res://apps/supply_chain_modeling/simulation/simulation.gd")
const SolarPowerPlant = preload("res://apps/supply_chain_modeling/simulation/facilities/solar_power_plant.gd")
const StorageFacility = preload("res://apps/supply_chain_modeling/simulation/facilities/storage.gd")
const RegolithReductionReactor = preload("res://apps/supply_chain_modeling/simulation/facilities/regolith_reduction_reactor.gd")
const WaterCollectionSystem = preload("res://apps/supply_chain_modeling/simulation/facilities/water_collection_system.gd")
const ElectrolyticFactory = preload("res://apps/supply_chain_modeling/simulation/facilities/electrolytic_factory.gd")

var simulation: SimulationManager
var solar_panel: SolarPowerPlant
var battery: StorageFacility
var regolith_silo: StorageFacility
var h2_tank: StorageFacility
var reactor: RegolithReductionReactor
var water_collection: WaterCollectionSystem
var water_tank: StorageFacility
var electrolyzer: ElectrolyticFactory
var o2_tank: StorageFacility

func _init():
	print("Starting Oxygen Extraction Integration Test...")
	
	simulation = SimulationManager.new()
	root.add_child(simulation)
	
	setup_components()
	test_oxygen_production_chain()
	
	print("Test completed.")
	quit()

func setup_components():
	# Create components
	solar_panel = SolarPowerPlant.new()
	solar_panel.name = "SolarPanel"
	simulation.add_node(solar_panel)
	
	battery = StorageFacility.new()
	battery.name = "Battery"
	battery.capacity = 10000.0 
	# Note: StorageFacility uses Liquid by default. 
	# We are mocking battery behavior or just connecting power directly for now.
	# Ideally we'd have a BatteryFacility.
	# For this test, we'll skip the battery node in the electrical path 
	# and connect Solar directly to loads, assuming Solar has internal regulation.
	
	regolith_silo = StorageFacility.new()
	regolith_silo.name = "RegolithSilo"
	regolith_silo.capacity = 1000.0
	regolith_silo.current_amount = 500.0
	regolith_silo.stored_resource_type = "regolith"
	simulation.add_node(regolith_silo)
	regolith_silo.ports["fluid_port"].domain = SolverDomain.SOLID
	
	h2_tank = StorageFacility.new()
	h2_tank.name = "H2Tank"
	h2_tank.capacity = 100.0
	h2_tank.current_amount = 50.0
	h2_tank.stored_resource_type = "hydrogen"
	simulation.add_node(h2_tank)
	h2_tank.ports["fluid_port"].domain = SolverDomain.GAS
	
	reactor = RegolithReductionReactor.new()
	reactor.name = "Reactor"
	simulation.add_node(reactor)
	
	water_collection = WaterCollectionSystem.new()
	water_collection.name = "WaterCollection"
	simulation.add_node(water_collection)
	
	water_tank = StorageFacility.new()
	water_tank.name = "WaterTank"
	water_tank.capacity = 100.0
	water_tank.current_amount = 0.0
	water_tank.stored_resource_type = "water"
	simulation.add_node(water_tank)
	# Water is Liquid, so default is fine
	
	electrolyzer = ElectrolyticFactory.new()
	electrolyzer.name = "Electrolyzer"
	simulation.add_node(electrolyzer)
	
	o2_tank = StorageFacility.new()
	o2_tank.name = "O2Tank"
	o2_tank.capacity = 100.0
	o2_tank.current_amount = 0.0
	o2_tank.stored_resource_type = "oxygen"
	simulation.add_node(o2_tank)
	o2_tank.ports["fluid_port"].domain = SolverDomain.GAS

func test_oxygen_production_chain():
	print("\n--- Test: Oxygen Production Chain ---")
	
	# Connect Solar -> Reactor (Power)
	simulation.connect_nodes("SolarPanel", 0, "Reactor", 2)
	
	# Connect Regolith Silo -> Reactor
	simulation.connect_nodes("RegolithSilo", 0, "Reactor", 0)
	
	# Connect H2 Tank -> Reactor
	simulation.connect_nodes("H2Tank", 0, "Reactor", 1)
	
	# Connect Reactor -> Water Collection (Steam/Gas)
	simulation.connect_nodes("Reactor", 0, "WaterCollection", 0)
	
	# Connect Solar -> Water Collection (Power)
	simulation.connect_nodes("SolarPanel", 0, "WaterCollection", 1)
	
	# Connect Water Collection -> Water Tank (Liquid)
	simulation.connect_nodes("WaterCollection", 0, "WaterTank", 0)
	
	# Connect Water Tank -> Electrolyzer
	simulation.connect_nodes("WaterTank", 0, "Electrolyzer", 0)
	
	# Connect Solar -> Electrolyzer (Power)
	simulation.connect_nodes("SolarPanel", 0, "Electrolyzer", 1)
	
	# Connect Electrolyzer -> O2 Tank
	simulation.connect_nodes("Electrolyzer", 1, "O2Tank", 0)
	
	# Connect Electrolyzer -> H2 Tank (Recycle)
	simulation.connect_nodes("Electrolyzer", 0, "H2Tank", 0)
	
	# Run simulation
	simulation.paused = false
	
	print("Initial State:")
	print("  Regolith: %.1f" % regolith_silo.current_amount)
	print("  Water: %.1f" % water_tank.current_amount)
	print("  Oxygen: %.1f" % o2_tank.current_amount)
	
	# Simulate 1 hour (60 steps of 1 minute)
	print("Simulating 1 hour...")
	for i in range(60):
		simulation._physics_process(60.0)
		
	print("Final State:")
	print("  Regolith: %.1f" % regolith_silo.current_amount)
	print("  Water: %.1f" % water_tank.current_amount)
	print("  Oxygen: %.1f" % o2_tank.current_amount)
	
	# Verify
	if regolith_silo.current_amount < 500.0:
		print("PASS: Regolith consumed")
	else:
		print("FAIL: Regolith not consumed")
		
	if water_tank.current_amount > 0.0:
		print("PASS: Water produced")
	else:
		print("FAIL: Water not produced")
		
	if o2_tank.current_amount > 0.0:
		print("PASS: Oxygen produced")
	else:
		print("FAIL: Oxygen not produced")
