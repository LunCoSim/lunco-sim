class_name UIStorage
extends UIBaseFacility


func _process(delta: float) -> void:	
	update_status_display()

func update_status_display() -> void:
	if not simulation_node is StorageFacility:
		return

	var storage = simulation_node as StorageFacility

	var capacity_label = $VBoxContainer/Label
	if capacity_label:
		capacity_label.text = "Capacity: " + str(storage.capacity)
	
	var progress_bar = $VBoxContainer/ProgressBar
	if progress_bar:
		progress_bar.max_value = storage.capacity
		progress_bar.value = storage.current_amount
