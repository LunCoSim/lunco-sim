extends UISimulationNode

class_name UIBaseResource

var resource: BaseResource

func _init():
	# Set up basic GraphNode properties
	mouse_filter = MOUSE_FILTER_PASS
	resizable = true
	
	# Configure the output slot (right side)
	set_slot_enabled_right(0, true)
	set_slot_type_right(0, 0)
	set_slot_color_right(0, Color.WHITE)

func _ready():
	# Disable left slot (resources are sources)
	set_slot_enabled_left(0, false)
	
	# Set up basic appearance
	size = Vector2(150, 80)  # Default size
	update_display()

func set_resource_properties(id: String, desc: String, type: String):
	if not resource:
		resource = BaseResource.new(id)
	resource.set_properties(desc, type, 0.0, 0.0)
	title = "Resource: " + id

func get_resource_data() -> Dictionary:
	return resource.get_resource_data()

func load_resource_data(data: Dictionary) -> void:
	if not resource:
		resource = BaseResource.new(data.get("id", ""))
	resource.load_resource_data(data)

func remove_resource(amount: float) -> float:
	var removed = resource.remove_resource(amount)
	update_display()
	return removed

func add_resource(amount: float) -> float:
	var added = resource.add_resource(amount)
	update_display()
	return added

func update_display() -> void:
	if not resource:
		return
		
	# Update the progress bar
	var progress = $Properties/ProgressBar
	if progress:
		progress.max_value = resource.max_amount
		progress.value = resource.current_amount
		
	# Update the amount label
	var amount_label = $Properties/Amount
	if amount_label:
		amount_label.text = "Amount: %.2f / %.2f units" % [resource.current_amount, resource.max_amount]
