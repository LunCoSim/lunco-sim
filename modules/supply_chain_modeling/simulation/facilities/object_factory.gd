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
	elif not h2_source:
		status = "H2 Not Connected"
	elif not power_source:
		status = "Power Not Connected"
	elif not h2o_storage:
		status = "H2O Not Connected"
	else:
		status = "Running"
	
	# Only process if status is Running
	if status != "Running":
		return
		
	# Calculate how much we can produce based on inputs
	var minutes = delta * 60  # Convert seconds to minutes
	
	# Get resources from inputs
	var got_o2 = false
	var got_h2 = false
	var got_power = false
	
	if o2_source and "remove_resource" in o2_source:
		var o2_received = o2_source.remove_resource(o2_input_rate * minutes)
		if o2_received > 0:
			o2_stored += o2_received
			got_o2 = true
	
	if h2_source and "remove_resource" in h2_source:
		var h2_received = h2_source.remove_resource(h2_input_rate * minutes)
		if h2_received > 0:
			h2_stored += h2_received
			got_h2 = true
	
	if power_source and "power_output" in power_source:
		power_available = power_source.power_output * power_source.efficiency
		got_power = power_available >= power_input_rate
	
	# Update status based on resource availability
	if not got_o2:
		status = "No O2 Input"
	elif not got_h2:
		status = "No H2 Input"
	elif not got_power:
		status = "Insufficient Power"
	else:
		status = "Running"
	
	# Only produce if we have enough resources
	if status == "Running" and \
	   o2_stored >= o2_input_rate * minutes and \
	   h2_stored >= h2_input_rate * minutes and \
	   power_available >= power_input_rate:
		
		# Calculate production
		var h2o_produced = h2o_output_rate * efficiency * minutes
		
		# Consume resources
		o2_stored -= o2_input_rate * minutes
		h2_stored -= h2_input_rate * minutes
		
		# Output H2O
		if h2o_storage and "add_resource" in h2o_storage:
			h2o_storage.add_resource(h2o_produced)
