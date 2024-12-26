class_name UIStorage
extends UIBaseFacility

func _ready() -> void:
	super()
	update_display()

func update_display() -> void:
	if not simulation_node is StorageFacility:
		return

	var storage = simulation_node as StorageFacility
	
	# Update progress bar
	var progress_bar = $VBoxContainer/ProgressBar
	if progress_bar:
		progress_bar.max_value = storage.capacity
			progress_bar.value = storage.current_amount
			progress_bar.modulate = storage.get_resource_color()
	
	# Update capacity label
	var label = $VBoxContainer/Label
	if label:
		label.text = "Capacity: %.1f %s" % [
			storage.capacity,
			storage.get_resource_unit()
		]
