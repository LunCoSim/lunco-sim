class_name Pump
extends BaseFacility

# Pump properties
@export var pump_rate: float = 10.0  # units/minute
@export var power_consumption: float = 50.0  # kW
@export var power_available: float = 0.0

func _init():
	facility_type = "pump"
	description = "Pumps resources between storages"

func _physics_process(delta: float) -> void:
	if not is_physics_processing():
		return
		
	# Get connected nodes through the simulation manager
	var simulation = get_parent()
	if not simulation:
		status = "No Simulation"
		return
		
	var source_storage = null
	var target_storage = null
	var power_source = null
	
	# Find our connections
	for connection in simulation.connections:
		if connection["to_node"] == name:
			var source_node = simulation.get_node(NodePath(connection["from_node"]))
			match connection["to_port"]:
				0: source_storage = source_node
				1: power_source = source_node
		elif connection["from_node"] == name and connection["from_port"] == 0:
			target_storage = simulation.get_node(NodePath(connection["to_node"]))
	
	# Check connections and update status
	if not source_storage:
		status = "Source Not Connected"
		return
	elif not target_storage:
		status = "Target Not Connected"
		return
	elif not power_source:
		status = "Power Not Connected"
		return
		
	# Check power availability
	if power_source and "power_output" in power_source:
		power_available = power_source.power_output * power_source.efficiency
		if power_available < power_consumption:
			status = "Insufficient Power"
			return
	
	status = "Running"
	
	# Calculate pumping for this time step
	var minutes = delta * 60  # Convert seconds to minutes
	var amount_to_pump = pump_rate * efficiency * minutes
	
	# Try to remove from source
	if "remove_resource" in source_storage:
		var removed = source_storage.remove_resource(amount_to_pump)
		
		# Add to target if we got any resources
		if removed > 0 and "add_resource" in target_storage:
			target_storage.add_resource(removed) 
