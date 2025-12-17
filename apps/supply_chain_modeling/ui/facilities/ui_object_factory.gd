class_name UIObjectFactory
extends UIBaseFacility


func _process(delta: float) -> void:
	update_status_display()

func update_status_display() -> void:

	if not simulation_node is ElectrolyticFactory:
		return

	var object_factory = simulation_node as ElectrolyticFactory

	# Update display labels
	var status_label = $Parameters/Status
	if status_label:
		status_label.text = "Status: " + object_factory.status
	
	var efficiency_label = $Parameters/Efficiency
	if efficiency_label:
		efficiency_label.text = "Efficiency: " + str(object_factory.efficiency * 100) + "%"
	
	var power_label = $Parameters/PowerConsumption
	if power_label:
		power_label.text = "Power: " + str(object_factory.power_consumption) + " kW"
	
	var o2_label = $Parameters/O2Level
	if o2_label:
		o2_label.text = "O2: %.2f units" % object_factory.o2_stored
	
	var h2_label = $Parameters/H2Level
	if h2_label:
		h2_label.text = "H2: %.2f units" % object_factory.h2_stored
  
