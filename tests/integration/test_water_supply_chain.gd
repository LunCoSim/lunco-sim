extends SceneTree

const SimulationManager = preload("res://apps/supply_chain_modeling/simulation/simulation.gd")
const StorageFacility = preload("res://apps/supply_chain_modeling/simulation/facilities/storage.gd")
const Pump = preload("res://apps/supply_chain_modeling/simulation/facilities/pump.gd")
const SolarPowerPlant = preload("res://apps/supply_chain_modeling/simulation/facilities/solar_power_plant.gd")
const ElectrolyticFactory = preload("res://apps/supply_chain_modeling/simulation/facilities/electrolytic_factory.gd")
const SolverDomain = preload("res://core/systems/solver/solver_domain.gd")

var simulation: SimulationManager
var water_tank: StorageFacility
var pump: Pump
var factory: ElectrolyticFactory
var solar: SolarPowerPlant

func _init():
	print("--- Debugging Water Supply Chain ---")
	simulation = SimulationManager.new()
	root.add_child(simulation)
	
	# 1. Setup Components
	water_tank = StorageFacility.new()
	water_tank.name = "WaterTank"
	water_tank.stored_resource_type = "water"
	water_tank.current_amount = 1000.0
	water_tank.capacity = 1000.0
	simulation.add_node(water_tank)
	
	pump = Pump.new()
	pump.name = "WaterPump"
	pump.domain = SolverDomain.LIQUID
	simulation.add_node(pump)
	
	factory = ElectrolyticFactory.new()
	factory.name = "Factory"
	simulation.add_node(factory)
	
	solar = SolarPowerPlant.new()
	solar.name = "Solar"
	simulation.add_node(solar)
	
	# 2. Connect
	print("Connecting nodes...")
	# Water -> Pump
	simulation.connect_nodes("WaterTank", 0, "WaterPump", 0)
	# Pump -> Factory (Water In is port 0)
	simulation.connect_nodes("WaterPump", 0, "Factory", 0)
	
	# Solar -> Pump (Power In is port 1)
	simulation.connect_nodes("Solar", 0, "WaterPump", 1)
	# Solar -> Factory (Power In is port 1)
	simulation.connect_nodes("Solar", 0, "Factory", 1)
	
	simulation.paused = false
	
	# 3. Simulate and Print
	print("\nStarting Simulation Loop...")
	for i in range(5):
		simulation._physics_process(60.0) # 1 minute per tick
		print_status(i + 1)
		
	quit()

func print_status(step):
	print("\n--- Step %d ---" % step)
	print("Solar: %s | Potential: %.2f" % [solar.status, solar.ports["power_out"].potential])
	
	var p_in = pump.ports["inlet"].potential
	var p_out = pump.ports["outlet"].potential
	var p_flow = 0.0
	if pump.internal_edges.size() > 0:
		p_flow = pump.internal_edges[0].flow_rate
	print("Pump: %s | P_In: %.2f | P_Out: %.2f | Flow: %.4f | PowerV: %.2f" % 
		[pump.status, p_in, p_out, p_flow, pump.ports["power_in"].potential])
		
	var f_buffer = 0.0
	if factory.ports.has("_internal_buffer"):
		f_buffer = factory.ports["_internal_buffer"].flow_accumulation
	var f_power = factory.ports["power_in"].potential
	
	# Internal edge debug
	var intake_flow = 0.0
	if factory.internal_edges.size() > 0:
		intake_flow = factory.internal_edges[0].flow_rate
		
	print("Factory: %s | Buffer: %.4f | IntakeFlow: %.4f | PowerV: %.2f" % 
		[factory.status, f_buffer, intake_flow, f_power])
