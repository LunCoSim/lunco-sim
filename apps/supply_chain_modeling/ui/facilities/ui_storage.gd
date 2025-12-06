class_name UIStorage
extends UIBaseFacility

@onready var resource_type_selector: OptionButton = $VBoxContainer/ResourceType

func _ready() -> void:
	super()
	_setup_resource_selector()
	update_display()
	
	# Connect the resource type change signal
	resource_type_selector.item_selected.connect(_on_resource_type_selected)

func _setup_resource_selector() -> void:
	resource_type_selector.clear()
	
	# Add empty option
	resource_type_selector.add_item("Select Resource Type", 0)
	
	# Get all available resources from LCResourceRegistry
	var resources = LCResourceRegistry.get_all_resources()
	
	# Add each resource as an option
	var index = 1
	for resource in resources:
		resource_type_selector.add_item(resource.display_name, index)
		# Store the resource ID in the metadata
		resource_type_selector.set_item_metadata(index, resource.resource_id)
		
		# Select current resource if it matches
		if simulation_node and simulation_node is StorageFacility:
			var storage = simulation_node as StorageFacility
			if storage.stored_resource_type == resource.resource_id:
				resource_type_selector.select(index)
		
		index += 1

func _on_resource_type_selected(index: int) -> void:
	if simulation_node and simulation_node is StorageFacility:
		var storage = simulation_node as StorageFacility
		var resource_name = resource_type_selector.get_item_metadata(index) if index > 0 else ""
		storage.set_resource_type(resource_name)
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
