class_name UIElectrolyticFactory
extends UIBaseFacility

func _ready():
    # Set up input and output ports
    set_slot(0, true, 0, Color.BLUE, false, 0, Color.BLUE)  # H2O input
    set_slot(1, true, 0, Color.YELLOW, false, 0, Color.YELLOW)  # Power input
    set_slot(2, false, 0, Color.WHITE, true, 0, Color.WHITE)  # H2 output
    set_slot(3, false, 0, Color.WHITE, true, 0, Color.WHITE)  # O2 output

func _process(delta: float) -> void:
    update_status_display()

func update_status_display() -> void:
    if not simulation_node is ElectrolyticFactory:
        return

    var factory = simulation_node as ElectrolyticFactory

    # Update display labels
    var status_label = $Parameters/Status
    if status_label:
        status_label.text = "Status: " + factory.status
    
    var efficiency_label = $Parameters/Efficiency
    if efficiency_label:
        efficiency_label.text = "Efficiency: " + str(factory.efficiency * 100) + "%"
    
    var power_label = $Parameters/PowerConsumption
    if power_label:
        power_label.text = "Power: " + str(factory.power_consumption) + " kW"
    
    var h2o_label = $Parameters/H2OLevel
    if h2o_label:
        h2o_label.text = "H2O: %.2f units" % factory.h2o_stored 