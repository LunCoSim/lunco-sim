extends GraphNode

var efficiency: float = 0.95  # 95% efficiency
var power_consumption: float = 100.0  # kW
var status: String = "Running"

# Input/output rates
var o2_input_rate: float = 1.0  # units/minute
var h2_input_rate: float = 2.0  # units/minute
var power_input_rate: float = 100.0  # kW
var h2o_output_rate: float = 1.0  # units/minute

func _init():
    mouse_filter = MOUSE_FILTER_PASS
    resizable = true

func _ready():
    update_status_display()

func process_resources(delta: float) -> void:
    if status != "Running":
        return
        
    # Check if we have all required inputs
    # Implementation will depend on how resource connections are tracked
    
    # Calculate production based on inputs and efficiency
    var actual_output = h2o_output_rate * efficiency * delta
    
    # Update connected storage/output nodes
    # Implementation will depend on how connections are handled

func update_status_display() -> void:
    var status_label = $Parameters/Status
    status_label.text = "Status: " + status
    
    var efficiency_label = $Parameters/Efficiency
    efficiency_label.text = "Efficiency: " + str(efficiency * 100) + "%"
    
    var power_label = $Parameters/PowerConsumption
    power_label.text = "Power: " + str(power_consumption) + " kW"

func set_status(new_status: String) -> void:
    status = new_status
    update_status_display() 