extends BaseFacility

# Input/output rates
var o2_input_rate: float = 1.0  # units/minute
var h2_input_rate: float = 2.0  # units/minute
var power_input_rate: float = 100.0  # kW
var h2o_output_rate: float = 1.0  # units/minute
var power_consumption: float = 100.0  # kW

func _init():
    super._init()
    set_facility_properties("Factory", "Water production facility", "producer")
    efficiency = 0.95

func process_resources(delta: float) -> void:
    if status != "Running":
        return
        
    # Calculate production based on inputs and efficiency
    var actual_output = h2o_output_rate * efficiency * delta
    
    # Implementation will depend on how resource connections are handled

func update_status_display() -> void:
    var status_label = $Parameters/Status
    if status_label:
        status_label.text = "Status: " + status
    
    var efficiency_label = $Parameters/Efficiency
    if efficiency_label:
        efficiency_label.text = "Efficiency: " + str(efficiency * 100) + "%"
    
    var power_label = $Parameters/PowerConsumption
    if power_label:
        power_label.text = "Power: " + str(power_consumption) + " kW"
  