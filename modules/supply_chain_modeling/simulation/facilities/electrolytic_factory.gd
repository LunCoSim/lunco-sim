class_name ElectrolyticFactory
extends BaseFacility

# Input/output rates
@export var h2o_input_rate: float = 2.0  # units/minute
@export var power_input_rate: float = 100.0  # kW
@export var h2_output_rate: float = 2.0  # units/minute
@export var o2_output_rate: float = 1.0  # units/minute
@export var power_consumption: float = 100.0  # kW

# Current resource amounts
@export var h2o_stored: float = 0.0
@export var power_available: float = 0.0

func _init():
    facility_type = "producer"
    description = "Breaks down H2O into H2 and O2 through electrolysis"

func _physics_process(delta: float) -> void:
    if not is_physics_processing():
        return
        
    # Get connected nodes through the simulation manager
    var simulation = get_parent()
    if not simulation:
        status = "No Simulation"
        return
        
    var h2o_source = null
    var power_source = null
    var h2_storage = null
    var o2_storage = null
    
    # Find our connections
    for connection in simulation.connections:
        if connection["to_node"] == name:
            var source_node = simulation.get_node(NodePath(connection["from_node"]))
            match connection["to_port"]:
                0: h2o_source = source_node
                1: power_source = source_node
        elif connection["from_node"] == name:
            var target_node = simulation.get_node(NodePath(connection["to_node"]))
            match connection["from_port"]:
                0: h2_storage = target_node
                1: o2_storage = target_node
    
    # Check connections and update status
    if not h2o_source:
        status = "H2O Not Connected"
        return
    elif not power_source:
        status = "Power Not Connected"
        return
    elif not h2_storage:
        status = "H2 Not Connected"
        return
    elif not o2_storage:
        status = "O2 Not Connected"
        return
    
    # Calculate time step
    var minutes = delta * 60  # Convert seconds to minutes
    
    # Check power availability
    power_available = power_source.power_output * power_source.efficiency if "power_output" in power_source else 0.0
    if power_available < power_input_rate:
        status = "Insufficient Power"
        return
        
    # Calculate maximum possible production based on available output space
    var max_h2_production = h2_output_rate * efficiency * minutes
    var max_o2_production = o2_output_rate * efficiency * minutes
    
    var h2_space = h2_storage.available_space() if "available_space" in h2_storage else 0.0
    var o2_space = o2_storage.available_space() if "available_space" in o2_storage else 0.0
    
    max_h2_production = min(max_h2_production, h2_space)
    max_o2_production = min(max_o2_production, o2_space)
    
    if max_h2_production <= 0 or max_o2_production <= 0:
        status = "Output Storage Full"
        return
        
    # Calculate required H2O input for the possible production
    var h2o_required = (h2o_input_rate * minutes) * (max_h2_production / (h2_output_rate * efficiency * minutes))
    
    # Check resource availability
    if not "remove_resource" in h2o_source:
        status = "Invalid Input Connection"
        return
        
    # Try to get H2O
    var h2o_available = h2o_source.remove_resource(h2o_required)
    
    # If we can't get enough H2O, return what we took
    if h2o_available < h2o_required:
        if h2o_available > 0:
            h2o_source.add_resource(h2o_available)
        status = "Insufficient H2O"
        return
        
    # Produce outputs
    var h2_produced = max_h2_production
    var o2_produced = max_o2_production
    
    var h2_added = h2_storage.add_resource(h2_produced)
    var o2_added = o2_storage.add_resource(o2_produced)
    
    # If we couldn't add all output, return proportional input
    if h2_added < h2_produced or o2_added < o2_produced:
        var return_ratio = min(
            (h2_produced - h2_added) / h2_produced,
            (o2_produced - o2_added) / o2_produced
        )
        h2o_source.add_resource(h2o_available * return_ratio)
    
    status = "Running" 