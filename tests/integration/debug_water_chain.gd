extends SceneTree

# Preload classes
const SimulationManager = preload("res://apps/supply_chain_modeling/simulation/simulation.gd")
const StorageFacility = preload("res://apps/supply_chain_modeling/simulation/facilities/storage.gd")
const Pump = preload("res://apps/supply_chain_modeling/simulation/facilities/pump.gd")
const SolarPowerPlant = preload("res://apps/supply_chain_modeling/simulation/facilities/solar_power_plant.gd")
const ElectrolyticFactory = preload("res://apps/supply_chain_modeling/simulation/facilities/electrolytic_factory.gd")
const SolverEdge = preload("res://core/systems/solver/solver_edge.gd")

var simulation: SimulationManager
var water_tank: StorageFacility
var pump: Pump
var factory: ElectrolyticFactory
var solar: SolarPowerPlant

func _init():
	print(">>> STARTING FULL SYSTEM DEBUG TEST <<<")
	print("Initializing Simulation...")
	simulation = SimulationManager.new()
	root.add_child(simulation)
	
	create_components()
	connect_components()
	run_simulation_loop()
	
	quit()

func create_components():
	print("\n[1] Creating Components...")
	
	# 1. Water Tank (Source)
	water_tank = StorageFacility.new()
	water_tank.name = "WaterSource"
	water_tank.stored_resource_type = "water"
	water_tank.capacity = 1000.0
	water_tank.current_amount = 1000.0 # START FULL
	simulation.add_node(water_tank)
	print("  + WaterSource: Created (Amount: %.1f / %.1f)" % [water_tank.current_amount, water_tank.capacity])
	
	# 2. Pump
	pump = Pump.new()
	pump.name = "WaterPump"
	pump.domain = SolverDomain.LIQUID
	simulation.add_node(pump)
	print("  + WaterPump: Created")
	
	# 3. Factory
	factory = ElectrolyticFactory.new()
	factory.name = "Factory"
	simulation.add_node(factory)
	print("  + Factory: Created")
	
	# 4. Solar
	solar = SolarPowerPlant.new()
	solar.name = "SolarPlant"
	simulation.add_node(solar)
	print("  + SolarPlant: Created")

func connect_components():
	print("\n[2] Connecting Components...")
	
	# Water Flow: Tank -> Pump -> Factory
	# Tank(Fluid) -> Pump(Inlet)
	var c1 = simulation.connect_nodes("WaterSource", 0, "WaterPump", 0) 
	print("  > Connect Source->Pump: %s" % c1)
	
	# Pump(Outlet) -> Factory(WaterIn)
	var c2 = simulation.connect_nodes("WaterPump", 0, "Factory", 0)
	print("  > Connect Pump->Factory: %s" % c2)
	
	# Power Flow: Solar -> Pump, Solar -> Factory
	var c3 = simulation.connect_nodes("SolarPlant", 0, "WaterPump", 1)
	print("  > Connect Solar->Pump: %s" % c3)
	
	var c4 = simulation.connect_nodes("SolarPlant", 0, "Factory", 1)
	print("  > Connect Solar->Factory: %s" % c4)

func run_simulation_loop():
	print("\n[3] Running Simulation Loop (10 ticks / 10 minutes)...")
	simulation.paused = false
	
	for i in range(1, 11):
		simulation._physics_process(60.0) # Simulate 1 minute (60s) per tick
		
		print("\n--- TICK %d (T=%.0f min) ---" % [i, simulation.simulation_time / 60.0])
		
		# Solar Status
		var v_solar = solar.ports["power_out"].potential
		print("  [SOLAR] Status: %s | Volts: %.2f V" % [solar.status, v_solar])
		
		# Pump Status
		var p_in = pump.ports["inlet"].potential
		var p_out = pump.ports["outlet"].potential
		var p_power = pump.ports["power_in"].potential
		var p_flow = 0.0
		if pump.internal_edges.size() > 0:
			p_flow = pump.internal_edges[0].flow_rate
		print("  [PUMP]  Status: %s | Power: %.2f V | P_In: %.2f | P_Out: %.2f | Flow: %.4f" % 
			[pump.status, p_power, p_in, p_out, p_flow])
			
		# Factory Status
		var f_power = factory.ports["power_in"].potential
		var f_buffer_amt = 0.0
		if factory.ports.has("_internal_buffer"):
			f_buffer_amt = factory.ports["_internal_buffer"].flow_accumulation
		
		var f_intake_flow = 0.0
		var f_intake_G = 0.0
		if factory.internal_edges.size() > 0:
			f_intake_flow = factory.internal_edges[0].flow_rate
			f_intake_G = factory.internal_edges[0].conductance
			
		print("  [FACT]  Status: %s | Power: %.2f V | Buffer: %.4f kg | IntakeFlow: %.4f | Intake G: %.4f" % 
			[factory.status, f_power, f_buffer_amt, f_intake_flow, f_intake_G])

		# Water Source Check
		print("  [TANK]  Amount: %.4f kg" % water_tank.current_amount)
		
		if "Insufficient H2O" in factory.status and water_tank.current_amount > 1.0:
			if p_flow > 0.001:
				print("    !!! MODEL ERROR: Pump is moving water (%.4f) but Factory says Insufficient H2O? Buffer is %.4f" % [p_flow, f_buffer_amt])
