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
