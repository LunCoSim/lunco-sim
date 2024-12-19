extends UISimulationNode

class_name UIBaseFacility

var facility: BaseFacility

func _init():
	# Set up basic GraphNode properties
	mouse_filter = MOUSE_FILTER_PASS
	resizable = true

	facility = BaseFacility.new("", "facility")

func _ready():
	super._ready()
	update_status_display()

func update_from_simulation() -> void:
	super.update_from_simulation()
	if simulation_node:
		$Parameters/Status.text = simulation_node.properties.status
		$Parameters/Efficiency.text = "Efficiency: " + str(simulation_node.properties.efficiency)
		
func set_facility_properties(id: String, desc: String, type: String):
	facility.facility_id = id
	facility.description = desc
	facility.facility_type = type
	title = "Facility: " + id

func update_status_display() -> void:
	# Virtual method to be implemented by child classes
	pass

func set_status(new_status: String) -> void:
	facility.status = new_status
	update_status_display()

