class_name ObjectFactory
extends BaseFacility

# Input/output rates
@export var o2_input_rate: float = 1.0  # units/minute
@export var h2_input_rate: float = 2.0  # units/minute
@export var power_input_rate: float = 100.0  # kW
@export var h2o_output_rate: float = 1.0  # units/minute
@export var power_consumption: float = 100.0  # kW

# Current resource amounts
@export var o2_stored: float = 0.0
@export var h2_stored: float = 0.0
@export var power_available: float = 0.0

func _init():
	pass

func _physics_process(delta: float) -> void:
	if not is_physics_processing():
		return
		
	# Get connected nodes through the simulation manager
	var simulation = get_parent()
	if not simulation:
		status = "No Simulation"
		return
		
	var o2_source = null
	var h2_source = null
	var power_source = null
	var h2o_storage = null
	
	# Find our connections from the simulation's connections array
	for connection in simulation.connections:
		if connection["to_node"] == name:
			var source_node = simulation.get_node(NodePath(connection["from_node"]))
			match connection["to_port"]:
				0: o2_source = source_node
				1: h2_source = source_node
				2: power_source = source_node
		elif connection["from_node"] == name and connection["from_port"] == 0:
			h2o_storage = simulation.get_node(NodePath(connection["to_node"]))
	
	# Check connections and update status
	if not o2_source:
		status = "O2 Not Connected"
		return
	elif not h2_source:
		status = "H2 Not Connected"
		return
	elif not power_source:
		status = "Power Not Connected"
		return
	elif not h2o_storage:
		status = "H2O Not Connected"
		return
	
	# Calculate time step
	var minutes = delta * 60  # Convert seconds to minutes
	
	# Check power availability
	power_available = power_source.power_output * power_source.efficiency if "power_output" in power_source else 0.0
	if power_available < power_input_rate:
		status = "Insufficient Power"
		return
		
	# Calculate maximum possible production based on available output space
	var max_h2o_production = h2o_output_rate * efficiency * minutes
	var available_output_space = h2o_storage.available_space() if "available_space" in h2o_storage else 0.0
	max_h2o_production = min(max_h2o_production, available_output_space)
	
	if max_h2o_production <= 0:
		status = "Output Storage Full"
		return
		
	# Calculate required inputs for the possible production
	var o2_required = (o2_input_rate * minutes) * (max_h2o_production / (h2o_output_rate * efficiency * minutes))
	var h2_required = (h2_input_rate * minutes) * (max_h2o_production / (h2o_output_rate * efficiency * minutes))
	
	# Check resource availability without consuming
	if not "remove_resource" in o2_source or not "remove_resource" in h2_source:
		status = "Invalid Input Connections"
		return
		
	# Try to get resources atomically
	var o2_available = o2_source.remove_resource(o2_required)
	var h2_available = h2_source.remove_resource(h2_required)
	
	# If we can't get all resources, return what we took
	if o2_available < o2_required or h2_available < h2_required:
		if o2_available > 0:
			o2_source.add_resource(o2_available)
		if h2_available > 0:
			h2_source.add_resource(h2_available)
		status = "Insufficient Resources"
		return
		
	# At this point we have all resources, produce output
	var h2o_produced = max_h2o_production
	var added = h2o_storage.add_resource(h2o_produced)
	
	# If we couldn't add all output (shouldn't happen due to earlier check), return inputs
	if added < h2o_produced:
		var return_ratio = (h2o_produced - added) / h2o_produced
		o2_source.add_resource(o2_available * return_ratio)
		h2_source.add_resource(h2_available * return_ratio)
	
	status = "Running"
