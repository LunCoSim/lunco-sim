class_name SolarPowerPlant
extends SolverSimulationNode

const SolverDomain = preload("res://core/systems/solver/solver_domain.gd")

@export var power_output: float = 1000.0  # kW (Peak)
@export var solar_irradiance: float = 1.0  # kW/m²
@export var panel_area: float = 100.0  # m²

@export var current_output: float = 0.0

func _init():
	facility_type = "solar_plant"
	description = "Generates electrical power from sunlight"

func _create_ports():
	# Output port (Electrical Source)
	ports["power_out"] = solver_graph.add_node(0.0, false, SolverDomain.ELECTRICAL)

func _create_internal_edges():
	pass

func update_solver_state():
	if not ports.has("power_out"):
		return
		
	# Calculate available power
	# P = Area * Irradiance * Efficiency
	var max_power_kw = panel_area * solar_irradiance * efficiency
	current_output = max_power_kw
	
	# Convert to Current Source (Amps)
	# We need to know the grid voltage to set the current source correctly?
	# Or we can model it as a Voltage Source?
	# Solar panels are typically Current Sources.
	# Let's assume a nominal voltage for calculation if the grid is down, 
	# but really the solver should handle the voltage.
	# For a current source, I_source = P / V_grid.
	# If V_grid is 0 (short circuit), I is max short circuit current.
	
	var port = ports["power_out"]
	var grid_voltage = port.potential
	
	if grid_voltage < 1.0:
		grid_voltage = 120.0 # Nominal voltage to start up
		
	var current_amps = (max_power_kw * 1000.0) / grid_voltage
	
	# Set flow source (Positive = entering the node = supplying power)
	port.flow_source = current_amps
	
	status = "Generating: %.1f kW" % current_output

func update_from_solver():
	pass

func set_solar_irradiance(new_irradiance: float) -> void:
	solar_irradiance = new_irradiance

func set_panel_area(new_area: float) -> void:
	panel_area = new_area

func save_state() -> Dictionary:
	var state = super.save_state()
	state["solar_irradiance"] = solar_irradiance
	state["panel_area"] = panel_area
	return state

func load_state(state: Dictionary) -> void:
	super.load_state(state)
	solar_irradiance = state.get("solar_irradiance", solar_irradiance)
	panel_area = state.get("panel_area", panel_area)
